//! Background thread: auto-lock after inactivity, lock when the computer is
//! locked or goes to sleep, and pick up vault changes made by other
//! processes (TUI, second device on a USB stick).
//!
//! System lock detection:
//! * Windows: real session notifications (`WM_WTSSESSION_CHANGE` with
//!   `WTS_SESSION_LOCK`, and `WM_POWERBROADCAST`/`PBT_APMSUSPEND`) arrive
//!   via [`MonitorSignal::SystemLock`] (see `platform::watch_session_events`).
//! * Everywhere (the only mechanism on Linux/macOS): a clock jump of more
//!   than [`SLEEP_JUMP`] between two ticks means the computer was suspended
//!   or hibernated – the vault is locked with reason `system`. A plain
//!   screen lock without suspend is not detected there.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Weak;
use std::time::{Duration, Instant, SystemTime};

use crate::state::{log, Core, LockReason};

/// Tick of the monitor loop.
const TICK: Duration = Duration::from_secs(1);
/// How often the auto-lock timeout is checked.
const AUTO_LOCK_INTERVAL: Duration = Duration::from_secs(5);
/// How often the vault file is checked for external changes.
const RELOAD_INTERVAL: Duration = Duration::from_secs(2);
/// A gap this large between two ticks means the computer was asleep.
const SLEEP_JUMP: Duration = Duration::from_secs(120);

/// Messages to the monitor thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorSignal {
    /// The user locked the session or the computer is about to sleep.
    SystemLock,
}

/// Starts the monitor thread. It ends when the core is dropped or the app
/// exits.
pub fn spawn(core: Weak<Core>, signals: Receiver<MonitorSignal>) {
    let result = std::thread::Builder::new()
        .name("vaultx-monitor".into())
        .spawn(move || run(&core, &signals));
    if let Err(e) = result {
        log(format_args!("could not start the monitor thread: {e}"));
    }
}

fn run(core: &Weak<Core>, signals: &Receiver<MonitorSignal>) {
    let mut last_tick = (Instant::now(), SystemTime::now());
    let mut last_auto_lock_check = Instant::now();
    let mut last_reload_check = Instant::now();
    let mut last_reload_error: Option<String> = None;
    let mut signals_open = true;

    loop {
        let signal = if signals_open {
            match signals.recv_timeout(TICK) {
                Ok(signal) => Some(signal),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    signals_open = false;
                    None
                }
            }
        } else {
            std::thread::sleep(TICK);
            None
        };
        let Some(core) = core.upgrade() else {
            return;
        };
        if core.is_exiting() {
            return;
        }

        if signal == Some(MonitorSignal::SystemLock) {
            lock_for_system(&core);
        }

        // Suspend/hibernate detection via clock jumps.
        let now = (Instant::now(), SystemTime::now());
        let slept = slept_between(last_tick, now);
        last_tick = now;
        if slept {
            lock_for_system(&core);
        }

        if last_auto_lock_check.elapsed() >= AUTO_LOCK_INTERVAL {
            last_auto_lock_check = Instant::now();
            check_auto_lock(&core);
        }

        if last_reload_check.elapsed() >= RELOAD_INTERVAL {
            last_reload_check = Instant::now();
            check_external_change(&core, &mut last_reload_error);
        }
    }
}

/// True if the gap between two ticks (monotonic or wall clock – Linux's
/// monotonic clock stops during suspend) shows that the computer slept. A
/// wall clock set backwards is ignored.
fn slept_between(previous: (Instant, SystemTime), now: (Instant, SystemTime)) -> bool {
    let monotonic = now.0.saturating_duration_since(previous.0);
    let wall = now.1.duration_since(previous.1).unwrap_or_default();
    monotonic.max(wall) > SLEEP_JUMP
}

/// Locks with reason `system` if `lockOnSystemLock` is on.
fn lock_for_system(core: &Core) {
    let enabled = {
        let st = core.state();
        st.vault.is_some() && st.settings.lock_on_system_lock
    };
    if enabled {
        core.lock(Some(LockReason::System));
    }
}

/// Locks with reason `timeout` after `autoLockMinutes` without activity.
fn check_auto_lock(core: &Core) {
    let expired = {
        let st = core.state();
        let minutes = st.settings.auto_lock_minutes;
        st.vault.is_some()
            && minutes > 0
            && st.idle_for() >= Duration::from_secs(u64::from(minutes) * 60)
    };
    if expired {
        core.lock(Some(LockReason::Timeout));
    }
}

/// Reloads the vault if another process saved it; emits `vault://changed`.
fn check_external_change(core: &Core, last_error: &mut Option<String>) {
    let result = {
        let mut st = core.state();
        match st.vault.as_mut() {
            Some(vault) => vault.reload_if_changed(),
            None => return,
        }
    };
    match result {
        Ok(true) => {
            *last_error = None;
            core.emit_changed();
        }
        Ok(false) => *last_error = None,
        Err(e) => {
            // Log each distinct problem once, not every 2 s.
            let code = e.code();
            if last_error.as_deref() != Some(code.as_str()) {
                log(format_args!(
                    "could not check the vault file for changes: {code}"
                ));
                *last_error = Some(code);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_jumps() {
        let i = Instant::now();
        let w = SystemTime::now();
        let tick = Duration::from_secs(1);
        assert!(!slept_between((i, w), (i + tick, w + tick)));
        // Linux: monotonic clock paused, wall clock jumped (suspend).
        assert!(slept_between(
            (i, w),
            (i + tick, w + Duration::from_secs(600))
        ));
        // Monotonic clock includes the sleep (Windows).
        assert!(slept_between(
            (i, w),
            (i + Duration::from_secs(600), w + tick)
        ));
        // Wall clock set backwards: no lock.
        assert!(!slept_between(
            (i, w + Duration::from_secs(3600)),
            (i + tick, w)
        ));
        // A slow tick is not a sleep.
        assert!(!slept_between(
            (i, w),
            (i + Duration::from_secs(5), w + Duration::from_secs(5))
        ));
    }
}
