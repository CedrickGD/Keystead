//! Browser bridge integration: the [`VaultBackend`] the bridge dispatcher
//! calls (working on the shared [`Core`] state) and starting/stopping the
//! local socket server.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use keystead_bridge::protocol::CopyField;
use keystead_bridge::{
    register, start_server, BridgeError, ClientStore, Dispatcher, ExtensionInfo, LoginSecret,
    PairedClient, PairingRequest, VaultBackend, VaultSummary,
};
use keystead_core::generator::{self, GeneratorOptions};
use keystead_core::model::{ItemSummary, ItemType, LoginUri, UriMatch, VaultItem};
use keystead_core::totp::{self, TotpCode};
use keystead_core::{matching, Error as CoreError, UnlockedVault};

use crate::error::{AppError, AppResult};
use crate::extension;
use crate::state::{log, Core, LockReason};

/// Debug builds only: auto-approve pairing requests (automated E2E tests).
#[cfg(debug_assertions)]
const AUTO_APPROVE_ENV: &str = "KEYSTEAD_TEST_AUTO_APPROVE_PAIRING";

/// The app side of the bridge. Holds a weak reference so the dispatcher
/// (owned by the core) does not keep the core alive.
struct Backend {
    core: Weak<Core>,
}

impl Backend {
    fn core(&self) -> Result<Arc<Core>, BridgeError> {
        self.core.upgrade().ok_or(BridgeError::Internal)
    }
}

fn summary_of(vault: &UnlockedVault) -> VaultSummary {
    VaultSummary {
        id: vault.id().to_owned(),
        name: vault.name().to_owned(),
    }
}

/// Picks the vault the bridge unlocks when the extension names none: the
/// last used one if it still exists, otherwise the only vault.
fn bridge_vault_id(core: &Core) -> AppResult<String> {
    let (store, last) = {
        let st = core.state();
        (st.store.clone(), st.settings.last_vault_id.clone())
    };
    let vaults = store.list_vaults()?;
    if let Some(last) = last.filter(|id| vaults.iter().any(|v| &v.id == id)) {
        return Ok(last);
    }
    match vaults.as_slice() {
        [only] => Ok(only.id.clone()),
        _ => Err(AppError::Core(CoreError::NotFound("vault".into()))),
    }
}

/// A clipboard result for the bridge: `locked` stays `locked` (the vault
/// closed or changed while the copy was under way), anything else is
/// logged and `internal`.
fn copy_result(result: AppResult<()>) -> Result<(), BridgeError> {
    result.map_err(|e| match e {
        AppError::Locked => BridgeError::Locked,
        other => {
            log(format_args!(
                "could not copy to the clipboard: {}",
                other.code()
            ));
            BridgeError::Internal
        }
    })
}

/// A display name for a login saved from the browser without a name.
fn name_from_url(url: &str) -> String {
    let url = url.trim();
    let host = tauri::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .or_else(|| {
            tauri::Url::parse(&format!("https://{url}"))
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
        });
    match host {
        Some(host) => {
            let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
            matching::registrable_domain(&host).unwrap_or(host)
        }
        None => url.to_owned(),
    }
}

