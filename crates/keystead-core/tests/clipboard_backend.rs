//! The clipboard policy (generation counter, timed clear, clear on lock,
//! the pending deadline and the catch-up clear) on a mock
//! `ClipboardBackend` – no real clipboard needed. The backend is
//! process-wide, so everything runs in one test, step by step.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use keystead_core::clipboard::{
    self, clear_overdue_secret, clear_pending_secret, copy_secret, copy_text, pending_clear_due,
    read_text, ClipboardBackend,
};
use keystead_core::Error;

/// The mock's clipboard content.
static TEXT: Mutex<Option<String>> = Mutex::new(None);
/// Every write as (text, sensitive).
static WRITES: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
/// Number of unconditional clears.
static CLEARS: AtomicUsize = AtomicUsize::new(0);
/// Behaves like a backend that cannot read the clipboard (Android ≥ 10 in
/// the background): reads yield nothing, a timed clear clears regardless.
static BLIND: AtomicBool = AtomicBool::new(false);
/// Every operation fails (clipboard busy/unavailable).
static FAIL: AtomicBool = AtomicBool::new(false);

struct Mock;

fn check_fail() -> keystead_core::Result<()> {
    if FAIL.load(Ordering::SeqCst) {
        Err(Error::Io(std::io::Error::other("mock clipboard down")))
    } else {
        Ok(())
    }
}

impl ClipboardBackend for Mock {
    fn write_text(&self, text: &str, sensitive: bool) -> keystead_core::Result<()> {
        check_fail()?;
        *TEXT.lock().unwrap() = Some(text.to_owned());
        WRITES.lock().unwrap().push((text.to_owned(), sensitive));
        Ok(())
    }

    fn read_text(&self) -> keystead_core::Result<Option<String>> {
        check_fail()?;
        if BLIND.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(TEXT.lock().unwrap().clone())
    }

