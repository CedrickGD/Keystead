//! End to end: browser frames → host relay (`host::run_with`) → real local
//! socket (`$KEYSTEAD_BRIDGE_SOCKET`) → server → dispatcher → a backend on a
//! real `keystead-core` vault.
//!
//! This binary contains a single test because it sets process environment
//! variables.

use std::io::{Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use keystead_bridge::framing::{
    read_frame, write_frame, MAX_BROWSER_MESSAGE_SIZE, MAX_MESSAGE_SIZE,
};
use keystead_bridge::host::{self, LocalSocketConnector};
use keystead_bridge::{
    start_server, BridgeError, ClientStore, Dispatcher, LoginSecret, Response, VaultBackend,
    VaultSummary,
};
use keystead_core::generator::{self, GeneratorOptions};
use keystead_core::model::{ItemSummary, ItemType, LoginUri, UriMatch, VaultItem};
use keystead_core::totp::{self, TotpCode};
use keystead_core::{KdfParams, UnlockedVault, VaultStore};
use serde_json::{json, Value};

const MASTER: &str = "Tr0ub4dor&3 – lang genug";

/// What the desktop app does, minus Tauri: holds the unlocked vault.
struct CoreBackend {
    store: VaultStore,
    vault_id: String,
    vault: Mutex<Option<UnlockedVault>>,
    dispatcher: Mutex<Weak<Dispatcher>>,
    approve: AtomicBool,
}

impl CoreBackend {
    fn with_vault<T>(
        &self,
        f: impl FnOnce(&mut UnlockedVault) -> Result<T, BridgeError>,
    ) -> Result<T, BridgeError> {
        let mut guard = self.vault.lock().map_err(|_| BridgeError::Internal)?;
        let vault = guard.as_mut().ok_or(BridgeError::Locked)?;
        f(vault)
    }

    fn login(vault: &UnlockedVault, item_id: &str) -> Result<VaultItem, BridgeError> {
        vault
            .item(item_id)
            .filter(|i| i.item_type == ItemType::Login && !i.is_trashed())
            .cloned()
            .ok_or(BridgeError::NotFound)
    }
}

impl VaultBackend for CoreBackend {
    fn app_version(&self) -> String {
        "2.0.0".into()
    }

    fn unlocked_vault(&self) -> Option<VaultSummary> {
        self.vault.lock().ok()?.as_ref().map(|v| VaultSummary {
            id: v.id().to_owned(),
            name: v.name().to_owned(),
        })
    }

    fn list_vaults(&self) -> Result<(Vec<VaultSummary>, Option<String>), BridgeError> {
        let vaults = self
            .store
            .list_vaults()?
            .into_iter()
            .map(|v| VaultSummary {
                id: v.id,
                name: v.name,
            })
            .collect();
        Ok((vaults, Some(self.vault_id.clone())))
    }

    fn unlock(&self, vault_id: Option<&str>, password: &str) -> Result<VaultSummary, BridgeError> {
        let vault = self
            .store
            .unlock(vault_id.unwrap_or(&self.vault_id), password)?;
        let summary = VaultSummary {
            id: vault.id().to_owned(),
            name: vault.name().to_owned(),
        };
        *self.vault.lock().map_err(|_| BridgeError::Internal)? = Some(vault);
        Ok(summary)
    }

    fn lock(&self) {
        if let Ok(mut v) = self.vault.lock() {
            *v = None;
        }
    }

    fn focus_app(&self) {}

    fn request_pairing(&self, request_id: &str, _client_name: &str, _code: &str) {
        // Plays the user clicking "approve"/"deny" in the app.
        let dispatcher = self.dispatcher.lock().unwrap().clone();
        let approve = self.approve.load(Ordering::SeqCst);
        let request_id = request_id.to_owned();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            if let Some(d) = dispatcher.upgrade() {
                assert!(d.respond_pairing(&request_id, approve));
            }
        });
    }

    fn logins_for_url(&self, url: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        self.with_vault(|v| Ok(v.logins_for_url(url)))
    }

    fn search(&self, query: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        self.with_vault(|v| Ok(v.search(query)))
    }

    fn get_login(&self, item_id: &str) -> Result<LoginSecret, BridgeError> {
        self.with_vault(|v| {
            let item = Self::login(v, item_id)?;
            let login = item.login.unwrap_or_default();
            let totp = match login.totp.trim() {
                "" => None,
                seed => Some(totp::totp_now(seed)?),
            };
            Ok(LoginSecret {
                id: item.id,
                name: item.name,
                username: login.username,
                password: login.password,
                totp,
                uris: login.uris.into_iter().map(|u| u.uri).collect(),
            })
        })
    }

    fn get_totp(&self, item_id: &str) -> Result<TotpCode, BridgeError> {
        self.with_vault(|v| {
            let seed = Self::login(v, item_id)?.login.unwrap_or_default().totp;
            if seed.trim().is_empty() {
                return Err(BridgeError::NotFound);
            }
            Ok(totp::totp_now(&seed)?)
        })
    }

    fn generate_password(&self, options: GeneratorOptions) -> Result<String, BridgeError> {
        let password = generator::generate(&options)?;
        if let Some(v) = self
            .vault
            .lock()
            .map_err(|_| BridgeError::Internal)?
            .as_mut()
        {
            v.add_generated_password(&password)?;
        }
        Ok(password)
    }

    fn save_login(
        &self,
        name: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<String, BridgeError> {
        self.with_vault(|v| {
            let mut item = VaultItem::new(ItemType::Login, name);
            if let Some(l) = item.login.as_mut() {
                l.username = username.into();
                l.password = password.into();
                if !url.is_empty() {
                    l.uris.push(LoginUri {
                        uri: url.into(),
                        match_type: UriMatch::Domain,
                    });
                }
            }
            Ok(v.save_item(item)?.id)
        })
    }

    fn update_password(&self, item_id: &str, password: &str) -> Result<String, BridgeError> {
        self.with_vault(|v| {
            let mut item = Self::login(v, item_id)?;
            if let Some(l) = item.login.as_mut() {
                l.password = password.into();
            }
            Ok(v.save_item(item)?.id)
        })
    }
}

