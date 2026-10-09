//! Terminal setup/teardown and the interactive event loop.
//!
//! The terminal is restored on every exit path: by [`TerminalGuard`]'s
//! `Drop`, and by a panic hook (release builds abort on panic, so `Drop`
//! would not run there).

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::app::App;
use crate::ui;

/// How often the UI redraws without input (TOTP countdown, auto-lock).
const TICK: Duration = Duration::from_millis(250);

static RAW_MODE: AtomicBool = AtomicBool::new(false);
static ALT_SCREEN: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

/// Installs (once) a panic hook that restores the terminal before the
/// panic message is printed.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}

/// Undoes whatever [`TerminalGuard`] / [`RawModeGuard`] changed. Safe to
/// call repeatedly.
pub fn restore_terminal() {
    // Raw mode first: it has more side effects than the alternate screen.
    if RAW_MODE.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
    }
    if ALT_SCREEN.swap(false, Ordering::SeqCst) {
        #[cfg(not(windows))]
        let _ = execute!(
            io::stdout(),
            ratatui::crossterm::event::DisableBracketedPaste
        );
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    }
}

/// Raw mode only (used by the command line while waiting for a key).
pub struct RawModeGuard(());

impl RawModeGuard {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        RAW_MODE.store(true, Ordering::SeqCst);
        Ok(RawModeGuard(()))
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if RAW_MODE.swap(false, Ordering::SeqCst) {
            let _ = disable_raw_mode();
        }
    }
}

/// Full-screen terminal: raw mode + alternate screen.
pub struct TerminalGuard {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        RAW_MODE.store(true, Ordering::SeqCst);
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen) {
            restore_terminal();
            return Err(e);
        }
        ALT_SCREEN.store(true, Ordering::SeqCst);
        // Pasted text then arrives as one event (multi-line pastes do not
        // submit forms). Windows consoles deliver pastes as key presses and
        // crossterm cannot parse bracketed pastes there.
        #[cfg(not(windows))]
        let _ = execute!(
            io::stdout(),
            ratatui::crossterm::event::EnableBracketedPaste
        );
        match Terminal::new(CrosstermBackend::new(io::stdout())) {
            Ok(terminal) => Ok(TerminalGuard { terminal }),
            Err(e) => {
                restore_terminal();
                Err(e)
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Runs the interactive UI until the user quits.
pub fn run(app: &mut App) -> io::Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let result = event_loop(&mut guard.terminal, app);
    drop(guard);
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.should_quit() {
            return Ok(());
        }
        if app.is_busy() {
            // The "please wait" frame is on screen; now do the slow work.
            app.run_pending();
            continue;
        }
        if event::poll(TICK)? {
            app.handle_event(event::read()?);
            // Handle everything that is already queued before redrawing.
            while !app.is_busy() && !app.should_quit() && event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
            }
        }
        app.tick(Instant::now());
    }
}