impl VaultBackend for Backend {
    fn app_version(&self) -> String {
        match self.core.upgrade() {
            Some(core) => core.app_version().to_owned(),
            None => env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// The extension folder the app maintains (`crate::extension`) – only
    /// once it holds the embedded version: the extension reloads itself when
    /// it sees a newer one, which must not happen while the startup thread
    /// is still writing the folder (or after it failed).
    fn extension_info(&self) -> Option<ExtensionInfo> {
        let core = self.core.upgrade()?;
        let data_dir = core.data_dir();
        let version = extension::deployed_version(&data_dir)?;
        Some(ExtensionInfo {
            version: version.to_owned(),
            dir: extension::extension_dir(&data_dir).display().to_string(),
        })
    }

    fn unlocked_vault(&self) -> Option<VaultSummary> {
        let core = self.core.upgrade()?;
        let st = core.state();
        st.vault.as_ref().map(summary_of)
    }

    fn list_vaults(&self) -> Result<(Vec<VaultSummary>, Option<String>), BridgeError> {
        let core = self.core()?;
        let (store, last) = {
            let st = core.state();
            (st.store.clone(), st.settings.last_vault_id.clone())
        };
        let vaults = store
            .list_vaults()?
            .into_iter()
            .map(|v| VaultSummary {
                id: v.id,
                name: v.name,
            })
            .collect();
        Ok((vaults, last))
    }

    /// Opening another vault than the open one switches: the new vault is
    /// unlocked (key derivation) while the open one stays usable, and only
    /// replaces it on success (`Core::install_vault_since` closes the old one like
    /// a lock). The UI follows via `vault://unlocked`.
    fn unlock(&self, vault_id: Option<&str>, password: &str) -> Result<VaultSummary, BridgeError> {
        let core = self.core()?;
        let vault_id = match vault_id {
            Some(id) => id.to_owned(),
            None => bridge_vault_id(&core)?,
        };
        {
            // Already open: just check the password.
            let st = core.state();
            if let Some(vault) = st.vault.as_ref().filter(|v| v.id() == vault_id) {
                return if vault.verify_master_password(password) {
                    Ok(summary_of(vault))
                } else {
                    Err(BridgeError::WrongPassword)
                };
            }
        }
        let info = core.unlock(&vault_id, password)?;
        core.emit_unlocked(&info);
        Ok(VaultSummary {
            id: info.id,
            name: info.name,
        })
    }

    fn lock(&self) {
        if let Some(core) = self.core.upgrade() {
            core.lock(Some(LockReason::Manual));
        }
    }

    fn focus_app(&self) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        core.show_main_window();
        let locked = core.state().vault.is_none();
        if locked {
            core.emit_unlock_request();
        }
    }

    fn request_pairing(&self, request_id: &str, client_name: &str, code: &str) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        #[cfg(debug_assertions)]
        if std::env::var_os(AUTO_APPROVE_ENV).is_some_and(|v| v == "1") {
            let dispatcher = core.state().dispatcher.clone();
            if let Some(dispatcher) = dispatcher {
                log(format_args!(
                    "{AUTO_APPROVE_ENV}=1: auto-approving the pairing request of \"{client_name}\""
                ));
                let request_id = request_id.to_owned();
                // The dispatcher waits for the answer after this returns.
                std::thread::spawn(move || {
                    dispatcher.respond_pairing(&request_id, true);
                });
                return;
            }
        }
        core.emit_pairing_request(PairingRequest {
            request_id: request_id.to_owned(),
            client_name: client_name.to_owned(),
            code: code.to_owned(),
        });
        core.show_main_window();
    }

    fn pairing_closed(&self, request_id: &str) {
        if let Some(core) = self.core.upgrade() {
            core.emit_pairing_closed(request_id);
        }
    }

    fn logins_for_url(&self, url: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        Ok(self.core()?.read(|v| {
            let mut rows = v.logins_for_url(url);
            crate::icons::attach_icons(v, &mut rows);
            rows
        })?)
    }

    fn search(&self, query: &str) -> Result<Vec<ItemSummary>, BridgeError> {
        Ok(self.core()?.read(|v| {
            let mut rows = v.search(query);
            // The dispatcher keeps the first 50 rows; icons go to the first
            // few of those.
            crate::icons::attach_icons(v, &mut rows);
            rows
        })?)
    }

    fn get_login(&self, item_id: &str) -> Result<LoginSecret, BridgeError> {
        let core = self.core()?;
        let secret = core.read(|v| {
            let item = v
                .item(item_id)
                .filter(|i| i.item_type == ItemType::Login && !i.is_trashed())?;
            let login = item.login.as_ref()?;
            let totp = (!login.totp.trim().is_empty())
                .then(|| totp::totp_now(&login.totp).ok())
                .flatten();
            Some(LoginSecret {
                id: item.id.clone(),
                name: item.name.clone(),
                username: login.username.clone(),
                password: login.password.clone(),
                totp,
                uris: login.uris.iter().map(|u| u.uri.clone()).collect(),
            })
        })?;
        secret.ok_or(BridgeError::NotFound)
    }

    fn get_totp(&self, item_id: &str) -> Result<TotpCode, BridgeError> {
        let core = self.core()?;
        let seed = core.read(|v| {
            v.item(item_id)
                .filter(|i| !i.is_trashed())
                .and_then(|i| i.login.as_ref())
                .map(|l| l.totp.clone())
                .filter(|s| !s.trim().is_empty())
        })?;
        let seed = seed.ok_or(BridgeError::NotFound)?;
        Ok(totp::totp_now(&seed)?)
    }

