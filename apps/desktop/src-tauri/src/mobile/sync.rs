//! The phone's cloud-folder sync on top of [`crate::folder_sync`]: the
//! Kotlin plugin `KeysteadFolderPlugin` (gen/android) – or, for a server
//! link (stage 2c), `remote::folder::RemoteDirFolder` over the WebDAV
//! client with the password from `crate::credentials` – as the
//! [`RemoteFolder`] of every sync, push and resolution
//! ([`crate::sync_server::target`]); the commands `folder_status`,
//! `link_folder`, `link_webdav`, `sync_now` and `unlink_folder` (plus the
//! phone's side of the shared `resolve_sync_conflict`, [`resolve`]), the
//! automatic sync at start and whenever the app comes back to the
//! foreground (at most every [`AUTO_SYNC_INTERVAL`]), and the push after an
//! edit ([`push_soon`]). See "Cloud-folder sync (phone)" and "Server sync
//! (WebDAV)" in docs/ARCHITECTURE.md.
//!
//! * Automatic syncs and pushes of a server link wait after server errors
//!   (`sync_server::Backoff`: 1, 2, 5, then 15 minutes) and – with
//!   "Nur über WLAN synchronisieren" (`Settings.syncWifiOnly`) – while the
//!   phone is not on Wi-Fi (the folder plugin's `networkKind`); the status
//!   says why (`skipped`). `sync_now` and linking always sync.
//!
//! * The plugin is called from Rust only: no capability grants
//!   `keystead-folder:*` to the webview, and Tauri checks the ACL for every
//!   `plugin:` command – the page can neither pick, list, copy nor write
//!   anything itself; copies only ever go to `<data>/sync/incoming` and
//!   writes only come from `<data>/sync/outgoing` (the plugin refuses
//!   anything else as well).
//! * A plugin call blocks until Android's UI thread has run it (the
//!   provider work itself runs on the plugin's own thread), so it is never
//!   made on the UI thread: the commands run on the blocking pool, the
//!   lifecycle hook spawns a thread and the pushes run on their own.
//! * One sync, push, resolution or link change at a time
//!   ([`SyncState::guard`]); the link is cached in memory and written to
//!   `<data>/sync/link.json` after each change. Every change is announced
//!   as `sync://status` ([`FolderStatus`]), a change of the recorded
//!   conflicts as `sync://conflict`; no result carries a secret.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use keystead_core::model::VaultInfo;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::commands::{parse, run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::folder_sync::{self, FolderStatus, LinkKind, LinkState, RemoteFolder};
use crate::remote::webdav::WebDavConfig;
use crate::state::{log, Core};
use crate::sync_conflict::{self, Keep};
use crate::sync_server::{self, Backoff, LinkTarget};

/// The plugin's name (`plugin:keystead-folder|…` – never granted to the
/// webview).
pub const PLUGIN_NAME: &str = "keystead-folder";
/// `sync://status` – payload `FolderStatus`.
pub const EVENT_STATUS: &str = "sync://status";
/// The automatic sync (start, back in the foreground) runs at most this
/// often; `sync_now` always runs.
const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(60);
/// An edit is written to the folder this long after the last one (one
/// write for a burst of edits); leaving the app writes at once.
const PUSH_DELAY: Duration = Duration::from_secs(3);
/// `skipped`: "Nur über WLAN" is on and the phone is not on Wi-Fi.
pub const SKIPPED_WIFI_ONLY: &str = "sync_wifi_only";

/// The sync's shared state (managed by Tauri).
pub struct SyncState {
    /// Held for a whole sync, push, resolution or link change: one at a
    /// time.
    guard: Mutex<()>,
    /// The linked folder (a copy of `link.json`), `None` without one.
    link: Mutex<Option<LinkState>>,
    /// The last sync of any kind, on both clocks (the monotonic one stands
    /// still while the phone sleeps).
    last_sync: Mutex<Option<(Instant, SystemTime)>>,
    syncing: AtomicBool,
    /// The push thread's queue ([`push_soon`], [`flush_pushes`]).
    pushes: Mutex<Option<Sender<PushRequest>>>,
    /// The pause of automatic syncs after server errors (server links).
    backoff: Mutex<Backoff>,
    /// Why the last automatic sync or push did not run (`skipped`).
    skipped: Mutex<Option<String>>,
}

