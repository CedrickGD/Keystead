//! Side effects of the interactive UI (clipboard, browser, clock), behind a
//! trait so the state machine can be tested without a desktop session.

use std::time::Duration;

use keystead_core::clipboard;

/// Operating-system services used by the interactive UI.
pub trait Platform {
    /// Copies a secret (excluded from clipboard history), cleared after
    /// `clear_after` if the clipboard still holds it.
    fn copy_secret(
        &mut self,
        text: &str,
        clear_after: Option<Duration>,
    ) -> keystead_core::Result<()>;
    /// Copies non-secret text.
    fn copy_text(&mut self, text: &str) -> keystead_core::Result<()>;
    /// Clears a pending secret from the clipboard now (lock/quit).
    fn clear_pending_secret(&mut self);
    /// Opens an http(s) URL in the default browser.
    fn open_url(&mut self, url: &str) -> std::io::Result<()>;
    /// Current Unix time in seconds (TOTP).
    fn unix_time(&self) -> u64;
}

/// The real implementation.
#[derive(Debug, Default)]
pub struct SystemPlatform;

impl Platform for SystemPlatform {
    fn copy_secret(
        &mut self,
        text: &str,
        clear_after: Option<Duration>,
    ) -> keystead_core::Result<()> {
        clipboard::copy_secret(text, clear_after)
    }

    fn copy_text(&mut self, text: &str) -> keystead_core::Result<()> {
        clipboard::copy_text(text)
    }

    fn clear_pending_secret(&mut self) {
        // Best effort: nothing sensible can be done if the clipboard is gone.
        let _ = clipboard::clear_pending_secret();
    }

    fn open_url(&mut self, url: &str) -> std::io::Result<()> {
        open::that_detached(url)
    }

    fn unix_time(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Normalises a stored website for opening: adds `https://` if the scheme
/// is missing and only allows http/https (never `file:`, `javascript:` or
/// app schemes). `None` if the URI cannot be opened.
pub fn browser_url(uri: &str) -> Option<String> {
    let uri = uri.trim();
    if uri.is_empty() || uri.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return (uri.len() > lower.find("//").map_or(0, |i| i + 2)).then(|| uri.to_owned());
    }
    // A scheme other than http(s) ("ftp:", "androidapp://", "javascript:").
    // "host:port" (digits after the colon) is not a scheme.
    if let Some((scheme, rest)) = uri.split_once(':') {
        let is_port = rest
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        if !is_port
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return None;
        }
    }
    Some(format!("https://{uri}"))
}

#[cfg(test)]
mod tests {
    use super::browser_url;

    #[test]
    fn browser_urls() {
        assert_eq!(
            browser_url("https://github.com/login").as_deref(),
            Some("https://github.com/login")
        );
        assert_eq!(
            browser_url("HTTP://example.com").as_deref(),
            Some("HTTP://example.com")
        );
        assert_eq!(
            browser_url(" example.com ").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            browser_url("localhost:8080/admin").as_deref(),
            Some("https://localhost:8080/admin")
        );
        assert_eq!(browser_url("javascript:alert(1)"), None);
        assert_eq!(browser_url("file:///etc/passwd"), None);
        assert_eq!(browser_url("androidapp://com.example"), None);
        assert_eq!(browser_url("https://"), None);
        assert_eq!(browser_url(""), None);
        assert_eq!(browser_url("a b"), None);
    }
}