    fn clear(&self) -> keystead_core::Result<()> {
        check_fail()?;
        *TEXT.lock().unwrap() = None;
        CLEARS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn clear_if_equals(&self, expected: &str) -> keystead_core::Result<bool> {
        if BLIND.load(Ordering::SeqCst) {
            check_fail()?;
            self.clear()?;
            return Ok(true);
        }
        // The trait's default: compare, then clear.
        let Some(current) = self.read_text()? else {
            return Ok(false);
        };
        if current == expected {
            self.clear()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn text() -> Option<String> {
    TEXT.lock().unwrap().clone()
}

/// Simulates another application replacing the clipboard content.
fn other_app_copies(s: &str) {
    *TEXT.lock().unwrap() = Some(s.to_owned());
}

fn last_write() -> (String, bool) {
    WRITES.lock().unwrap().last().cloned().expect("a write")
}

/// Waits (up to 5 s) until `cond` holds; false on timeout.
fn wait_until(cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

/// Asserts that `cond` keeps holding for `for_` (polling).
fn holds_for(for_: Duration, cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + for_;
    while Instant::now() < deadline {
        if !cond() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

const TIMER: Duration = Duration::from_millis(150);

#[test]
fn policy_on_a_mock_backend() {
    assert!(clipboard::install_backend(Box::new(Mock)));
    // Only the first backend counts.
    assert!(!clipboard::install_backend(Box::new(Mock)));

    // --- plain text: not sensitive, nothing pending ------------------------
    copy_text("user@example.com").unwrap();
    assert_eq!(read_text().unwrap().as_deref(), Some("user@example.com"));
    assert_eq!(last_write(), ("user@example.com".to_owned(), false));
    assert!(!clear_pending_secret().unwrap());
    assert_eq!(text().as_deref(), Some("user@example.com"));

    // --- secret without timer: stays until lock/quit clears it -------------
    copy_secret("s1", None).unwrap();
    assert_eq!(last_write(), ("s1".to_owned(), true));
    assert!(holds_for(Duration::from_millis(300), || {
        text().as_deref() == Some("s1")
    }));
    assert!(clear_pending_secret().unwrap());
    assert_eq!(text(), None);
    // Already cleared: nothing pending any more.
    assert!(!clear_pending_secret().unwrap());

    // --- timed clear -------------------------------------------------------
    let clears = CLEARS.load(Ordering::SeqCst);
    copy_secret("s2", Some(TIMER)).unwrap();
    assert!(wait_until(|| text().is_none()), "timer clears the secret");
    assert_eq!(CLEARS.load(Ordering::SeqCst), clears + 1);
    // The timer took the pending secret with it.
    assert!(!clear_pending_secret().unwrap());

    // --- a zero delay means no timer ---------------------------------------
    copy_secret("s2b", Some(Duration::ZERO)).unwrap();
    assert!(holds_for(Duration::from_millis(300), || {
        text().as_deref() == Some("s2b")
    }));
    assert!(clear_pending_secret().unwrap());

    // --- plain text copied afterwards survives the timer -------------------
    copy_secret("s3", Some(TIMER)).unwrap();
    copy_text("plain").unwrap();
    assert!(holds_for(TIMER * 4, || text().as_deref() == Some("plain")));
    assert!(!clear_pending_secret().unwrap());
    assert_eq!(text().as_deref(), Some("plain"));

    // --- a newer secret survives the older secret's timer ------------------
    copy_secret("s4a", Some(TIMER)).unwrap();
    copy_secret("s4b", None).unwrap();
    assert!(holds_for(TIMER * 4, || text().as_deref() == Some("s4b")));
    assert!(clear_pending_secret().unwrap());
    assert_eq!(text(), None);

    // --- re-copying the same secret restarts the timer (generation) --------
    let long = Duration::from_millis(600);
    copy_secret("s5", Some(long)).unwrap();
    std::thread::sleep(Duration::from_millis(250));
    copy_secret("s5", Some(long)).unwrap();
    // The first timer fires at ~600 ms and finds a newer generation: no-op.
    assert!(holds_for(Duration::from_millis(250), || {
        text().as_deref() == Some("s5")
    }));
    assert!(wait_until(|| text().is_none()), "second timer clears");

    // --- content changed by another app is left alone ----------------------
    copy_secret("s6", Some(TIMER)).unwrap();
    other_app_copies("theirs");
    assert!(holds_for(TIMER * 4, || text().as_deref() == Some("theirs")));
    assert!(!clear_pending_secret().unwrap());
    assert_eq!(text().as_deref(), Some("theirs"));
    // ... also on lock.
    copy_secret("s7", None).unwrap();
    other_app_copies("theirs-2");
    assert!(!clear_pending_secret().unwrap());
    assert_eq!(text().as_deref(), Some("theirs-2"));
    assert!(!clipboard::clear_if_equals("s7").unwrap());
    assert!(clipboard::clear_if_equals("theirs-2").unwrap());
    assert_eq!(text(), None);

    // --- the pending deadline (the phone's background lock waits for it) ---
    assert_eq!(pending_clear_due(), None);
    copy_secret("s7a", None).unwrap();
    // No timer: nothing to wait for (lock/quit clears it).
    assert_eq!(pending_clear_due(), None);
    let before = Instant::now();
    copy_secret("s7b", Some(long)).unwrap();
    let due = pending_clear_due().expect("a timed clear is pending");
    assert!(due >= before + long && due <= Instant::now() + long);
    copy_text("plain-7").unwrap();
    // Plain text forgets the secret, and with it the deadline.
    assert_eq!(pending_clear_due(), None);
    copy_secret("s7c", Some(TIMER)).unwrap();
    assert!(pending_clear_due().is_some());
    assert!(wait_until(|| text().is_none()), "timer clears the secret");
    assert_eq!(pending_clear_due(), None);

    // --- catch-up: the timer thread slept through the deadline ------------
    // (the phone was in deep sleep: the monotonic clock stood still, the
    // wall clock did not)
    copy_secret("s7d", Some(long)).unwrap();
    let wall_now = SystemTime::now();
    // Not due yet by the wall clock: nothing happens.
    assert!(!clear_overdue_secret(wall_now).unwrap());
    assert_eq!(text().as_deref(), Some("s7d"));
    assert!(pending_clear_due().is_some());
    // Overdue by the wall clock: cleared now, and nothing is pending.
    assert!(clear_overdue_secret(wall_now + long + Duration::from_secs(1)).unwrap());
    assert_eq!(text(), None);
    assert_eq!(pending_clear_due(), None);
    assert!(!clear_overdue_secret(wall_now + long + Duration::from_secs(1)).unwrap());
    // The late timer finds nothing of its own: what was copied afterwards
    // survives it.
    copy_text("plain-7d").unwrap();
    assert!(holds_for(long + TIMER, || text().as_deref() == Some("plain-7d")));
    // An untimed secret is never overdue; something another app copied
    // over an overdue secret is left alone.
    copy_secret("s7e", None).unwrap();
    assert!(!clear_overdue_secret(SystemTime::now() + Duration::from_secs(3600)).unwrap());
    assert_eq!(text().as_deref(), Some("s7e"));
    copy_secret("s7f", Some(long)).unwrap();
    other_app_copies("theirs-7");
    assert!(!clear_overdue_secret(SystemTime::now() + long + Duration::from_secs(1)).unwrap());
    assert_eq!(text().as_deref(), Some("theirs-7"));
    assert_eq!(pending_clear_due(), None);

    // --- backend errors: nothing pending from a failed copy ----------------
    copy_text("before").unwrap();
    FAIL.store(true, Ordering::SeqCst);
    assert!(matches!(copy_secret("s8", Some(TIMER)), Err(Error::Io(_))));
    assert!(matches!(copy_text("p8"), Err(Error::Io(_))));
    assert!(matches!(clear_pending_secret(), Ok(false)));
    FAIL.store(false, Ordering::SeqCst);
    assert_eq!(text().as_deref(), Some("before"));
    // A pending secret whose clear fails stays forgotten (taken), like
    // today: the caller gets the error.
    copy_secret("s9", None).unwrap();
    FAIL.store(true, Ordering::SeqCst);
    assert!(matches!(clear_pending_secret(), Err(Error::Io(_))));
    FAIL.store(false, Ordering::SeqCst);
    assert!(!clear_pending_secret().unwrap());
    assert!(clipboard::clear_if_equals("s9").unwrap());

    // --- a backend that cannot read clears unconditionally (mobile) --------
    BLIND.store(true, Ordering::SeqCst);
    assert_eq!(read_text().unwrap(), None);
    let clears = CLEARS.load(Ordering::SeqCst);
    copy_secret("s10", Some(TIMER)).unwrap();
    other_app_copies("theirs-3");
    assert!(wait_until(|| text().is_none()), "blind timer clears anyway");
    assert_eq!(CLEARS.load(Ordering::SeqCst), clears + 1);
    copy_secret("s11", None).unwrap();
    other_app_copies("theirs-4");
    assert!(clear_pending_secret().unwrap());
    assert_eq!(text(), None);
    // Plain text is never cleared by a timer, blind or not.
    copy_secret("s12", Some(TIMER)).unwrap();
    copy_text("plain-blind").unwrap();
    assert!(holds_for(TIMER * 4, || {
        text().as_deref() == Some("plain-blind")
    }));
    BLIND.store(false, Ordering::SeqCst);
}