/// What the push thread is asked to do.
enum PushRequest {
    /// Vault `id` changed (or opened): write it after [`PUSH_DELAY`].
    Vault(String),
    /// The app leaves the foreground: write what waits now.
    Flush,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The Tauri plugin that loads the Kotlin side (Android) – no commands of
/// its own on the Rust side.
pub fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new(PLUGIN_NAME)
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            app.manage(android::FolderPlugin(api.register_android_plugin(
                "io.github.cedrickgd.keystead",
                "KeysteadFolderPlugin",
            )?));
            // iOS: no folder plugin yet (the commands answer
            // `unsupported:platform`).
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}

/// After the core exists (`gui::setup`): loads the link (seeding the sync
/// bases of a link of stage 2a once, `folder_sync::seed_bases`), starts the
/// push thread and runs the first automatic sync.
pub fn start(app: &AppHandle, core: &Core) {
    let mut link = LinkState::load(&core.data_dir());
    if let Some(link) = link.as_mut() {
        if folder_sync::seed_bases(core, link) {
            if let Err(e) = link.save(&core.data_dir()) {
                log(format_args!("could not save the folder link: {}", e.code()));
            }
        }
    }
    let (tx, rx) = mpsc::channel();
    app.manage(SyncState {
        guard: Mutex::new(()),
        link: Mutex::new(link),
        last_sync: Mutex::new(None),
        syncing: AtomicBool::new(false),
        pushes: Mutex::new(Some(tx)),
        backoff: Mutex::new(Backoff::default()),
        skipped: Mutex::new(None),
    });
    let push_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("keystead-sync".into())
        .spawn(move || push_worker(&push_app, &rx));
    if let Err(e) = spawned {
        log(format_args!("could not start the push thread: {e}"));
    }
    auto_sync(app);
}

/// Start and `Resumed`: syncs in a thread of its own if a folder is linked,
/// no sync ran in the last [`AUTO_SYNC_INTERVAL`] and none is running.
pub fn auto_sync(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("keystead-sync".into())
        .spawn(move || {
            let (Some(state), Some(core)) =
                (app.try_state::<SyncState>(), app.try_state::<Arc<Core>>())
            else {
                return;
            };
            let recent = lock(&state.last_sync).is_some_and(|(instant, wall)| {
                let wall = SystemTime::now()
                    .duration_since(wall)
                    .unwrap_or(Duration::MAX);
                instant.elapsed().max(wall) < AUTO_SYNC_INTERVAL
            });
            if recent {
                return;
            }
            let Ok(_guard) = state.guard.try_lock() else {
                return;
            };
            let Some(mut link) = lock(&state.link).clone() else {
                return;
            };
            if skip_automatic(&app, &core, &state, &link) {
                return;
            }
            if let Err(e) = sync_locked(&app, &core, &state, &mut link) {
                log(format_args!("automatic sync failed: {}", e.code()));
            }
        });
    if let Err(e) = spawned {
        log(format_args!("could not start the sync thread: {e}"));
    }
}

/// `Core::request_mirror` on the phone: vault `vault_id` changed or was
/// opened – it is synced (pushed, or its conflict checked) [`PUSH_DELAY`]
/// after the last such request, on the push thread. Without a linked
/// folder only the status is announced (the vault is `localOnly`).
pub fn push_soon(app: &AppHandle, vault_id: &str) {
    send_push(app, PushRequest::Vault(vault_id.to_owned()));
}

/// `WindowEvent::Suspended`: what waits for [`PUSH_DELAY`] is written now
/// (the process may be frozen or ended any moment in the background).
pub fn flush_pushes(app: &AppHandle) {
    send_push(app, PushRequest::Flush);
}

