//! System tray icon: "Keystead öffnen" / "Sperren" / "Beenden" (labels follow
//! the language setting); a left click opens the window.

use std::sync::Arc;

use keystead_core::settings::Language;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::state::{log, show_main_window, Core, LockReason};

const TRAY_ID: &str = "keystead-tray";
const MENU_OPEN: &str = "tray-open";
const MENU_LOCK: &str = "tray-lock";
const MENU_QUIT: &str = "tray-quit";

/// The tray icon and its menu entries (kept to update their labels).
pub struct TrayMenu {
    _icon: TrayIcon<Wry>,
    open: MenuItem<Wry>,
    lock: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

struct Labels {
    open: &'static str,
    lock: &'static str,
    quit: &'static str,
    tooltip: &'static str,
}

fn labels(language: Language) -> Labels {
    match language {
        Language::De => Labels {
            open: "Keystead öffnen",
            lock: "Sperren",
            quit: "Beenden",
            tooltip: "Keystead",
        },
        Language::En => Labels {
            open: "Open Keystead",
            lock: "Lock",
            quit: "Quit",
            tooltip: "Keystead",
        },
    }
}

impl TrayMenu {
    /// Switches the menu labels to `language`.
    pub fn set_language(&self, language: Language) {
        let l = labels(language);
        for (item, text) in [
            (&self.open, l.open),
            (&self.lock, l.lock),
            (&self.quit, l.quit),
        ] {
            if let Err(e) = item.set_text(text) {
                log(format_args!("could not update the tray menu: {e}"));
            }
        }
    }
}

/// Creates the tray icon. Returns `None` (logged) if the platform has no
/// usable tray, e.g. a Linux desktop without AppIndicator support.
pub fn create(app: &AppHandle, language: Language) -> Option<TrayMenu> {
    if !crate::platform::tray_supported() {
        log("no system tray support (AppIndicator library missing); running without tray icon");
        return None;
    }
    match build(app, language) {
        Ok(menu) => Some(menu),
        Err(e) => {
            log(format_args!("could not create the tray icon: {e}"));
            None
        }
    }
}

fn build(app: &AppHandle, language: Language) -> tauri::Result<TrayMenu> {
    let l = labels(language);
    let open = MenuItem::with_id(app, MENU_OPEN, l.open, true, None::<&str>)?;
    let lock = MenuItem::with_id(app, MENU_LOCK, l.lock, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, l.quit, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &lock, &separator, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(l.tooltip)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_event);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app)?;
    Ok(TrayMenu {
        _icon: icon,
        open,
        lock,
        quit,
    })
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_OPEN => show_main_window(app),
        MENU_LOCK => {
            // Runs on the main thread: never wait for the state lock here.
            if let Some(core) = app.try_state::<Arc<Core>>() {
                let core = Arc::clone(core.inner());
                std::thread::spawn(move || {
                    core.lock(Some(LockReason::Manual));
                });
            }
        }
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

fn on_tray_event(tray: &TrayIcon<Wry>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_main_window(tray.app_handle());
    }
}
