//! The Keystead app as a library – one crate for the desktop and the phone
//! (see "Executable modes" in docs/ARCHITECTURE.md).
//!
//! * Desktop: `Keystead(.exe)` (`main.rs`) sorts out the native host and
//!   the terminal UI, then calls [`run_desktop`].
//! * Phone: the Android/iOS shell loads this library and calls [`run`]
//!   (`tauri::mobile_entry_point`), which starts the same Tauri app without
//!   the desktop-only parts – tray, single instance, updater, browser
//!   bridge, extension, portable mode, website-icon fetcher, leaked-password
//!   check, the file-dialog based attachment commands (the image preview
//!   stays) and import/export. In this stage the phone is **read-only**:
//!   only the read commands are registered (`gui.rs`).
//!
//! `#[cfg(desktop)]` / `#[cfg(mobile)]` are the aliases `tauri_build` sets
//! from the target OS.

// Windows: the unit-test executable of this library links the app's
// resource file (icon, version info, application manifest) that `build.rs`
// compiled – the manifest's Common Controls v6 dependency is what the
// dialog plugin's imports resolve against; without it the executable does
// not start. The binaries get the same file from `tauri_build` directly.
#[cfg(all(test, windows))]
#[link(name = "resource", kind = "static")]
extern "C" {}

mod attachments;
#[cfg(desktop)]
mod bridge;
mod commands;
mod error;
#[cfg(desktop)]
mod extension;
// The phone's cloud-folder sync (copy-in); platform-neutral, so its unit
// tests run on the desktop host too.
#[cfg(any(mobile, test))]
mod folder_sync;
mod gui;
mod icons;
#[cfg(desktop)]
mod import_flow;
#[cfg(mobile)]
mod mobile;
mod monitor;
mod platform;
#[cfg(desktop)]
mod portable;
#[cfg(desktop)]
mod pwned;
mod secrets;
mod state;
#[cfg(desktop)]
mod tray;
mod update;
mod wipe;

/// Starts the desktop app without showing the window (used by the native
/// host).
#[cfg(desktop)]
pub const BACKGROUND_ARG: &str = keystead_bridge::host::BACKGROUND_ARG;

/// Runs the desktop app; returns the process exit code. `background`:
/// start hidden (`--background`, see [`BACKGROUND_ARG`]).
#[cfg(desktop)]
pub fn run_desktop(background: bool) -> i32 {
    gui::run(background)
}

/// Terminal mode, before `keystead_tui::run()`: gives the TUI its own
/// console on Windows (see `platform::prepare_cli_console`).
#[cfg(desktop)]
pub fn prepare_cli_console() {
    platform::prepare_cli_console();
}

/// The phone app's entry point (called by the Android/iOS shell).
#[cfg(mobile)]
#[tauri::mobile_entry_point]
pub fn run() {
    gui::run_mobile();
}
