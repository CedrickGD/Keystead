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
//! * **Cloud-folder sync** (`sync`): the Kotlin folder plugin, the sync
//!   commands, the automatic sync at start and resume and the push after
//!   an edit (two-way, see `crate::folder_sync`) – or a WebDAV server's
//!   folder (stage 2c, `crate::sync_server`).
//! * **Server password** (`secret`): sealed by the Android Keystore through
//!   the Kotlin plugin `KeysteadSecretPlugin` (see `crate::credentials`).
//! * **Fingerprint unlock** (`biometric`): the Android Keystore through
//!   `tauri-plugin-biometric` and the device-unlock commands (see
//!   `crate::device_unlock`).
//! * **Clipboard**: `keystead-core` has no clipboard backend on the phone
//!   (arboard is desktop-only); [`PluginClipboard`] provides one through
//!   `tauri-plugin-clipboard-manager`, called from Rust only – the webview
//!   gets no permission for the plugin. The core keeps the whole policy
//!   (sensitive copies, timed clear, clear on lock); the backend only
//!   knows that Android lets an app without window focus write the
//!   clipboard but not read or query it.

use keystead_core::clipboard::{self, ClipboardBackend};
use keystead_core::{paths, Error as CoreError};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt as _;

use crate::state::log;

pub mod biometric;
pub mod secret;
pub mod sync;

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

/// The phone's clipboard through `tauri-plugin-clipboard-manager`.
///
/// * A sensitive text goes through `write_secret`: Android 13+ marks the
///   clip `EXTRA_IS_SENSITIVE` (hidden from the clipboard preview), iOS
///   keeps it off the Universal Clipboard.
/// * Android ≥ 10 refuses clipboard reads from an app without window
///   focus, so this backend never reads:
///   [`ClipboardBackend::clear_if_equals`] clears unconditionally. The core
///   only calls it for a secret the app copied itself (its timer ran out,
///   or the vault was locked); at worst something the user copied in the
///   meantime is lost – better than a secret that stays.
/// * Clearing always overwrites the clip with an empty text first, then
///   calls the plugin's `clear`. Android allows `setPrimaryClip` without
///   window focus but answers `hasPrimaryClip()` with false – and the
///   plugin's `clear` only calls `clearPrimaryClip()` after that check, so
///   without focus it would silently do nothing while the core has already
///   dropped the pending secret (nothing would retry). Focus is not the
///   lifecycle: the app is also unfocused while resumed – right after
///   `Resumed` before the window gets focus (the monitor's catch-up clear
///   runs then), in split screen or freeform with the other app focused,
///   under the notification shade. The price: Android 13+ may show its
///   "copied" bubble for the empty write while Keystead is in front.
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
        // See the type's docs: the overwrite is the removal that works
        // without window focus; the clear afterwards drops the empty clip
        // where it can. Both run even if the first fails.
        let overwritten = clipboard.write_text("").map_err(plugin_error);
        let cleared = clipboard.clear().map_err(plugin_error);
        overwritten.and(cleared)
    }

    fn clear_if_equals(&self, _expected: &str) -> keystead_core::Result<bool> {
        self.clear()?;
        Ok(true)
    }
}
