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

    /// Remembers `vault_id` as the last used vault (persisted).
    fn remember_vault(&mut self, vault_id: &str) {
        if self.settings.last_vault_id.as_deref() == Some(vault_id) {
            return;
        }
        self.settings.last_vault_id = Some(vault_id.to_owned());
        if let Err(e) = self.settings.save() {
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

/// The backend shared by all threads (`Arc<Core>` is managed Tauri state).
pub struct Core {
    frontend: Frontend,
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
}

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Poisoning only happens after a panic in another thread; the guarded
    // data stays consistent (single assignments), so keep going.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Core {
    pub fn new(app: AppHandle, store: VaultStore, settings: Settings) -> Arc<Core> {
        Self::with_frontend(Frontend::App(app), store, settings)
    }

    fn with_frontend(frontend: Frontend, store: VaultStore, settings: Settings) -> Arc<Core> {
        let minimize_to_tray = settings.minimize_to_tray;
        Arc::new(Core {
            frontend,
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
        })
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

    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
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

    /// Makes `vault` the open vault (replacing another one), resets the
    /// auto-lock timer and remembers it as the last used vault.
    pub fn install_vault(&self, vault: UnlockedVault) -> VaultInfo {
        let info = vault.info();
        let previous = {
            let mut st = self.state();
            let previous = st.vault.replace(vault);
            st.touch();
            st.remember_vault(&info.id);
            previous
        };
        drop(previous);
        info
    }

    /// Unlocks `vault_id` with its master password. The key derivation runs
    /// without holding the state lock.
    pub fn unlock(&self, vault_id: &str, master_password: &str) -> AppResult<VaultInfo> {
        let store = self.state().store.clone();
        let vault = store.unlock(vault_id, master_password)?;
        Ok(self.install_vault(vault))
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
        mut f: impl FnMut(&mut UnlockedVault) -> keystead_core::Result<T>,
    ) -> AppResult<T> {
        let (result, reloaded) = {
            let mut st = self.state();
            let vault = st.vault_mut()?;
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
        Self::with_frontend(Frontend::Recorder(Mutex::new(Vec::new())), store, settings)
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
