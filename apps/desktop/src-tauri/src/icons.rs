//! Website icons (see "Website icons" in docs/ARCHITECTURE.md).
//!
//! The app loads the icon of every login's website itself, directly from
//! that website – no icon service, no proxy – and stores it inside the
//! encrypted vault (`keystead_core::icons`, `UnlockedVault::set_icons`):
//!
//! * **When**: 5 s after a vault is opened, ~2 s after a change (a login
//!   with a new website was saved, also by the browser extension), and every
//!   24 h while the vault is open; only while `Settings.websiteIcons` is on.
//!   At most [`MAX_HOSTS_PER_RUN`] sites per run, [`CONCURRENCY`] at a time;
//!   a run that hit the limit is followed by the next one a minute later.
//!   Locking, switching the vault or turning the setting off cancels a run
//!   at once – open requests are dropped, no new one starts; results are
//!   only written into the vault the run started for.
//! * **How** ([`Fetcher`]): `GET https://<host>/` (only https, ≤ 3
//!   redirects, each again https on the default port to a host that may be
//!   contacted, 5 s timeout, ≤ 512 KiB, no cookies, no referrer, user agent
//!   `Keystead/<version> (icon fetcher)`), the best `<link rel=icon |
//!   shortcut icon | apple-touch-icon>` of the page's head (PNG/ICO
//!   preferred, ≥ 32 px; no SVG, `data:` only as `image/png;base64`), then
//!   `https://<host>/favicon.ico`. Decoded (PNG, ICO, JPEG, GIF, WebP; at
//!   most [`MAX_DIMENSION`] px per side) on the blocking thread pool, scaled
//!   into 64×64 (aspect kept, transparent padding), stored as PNG.
//! * **Privacy & SSRF guards**: hosts that are IP literals, `localhost`,
//!   `.local`/intranet names are never contacted
//!   (`keystead_core::icons::is_fetchable_host`); every connection goes
//!   through [`PublicResolver`], which only hands out public addresses, so
//!   a site (or a redirect, or an icon link) cannot make the app talk to the
//!   local network. No vault data is ever sent. The app never goes around a
//!   proxy: if the system or the environment sets one for a site, that site
//!   is skipped ([`proxy_matcher`]).
//! * Failures are silent (logged as counts, never host names): the site's
//!   entry gets `failedAt` and is retried after 7 days. A run in which not a
//!   single site answered counts as **offline** and records nothing (retried
//!   after 1 h, doubling up to 24 h).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use hyper_util::client::proxy::matcher::Matcher as ProxyMatcher;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, ImageEncoder as _, ImageFormat, ImageReader, Limits, RgbaImage};
use keystead_core::icons::{self as core_icons, ICON_SIZE};
use keystead_core::model::{now_ms, ItemSummary};
use keystead_core::UnlockedVault;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tauri::Url;

use crate::commands::{run, CmdResult, Shared};
use crate::error::AppError;
use crate::state::{log, Core};

/// `vault://icons` – payload `{}`: new icons were stored, the UI re-reads
/// them (`get_icons`).
pub const EVENT_ICONS: &str = "vault://icons";

/// First run after a vault was opened.
const OPEN_DELAY: Duration = Duration::from_secs(5);
/// Run after a change (debounce: an import saves once, a user saves a few
/// times in a row).
const CHANGE_DELAY: Duration = Duration::from_secs(2);
/// Regular run while the vault stays open.
const RUN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Next run when the last one stopped at [`MAX_HOSTS_PER_RUN`].
const MORE_DUE_DELAY: Duration = Duration::from_secs(60);
/// First retry after a run that found no network.
const OFFLINE_RETRY: Duration = Duration::from_secs(60 * 60);

/// Sites per run.
pub const MAX_HOSTS_PER_RUN: usize = 200;
/// Sites fetched at the same time.
pub const CONCURRENCY: usize = 4;
/// Results are written into the vault in batches (each one save) …
const PERSIST_BATCH: usize = 16;
/// … or after this long, so icons appear while a long run goes on.
const PERSIST_EVERY: Duration = Duration::from_secs(5);

/// How often a run checks whether it was cancelled while requests are open.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// Timeout of every request (connect + answer + body).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Everything for one site (page, up to [`MAX_CANDIDATES`] icons, favicon,
/// decoding).
const HOST_DEADLINE: Duration = Duration::from_secs(15);
/// Redirects per request.
const MAX_REDIRECTS: usize = 3;
/// Largest page or image read (bytes). A longer page is cut (the head comes
/// first); a larger image is refused.
const MAX_BYTES: usize = 512 * 1024;
/// Icon links of a page tried before `/favicon.ico`.
const MAX_CANDIDATES: usize = 3;
/// Decoder limits: largest image side (no favicon needs more; it bounds the
/// decoding work per image) and decoder memory.
const MAX_DIMENSION: u32 = 1024;
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;
/// Smaller images are no icon (tracking pixels, spacers).
const MIN_DIMENSION: u32 = 8;

/// Icons the browser bridge attaches to `logins_for_url` / `search` rows:
/// only for the first rows of a reply, and only small ones (the reply to
/// the browser is capped at 1 MiB).
pub const BRIDGE_ICON_ROWS: usize = 20;
pub const BRIDGE_ICON_MAX_LEN: usize = 16 * 1024;