    fn generate_password(&self, options: GeneratorOptions) -> Result<String, BridgeError> {
        let core = self.core()?;
        let password = generator::generate(&options)?;
        match core.mutate(|v| v.add_generated_password(&password)) {
            Ok(()) | Err(AppError::Locked) => Ok(password),
            Err(e) => Err(e.into()),
        }
    }

    /// A password the user took from the extension's suggestion (generated
    /// with `remember: false`, i.e. `preview_password`, which stores nothing).
    fn remember_generated(&self, password: &str) -> Result<(), BridgeError> {
        Ok(self
            .core()?
            .mutate(|v| v.add_generated_password(password))?)
    }

    fn save_login(
        &self,
        name: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<String, BridgeError> {
        let core = self.core()?;
        let name = if name.trim().is_empty() {
            name_from_url(url)
        } else {
            name.trim().to_owned()
        };
        let mut item = VaultItem::new(ItemType::Login, name);
        if let Some(login) = item.login.as_mut() {
            login.username = username.to_owned();
            login.password = password.to_owned();
            if !url.trim().is_empty() {
                login.uris.push(LoginUri {
                    uri: url.trim().to_owned(),
                    match_type: UriMatch::Domain,
                });
            }
        }
        let saved = core.mutate(|v| v.save_item(item.clone()))?;
        core.emit_changed();
        Ok(saved.id)
    }

    fn update_password(&self, item_id: &str, password: &str) -> Result<String, BridgeError> {
        let core = self.core()?;
        let saved = core.mutate(|v| {
            let mut item = v
                .item(item_id)
                .filter(|i| i.item_type == ItemType::Login && !i.is_trashed())
                .cloned()
                .ok_or_else(|| CoreError::NotFound(format!("item {item_id}")))?;
            if let Some(login) = item.login.as_mut() {
                login.password = password.to_owned();
            }
            v.save_item(item)
        })?;
        core.emit_changed();
        Ok(saved.id)
    }

    /// Same path as the UI's `copy_text` (settings from memory; cleared on
    /// lock and quit also with `clipboardClearSeconds` = 0).
    fn copy_secret(&self, text: &str) -> Result<(), BridgeError> {
        copy_result(self.core()?.copy_to_clipboard(text, true))
    }

    /// Reads the field and the lock epoch under one state lock, then copies
    /// only if no lock or vault switch happened in between
    /// (`Core::copy_to_clipboard_since`; otherwise `locked`).
    fn copy_field(&self, item_id: &str, field: CopyField) -> Result<Option<u32>, BridgeError> {
        let core = self.core()?;
        let (text, remaining, epoch) = {
            let st = core.state();
            let item = st
                .vault()?
                .item(item_id)
                .filter(|i| !i.is_trashed())
                .ok_or(BridgeError::NotFound)?;
            let login = item.login.as_ref().ok_or(BridgeError::NotFound)?;
            let (text, remaining) = match field {
                CopyField::Password => {
                    if item.item_type != ItemType::Login {
                        return Err(BridgeError::NotFound);
                    }
                    (zeroize::Zeroizing::new(login.password.clone()), None)
                }
                CopyField::Totp => {
                    let seed = login.totp.trim();
                    if seed.is_empty() {
                        return Err(BridgeError::NotFound);
                    }
                    let code = totp::totp_now(seed)?;
                    let digits: String = code.code.chars().filter(|c| !c.is_whitespace()).collect();
                    (zeroize::Zeroizing::new(digits), Some(code.remaining))
                }
            };
            (text, remaining, core.lock_epoch())
        };
        copy_result(core.copy_to_clipboard_since(&text, true, Some(epoch)))?;
        Ok(remaining)
    }

    fn on_activity(&self) {
        if let Some(core) = self.core.upgrade() {
            core.touch_activity();
        }
    }
}

/// Starts the bridge server (no-op if it runs). Fails with
/// `io:bridge_already_running` if another Keystead instance serves the
/// endpoint.
pub fn start(core: &Arc<Core>) -> AppResult<()> {
    if is_running(core) {
        return Ok(());
    }
    // Drop a stale (stopped) handle first.
    stop(core);
    let clients = ClientStore::open_default()?;
    let backend: Arc<dyn VaultBackend> = Arc::new(Backend {
        core: Arc::downgrade(core),
    });
    let dispatcher = Arc::new(Dispatcher::new(backend, clients));
    let handle = start_server(dispatcher.clone())?;
    let mut st = core.state();
    st.bridge = Some(handle);
    st.dispatcher = Some(dispatcher);
    Ok(())
}

/// True while this instance serves the bridge endpoint.
pub fn is_running(core: &Core) -> bool {
    core.state().bridge.as_ref().is_some_and(|b| b.is_running())
}

/// Stops the bridge server (no-op if it is not running). Pending pairing
/// requests are denied by the dispatcher when their connection closes.
pub fn stop(core: &Core) {
    let (handle, dispatcher) = {
        let mut st = core.state();
        (st.bridge.take(), st.dispatcher.take())
    };
    if let Some(mut handle) = handle {
        handle.stop();
    }
    drop(dispatcher);
}

/// Starts the bridge if browser integration is enabled, logging failures
/// (a second instance or a broken socket must not keep the app from
/// starting).
pub fn start_if_enabled(core: &Arc<Core>) {
    if !core.state().settings.browser_integration {
        return;
    }
    if let Err(e) = start(core) {
        log(format_args!("browser bridge not started: {}", e.code()));
    }
}

/// Re-registers the native host for all browsers that have a registration
/// pointing elsewhere (e.g. the portable exe was moved).
pub fn reregister_if_needed() {
    let Some(exe) = current_exe() else {
        return;
    };
    if !register::needs_reregister(&exe) {
        return;
    }
    let browsers = register::registered_browsers();
    if browsers.is_empty() {
        return;
    }
    match register::register(&browsers, &exe) {
        Ok(()) => log(format_args!(
            "re-registered the native host for {} browser(s)",
            browsers.len()
        )),
        Err(e) => log(format_args!(
            "could not re-register the native host: {}",
            e.code()
        )),
    }
}

/// The path of the running executable (the native host is Keystead itself).
pub fn current_exe() -> Option<PathBuf> {
    match std::env::current_exe() {
        Ok(exe) => Some(exe),
        Err(e) => {
            log(format_args!("cannot determine the executable path: {e}"));
            None
        }
    }
}

/// Paired clients – from the running dispatcher, else from the file.
pub fn clients(core: &Core) -> AppResult<Vec<PairedClient>> {
    let dispatcher = core.state().dispatcher.clone();
    match dispatcher {
        Some(d) => Ok(d.clients()),
        None => Ok(ClientStore::open_default()?.list()),
    }
}

/// Revokes a paired client; `not_found` if it does not exist.
pub fn revoke(core: &Core, client_id: &str) -> AppResult<()> {
    let dispatcher = core.state().dispatcher.clone();
    let removed = match dispatcher {
        Some(d) => d.revoke(client_id)?,
        None => ClientStore::open_default()?.revoke(client_id)?,
    };
    if removed {
        Ok(())
    } else {
        Err(AppError::Core(CoreError::NotFound(format!(
            "client {client_id}"
        ))))
    }
}

/// Pairing requests waiting for the user.
pub fn pending_pairings(core: &Core) -> Vec<PairingRequest> {
    core.state()
        .dispatcher
        .clone()
        .map(|d| d.pending_pairings())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use keystead_core::settings::Settings;
    use keystead_core::KdfParams;

    use super::*;
    use crate::state::test_support::core_with_open_vault;
    use crate::state::EVENT_UNLOCKED;

    /// A core with "Test" (master password "master") open and a second,
    /// locked vault "Arbeit" ("work"); returns their ids.
    fn two_vaults(dir: &Path) -> (Arc<Core>, Backend, String, String) {
        let core = core_with_open_vault(dir, Settings::default());
        let test_id = core.read(|v| v.id().to_owned()).unwrap();
        let work_id = core
            .store()
            .create_vault_with_params("Arbeit", "work", KdfParams::insecure_for_tests())
            .unwrap()
            .id()
            .to_owned();
        let backend = Backend {
            core: Arc::downgrade(&core),
        };
        (core, backend, test_id, work_id)
    }

    fn open_id(core: &Core) -> Option<String> {
        core.read(|v| v.id().to_owned()).ok()
    }

    fn unlocked_events(core: &Core) -> Vec<serde_json::Value> {
        core.emitted()
            .into_iter()
            .filter(|(name, _)| name == EVENT_UNLOCKED)
            .map(|(_, payload)| payload)
            .collect()
    }

    fn stored_last_vault(core: &Core) -> Option<String> {
        Settings::load_from(&core.data_dir().join("settings.json")).last_vault_id
    }

    #[test]
    fn failed_switch_keeps_the_open_vault() {
        let dir = tempfile::tempdir().unwrap();
        let (core, backend, test_id, work_id) = two_vaults(dir.path());

        assert_eq!(
            backend.unlock(Some(&work_id), "master"),
            Err(BridgeError::WrongPassword)
        );
        assert_eq!(
            backend.unlock(Some("no-such-vault"), "work"),
            Err(BridgeError::NotFound)
        );
        assert_eq!(open_id(&core).as_deref(), Some(test_id.as_str()));
        assert_eq!(backend.unlocked_vault().unwrap().id, test_id);
        assert!(core.read(|v| v.items().len()).is_ok(), "still usable");
        assert!(unlocked_events(&core).is_empty());
        assert_eq!(core.state().settings.last_vault_id, None);
    }

    #[test]
    fn switch_replaces_the_open_vault_and_remembers_it() {
        let dir = tempfile::tempdir().unwrap();
        let (core, backend, _test_id, work_id) = two_vaults(dir.path());

        let opened = backend.unlock(Some(&work_id), "work").unwrap();
        assert_eq!(
            opened,
            VaultSummary {
                id: work_id.clone(),
                name: "Arbeit".into()
            }
        );
        assert_eq!(open_id(&core).as_deref(), Some(work_id.as_str()));
        // The UI is told to show the new vault; no lock event in between.
        let events = unlocked_events(&core);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["id"], work_id.as_str());
        assert_eq!(events[0]["name"], "Arbeit");
        assert!(core
            .emitted()
            .iter()
            .all(|(name, _)| name == EVENT_UNLOCKED));
        // lastVaultId follows, in memory and on disk (the app's unlock
        // screen preselects it after the next lock).
        assert_eq!(
            core.state().settings.last_vault_id.as_deref(),
            Some(work_id.as_str())
        );
        assert_eq!(stored_last_vault(&core).as_deref(), Some(work_id.as_str()));

        // Unlocking the open vault again only checks the password.
        assert_eq!(backend.unlock(Some(&work_id), "work").unwrap().id, work_id);
        assert_eq!(
            backend.unlock(Some(&work_id), "master"),
            Err(BridgeError::WrongPassword)
        );
        assert_eq!(open_id(&core).as_deref(), Some(work_id.as_str()));
        assert_eq!(unlocked_events(&core).len(), 1);
    }

