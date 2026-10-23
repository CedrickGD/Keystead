//! GUI mode: Tauri setup, main window, background services and shutdown –
//! on the desktop also tray, single instance, browser bridge, extension
//! deployment, update checks and the website-icon fetcher.
//!
//! One builder for both: `#[cfg(desktop)]` marks the plugins, commands and
//! services the phone does without; the phone registers the vault commands
//! except the desktop-only ones (master password, recovery key, deleting a
//! vault, attachment changes, import/export) plus its own – the
//! cloud-folder and server sync and the fingerprint unlock (see "On the
//! phone" under "Desktop backend ↔ frontend" in docs/ARCHITECTURE.md).

use std::sync::mpsc;
use std::sync::Arc;

use keystead_core::settings::Settings;
use keystead_core::{paths, VaultStore};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewWindow, WebviewWindowBuilder, WindowEvent};

#[cfg(mobile)]
use crate::monitor::MonitorSignal;
#[cfg(desktop)]
use crate::state::show_main_window;
use crate::state::{log, Core, MAIN_WINDOW};
#[cfg(desktop)]
use crate::{bridge, extension, import_flow, tray, update, BACKGROUND_ARG};
use crate::{commands, monitor, platform};

/// Runs the desktop app; returns the process exit code.
#[cfg(desktop)]
pub fn run(background: bool) -> i32 {
    let app = match build_app(background) {
        Ok(app) => app,
        Err(e) => {
            log(format_args!("could not start the app: {e}"));
            return 1;
        }
    };
    app.run_return(|app, event| {
        if let RunEvent::Exit = event {
            if let Some(core) = app.try_state::<Arc<Core>>() {
                core.shutdown();
            }
        }
    })
}

/// Runs the phone app. On Android the process ends with the activity, so
/// `RunEvent::Exit` (and `Core::shutdown`) practically never runs there:
/// nothing security-relevant may depend on it (the lock on leaving the app
/// is the monitor's job).
#[cfg(mobile)]
pub fn run_mobile() {
    match build_app(false) {
        Ok(app) => app.run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    core.shutdown();
                }
            }
        }),
        Err(e) => log(format_args!("could not start the app: {e}")),
    }
}