fn send_push(app: &AppHandle, request: PushRequest) {
    let Some(state) = app.try_state::<SyncState>() else {
        return;
    };
    let pushes = lock(&state.pushes);
    if let Some(tx) = pushes.as_ref() {
        let _ = tx.send(request);
    }
}

/// The push thread: collects the vaults to sync and runs them once no new
/// request came for [`PUSH_DELAY`] (or at once on a flush).
fn push_worker(app: &AppHandle, rx: &Receiver<PushRequest>) {
    let mut pending: BTreeSet<String> = BTreeSet::new();
    let mut due: Option<Instant> = None;
    loop {
        let request = match due {
            None => match rx.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            },
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(request) => Some(request),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    run_pushes(app, std::mem::take(&mut pending));
                    return;
                }
            },
        };
        match request {
            Some(PushRequest::Vault(id)) => {
                pending.insert(id);
                due = Some(Instant::now() + PUSH_DELAY);
            }
            Some(PushRequest::Flush) if !pending.is_empty() => due = Some(Instant::now()),
            Some(PushRequest::Flush) | None => {}
        }
        if due.is_some_and(|at| at <= Instant::now()) {
            due = None;
            run_pushes(app, std::mem::take(&mut pending));
        }
    }
}

/// Syncs each of `ids` (one at a time with every other sync).
fn run_pushes(app: &AppHandle, ids: BTreeSet<String>) {
    if ids.is_empty() {
        return;
    }
    let (Some(state), Some(core)) = (app.try_state::<SyncState>(), app.try_state::<Arc<Core>>())
    else {
        return;
    };
    let _guard = lock(&state.guard);
    let Some(mut link) = lock(&state.link).clone() else {
        // No folder: the vaults are only on the phone (`localOnly`).
        emit_status(app, &core, &state);
        return;
    };
    // A server that failed shortly before, or mobile data with "Nur über
    // WLAN": the edits wait for the next sync (it pushes them).
    if skip_automatic(app, &core, &state, &link) {
        return;
    }
    state.syncing.store(true, Ordering::SeqCst);
    emit_status(app, &core, &state);
    let target = link.clone();
    let mut failed = None;
    for id in &ids {
        let result = with_folder(app, &core, &target, |folder| {
            folder_sync::sync_vault(&core, folder, &mut link, id, now_ms())
        });
        if let Err(e) = result {
            log(format_args!("push failed: {}", e.code()));
            // The folder cannot be reached: the rest waits for the next
            // sync.
            link.last_error = Some(e.code());
            failed = Some(e.code());
            break;
        }
    }
    record_backoff(&state, &link, failed.as_deref());
    store_link(&core, &state, &link);
    state.syncing.store(false, Ordering::SeqCst);
    emit_status(app, &core, &state);
}

fn status(core: &Core, state: &SyncState) -> FolderStatus {
    let mut status = FolderStatus::of(lock(&state.link).as_ref());
    status.syncing = state.syncing.load(Ordering::SeqCst);
    if status.linked {
        status.skipped = lock(&state.skipped).clone();
        status.retry_at = lock(&state.backoff).retry_at_ms();
    } else {
        status.files = folder_sync::local_only_results(core);
    }
    status
}

/// Whether an automatic sync or push of `link` waits (a server link only):
/// the server failed shortly before ([`Backoff`]), or "Nur über WLAN" is on
/// and the phone is not on Wi-Fi. Records and announces why.
fn skip_automatic(app: &AppHandle, core: &Core, state: &SyncState, link: &LinkState) -> bool {
    if link.kind != LinkKind::Webdav {
        return false;
    }
    let reason = if !lock(&state.backoff).allows_now() {
        Some(sync_server::SKIPPED_BACKOFF)
    } else if core.state().settings.sync_wifi_only && !on_wifi(app) {
        Some(SKIPPED_WIFI_ONLY)
    } else {
        None
    };
    let Some(reason) = reason else {
        return false;
    };
    let before = lock(&state.skipped).replace(reason.to_owned());
    if before.as_deref() != Some(reason) {
        emit_status(app, core, state);
    }
    true
}

