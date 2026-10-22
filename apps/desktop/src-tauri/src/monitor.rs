//! Background thread: auto-lock after inactivity, lock when the computer is
//! locked or goes to sleep (desktop) or when the app stayed in the
//! background (phone), and pick up vault changes made by other processes
//! (TUI, second device on a USB stick, a synced vault file).
//!
//! System lock detection (desktop):
//! * Windows: real session notifications (`WM_WTSSESSION_CHANGE` with
//!   `WTS_SESSION_LOCK`, and `WM_POWERBROADCAST`/`PBT_APMSUSPEND`) arrive
//!   via [`MonitorSignal::SystemLock`] (see `platform::watch_session_events`).
//! * Everywhere (the only mechanism on Linux/macOS): a clock jump of more
//!   than [`SLEEP_JUMP`] between two ticks means the computer was suspended
//!   or hibernated – the vault is locked with reason `system`. A plain
//!   screen lock without suspend is not detected there.
//!
//! Background lock (phone): the activity's `onPause`/`onResume` arrive as
//! [`MonitorSignal::Background`]/[`MonitorSignal::Foreground`] (from
//! `gui::on_window_event`, `WindowEvent::Suspended`/`Resumed`). After
//! `Settings.lockInBackgroundSeconds` away from the foreground the vault is
//! locked with reason `background` – but never while a copied secret still
//! waits for its clipboard-clear timer, because locking clears the
//! clipboard and the secret was copied to be pasted into the other app
//! ([`background_lock_due`]). Time in the background is measured like idle
//! time, on both clocks: a phone sleeps all the time and its monotonic
//! clock stands still meanwhile. Every tick, and first thing when the app
//! comes back to the foreground, catches up on a clipboard clear the timer
//! thread slept through (wall clock); coming back then gives the lock a
//! last check – a long absence locks even if Android froze the cached
//! process the whole time and no tick ran – and resets. The desktop
//! ignores the setting; a clock jump means nothing on a phone.
//!
//! The app's own fingerprint prompt (`mobile::biometric`) is an activity
//! of its own, so it pauses the main activity too: an `onPause` while that
//! prompt is open ([`prompt_covers_pause`]) does not start the background
//! clock. If the activity is still paused once the prompt has closed (the
//! user left the app from the prompt), the clock starts then.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Weak;
use std::time::{Duration, Instant, SystemTime};

#[cfg(mobile)]
use keystead_core::clipboard;

use crate::state::{log, Core, LockReason};

/// Tick of the monitor loop.
const TICK: Duration = Duration::from_secs(1);
/// How often the auto-lock timeout is checked.
const AUTO_LOCK_INTERVAL: Duration = Duration::from_secs(5);
/// How often the vault file is checked for external changes.
const RELOAD_INTERVAL: Duration = Duration::from_secs(2);
/// A gap this large between two ticks means the computer was asleep.
#[cfg(desktop)]
const SLEEP_JUMP: Duration = Duration::from_secs(120);
/// Phone: an `onPause` this long after the app's own prompt closed still
/// belongs to it (the activity result arrives before `onResume`).
#[cfg(any(mobile, test))]
const PROMPT_GRACE: Duration = Duration::from_secs(2);
/// Phone: a prompt open longer than this no longer holds the background
/// clock back (a prompt that never answers must not keep the vault open).
#[cfg(any(mobile, test))]
const PROMPT_MAX: Duration = Duration::from_secs(120);

/// Phone: when the app's own system prompt (fingerprint) was opened and
/// closed (`Core::begin_own_prompt`).
#[cfg(any(mobile, test))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromptSpan {
    pub opened: Option<Instant>,
    pub closed: Option<Instant>,
}

/// Whether an `onPause` at `now` is the app's own prompt rather than the
/// user leaving the app (pure): the prompt is open (at most
/// [`PROMPT_MAX`]) or closed less than [`PROMPT_GRACE`] ago.
#[cfg(any(mobile, test))]
pub fn prompt_covers_pause(span: PromptSpan, now: Instant) -> bool {
    match (span.opened, span.closed) {
        (Some(opened), None) => now.saturating_duration_since(opened) < PROMPT_MAX,
        (Some(_), Some(closed)) => now.saturating_duration_since(closed) < PROMPT_GRACE,
        _ => false,
    }
}

