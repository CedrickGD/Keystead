//! Operating-system specific helpers: console for terminal mode, opening
//! folders/websites, spawning a terminal, tray availability and (Windows)
//! session lock notifications.

use std::path::Path;
use std::process::{Child, Command};

use crate::error::{AppError, AppResult};
use crate::state::log;

/// The `platform` value of `AppInfo`.
pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

// ---------------------------------------------------------------------------
// Terminal mode
// ---------------------------------------------------------------------------

/// Windows: gives terminal mode its own new console window (the release
/// exe uses the GUI subsystem and has none) and points the standard handles
/// at it. It also marks the session as running a terminal UI (see
/// [`terminal_ui_running`]) and clears a secret the TUI copied when the
/// console is closed or the process is ended by Ctrl+C, logoff or shutdown
/// (the TUI's timed clear runs in-process and would never fire). Other
/// platforms: nothing to do.
pub fn prepare_cli_console() {
    #[cfg(windows)]
    {
        windows::new_console();
        windows::guard_terminal_session();
    }
}

/// True while a terminal UI (`Keystead --cli`) runs in this Windows session.
/// The update's setup ends every running `Keystead.exe` without asking, so
/// `install_update` refuses while one runs (its unsaved input and a copied
/// secret would be lost resp. stay on the clipboard). Always false elsewhere
/// (an AppImage update does not end running copies).
pub fn terminal_ui_running() -> bool {
    #[cfg(windows)]
    {
        windows::terminal_ui_running()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// Opening things
// ---------------------------------------------------------------------------

/// True for `http`/`https` URLs, the only ones handed to the system browser.
pub fn is_web_url(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

/// Opens an http(s) URL in the default browser. Other schemes are refused
/// (logged).
pub fn open_url(url: &tauri::Url) {
    if !is_web_url(url) {
        log(format_args!(
            "refusing to open a non-web URL (scheme {})",
            url.scheme()
        ));
        return;
    }
    if let Err(e) = open::that_detached(url.as_str()) {
        log(format_args!("could not open the browser: {e}"));
    }
}

/// Opens a folder in the file manager (Explorer, xdg-open, Finder).
pub fn open_folder(path: &Path) -> AppResult<()> {
    std::fs::create_dir_all(path).map_err(|e| AppError::io(format!("{}: {e}", path.display())))?;
    open::that_detached(path).map_err(|e| AppError::io(format!("{}: {e}", path.display())))
}

/// Reaps a spawned child in the background so it does not linger as a
/// zombie process (Unix) and its handle is released (Windows).
fn detach(mut child: Child) {
    let _ = std::thread::Builder::new()
        .name("keystead-child".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// Starts `<exe> --cli` (the terminal UI) in a new terminal window.
pub fn open_terminal(exe: &Path) -> AppResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;
        let child = Command::new(exe)
            .arg("--cli")
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|e| AppError::io(format!("terminal: {e}")))?;
        detach(child);
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // AppleScript string with a shell-quoted path inside.
        let shell_quoted = format!("'{}'", exe.to_string_lossy().replace('\'', r"'\''"));
        let command = format!("{shell_quoted} --cli");
        let applescript_string = command.replace('\\', r"\\").replace('"', "\\\"");
        let child = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "tell application \"Terminal\" to do script \"{applescript_string}\""
            ))
            .arg("-e")
            .arg("tell application \"Terminal\" to activate")
            .spawn()
            .map_err(|e| AppError::io(format!("terminal: {e}")))?;
        detach(child);
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (program, prefix) in linux_terminals() {
            let mut command = Command::new(&program);
            command.args(&prefix).arg(exe).arg("--cli");
            match command.spawn() {
                Ok(child) => {
                    detach(child);
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    log(format_args!("could not start {program}: {e}"));
                    continue;
                }
            }
        }
        Err(AppError::unsupported("no_terminal"))
    }
}