/// After a sync or push of `link` ran: no reason to skip any more, and the
/// pause after server errors (server links only).
fn record_backoff(state: &SyncState, link: &LinkState, error: Option<&str>) {
    *lock(&state.skipped) = None;
    if link.kind == LinkKind::Webdav {
        lock(&state.backoff).record_now(error);
    }
}

/// Whether the phone is on Wi-Fi (or Ethernet) right now – the folder
/// plugin's `networkKind`. Unknown counts as not.
#[cfg(target_os = "android")]
fn on_wifi(app: &AppHandle) -> bool {
    app.try_state::<android::FolderPlugin>()
        .is_some_and(|plugin| plugin.network_kind().is_ok_and(|kind| kind == "wifi"))
}

#[cfg(not(target_os = "android"))]
fn on_wifi(_app: &AppHandle) -> bool {
    false
}

fn emit_status(app: &AppHandle, core: &Core, state: &SyncState) {
    if let Err(e) = app.emit(EVENT_STATUS, status(core, state)) {
        log(format_args!("could not emit {EVENT_STATUS}: {e}"));
    }
}

/// Writes `link` to `link.json` and the cache.
fn store_link(core: &Core, state: &SyncState, link: &LinkState) {
    if let Err(e) = link.save(&core.data_dir()) {
        log(format_args!("could not save the folder link: {}", e.code()));
    }
    *lock(&state.link) = Some(link.clone());
}

/// Runs a sync of `link` (the caller holds the guard), stores and announces
/// the result. The sync's own error is also in the link (`last_error`).
fn sync_locked(
    app: &AppHandle,
    core: &Core,
    state: &SyncState,
    link: &mut LinkState,
) -> AppResult<()> {
    state.syncing.store(true, Ordering::SeqCst);
    emit_status(app, core, state);
    let target = link.clone();
    let result = with_folder(app, core, &target, |folder| {
        folder_sync::sync_folder(core, folder, link, now_ms()).map(|_| ())
    });
    if let Err(e) = &result {
        link.last_error = Some(e.code());
    }
    let error = result.as_ref().err().map(AppError::code);
    record_backoff(state, link, error.as_deref());
    store_link(core, state, link);
    *lock(&state.last_sync) = Some((Instant::now(), SystemTime::now()));
    state.syncing.store(false, Ordering::SeqCst);
    emit_status(app, core, state);
    result
}

/// Runs `f` with the folder of `link`: a server's through the WebDAV
/// client, a cloud app's through the plugin.
fn with_folder<T>(
    app: &AppHandle,
    core: &Core,
    link: &LinkState,
    f: impl FnOnce(&dyn RemoteFolder) -> AppResult<T>,
) -> AppResult<T> {
    match sync_server::target(core, link)? {
        LinkTarget::Server(folder) => f(&*folder),
        LinkTarget::Folder(tree) => with_plugin_folder(app, core, &tree, f),
    }
}

/// Runs `f` with the folder `tree` through the plugin.
#[cfg(target_os = "android")]
fn with_plugin_folder<T>(
    app: &AppHandle,
    core: &Core,
    tree: &str,
    f: impl FnOnce(&dyn RemoteFolder) -> AppResult<T>,
) -> AppResult<T> {
    let plugin = app
        .try_state::<android::FolderPlugin>()
        .ok_or_else(unsupported_platform)?;
    let folder = android::PluginFolder {
        plugin: &plugin,
        tree: tree.to_owned(),
        data_dir: core.data_dir(),
    };
    f(&folder)
}

#[cfg(not(target_os = "android"))]
fn with_plugin_folder<T>(
    _app: &AppHandle,
    _core: &Core,
    _tree: &str,
    _f: impl FnOnce(&dyn RemoteFolder) -> AppResult<T>,
) -> AppResult<T> {
    Err(unsupported_platform())
}

/// The link `previous` is replaced or removed: a cloud app's folder gets
/// its permission back, a server's password is forgotten (unless the new
/// link uses the same server folder, `keep`).
fn forget_link(app: &AppHandle, core: &Core, previous: &LinkState, keep: Option<&str>) {
    match previous.kind {
        LinkKind::Folder => release_folder(app, &previous.tree_uri),
        LinkKind::Webdav => sync_server::forget_password(core, &previous.tree_uri, keep),
    }
}

