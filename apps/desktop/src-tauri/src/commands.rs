//! Tauri commands – exactly the table "Desktop backend ↔ frontend" in
//! docs/ARCHITECTURE.md. Every command is `async` and runs its work on the
//! blocking thread pool (key derivation, file I/O), never on the main
//! thread. Errors are stable codes (see [`AppError::code`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use keystead_bridge::{register, BrowserId, BrowserInfo, PairedClient, EXTENSION_ID};
use keystead_core::generator::{self, GeneratorOptions};
use keystead_core::health::{self, HealthReport, Strength};
use keystead_core::import::{self, ImportReport, LegacyVaultInfo};
use keystead_core::model::{Folder, GeneratedPassword, VaultInfo, VaultItem};
use keystead_core::settings::Settings;
use keystead_core::totp::{self, TotpCode};
use keystead_core::{export, paths, Error as CoreError, VaultStore};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};

use crate::bridge;
use crate::error::{AppError, AppResult};
use crate::extension;
use crate::platform;
use crate::portable;
use crate::state::{log, Core, LockReason};
use crate::update::{self, UpdateInfo};
use crate::wipe::Wiped;

type Shared<'a> = State<'a, Arc<Core>>;
type CmdResult<T> = Result<T, String>;

/// Delay before events a freshly loaded page may have missed are re-sent:
/// its listeners subscribe asynchronously after boot, and the boot applies
/// its `session_state` answer only once all boot requests have returned.
const BOOT_RESEND_DELAY: Duration = Duration::from_millis(1000);

/// Runs `f` on the blocking pool. `activity`: the command is a deliberate
/// user action and resets the auto-lock timer (passive/polled commands such
/// as `totp_code` or `browser_status` must not keep the vault open).
async fn run<T, F>(core: &Shared<'_>, activity: bool, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Arc<Core>) -> AppResult<T> + Send + 'static,
{
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        if activity {
            core.touch_activity();
        }
        f(&core)
    })
    .await
    .map_err(|e| format!("io:{e}"))?
    .map_err(String::from)
}

/// Parses a JSON argument; malformed input → `invalid_input:<what>`.
fn parse<T: DeserializeOwned>(value: Value, what: &str) -> AppResult<T> {
    serde_json::from_value(value).map_err(|_| AppError::invalid(what))
}

fn exe() -> AppResult<std::path::PathBuf> {
    bridge::current_exe().ok_or_else(|| AppError::io("current_exe"))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    data_dir: String,
    portable: bool,
    platform: &'static str,
    extension_id: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    unlocked: bool,
    vault: Option<VaultInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    server_running: bool,
    extension_id: &'static str,
    /// The folder the app keeps the browser extension in (load it unpacked
    /// from there) and the version the app delivers.
    extension_dir: String,
    extension_version: String,
    browsers: Vec<BrowserInfo>,
    clients: Vec<PairedClient>,
}

fn app_info_of(core: &Core) -> AppInfo {
    AppInfo {
        // The version from tauri.conf.json, which CI sets per build (e.g.
        // "2.0.0-beta.7"), or CI's KEYSTEAD_VERSION_LABEL.
        version: core.app_version().to_owned(),
        data_dir: core.data_dir().display().to_string(),
        portable: paths::is_portable(),
        platform: platform::platform_name(),
        extension_id: EXTENSION_ID,
    }
}

fn browser_status_of(core: &Core) -> AppResult<BrowserStatus> {
    let server_running = bridge::is_running(core);
    Ok(BrowserStatus {
        server_running,
        extension_id: EXTENSION_ID,
        extension_dir: extension::extension_dir(&core.data_dir())
            .display()
            .to_string(),
        extension_version: extension::bundled_version().to_owned(),
        browsers: register::browsers(),
        clients: bridge::clients(core)?,
    })
}