    fn code<T>(result: AppResult<T>) -> Result<(), String> {
        result.map(|_| ()).map_err(|e| e.code())
    }

    /// The extension replaced the page's vault; the page has not reloaded yet
    /// (the `vault://unlocked` event is still on its way) and sends an edit
    /// meant for the old vault. It must not land in the new one.
    #[test]
    fn page_commands_for_a_replaced_vault_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (core, backend, test_id, work_id) = two_vaults(dir.path());
        let mut item = VaultItem::new(ItemType::Login, "Alt");
        item.login.as_mut().unwrap().password = "old-vault-secret".into();
        let saved = core
            .mutate_for_page(Some(&test_id), |v| v.save_item(item.clone()))
            .unwrap();

        backend.unlock(Some(&work_id), "work").unwrap();

        // Saving under an id "Arbeit" does not know would create the item there.
        let save = core.mutate_for_page(Some(&test_id), |v| v.save_item(saved.clone()));
        assert_eq!(code(save), Err("locked".to_owned()));
        let rename = core.mutate_for_page(Some(&test_id), |v| v.rename("Umbenannt"));
        assert_eq!(code(rename), Err("locked".to_owned()));
        assert_eq!(
            code(core.state().page_vault(Some(&test_id))),
            Err("locked".to_owned()),
            "export_data"
        );
        // A page that names no vault never changes one.
        let unnamed = core.mutate_for_page(None, |v| v.empty_trash());
        assert_eq!(code(unnamed), Err("locked".to_owned()));
        let (items, name) = core
            .read(|v| (v.items().len(), v.name().to_owned()))
            .unwrap();
        assert_eq!((items, name.as_str()), (0, "Arbeit"), "new vault untouched");