const ACCEPT_HTML: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5";
const ACCEPT_IMAGE: &str = "image/png,image/x-icon,image/webp,image/*;q=0.8,*/*;q=0.5";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Tracker (part of `Core`) and scheduler thread
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    /// A vault was opened (unlock, new vault, switch).
    Opened,
    /// The open vault was closed.
    Closed,
    /// The vault was changed, or the setting turned on: check soon.
    Changed,
}

/// The icon scheduler's view of the vault session, held by [`Core`]: the
/// state hooks (`install_vault`, `finish_lock`, `mutate_with`, settings)
/// call it; a run started under an older `generation` stops.
#[derive(Default)]
pub struct IconTracker {
    generation: AtomicU64,
    wake: Mutex<Option<Sender<Signal>>>,
}

impl IconTracker {
    fn send(&self, signal: Signal) {
        if let Some(tx) = lock(&self.wake).as_ref() {
            let _ = tx.send(signal);
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn cancel_runs(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// A vault was opened: a run for it starts in 5 s; a run for the vault
    /// before (if any) stops.
    pub fn vault_opened(&self) {
        self.cancel_runs();
        self.send(Signal::Opened);
    }

    /// The open vault was closed: a running fetch stops, nothing is written.
    pub fn vault_closed(&self) {
        self.cancel_runs();
        self.send(Signal::Closed);
    }

    /// The open vault changed (maybe a login with a new website).
    pub fn vault_changed(&self) {
        self.send(Signal::Changed);
    }

    /// `websiteIcons` was switched: on → run soon, off → stop now.
    pub fn setting_changed(&self, enabled: bool) {
        if enabled {
            self.send(Signal::Changed);
        } else {
            self.cancel_runs();
        }
    }

    /// Disconnects the scheduler (app shutdown).
    pub fn stop(&self) {
        self.cancel_runs();
        *lock(&self.wake) = None;
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOutcome {
    /// `websiteIcons` is off.
    Disabled,
    /// No vault open, or it was closed/switched meanwhile.
    Locked,
    /// Locked, switched or setting turned off during the run.
    Cancelled,
    /// Nothing was due.
    Idle,
    /// Every due site would go through a proxy: nothing fetched.
    Proxied,
    /// Stored the results; `more_due`: the run stopped at the limit.
    Done { more_due: bool },
    /// No site answered: nothing recorded.
    Offline,
    /// Writing the vault failed (logged); retried with the next run.
    StoreFailed,
}

/// Starts the scheduler thread (GUI mode).
pub fn spawn_scheduler(core: &Arc<Core>) {
    match Fetcher::new(core.app_version()) {
        Ok(fetcher) => {
            start_scheduler(core, fetcher);
        }
        Err(e) => log(format_args!("website icons disabled: {e}")),
    }
}

/// Starts the scheduler thread with `fetcher`; it ends when the core is
/// gone or [`IconTracker::stop`] disconnects it.
fn start_scheduler(core: &Arc<Core>, fetcher: Fetcher) -> Option<std::thread::JoinHandle<()>> {
    let (tx, rx) = mpsc::channel();
    *lock(&core.icons().wake) = Some(tx);
    let weak = Arc::downgrade(core);
    let spawned = std::thread::Builder::new()
        .name("keystead-icons".into())
        .spawn(move || scheduler_loop(&weak, &rx, &fetcher));
    match spawned {
        Ok(handle) => Some(handle),
        Err(e) => {
            log(format_args!("could not start the icon fetcher: {e}"));
            None
        }
    }
}

fn scheduler_loop(core: &Weak<Core>, signals: &mpsc::Receiver<Signal>, fetcher: &Fetcher) {
    let mut next_run: Option<Instant> = None;
    // Earliest start of a run caused by a change (after a run that stopped
    // at the limit).
    let mut cooldown_until = Instant::now();
    let mut offline_retry = OFFLINE_RETRY;
    loop {
        let signal = match next_run {
            None => signals.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(at) => signals.recv_timeout(at.saturating_duration_since(Instant::now())),
        };
        let now = Instant::now();
        match signal {
            Ok(Signal::Opened) => {
                next_run = Some(now + OPEN_DELAY);
                cooldown_until = now;
                offline_retry = OFFLINE_RETRY;
            }
            Ok(Signal::Closed) => next_run = None,
            Ok(Signal::Changed) => {
                let at = (now + CHANGE_DELAY).max(cooldown_until);
                next_run = Some(next_run.map_or(at, |n| n.min(at)));
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let Some(core) = core.upgrade() else { return };
                if core.is_exiting() {
                    return;
                }
                let outcome = run_once(&core, fetcher);
                let now = Instant::now();
                next_run = match outcome {
                    RunOutcome::Disabled | RunOutcome::Locked | RunOutcome::Cancelled => None,
                    RunOutcome::Idle
                    | RunOutcome::Proxied
                    | RunOutcome::Done { more_due: false } => {
                        cooldown_until = now;
                        offline_retry = OFFLINE_RETRY;
                        Some(now + RUN_INTERVAL)
                    }
                    RunOutcome::Done { more_due: true } | RunOutcome::StoreFailed => {
                        cooldown_until = now + MORE_DUE_DELAY;
                        Some(cooldown_until)
                    }
                    RunOutcome::Offline => {
                        let at = now + offline_retry;
                        cooldown_until = at;
                        offline_retry = (offline_retry * 2).min(RUN_INTERVAL);
                        Some(at)
                    }
                };
            }
        }
    }
}

type FetchResult = (String, Result<Vec<u8>, FetchError>);

/// "Stop now" check of a run (locked, switched, setting off, quitting).
type Stop = dyn Fn() -> bool + Send + Sync;

/// One run: fetches the due icons of the open vault and stores them (in
/// batches, each one save) into that vault only.
fn run_once(core: &Arc<Core>, fetcher: &Fetcher) -> RunOutcome {
    let generation = core.icons().generation();
    let (vault_id, mut hosts) = {
        let st = core.state();
        if !st.settings.website_icons {
            return RunOutcome::Disabled;
        }
        let Some(vault) = st.vault.as_ref() else {
            return RunOutcome::Locked;
        };
        (
            vault.id().to_owned(),
            vault.icon_hosts_needing_fetch(now_ms()),
        )
    };
    if hosts.is_empty() {
        return RunOutcome::Idle;
    }
    // Never around a proxy the user set up (see `proxy_matcher`).
    if let Some(proxies) = fetcher.proxy_settings() {
        // A proxy setup the matcher cannot interpret (PAC script,
        // per-protocol Windows proxy, …): fail closed, fetch nothing.
        if proxy_setup_unclear(&proxies) {
            log(format_args!(
                "website icons: skipped (a proxy configuration this app cannot evaluate is set)"
            ));
            return RunOutcome::Proxied;
        }
        let due = hosts.len();
        hosts.retain(|host| !proxied_by(&proxies, host));
        if hosts.len() < due {
            log(format_args!(
                "website icons: {} skipped (a proxy is configured)",
                due - hosts.len()
            ));
        }
        if hosts.is_empty() {
            return RunOutcome::Proxied;
        }
    }
    let more_due = hosts.len() > MAX_HOSTS_PER_RUN;
    hosts.truncate(MAX_HOSTS_PER_RUN);

    let weak = Arc::downgrade(core);
    let cancelled: Arc<Stop> = Arc::new(move || {
        weak.upgrade()
            .is_none_or(|c| c.is_exiting() || c.icons().generation() != generation)
    });
    let (tx, rx) = mpsc::channel::<FetchResult>();
    tauri::async_runtime::spawn(fetch_all(
        fetcher.clone(),
        hosts,
        CONCURRENCY,
        Arc::clone(&cancelled),
        tx,
    ));

    let mut pending: Vec<FetchResult> = Vec::new();
    let (mut loaded, mut failed, mut answered) = (0usize, 0usize, false);
    let mut last_store = Instant::now();
    let mut finished = false;
    while !finished {
        match rx.recv_timeout(PERSIST_EVERY) {
            Ok(result) => {
                match &result.1 {
                    Ok(_) => {
                        loaded += 1;
                        answered = true;
                    }
                    Err(e) => {
                        failed += 1;
                        answered |= !e.is_network();
                    }
                }
                pending.push(result);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => finished = true,
        }
        if cancelled() {
            return RunOutcome::Cancelled;
        }
        // Until some site answered, the computer may be offline: hold the
        // failures back instead of marking every site as failed.
        let due = finished
            || pending.len() >= PERSIST_BATCH
            || (!pending.is_empty() && last_store.elapsed() >= PERSIST_EVERY);
        if answered && due && !pending.is_empty() {
            if let Err(outcome) = store(core, &vault_id, generation, &mut pending) {
                return outcome;
            }
            last_store = Instant::now();
        }
    }
    log(format_args!(
        "website icons: {loaded} loaded, {failed} not available"
    ));
    if !answered {
        return RunOutcome::Offline;
    }
    RunOutcome::Done { more_due }
}

/// Writes fetch results into the vault `vault_id` (one save) and tells the
/// UI. Nothing is written if that vault is no longer the open one or the
/// run was cancelled (checked under the state lock).
fn store(
    core: &Arc<Core>,
    vault_id: &str,
    generation: u64,
    pending: &mut Vec<FetchResult>,
) -> Result<(), RunOutcome> {
    let results: Vec<(String, Option<Vec<u8>>)> = pending
        .drain(..)
        .map(|(host, result)| (host, result.ok()))
        .collect();
    let stored = core.mutate_for_page(Some(vault_id), |vault| {
        if core.icons().generation() != generation {
            return Ok(None);
        }
        vault.set_icons(&results, now_ms()).map(Some)
    });
    match stored {
        Ok(Some(_)) => {
            core.emit(EVENT_ICONS, serde_json::json!({}));
            Ok(())
        }
        Ok(None) => Err(RunOutcome::Cancelled),
        Err(AppError::Locked) => Err(RunOutcome::Locked),
        Err(e) => {
            log(format_args!("could not store website icons: {}", e.code()));
            Err(RunOutcome::StoreFailed)
        }
    }
}

/// Fetches `hosts` with `workers` parallel tasks and sends every result.
/// Once `cancelled`, no new request starts and the open ones are dropped
/// (within [`CANCEL_POLL`]).
async fn fetch_all(
    fetcher: Fetcher,
    hosts: Vec<String>,
    workers: usize,
    cancelled: Arc<Stop>,
    results: Sender<FetchResult>,
) {
    let queue = Arc::new(Mutex::new(VecDeque::from(hosts)));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..workers.max(1) {
        let (fetcher, queue, cancelled, results) = (
            fetcher.clone(),
            Arc::clone(&queue),
            Arc::clone(&cancelled),
            results.clone(),
        );
        tasks.spawn(async move {
            loop {
                if cancelled() {
                    return;
                }
                let Some(host) = lock(&queue).pop_front() else {
                    return;
                };
                let result = fetcher.fetch(&host, &*cancelled).await;
                if results.send((host, result)).is_err() {
                    return;
                }
            }
        });
    }
    drop(results);
    loop {
        match tokio::time::timeout(CANCEL_POLL, tasks.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(_) if cancelled() => {
                // Drops the open requests (and their connections) now.
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return;
            }
            Err(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Commands and the browser bridge
// ---------------------------------------------------------------------------

/// `get_icons(pageVaultId)` → `{ [host]: "data:image/png;base64,…" }` of
/// the page's vault (`locked` if another vault is open).
#[tauri::command]
pub async fn get_icons(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<BTreeMap<String, String>> {
    run(&core, false, move |c| {
        let st = c.state();
        let vault = st.page_vault(page_vault_id.as_deref())?;
        Ok(icon_map(vault))
    })
    .await
}

/// `clear_icons(pageVaultId)` † → number of removed icons.
#[tauri::command]
pub async fn clear_icons(core: Shared<'_>, page_vault_id: Option<String>) -> CmdResult<usize> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.clear_icons())
    })
    .await
}

fn icon_map(vault: &UnlockedVault) -> BTreeMap<String, String> {
    vault
        .data()
        .icons
        .iter()
        .filter_map(|(host, entry)| {
            Some((host.clone(), core_icons::data_url(entry.png.as_deref()?)))
        })
        .collect()
}

/// Adds the stored icon (data URL) to the first [`BRIDGE_ICON_ROWS`] rows
/// of a bridge reply, if it is at most [`BRIDGE_ICON_MAX_LEN`] long.
pub fn attach_icons(vault: &UnlockedVault, rows: &mut [ItemSummary]) {
    for row in rows.iter_mut().take(BRIDGE_ICON_ROWS) {
        row.icon = vault
            .item(&row.id)
            .and_then(|item| vault.icon_for_item(item))
            .map(core_icons::data_url)
            .filter(|url| url.len() <= BRIDGE_ICON_MAX_LEN);
    }
}

// ---------------------------------------------------------------------------
// Fetcher
// ---------------------------------------------------------------------------

/// Why a site has no icon.
#[derive(Debug)]
pub(crate) enum FetchError {
    /// No connection (DNS, connect, timeout before an answer): maybe
    /// offline.
    Network(String),
    /// The site answered, but without a usable icon.
    Failed(String),
    /// Address not allowed (scheme, host, port, non-public address).
    Refused(String),
}

impl FetchError {
    fn is_network(&self) -> bool {
        matches!(self, FetchError::Network(_))
    }

    fn failed(detail: impl Into<String>) -> Self {
        FetchError::Failed(detail.into())
    }

    fn from_reqwest(e: &reqwest::Error) -> Self {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
        while let Some(s) = source {
            if s.downcast_ref::<NonPublicAddress>().is_some() {
                return FetchError::Refused("non-public address".into());
            }
            source = s.source();
        }
        if e.is_redirect() {
            FetchError::Refused(e.to_string())
        } else if e.is_connect() || e.is_timeout() {
            FetchError::Network(e.to_string())
        } else {
            FetchError::Failed(e.to_string())
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Network(d) => write!(f, "network: {d}"),
            FetchError::Failed(d) => write!(f, "failed: {d}"),
            FetchError::Refused(d) => write!(f, "refused: {d}"),
        }
    }
}

/// Which addresses a fetcher may contact.
#[derive(Debug)]
enum Allow {
    /// Production: https on the default port, hosts that pass
    /// `is_fetchable_host`, public IP addresses only ([`PublicResolver`]).
    Public,
    /// Tests: plain http to one local server; `https://<host>/` becomes
    /// `<origin><host>/`.
    #[cfg(test)]
    Local { origin: Url },
}

#[derive(Debug)]
struct Policy {
    allow: Allow,
}

impl Policy {
    /// May a request (or a redirect) go to `url`?
    fn url_allowed(&self, url: &Url) -> bool {
        if !url.username().is_empty() || url.password().is_some() {
            return false;
        }
        match &self.allow {
            Allow::Public => {
                url.scheme() == "https"
                    && url.port().is_none()
                    && url.domain().is_some_and(core_icons::is_fetchable_host)
            }
            #[cfg(test)]
            Allow::Local { origin } => {
                url.scheme() == "http"
                    && url.host_str() == origin.host_str()
                    && url.port() == origin.port()
            }
        }
    }

    fn site_url(&self, host: &str, path: &str) -> Option<Url> {
        match &self.allow {
            Allow::Public => Url::parse(&format!("https://{host}/{path}")).ok(),
            #[cfg(test)]
            Allow::Local { origin } => origin.join(&format!("{host}/{path}")).ok(),
        }
    }
}

/// Error of [`PublicResolver`]: the name only has non-public addresses.
#[derive(Debug)]
struct NonPublicAddress;

impl std::fmt::Display for NonPublicAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the host has no public address")
    }
}

impl std::error::Error for NonPublicAddress {}

/// DNS for the icon fetcher: the system resolver, minus every address that
/// is not public (loopback, private, link-local, unique-local, CGNAT, …).
/// Connections only go to what this returns, so a DNS name pointing into
/// the local network (also via redirects or rebinding) is never reached.
struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = addrs.filter(|a| is_public_ip(a.ip())).collect();
            if public.is_empty() {
                return Err(Box::new(NonPublicAddress) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// True for addresses on the public internet.
pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            let embedded_v4 =
                |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
            if s[..6] == [0; 6] {
                // ::, ::1 and the deprecated IPv4-compatible ::a.b.c.d.
                return false;
            }
            if s[0] & 0xfe00 == 0xfc00 // fc00::/7 unique local
                || s[0] & 0xffc0 == 0xfe80 // fe80::/10 link-local
                || s[0] & 0xffc0 == 0xfec0 // fec0::/10 site-local
                || s[0] & 0xff00 == 0xff00 // multicast
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x0100 && s[1..4] == [0, 0, 0]) // discard-only
                || (s[0] == 0x2001 && s[1] == 0) // Teredo
                || (s[0] == 0x2001 && matches!(s[1] & 0xfff0, 0x0010 | 0x0020))
            // ORCHID
            {
                return false;
            }
            if s[0] == 0x0064 && s[1] == 0xff9b {
                // NAT64: the well-known prefix carries an IPv4 address,
                // 64:ff9b:1::/48 is for local use.
                return s[2..6] == [0; 4] && is_public_v4(embedded_v4(s[6], s[7]));
            }
            if s[0] == 0x2002 {
                // 6to4
                return is_public_v4(embedded_v4(s[1], s[2]));
            }
            true
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(o[0] == 0 // "this network", 0.0.0.0
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (o[0] == 100 && o[1] & 0xc0 == 64) // 100.64/10 carrier-grade NAT
        || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2)) // IETF, TEST-NET-1
        || (o[0] == 192 && o[1] == 88 && o[2] == 99) // 6to4 relay
        || (o[0] == 198 && o[1] & 0xfe == 18) // benchmarking
        || (o[0] == 198 && o[1] == 51 && o[2] == 100) // TEST-NET-2
        || (o[0] == 203 && o[1] == 0 && o[2] == 113) // TEST-NET-3
        || o[0] >= 224) // multicast, reserved, broadcast
}

/// The proxy settings of the system and the environment (`HTTPS_PROXY`,
/// `ALL_PROXY`, `NO_PROXY`; Windows Internet settings; macOS network
/// settings), read anew for every run. The fetcher connects directly (only
/// then does [`PublicResolver`] see every host), so a site the user's proxy
/// would handle is not fetched at all: going around a proxy would reveal
/// the user's own address to the websites of their accounts, and through
/// it the proxy would resolve names this app cannot check.
fn proxy_matcher() -> ProxyMatcher {
    ProxyMatcher::from_system()
}

/// Proxy settings that exist but that [`proxy_matcher`] does not turn into a
/// proxy for https sites – e.g. a PAC script, or the per-protocol form of
/// the Windows proxy. The fetcher then fetches nothing rather than possibly
/// going around the user's proxy.
fn proxy_setup_unclear(matcher: &ProxyMatcher) -> bool {
    let env_https = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|v| !v.trim().is_empty()));
    proxy_unclear_from(
        env_https,
        &system_proxy_hints(),
        proxied_by(matcher, "example.com"),
    )
}

/// Proxy-related system settings beyond what [`proxy_matcher`] understands.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SystemProxyHints {
    /// A PAC / auto-config script is configured.
    auto_config: bool,
    /// A manual proxy is switched on.
    manual_enabled: bool,
}

#[cfg(windows)]
fn system_proxy_hints() -> SystemProxyHints {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
    else {
        return SystemProxyHints::default();
    };
    let auto_config = key
        .get_value::<String, _>("AutoConfigURL")
        .is_ok_and(|url| !url.trim().is_empty());
    let manual_enabled = key.get_value::<u32, _>("ProxyEnable").is_ok_and(|v| v != 0)
        && key
            .get_value::<String, _>("ProxyServer")
            .is_ok_and(|server| !server.trim().is_empty());
    SystemProxyHints {
        auto_config,
        manual_enabled,
    }
}

#[cfg(not(windows))]
fn system_proxy_hints() -> SystemProxyHints {
    SystemProxyHints::default()
}

/// The decision behind [`proxy_setup_unclear`]: some proxy is configured,
/// but the matcher would still connect directly to a public https site.
fn proxy_unclear_from(env_https: bool, hints: &SystemProxyHints, matcher_proxies: bool) -> bool {
    if hints.auto_config {
        return true;
    }
    (env_https || hints.manual_enabled) && !matcher_proxies
}

/// Would `matcher` send a request for `https://<host>/` through a proxy?
fn proxied_by(matcher: &ProxyMatcher, host: &str) -> bool {
    format!("https://{host}/")
        .parse::<http::Uri>()
        .is_ok_and(|uri| matcher.intercept(&uri).is_some())
}

/// Loads website icons (see the module docs). Cheap to clone.
#[derive(Clone)]
pub(crate) struct Fetcher {
    client: reqwest::Client,
    policy: Arc<Policy>,
    /// Reads the proxy settings before a run ([`proxy_matcher`]); `None` for
    /// the local test server and when the client itself uses the proxy.
    proxies: Option<fn() -> ProxyMatcher>,
}

impl Fetcher {
    /// The production fetcher: no proxy, [`PublicResolver`], https only.
    pub fn new(app_version: &str) -> Result<Self, String> {
        Self::build(Allow::Public, app_version, REQUEST_TIMEOUT, false)
    }

    /// The current proxy settings, if this fetcher has to respect them.
    fn proxy_settings(&self) -> Option<ProxyMatcher> {
        self.proxies.map(|read| read())
    }

    fn build(
        allow: Allow,
        app_version: &str,
        timeout: Duration,
        use_env_proxy: bool,
    ) -> Result<Self, String> {
        // `rustls-no-provider` (shared with the updater): install ring as
        // the process default before the first client is built.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let proxies: Option<fn() -> ProxyMatcher> =
            (matches!(allow, Allow::Public) && !use_env_proxy).then_some(proxy_matcher);
        let policy = Arc::new(Policy { allow });
        let redirects = Arc::clone(&policy);
        let mut builder = reqwest::Client::builder()
            .user_agent(format!("Keystead/{app_version} (icon fetcher)"))
            .timeout(timeout)
            .connect_timeout(timeout)
            .referer(false)
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() > MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if !redirects.url_allowed(attempt.url()) {
                    attempt.error("redirect to an address that is not allowed")
                } else {
                    attempt.follow()
                }
            }))
            .dns_resolver(PublicResolver);
        if !use_env_proxy {
            // Directly to the website (the user's choice), and only then
            // does `PublicResolver` see every host the app connects to.
            builder = builder.no_proxy();
        }
        let client = builder.build().map_err(|e| e.to_string())?;
        Ok(Fetcher {
            client,
            policy,
            proxies,
        })
    }

    /// The 64×64 PNG icon of `host`, or why there is none. Never takes
    /// longer than [`HOST_DEADLINE`]; once `stop` says so, no further
    /// request is sent.
    pub async fn fetch(&self, host: &str, stop: &Stop) -> Result<Vec<u8>, FetchError> {
        match tokio::time::timeout(HOST_DEADLINE, self.fetch_inner(host, stop)).await {
            Ok(result) => result,
            Err(_) => Err(FetchError::Network("deadline".into())),
        }
    }

    async fn fetch_inner(&self, host: &str, stop: &Stop) -> Result<Vec<u8>, FetchError> {
        if !core_icons::is_fetchable_host(host) {
            return Err(FetchError::Refused("host".into()));
        }
        let page = self
            .policy
            .site_url(host, "")
            .ok_or_else(|| FetchError::Refused("host".into()))?;
        let mut errors: Vec<FetchError> = Vec::new();
        let mut tried: HashSet<Url> = HashSet::new();
        match self.get(&page, ACCEPT_HTML, true, stop).await {
            Ok((final_url, body)) => {
                let html = String::from_utf8_lossy(&body);
                for candidate in icon_candidates(&html, &final_url)
                    .into_iter()
                    .take(MAX_CANDIDATES)
                {
                    let bytes = match candidate.source {
                        Source::Png(bytes) => Ok(bytes),
                        Source::Url(url) => {
                            if !tried.insert(url.clone()) {
                                continue;
                            }
                            self.get(&url, ACCEPT_IMAGE, false, stop)
                                .await
                                .map(|(_, b)| b)
                        }
                    };
                    let icon = match bytes {
                        Ok(bytes) => decode_icon(bytes).await,
                        Err(e) => Err(e),
                    };
                    match icon {
                        Ok(png) => return Ok(png),
                        Err(e) => errors.push(e),
                    }
                }
            }
            Err(e) => errors.push(e),
        }
        if let Some(favicon) = self.policy.site_url(host, "favicon.ico") {
            if tried.insert(favicon.clone()) {
                match self.get(&favicon, ACCEPT_IMAGE, false, stop).await {
                    Ok((_, bytes)) => match decode_icon(bytes).await {
                        Ok(png) => return Ok(png),
                        Err(e) => errors.push(e),
                    },
                    Err(e) => errors.push(e),
                }
            }
        }
        // Report "the site answered" over "no connection" (offline check).
        let pos = errors.iter().position(|e| !e.is_network()).unwrap_or(0);
        Err(if errors.is_empty() {
            FetchError::failed("no icon")
        } else {
            errors.swap_remove(pos)
        })
    }

    /// GET `url` (allowed by the policy; redirects are checked too) and
    /// read at most [`MAX_BYTES`]: `truncate` cuts a longer body, otherwise
    /// it is refused. Returns the final URL and the body. Sends nothing once
    /// `stop` says so.
    async fn get(
        &self,
        url: &Url,
        accept: &str,
        truncate: bool,
        stop: &Stop,
    ) -> Result<(Url, Vec<u8>), FetchError> {
        if stop() {
            return Err(FetchError::Network("cancelled".into()));
        }
        if !self.policy.url_allowed(url) {
            return Err(FetchError::Refused("url".into()));
        }
        let mut response = self
            .client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, accept)
            .send()
            .await
            .map_err(|e| FetchError::from_reqwest(&e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::failed(format!("HTTP {status}")));
        }
        let final_url = response.url().clone();
        if !truncate
            && response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(FetchError::failed("too large"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::from_reqwest(&e))?
        {
            let room = MAX_BYTES - body.len();
            if chunk.len() > room {
                if truncate {
                    body.extend_from_slice(&chunk[..room]);
                    break;
                }
                return Err(FetchError::failed("too large"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((final_url, body))
    }
}

// ---------------------------------------------------------------------------
// HTML: icon links
// ---------------------------------------------------------------------------

/// Where an icon comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    Url(Url),
    /// A `data:image/png;base64,…` link.
    Png(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Png,
    Ico,
    Other,
}

/// An icon link of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub source: Source,
    kind: Kind,
    /// Largest side from `sizes` (an apple-touch-icon without `sizes` is
    /// 180 px by convention).
    size: Option<u32>,
}

impl Candidate {
    /// Sort key: sizes ≥ 64 (closest first), then 32–63 (largest first),
    /// then unknown, then < 32; PNG before ICO before others.
    fn rank(&self) -> (u8, Kind, u32) {
        let (class, distance) = match self.size {
            Some(s) if s >= 64 => (0, s - 64),
            Some(s) if s >= 32 => (1, 64 - s),
            None => (2, 0),
            Some(s) => (3, 64 - s),
        };
        (class, self.kind, distance)
    }
}

/// A start tag of interest (`link`, `base`) with its attributes.
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The icon links of a page's head, best first (see [`Candidate::rank`]).
/// Relative links are resolved against `<base href>` or `page`. SVG icons
/// and `data:` links other than `data:image/png;base64` are left out.
pub(crate) fn icon_candidates(html: &str, page: &Url) -> Vec<Candidate> {
    let tags = head_tags(html);
    let base = tags
        .iter()
        .find(|t| t.name == "base")
        .and_then(|t| t.attr("href"))
        .and_then(|href| page.join(href.trim()).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or_else(|| page.clone());
    let mut out: Vec<Candidate> = Vec::new();
    for tag in tags.iter().filter(|t| t.name == "link") {
        let rel = tag.attr("rel").unwrap_or("").to_ascii_lowercase();
        let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
        let apple = tokens
            .iter()
            .any(|t| matches!(*t, "apple-touch-icon" | "apple-touch-icon-precomposed"));
        if !apple && !tokens.contains(&"icon") {
            continue;
        }
        let href = tag.attr("href").unwrap_or("").trim();
        if href.is_empty() {
            continue;
        }
        let mime = tag.attr("type").unwrap_or("").trim().to_ascii_lowercase();
        if mime.contains("svg") {
            continue;
        }
        let size = parse_sizes(tag.attr("sizes").unwrap_or("")).or(apple.then_some(180));
        let (source, kind) = if href
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("data:"))
        {
            match decode_png_data_url(href) {
                Some(bytes) => (Source::Png(bytes), Kind::Png),
                None => continue,
            }
        } else {
            let Ok(url) = base.join(href) else { continue };
            if !matches!(url.scheme(), "http" | "https") {
                continue;
            }
            let path = url.path().to_ascii_lowercase();
            if path.ends_with(".svg") || path.ends_with(".svgz") {
                continue;
            }
            let kind = match mime.as_str() {
                "image/png" => Kind::Png,
                "image/x-icon" | "image/vnd.microsoft.icon" | "image/ico" | "image/icon" => {
                    Kind::Ico
                }
                "" if path.ends_with(".png") => Kind::Png,
                "" if path.ends_with(".ico") => Kind::Ico,
                _ => Kind::Other,
            };
            (Source::Url(url), kind)
        };
        if out.iter().any(|c| c.source == source) {
            continue;
        }
        out.push(Candidate { source, kind, size });
    }
    out.sort_by_key(Candidate::rank);
    out
}

/// Largest side named in a `sizes` attribute (`"16x16 32x32"` → 32;
/// `"any"` → none).
fn parse_sizes(sizes: &str) -> Option<u32> {
    sizes
        .split_ascii_whitespace()
        .filter_map(|s| {
            let (w, h) = s
                .to_ascii_lowercase()
                .split_once('x')
                .map(|(w, h)| (w.to_owned(), h.to_owned()))?;
            Some(w.parse::<u32>().ok()?.max(h.parse::<u32>().ok()?))
        })
        .max()
}

/// The bytes of a `data:image/png;base64,…` URL (≤ [`MAX_BYTES`]).
fn decode_png_data_url(href: &str) -> Option<Vec<u8>> {
    let (meta, data) = href.split_once(',')?;
    if !meta.trim().eq_ignore_ascii_case("data:image/png;base64")
        || data.len() > MAX_BYTES * 4 / 3 + 4
    {
        return None;
    }
    let data: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    keystead_core::crypto::b64_decode(&data).ok()
}

/// `link` and `base` start tags before `<body>` / `</head>`, skipping
/// comments and the content of `script`, `style` and similar elements.
fn head_tags(html: &str) -> Vec<Tag> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = b[i..].iter().position(|&c| c == b'<') {
        let start = i + off + 1;
        i = start;
        let rest = &b[start..];
        if rest.starts_with(b"!--") {
            match find(&rest[3..], b"-->") {
                Some(end) => {
                    i = start + 3 + end + 3;
                    continue;
                }
                None => break,
            }
        }
        if matches!(rest.first(), Some(b'!' | b'?')) {
            // Doctype, CDATA, processing instruction.
            match rest.iter().position(|&c| c == b'>') {
                Some(end) => {
                    i = start + end + 1;
                    continue;
                }
                None => break,
            }
        }
        let closing = rest.first() == Some(&b'/');
        let name_start = start + usize::from(closing);
        let name_len = b[name_start..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-' || **c == b':')
            .count();
        if name_len == 0 || !b[name_start].is_ascii_alphabetic() {
            // A stray `<` in text.
            continue;
        }
        let name = html[name_start..name_start + name_len].to_ascii_lowercase();
        let (attrs, end) = parse_attrs(html, name_start + name_len);
        i = end;
        if closing {
            if name == "head" {
                break;
            }
            continue;
        }
        match name.as_str() {
            "body" => break,
            "script" | "style" | "template" | "textarea" | "title" | "noscript" | "xmp" => {
                // Raw text up to the matching end tag.
                match find_ascii_ci(&b[i..], format!("</{name}").as_bytes()) {
                    Some(p) => i += p,
                    None => break,
                }
            }
            "link" | "base" => out.push(Tag { name, attrs }),
            _ => {}
        }
    }
    out
}

/// Attributes of a start tag beginning at `pos` (after the name); returns
/// them and the index after the closing `>` (or the end of the input).
/// Values are entity-decoded; the first of duplicate names wins.
fn parse_attrs(html: &str, mut i: usize) -> (Vec<(String, String)>, usize) {
    let b = html.as_bytes();
    let len = b.len();
    let mut attrs: Vec<(String, String)> = Vec::new();
    loop {
        while i < len && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= len {
            return (attrs, len);
        }
        if b[i] == b'>' {
            return (attrs, i + 1);
        }
        let name_start = i;
        while i < len && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        if i == name_start {
            // `=` without a name.
            i += 1;
            continue;
        }
        let name = html[name_start..i].to_ascii_lowercase();
        while i < len && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < len && b[i] == b'=' {
            i += 1;
            while i < len && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && matches!(b[i], b'"' | b'\'') {
                let quote = b[i];
                i += 1;
                let value_start = i;
                while i < len && b[i] != quote {
                    i += 1;
                }
                value = decode_entities(&html[value_start..i]);
                i = (i + 1).min(len);
            } else {
                let value_start = i;
                while i < len && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                value = decode_entities(&html[value_start..i]);
            }
        }
        if attrs.len() < 32 && !attrs.iter().any(|(n, _)| *n == name) {
            attrs.push((name, value));
        }
    }
}

/// Decodes `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`, `&#NN;` and
/// `&#xHH;`; anything else stays as written.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.as_bytes()[1..]
            .iter()
            .take(11)
            .position(|&c| c == b';')
            .and_then(|semi| {
                let entity = &rest[1..1 + semi];
                let c = match entity {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    _ => {
                        let num = entity.strip_prefix('#')?;
                        let code = match num.strip_prefix(['x', 'X']) {
                            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                            None => num.parse::<u32>().ok()?,
                        };
                        char::from_u32(code)?
                    }
                };
                Some((c, semi + 2))
            });
        match decoded {
            Some((c, consumed)) => {
                out.push(c);
                rest = &rest[consumed..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_ascii_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

/// [`process_icon`] on the blocking thread pool: decoding and scaling are
/// CPU work that must not hold up the async runtime (which also serves the
/// app's commands); [`HOST_DEADLINE`] still ends the wait for it.
async fn decode_icon(bytes: Vec<u8>) -> Result<Vec<u8>, FetchError> {
    tauri::async_runtime::spawn_blocking(move || process_icon(&bytes))
        .await
        .unwrap_or_else(|_| Err(FetchError::failed("decoder stopped")))
}

/// Decodes an icon (PNG, ICO, JPEG, GIF, WebP – recognised by content, not
/// by the server's content type; at most [`MAX_DIMENSION`] px per side),
/// fits it into 64×64 keeping the aspect ratio on a transparent square, and
/// encodes it as PNG. CPU-bound: call it through [`decode_icon`].
pub(crate) fn process_icon(bytes: &[u8]) -> Result<Vec<u8>, FetchError> {
    let format = image::guess_format(bytes).map_err(|_| FetchError::failed("not an image"))?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Ico
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
    ) {
        return Err(FetchError::failed("unsupported image format"));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| FetchError::failed("undecodable image"))?;
    let (w, h) = (decoded.width(), decoded.height());
    if w < MIN_DIMENSION || h < MIN_DIMENSION {
        return Err(FetchError::failed("image too small"));
    }
    let rgba = decoded.into_rgba8();
    if !rgba.pixels().any(|p| p[3] > 16) {
        return Err(FetchError::failed("blank image"));
    }
    let longest = w.max(h);
    let scale = |side: u32| {
        ((f64::from(side) * f64::from(ICON_SIZE) / f64::from(longest)).round() as u32)
            .clamp(1, ICON_SIZE)
    };
    let (nw, nh) = (scale(w), scale(h));
    let resized = if (nw, nh) == (w, h) {
        rgba
    } else {
        let filter = if longest > ICON_SIZE {
            FilterType::Lanczos3
        } else {
            FilterType::CatmullRom
        };
        imageops::resize(&rgba, nw, nh, filter)
    };
    let mut canvas = RgbaImage::new(ICON_SIZE, ICON_SIZE);
    imageops::overlay(
        &mut canvas,
        &resized,
        i64::from((ICON_SIZE - nw) / 2),
        i64::from((ICON_SIZE - nh) / 2),
    );
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Best, PngFilter::Adaptive)
        .write_image(
            canvas.as_raw(),
            ICON_SIZE,
            ICON_SIZE,
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| FetchError::failed("encoding failed"))?;
    Ok(png)
}

#[cfg(test)]
mod tests;
