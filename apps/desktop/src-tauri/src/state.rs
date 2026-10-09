//! Shared backend state: the vault store, the unlocked vault, settings, the
//! auto-lock clock and the browser bridge. Used by the Tauri commands, the
//! bridge backend, the tray and the monitor thread.
//!
//! Locking rule: `Core::state` is never held while calling into the window
//! system or emitting events (those may need the main thread, which itself
//! may be waiting for the state), nor across a blocking pairing wait.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use keystead_bridge::{Dispatcher, ServerHandle};
use keystead_core::model::VaultInfo;
use keystead_core::settings::Settings;
use keystead_core::{clipboard, Error as CoreError, UnlockedVault, VaultStore};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::monitor::MonitorSignal;
use crate::tray::TrayMenu;
use crate::update::UpdateInfo;

/// `vault://locked` – payload `{ reason }`.
pub const EVENT_LOCKED: &str = "vault://locked";
/// `vault://changed` – payload `{}`.
pub const EVENT_CHANGED: &str = "vault://changed";
/// `vault://unlocked` – payload `VaultInfo` (unlocked outside the UI).
pub const EVENT_UNLOCKED: &str = "vault://unlocked";
/// `bridge://pairing-request` – payload `{ requestId, clientName, code }`.
pub const EVENT_PAIRING_REQUEST: &str = "bridge://pairing-request";
/// `bridge://pairing-closed` – payload `{ requestId }`: a pairing request
/// ended without the user's decision (cancelled in the browser, replaced by a
/// newer one, timed out); its dialog can be closed.
pub const EVENT_PAIRING_CLOSED: &str = "bridge://pairing-closed";
/// `bridge://unlock-request` – payload `{}`.
pub const EVENT_UNLOCK_REQUEST: &str = "bridge://unlock-request";

/// Label of the main window.
pub const MAIN_WINDOW: &str = "main";

/// Why the vault was locked (payload of `vault://locked`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockReason {
    Manual,
    Timeout,
    System,
}

#[derive(Debug, Clone, Serialize)]
struct LockedPayload {
    reason: LockReason,
}

#[derive(Debug, Clone, Serialize)]
struct EmptyPayload {}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingClosedPayload {
    request_id: String,
}

/// Writes a diagnostic line to stderr. Never pass secrets. A missing or
/// broken stderr (Windows GUI subsystem) is ignored.
pub fn log(message: impl std::fmt::Display) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "[keystead] {message}");
}

/// Everything guarded by the state mutex.
pub struct AppState {
    pub store: VaultStore,
    pub vault: Option<UnlockedVault>,
    pub settings: Settings,
    /// Last user activity (UI input, user commands, browser actions).
    pub last_activity: Instant,
    /// Wall-clock time of the last activity: unlike `Instant` on Linux it
    /// keeps running while the computer sleeps.
    pub last_activity_wall: SystemTime,
    pub bridge: Option<ServerHandle>,
    pub dispatcher: Option<Arc<Dispatcher>>,
}

impl AppState {
    /// Resets the auto-lock timer.
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
        self.last_activity_wall = SystemTime::now();
    }

    /// Time since the last activity, including time the computer slept.
    pub fn idle_for(&self) -> Duration {
        let monotonic = self.last_activity.elapsed();
        let wall = SystemTime::now()
            .duration_since(self.last_activity_wall)
            .unwrap_or_default();
        monotonic.max(wall)
    }

    pub fn vault(&self) -> AppResult<&UnlockedVault> {
        self.vault.as_ref().ok_or(AppError::Locked)
    }

    pub fn vault_mut(&mut self) -> AppResult<&mut UnlockedVault> {
        self.vault.as_mut().ok_or(AppError::Locked)
    }

    /// The open vault if it is `page_vault_id`, the vault the UI page works
    /// on (see [`Core::mutate_for_page`]); `locked` otherwise.
    pub fn page_vault(&self, page_vault_id: Option<&str>) -> AppResult<&UnlockedVault> {
        self.vault
            .as_ref()
            .filter(|v| Some(v.id()) == page_vault_id)
            .ok_or(AppError::Locked)
    }

    fn page_vault_mut(&mut self, page_vault_id: Option<&str>) -> AppResult<&mut UnlockedVault> {
        self.vault
            .as_mut()
            .filter(|v| Some(v.id()) == page_vault_id)
            .ok_or(AppError::Locked)
    }

    /// Remembers `vault_id` as the last used vault (persisted).
    fn remember_vault(&mut self, vault_id: &str) {
        if self.settings.last_vault_id.as_deref() == Some(vault_id) {
            return;
        }
        self.settings.last_vault_id = Some(vault_id.to_owned());
        self.save_settings();
    }

    /// Persists `settings` (`<data_dir>/settings.json`), logging failures.
    pub fn save_settings(&self) {
        // Unit tests keep it next to their temporary store, never in the
        // real data directory.
        #[cfg(test)]
        let result = self
            .settings
            .save_to(&self.store.root().join("settings.json"));
        #[cfg(not(test))]
        let result = self.settings.save();
        if let Err(e) = result {
            log(format_args!("could not save the settings: {}", e.code()));
        }
    }
}