        // The reloaded page works on the new vault; the bridge's own writes
        // (for the vault it just opened) are not affected.
        core.mutate_for_page(Some(&work_id), |v| v.save_item(item.clone()))
            .unwrap();
        backend
            .save_login("", "https://example.org/login", "bob", "pw")
            .unwrap();
        assert_eq!(core.read(|v| v.items().len()).unwrap(), 2);

        core.lock(None);
        let locked = core.mutate_for_page(Some(&work_id), |v| v.empty_trash());
        assert_eq!(code(locked), Err("locked".to_owned()));
    }

    #[test]
    fn list_vaults_and_unlock_without_an_id() {
        let dir = tempfile::tempdir().unwrap();
        let (core, backend, test_id, work_id) = two_vaults(dir.path());

        let (vaults, last) = backend.list_vaults().unwrap();
        let mut names: Vec<_> = vaults.iter().map(|v| v.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Arbeit", "Test"]);
        assert_eq!(last, None);
        // Two vaults, none used yet: the bridge cannot pick one.
        core.lock(None);
        assert_eq!(backend.unlock(None, "master"), Err(BridgeError::NotFound));

        // Without an id the last used vault opens.
        assert_eq!(
            backend.unlock(Some(&test_id), "master").unwrap().id,
            test_id
        );
        assert_eq!(
            backend.list_vaults().unwrap().1.as_deref(),
            Some(test_id.as_str())
        );
        core.lock(None);
        assert_eq!(backend.unlock(None, "master").unwrap().id, test_id);
        assert_eq!(backend.unlock(Some(&work_id), "work").unwrap().id, work_id);
        assert_eq!(
            backend.list_vaults().unwrap().1.as_deref(),
            Some(work_id.as_str())
        );
    }

    /// `xvfb-run cargo test -p keystead-desktop -- --ignored clipboard --test-threads=1`
    /// (the clipboard tests share the system clipboard).
    #[test]
    #[ignore = "needs a clipboard (X11 display or Windows desktop)"]
    fn clipboard_secret_of_the_old_vault_is_cleared_on_switch() {
        let dir = tempfile::tempdir().unwrap();
        let (core, backend, _test_id, work_id) = two_vaults(dir.path());
        core.copy_to_clipboard("copied-from-test", true).unwrap();
        // A failed switch leaves it alone.
        assert!(backend.unlock(Some(&work_id), "wrong").is_err());
        assert_eq!(
            keystead_core::clipboard::read_text().unwrap().as_deref(),
            Some("copied-from-test")
        );
        backend.unlock(Some(&work_id), "work").unwrap();
        assert_ne!(
            keystead_core::clipboard::read_text().unwrap().as_deref(),
            Some("copied-from-test")
        );
    }

    /// The extension's password suggestion: the preview stays out of the
    /// generator history until the user takes it (`remember_generated`).
    #[test]
    fn suggested_passwords_enter_the_history_only_when_taken() {
        let dir = tempfile::tempdir().unwrap();
        let (core, backend, _test_id, _work_id) = two_vaults(dir.path());
        let history = |core: &Core| {
            core.read(|v| {
                v.generator_history()
                    .iter()
                    .map(|g| g.password.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap()
        };

        let preview = backend
            .preview_password(GeneratorOptions::default())
            .unwrap();
        assert_eq!(preview.chars().count(), 20);
        assert!(history(&core).is_empty());

        backend.remember_generated(&preview).unwrap();
        assert_eq!(history(&core), vec![preview.clone()]);
        let generated = backend
            .generate_password(GeneratorOptions::default())
            .unwrap();
        assert_eq!(history(&core), vec![generated, preview]);

        core.lock(None);
        assert_eq!(
            backend.remember_generated("pw"),
            Err(BridgeError::Locked)
        );
    }

    #[test]
    fn names_from_urls() {
        assert_eq!(name_from_url("https://www.github.com/login"), "github.com");
        assert_eq!(
            name_from_url("https://accounts.google.co.uk/x"),
            "google.co.uk"
        );
        assert_eq!(name_from_url("example.org/path"), "example.org");
        assert_eq!(name_from_url("http://localhost:8080"), "localhost");
        assert_eq!(name_from_url(""), "");
    }
}