fn parse_browsers(ids: &[String]) -> AppResult<Vec<BrowserId>> {
    ids.iter()
        .map(|id| {
            id.parse::<BrowserId>()
                .map_err(|_| AppError::invalid("browser"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// App & session
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn app_info(core: Shared<'_>) -> CmdResult<AppInfo> {
    run(&core, false, |c| Ok(app_info_of(c))).await
}

#[tauri::command]
pub async fn list_vaults(core: Shared<'_>) -> CmdResult<Vec<VaultInfo>> {
    run(&core, false, |c| Ok(c.store().list_vaults()?)).await
}

#[tauri::command]
pub async fn session_state(core: Shared<'_>) -> CmdResult<SessionState> {
    run(&core, false, |c| {
        let vault = c.read(|v| v.info()).ok();
        if c.take_page_loaded() {
            let answered_vault = vault.as_ref().map(|v| v.id.clone());
            let c = Arc::clone(c);
            std::thread::spawn(move || {
                std::thread::sleep(BOOT_RESEND_DELAY);
                // A pairing request may have arrived before the page listened.
                for request in bridge::pending_pairings(&c) {
                    c.emit_pairing_request(request);
                }
                // The extension may have unlocked, switched or locked the
                // vault while the page booted: its event got lost, or the boot
                // overwrote it with this (by then stale) answer.
                match (answered_vault, c.read(|v| v.info()).ok()) {
                    (answered, Some(info)) if answered.as_deref() != Some(info.id.as_str()) => {
                        c.emit_unlocked(&info)
                    }
                    (Some(_), None) => c.emit_locked(LockReason::Manual),
                    _ => {}
                }
            });
        }
        Ok(SessionState {
            unlocked: vault.is_some(),
            vault,
        })
    })
    .await
}

#[tauri::command]
pub async fn create_vault(
    core: Shared<'_>,
    name: String,
    master_password: String,
) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| {
        let vault = c.store().create_vault(&name, &master_password)?;
        Ok(c.install_vault(vault))
    })
    .await
}

#[tauri::command]
pub async fn unlock_vault(
    core: Shared<'_>,
    vault_id: String,
    master_password: String,
) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| c.unlock(&vault_id, &master_password)).await
}

#[tauri::command]
pub async fn unlock_with_recovery(
    core: Shared<'_>,
    vault_id: String,
    recovery_key: String,
    new_master_password: String,
) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| {
        let vault =
            c.store()
                .unlock_with_recovery_key(&vault_id, &recovery_key, &new_master_password)?;
        Ok(c.install_vault(vault))
    })
    .await
}

#[tauri::command]
pub async fn lock_vault(core: Shared<'_>) -> CmdResult<()> {
    // The UI navigates itself; no `vault://locked` for its own request.
    run(&core, false, |c| {
        c.lock(None);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn touch_activity(core: Shared<'_>) -> CmdResult<()> {
    run(&core, true, |_| Ok(())).await
}

// ---------------------------------------------------------------------------
// Items & folders
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_items(core: Shared<'_>) -> CmdResult<Wiped<Vec<VaultItem>>> {
    run(&core, false, |c| c.read(|v| Wiped(v.items().to_vec()))).await
}

#[tauri::command]
pub async fn list_folders(core: Shared<'_>) -> CmdResult<Vec<Folder>> {
    run(&core, false, |c| {
        c.read(|v| {
            let mut folders = v.folders().to_vec();
            folders.sort_by_cached_key(|f| (f.name.to_lowercase(), f.id.clone()));
            folders
        })
    })
    .await
}

// The commands that change or export the open vault also take
// `page_vault_id`: the vault the UI page works on. Another open vault (the
// browser extension switched vaults, the page has not reloaded yet) answers
// `locked` – see `Core::mutate_for_page`.

#[tauri::command]
pub async fn save_item(
    core: Shared<'_>,
    item: Value,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<VaultItem>> {
    run(&core, true, move |c| {
        let item = Wiped(parse::<VaultItem>(item, "item")?);
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.save_item(item.0.clone()))
            .map(Wiped)
    })
    .await
}

#[tauri::command]
pub async fn trash_item(
    core: Shared<'_>,
    id: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.trash_item(&id))
    })
    .await
}

#[tauri::command]
pub async fn restore_item(
    core: Shared<'_>,
    id: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.restore_item(&id))
    })
    .await
}

#[tauri::command]
pub async fn delete_item(
    core: Shared<'_>,
    id: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.delete_item(&id))
    })
    .await
}

#[tauri::command]
pub async fn empty_trash(core: Shared<'_>, page_vault_id: Option<String>) -> CmdResult<usize> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.empty_trash())
    })
    .await
}

#[tauri::command]
pub async fn save_folder(
    core: Shared<'_>,
    folder: Value,
    page_vault_id: Option<String>,
) -> CmdResult<Folder> {
    run(&core, true, move |c| {
        let folder: Folder = parse(folder, "folder")?;
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.save_folder(folder.clone()))
    })
    .await
}