/// Messages to the monitor thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorSignal {
    /// Desktop: the user locked the session or the computer is about to
    /// sleep (sent on Windows only; Linux/macOS rely on the clock jump).
    #[cfg(desktop)]
    #[cfg_attr(not(windows), allow(dead_code))]
    SystemLock,
    /// Phone: the app left the foreground (Android `onPause`).
    #[cfg(mobile)]
    Background,
    /// Phone: the app is back in the foreground (Android `onResume`).
    #[cfg(mobile)]
    Foreground,
}

/// Starts the monitor thread. It ends when the core is dropped or the app
/// exits.
pub fn spawn(core: Weak<Core>, signals: Receiver<MonitorSignal>) {
    let result = std::thread::Builder::new()
        .name("keystead-monitor".into())
        .spawn(move || run(&core, &signals));
    if let Err(e) = result {
        log(format_args!("could not start the monitor thread: {e}"));
    }
}

fn run(core: &Weak<Core>, signals: &Receiver<MonitorSignal>) {
    #[cfg(desktop)]
    let mut last_tick = (Instant::now(), SystemTime::now());
    #[cfg(mobile)]
    let mut background = BackgroundTimer::default();
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

        match signal {
            #[cfg(desktop)]
            Some(MonitorSignal::SystemLock) => lock_for_system(&core),
            #[cfg(mobile)]
            Some(MonitorSignal::Background) => background.enter(core.own_prompt_covers_pause()),
            #[cfg(mobile)]
            Some(MonitorSignal::Foreground) => background.leave(&core),
            None => {}
        }

        // Suspend/hibernate detection via clock jumps (desktop).
        #[cfg(desktop)]
        {
            let now = (Instant::now(), SystemTime::now());
            let slept = slept_between(last_tick, now);
            last_tick = now;
            if slept {
                lock_for_system(&core);
            }
        }

        // Phone, every tick: a clipboard clear the timer thread slept
        // through (deep sleep), then the background lock.
        #[cfg(mobile)]
        {
            catch_up_clipboard_clear();
            background.check(&core);
        }

        if last_auto_lock_check.elapsed() >= AUTO_LOCK_INTERVAL {
            last_auto_lock_check = Instant::now();
            check_auto_lock(&core);
            // An analysed import nobody committed: drop its secrets.
            #[cfg(desktop)]
            core.import_slot().expire(Instant::now());
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
#[cfg(desktop)]
fn slept_between(previous: (Instant, SystemTime), now: (Instant, SystemTime)) -> bool {
    let monotonic = now.0.saturating_duration_since(previous.0);
    let wall = now.1.duration_since(previous.1).unwrap_or_default();
    monotonic.max(wall) > SLEEP_JUMP
}

/// Locks with reason `system` if `lockOnSystemLock` is on – also while no
/// vault is open: `Core::lock` then only starts a new lock epoch, so an
/// unlock whose key derivation is running (the user pressed Win+L right
/// after entering the password) does not open the vault afterwards.
#[cfg(desktop)]
fn lock_for_system(core: &Core) {
    let enabled = core.state().settings.lock_on_system_lock;
    if enabled {
        core.lock(Some(LockReason::System));
    }
}

/// Whether the phone's vault is to be locked after `in_background` away
/// from the foreground (pure; the policy of the background lock).
///
/// * `lock_in_background_seconds`: `Settings.lockInBackgroundSeconds` –
///   `None` = never, `0` = as soon as the app leaves the foreground.
/// * `clipboard_clear_pending_for`: how long until a copied secret's timed
///   clipboard clear fires (`None` = nothing pending, `Some(0)` = overdue).
///   The lock waits for it: locking clears the clipboard, and the secret
///   was copied to be pasted into the other app. A secret copied without a
///   timer (`clipboardClearSeconds` = 0) does not count – there is nothing
///   to wait for, the lock clears it as documented.
#[cfg(any(mobile, test))]
fn background_lock_due(
    in_background: Duration,
    lock_in_background_seconds: Option<u32>,
    clipboard_clear_pending_for: Option<Duration>,
) -> bool {
    let Some(seconds) = lock_in_background_seconds else {
        return false;
    };
    if in_background < Duration::from_secs(u64::from(seconds)) {
        return false;
    }
    !clipboard_clear_pending_for.is_some_and(|remaining| !remaining.is_zero())
}

/// Phone: time since the app left the foreground, on both clocks (like
/// `AppState::idle_for`: the monotonic clock stands still while the phone
/// sleeps), and the lock it leads to.
#[cfg(mobile)]
#[derive(Default)]
struct BackgroundTimer {
    since: Option<(Instant, SystemTime)>,
    /// Paused behind the app's own prompt: the clock starts if the
    /// activity is still paused once the prompt has closed.
    behind_prompt: bool,
}

#[cfg(mobile)]
impl BackgroundTimer {
    /// `Suspended`: starts the clock (a repeated signal keeps the first) –
    /// unless the app's own prompt caused it.
    fn enter(&mut self, behind_own_prompt: bool) {
        if behind_own_prompt {
            self.behind_prompt = true;
        } else if self.since.is_none() {
            self.since = Some((Instant::now(), SystemTime::now()));
        }
    }

    /// Time in the background so far; `None` in the foreground.
    fn elapsed(&self) -> Option<Duration> {
        self.since.map(|(instant, wall)| {
            let monotonic = instant.elapsed();
            let wall = SystemTime::now().duration_since(wall).unwrap_or_default();
            monotonic.max(wall)
        })
    }

    /// Every tick: locks once the policy says so, then stops the clock (one
    /// lock per stay in the background).
    fn check(&mut self, core: &Core) {
        // The prompt has closed and the activity did not come back: the
        // user left the app from the prompt.
        if self.behind_prompt && !core.own_prompt_covers_pause() {
            self.behind_prompt = false;
            self.enter(false);
        }
        let Some(in_background) = self.elapsed() else {
            return;
        };
        let seconds = core.state().settings.lock_in_background_seconds;
        if background_lock_due(in_background, seconds, clipboard_clear_pending_for()) {
            lock_for_background(core);
            self.since = None;
        }
    }

    /// `Resumed`: a clipboard clear the timer thread slept through runs now
    /// (the wall clock decides), then the lock gets its last check – a long
    /// absence locks even if the process was frozen meanwhile and no tick
    /// ran –, then the clock stops.
    fn leave(&mut self, core: &Core) {
        self.behind_prompt = false;
        catch_up_clipboard_clear();
        self.check(core);
        self.since = None;
    }
}

/// Clears a copied secret whose timed clear is overdue by the wall clock:
/// the timer thread sleeps on the monotonic clock, which stands still while
/// the phone is in deep sleep (see `clipboard::clear_overdue_secret`).
#[cfg(mobile)]
fn catch_up_clipboard_clear() {
    if let Err(e) = clipboard::clear_overdue_secret(SystemTime::now()) {
        log(format_args!("could not clear the clipboard: {}", e.code()));
    }
}

/// How long until the pending clipboard clear fires (`Some(0)` = overdue).
#[cfg(mobile)]
fn clipboard_clear_pending_for() -> Option<Duration> {
    clipboard::pending_clear_due().map(|due| due.saturating_duration_since(Instant::now()))
}

/// Locks with reason `background` – also while no vault is open (a new
/// lock epoch, like `lock_for_system`: an unlock whose key derivation was
/// still running when the app was put away must not open the vault in the
/// background).
#[cfg(mobile)]
fn lock_for_background(core: &Core) {
    core.lock(Some(LockReason::Background));
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
/// The new revision also goes to the cloud-folder mirror (desktop).
fn check_external_change(core: &Core, last_error: &mut Option<String>) {
    let (result, vault_id) = {
        let mut st = core.state();
        match st.vault.as_mut() {
            Some(vault) => (vault.reload_if_changed(), vault.id().to_owned()),
            None => return,
        }
    };
    match result {
        Ok(true) => {
            *last_error = None;
            core.emit_changed();
            core.request_mirror(&vault_id);
        }
        Ok(false) => *last_error = None,
        // The master password or vault key was changed by another process:
        // the cached key no longer opens the file, so end this session.
        // The file with the new password is copied all the same (it is
        // encrypted; the phone learns of the change from it).
        Err(keystead_core::Error::KeyChanged) => {
            *last_error = None;
            core.lock(Some(LockReason::Manual));
            core.request_mirror(&vault_id);
            // Phone: the fingerprint unlock's sealed KEK no longer opens
            // the file either.
            #[cfg(mobile)]
            crate::device_unlock::invalidate(
                &core.data_dir(),
                &vault_id,
                "the master password was changed",
            );
        }
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
    #[cfg(desktop)]
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

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn the_own_prompt_covers_a_pause_while_open_and_briefly_after() {
        let t = Instant::now();
        let span = |opened: Option<u64>, closed: Option<u64>| PromptSpan {
            opened: opened.map(|s| t + S(s)),
            closed: closed.map(|s| t + S(s)),
        };
        // Never opened: every pause is the user leaving.
        assert!(!prompt_covers_pause(PromptSpan::default(), t + S(1)));
        // Open: covered, but not for ever.
        assert!(prompt_covers_pause(span(Some(0), None), t));
        assert!(prompt_covers_pause(span(Some(0), None), t + S(119)));
        assert!(!prompt_covers_pause(span(Some(0), None), t + S(120)));
        // Closed: the resume follows at once; a pause later is a new one.
        assert!(prompt_covers_pause(span(Some(0), Some(10)), t + S(11)));
        assert!(!prompt_covers_pause(span(Some(0), Some(10)), t + S(12)));
        assert!(!prompt_covers_pause(span(Some(0), Some(10)), t + S(60)));
        // A clock read before the span (another thread) counts as covered.
        assert!(prompt_covers_pause(span(Some(5), None), t));
    }

    #[test]
    fn background_lock_never() {
        // `null` in the settings: no lock, however long, pending clear or not.
        assert!(!background_lock_due(S(0), None, None));
        assert!(!background_lock_due(S(24 * 3600), None, None));
        assert!(!background_lock_due(S(24 * 3600), None, Some(S(0))));
        assert!(!background_lock_due(S(24 * 3600), None, Some(S(10))));
    }

    #[test]
    fn background_lock_immediately() {
        // 0 s: due as soon as the app is in the background ...
        assert!(background_lock_due(S(0), Some(0), None));
        assert!(background_lock_due(Duration::from_millis(1), Some(0), None));
        // ... unless a copied secret still waits for its clear timer.
        assert!(!background_lock_due(S(0), Some(0), Some(S(25))));
        assert!(!background_lock_due(S(20), Some(0), Some(S(5))));
        // The timer has fired (or is overdue): nothing holds the lock back.
        assert!(background_lock_due(S(25), Some(0), Some(S(0))));
        assert!(background_lock_due(S(25), Some(0), None));
    }

    #[test]
    fn background_lock_after_a_delay() {
        let one_minute = Some(60);
        assert!(!background_lock_due(S(0), one_minute, None));
        assert!(!background_lock_due(S(59), one_minute, None));
        assert!(background_lock_due(S(60), one_minute, None));
        assert!(background_lock_due(S(3600), one_minute, None));
        // A pending clear postpones the lock past the delay ...
        assert!(!background_lock_due(S(60), one_minute, Some(S(5))));
        assert!(!background_lock_due(S(90), one_minute, Some(S(1))));
        // ... until it has fired.
        assert!(background_lock_due(S(65), one_minute, Some(S(0))));
        assert!(background_lock_due(S(65), one_minute, None));
        // Before the delay a pending clear changes nothing.
        assert!(!background_lock_due(S(10), one_minute, Some(S(20))));
        assert!(!background_lock_due(S(10), one_minute, Some(S(0))));
        // Five minutes.
        assert!(!background_lock_due(S(299), Some(300), None));
        assert!(background_lock_due(S(300), Some(300), None));
    }
}
