//! `Keystead(.exe)` – one portable executable for everything (see "Executable
//! modes" in docs/ARCHITECTURE.md):
//!
//! 1. native messaging host (an argument starts with `chrome-extension://`),
//! 2. terminal UI (`--cli` / `cli` as first argument),
//! 3. the desktop app (everything else; `--background` starts in the tray).

// Release builds use the Windows GUI subsystem (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod commands;
mod error;
mod extension;
mod gui;
mod monitor;
mod platform;
mod portable;
mod state;
mod tray;
mod update;
mod wipe;

use std::ffi::OsString;

/// Starts the app without showing the window (used by the native host).
pub const BACKGROUND_ARG: &str = keystead_bridge::host::BACKGROUND_ARG;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // 1. Native host mode: never initialise Tauri, stdout carries frames only.
    if keystead_bridge::host::is_host_invocation(&args) {
        std::process::exit(keystead_bridge::host::run());
    }

    // 2. Terminal mode.
    if args.first().is_some_and(|a| a == "--cli" || a == "cli") {
        platform::prepare_cli_console();
        std::process::exit(keystead_tui::run());
    }

    // 3. Desktop app.
    let background = args.iter().any(|a| a == BACKGROUND_ARG);
    std::process::exit(gui::run(background));
}