#[tauri::command]
pub async fn delete_folder(
    core: Shared<'_>,
    id: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.delete_folder(&id))
    })
    .await
}

// ---------------------------------------------------------------------------
// Generator, strength, TOTP, clipboard, health
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn generate_password(
    core: Shared<'_>,
    options: Value,
    remember: bool,
    page_vault_id: Option<String>,
) -> CmdResult<String> {
    run(&core, true, move |c| {
        let options: GeneratorOptions = parse(options, "options")?;
        let password = generator::generate(&options)?;
        if remember {
            // Only an unlocked vault has a history; generating works anyway.
            match c.mutate_for_page(page_vault_id.as_deref(), |v| {
                v.add_generated_password(&password)
            }) {
                Ok(()) | Err(AppError::Locked) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(password)
    })
    .await
}

#[tauri::command]
pub async fn generator_history(core: Shared<'_>) -> CmdResult<Wiped<Vec<GeneratedPassword>>> {
    run(&core, true, |c| {
        c.read(|v| Wiped(v.generator_history().to_vec()))
    })
    .await
}

#[tauri::command]
pub async fn clear_generator_history(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.clear_generator_history())
    })
    .await
}

#[tauri::command]
pub async fn password_strength(core: Shared<'_>, password: String) -> CmdResult<Strength> {
    run(&core, true, move |_| Ok(health::strength(&password, &[]))).await
}

#[tauri::command]
pub async fn totp_code(core: Shared<'_>, seed: String) -> CmdResult<TotpCode> {
    // Polled by the UI every period: not an activity.
    run(&core, false, move |_| Ok(totp::totp_now(&seed)?)).await
}

#[tauri::command]
pub async fn copy_text(core: Shared<'_>, text: String, sensitive: bool) -> CmdResult<()> {
    run(&core, true, move |c| {
        let text = zeroize::Zeroizing::new(text);
        c.copy_to_clipboard(&text, sensitive)
    })
    .await
}

#[tauri::command]
pub async fn health_report(core: Shared<'_>) -> CmdResult<HealthReport> {
    run(&core, true, |c| c.read(|v| health::health_report(v.data()))).await
}

// ---------------------------------------------------------------------------
// Vault management
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn change_master_password(
    core: Shared<'_>,
    current: String,
    new_password: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| {
            v.change_master_password(&current, &new_password)
        })
    })
    .await
}

#[tauri::command]
pub async fn create_recovery_key(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<String> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.create_recovery_key())
    })
    .await
}

#[tauri::command]
pub async fn remove_recovery_key(core: Shared<'_>, page_vault_id: Option<String>) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.remove_recovery_key())
    })
    .await
}

#[tauri::command]
pub async fn rename_vault(
    core: Shared<'_>,
    name: String,
    page_vault_id: Option<String>,
) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| {
        // The info of the renamed vault itself, not of one opened meanwhile.
        c.mutate_for_page(page_vault_id.as_deref(), |v| {
            v.rename(&name)?;
            Ok(v.info())
        })
    })
    .await
}

