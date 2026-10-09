//! The default `VaultBackend::copy_secret` (used by `copy_field` /
//! `copy_secret`) copies through the core clipboard module: the secret is
//! cleared after `clipboardClearSeconds` from the settings file, but only if
//! the clipboard still holds it, and never with `0`.
//!
//! Needs a real clipboard (a display on Linux), and sets
//! `$KEYSTEAD_DATA_DIR` for the whole test binary:
//! `xvfb-run -a cargo test -p keystead-bridge --test clipboard_copy -- --ignored`

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use keystead_bridge::server::handle_frame;
use keystead_bridge::{
    BridgeError, ClientStore, Dispatcher, LoginSecret, Response, VaultBackend, VaultSummary,
};
use keystead_core::clipboard;
use keystead_core::generator::GeneratorOptions;
use keystead_core::model::ItemSummary;
use keystead_core::totp::TotpCode;
use serde_json::{json, Value};

const PASSWORD: &str = "clipboard-test-secret-1";

/// Unlocked, one login; does not override `copy_secret`.
struct DefaultCopyBackend;

impl VaultBackend for DefaultCopyBackend {
    fn app_version(&self) -> String {
        "test".into()
    }
    fn unlocked_vault(&self) -> Option<VaultSummary> {
        Some(VaultSummary {
            id: "v".into(),
            name: "V".into(),
        })
    }
    fn list_vaults(&self) -> Result<(Vec<VaultSummary>, Option<String>), BridgeError> {
        Ok((self.unlocked_vault().into_iter().collect(), None))
    }
    fn unlock(
        &self,
        _vault_id: Option<&str>,
        _password: &str,
    ) -> Result<VaultSummary, BridgeError> {
        self.unlocked_vault().ok_or(BridgeError::Internal)
    }
    fn lock(&self) {}
    fn focus_app(&self) {}
    fn request_pairing(&self, _request_id: &str, _client_name: &str, _code: &str) {}
    fn logins_for_url(&self, _url: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        Ok(Vec::new())
    }
    fn search(&self, _query: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        Ok(Vec::new())
    }
    fn get_login(&self, item_id: &str) -> Result<LoginSecret, BridgeError> {
        if item_id != "item" {
            return Err(BridgeError::NotFound);
        }
        Ok(LoginSecret {
            id: "item".into(),
            name: "Item".into(),
            username: "me".into(),
            password: PASSWORD.into(),
            totp: None,
            uris: Vec::new(),
        })
    }
    fn get_totp(&self, _item_id: &str) -> Result<TotpCode, BridgeError> {
        Err(BridgeError::NotFound)
    }
    fn generate_password(&self, _options: GeneratorOptions) -> Result<String, BridgeError> {
        Err(BridgeError::Internal)
    }
    fn save_login(
        &self,
        _name: &str,
        _url: &str,
        _username: &str,
        _password: &str,
    ) -> Result<String, BridgeError> {
        Err(BridgeError::Internal)
    }
    fn update_password(&self, _item_id: &str, _password: &str) -> Result<String, BridgeError> {
        Err(BridgeError::Internal)
    }
}

fn clipboard_text() -> Option<String> {
    clipboard::read_text().expect("clipboard readable")
}

#[test]
#[ignore = "needs a display/clipboard; sets KEYSTEAD_DATA_DIR"]
fn default_copy_clears_after_the_configured_delay() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var(keystead_core::paths::DATA_DIR_ENV, dir.path());
    let settings = dir.path().join("settings.json");
    std::fs::write(&settings, r#"{"clipboardClearSeconds": 1}"#).unwrap();

    let mut store = ClientStore::open(dir.path().join("bridge-clients.json")).unwrap();
    let (client, token) = store.add("Test").unwrap();
    let dispatcher = Dispatcher::new(Arc::new(DefaultCopyBackend), store);
    let call = |request: Value| -> Response {
        let mut request = request;
        request["clientId"] = json!(client.id);
        request["token"] = json!(token);
        handle_frame(&dispatcher, &serde_json::to_vec(&request).unwrap())
    };

    // copy_field: on the clipboard, cleared after the delay.
    let r = call(json!({"id": "1", "type": "copy_field", "itemId": "item", "field": "password"}));
    assert_eq!(r, Response::success("1", &json!({"remaining": null})));
    assert_eq!(clipboard_text().as_deref(), Some(PASSWORD));
    thread::sleep(Duration::from_millis(1800));
    assert_ne!(clipboard_text().as_deref(), Some(PASSWORD), "cleared");

    // Something else copied meanwhile is left alone.
    let r = call(json!({"id": "2", "type": "copy_secret", "text": "generated-pw-2"}));
    assert_eq!(r, Response::null("2"));
    assert_eq!(clipboard_text().as_deref(), Some("generated-pw-2"));
    clipboard::copy_text("copied by the user").unwrap();
    thread::sleep(Duration::from_millis(1800));
    assert_eq!(clipboard_text().as_deref(), Some("copied by the user"));

    // 0 = never cleared; a pending secret is still cleared on lock.
    std::fs::write(&settings, r#"{"clipboardClearSeconds": 0}"#).unwrap();
    let r = call(json!({"id": "3", "type": "copy_secret", "text": "keep-me-3"}));
    assert!(r.ok, "{r:?}");
    thread::sleep(Duration::from_millis(1800));
    assert_eq!(clipboard_text().as_deref(), Some("keep-me-3"));

    std::fs::write(&settings, r#"{"clipboardClearSeconds": 60}"#).unwrap();
    let r = call(json!({"id": "4", "type": "copy_secret", "text": "until-lock-4"}));
    assert!(r.ok, "{r:?}");
    assert!(
        clipboard::clear_pending_secret().unwrap(),
        "the app's lock clears it"
    );
    assert_ne!(clipboard_text().as_deref(), Some("until-lock-4"));
}