/// The Tauri app with plugins, commands and the setup hook; `background`:
/// start hidden (desktop only).
fn build_app(background: bool) -> tauri::Result<tauri::App> {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder
        // Must be the first plugin: a second instance exits right here.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second `--background` launch (from the native host) must
            // not pop up the window; a normal launch brings it to front.
            if !argv.iter().skip(1).any(|a| a == BACKGROUND_ARG) {
                show_main_window(app);
            }
        }))
        // The UI never calls these plugins directly (no permission granted):
        // `check_update` / `install_update` below drive the updater. Its
        // Windows exit hook (`on_before_exit`) is set where the updater is
        // built, `update::build_updater`.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());
    #[cfg(mobile)]
    let builder = builder
        // Called from Rust only (`mobile::PluginClipboard`, `platform::open_url`);
        // the webview gets no permission for them either. The opener's own
        // click hook stays out: it would swallow every `target="_blank"`
        // link and send it to `plugin:opener|open_url`, which the webview
        // may not call – links take the navigation hook in
        // `build_main_window` instead.
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        // The cloud-folder sync's Kotlin plugin and the Keystore plugin for
        // the server password (Rust-only as well).
        .plugin(crate::mobile::sync::plugin())
        .plugin(crate::mobile::secret::plugin());
    // Fingerprint unlock: the Android Keystore behind the system prompt
    // (`mobile::biometric`; Rust-only as well – no `biometric:` grant).
    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_biometric::init());
    builder
        .plugin(tauri_plugin_dialog::init())
        // `#[cfg(desktop)]`: not registered on the phone (the feature is
        // desktop-only); `#[cfg(mobile)]`: the phone's own (cloud-folder
        // sync, fingerprint unlock). The unit test below keeps this list and
        // the table in docs/ARCHITECTURE.md in step.
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_vaults,
            commands::session_state,
            commands::create_vault,
            commands::unlock_vault,
            #[cfg(desktop)]
            commands::unlock_with_recovery,
            commands::lock_vault,
            commands::touch_activity,
            commands::list_items,
            commands::list_folders,
            commands::save_item,
            crate::secrets::reveal_secret,
            crate::secrets::copy_secret_field,
            crate::secrets::totp_for_item,
            crate::secrets::get_item_for_edit,
            crate::secrets::set_favorite,
            #[cfg(desktop)]
            crate::attachments::add_attachment,
            #[cfg(desktop)]
            crate::attachments::save_attachment,
            #[cfg(desktop)]
            crate::attachments::remove_attachment,
            crate::attachments::reveal_attachment,
            commands::trash_item,
            commands::restore_item,
            commands::delete_item,
            commands::empty_trash,
            commands::save_folder,
            commands::delete_folder,
            commands::rename_tag,
            commands::delete_tag,
            crate::generator::generate_password,
            crate::generator::remember_generated,
            crate::generator::generator_history,
            crate::generator::reveal_generated,
            crate::generator::copy_generated,
            crate::generator::clear_generator_history,
            crate::generator::generator_presets,
            crate::generator::save_generator_preset,
            crate::generator::delete_generator_preset,
            crate::generator::set_default_generator_preset,
            commands::password_strength,
            commands::totp_code,
            commands::copy_text,
            commands::health_report,
            #[cfg(desktop)]
            commands::change_master_password,
            #[cfg(desktop)]
            commands::create_recovery_key,
            #[cfg(desktop)]
            commands::remove_recovery_key,
            commands::rename_vault,
            #[cfg(desktop)]
            commands::delete_vault,
            #[cfg(desktop)]
            commands::legacy_scan,
            #[cfg(desktop)]
            commands::import_data,
            #[cfg(desktop)]
            import_flow::analyze_import,
            #[cfg(desktop)]
            import_flow::commit_import,
            #[cfg(desktop)]
            import_flow::cancel_import,
            #[cfg(desktop)]
            import_flow::show_export,
            #[cfg(desktop)]
            commands::export_data,
            commands::get_settings,
            commands::save_settings,
            #[cfg(desktop)]
            crate::mirror::mirror_status,
            #[cfg(desktop)]
            crate::mirror::folder_offers,
            #[cfg(desktop)]
            crate::mirror::adopt_folder_vault,
            #[cfg(desktop)]
            crate::mirror::dismiss_folder_vault,
            #[cfg(desktop)]
            crate::mirror::folder_conflict_copies,
            #[cfg(desktop)]
            crate::mirror::restore_folder_conflict_copy,
            #[cfg(desktop)]
            crate::mirror::dismiss_folder_conflict_copy,
            crate::sync_conflict::sync_conflicts,
            crate::sync_conflict::resolve_sync_conflict,
            crate::sync_server::webdav_test,
            crate::sync_server::webdav_create_folder,
            #[cfg(desktop)]
            crate::mirror::set_sync_server,
            #[cfg(desktop)]
            crate::mirror::clear_sync_server,
            #[cfg(desktop)]
            commands::browser_status,
            #[cfg(desktop)]
            commands::register_browsers,
            #[cfg(desktop)]
            commands::unregister_browsers,
            #[cfg(desktop)]
            commands::revoke_client,
            #[cfg(desktop)]
            commands::respond_pairing,
            #[cfg(desktop)]
            commands::open_terminal,
            #[cfg(desktop)]
            commands::open_data_dir,
            #[cfg(desktop)]
            commands::open_extension_dir,
            #[cfg(desktop)]
            commands::set_portable_mode,
            #[cfg(desktop)]
            commands::check_update,
            #[cfg(desktop)]
            commands::pending_update,
            #[cfg(desktop)]
            commands::install_update,
            crate::icons::get_icons,
            #[cfg(desktop)]
            crate::icons::clear_icons,
            #[cfg(desktop)]
            crate::pwned::check_pwned_password,
            #[cfg(desktop)]
            crate::pwned::check_pwned_vault,
            #[cfg(mobile)]
            crate::mobile::sync::folder_status,
            #[cfg(mobile)]
            crate::mobile::sync::link_folder,
            #[cfg(mobile)]
            crate::mobile::sync::link_webdav,
            #[cfg(mobile)]
            crate::mobile::sync::sync_now,
            #[cfg(mobile)]
            crate::mobile::sync::unlink_folder,
            #[cfg(mobile)]
            crate::mobile::biometric::device_unlock_status,
            #[cfg(mobile)]
            crate::mobile::biometric::enable_device_unlock,
            #[cfg(mobile)]
            crate::mobile::biometric::disable_device_unlock,
            #[cfg(mobile)]
            crate::mobile::biometric::unlock_with_device,
        ])
        .setup(move |app| {
            setup(app.handle(), background);
            Ok(())
        })
        .on_window_event(on_window_event)
        .build(tauri::generate_context!())
}

