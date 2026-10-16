//! `Keystead(.exe)` – one portable executable for everything (see "Executable
//! modes" in docs/ARCHITECTURE.md):
//!
//! 1. native messaging host (an argument starts with `chrome-extension://`),
//! 2. terminal UI (`--cli` / `cli` as first argument),
//! 3. the desktop app (everything else; `--background` starts in the tray).
//!
//! The app itself lives in the library (`lib.rs`), which the phone loads
//! directly; this binary has no role there.

// Release builds use the Windows GUI subsystem (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(desktop)]
fn main() {
    use std::ffi::OsString;

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // 1. Native host mode: never initialise Tauri, stdout carries frames only.
    if keystead_bridge::host::is_host_invocation(&args) {
        std::process::exit(keystead_bridge::host::run());
    }

    // 2. Terminal mode.
    if args.first().is_some_and(|a| a == "--cli" || a == "cli") {
        keystead_lib::prepare_cli_console();
        std::process::exit(keystead_tui::run());
    }

    // 3. Desktop app.
    let background = args.iter().any(|a| a == keystead_lib::BACKGROUND_ARG);
    std::process::exit(keystead_lib::run_desktop(background));
}

/// The phone starts through the library's `tauri::mobile_entry_point`
/// (`keystead_lib::run`); this binary is not part of the app there.
#[cfg(mobile)]
fn main() {}
