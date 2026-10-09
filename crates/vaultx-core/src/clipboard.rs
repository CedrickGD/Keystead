//! System clipboard access shared by the desktop app and the TUI.
//!
//! One long-lived [`arboard::Clipboard`] is kept for the whole process: on
//! Linux/X11 the copying process has to serve the clipboard content, so it
//! would vanish as soon as the `Clipboard` is dropped. Secrets are marked
//! as "exclude from clipboard history" (Windows clipboard history/cloud
//! clipboard, KDE password-manager hint on Linux, macOS concealed type) and
//! can be cleared automatically after a delay – but only if the clipboard
//! still holds the copied secret.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use zeroize::Zeroizing;

use crate::error::{Error, Result};

#[cfg(windows)]
use arboard::SetExtWindows as _;

#[cfg(target_os = "macos")]
use arboard::SetExtApple as _;

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
use arboard::SetExtLinux as _;

/// Process-wide clipboard handle (created lazily).
static CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
/// Incremented on every copy; a pending clear only runs if no newer copy
/// happened in between.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The secret awaiting its delayed clear (with its generation).
static PENDING: Mutex<Option<(u64, Zeroizing<String>)>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock cannot leave the data inconsistent.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn map_err(e: arboard::Error) -> Error {
    match e {
        arboard::Error::ClipboardNotSupported => {
            Error::Unsupported("clipboard not available".into())
        }
        other => Error::Io(std::io::Error::other(format!("clipboard: {other}"))),
    }
}

/// Runs `f` with the shared clipboard, retrying briefly while another
/// application holds the (Windows) clipboard open.
fn with_clipboard<R>(
    mut f: impl FnMut(&mut arboard::Clipboard) -> std::result::Result<R, arboard::Error>,
) -> Result<R> {
    let mut guard = lock(CLIPBOARD.get_or_init(|| Mutex::new(None)));
    let mut attempt = 0u64;
    loop {
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(map_err)?);
        }
        let Some(cb) = guard.as_mut() else {
            return Err(Error::Unsupported("clipboard not available".into()));
        };
        match f(cb) {
            Ok(r) => return Ok(r),
            Err(arboard::Error::ClipboardOccupied) if attempt < 5 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(30 * attempt));
            }
            Err(e @ arboard::Error::Unknown { .. }) => {
                // E.g. the display connection broke: reconnect next time.
                *guard = None;
                return Err(map_err(e));
            }
            Err(e) => return Err(map_err(e)),
        }
    }
}

fn set_text(text: &str, sensitive: bool) -> Result<()> {
    with_clipboard(|cb| {
        let set = cb.set();
        #[cfg(any(
            windows,
            target_os = "macos",
            all(
                unix,
                not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
            )
        ))]
        let set = if sensitive {
            set.exclude_from_history()
        } else {
            set
        };
        #[cfg(not(any(
            windows,
            target_os = "macos",
            all(
                unix,
                not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
            )
        )))]
        let _ = sensitive;
        set.text(text.to_owned())
    })
}

/// Copies non-secret text (no history exclusion, no automatic clearing).
pub fn copy_text(text: &str) -> Result<()> {
    set_text(text, false)?;
    GENERATION.fetch_add(1, Ordering::SeqCst);
    *lock(&PENDING) = None;
    Ok(())
}

/// Copies a secret, excluded from clipboard history. With `clear_after`
/// (non-zero), a background thread clears the clipboard after the delay if
/// it still contains `text` and nothing else was copied through this module
/// in the meantime.
pub fn copy_secret(text: &str, clear_after: Option<Duration>) -> Result<()> {
    set_text(text, true)?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(delay) = clear_after.filter(|d| !d.is_zero()) else {
        *lock(&PENDING) = None;
        return Ok(());
    };
    *lock(&PENDING) = Some((generation, Zeroizing::new(text.to_owned())));
    std::thread::Builder::new()
        .name("vaultx-clipboard-clear".into())
        .spawn(move || {
            std::thread::sleep(delay);
            let pending = {
                let mut p = lock(&PENDING);
                match p.as_ref() {
                    Some((g, _)) if *g == generation => p.take(),
                    _ => None,
                }
            };
            if let Some((_, secret)) = pending {
                // Nothing useful can be done with an error in the background.
                let _ = clear_if_equals(&secret);
            }
        })
        .map_err(|e| Error::Io(std::io::Error::other(format!("clipboard timer: {e}"))))?;
    Ok(())
}

/// Clears the clipboard now if it still holds the secret whose delayed clear
/// is pending (e.g. when the vault gets locked). Returns true if cleared.
pub fn clear_pending_secret() -> Result<bool> {
    let pending = lock(&PENDING).take();
    match pending {
        Some((_, secret)) => clear_if_equals(&secret),
        None => Ok(false),
    }
}

/// Clears the clipboard if its text equals `expected`. Returns true if cleared.
pub fn clear_if_equals(expected: &str) -> Result<bool> {
    with_clipboard(|cb| {
        let current = match cb.get_text() {
            Ok(t) => Zeroizing::new(t),
            // Empty or non-text content: nothing of ours to clear.
            Err(arboard::Error::ContentNotAvailable) => return Ok(false),
            Err(e) => return Err(e),
        };
        if current.as_str() == expected {
            cb.clear()?;
            Ok(true)
        } else {
            Ok(false)
        }
    })
}

/// Reads the clipboard text (`None` if empty or not text).
pub fn read_text() -> Result<Option<String>> {
    with_clipboard(|cb| match cb.get_text() {
        Ok(t) => Ok(Some(t)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(e) => Err(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // These need a real clipboard (a display server on Linux). Run with
    // `xvfb-run cargo test -p vaultx-core -- --ignored clipboard`.

    #[test]
    #[ignore = "needs a display/clipboard"]
    fn clipboard_copy_and_delayed_clear() {
        copy_secret("s3cret-value", Some(Duration::from_millis(300))).unwrap();
        assert_eq!(read_text().unwrap().as_deref(), Some("s3cret-value"));
        std::thread::sleep(Duration::from_millis(800));
        assert_ne!(read_text().unwrap().as_deref(), Some("s3cret-value"));

        // Replaced content must survive the timer.
        copy_secret("other-secret", Some(Duration::from_millis(300))).unwrap();
        copy_text("user@example.com").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(read_text().unwrap().as_deref(), Some("user@example.com"));

        // A re-copy of the same secret restarts the timer.
        copy_secret("again", Some(Duration::from_millis(400))).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        copy_secret("again", Some(Duration::from_millis(400))).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(read_text().unwrap().as_deref(), Some("again"));
        std::thread::sleep(Duration::from_millis(500));
        assert_ne!(read_text().unwrap().as_deref(), Some("again"));

        copy_secret("lock-me", Some(Duration::from_secs(60))).unwrap();
        assert!(clear_pending_secret().unwrap());
        assert_ne!(read_text().unwrap().as_deref(), Some("lock-me"));
        assert!(!clear_pending_secret().unwrap());
    }
}