/// Desktop: closing the window hides it when "minimize to tray" is on;
/// getting the focus has the cloud-folder mirror look through its folder
/// (`mirror::Job::Scan` – the phone may have written meanwhile). Phone: `Suspended`/`Resumed` (the activity's `onPause`/`onResume`)
/// drive the background lock in the monitor, the UI's `app://suspended`
/// and the cloud-folder sync (pushes flushed on `Suspended`, a sync on
/// `Resumed`).
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    #[cfg(desktop)]
    if let WindowEvent::CloseRequested { api, .. } = event {
        let Some(core) = window.app_handle().try_state::<Arc<Core>>() else {
            return;
        };
        // Without a tray icon a hidden window could not be reopened.
        if core.minimize_to_tray() && core.tray_available() {
            api.prevent_close();
            let _ = window.hide();
        }
    }
    #[cfg(desktop)]
    if let WindowEvent::Focused(true) = event {
        if let Some(core) = window.app_handle().try_state::<Arc<Core>>() {
            core.mirror().request(crate::mirror::Job::Scan);
        }
    }
    #[cfg(mobile)]
    {
        let Some(core) = window.app_handle().try_state::<Arc<Core>>() else {
            return;
        };
        match event {
            WindowEvent::Suspended => {
                // The page first (it may be paused any moment), then the
                // monitor's clock.
                core.emit_suspended();
                core.signal_monitor(MonitorSignal::Background);
                // Edits waiting for their push go to the folder now (the
                // process may be frozen or ended in the background).
                crate::mobile::sync::flush_pushes(window.app_handle());
            }
            WindowEvent::Resumed => {
                core.signal_monitor(MonitorSignal::Foreground);
                // Pick up what the PC saved meanwhile (throttled, own thread).
                crate::mobile::sync::auto_sync(window.app_handle());
            }
            _ => {}
        }
    }
}

