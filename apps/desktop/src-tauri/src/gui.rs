//! GUI mode: Tauri setup, main window, tray, single instance, background
//! services and shutdown.

use std::sync::mpsc;
use std::sync::Arc;

use keystead_core::settings::Settings;
use keystead_core::{paths, VaultStore};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::state::{log, show_main_window, Core, MAIN_WINDOW};
use crate::{bridge, commands, monitor, platform, tray, BACKGROUND_ARG};

/// Runs the desktop app; returns the process exit code.
pub fn run(background: bool) -> i32 {
    let app = tauri::Builder::default()
        // Must be the first plugin: a second instance exits right here.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second `--background` launch (from the native host) must
            // not pop up the window; a normal launch brings it to front.
            if !argv.iter().skip(1).any(|a| a == BACKGROUND_ARG) {
                show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_vaults,
            commands::session_state,
            commands::create_vault,
            commands::unlock_vault,
            commands::unlock_with_recovery,
            commands::lock_vault,
            commands::touch_activity,
            commands::list_items,
            commands::list_folders,
            commands::save_item,
            commands::trash_item,
            commands::restore_item,
            commands::delete_item,
            commands::empty_trash,
            commands::save_folder,
            commands::delete_folder,
            commands::generate_password,
            commands::generator_history,
            commands::clear_generator_history,
            commands::password_strength,
            commands::totp_code,
            commands::copy_text,
            commands::health_report,
            commands::change_master_password,
            commands::create_recovery_key,
            commands::remove_recovery_key,
            commands::rename_vault,
            commands::delete_vault,
            commands::legacy_scan,
            commands::import_data,
            commands::export_data,
            commands::get_settings,
            commands::save_settings,
            commands::browser_status,
            commands::register_browsers,
            commands::unregister_browsers,
            commands::revoke_client,
            commands::respond_pairing,
            commands::open_terminal,
            commands::open_data_dir,
            commands::set_portable_mode,
        ])
        .setup(move |app| {
            setup(app.handle(), background);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW {
                return;
            }
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
        })
        .build(tauri::generate_context!());

    let app = match app {
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

/// Creates the state, window, tray and background services. Errors are
/// logged; only a missing main window is fatal.
fn setup(app: &AppHandle, background: bool) {
    let settings = Settings::load();
    let store = VaultStore::open_default().unwrap_or_else(|e| {
        log(format_args!("data directory not usable: {}", e.code()));
        VaultStore::new(paths::data_dir())
    });
    let core = Core::new(app.clone(), store, settings.clone());
    app.manage(Arc::clone(&core));

    let window = match build_main_window(app, &core) {
        Ok(window) => window,
        Err(e) => {
            log(format_args!("could not create the main window: {e}"));
            app.exit(1);
            return;
        }
    };

    core.set_tray(tray::create(app, settings.language));

    let (signals_tx, signals_rx) = mpsc::channel();
    core.set_monitor(signals_tx);
    platform::watch_session_events(&window, &core);
    monitor::spawn(Arc::downgrade(&core), signals_rx);

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

    // Start hidden for `--background` (native host) and "start in tray" –
    // the latter only if there is a tray icon to bring the window back.
    let hidden = background || (settings.start_in_tray && core.tray_available());
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
    WebviewWindowBuilder::from_config(app, &config)?
        // Revealed passwords, card numbers and the recovery key must not end
        // up in screenshots, recordings, screen sharing or Windows Recall
        // (`WDA_EXCLUDEFROMCAPTURE`; also set in tauri.conf.json).
        .content_protected(true)
        // Links and `window.open` go to the system browser, never into a
        // webview; the app itself never navigates away from its UI.
        .on_navigation(move |url| {
            if is_app_url(url, dev_url.as_ref()) {
                return true;
            }
            platform::open_url(url);
            false
        })
        .on_new_window(|url, _features| {
            platform::open_url(&url);
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
}
