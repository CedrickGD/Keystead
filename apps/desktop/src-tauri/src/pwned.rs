//! Leaked-password check against Have I Been Pwned's "Pwned Passwords"
//! range API (see "Leaked-password check" in docs/ARCHITECTURE.md).
//!
//! Only on the user's button press, never automatically: `check_pwned_password`
//! for one password (item editor) and `check_pwned_vault` for every distinct
//! login password of the page's vault (security report). Hashing, grouping
//! and response parsing live in `keystead_core::pwned`; this module sends
//! `GET https://api.pwnedpasswords.com/range/<PREFIX>` – the first 5 hex
//! characters of a password's SHA-1, nothing else (no query, no body, no
//! cookies) – and maps the breach counts back to items. Nothing is cached or
//! written into the vault; logs carry counts only, never prefixes.
//!
//! * **Client**: [`REQUEST_TIMEOUT`], no redirects, no referrer, no cookie
//!   store (Cloudflare sets one), user agent `Keystead/<version>` like the
//!   updater, `Add-Padding: true` (every answer then has ~800–1000 lines, so
//!   its size says nothing about the prefix), answer capped at [`MAX_BYTES`].
//! * **Proxy**: the client uses the system proxy. A proxy configuration it
//!   cannot evaluate (PAC script, per-protocol Windows proxy, …) fails closed
//!   with `unsupported:pwned_proxy` before any request – Keystead never goes
//!   around a proxy (the icon fetcher's rule, `icons::proxy_setup_unclear`).
//! * **Errors**: a non-2xx answer is `io:HTTP <status>` (no retry, also for
//!   429), a network error `io:<detail>`. The vault check sends one request
//!   per distinct prefix, at most [`CONCURRENCY`] at a time, and stops at
//!   the first HTTP error, on a lock or vault switch (`locked`) and on app
//!   exit. A network error before any answer ends the run too (probably
//!   offline); one after an answer only leaves its passwords unchecked
//!   (`PwnedReport.failed`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use keystead_core::model::now_ms;
use keystead_core::pwned::{
    breach_candidates, pwned_count, pwned_hash, BreachCandidate, PwnedHash,
};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use zeroize::Zeroizing;