fn unsupported_platform() -> AppError {
    AppError::unsupported("platform")
}

fn sync_state(app: &AppHandle) -> AppResult<tauri::State<'_, SyncState>> {
    app.try_state::<SyncState>()
        .ok_or_else(|| AppError::io("sync not started"))
}

// ---------------------------------------------------------------------------
// Commands (phone only)
// ---------------------------------------------------------------------------

/// The linked folder and what the last sync found (without a folder: the
/// phone's vaults, `localOnly`). No I/O on the folder.
#[tauri::command]
pub async fn folder_status(app: AppHandle, core: Shared<'_>) -> CmdResult<FolderStatus> {
    run(&core, false, move |c| Ok(status(c, &*sync_state(&app)?))).await
}

/// Opens the system's folder picker (the user picks the folder the PC
/// keeps its copies in, e.g. through the Nextcloud or OneDrive app), keeps
/// read and – where the provider grants it – write access to it across
/// restarts, links it in place of the previous one and syncs. `null` when
/// the picker was closed without a choice. A failed first sync is not an
/// error: the status says so (`lastError`). Linking another folder forgets
/// the recorded conflicts (they were about the old one); a server link it
/// replaces forgets its password.
#[tauri::command]
pub async fn link_folder(app: AppHandle, core: Shared<'_>) -> CmdResult<Option<FolderStatus>> {
    core.touch_activity();
    let current = lock(&sync_state(&app)?.link)
        .as_ref()
        .filter(|l| l.kind == LinkKind::Folder)
        .map(|l| l.tree_uri.clone());
    let Some(picked) = pick_folder(&app, current).await? else {
        return Ok(None);
    };
    run(&core, true, move |c| {
        let link = LinkState::new(picked.tree, picked.name, now_ms(), picked.writable);
        replace_link(&app, c, link).map(Some)
    })
    .await
}

/// `link_webdav(config)` → [`FolderStatus`]: links the phone to a folder on
/// a WebDAV server (stage 2c) in place of the previous link and syncs. The
/// connection test must pass (its code otherwise, e.g.
/// `unsupported:sync_server_folder_missing` or
/// `unsupported:sync_server_certificate` – nothing changes then); the
/// password is stored sealed by the Android Keystore (`crate::credentials`)
/// and never returned. A failed first sync is not an error (`lastError`).
#[tauri::command]
pub async fn link_webdav(
    app: AppHandle,
    core: Shared<'_>,
    config: serde_json::Value,
) -> CmdResult<FolderStatus> {
    run(&core, true, move |c| {
        let cfg: WebDavConfig = parse(config, sync_server::BAD_CONFIG)?;
        let link = sync_server::link_server(c, &cfg, now_ms())?;
        replace_link(&app, c, link)
    })
    .await
}

/// Makes `link` the phone's link (the previous one forgotten – its folder
/// permission given back, or its server password removed – and its
/// conflicts if it was another folder) and syncs.
fn replace_link(app: &AppHandle, core: &Core, mut link: LinkState) -> AppResult<FolderStatus> {
    let state = sync_state(app)?;
    let _guard = lock(&state.guard);
    let previous = lock(&state.link).clone();
    link.save(&core.data_dir())?;
    *lock(&state.link) = Some(link.clone());
    lock(&state.backoff).reset();
    *lock(&state.skipped) = None;
    if let Some(old) = previous.filter(|old| old.tree_uri != link.tree_uri || old.kind != link.kind)
    {
        forget_link(app, core, &old, Some(&link.tree_uri));
        if old.tree_uri != link.tree_uri {
            if let Err(e) = sync_conflict::clear_conflicts(&core.data_dir()) {
                log(format_args!("could not forget the conflicts: {}", e.code()));
            }
            sync_conflict::emit_conflicts(core);
        }
    }
    let _ = sync_locked(app, core, &state, &mut link);
    Ok(status(core, &state))
}