/// Where events and window requests go.
enum Frontend {
    /// The app's webview (the only variant outside unit tests).
    App(AppHandle),
    /// Unit tests: no window; emitted events are recorded.
    #[cfg(test)]
    Recorder(Mutex<Vec<(String, serde_json::Value)>>),
}

/// What the update checks found (see `crate::update`).
#[derive(Default)]
struct UpdateMemory {
    /// The newest update found on the current channel (`pending_update`,
    /// e.g. for a page that reloaded after a lock).
    pending: Option<UpdateInfo>,
    /// The version last announced with `update://available` (or shown by a
    /// manual check): announced once per version.
    announced: Option<String>,
}

/// The backend shared by all threads (`Arc<Core>` is managed Tauri state).
pub struct Core {
    frontend: Frontend,
    /// The version shown to the user and the browser (`app_info`, bridge
    /// `status`).
    version: String,
    state: Mutex<AppState>,
    /// Mirror of `settings.minimize_to_tray` for the main thread (window
    /// close handling must not wait for the state lock).
    minimize_to_tray: AtomicBool,
    /// A tray icon exists (otherwise hiding the window would strand the app).
    tray_available: AtomicBool,
    /// The app is shutting down.
    exiting: AtomicBool,
    /// The webview (re)loaded: the next `session_state` re-sends pending
    /// pairing requests the fresh page could not have received.
    page_loaded: AtomicBool,
    /// Tray menu entries (labels follow the language setting).
    tray: Mutex<Option<TrayMenu>>,
    /// Wakes the monitor thread (e.g. the Windows session was locked).
    monitor: Mutex<Option<Sender<MonitorSignal>>>,
    /// A secret copied with `clipboardClearSeconds` = 0. The core only
    /// remembers secrets that have a clear timer running, so this one is
    /// cleared here on lock and quit (if the clipboard still holds it).
    untimed_secret: Mutex<Option<Zeroizing<String>>>,
    /// Results of the update checks.
    updates: Mutex<UpdateMemory>,
    /// `install_update` is running.
    update_installing: AtomicBool,
    /// The vault was closed for an update installation
    /// (`lock_for_update`): no vault may be opened until the process ends,
    /// or the installation failed (`resume_after_failed_update`).
    update_locked: AtomicBool,
    /// Wakes the background update check (settings changed).
    update_wake: Mutex<Option<Sender<()>>>,
}

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Poisoning only happens after a panic in another thread; the guarded
    // data stays consistent (single assignments), so keep going.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Core {
    pub fn new(app: AppHandle, store: VaultStore, settings: Settings) -> Arc<Core> {
        let version = crate::update::version_label(&app.package_info().version.to_string());
        Self::with_frontend(Frontend::App(app), version, store, settings)
    }

    fn with_frontend(
        frontend: Frontend,
        version: String,
        store: VaultStore,
        settings: Settings,
    ) -> Arc<Core> {
        let minimize_to_tray = settings.minimize_to_tray;
        Arc::new(Core {
            frontend,
            version,
            state: Mutex::new(AppState {
                store,
                vault: None,
                settings,
                last_activity: Instant::now(),
                last_activity_wall: SystemTime::now(),
                bridge: None,
                dispatcher: None,
            }),
            minimize_to_tray: AtomicBool::new(minimize_to_tray),
            tray_available: AtomicBool::new(false),
            exiting: AtomicBool::new(false),
            page_loaded: AtomicBool::new(false),
            tray: Mutex::new(None),
            monitor: Mutex::new(None),
            untimed_secret: Mutex::new(None),
            updates: Mutex::new(UpdateMemory::default()),
            update_installing: AtomicBool::new(false),
            update_locked: AtomicBool::new(false),
            update_wake: Mutex::new(None),
        })
    }

    /// The app version shown to the user and to the browser extension.
    pub fn app_version(&self) -> &str {
        &self.version
    }

    /// Locks the shared state. See the module docs for the locking rule.
    pub fn state(&self) -> MutexGuard<'_, AppState> {
        lock_mutex(&self.state)
    }

    // -----------------------------------------------------------------
    // Flags
    // -----------------------------------------------------------------

    pub fn minimize_to_tray(&self) -> bool {
        self.minimize_to_tray.load(Ordering::SeqCst)
    }

    pub fn set_minimize_to_tray(&self, value: bool) {
        self.minimize_to_tray.store(value, Ordering::SeqCst);
    }

    pub fn tray_available(&self) -> bool {
        self.tray_available.load(Ordering::SeqCst)
    }

    pub fn set_tray(&self, menu: Option<TrayMenu>) {
        self.tray_available.store(menu.is_some(), Ordering::SeqCst);
        *lock_mutex(&self.tray) = menu;
    }

    /// Runs `f` with the tray menu (if any). Do not hold the state lock.
    pub fn with_tray(&self, f: impl FnOnce(&TrayMenu)) {
        let tray = lock_mutex(&self.tray);
        if let Some(menu) = tray.as_ref() {
            f(menu);
        }
    }

    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::SeqCst)
    }

    pub fn mark_page_loaded(&self) {
        self.page_loaded.store(true, Ordering::SeqCst);
    }

    /// True once after each page load.
    pub fn take_page_loaded(&self) -> bool {
        self.page_loaded.swap(false, Ordering::SeqCst)
    }

    pub fn set_monitor(&self, sender: Sender<MonitorSignal>) {
        *lock_mutex(&self.monitor) = Some(sender);
    }

    /// Sends a signal to the monitor thread (ignored if it is gone).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn signal_monitor(&self, signal: MonitorSignal) {
        if let Some(tx) = lock_mutex(&self.monitor).as_ref() {
            let _ = tx.send(signal);
        }
    }

    // -----------------------------------------------------------------
    // Events & window
    // -----------------------------------------------------------------

    pub(crate) fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
        match &self.frontend {
            Frontend::App(app) => {
                if let Err(e) = app.emit(event, payload) {
                    log(format_args!("could not emit {event}: {e}"));
                }
            }
            #[cfg(test)]
            Frontend::Recorder(events) => {
                let payload = serde_json::to_value(payload).unwrap_or_default();
                lock_mutex(events).push((event.to_owned(), payload));
            }
        }
    }

    pub fn emit_locked(&self, reason: LockReason) {
        self.emit(EVENT_LOCKED, LockedPayload { reason });
    }

    pub fn emit_changed(&self) {
        self.emit(EVENT_CHANGED, EmptyPayload {});
    }

    pub fn emit_unlocked(&self, info: &VaultInfo) {
        self.emit(EVENT_UNLOCKED, info.clone());
    }

    pub fn emit_unlock_request(&self) {
        self.emit(EVENT_UNLOCK_REQUEST, EmptyPayload {});
    }

    pub fn emit_pairing_request<S: Serialize + Clone>(&self, request: S) {
        self.emit(EVENT_PAIRING_REQUEST, request);
    }

    pub fn emit_pairing_closed(&self, request_id: &str) {
        self.emit(
            EVENT_PAIRING_CLOSED,
            PairingClosedPayload {
                request_id: request_id.to_owned(),
            },
        );
    }

    /// Shows, restores and focuses the main window.
    pub fn show_main_window(&self) {
        match &self.frontend {
            Frontend::App(app) => show_main_window(app),
            #[cfg(test)]
            Frontend::Recorder(_) => {}
        }
    }

    // -----------------------------------------------------------------
    // Activity, lock & unlock
    // -----------------------------------------------------------------

    /// Resets the auto-lock timer.
    pub fn touch_activity(&self) {
        self.state().touch();
    }

    /// Locks the vault (drops – and thereby wipes – the decrypted data) and
    /// clears a secret still pending in the clipboard. Emits
    /// `vault://locked` with `reason` if given and a vault was open.
    /// Returns whether a vault was open.
    pub fn lock(&self, reason: Option<LockReason>) -> bool {
        let vault = self.state().vault.take();
        self.finish_lock(vault, reason)
    }

    /// Completes locking for a vault already taken out of the state (call
    /// it without holding the state lock): wipes it, clears a copied secret
    /// still in the clipboard and emits `vault://locked` with `reason` if
    /// given. No-op for `None`. Returns whether a vault was closed.
    pub fn finish_lock(&self, vault: Option<UnlockedVault>, reason: Option<LockReason>) -> bool {
        let was_open = vault.is_some();
        drop(vault);
        if was_open {
            self.clear_copied_secret();
            if let Some(reason) = reason {
                self.emit_locked(reason);
            }
        }
        was_open
    }

    /// Copies `text` to the clipboard. A `sensitive` text is kept out of
    /// clipboard history, cleared after `clipboardClearSeconds` (if the
    /// clipboard still holds it) and in any case on lock and quit.
    pub fn copy_to_clipboard(&self, text: &str, sensitive: bool) -> AppResult<()> {
        let seconds = self.state().settings.clipboard_clear_seconds;
        // Held across the copy so concurrent copies cannot interleave.
        let mut untimed = lock_mutex(&self.untimed_secret);
        if sensitive {
            let clear_after = (seconds > 0).then(|| Duration::from_secs(u64::from(seconds)));
            clipboard::copy_secret(text, clear_after)?;
            *untimed = clear_after
                .is_none()
                .then(|| Zeroizing::new(text.to_owned()));
        } else {
            clipboard::copy_text(text)?;
            *untimed = None;
        }
        Ok(())
    }

    /// Clears a secret copied through the app if the clipboard still holds
    /// it: the one with a pending clear timer (core) and the one copied
    /// without a timer (`clipboardClearSeconds` = 0).
    fn clear_copied_secret(&self) {
        if let Err(e) = clipboard::clear_pending_secret() {
            log(format_args!("could not clear the clipboard: {}", e.code()));
        }
        let untimed = lock_mutex(&self.untimed_secret).take();
        if let Some(secret) = untimed {
            if let Err(e) = clipboard::clear_if_equals(&secret) {
                log(format_args!("could not clear the clipboard: {}", e.code()));
            }
        }
    }

    /// Makes `vault` the open vault, resets the auto-lock timer and
    /// remembers it as the last used vault. A vault that was open before
    /// (the browser extension switched vaults) is closed like on lock – its
    /// data wiped, a secret it copied cleared from the clipboard – without a
    /// `vault://locked` event (the caller announces the new vault).
    ///
    /// Refused (`invalid_input:update_in_progress`, `vault` wiped) once
    /// [`Core::lock_for_update`] closed the vault for an update: every unlock
    /// (UI, browser extension, recovery key, new vault) ends here.
    pub fn install_vault(&self, vault: UnlockedVault) -> AppResult<VaultInfo> {
        let info = vault.info();
        let previous = {
            let mut st = self.state();
            // Read under the state lock: `lock_for_update` sets the flag
            // before it takes the vault under this lock, so a vault opened
            // concurrently is either refused here or closed there.
            if self.update_locked.load(Ordering::SeqCst) {
                drop(st);
                drop(vault);
                return Err(AppError::invalid("update_in_progress"));
            }
            let previous = st.vault.replace(vault);
            st.touch();
            st.remember_vault(&info.id);
            previous
        };
        self.finish_lock(previous, None);
        Ok(info)
    }

    /// Unlocks `vault_id` with its master password. The key derivation runs
    /// without holding the state lock.
    pub fn unlock(&self, vault_id: &str, master_password: &str) -> AppResult<VaultInfo> {
        let store = self.state().store.clone();
        let vault = store.unlock(vault_id, master_password)?;
        self.install_vault(vault)
    }

    /// Clone of the store (for slow operations outside the state lock).
    pub fn store(&self) -> VaultStore {
        self.state().store.clone()
    }

    /// The data directory of the store.
    pub fn data_dir(&self) -> PathBuf {
        self.state().store.root().to_path_buf()
    }

    /// Runs `f` on the unlocked vault (`locked` if none).
    pub fn read<T>(&self, f: impl FnOnce(&UnlockedVault) -> T) -> AppResult<T> {
        let st = self.state();
        Ok(f(st.vault()?))
    }

    /// Runs a mutating operation on the unlocked vault. On
    /// `Error::Conflict` (another process saved the file) the vault is
    /// reloaded and `f` retried once; the UI then gets `vault://changed`.
    pub fn mutate<T>(
        &self,
        f: impl FnMut(&mut UnlockedVault) -> keystead_core::Result<T>,
    ) -> AppResult<T> {
        self.mutate_with(AppState::vault_mut, f)
    }

    /// [`Core::mutate`] for a command of the UI page that works on
    /// `page_vault_id`. Any other open vault is `locked` for it: the browser
    /// extension may have replaced the page's vault with another one
    /// (`install_vault`) a moment before the page learns of it and reloads,
    /// and an edit meant for the old vault (an item saved under an id the
    /// new vault does not know becomes a new item) must not land in the new
    /// one. Checked under the same state lock as the operation itself.
    pub fn mutate_for_page<T>(
        &self,
        page_vault_id: Option<&str>,
        f: impl FnMut(&mut UnlockedVault) -> keystead_core::Result<T>,
    ) -> AppResult<T> {
        self.mutate_with(|st| st.page_vault_mut(page_vault_id), f)
    }

    fn mutate_with<T>(
        &self,
        select: impl FnOnce(&mut AppState) -> AppResult<&mut UnlockedVault>,
        mut f: impl FnMut(&mut UnlockedVault) -> keystead_core::Result<T>,
    ) -> AppResult<T> {
        let (result, reloaded) = {
            let mut st = self.state();
            let vault = select(&mut st)?;
            match f(vault) {
                Err(CoreError::Conflict) => {
                    let reloaded = vault.reload_if_changed()?;
                    (f(vault), reloaded)
                }
                other => (other, false),
            }
        };
        if reloaded {
            self.emit_changed();
        }
        Ok(result?)
    }

    /// Before an update is installed – on Windows the process then ends with
    /// `exit(0)`, without `RunEvent::Exit` and [`Core::shutdown`]: from now on
    /// no vault can be opened ([`Core::install_vault`]), the browser bridge
    /// stops (no extension request during the install; waiting pairings are
    /// denied), the vault is closed (no event: the UI shows the update
    /// progress) and a copied secret still in the clipboard is cleared.
    /// Idempotent (it runs again right before the setup starts). Returns
    /// whether a vault was open.
    pub fn lock_for_update(&self) -> bool {
        self.update_locked.store(true, Ordering::SeqCst);
        crate::bridge::stop(self);
        let was_open = self.lock(None);
        if !was_open {
            self.clear_copied_secret();
        }
        was_open
    }

    /// The installation failed after [`Core::lock_for_update`]: vaults can
    /// be opened again, and the bridge runs again if browser integration is
    /// on.
    pub fn resume_after_failed_update(self: &Arc<Self>) {
        self.update_locked.store(false, Ordering::SeqCst);
        if !self.is_exiting() {
            crate::bridge::start_if_enabled(self);
        }
    }

    // -----------------------------------------------------------------
    // Updates
    // -----------------------------------------------------------------

    /// Records the result of an update check. Returns true if it found a
    /// version not announced before (→ `update://available`).
    pub fn remember_update(&self, info: &UpdateInfo) -> bool {
        let mut memory = lock_mutex(&self.updates);
        if !info.available {
            memory.pending = None;
            return false;
        }
        memory.pending = Some(info.clone());
        let new = memory.announced.as_deref() != info.version.as_deref();
        memory.announced = info.version.clone();
        new
    }

    /// The update the last check found, if any.
    pub fn pending_update(&self) -> Option<UpdateInfo> {
        lock_mutex(&self.updates).pending.clone()
    }

    pub fn set_update_wake(&self, sender: Sender<()>) {
        *lock_mutex(&self.update_wake) = Some(sender);
    }

    /// Lets the background update check run now (setting turned on,
    /// channel changed). The result of the old channel is dropped, and what
    /// it finds is announced again (the UI forgot it with the old channel).
    pub fn wake_update_checker(&self) {
        {
            let mut memory = lock_mutex(&self.updates);
            memory.pending = None;
            memory.announced = None;
        }
        if let Some(tx) = lock_mutex(&self.update_wake).as_ref() {
            let _ = tx.send(());
        }
    }

    /// Marks an update installation as started; false if one is running.
    pub fn begin_update_install(&self) -> bool {
        !self.update_installing.swap(true, Ordering::SeqCst)
    }

    pub fn end_update_install(&self) {
        self.update_installing.store(false, Ordering::SeqCst);
    }

    pub fn update_installing(&self) -> bool {
        self.update_installing.load(Ordering::SeqCst)
    }

    /// Marks the app as shutting down: stops the bridge, locks the vault and
    /// clears a pending clipboard secret (on Windows the clipboard outlives
    /// the process).
    pub fn shutdown(&self) {
        if self.exiting.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::bridge::stop(self);
        if !self.lock(None) {
            // Copied while no vault was open (or the vault was locked
            // before): still clear it.
            self.clear_copied_secret();
        }
        *lock_mutex(&self.monitor) = None;
        *lock_mutex(&self.update_wake) = None;
    }
}