/// The browser side of the native-messaging pipe.
struct Browser<W: Write, R: Read> {
    to_host: W,
    from_host: R,
}

impl<W: Write, R: Read> Browser<W, R> {
    fn send(&mut self, msg: Value) -> Response {
        write_frame(
            &mut self.to_host,
            msg.to_string().as_bytes(),
            MAX_MESSAGE_SIZE,
        )
        .unwrap();
        let frame = read_frame(&mut self.from_host, MAX_BROWSER_MESSAGE_SIZE)
            .unwrap()
            .expect("reply from host");
        let reply: Response = serde_json::from_slice(&frame).unwrap();
        assert_eq!(reply.id, msg["id"], "id preserved");
        reply
    }
}

fn frames(messages: &[Value]) -> Vec<u8> {
    let mut buf = Vec::new();
    for m in messages {
        write_frame(&mut buf, m.to_string().as_bytes(), MAX_MESSAGE_SIZE).unwrap();
    }
    buf
}

fn replies(output: &[u8]) -> Vec<Response> {
    let mut r = Cursor::new(output);
    let mut out = Vec::new();
    while let Some(f) = read_frame(&mut r, MAX_BROWSER_MESSAGE_SIZE).unwrap() {
        out.push(serde_json::from_slice(&f).unwrap());
    }
    out
}

