//! The phone's cloud-folder sync on top of [`crate::folder_sync`]: the
//! Kotlin plugin `KeysteadFolderPlugin` (gen/android) as the
//! [`RemoteFolder`], the commands `folder_status`, `link_folder`,
//! `sync_now` and `unlink_folder`, and the automatic sync at start and
//! whenever the app comes back to the foreground (at most every
//! [`AUTO_SYNC_INTERVAL`]). See "Cloud-folder sync (phone)" in
//! docs/ARCHITECTURE.md.
//!
//! * The plugin is called from Rust only: no capability grants
//!   `keystead-folder:*` to the webview, and Tauri checks the ACL for every
//!   `plugin:` command – the page can neither pick, list nor copy anything
//!   itself, and the copies only ever go to `<data>/sync/incoming` (the
//!   plugin refuses any other target as well).
//! * A plugin call blocks until Android's UI thread has run it (the
//!   provider work itself runs on the plugin's own thread), so it is never
//!   made on the UI thread: the commands run on the blocking pool and the
//!   lifecycle hook spawns a thread.
//! * One sync or link change at a time ([`SyncState::guard`]); the link is
//!   cached in memory and written to `<data>/sync/link.json` after each
//!   change. Every change is announced as `sync://status`
//!   ([`FolderStatus`]); no result carries a secret.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::commands::{run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::folder_sync::{self, FolderStatus, LinkState};
use crate::state::{log, Core};

/// The plugin's name (`plugin:keystead-folder|…` – never granted to the
/// webview).
pub const PLUGIN_NAME: &str = "keystead-folder";
/// `sync://status` – payload `FolderStatus`.
pub const EVENT_STATUS: &str = "sync://status";
/// The automatic sync (start, back in the foreground) runs at most this
/// often; `sync_now` always runs.
const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// The sync's shared state (managed by Tauri).
pub struct SyncState {
    /// Held for a whole sync or link change: one at a time.
    guard: Mutex<()>,
    /// The linked folder (a copy of `link.json`), `None` without one.
    link: Mutex<Option<LinkState>>,
    /// The last sync of any kind, on both clocks (the monotonic one stands
    /// still while the phone sleeps).
    last_sync: Mutex<Option<(Instant, SystemTime)>>,
    syncing: AtomicBool,
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

/// After the core exists (`gui::setup`): loads the link and runs the first
/// automatic sync.
pub fn start(app: &AppHandle, core: &Core) {
    app.manage(SyncState {
        guard: Mutex::new(()),
        link: Mutex::new(LinkState::load(&core.data_dir())),
        last_sync: Mutex::new(None),
        syncing: AtomicBool::new(false),
    });
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
            if let Err(e) = sync_locked(&app, &core, &state, &mut link) {
                log(format_args!("automatic sync failed: {}", e.code()));
            }
        });
    if let Err(e) = spawned {
        log(format_args!("could not start the sync thread: {e}"));
    }
}

fn status(state: &SyncState) -> FolderStatus {
    let mut status = FolderStatus::of(lock(&state.link).as_ref());
    status.syncing = state.syncing.load(Ordering::SeqCst);
    status
}

fn emit_status(app: &AppHandle, state: &SyncState) {
    if let Err(e) = app.emit(EVENT_STATUS, status(state)) {
        log(format_args!("could not emit {EVENT_STATUS}: {e}"));
    }
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
    emit_status(app, state);
    let result = sync_with_plugin(app, core, link);
    if let Err(e) = link.save(&core.data_dir()) {
        log(format_args!("could not save the folder link: {}", e.code()));
    }
    *lock(&state.link) = Some(link.clone());
    *lock(&state.last_sync) = Some((Instant::now(), SystemTime::now()));
    state.syncing.store(false, Ordering::SeqCst);
    emit_status(app, state);
    result
}