/// Shows, restores and focuses the main window.
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
impl Core {
    /// A core without a window, for unit tests; emitted events are recorded.
    pub fn for_tests(store: VaultStore, settings: Settings) -> Arc<Core> {
        Self::with_frontend(
            Frontend::Recorder(Mutex::new(Vec::new())),
            "2.0.0-test".to_owned(),
            store,
            settings,
        )
    }

    /// The events emitted so far (name, payload).
    pub fn emitted(&self) -> Vec<(String, serde_json::Value)> {
        match &self.frontend {
            Frontend::Recorder(events) => lock_mutex(events).clone(),
            Frontend::App(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;
    use std::sync::Arc;

    use keystead_core::settings::Settings;
    use keystead_core::{KdfParams, VaultStore};

    use super::Core;

    /// A test core (no bridge, no window) whose store lives in `dir`, with
    /// a freshly created vault open. Never touches the real data directory.
    pub fn core_with_open_vault(dir: &Path, settings: Settings) -> Arc<Core> {
        let store = VaultStore::new(dir.join("data"));
        let vault = store
            .create_vault_with_params("Test", "master", KdfParams::insecure_for_tests())
            .unwrap();
        let settings = Settings {
            browser_integration: false,
            ..settings
        };
        let core = Core::for_tests(store, settings);
        core.state().vault = Some(vault);
        core
    }
}

#[cfg(test)]
mod tests {
    use keystead_core::settings::Settings;
    use keystead_core::KdfParams;

    use super::test_support::core_with_open_vault;
    use super::*;

    #[test]
    fn lock_emits_the_reason_only_when_a_vault_was_open() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        assert!(core.lock(Some(LockReason::Timeout)));
        assert!(core.state().vault.is_none());
        assert!(!core.lock(Some(LockReason::Manual)));
        let events = core.emitted();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, EVENT_LOCKED);
        assert_eq!(events[0].1, serde_json::json!({ "reason": "timeout" }));
    }

    fn update(version: &str, available: bool) -> UpdateInfo {
        UpdateInfo {
            available,
            current_version: "2.0.0-beta.1".into(),
            version: Some(version.into()),
            notes: None,
            date: None,
            can_install: available,
            release_url: crate::update::release_url(Some(version)),
        }
    }

    #[test]
    fn each_update_version_is_announced_once() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        assert_eq!(core.pending_update(), None);
        assert!(core.remember_update(&update("2.0.0-beta.2", true)));
        assert!(!core.remember_update(&update("2.0.0-beta.2", true)));
        assert_eq!(core.pending_update(), Some(update("2.0.0-beta.2", true)));
        assert!(core.remember_update(&update("2.0.0-beta.3", true)));
        // Nothing newer (e.g. other channel): no pending update, no event.
        assert!(!core.remember_update(&update("2.0.0-beta.1", false)));
        assert_eq!(core.pending_update(), None);
        // Settings changed: the old channel's result is dropped and the next
        // find is announced again.
        core.remember_update(&update("2.0.0-beta.4", true));
        core.wake_update_checker();
        assert_eq!(core.pending_update(), None);
        assert!(core.remember_update(&update("2.0.0-beta.4", true)));
    }

