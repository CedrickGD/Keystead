//! Phone-only start-up (Android, iOS): the data directory, diagnostics and
//! the clipboard backend. Runs as the first thing in `gui::setup`, before
//! the settings or a vault store are opened.
//!
//! * **Data directory**: a phone has neither an environment nor a usable OS
//!   default, so the app's private folder (Tauri `app_local_data_dir()`:
//!   Android `<app data>/<identifier>`) becomes the process-wide override
//!   of `keystead_core::paths` (see "Data locations" in
//!   docs/ARCHITECTURE.md).
//! * **Diagnostics**: `state::log` writes to stderr; on Android the
//!   activity runtime forwards stdout/stderr to logcat (tag
//!   `RustStdoutStderr`), so nothing extra is installed. Never log secrets.
//! * **Clipboard**: `keystead-core` has no clipboard backend on the phone
//!   (arboard is desktop-only); [`PluginClipboard`] provides one through
//!   `tauri-plugin-clipboard-manager`, called from Rust only – the webview
//!   gets no permission for the plugin. The core keeps the whole policy
//!   (sensitive copies, timed clear, clear on lock); the backend only
//!   knows whether the app is in the background ([`set_in_background`]),
//!   because Android lets a background app write the clipboard but not
//!   read or query it.

use std::sync::atomic::{AtomicBool, Ordering};

use keystead_core::clipboard::{self, ClipboardBackend};
use keystead_core::{paths, Error as CoreError};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt as _;

use crate::state::log;

/// Sets the data directory and installs the clipboard backend. Must run
/// before `Settings::load()` / `VaultStore::open_default()`.
pub fn prepare(app: &AppHandle) {
    match app.path().app_local_data_dir() {
        Ok(dir) => {
            // Only the first call counts; a second `prepare` (never
            // expected) keeps the directory already in use.
            if !paths::set_data_dir_override(dir) {
                log("data directory already set; keeping it");
            }
        }
        Err(e) => log(format_args!("no app data directory: {e}")),
    }
    if !clipboard::install_backend(Box::new(PluginClipboard { app: app.clone() })) {
        log("clipboard backend already installed; keeping it");
    }
}

/// Whether the app is in the background (`WindowEvent::Suspended` until
/// `Resumed`, set by `gui::on_window_event`).
static IN_BACKGROUND: AtomicBool = AtomicBool::new(false);

/// Tells the clipboard backend whether the app is in the background.
pub fn set_in_background(background: bool) {
    IN_BACKGROUND.store(background, Ordering::SeqCst);
}

/// The phone's clipboard through `tauri-plugin-clipboard-manager`.
///
/// * A sensitive text goes through `write_secret`: Android 13+ marks the
///   clip `EXTRA_IS_SENSITIVE` (hidden from the clipboard preview), iOS
///   keeps it off the Universal Clipboard.
/// * Android ≥ 10 refuses clipboard reads from an app in the background, so
///   this backend never reads: [`ClipboardBackend::clear_if_equals`] clears
///   unconditionally. The core only calls it for a secret the app copied
///   itself (its timer ran out, or the vault was locked); at worst something
///   the user copied in the meantime is lost – better than a secret that
///   stays.
/// * Clearing from the background (the usual case: the timer fires while
///   the user pastes in the other app): Android allows `setPrimaryClip`
///   without focus but answers `hasPrimaryClip()` with false – and the
///   plugin's `clear` only calls `clearPrimaryClip()` after that check, so
///   from the background it would do nothing. The backend therefore
///   overwrites the clip with an empty text first (which always works) and
///   clears afterwards; in the foreground the plain clear suffices and
///   avoids the system's "copied" bubble for the empty write.
struct PluginClipboard {
    app: AppHandle,
}

fn plugin_error(e: tauri_plugin_clipboard_manager::Error) -> CoreError {
    CoreError::Io(std::io::Error::other(format!("clipboard: {e}")))
}

impl ClipboardBackend for PluginClipboard {
    fn write_text(&self, text: &str, sensitive: bool) -> keystead_core::Result<()> {
        let clipboard = self.app.clipboard();
        let written = if sensitive {
            clipboard.write_secret(text)
        } else {
            clipboard.write_text(text)
        };
        written.map_err(plugin_error)
    }

    fn read_text(&self) -> keystead_core::Result<Option<String>> {
        // Reads are refused in the background (and never needed: see the
        // type's docs).
        Ok(None)
    }

    fn clear(&self) -> keystead_core::Result<()> {
        let clipboard = self.app.clipboard();
        if IN_BACKGROUND.load(Ordering::SeqCst) {
            // See the type's docs: the real removal from the background.
            clipboard.write_text("").map_err(plugin_error)?;
        }
        clipboard.clear().map_err(plugin_error)
    }

    fn clear_if_equals(&self, _expected: &str) -> keystead_core::Result<bool> {
        self.clear()?;
        Ok(true)
    }
}