/// Creates the state, window, tray and background services. Errors are
/// logged; only a missing main window is fatal.
fn setup(app: &AppHandle, background: bool) {
    // The phone's data directory and clipboard backend: before the
    // settings or the store are touched.
    #[cfg(mobile)]
    crate::mobile::prepare(app);

    let settings = Settings::load();
    let store = VaultStore::open_default().unwrap_or_else(|e| {
        log(format_args!("data directory not usable: {}", e.code()));
        VaultStore::new(paths::data_dir())
    });
    let core = Core::new(app.clone(), store, settings.clone());
    app.manage(Arc::clone(&core));
    // The server password is sealed by the Android Keystore from now on.
    #[cfg(mobile)]
    crate::mobile::secret::install(app, &core);
    // The cloud-folder mirror's worker: before anything can open a vault.
    #[cfg(desktop)]
    crate::mirror::spawn(&core);
    // The linked cloud folder, and the first sync of this start.
    #[cfg(mobile)]
    crate::mobile::sync::start(app, &core);

    let window = match build_main_window(app, &core) {
        Ok(window) => window,
        Err(e) => {
            log(format_args!("could not create the main window: {e}"));
            app.exit(1);
            return;
        }
    };

    #[cfg(desktop)]
    core.set_tray(tray::create(app, settings.language));

    let (signals_tx, signals_rx) = mpsc::channel();
    core.set_monitor(signals_tx);
    #[cfg(desktop)]
    platform::watch_session_events(&window, &core);
    monitor::spawn(Arc::downgrade(&core), signals_rx);

    #[cfg(desktop)]
    {
        bridge::start_if_enabled(&core);
        if background && !bridge::is_running(&core) {
            // Only the native host starts the app with --background, to reach
            // the bridge. Without a running bridge (browser integration off, or
            // the endpoint is served elsewhere) the host cannot use this
            // instance, and a hidden window would just linger unnoticed.
            log("started in the background, but the browser bridge is not running: exiting");
            app.exit(0);
            return;
        }
        if settings.browser_integration {
            // Registry/file work off the main thread.
            let _ = std::thread::Builder::new()
                .name("keystead-register".into())
                .spawn(bridge::reregister_if_needed);
        }
        // The browser extension the app delivers: (re)write its folder when the
        // embedded version differs (file work off the main thread).
        let data_dir = core.data_dir();
        let _ = std::thread::Builder::new()
            .name("keystead-extension".into())
            .spawn(move || extension::deploy_logged(&data_dir));
        update::spawn_background_checks(app.clone(), &core);
        // Website icons: fetched in the background while a vault is open.
        crate::icons::spawn_scheduler(&core);
    }

    // Start hidden for `--background` (native host) and "start in tray" –
    // the latter only if there is a tray icon to bring the window back.
    #[cfg(desktop)]
    let hidden = background || (settings.start_in_tray && core.tray_available());
    #[cfg(mobile)]
    let hidden = {
        let _ = background;
        false
    };
    if !hidden {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Origins the main webview may navigate to (the bundled UI, and the Vite
/// dev server in `tauri dev`).
fn is_app_url(url: &Url, dev_url: Option<&Url>) -> bool {
    match url.scheme() {
        "tauri" | "about" => true,
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || dev_url.is_some_and(|dev| {
                    dev.scheme() == url.scheme()
                        && dev.host_str() == url.host_str()
                        && dev.port_or_known_default() == url.port_or_known_default()
                })
        }
        _ => false,
    }
}

fn build_main_window(app: &AppHandle, core: &Arc<Core>) -> tauri::Result<WebviewWindow> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW)
        .cloned()
        .ok_or_else(|| {
            tauri::Error::Io(std::io::Error::other(
                "main window missing in tauri.conf.json",
            ))
        })?;
    let dev_url = if cfg!(dev) {
        app.config().build.dev_url.clone()
    } else {
        None
    };
    let page_core = Arc::downgrade(core);
    let navigation_app = app.clone();
    let new_window_app = app.clone();
    WebviewWindowBuilder::from_config(app, &config)?
        // Revealed passwords, card numbers and the recovery key must not end
        // up in screenshots, recordings, screen sharing or Windows Recall
        // (`WDA_EXCLUDEFROMCAPTURE`; also set in tauri.conf.json). A no-op
        // on Android, where the activity sets `FLAG_SECURE` instead.
        .content_protected(true)
        // Links and `window.open` go to the system browser, never into a
        // webview; the app itself never navigates away from its UI.
        .on_navigation(move |url| {
            if is_app_url(url, dev_url.as_ref()) {
                return true;
            }
            platform::open_url(&navigation_app, url);
            false
        })
        .on_new_window(move |url, _features| {
            platform::open_url(&new_window_app, &url);
            NewWindowResponse::Deny
        })
        .on_page_load(move |_window, payload| {
            // `Started`: the page's first `session_state` call may come
            // before the load has finished.
            if payload.event() == PageLoadEvent::Started {
                if let Some(core) = page_core.upgrade() {
                    core.mark_page_loaded();
                }
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn main_window_is_excluded_from_screen_capture() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = config["app"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["label"] == MAIN_WINDOW)
            .unwrap();
        assert_eq!(main["contentProtected"], serde_json::Value::Bool(true));
    }

    #[test]
    fn updater_is_configured_for_signed_passive_updates() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["bundle"]["createUpdaterArtifacts"], true);
        let updater = &config["plugins"]["updater"];
        // The public half of the release signing key (minisign); the private
        // key only exists as the CI secret TAURI_SIGNING_PRIVATE_KEY.
        assert!(
            updater["pubkey"].as_str().is_some_and(
                |k| k.starts_with("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6")
            )
        );
        assert_eq!(updater["requireSignedVersion"], true);
        assert_eq!(updater["windows"]["installMode"], "passive");
        // Endpoints are chosen at runtime by channel (`update::endpoint`).
        assert!(updater.get("endpoints").is_none());
        // The NSIS setup must keep `currentUser`: `update::is_installed_copy`
        // looks for the exe in %LOCALAPPDATA%\Keystead and under HKCU.
        assert_eq!(
            config["bundle"]["windows"]["nsis"]["installMode"],
            "currentUser"
        );
        assert_eq!(config["productName"], "Keystead");
    }

    /// The desktop's and the phone's capability files (`capabilities/`).
    fn capabilities() -> [(&'static str, serde_json::Value); 2] {
        [
            (
                "default.json",
                serde_json::from_str(include_str!("../capabilities/default.json")).unwrap(),
            ),
            (
                "mobile.json",
                serde_json::from_str(include_str!("../capabilities/mobile.json")).unwrap(),
            ),
        ]
    }

    fn permission_ids(capability: &serde_json::Value) -> BTreeSet<String> {
        capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|permission| {
                permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn the_webview_cannot_call_the_rust_only_plugins() {
        // The UI only uses the app's own commands (check_update,
        // install_update, copy_text, …); the plugins' JS APIs stay
        // unreachable – also the phone's clipboard and opener plugins, so
        // the core keeps the whole clipboard policy and every link goes
        // through `platform::open_url`.
        for (file, capability) in capabilities() {
            for id in permission_ids(&capability) {
                for plugin in [
                    "updater:",
                    "process:",
                    "clipboard-manager:",
                    "opener:",
                    "keystead-folder:",
                    "keystead-secret:",
                    "biometric:",
                ] {
                    assert!(!id.starts_with(plugin), "{file}: {id}");
                }
            }
        }
        // The opener's injected click hook would send `target="_blank"`
        // links to its (forbidden) JS command instead of the navigation
        // hook, so it stays off.
        let source = include_str!("gui.rs");
        assert!(source.contains(concat!(".open_js_links_on_click", "(false)")));
        assert!(!source.contains(concat!("tauri_plugin_opener", "::init()")));
    }

    #[test]
    fn the_capabilities_are_split_by_platform() {
        // Tauri applies a capability only on the platforms it names
        // (`platforms`, tauri-utils `Target`: "macOS" and "iOS" are spelled
        // like that); without the field it would apply everywhere.
        let [(_, desktop), (_, mobile)] = capabilities();
        assert_eq!(
            desktop["platforms"],
            serde_json::json!(["windows", "linux", "macOS"])
        );
        assert_eq!(mobile["platforms"], serde_json::json!(["android", "iOS"]));
        for capability in [&desktop, &mobile] {
            assert_eq!(capability["windows"], serde_json::json!([MAIN_WINDOW]));
        }
        // The phone gets nothing the desktop does not have (no file
        // dialogs in this stage either).
        let desktop_ids = permission_ids(&desktop);
        let mobile_ids = permission_ids(&mobile);
        assert!(mobile_ids.is_subset(&desktop_ids), "{mobile_ids:?}");
        assert!(mobile_ids.contains("core:default"));
        assert!(mobile_ids.iter().all(|id| id.starts_with("core:")));
    }

    #[test]
    fn the_android_config_follows_the_design_decisions() {
        // tauri.android.conf.json is merged into tauri.conf.json for Android
        // builds (identifier, minimum API level, the window); everything it
        // does not mention – above all the CSP – is inherited unchanged.
        let android: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.android.conf.json")).unwrap();
        assert_eq!(android["identifier"], "io.github.cedrickgd.keystead");
        assert_eq!(android["bundle"]["android"]["minSdkVersion"], 29);
        assert!(
            android["app"].get("security").is_none(),
            "the CSP must stay the desktop's (inherited)"
        );
        let windows = android["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1);
        let main = &windows[0];
        assert_eq!(main["label"], MAIN_WINDOW);
        assert_eq!(main["title"], "Keystead");
        // `setup` builds the window itself (navigation and new-window hooks
        // route links to the system browser), so Tauri must not create it.
        assert_eq!(main["create"], false);
        // A phone window is plain: visible, no size, no desktop decoration.
        for key in [
            "visible",
            "width",
            "height",
            "minWidth",
            "minHeight",
            "center",
        ] {
            assert!(main.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn the_android_project_is_hardened() {
        // `gen/android` is generated by `tauri android init` and committed
        // with these changes; a re-run of `init` or a careless edit must not
        // lose them (see "Mobile (Android)" in docs/ARCHITECTURE.md).
        let activity = include_str!(
            "../gen/android/app/src/main/java/io/github/cedrickgd/keystead/MainActivity.kt"
        );
        assert!(activity.contains("FLAG_SECURE"));
        assert!(activity.contains("allowFileAccess = false"));
        assert!(activity.contains("allowContentAccess = false"));
        assert!(activity.contains("keystead:back"));
        assert!(activity.contains("moveTaskToBack(true)"));
        // Edge-to-edge: the webview's parent is padded by the system bars,
        // a display cutout and the keyboard (the phone layout relies on it –
        // no header under the status bar, no field under the keyboard).
        assert!(activity.contains("ViewCompat.setOnApplyWindowInsetsListener(content)"));
        for inset in ["systemBars()", "displayCutout()", "ime()"] {
            assert!(
                activity.contains(&format!("WindowInsetsCompat.Type.{inset}")),
                "{inset}"
            );
        }
        // The back callback is moved above Tauri's AppPlugin (which would
        // call `webView.goBack()` first) once the webview exists.
        let on_webview_create = &activity[activity.find("fun onWebViewCreate").unwrap()..];
        let on_webview_create = &on_webview_create[..on_webview_create.find("\n  }").unwrap()];
        assert!(on_webview_create.contains("backCallback.remove()"));
        assert!(
            on_webview_create.contains("onBackPressedDispatcher.addCallback(this, backCallback)")
        );

        // The cloud-folder plugin: read and write access (persisted, given
        // back on unlink; read only where the provider refuses write),
        // copies only into the app's sync/incoming folder, writes only of
        // `<id>.keystead` files staged in sync/outgoing – truncated and
        // rewritten or created, never deleted or renamed, never a blob –,
        // the provider work off the UI thread.
        let folder = include_str!(
            "../gen/android/app/src/main/java/io/github/cedrickgd/keystead/KeysteadFolderPlugin.kt"
        );
        assert!(folder.contains(
            "class KeysteadFolderPlugin(private val activity: Activity) : Plugin(activity)"
        ));
        assert!(folder.contains("Intent.ACTION_OPEN_DOCUMENT_TREE"));
        assert!(folder.contains("takePersistableUriPermission(tree, READ_WRITE)"));
        assert!(folder
            .contains("takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)"));
        assert!(folder.contains(
            "Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION"
        ));
        assert!(folder.contains("releasePersistableUriPermission"));
        // Writes: an existing file truncated and rewritten ("wt", synced to
        // disk), a missing one created under exactly its name.
        assert!(folder.contains(r#"copyInto(target, source, "wt")"#));
        assert!(folder.contains("openFileDescriptor(target, mode)"));
        assert!(folder.contains("fd.sync()"));
        assert!(folder.contains("DocumentsContract.createDocument("));
        assert!(folder.contains(r#""name_taken""#));
        assert!(folder.contains(r#""write_unverified""#));
        assert!(folder.contains("VAULT_FILE_NAME.matches(args.name)"));
        assert!(folder.contains("needWrite = true"));
        // The phone's conflict copies: only created, never written over.
        let copy = &folder[folder.find("fun writeConflictCopy").unwrap()..];
        let copy = &copy[..copy.find("\n  }").unwrap()];
        assert!(copy.contains("CONFLICT_COPY_NAME.matches(args.name)"));
        assert!(copy.contains("childNamed(tree, rootId, args.name) != null) throw NameTaken()"));
        assert!(copy.contains("if (nameOf(made) != args.name) throw NameTaken()"));
        assert!(copy.contains(r#"copyInto(made, source, "w")"#));
        assert!(!copy.contains(r#""wt""#));
        assert!(folder.contains(r#"-[0-9]{8}-[0-9]{6}-phone$"#));
        for never in [
            "deleteDocument",
            "renameDocument",
            "moveDocument",
            "openOutputStream",
        ] {
            assert!(
                !folder.contains(never),
                "{never}: a file of the folder is never removed, renamed or moved"
            );
        }
        assert!(!folder.contains(".attachments"), "no blob is ever written");
        assert!(folder.contains(r#"incoming.name != "incoming""#));
        assert!(folder.contains(r#"outgoing.name != "outgoing""#));
        assert!(folder.contains("Executors.newSingleThreadExecutor()"));
        for command in [
            "pickFolder",
            "releaseFolder",
            "folderAccess",
            "listChildren",
            "copyFile",
            "writeFile",
            "writeConflictCopy",
            "networkKind",
        ] {
            assert!(
                folder.contains(&format!("fun {command}(invoke: Invoke)")),
                "{command}"
            );
        }
        assert!(source_registers_the_folder_plugin());
        // The Wi-Fi check: the active network's transports, nothing more.
        let network = &folder[folder.find("fun networkKind").unwrap()..];
        let network = &network[..network.find("\n  }").unwrap()];
        assert!(network.contains("getNetworkCapabilities"));
        assert!(network.contains("TRANSPORT_WIFI"));
        assert!(network.contains("TRANSPORT_CELLULAR"));

        // The server password's plugin: AES-256-GCM in the Android
        // Keystore, no authentication per use but only while the phone is
        // unlocked, the associated data bound, nothing logged, off the UI
        // thread, Rust-only.
        let secret = include_str!(
            "../gen/android/app/src/main/java/io/github/cedrickgd/keystead/KeysteadSecretPlugin.kt"
        );
        assert!(secret.contains(
            "class KeysteadSecretPlugin(private val activity: Activity) : Plugin(activity)"
        ));
        assert!(secret.contains(r#"private const val KEYSTORE = "AndroidKeyStore""#));
        assert!(secret.contains(r#"private const val ALIAS = "keystead-sync-credential""#));
        assert!(secret.contains(r#"private const val TRANSFORMATION = "AES/GCM/NoPadding""#));
        assert!(secret.contains("KeyProperties.BLOCK_MODE_GCM"));
        assert!(secret.contains(".setKeySize(256)"));
        assert!(secret.contains(".setUserAuthenticationRequired(false)"));
        assert!(secret.contains(".setUnlockedDeviceRequired(true)"));
        assert!(secret.contains(".setRandomizedEncryptionRequired(true)"));
        assert_eq!(secret.matches("cipher.updateAAD(").count(), 2);
        assert!(secret.contains("Executors.newSingleThreadExecutor()"));
        for never in [
            "Log.",
            "println",
            "printStackTrace",
            "EncryptedSharedPreferences",
            "SharedPreferences",
            "setUserAuthenticationParameters",
            "BiometricPrompt",
        ] {
            assert!(!secret.contains(never), "{never}");
        }
        for command in ["seal", "open"] {
            assert!(
                secret.contains(&format!("fun {command}(invoke: Invoke)")),
                "{command}"
            );
        }
        let secret_rs = include_str!("mobile/secret.rs");
        assert!(secret_rs.contains(r#""io.github.cedrickgd.keystead","#));
        assert!(secret_rs.contains(r#""KeysteadSecretPlugin","#));
        assert!(secret_rs.contains(r#"pub const PLUGIN_NAME: &str = "keystead-secret";"#));

        // The network (stage 2c): exactly INTERNET (the server sync) and
        // ACCESS_NETWORK_STATE (the Wi-Fi check), no cleartext, the system's
        // trust anchors only (the sync's TLS is Rust's own anyway).
        let manifest = include_str!("../gen/android/app/src/main/AndroidManifest.xml");
        assert!(manifest.contains(r#"android:allowBackup="false""#));
        assert!(manifest.contains(r#"android:dataExtractionRules="@xml/data_extraction_rules""#));
        assert!(manifest.contains(r#"android:windowSoftInputMode="adjustResize""#));
        assert!(manifest.contains(r#"android.webkit.WebView.EnableSafeBrowsing""#));
        let permissions: BTreeSet<&str> = manifest
            .split("<uses-permission")
            .skip(1)
            .map(|rest| {
                let name = &rest[rest.find(r#"android:name=""#).unwrap() + 14..];
                &name[..name.find('"').unwrap()]
            })
            .collect();
        assert_eq!(
            permissions,
            BTreeSet::from([
                "android.permission.INTERNET",
                "android.permission.ACCESS_NETWORK_STATE",
            ])
        );
        assert_eq!(manifest.matches("<uses-permission").count(), 2);
        assert!(!manifest.contains("uses-permission-sdk-23"));
        assert!(
            manifest.contains(r#"android:networkSecurityConfig="@xml/network_security_config""#)
        );
        assert!(!manifest.contains("android:debuggable"));
        let network =
            include_str!("../gen/android/app/src/main/res/xml/network_security_config.xml");
        assert!(network.contains(r#"<base-config cleartextTrafficPermitted="false">"#));
        assert!(network.contains(r#"<certificates src="system" />"#));
        assert!(!network.contains(r#"src="user""#));
        assert!(!network.contains("<domain-config"));
        assert!(!network.contains("<debug-overrides"));
        assert_eq!(network.matches("<certificates").count(), 1);
        // Debug builds (`tauri android dev` loads the UI from the Vite dev
        // server over plain HTTP) add no permission, only cleartext.
        let debug = include_str!("../gen/android/app/src/debug/AndroidManifest.xml");
        assert!(!debug.contains("<uses-permission"));
        let debug_network =
            include_str!("../gen/android/app/src/debug/res/xml/network_security_config.xml");
        assert!(debug_network.contains(r#"<base-config cleartextTrafficPermitted="true">"#));
        assert!(debug_network.contains(r#"<certificates src="system" />"#));
        assert!(!debug_network.contains(r#"src="user""#));

        let rules = include_str!("../gen/android/app/src/main/res/xml/data_extraction_rules.xml");
        assert!(rules.contains("<cloud-backup"));
        assert!(rules.contains("<device-transfer>"));
        assert!(!rules.contains("<include"));
        // Every domain excluded in both sections – only names Android
        // knows (an unknown one is skipped silently): the credential-
        // encrypted storage and the device-protected (`device_*`) one.
        let domains = [
            "root",
            "file",
            "database",
            "sharedpref",
            "external",
            "device_root",
            "device_file",
            "device_database",
            "device_sharedpref",
        ];
        for domain in domains {
            let needle = format!(r#"<exclude domain="{domain}" />"#);
            assert_eq!(rules.matches(&needle).count(), 2, "{domain}");
        }
        assert_eq!(
            rules.matches("<exclude domain=").count(),
            2 * domains.len(),
            "no other domain"
        );

        let gradle = include_str!("../gen/android/app/build.gradle.kts");
        assert!(gradle.contains("minSdk = 29"));
        assert!(gradle.contains(r#"applicationId = "io.github.cedrickgd.keystead""#));
        assert!(gradle.contains(r#"namespace = "io.github.cedrickgd.keystead""#));
        assert!(gradle.contains(r#"rootProject.file("keystore.properties")"#));
        // The keystore never enters the repository: refused inside the
        // checkout, and ignored here and in the root .gitignore.
        assert!(gradle.contains("storeFile must lie outside the repository"));
        let ignored = include_str!("../gen/android/.gitignore");
        let root_ignored = include_str!("../../../../.gitignore");
        for entry in [
            "keystore.properties",
            "local.properties",
            "build",
            ".gradle",
            "*.jks",
            "*.keystore",
            "*.p12",
        ] {
            assert!(ignored.lines().any(|line| line == entry), "{entry}");
        }
        for entry in ["*.jks", "*.keystore", "*.p12"] {
            assert!(root_ignored.lines().any(|line| line == entry), "{entry}");
        }
        let app_ignored = include_str!("../gen/android/app/.gitignore");
        assert!(app_ignored.lines().any(|line| line == "/tauri.properties"));
    }

    /// `mobile/sync.rs` loads the Kotlin class under the app's package.
    fn source_registers_the_folder_plugin() -> bool {
        let source = include_str!("mobile/sync.rs");
        source.contains(r#""io.github.cedrickgd.keystead","#)
            && source.contains(r#""KeysteadFolderPlugin","#)
            && source.contains(r#"pub const PLUGIN_NAME: &str = "keystead-folder";"#)
    }

    #[test]
    fn app_urls() {
        let u = |s: &str| Url::parse(s).unwrap();
        let dev = u("http://localhost:1420");
        assert!(is_app_url(&u("tauri://localhost/index.html"), None));
        assert!(is_app_url(&u("http://tauri.localhost/"), None));
        assert!(is_app_url(&u("http://localhost:1420/#x"), Some(&dev)));
        assert!(!is_app_url(&u("http://localhost:1420/"), None));
        assert!(!is_app_url(&u("http://localhost:8080/"), Some(&dev)));
        assert!(!is_app_url(&u("https://example.com/"), Some(&dev)));
        assert!(!is_app_url(&u("file:///etc/passwd"), None));
        assert!(!is_app_url(&u("javascript:alert(1)"), None));
    }

    /// Where a command of the handler list is registered.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Registered {
        Both,
        DesktopOnly,
        PhoneOnly,
    }

    /// The entries of this file's `generate_handler!` list as
    /// (command name, where).
    fn handler_entries() -> Vec<(String, Registered)> {
        let source = include_str!("gui.rs");
        let start = source.find("generate_handler![").unwrap() + "generate_handler![".len();
        let body = &source[start..];
        let body = &body[..body.find("])").unwrap()];
        let mut entries = Vec::new();
        let mut gate = Registered::Both;
        for line in body.lines().map(str::trim) {
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            match line {
                "#[cfg(desktop)]" => {
                    gate = Registered::DesktopOnly;
                    continue;
                }
                "#[cfg(mobile)]" => {
                    gate = Registered::PhoneOnly;
                    continue;
                }
                _ => {}
            }
            assert!(!line.starts_with('#'), "unexpected attribute: {line}");
            let path = line.trim_end_matches(',');
            let name = path.rsplit("::").next().unwrap().to_owned();
            entries.push((name, gate));
            gate = Registered::Both;
        }
        entries
    }

    /// The backticked command names in the first cell of every row of the
    /// command table in docs/ARCHITECTURE.md, and the ones its "On the
    /// phone" paragraph lists.
    fn documented_commands() -> (BTreeSet<String>, BTreeSet<String>) {
        let docs = include_str!("../../../../docs/ARCHITECTURE.md");
        let section = docs
            .split("## Desktop backend ↔ frontend (Tauri commands)")
            .nth(1)
            .unwrap();
        let section = &section[..section.find("**Events**").unwrap()];
        let names = |text: &str| -> BTreeSet<String> {
            text.split('`')
                .skip(1)
                .step_by(2)
                .filter(|t| !t.is_empty() && t.bytes().all(|b| b == b'_' || b.is_ascii_lowercase()))
                .map(str::to_owned)
                .collect()
        };
        let mut table = BTreeSet::new();
        let mut phone = BTreeSet::new();
        for line in section.lines() {
            if line.starts_with("| `") {
                let first_cell = line.trim_start_matches('|').split('|').next().unwrap();
                table.extend(names(first_cell));
            } else if line.starts_with("**On the phone**") {
                phone = names(line);
            }
        }
        (table, phone)
    }

    #[test]
    fn the_handler_lists_match_the_architecture_table() {
        let entries = handler_entries();
        let registered: BTreeSet<String> = entries.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(registered.len(), entries.len(), "duplicate entries");
        let (table, phone_documented) = documented_commands();
        // Every command is documented, and nothing undocumented is
        // registered on either side.
        assert_eq!(registered, table);
        // The phone registers exactly the documented subset: the vault
        // commands without the desktop-only ones, plus its own.
        let phone: BTreeSet<String> = entries
            .iter()
            .filter(|(_, gate)| *gate != Registered::DesktopOnly)
            .map(|(n, _)| n.clone())
            .collect();
        assert!(!phone.is_empty());
        assert_eq!(phone, phone_documented);
        let phone_only: BTreeSet<&str> = entries
            .iter()
            .filter(|(_, gate)| *gate == Registered::PhoneOnly)
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(
            phone_only,
            BTreeSet::from([
                "folder_status",
                "link_folder",
                "link_webdav",
                "sync_now",
                "unlink_folder",
                "device_unlock_status",
                "enable_device_unlock",
                "disable_device_unlock",
                "unlock_with_device",
            ])
        );
        // The phone edits items, folders, tags and the vault's name and
        // creates vaults ...
        for name in [
            "create_vault",
            "save_item",
            "set_favorite",
            "trash_item",
            "delete_item",
            "rename_vault",
            "generate_password",
            "totp_code",
            "webdav_test",
            "webdav_create_folder",
        ] {
            assert!(phone.contains(name), "{name} must be on the phone");
        }
        // ... but never changes what opens a vault, deletes one, changes
        // attachments, opens a file dialog or needs the network.
        for name in [
            "change_master_password",
            "create_recovery_key",
            "remove_recovery_key",
            "unlock_with_recovery",
            "delete_vault",
            "import_data",
            "analyze_import",
            "export_data",
            "add_attachment",
            "save_attachment",
            "remove_attachment",
            "check_update",
            "check_pwned_vault",
            "browser_status",
            "set_sync_server",
            "clear_sync_server",
        ] {
            assert!(!phone.contains(name), "{name} must stay desktop-only");
        }
    }
}