#[tauri::command]
pub async fn delete_vault(
    core: Shared<'_>,
    vault_id: String,
    master_password: String,
) -> CmdResult<()> {
    run(&core, false, move |c| {
        // Verifies the password (slow) without holding the state lock.
        c.store().delete_vault(&vault_id, &master_password)?;
        let closed = {
            let mut st = c.state();
            let closed = if st.vault.as_ref().is_some_and(|v| v.id() == vault_id) {
                st.vault.take()
            } else {
                None
            };
            if st.settings.last_vault_id.as_deref() == Some(vault_id.as_str()) {
                st.settings.last_vault_id = None;
                st.save_settings();
            }
            closed
        };
        // The UI navigates itself after deleting; no `vault://locked`.
        c.finish_lock(closed, None);
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Import & export
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn legacy_scan(core: Shared<'_>) -> CmdResult<Vec<LegacyVaultInfo>> {
    run(&core, false, |_| Ok(import::legacy_scan())).await
}

#[tauri::command]
pub async fn import_data(
    core: Shared<'_>,
    format: String,
    path: String,
    password: Option<String>,
    page_vault_id: Option<String>,
) -> CmdResult<ImportReport> {
    run(&core, true, move |c| {
        if path.trim().is_empty() {
            return Err(AppError::invalid("path_required"));
        }
        let report = c.mutate_for_page(page_vault_id.as_deref(), |v| {
            import::import_into(v, &format, Path::new(&path), password.as_deref())
        })?;
        c.emit_changed();
        Ok(report)
    })
    .await
}

#[tauri::command]
pub async fn export_data(
    core: Shared<'_>,
    format: String,
    path: String,
    password: Option<String>,
    master_password: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        if path.trim().is_empty() {
            return Err(AppError::invalid("path_required"));
        }
        let st = c.state();
        let vault = st.page_vault(page_vault_id.as_deref())?;
        if !vault.verify_master_password(&master_password) {
            return Err(CoreError::WrongPassword.into());
        }
        export::export_to_file(vault.data(), &format, Path::new(&path), password.as_deref())?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_settings(core: Shared<'_>) -> CmdResult<Settings> {
    run(&core, false, |c| Ok(c.state().settings.clone())).await
}

#[tauri::command]
pub async fn save_settings(core: Shared<'_>, settings: Value) -> CmdResult<Settings> {
    run(&core, true, move |c| {
        let settings: Settings = parse(settings, "settings")?;
        apply_settings(c, settings)
    })
    .await
}

/// Persists new settings and applies their side effects (tray labels,
/// close-to-tray, bridge on/off).
fn apply_settings(core: &Arc<Core>, settings: Settings) -> AppResult<Settings> {
    let settings = settings.normalized();
    let old = {
        let mut st = core.state();
        settings.save()?;
        std::mem::replace(&mut st.settings, settings.clone())
    };
    if settings.update_channel != old.update_channel || (settings.update_check && !old.update_check)
    {
        core.wake_update_checker();
    }
    core.set_minimize_to_tray(settings.minimize_to_tray);
    if old.language != settings.language {
        core.with_tray(|tray| tray.set_language(settings.language));
    }
    if old.browser_integration != settings.browser_integration {
        if settings.browser_integration {
            if let Err(e) = bridge::start(core) {
                // Reported through `browser_status().serverRunning`.
                log(format_args!("browser bridge not started: {}", e.code()));
            }
            bridge::reregister_if_needed();
        } else {
            bridge::stop(core);
        }
    }
    Ok(settings)
}

// ---------------------------------------------------------------------------
// Browser integration
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn browser_status(core: Shared<'_>) -> CmdResult<BrowserStatus> {
    // Polled by the settings page: not an activity.
    run(&core, false, |c| browser_status_of(c)).await
}

#[tauri::command]
pub async fn register_browsers(
    core: Shared<'_>,
    browsers: Vec<String>,
) -> CmdResult<BrowserStatus> {
    run(&core, true, move |c| {
        let ids = parse_browsers(&browsers)?;
        register::register(&ids, &exe()?)?;
        browser_status_of(c)
    })
    .await
}

#[tauri::command]
pub async fn unregister_browsers(
    core: Shared<'_>,
    browsers: Vec<String>,
) -> CmdResult<BrowserStatus> {
    run(&core, true, move |c| {
        let ids = parse_browsers(&browsers)?;
        register::unregister(&ids)?;
        browser_status_of(c)
    })
    .await
}

#[tauri::command]
pub async fn revoke_client(core: Shared<'_>, client_id: String) -> CmdResult<BrowserStatus> {
    run(&core, true, move |c| {
        bridge::revoke(c, &client_id)?;
        browser_status_of(c)
    })
    .await
}

#[tauri::command]
pub async fn respond_pairing(core: Shared<'_>, request_id: String, approve: bool) -> CmdResult<()> {
    run(&core, true, move |c| {
        let dispatcher = c.state().dispatcher.clone();
        let answered = dispatcher.is_some_and(|d| d.respond_pairing(&request_id, approve));
        if answered {
            Ok(())
        } else {
            // Unknown, already answered or timed out.
            Err(CoreError::NotFound(format!("pairing request {request_id}")).into())
        }
    })
    .await
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn open_terminal(core: Shared<'_>) -> CmdResult<()> {
    run(&core, true, |_| platform::open_terminal(&exe()?)).await
}

#[tauri::command]
pub async fn open_data_dir(core: Shared<'_>) -> CmdResult<()> {
    run(&core, true, |c| platform::open_folder(&c.data_dir())).await
}

/// Opens the folder the app keeps the browser extension in (writes it
/// first if it is missing).
#[tauri::command]
pub async fn open_extension_dir(core: Shared<'_>) -> CmdResult<()> {
    run(&core, true, |c| {
        let dir = extension::deploy_logged(&c.data_dir());
        platform::open_folder(&dir)
    })
    .await
}

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

/// Checks the configured update channel now ("Nach Updates suchen").
#[tauri::command]
pub async fn check_update(app: AppHandle, core: Shared<'_>) -> CmdResult<UpdateInfo> {
    let core = Arc::clone(core.inner());
    core.touch_activity();
    Ok(update::check_now(&app, &core).await?)
}

/// The update the last check found (a page that reloaded shows its banner
/// again), without a network request.
#[tauri::command]
pub async fn pending_update(core: Shared<'_>) -> CmdResult<Option<UpdateInfo>> {
    run(&core, false, |c| Ok(c.pending_update())).await
}

/// Downloads, verifies and installs the update, then restarts (see
/// `update::install`).
#[tauri::command]
pub async fn install_update(app: AppHandle, core: Shared<'_>) -> CmdResult<()> {
    let core = Arc::clone(core.inner());
    core.touch_activity();
    Ok(update::install(&app, &core).await?)
}

/// Moves the data between the OS data directory and `Keystead-Data` next to
/// the exe. The open vault is locked first (its file moves); the UI then
/// asks for the master password again.
#[tauri::command]
pub async fn set_portable_mode(core: Shared<'_>, enabled: bool) -> CmdResult<AppInfo> {
    run(&core, true, move |c| {
        switch_portable_mode(c, enabled, portable::preflight, portable::set_portable)?;
        Ok(app_info_of(c))
    })
    .await
}

/// `set_portable_mode` with the data move passed in (unit tests).
/// `preflight` runs while the vault is still open and the bridge running,
/// so the expected refusals change nothing. Once the vault had to be
/// closed for the move, `vault://locked {manual}` is emitted – whether the
/// move then succeeds or not – so the UI never keeps showing the items of a
/// vault that is no longer open.
fn switch_portable_mode(
    c: &Arc<Core>,
    enabled: bool,
    preflight: impl FnOnce(bool) -> AppResult<bool>,
    move_data: impl FnOnce(bool) -> AppResult<PathBuf>,
) -> AppResult<()> {
    if !preflight(enabled)? {
        return Ok(());
    }
    // Nothing may use the data directory while it moves.
    bridge::stop(c);
    let (result, closed) = {
        // Held across the move: nothing can unlock the vault meanwhile.
        let mut st = c.state();
        let closed = st.vault.take();
        let result = move_data(enabled).map(|dir| {
            st.store = VaultStore::new(dir);
        });
        (result, closed)
    };
    c.finish_lock(closed, Some(LockReason::Manual));
    if result.is_ok() {
        // The host manifest moved with the data directory.
        bridge::reregister_if_needed();
        // The extension folder is not moved (the browser may use it): write
        // it at the new location, the extension points the user there.
        extension::deploy_logged(&c.data_dir());
    }
    bridge::start_if_enabled(c);
    result
}

#[cfg(test)]
mod tests {
    use keystead_core::settings::Settings;

    use super::*;
    use crate::state::test_support::core_with_open_vault;
    use crate::state::EVENT_LOCKED;

    fn locked_events(c: &Core) -> Vec<Value> {
        c.emitted()
            .into_iter()
            .filter(|(name, _)| name == EVENT_LOCKED)
            .map(|(_, payload)| payload)
            .collect()
    }

    #[test]
    fn refused_portable_switch_leaves_the_vault_open() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let err = switch_portable_mode(
            &c,
            true,
            |_| Err(AppError::unsupported("portable_installed")),
            |_| panic!("must not move after a failed preflight"),
        )
        .unwrap_err();
        assert_eq!(err.code(), "unsupported:portable_installed");
        assert!(c.state().vault.is_some(), "vault must stay open");
        assert!(locked_events(&c).is_empty());

        // Already in the requested mode: nothing happens either.
        switch_portable_mode(&c, true, |_| Ok(false), |_| panic!("no move")).unwrap();
        assert!(c.state().vault.is_some());
    }

    #[test]
    fn failed_portable_move_announces_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let root = c.data_dir();
        let err = switch_portable_mode(&c, true, |_| Ok(true), |_| Err(AppError::io("read-only")))
            .unwrap_err();
        assert_eq!(err.code(), "io:read-only");
        assert!(c.state().vault.is_none());
        assert_eq!(c.data_dir(), root, "store unchanged");
        assert_eq!(
            locked_events(&c),
            vec![serde_json::json!({ "reason": "manual" })]
        );
    }
}
