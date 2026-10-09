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

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use keystead_bridge::{Dispatcher, ServerHandle};
use keystead_core::model::VaultInfo;
use keystead_core::settings::Settings;
use keystead_core::{clipboard, Error as CoreError, UnlockedVault, VaultStore};

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

/// The backend shared by all threads (`Arc<Core>` is managed Tauri state).
pub struct Core {
    app: AppHandle,
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
}

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Poisoning only happens after a panic in another thread; the guarded
    // data stays consistent (single assignments), so keep going.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Core {
    pub fn new(app: AppHandle, store: VaultStore, settings: Settings) -> Arc<Core> {
        let minimize_to_tray = settings.minimize_to_tray;
        Arc::new(Core {
            app,
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
        if let Err(e) = self.app.emit(event, payload) {
            log(format_args!("could not emit {event}: {e}"));
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

    /// Shows, restores and focuses the main window.
    pub fn show_main_window(&self) {
        show_main_window(&self.app);
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
        let was_open = vault.is_some();
        // Wipe outside the state lock.
        drop(vault);
        if was_open {
            if let Err(e) = clipboard::clear_pending_secret() {
                log(format_args!("could not clear the clipboard: {}", e.code()));
            }
            if let Some(reason) = reason {
                self.emit_locked(reason);
            }
        }
        was_open
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
        self.lock(None);
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