use crate::commands::{CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::icons;
use crate::state::{log, Core};

/// The range API; the prefix is appended (`…/range/5BAA6`).
const RANGE_URL: &str = "https://api.pwnedpasswords.com/range/";
/// Per request: connect, answer and body.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest answer read (a padded range answer is ~80 KB).
const MAX_BYTES: usize = 512 * 1024;
/// Requests of a vault check sent at the same time.
pub const CONCURRENCY: usize = 4;
/// How often a vault check looks for a lock while requests are open.
const CANCEL_POLL: Duration = Duration::from_millis(100);
/// Like the updater's user agent (the service answers without one too).
const USER_AGENT: &str = concat!("Keystead/", env!("CARGO_PKG_VERSION"));
const TOO_LARGE: &str = "answer larger than 512 KiB";

/// Result of `check_pwned_vault`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PwnedReport {
    /// When the check ran (Unix ms).
    pub checked_at: i64,
    /// Distinct passwords of the vault's non-trashed logins (empty ones
    /// skipped) the check covered.
    pub checked: usize,
    /// Every item whose password appears in known breaches, with the count.
    pub leaked: Vec<LeakedItem>,
    /// Distinct passwords whose request failed after others had succeeded:
    /// not known either way (the UI notes it).
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeakedItem {
    pub item_id: String,
    pub count: u64,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `check_pwned_password(password)` → the password's breach count (0: not in
/// known breaches). Only the SHA-1 prefix leaves the process; no vault
/// needed. Counts as user activity.
#[tauri::command]
pub async fn check_pwned_password(core: Shared<'_>, password: String) -> CmdResult<u64> {
    let password = Zeroizing::new(password);
    let core = Arc::clone(core.inner());
    core.touch_activity();
    let service = Service::new()?;
    Ok(check_password(&password, &service).await?)
}

/// `check_pwned_vault(pageVaultId)` → [`PwnedReport`] for the page's vault
/// (`locked` if another vault is open, or if the vault was locked or
/// switched while the requests ran). Counts as user activity.
#[tauri::command]
pub async fn check_pwned_vault(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<PwnedReport> {
    let core = Arc::clone(core.inner());
    core.touch_activity();
    let service = Service::new()?;
    Ok(check_vault(&core, page_vault_id.as_deref(), &service).await?)
}

/// The work of [`check_pwned_password`].
pub(crate) async fn check_password(password: &str, service: &Service) -> AppResult<u64> {
    let hash = pwned_hash(password);
    service.ensure_proxy_clear().await?;
    let body = service
        .range(&hash.prefix)
        .await
        .map_err(RangeError::into_app)?;
    Ok(pwned_count(&body, &hash))
}

/// The work of [`check_pwned_vault`]: (1) under the state lock, the distinct
/// passwords of the page's vault and the lock epoch they belong to; (2)
/// without the lock, the range requests ([`fetch_counts`]); (3) the items
/// behind the leaked passwords – discarded (`locked`) if the vault was
/// locked or switched since (1).
pub(crate) async fn check_vault(
    core: &Arc<Core>,
    page_vault_id: Option<&str>,
    service: &Service,
) -> AppResult<PwnedReport> {
    let (candidates, epoch) = {
        let (core, service) = (Arc::clone(core), service.clone());
        let page_vault_id = page_vault_id.map(str::to_owned);
        tauri::async_runtime::spawn_blocking(move || -> AppResult<(Vec<BreachCandidate>, u64)> {
            let (candidates, epoch) = {
                let st = core.state();
                let vault = st.page_vault(page_vault_id.as_deref())?;
                (breach_candidates(vault.data()), core.lock_epoch())
            };
            if !candidates.is_empty() {
                service.check_proxy()?;
            }
            Ok((candidates, epoch))
        })
        .await
        .map_err(AppError::io)??
    };
    let checked = candidates.len();

    let weak = Arc::downgrade(core);
    let cancelled: Arc<Stop> = Arc::new(move || {
        weak.upgrade()
            .is_none_or(|c| c.is_exiting() || c.lock_epoch() != epoch)
    });
    let counts = fetch_counts(service, &candidates, cancelled).await?;

    if core.is_exiting() || core.lock_epoch() != epoch {
        return Err(AppError::Locked);
    }
    let mut leaked = Vec::new();
    let mut failed = 0;
    for (candidate, count) in candidates.iter().zip(&counts) {
        match count {
            Some(0) => {}
            Some(count) => leaked.extend(candidate.item_ids.iter().map(|id| LeakedItem {
                item_id: id.clone(),
                count: *count,
            })),
            None => failed += 1,
        }
    }
    leaked.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
    log(format_args!(
        "pwned check: {checked} distinct passwords, {} leaked items, {failed} not checked",
        leaked.len()
    ));
    Ok(PwnedReport {
        checked_at: now_ms(),
        checked,
        leaked,
        failed,
    })
}

/// "Stop now" check of a vault check (locked, switched, quitting).
type Stop = dyn Fn() -> bool + Send + Sync;

/// Step 2 of the vault check: one request per distinct prefix (passwords
/// sharing a prefix share the answer), at most [`CONCURRENCY`] at a time.
/// Returns every candidate's breach count, `None` where its request failed.
/// Stops at the first HTTP error (`io:HTTP <status>`), once `cancelled`
/// (`locked`) and at a network error before any answer (`io:<detail>`); the
/// open requests are dropped then. A network error after an answer only
/// leaves its candidates unchecked.
async fn fetch_counts(
    service: &Service,
    candidates: &[BreachCandidate],
    cancelled: Arc<Stop>,
) -> AppResult<Vec<Option<u64>>> {
    let mut by_prefix: BTreeMap<&str, Vec<(usize, PwnedHash)>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        by_prefix
            .entry(candidate.hash.prefix.as_str())
            .or_default()
            .push((index, candidate.hash.clone()));
    }
    let semaphore = Arc::new(Semaphore::new(CONCURRENCY));
    // The status of the first HTTP error, recorded by the request that got
    // it before its permit is released: a request waiting for a permit then
    // stops instead of connecting (the coordinator below reacts a moment
    // later, and the runtime may poll the waiter first).
    let http_error = Arc::new(AtomicU16::new(0));
    let mut tasks: JoinSet<Result<Vec<(usize, u64)>, RangeError>> = JoinSet::new();
    for (prefix, hashes) in by_prefix {
        let (service, prefix, semaphore, cancelled, http_error) = (
            service.clone(),
            prefix.to_owned(),
            Arc::clone(&semaphore),
            Arc::clone(&cancelled),
            Arc::clone(&http_error),
        );
        tasks.spawn(async move {
            // The semaphore is never closed; the error cannot happen.
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| RangeError::Cancelled)?;
            if cancelled() {
                return Err(RangeError::Cancelled);
            }
            match http_error.load(Ordering::SeqCst) {
                0 => {}
                status => return Err(RangeError::Http(status)),
            }
            let body = match service.range(&prefix).await {
                Ok(body) => body,
                Err(RangeError::Http(status)) => {
                    let _ =
                        http_error.compare_exchange(0, status, Ordering::SeqCst, Ordering::SeqCst);
                    return Err(RangeError::Http(status));
                }
                Err(other) => return Err(other),
            };
            Ok(hashes
                .iter()
                .map(|(index, hash)| (*index, pwned_count(&body, hash)))
                .collect())
        });
    }

    let mut counts = vec![None; candidates.len()];
    let mut answered = false;
    loop {
        let stop = match tokio::time::timeout(CANCEL_POLL, tasks.join_next()).await {
            Ok(None) => break,
            Ok(Some(Ok(Ok(found)))) => {
                answered = true;
                for (index, count) in found {
                    counts[index] = Some(count);
                }
                None
            }
            Ok(Some(Ok(Err(RangeError::Http(status))))) => {
                Some(AppError::io(format!("HTTP {status}")))
            }
            // Nothing answered yet: probably offline.
            Ok(Some(Ok(Err(RangeError::Network(detail))))) if !answered => {
                Some(AppError::io(detail))
            }
            // After an answer, a failed request (or a task that panicked)
            // leaves its passwords unchecked.
            Ok(Some(Ok(Err(RangeError::Network(_))))) | Ok(Some(Err(_))) => None,
            Ok(Some(Ok(Err(RangeError::Cancelled)))) => Some(AppError::Locked),
            Err(_) if cancelled() => Some(AppError::Locked),
            Err(_) => None,
        };
        if let Some(error) = stop {
            // Drops the open requests (and their connections) now.
            tasks.shutdown().await;
            return Err(error);
        }
    }
    Ok(counts)
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Why a range request has no answer.
#[derive(Debug)]
enum RangeError {
    /// The service answered with an error status: the run stops.
    Http(u16),
    /// No complete answer: connect, timeout, body, too large.
    Network(String),
    /// Not sent: the run was cancelled first.
    Cancelled,
}

impl RangeError {
    fn network(e: reqwest::Error) -> Self {
        RangeError::Network(network_detail(e))
    }

    fn into_app(self) -> AppError {
        match self {
            RangeError::Http(status) => AppError::io(format!("HTTP {status}")),
            RangeError::Network(detail) => AppError::io(detail),
            RangeError::Cancelled => AppError::Locked,
        }
    }
}

/// A reqwest error for the UI: without the URL (it would carry the prefix),
/// with the causes behind it (reqwest's own message is only "error sending
/// request").
fn network_detail(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut detail = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !text.is_empty() && !detail.ends_with(&text) {
            detail.push_str(": ");
            detail.push_str(&text);
        }
        source = cause.source();
    }
    detail
}

/// Sends the range requests (see the module docs). Cheap to clone.
#[derive(Clone)]
pub(crate) struct Service {
    client: reqwest::Client,
    /// [`RANGE_URL`]; tests point it at a local server.
    base: String,
    /// Reads the proxy settings before a request; true → fail closed.
    proxy_unclear: fn() -> bool,
}

impl Service {
    /// The production service: the shared client, the real API, the system
    /// proxy rule.
    pub fn new() -> AppResult<Self> {
        Ok(Service {
            client: client()?,
            base: RANGE_URL.to_owned(),
            proxy_unclear: system_proxy_unclear,
        })
    }

    /// A service for a local plain-http test server at `origin`
    /// (`http://127.0.0.1:<port>/`): same request shape, no proxy, short
    /// timeout, proxy settings never unclear.
    #[cfg(test)]
    fn local(origin: &str) -> Self {
        Service {
            client: client_builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .connect_timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            base: format!("{origin}range/"),
            proxy_unclear: || false,
        }
    }

    /// `unsupported:pwned_proxy` if the proxy configuration cannot be
    /// evaluated (reads the environment and, on Windows, the registry).
    fn check_proxy(&self) -> AppResult<()> {
        if (self.proxy_unclear)() {
            return Err(AppError::unsupported("pwned_proxy"));
        }
        Ok(())
    }

    /// [`Service::check_proxy`] off the async runtime.
    async fn ensure_proxy_clear(&self) -> AppResult<()> {
        let service = self.clone();
        tauri::async_runtime::spawn_blocking(move || service.check_proxy())
            .await
            .map_err(AppError::io)?
    }

    /// `GET <base><prefix>` with `Accept: text/plain` and `Add-Padding:
    /// true`: the answer's text (at most [`MAX_BYTES`]).
    async fn range(&self, prefix: &str) -> Result<String, RangeError> {
        let mut response = self
            .client
            .get(format!("{}{prefix}", self.base))
            .header(reqwest::header::ACCEPT, "text/plain")
            .header("Add-Padding", "true")
            .send()
            .await
            .map_err(RangeError::network)?;
        let status = response.status();
        if !status.is_success() {
            return Err(RangeError::Http(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(RangeError::Network(TOO_LARGE.into()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(RangeError::network)? {
            if body.len() + chunk.len() > MAX_BYTES {
                return Err(RangeError::Network(TOO_LARGE.into()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

/// The icon fetcher's rule: some proxy is configured, but the matcher
/// reqwest's `system-proxy` uses would still connect directly (PAC script,
/// per-protocol Windows proxy) – the request would go around the proxy.
fn system_proxy_unclear() -> bool {
    icons::proxy_setup_unclear(&icons::proxy_matcher())
}

/// The request shape shared by the production and the test client.
fn client_builder() -> reqwest::ClientBuilder {
    // `rustls-no-provider` (shared with the updater and the icon fetcher):
    // install ring as the process default before the first client is built.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    // No cookie store (reqwest has none without the `cookies` feature), so
    // Cloudflare's cookie is never sent back.
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(REQUEST_TIMEOUT)
        .referer(false)
        .redirect(reqwest::redirect::Policy::none())
}

/// The production client (system proxy), built once.
fn client() -> AppResult<reqwest::Client> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| client_builder().build().map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| AppError::io(format!("client: {e}")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::time::Instant;

    use keystead_core::model::{ItemType, LoginData, VaultItem};
    use keystead_core::settings::Settings;

    use super::*;
    use crate::icons::{proxy_unclear_from, SystemProxyHints};
    use crate::state::test_support::core_with_open_vault;

    /// SHA-1("password") = 5BAA61E4C9B93F3F0682250B6CF8331B7EE68FD8.
    const PREFIX: &str = "5BAA6";
    const SUFFIX: &str = "1E4C9B93F3F0682250B6CF8331B7EE68FD8";
    /// Lines the service pads every answer with (count 0).
    const PADDING: &str =
        "0018A45C4D1DEF81644B54AB7F969B88D65:0\r\n00D4F6E8FA6EECAD2A3AA415EEC418D38EC:0\r\n";

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    // -----------------------------------------------------------------
    // A local range API
    // -----------------------------------------------------------------

    /// Status 0: the connection is closed without an answer.
    const DROP: u16 = 0;

    #[derive(Clone)]
    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
        delay: Duration,
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            headers: Vec::new(),
            body: body.to_owned(),
            delay: Duration::ZERO,
        }
    }

    fn after(delay: Duration, reply: Reply) -> Reply {
        Reply { delay, ..reply }
    }

    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        headers: Vec<(String, String)>,
        at: Instant,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        }
    }

    struct Server {
        origin: String,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }

        fn paths(&self) -> Vec<String> {
            self.seen().into_iter().map(|s| s.path).collect()
        }
    }

    /// A plain-http server on 127.0.0.1; `route(path)` answers each request
    /// (one thread per connection, `Connection: close`).
    fn serve(route: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let route = Arc::new(route);
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (route, log) = (Arc::clone(&route), Arc::clone(&log));
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        return;
                    }
                    let mut headers = Vec::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) <= 2 {
                            break;
                        }
                        if let Some((k, v)) = line.trim_end().split_once(':') {
                            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
                        }
                    }
                    let path = request_line.split(' ').nth(1).unwrap_or("/").to_owned();
                    log.lock().unwrap().push(Seen {
                        path: path.clone(),
                        headers,
                        at: Instant::now(),
                    });
                    let r = route(&path);
                    std::thread::sleep(r.delay);
                    if r.status == DROP {
                        return;
                    }
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                        r.status,
                        r.body.len()
                    );
                    for (k, v) in &r.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(r.body.as_bytes());
                });
            }
        });
        Server { origin, seen }
    }

    /// The URL path the check requests for `password`.
    fn path_of(password: &str) -> String {
        format!("/range/{}", pwned_hash(password).prefix)
    }

    /// A padded answer listing `password` with `count`.
    fn listing(password: &str, count: u64) -> String {
        format!(
            "{PADDING}{}:{count}\r\n",
            pwned_hash(password).suffix.as_str()
        )
    }

    fn login(password: &str) -> VaultItem {
        let mut item = VaultItem::new(ItemType::Login, "Login");
        item.login = Some(LoginData {
            username: "me".into(),
            password: password.into(),
            ..Default::default()
        });
        item
    }

    fn save(core: &Core, item: VaultItem) -> String {
        core.mutate(|v| v.save_item(item.clone())).unwrap().id
    }

    fn leaked(report: &PwnedReport) -> Vec<(&str, u64)> {
        report
            .leaked
            .iter()
            .map(|l| (l.item_id.as_str(), l.count))
            .collect()
    }

    // -----------------------------------------------------------------
    // One password
    // -----------------------------------------------------------------

    #[test]
    fn the_request_carries_only_the_prefix() {
        let body = format!("{PADDING}{SUFFIX}:3861493\r\n");
        let server = serve(move |path| {
            if path == "/range/5BAA6" {
                Reply {
                    // Cloudflare's cookie must not come back.
                    headers: vec![("Set-Cookie", "__cf_bm=abc; Path=/; HttpOnly".into())],
                    ..reply(200, &body)
                }
            } else {
                reply(200, PADDING)
            }
        });
        let service = Service::local(&server.origin);
        assert_eq!(
            block_on(check_password("password", &service)).unwrap(),
            3_861_493
        );
        assert_eq!(
            block_on(check_password("password", &service)).unwrap(),
            3_861_493
        );
        assert_eq!(
            block_on(check_password("Tr0ub4dor&3", &service)).unwrap(),
            0
        );
        let seen = server.seen();
        assert_eq!(
            seen.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(),
            vec![
                "/range/5BAA6",
                "/range/5BAA6",
                path_of("Tr0ub4dor&3").as_str()
            ]
        );
        for s in &seen {
            // Exactly the prefix: no query, no suffix.
            assert_eq!(s.path.len(), "/range/".len() + PREFIX.len(), "{}", s.path);
            assert_eq!(s.header("add-padding"), Some("true"));
            assert_eq!(s.header("accept"), Some("text/plain"));
            assert_eq!(s.header("user-agent"), Some(USER_AGENT));
            assert!(USER_AGENT.starts_with("Keystead/"));
            assert_eq!(s.header("cookie"), None, "{s:?}");
            assert_eq!(s.header("referer"), None);
            assert_eq!(s.header("authorization"), None);
        }
    }

    #[test]
    fn error_answers_are_http_errors_and_redirects_are_not_followed() {
        let server = serve(|path| match path {
            "/range/5BAA6" => reply(429, "Rate limited"),
            "/elsewhere" => reply(200, PADDING),
            path if path == path_of("Tr0ub4dor&3") => reply(503, ""),
            _ => Reply {
                headers: vec![("Location", "/elsewhere".into())],
                ..reply(302, "")
            },
        });
        let service = Service::local(&server.origin);
        let code = |password: &str| {
            block_on(check_password(password, &service))
                .unwrap_err()
                .code()
        };
        assert_eq!(code("password"), "io:HTTP 429");
        assert_eq!(code("Tr0ub4dor&3"), "io:HTTP 503");
        assert_eq!(code("something else"), "io:HTTP 302");
        assert!(!server.paths().contains(&"/elsewhere".to_owned()));
    }

    #[test]
    fn network_errors_and_oversized_answers() {
        // Nobody listening: a network error without the URL (the prefix).
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/", listener.local_addr().unwrap())
        };
        let err = block_on(check_password("password", &Service::local(&closed))).unwrap_err();
        let code = err.code();
        assert!(
            code.starts_with("io:") && code.len() > "io:".len(),
            "{code}"
        );
        assert!(!code.contains("range") && !code.contains(PREFIX), "{code}");

        // The connection is closed before an answer.
        let dropped = serve(|_| reply(DROP, ""));
        let code = block_on(check_password("password", &Service::local(&dropped.origin)))
            .unwrap_err()
            .code();
        assert!(code.starts_with("io:"), "{code}");

        // An answer larger than the cap is refused (also with a wrong length).
        let huge = "A".repeat(MAX_BYTES + 1);
        let server = serve(move |_| reply(200, &huge));
        let code = block_on(check_password("password", &Service::local(&server.origin)))
            .unwrap_err()
            .code();
        assert_eq!(code, format!("io:{TOO_LARGE}"));
    }

    #[test]
    fn unclear_proxy_setups_fail_closed_before_any_request() {
        let server = serve(|_| reply(200, PADDING));
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        save(&core, login("password"));

        // A PAC script (as the icon fetcher judges it): nothing is sent.
        let mut service = Service::local(&server.origin);
        service.proxy_unclear = || {
            proxy_unclear_from(
                false,
                &SystemProxyHints {
                    auto_config: true,
                    manual_enabled: false,
                },
                false,
            )
        };
        assert_eq!(
            block_on(check_password("password", &service))
                .unwrap_err()
                .code(),
            "unsupported:pwned_proxy"
        );
        assert_eq!(
            block_on(check_vault(&core, Some(&id), &service))
                .unwrap_err()
                .code(),
            "unsupported:pwned_proxy"
        );
        assert!(server.paths().is_empty());

        // A proxy the client itself uses: no refusal.
        service.proxy_unclear = || proxy_unclear_from(true, &SystemProxyHints::default(), true);
        assert_eq!(block_on(check_password("password", &service)).unwrap(), 0);
        assert_eq!(server.paths().len(), 1);

        // The production service applies the system rule and the real API.
        let production = Service::new().unwrap();
        assert!(std::ptr::fn_addr_eq(
            production.proxy_unclear,
            system_proxy_unclear as fn() -> bool
        ));
        assert_eq!(production.base, "https://api.pwnedpasswords.com/range/");
    }

    // -----------------------------------------------------------------
    // The vault
    // -----------------------------------------------------------------

    #[test]
    fn vault_check_maps_counts_to_items_and_skips_trash() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        let body = format!("{PADDING}{SUFFIX}:3861493\r\n");
        let server = serve(move |path| {
            if path == "/range/5BAA6" {
                reply(200, &body)
            } else {
                reply(200, PADDING)
            }
        });
        let service = Service::local(&server.origin);

        // No logins yet: nothing to ask.
        let report = block_on(check_vault(&core, Some(&id), &service)).unwrap();
        assert_eq!(
            report,
            PwnedReport {
                checked_at: report.checked_at,
                checked: 0,
                leaked: vec![],
                failed: 0,
            }
        );
        assert!(server.paths().is_empty());

        let a = save(&core, login("password"));
        let b = save(&core, login("password"));
        let c = save(&core, login("Tr0ub4dor&3"));
        // Skipped: trashed, empty password, a note.
        let trashed = save(&core, login("password"));
        core.mutate(|v| v.trash_item(&trashed)).unwrap();
        save(&core, login(""));
        save(&core, VaultItem::new(ItemType::Note, "Note"));

        let started = now_ms();
        let report = block_on(check_vault(&core, Some(&id), &service)).unwrap();
        assert!(report.checked_at >= started);
        assert_eq!(report.checked, 2, "distinct passwords");
        assert_eq!(report.failed, 0);
        let mut expected = vec![(a.as_str(), 3_861_493), (b.as_str(), 3_861_493)];
        expected.sort();
        assert_eq!(leaked(&report), expected);
        assert!(!report.leaked.iter().any(|l| l.item_id == c));
        // One request per distinct password, none for the trashed copy.
        let mut paths = server.paths();
        paths.sort();
        assert_eq!(
            paths,
            vec!["/range/5BAA6".to_owned(), path_of("Tr0ub4dor&3")]
        );

        // Another page's vault, or none: `locked`, no request.
        for page in [None, Some("other-vault")] {
            assert_eq!(
                block_on(check_vault(&core, page, &service))
                    .unwrap_err()
                    .code(),
                "locked"
            );
        }
        assert_eq!(server.paths().len(), 2);

        let json = serde_json::to_value(&report).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["checked", "checkedAt", "failed", "leaked"]);
        assert_eq!(json["checkedAt"], report.checked_at);
        assert_eq!(json["leaked"][0]["itemId"], expected[0].0);
        assert_eq!(json["leaked"][0]["count"], 3_861_493);
        assert_eq!(json["leaked"][0].as_object().unwrap().len(), 2);
    }

    /// Two different passwords whose SHA-1 starts with the same 5 hex
    /// characters (the birthday bound finds them after ~1200 hashes).
    fn colliding_passwords() -> (String, String) {
        let mut by_prefix: HashMap<String, String> = HashMap::new();
        for i in 0.. {
            let password = format!("p{i}");
            let prefix = pwned_hash(&password).prefix;
            if let Some(other) = by_prefix.insert(prefix, password.clone()) {
                return (other, password);
            }
        }
        unreachable!()
    }

    #[test]
    fn passwords_sharing_a_prefix_share_one_request() {
        let (first, second) = colliding_passwords();
        assert_ne!(first, second);
        assert_eq!(pwned_hash(&first).prefix, pwned_hash(&second).prefix);
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        let x = save(&core, login(&first));
        let y = save(&core, login(&second));
        // The answer lists the first one (count 7) and the second as padding.
        let body = format!(
            "{}{}:0\r\n",
            listing(&first, 7),
            pwned_hash(&second).suffix.as_str()
        );
        let path = path_of(&first);
        let server = serve(move |p| {
            assert_eq!(p, path);
            reply(200, &body)
        });
        let report = block_on(check_vault(
            &core,
            Some(&id),
            &Service::local(&server.origin),
        ))
        .unwrap();
        assert_eq!(report.checked, 2);
        assert_eq!(report.failed, 0);
        assert_eq!(leaked(&report), vec![(x.as_str(), 7)]);
        assert!(!report.leaked.iter().any(|l| l.item_id == y));
        assert_eq!(server.paths().len(), 1);
    }

    #[test]
    fn leaked_items_are_sorted_by_count() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        let few = save(&core, login("few"));
        let many = save(&core, login("many"));
        let none = save(&core, login("none"));
        let server = serve(|path| {
            if path == path_of("few") {
                reply(200, &listing("few", 12))
            } else if path == path_of("many") {
                reply(200, &listing("many", 500_000))
            } else {
                reply(200, PADDING)
            }
        });
        let report = block_on(check_vault(
            &core,
            Some(&id),
            &Service::local(&server.origin),
        ))
        .unwrap();
        assert_eq!(
            leaked(&report),
            vec![(many.as_str(), 500_000), (few.as_str(), 12)]
        );
        assert!(!report.leaked.iter().any(|l| l.item_id == none));
        assert_eq!((report.checked, report.failed), (3, 0));
    }

    #[test]
    fn an_http_error_aborts_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        for n in 0..8 {
            save(&core, login(&format!("pw{n}")));
        }
        // Every answer is a rate limit, 300 ms after the request.
        let server = serve(|_| after(Duration::from_millis(300), reply(429, "")));
        let err = block_on(check_vault(
            &core,
            Some(&id),
            &Service::local(&server.origin),
        ))
        .unwrap_err();
        assert_eq!(err.code(), "io:HTTP 429");
        // The first wave was sent, nothing after the first error (the waiting
        // requests would have followed the answers at 300 ms) – not even a
        // connection that is dropped before the request.
        std::thread::sleep(Duration::from_millis(600));
        let seen = server.seen();
        assert!(!seen.is_empty() && seen.len() <= CONCURRENCY, "{seen:?}");
        assert!(
            seen.iter().all(|s| s.path.starts_with("/range/")),
            "{seen:?}"
        );
    }

    #[test]
    fn a_network_error_after_an_answer_leaves_the_password_unchecked() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        let ok = save(&core, login("answered"));
        let unknown = save(&core, login("dropped"));
        // One prefix is answered at once, the other connection is closed
        // without an answer 300 ms later.
        let server = serve(|path| {
            if path == path_of("answered") {
                reply(200, &listing("answered", 3))
            } else {
                after(Duration::from_millis(300), reply(DROP, ""))
            }
        });
        let report = block_on(check_vault(
            &core,
            Some(&id),
            &Service::local(&server.origin),
        ))
        .unwrap();
        assert_eq!(report.checked, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(leaked(&report), vec![(ok.as_str(), 3)]);
        assert!(!report.leaked.iter().any(|l| l.item_id == unknown));

        // Nothing answers: offline, the whole check fails.
        let offline = serve(|_| reply(DROP, ""));
        let code = block_on(check_vault(
            &core,
            Some(&id),
            &Service::local(&offline.origin),
        ))
        .unwrap_err()
        .code();
        assert!(code.starts_with("io:"), "{code}");
    }

    #[test]
    fn a_lock_or_vault_switch_during_the_run_discards_it() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with_open_vault(dir.path(), Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        for n in 0..6 {
            save(&core, login(&format!("slow{n}")));
        }
        // Every answer takes 700 ms; without cancelling, the two waiting
        // requests would follow the first four.
        let server = serve(|_| after(Duration::from_millis(700), reply(200, PADDING)));
        let service = Service::local(&server.origin);

        // A new lock epoch while the requests run – a vault switch leaves a
        // vault open, so only the epoch tells.
        let switched_at = Arc::new(Mutex::new(None::<Instant>));
        let switcher = {
            let (core, switched_at) = (Arc::clone(&core), Arc::clone(&switched_at));
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                core.begin_lock();
                *switched_at.lock().unwrap() = Some(Instant::now());
            })
        };
        let err = block_on(check_vault(&core, Some(&id), &service)).unwrap_err();
        let returned = Instant::now();
        switcher.join().unwrap();
        assert_eq!(err.code(), "locked");
        let switched_at = switched_at.lock().unwrap().unwrap();
        // The open requests were dropped instead of awaited …
        assert!(
            returned < switched_at + Duration::from_millis(500),
            "returned {:?} after the switch",
            returned.saturating_duration_since(switched_at)
        );
        // … and no request was sent afterwards.
        std::thread::sleep(Duration::from_millis(900));
        let seen = server.seen();
        assert_eq!(seen.len(), CONCURRENCY, "{seen:?}");
        assert!(seen.iter().all(|s| s.at < switched_at), "{seen:?}");

        // A real lock during the run: the same.
        let locker = {
            let core = Arc::clone(&core);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                core.lock(None);
            })
        };
        let err = block_on(check_vault(&core, Some(&id), &service)).unwrap_err();
        locker.join().unwrap();
        assert_eq!(err.code(), "locked");
        assert!(core.state().vault.is_none());
    }

    /// Asks the real service about "password" (> 50 million breaches) with
    /// the production client (system proxy):
    /// `cargo test -p keystead-desktop -- --ignored --nocapture live_range`
    #[test]
    #[ignore = "needs the internet"]
    fn live_range_api_knows_password() {
        let service = Service::new().unwrap();
        let started = Instant::now();
        let count = block_on(check_password("password", &service)).unwrap();
        println!("\"password\": {count} breaches ({:?})", started.elapsed());
        assert!(count > 50_000_000, "{count}");
    }
}