/// Terminal emulators to try, with the arguments that precede the command.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_terminals() -> Vec<(String, Vec<&'static str>)> {
    let mut list = Vec::new();
    if let Some(term) = std::env::var_os("TERMINAL").filter(|t| !t.is_empty()) {
        list.push((term.to_string_lossy().into_owned(), vec!["-e"]));
    }
    let known: [(&str, &[&'static str]); 8] = [
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-x"]),
        ("kitty", &[]),
        ("alacritty", &["-e"]),
        ("foot", &[]),
        ("xterm", &["-e"]),
    ];
    list.extend(
        known
            .iter()
            .map(|(program, prefix)| ((*program).to_owned(), prefix.to_vec())),
    );
    list
}

// ---------------------------------------------------------------------------
// Tray support
// ---------------------------------------------------------------------------

/// Whether a tray icon can be created. On Linux the tray needs the
/// (Ayatana) AppIndicator library at runtime; without it the tray crate
/// would abort the process.
pub fn tray_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
            .iter()
            // SAFETY: loading these well-known system libraries runs only
            // their (side-effect free) initialisers; the handle is dropped
            // immediately, the tray crate loads the library again itself.
            .any(|name| unsafe { libloading::Library::new(name) }.is_ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

// ---------------------------------------------------------------------------
// Session lock notifications
// ---------------------------------------------------------------------------

/// Windows: reports session locks and suspend to the monitor thread via
/// `core.signal_monitor`. Elsewhere a no-op (the monitor detects suspend
/// by clock jumps).
pub fn watch_session_events(
    window: &tauri::WebviewWindow,
    core: &std::sync::Arc<crate::state::Core>,
) {
    #[cfg(windows)]
    windows::watch_session_events(window, core);
    #[cfg(not(windows))]
    let _ = (window, core);
}

#[cfg(windows)]
mod windows {
    use std::sync::{Arc, OnceLock, Weak};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, FreeConsole, SetConsoleCtrlHandler, SetStdHandle, CTRL_BREAK_EVENT,
        CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        PBT_APMSUSPEND, WM_NCDESTROY, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK,
    };

    use crate::monitor::MonitorSignal;
    use crate::state::{log, Core};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Detaches from any console, allocates a new one and makes it the
    /// standard input/output/error (crossterm and rpassword use the console
    /// devices `CONIN$`/`CONOUT$` themselves; std I/O uses these handles).
    pub fn new_console() {
        // SAFETY: plain Win32 calls without pointers except the
        // NUL-terminated device names, which outlive the calls.
        unsafe {
            FreeConsole();
            if AllocConsole() == 0 {
                return;
            }
            let open = |name: &str| {
                let name = wide(name);
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            let input = open("CONIN$");
            if input != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_INPUT_HANDLE, input);
            }
            let output = open("CONOUT$");
            if output != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_OUTPUT_HANDLE, output);
                SetStdHandle(STD_ERROR_HANDLE, output);
            }
        }
    }

    /// Named mutex a running terminal UI holds for its whole lifetime
    /// (`Local\`: this logon session only). Only terminal mode creates it –
    /// never the native host or the desktop app.
    const TERMINAL_UI_MUTEX: &str = "Local\\Keystead-TerminalUI";

    /// Terminal mode: hold [`TERMINAL_UI_MUTEX`] until the process ends
    /// (the handle is never closed; Windows releases it at exit) and clear a
    /// copied secret when the console goes away.
    pub fn guard_terminal_session() {
        let name = wide(TERMINAL_UI_MUTEX);
        // SAFETY: NUL-terminated name that outlives the calls; a null
        // handler is never passed.
        unsafe {
            // Leaked on purpose: held as long as the process runs.
            let _ = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            SetConsoleCtrlHandler(Some(console_ctrl_handler), 1);
        }
    }

    /// Runs on its own thread when the console is closed, the user logs off,
    /// Windows shuts down or Ctrl+C/Ctrl+Break arrives as a signal (the
    /// full-screen UI reads Ctrl+C as a key). Clears the TUI's copied secret
    /// if the clipboard still holds it – the timed clear lives in this
    /// process and dies with it – and lets the default handler end the
    /// process (FALSE). Must stay fast (Windows allows ~5 s) and touches
    /// nothing of the TUI's state.
    unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> i32 {
        if matches!(
            ctrl_type,
            CTRL_C_EVENT
                | CTRL_BREAK_EVENT
                | CTRL_CLOSE_EVENT
                | CTRL_LOGOFF_EVENT
                | CTRL_SHUTDOWN_EVENT
        ) {
            let _ = keystead_core::clipboard::clear_pending_secret();
        }
        0
    }

    pub fn terminal_ui_running() -> bool {
        let name = wide(TERMINAL_UI_MUTEX);
        // SAFETY: NUL-terminated name that outlives the call; the handle is
        // closed right away.
        unsafe {
            let handle = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
            if handle.is_null() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }

    const SUBCLASS_ID: usize = 0x5641_554c; // "VAUL"

    /// The core the window procedure reports to (one main window per process).
    static SESSION_CORE: OnceLock<Weak<Core>> = OnceLock::new();

    pub fn watch_session_events(window: &tauri::WebviewWindow, core: &Arc<Core>) {
        let hwnd: HWND = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get() as HWND,
            Ok(_) => return,
            Err(e) => {
                log(format_args!(
                    "no window handle for session notifications: {e}"
                ));
                return;
            }
        };
        if SESSION_CORE.set(Arc::downgrade(core)).is_err() {
            return;
        }
        // SAFETY: `hwnd` is the live main window and this runs on its
        // thread (Tauri's setup hook runs on the main thread); the
        // subclass is removed again on WM_NCDESTROY.
        unsafe {
            if SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) == 0 {
                log("could not subscribe to session notifications");
                return;
            }
            if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
                log("could not register for session lock notifications");
            }
        }
    }

    fn notify_system_lock() {
        if let Some(core) = SESSION_CORE.get().and_then(Weak::upgrade) {
            core.signal_monitor(MonitorSignal::SystemLock);
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_WTSSESSION_CHANGE if wparam == WTS_SESSION_LOCK as usize => notify_system_lock(),
            WM_POWERBROADCAST if wparam == PBT_APMSUSPEND as usize => notify_system_lock(),
            WM_NCDESTROY => {
                // SAFETY: the window is still valid during WM_NCDESTROY.
                unsafe {
                    WTSUnRegisterSessionNotification(hwnd);
                    RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                }
            }
            _ => {}
        }
        // SAFETY: forwards the unchanged message to the next procedure.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}