#[cfg(target_os = "android")]
fn sync_with_plugin(app: &AppHandle, core: &Core, link: &mut LinkState) -> AppResult<()> {
    let plugin = app
        .try_state::<android::FolderPlugin>()
        .ok_or_else(unsupported_platform)?;
    let folder = android::PluginFolder {
        plugin: &plugin,
        tree: link.tree_uri.clone(),
    };
    folder_sync::sync_folder(core, &folder, link, now_ms()).map(|_| ())
}

#[cfg(not(target_os = "android"))]
fn sync_with_plugin(_app: &AppHandle, _core: &Core, link: &mut LinkState) -> AppResult<()> {
    let e = unsupported_platform();
    link.last_error = Some(e.code());
    Err(e)
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

/// The linked folder and what the last sync found. No I/O on the folder.
#[tauri::command]
pub async fn folder_status(app: AppHandle, core: Shared<'_>) -> CmdResult<FolderStatus> {
    run(&core, false, move |_| Ok(status(&*sync_state(&app)?))).await
}

/// Opens the system's folder picker (the user picks the folder the PC
/// keeps its copies in, e.g. through the Nextcloud or OneDrive app), keeps
/// read access to it across restarts, links it in place of the previous
/// one and syncs. `null` when the picker was closed without a choice. A
/// failed first sync is not an error: the status says so (`lastError`).
#[tauri::command]
pub async fn link_folder(app: AppHandle, core: Shared<'_>) -> CmdResult<Option<FolderStatus>> {
    core.touch_activity();
    let current = lock(&sync_state(&app)?.link)
        .as_ref()
        .map(|l| l.tree_uri.clone());
    let Some(picked) = pick_folder(&app, current).await? else {
        return Ok(None);
    };
    run(&core, true, move |c| {
        let state = sync_state(&app)?;
        let _guard = lock(&state.guard);
        let previous = lock(&state.link).clone();
        let mut link = LinkState::new(picked.tree, picked.name, now_ms());
        link.save(&c.data_dir())?;
        *lock(&state.link) = Some(link.clone());
        if let Some(old) = previous.filter(|old| old.tree_uri != link.tree_uri) {
            release_folder(&app, &old.tree_uri);
        }
        let _ = sync_locked(&app, c, &state, &mut link);
        Ok(Some(status(&state)))
    })
    .await
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
        Ok(status(&state))
    })
    .await
}

/// Forgets the folder: gives the read permission back and removes
/// `link.json`. The vault copies stay on the phone (and stop being
/// updated).
#[tauri::command]
pub async fn unlink_folder(app: AppHandle, core: Shared<'_>) -> CmdResult<FolderStatus> {
    run(&core, true, move |c| {
        let state = sync_state(&app)?;
        let _guard = lock(&state.guard);
        let previous = lock(&state.link).take();
        LinkState::remove(&c.data_dir())?;
        if let Some(old) = previous {
            release_folder(&app, &old.tree_uri);
        }
        emit_status(&app, &state);
        Ok(status(&state))
    })
    .await
}

// ---------------------------------------------------------------------------
// The Kotlin plugin
// ---------------------------------------------------------------------------

/// The picker's answer.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Picked {
    tree: String,
    name: String,
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
    use std::path::Path;

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
    }

    #[derive(Serialize)]
    struct TreeArgs<'a> {
        tree: &'a str,
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

    /// The plugin's rejection codes (`invoke.reject(message, code)`) as the
    /// app's error codes.
    fn plugin_error(e: PluginInvokeError) -> AppError {
        match e {
            PluginInvokeError::InvokeRejected(r) => match r.code.as_deref() {
                Some("access_lost") => folder_sync::access_lost(),
                Some("too_large") => AppError::unsupported(folder_sync::TOO_LARGE),
                Some("not_found") => AppError::Code("not_found".into()),
                Some("picker_unavailable") => AppError::unsupported("folder_picker"),
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
                })),
                Err(PluginInvokeError::InvokeRejected(r))
                    if r.code.as_deref() == Some("cancelled") =>
                {
                    Ok(None)
                }
                Err(e) => Err(plugin_error(e)),
            }
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
    }
}