    #[test]
    fn only_one_update_installs_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        assert!(core.begin_update_install());
        assert!(core.update_installing());
        assert!(!core.begin_update_install());
        core.end_update_install();
        assert!(core.begin_update_install());
    }

    #[test]
    fn installing_an_update_closes_the_vault_without_an_event() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        assert!(core.lock_for_update());
        assert!(core.state().vault.is_none());
        assert!(!core.lock_for_update());
        assert!(
            core.emitted().is_empty(),
            "the UI shows the update progress"
        );
        assert_eq!(core.app_version(), "2.0.0-test");
    }

    #[test]
    fn no_vault_opens_while_an_update_installs() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.state().vault.as_ref().unwrap().id().to_owned();
        assert!(core.lock_for_update());

        // UI, browser extension (both via `unlock`) and a new vault
        // (`install_vault`) are refused; the vault stays closed.
        let refused = core.unlock(&id, "master").unwrap_err();
        assert_eq!(refused.code(), "invalid_input:update_in_progress");
        let other = core
            .store()
            .create_vault_with_params("Other", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        assert_eq!(
            core.install_vault(other).unwrap_err().code(),
            "invalid_input:update_in_progress"
        );
        assert!(core.state().vault.is_none());
        // Runs again right before the setup starts: still closed, no event.
        assert!(!core.lock_for_update());
        assert!(core.emitted().is_empty());

        // The installation failed: unlocking works again.
        core.resume_after_failed_update();
        assert_eq!(core.unlock(&id, "master").unwrap().id, id);
        assert!(core.state().vault.is_some());
    }

    #[test]
    fn an_unlock_racing_the_update_lock_never_leaves_the_vault_open() {
        use std::sync::atomic::AtomicUsize;

        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.state().vault.as_ref().unwrap().id().to_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let refused = Arc::new(AtomicUsize::new(0));
        let unlocker = {
            let (core, stop, refused, id) = (
                Arc::clone(&core),
                Arc::clone(&stop),
                Arc::clone(&refused),
                id.clone(),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    if core.unlock(&id, "master").is_err() {
                        refused.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        };
        std::thread::sleep(Duration::from_millis(30));
        core.lock_for_update();
        // Keep unlocking well after the lock (bounded: without the guard
        // nothing is ever refused).
        let deadline = Instant::now() + Duration::from_secs(3);
        while refused.load(Ordering::SeqCst) < 5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Ordering::SeqCst);
        unlocker.join().unwrap();
        assert!(
            core.state().vault.is_none(),
            "an unlock finished after lock_for_update"
        );
        assert!(refused.load(Ordering::SeqCst) >= 5);
    }

    /// `xvfb-run cargo test -p keystead-desktop -- --ignored clipboard`
    #[test]
    #[ignore = "needs a clipboard (X11 display or Windows desktop)"]
    fn clipboard_secret_without_timer_is_cleared_on_lock_and_quit() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            clipboard_clear_seconds: 0,
            ..Settings::default()
        };
        let core = core_with_open_vault(dir.path(), settings.clone());

        core.copy_to_clipboard("lock-zero", true).unwrap();
        assert_eq!(
            clipboard::read_text().unwrap().as_deref(),
            Some("lock-zero")
        );
        assert!(core.lock(None));
        assert_ne!(
            clipboard::read_text().unwrap().as_deref(),
            Some("lock-zero")
        );

        // Something copied afterwards is not the secret: left alone.
        let core = core_with_open_vault(dir.path(), settings.clone());
        core.copy_to_clipboard("secret-a", true).unwrap();
        core.copy_to_clipboard("plain-b", false).unwrap();
        assert!(core.lock(None));
        assert_eq!(clipboard::read_text().unwrap().as_deref(), Some("plain-b"));

        // Quitting clears it even when no vault is open any more.
        let core = core_with_open_vault(dir.path(), settings);
        assert!(core.lock(None));
        core.copy_to_clipboard("quit-zero", true).unwrap();
        core.shutdown();
        assert_ne!(
            clipboard::read_text().unwrap().as_deref(),
            Some("quit-zero")
        );
    }
}