/// Syncs now (waits for a sync already running). Errors with the sync's
/// error code – e.g. `unsupported:folder_access_lost` – after recording it
/// in the status; `unsupported:folder_not_linked` without a folder.
#[tauri::command]
pub async fn sync_now(app: AppHandle, core: Shared<'_>) -> CmdResult<FolderStatus> {
    run(&core, true, move |c| {
        let state = sync_state(&app)?;
        let _guard = lock(&state.guard);
        let mut link = lock(&state.link)
            .clone()
            .ok_or_else(|| AppError::unsupported("folder_not_linked"))?;
        sync_locked(&app, c, &state, &mut link)?;
        Ok(status(c, &state))
    })
    .await
}

/// Forgets the folder or server: gives the folder's permission back (a
/// server: removes its stored password) and removes `link.json` and the
/// recorded conflicts. The vault copies stay on the phone (and stop being
/// synced).
#[tauri::command]
pub async fn unlink_folder(app: AppHandle, core: Shared<'_>) -> CmdResult<FolderStatus> {
    run(&core, true, move |c| {
        let state = sync_state(&app)?;
        let _guard = lock(&state.guard);
        let previous = lock(&state.link).take();
        LinkState::remove(&c.data_dir())?;
        lock(&state.backoff).reset();
        *lock(&state.skipped) = None;
        if let Some(old) = previous {
            forget_link(&app, c, &old, None);
        }
        sync_conflict::emit_conflicts(c);
        emit_status(&app, c, &state);
        Ok(status(c, &state))
    })
    .await
}

/// `resolve_sync_conflict` on the phone (the command is shared,
/// `sync_conflict::resolve_sync_conflict`): keeps one side of the open
/// vault's conflict (`folder_sync::resolve_conflict`) with the linked
/// folder, one at a time with every other sync.
pub fn resolve(
    app: &AppHandle,
    core: &Core,
    vault_id: &str,
    page_vault_id: Option<&str>,
    keep: Keep,
    seen: (&str, &str),
) -> AppResult<VaultInfo> {
    let state = sync_state(app)?;
    let _guard = lock(&state.guard);
    let mut link = lock(&state.link)
        .clone()
        .ok_or_else(|| AppError::unsupported("folder_not_linked"))?;
    let target = link.clone();
    let result = with_folder(app, core, &target, |folder| {
        folder_sync::resolve_conflict(
            core,
            folder,
            &mut link,
            vault_id,
            page_vault_id,
            keep,
            seen,
            now_ms(),
        )
    });
    store_link(core, &state, &link);
    emit_status(app, core, &state);
    result
}

// ---------------------------------------------------------------------------
// The Kotlin plugin
// ---------------------------------------------------------------------------

/// The picker's answer.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Picked {
    tree: String,
    name: String,
    /// The provider granted write access too.
    writable: bool,
}

#[cfg(target_os = "android")]
async fn pick_folder(app: &AppHandle, initial: Option<String>) -> AppResult<Option<Picked>> {
    let plugin = app
        .try_state::<android::FolderPlugin>()
        .ok_or_else(unsupported_platform)?;
    plugin.pick(initial).await
}

#[cfg(not(target_os = "android"))]
async fn pick_folder(_app: &AppHandle, _initial: Option<String>) -> AppResult<Option<Picked>> {
    Err(unsupported_platform())
}

