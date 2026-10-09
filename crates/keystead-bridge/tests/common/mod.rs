//! Shared test helpers: an in-memory `VaultBackend`.
#![allow(dead_code)] // not every test binary uses every helper

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use keystead_bridge::server::{handle_frame, BridgeHandler};
use keystead_bridge::{BridgeError, LoginSecret, PairingRequest, Response, VaultBackend};
use keystead_core::generator::{self, GeneratorOptions};
use keystead_core::matching;
use keystead_core::model::{ItemSummary, ItemType, LoginUri, UriMatch, VaultItem};
use keystead_core::totp::{self, TotpCode};

pub const PASSWORD: &str = "correct horse battery staple";
pub const VAULT_NAME: &str = "Privat";
pub const TOTP_SEED: &str = "JBSWY3DPEHPK3PXP";
pub const GITHUB_ID: &str = "item-github";
pub const EXAMPLE_ID: &str = "item-example";
pub const NOTE_ID: &str = "item-note";

/// In-memory backend with three items; records calls.
pub struct FakeBackend {
    unlocked: Mutex<bool>,
    items: Mutex<Vec<VaultItem>>,
    pairings: Mutex<VecDeque<PairingRequest>>,
    pairing_cv: Condvar,
    pub unlock_calls: AtomicUsize,
    pub activity: AtomicUsize,
    pub focus_calls: AtomicUsize,
    pub lock_calls: AtomicUsize,
}

fn login(id: &str, name: &str, username: &str, password: &str, uri: &str, totp: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Login, name);
    item.id = id.to_owned();
    if let Some(l) = item.login.as_mut() {
        l.username = username.to_owned();
        l.password = password.to_owned();
        l.uris.push(LoginUri {
            uri: uri.to_owned(),
            match_type: UriMatch::Domain,
        });
        l.totp = totp.to_owned();
    }
    item
}

impl FakeBackend {
    pub fn new() -> FakeBackend {
        let mut note = VaultItem::new(ItemType::Note, "Notiz");
        note.id = NOTE_ID.to_owned();
        FakeBackend {
            unlocked: Mutex::new(false),
            items: Mutex::new(vec![
                login(
                    GITHUB_ID,
                    "GitHub",
                    "octocat",
                    "gh-secret",
                    "https://github.com",
                    TOTP_SEED,
                ),
                login(
                    EXAMPLE_ID,
                    "Example",
                    "me@example.com",
                    "ex-secret",
                    "https://example.com/login",
                    "",
                ),
                note,
            ]),
            pairings: Mutex::new(VecDeque::new()),
            pairing_cv: Condvar::new(),
            unlock_calls: AtomicUsize::new(0),
            activity: AtomicUsize::new(0),
            focus_calls: AtomicUsize::new(0),
            lock_calls: AtomicUsize::new(0),
        }
    }

    pub fn set_unlocked(&self, unlocked: bool) {
        *self.unlocked.lock().unwrap() = unlocked;
    }

    /// Waits for the next `request_pairing` call.
    pub fn next_pairing(&self, timeout: Duration) -> Option<PairingRequest> {
        let deadline = Instant::now() + timeout;
        let mut queue = self.pairings.lock().unwrap();
        loop {
            if let Some(req) = queue.pop_front() {
                return Some(req);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            queue = self.pairing_cv.wait_timeout(queue, left).unwrap().0;
        }
    }

    fn find_login(&self, item_id: &str) -> Result<VaultItem, BridgeError> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.id == item_id && i.item_type == ItemType::Login)
            .cloned()
            .ok_or(BridgeError::NotFound)
    }
}

impl VaultBackend for FakeBackend {
    fn app_version(&self) -> String {
        "2.0.0-test".to_owned()
    }

    fn unlocked_vault_name(&self) -> Option<String> {
        (*self.unlocked.lock().unwrap()).then(|| VAULT_NAME.to_owned())
    }

    fn unlock(&self, password: &str) -> Result<String, BridgeError> {
        self.unlock_calls.fetch_add(1, Ordering::SeqCst);
        if password == PASSWORD {
            self.set_unlocked(true);
            Ok(VAULT_NAME.to_owned())
        } else {
            Err(BridgeError::WrongPassword)
        }
    }

    fn lock(&self) {
        self.lock_calls.fetch_add(1, Ordering::SeqCst);
        self.set_unlocked(false);
    }

    fn focus_app(&self) {
        self.focus_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn request_pairing(&self, request_id: &str, client_name: &str, code: &str) {
        self.pairings.lock().unwrap().push_back(PairingRequest {
            request_id: request_id.to_owned(),
            client_name: client_name.to_owned(),
            code: code.to_owned(),
        });
        self.pairing_cv.notify_all();
    }

    fn logins_for_url(&self, url: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|i| {
                i.login
                    .as_ref()
                    .is_some_and(|l| l.uris.iter().any(|u| matching::uri_matches(u, url)))
            })
            .map(VaultItem::summary)
            .collect())
    }

    fn search(&self, query: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        if query == "many" {
            return Ok((0..60)
                .map(|i| {
                    let mut item = VaultItem::new(ItemType::Note, format!("Note {i}"));
                    item.id = format!("many-{i}");
                    item.summary()
                })
                .collect());
        }
        let q = query.to_lowercase();
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.name.to_lowercase().contains(&q))
            .map(VaultItem::summary)
            .collect())
    }

    fn get_login(&self, item_id: &str) -> Result<LoginSecret, BridgeError> {
        let item = self.find_login(item_id)?;
        let login = item.login.clone().unwrap_or_default();
        let totp = if login.totp.is_empty() {
            None
        } else {
            Some(totp::totp_now(&login.totp)?)
        };
        Ok(LoginSecret {
            id: item.id,
            name: item.name,
            username: login.username,
            password: login.password,
            totp,
            uris: login.uris.into_iter().map(|u| u.uri).collect(),
        })
    }

    fn get_totp(&self, item_id: &str) -> Result<TotpCode, BridgeError> {
        let item = self.find_login(item_id)?;
        let seed = item.login.map(|l| l.totp).unwrap_or_default();
        if seed.is_empty() {
            return Err(BridgeError::NotFound);
        }
        Ok(totp::totp_now(&seed)?)
    }

    fn generate_password(&self, options: GeneratorOptions) -> Result<String, BridgeError> {
        Ok(generator::generate(&options)?)
    }

    fn save_login(
        &self,
        name: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<String, BridgeError> {
        let mut items = self.items.lock().unwrap();
        let id = format!("new-{}", items.len());
        items.push(login(&id, name, username, password, url, ""));
        Ok(id)
    }

    fn update_password(&self, item_id: &str, password: &str) -> Result<String, BridgeError> {
        let mut items = self.items.lock().unwrap();
        let item = items
            .iter_mut()
            .find(|i| i.id == item_id && i.item_type == ItemType::Login)
            .ok_or(BridgeError::NotFound)?;
        if let Some(l) = item.login.as_mut() {
            l.password = password.to_owned();
        }
        Ok(item.id.clone())
    }

    fn on_activity(&self) {
        self.activity.fetch_add(1, Ordering::SeqCst);
    }
}

/// Sends a raw JSON request through the frame parser and the handler.
pub fn call(handler: &dyn BridgeHandler, request: Value) -> Response {
    handle_frame(handler, &serde_json::to_vec(&request).unwrap())
}

/// Adds `clientId`/`token` to a JSON request.
pub fn with_creds(mut request: Value, client_id: &str, token: &str) -> Value {
    request["clientId"] = Value::String(client_id.to_owned());
    request["token"] = Value::String(token.to_owned());
    request
}