#[test]
fn browser_to_vault_through_host_and_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket_value = if cfg!(windows) {
        format!("keystead-bridge-e2e-{}", std::process::id())
    } else {
        dir.path()
            .join("bridge.sock")
            .to_string_lossy()
            .into_owned()
    };
    // Single-test binary: no other thread reads the environment concurrently.
    std::env::set_var(keystead_bridge::socket::SOCKET_ENV, &socket_value);
    // If the host tries to launch the app, it fails fast.
    std::env::set_var(host::APP_EXE_ENV, dir.path().join("no-such-Keystead"));

    // A real vault with one login (with TOTP).
    let store = VaultStore::new(dir.path().join("data"));
    let mut vault = store
        .create_vault_with_params("Privat", MASTER, KdfParams::insecure_for_tests())
        .unwrap();
    let mut github = VaultItem::new(ItemType::Login, "GitHub");
    if let Some(l) = github.login.as_mut() {
        l.username = "octocat".into();
        l.password = "gh-secret".into();
        l.totp = "JBSWY3DPEHPK3PXP".into();
        l.uris.push(LoginUri {
            uri: "https://github.com".into(),
            match_type: UriMatch::Domain,
        });
    }
    let github_id = vault.save_item(github).unwrap().id;
    let vault_id = vault.id().to_owned();
    drop(vault);

    let backend = Arc::new(CoreBackend {
        store,
        vault_id,
        vault: Mutex::new(None),
        dispatcher: Mutex::new(Weak::new()),
        approve: AtomicBool::new(false),
    });
    let clients = ClientStore::open(dir.path().join("data/bridge-clients.json")).unwrap();
    let dispatcher = Arc::new(Dispatcher::new(backend.clone(), clients));
    *backend.dispatcher.lock().unwrap() = Arc::downgrade(&dispatcher);
    let mut server = start_server(dispatcher.clone()).unwrap();

    // --- Phase A: canned stdin, in-memory stdout --------------------------
    let input = frames(&[
        json!({"id": "a1", "type": "status"}),
        json!({"id": "a2", "type": "focus_app"}),
        json!({"id": "a3", "type": "logins_for_url", "url": "https://github.com"}),
        json!({"id": "a4", "type": "pair", "clientName": "Chrome", "code": "12"}),
        json!({"id": "a5", "type": "teleport"}),
        json!({"id": "a6", "type": "pair", "clientName": "Chrome – Test", "code": "314159"}),
    ]);
    let mut output = Vec::new();
    let code = host::run_with(
        Cursor::new(input),
        &mut output,
        LocalSocketConnector::from_env(),
    );
    assert_eq!(code, 0, "clean exit on EOF");
    let r = replies(&output);
    assert_eq!(r.len(), 6);
    assert_eq!(
        r[0].data,
        json!({"appVersion": "2.0.0", "paired": false, "unlocked": false, "vaultName": null,
            "vaultId": null})
    );
    assert_eq!(r[1], Response::null("a2"));
    assert_eq!(r[2], Response::error("a3", BridgeError::NotPaired));
    assert_eq!(r[3], Response::error("a4", BridgeError::InvalidRequest));
    assert_eq!(r[4], Response::error("a5", BridgeError::InvalidRequest));
    assert_eq!(r[5], Response::error("a6", BridgeError::PairingDenied));

    // --- Phase B: interactive session over OS pipes ------------------------
    backend.approve.store(true, Ordering::SeqCst);
    let (host_in, to_host) = std::io::pipe().unwrap();
    let (from_host, host_out) = std::io::pipe().unwrap();
    let host_thread =
        thread::spawn(move || host::run_with(host_in, host_out, LocalSocketConnector::from_env()));
    let mut browser = Browser { to_host, from_host };

    let r = browser
        .send(json!({"id": "p", "type": "pair", "clientName": "Chrome – Test", "code": "314159"}));
    assert!(r.ok, "{r:?}");
    let cid = r.data["clientId"].as_str().unwrap().to_owned();
    let tok = r.data["token"].as_str().unwrap().to_owned();
    let auth = |mut v: Value| {
        v["clientId"] = json!(cid);
        v["token"] = json!(tok);
        v
    };

    let r = browser.send(auth(
        json!({"id": "b1", "type": "logins_for_url", "url": "https://github.com"}),
    ));
    assert_eq!(r.error_code(), Some(BridgeError::Locked));
    let vault_id = backend.vault_id.clone();
    let r = browser.send(auth(json!({"id": "b1b", "type": "list_vaults"})));
    assert_eq!(
        r.data,
        json!({"vaults": [{"id": vault_id, "name": "Privat"}], "currentVaultId": null,
            "lastVaultId": vault_id})
    );
    let r = browser.send(auth(
        json!({"id": "b1c", "type": "unlock", "password": MASTER, "vaultId": "no-such-vault"}),
    ));
    assert_eq!(r.error_code(), Some(BridgeError::NotFound));
    let r = browser.send(auth(
        json!({"id": "b2", "type": "unlock", "password": "wrong"}),
    ));
    assert_eq!(r.error_code(), Some(BridgeError::WrongPassword));
    let r = browser.send(auth(
        json!({"id": "b3", "type": "unlock", "password": MASTER}),
    ));
    assert_eq!(r.data, json!({"vaultName": "Privat", "vaultId": vault_id}));
    let r = browser.send(auth(json!({"id": "b4", "type": "status"})));
    assert_eq!(
        r.data,
        json!({"appVersion": "2.0.0", "paired": true, "unlocked": true, "vaultName": "Privat",
            "vaultId": vault_id})
    );

    let r = browser.send(auth(
        json!({"id": "b5", "type": "logins_for_url", "url": "https://github.com/login"}),
    ));
    let list = r.data.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], github_id.as_str());
    assert_eq!(list[0]["subtitle"], "octocat");

    let r = browser.send(auth(
        json!({"id": "b6", "type": "get_login", "itemId": github_id}),
    ));
    assert_eq!(r.data["password"], "gh-secret");
    assert_eq!(r.data["totp"]["code"].as_str().unwrap().len(), 6);

    let r = browser.send(auth(
        json!({"id": "b7", "type": "get_totp", "itemId": github_id}),
    ));
    assert_eq!(r.data["period"], 30);

    let r = browser.send(auth(
        json!({"id": "b8", "type": "generate_password", "options": {"length": 24}}),
    ));
    let generated = r.data.as_str().unwrap().to_owned();
    assert_eq!(generated.len(), 24);

    let r = browser.send(auth(
        json!({"id": "b9", "type": "save_login", "name": "Shop",
        "url": "https://shop.example.com", "username": "buyer", "password": generated}),
    ));
    let shop_id = r.data["id"].as_str().unwrap().to_owned();
    let r = browser.send(auth(
        json!({"id": "b10", "type": "update_password", "itemId": shop_id, "password": "neu"}),
    ));
    assert_eq!(r.data, json!({"id": shop_id}));
    let r = browser.send(auth(
        json!({"id": "b11", "type": "search", "query": "shop"}),
    ));
    assert_eq!(r.data[0]["id"], shop_id.as_str());

    // Persisted in the real vault file (incl. generator history).
    let reopened = backend.store.unlock(&backend.vault_id, MASTER).unwrap();
    assert_eq!(reopened.item(&shop_id).unwrap().password(), "neu");
    assert_eq!(reopened.generator_history()[0].password, generated);

    let r = browser.send(auth(json!({"id": "b12", "type": "lock"})));
    assert_eq!(r, Response::null("b12"));
    let r = browser.send(auth(
        json!({"id": "b13", "type": "get_login", "itemId": github_id}),
    ));
    assert_eq!(r.error_code(), Some(BridgeError::Locked));

    // App restart (e.g. browser integration toggled): the host reconnects
    // transparently on its next request.
    server.stop();
    let mut server = start_server(dispatcher.clone()).unwrap();
    let r = browser.send(auth(json!({"id": "c1", "type": "status"})));
    assert_eq!(r.data["paired"], true);

    // App gone and cannot be launched: app_unavailable, id preserved.
    server.stop();
    let r = browser.send(auth(json!({"id": "c2", "type": "status"})));
    assert_eq!(r, Response::error("c2", BridgeError::AppUnavailable));

    // Browser closes the port: host exits cleanly.
    drop(browser.to_host);
    assert_eq!(host_thread.join().unwrap(), 0);
}