/// Gives the folder's persisted permission back (best effort).
fn release_folder(app: &AppHandle, tree: &str) {
    #[cfg(target_os = "android")]
    if let Some(plugin) = app.try_state::<android::FolderPlugin>() {
        if let Err(e) = plugin.release(tree) {
            log(format_args!("could not release the folder: {}", e.code()));
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = (app, tree);
}

#[cfg(target_os = "android")]
mod android {
    use std::fs;
    use std::path::{Path, PathBuf};

    use keystead_core::format;
    use keystead_core::sync;
    use serde::{Deserialize, Serialize};
    use tauri::plugin::mobile::PluginInvokeError;
    use tauri::plugin::PluginHandle;
    use tauri::Wry;

    use super::Picked;
    use crate::error::{AppError, AppResult};
    use crate::folder_sync::{self, RemoteEntry, RemoteFolder};

    /// `KeysteadFolderPlugin` (gen/android/app/src/main/java/…), registered
    /// by [`super::plugin`].
    pub struct FolderPlugin(pub PluginHandle<Wry>);

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PickArgs {
        initial_uri: Option<String>,
    }

    #[derive(Deserialize)]
    struct PickedFolder {
        tree: String,
        name: String,
        #[serde(default)]
        writable: bool,
    }

    #[derive(Serialize)]
    struct TreeArgs<'a> {
        tree: &'a str,
    }

    #[derive(Deserialize)]
    struct Access {
        read: bool,
        write: bool,
    }

    #[derive(Serialize)]
    struct ListArgs<'a> {
        tree: &'a str,
        parent: Option<&'a str>,
    }

    #[derive(Deserialize)]
    struct Listing {
        entries: Vec<RemoteEntry>,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CopyArgs<'a> {
        tree: &'a str,
        uri: &'a str,
        target: &'a str,
        max_bytes: u64,
    }

    #[derive(Deserialize)]
    struct Copied {
        size: u64,
    }

    #[derive(Deserialize)]
    struct Network {
        kind: String,
    }

    #[derive(Serialize)]
    struct WriteArgs<'a> {
        tree: &'a str,
        name: &'a str,
        source: &'a str,
    }

    #[derive(Deserialize)]
    struct Written {
        uri: String,
        size: u64,
    }

    #[derive(Serialize)]
    struct CopyWriteArgs<'a> {
        tree: &'a str,
        name: &'a str,
        source: &'a str,
    }

    /// The plugin's rejection codes (`invoke.reject(message, code)`) as the
    /// app's error codes.
    fn plugin_error(e: PluginInvokeError) -> AppError {
        match e {
            PluginInvokeError::InvokeRejected(r) => match r.code.as_deref() {
                Some("access_lost") => folder_sync::access_lost(),
                Some("too_large") => AppError::unsupported(folder_sync::TOO_LARGE),
                Some("not_found") => AppError::Code("not_found".into()),
                Some("picker_unavailable") => AppError::unsupported("folder_picker"),
                Some("not_writable") => AppError::unsupported(folder_sync::NOT_WRITABLE),
                Some("name_taken") => AppError::unsupported(folder_sync::NAME_TAKEN),
                Some("write_unverified") => AppError::unsupported(folder_sync::WRITE_UNVERIFIED),
                Some("invalid") => AppError::invalid("folder_write"),
                _ => AppError::io(r.message.unwrap_or_else(|| "folder plugin".into())),
            },
            other => AppError::io(other),
        }
    }

    impl FolderPlugin {
        /// The folder picker; `None` when it was closed without a choice.
        pub async fn pick(&self, initial_uri: Option<String>) -> AppResult<Option<Picked>> {
            match self
                .0
                .run_mobile_plugin_async::<PickedFolder>("pickFolder", PickArgs { initial_uri })
                .await
            {
                Ok(p) => Ok(Some(Picked {
                    tree: p.tree,
                    name: p.name,
                    writable: p.writable,
                })),
                Err(PluginInvokeError::InvokeRejected(r))
                    if r.code.as_deref() == Some("cancelled") =>
                {
                    Ok(None)
                }
                Err(e) => Err(plugin_error(e)),
            }
        }

        /// `wifi`, `cellular`, `other` or `none` (see the plugin).
        pub fn network_kind(&self) -> AppResult<String> {
            self.0
                .run_mobile_plugin::<Network>("networkKind", ())
                .map(|n| n.kind)
                .map_err(plugin_error)
        }

        pub fn release(&self, tree: &str) -> AppResult<()> {
            self.0
                .run_mobile_plugin::<serde_json::Value>("releaseFolder", TreeArgs { tree })
                .map(|_| ())
                .map_err(plugin_error)
        }
    }

    /// The linked folder through the plugin.
    pub struct PluginFolder<'a> {
        pub plugin: &'a FolderPlugin,
        pub tree: String,
        /// The app's data directory: a push stages its file in
        /// `<data>/sync/outgoing`.
        pub data_dir: PathBuf,
    }

    impl RemoteFolder for PluginFolder<'_> {
        fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>> {
            let args = ListArgs {
                tree: &self.tree,
                parent: parent.map(|p| p.uri.as_str()),
            };
            match self
                .plugin
                .0
                .run_mobile_plugin::<Listing>("listChildren", args)
            {
                Ok(listing) => Ok(listing.entries),
                // The linked folder itself is gone (deleted, moved).
                Err(e) => match plugin_error(e) {
                    e if parent.is_none() && e.code() == "not_found" => {
                        Err(folder_sync::access_lost())
                    }
                    e => Err(e),
                },
            }
        }

        fn copy_to(&self, entry: &RemoteEntry, target: &Path, max_bytes: u64) -> AppResult<u64> {
            let target = target
                .to_str()
                .ok_or_else(|| AppError::io("incoming path"))?;
            let args = CopyArgs {
                tree: &self.tree,
                uri: &entry.uri,
                target,
                max_bytes,
            };
            self.plugin
                .0
                .run_mobile_plugin::<Copied>("copyFile", args)
                .map(|c| c.size)
                .map_err(plugin_error)
        }

        fn writable(&self) -> AppResult<bool> {
            let access = self
                .plugin
                .0
                .run_mobile_plugin::<Access>("folderAccess", TreeArgs { tree: &self.tree })
                .map_err(plugin_error)?;
            if !access.read {
                return Err(folder_sync::access_lost());
            }
            Ok(access.write)
        }

        /// Stages `bytes` as `<data>/sync/outgoing/<name>` (the only place
        /// the plugin reads from), lets the plugin write it into the folder
        /// and removes it again.
        fn write_vault(&self, name: &str, bytes: &[u8]) -> AppResult<RemoteEntry> {
            if sync::vault_id_of_file_name(name).is_none() {
                return Err(AppError::invalid("folder_write"));
            }
            let outgoing = folder_sync::outgoing_dir(&self.data_dir);
            fs::create_dir_all(&outgoing)?;
            let source = outgoing.join(name);
            format::write_atomic(&source, bytes, false)?;
            let written = source
                .to_str()
                .ok_or_else(|| AppError::io("outgoing path"))
                .and_then(|source| {
                    self.plugin
                        .0
                        .run_mobile_plugin::<Written>(
                            "writeFile",
                            WriteArgs {
                                tree: &self.tree,
                                name,
                                source,
                            },
                        )
                        .map_err(plugin_error)
                });
            let _ = fs::remove_file(&source);
            let written = written?;
            Ok(RemoteEntry {
                name: name.to_owned(),
                uri: written.uri,
                dir: false,
                size: Some(written.size),
                etag: None,
            })
        }

        /// Stages `bytes` as `<data>/sync/outgoing/<name>` and lets the
        /// plugin create the file `name` (a phone conflict copy only) –
        /// never over an existing one.
        fn write_conflict_copy(&self, name: &str, bytes: &[u8]) -> AppResult<()> {
            if sync::vault_id_of_phone_conflict_copy(name).is_none() {
                return Err(AppError::invalid("folder_write"));
            }
            let outgoing = folder_sync::outgoing_dir(&self.data_dir);
            fs::create_dir_all(&outgoing)?;
            let source = outgoing.join(name);
            format::write_atomic(&source, bytes, false)?;
            let written = source
                .to_str()
                .ok_or_else(|| AppError::io("outgoing path"))
                .and_then(|source| {
                    self.plugin
                        .0
                        .run_mobile_plugin::<Written>(
                            "writeConflictCopy",
                            CopyWriteArgs {
                                tree: &self.tree,
                                name,
                                source,
                            },
                        )
                        .map_err(plugin_error)
                });
            let _ = fs::remove_file(&source);
            written.map(|_| ())
        }
    }
}
