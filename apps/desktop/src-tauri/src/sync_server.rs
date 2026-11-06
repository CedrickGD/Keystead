//! Server sync (stage 2c) – what the phone's link (`mobile::sync`) and the
//! PC's mirror (`mirror`) share: the server as stored ([`SyncServer`], no
//! password) ↔ the WebDAV client's [`WebDavConfig`], connecting
//! ([`connect`]: the connection test must pass, then the password is
//! stored), opening the server for a sync ([`open`]), the pause after
//! server errors ([`Backoff`]) and the commands `webdav_test` and
//! `webdav_create_folder` (both platforms). See "Server sync (WebDAV)" in
//! docs/ARCHITECTURE.md. Since stage 2d also a cloud account (Dropbox or
//! Google Drive, [`SyncCloud`]): the login (Dropbox: [`dropbox_start_login`]
//! – the URL the page opens – and [`dropbox_finish_login`] with the code the
//! user copied; Google: [`gdrive_start_login`] – the PC's loopback
//! redirect, the phone's device code –, [`gdrive_check_login`] while the
//! dialog waits, then [`gdrive_finish_login`]; the refresh token stored like
//! a password; a waiting login is offered again), opening it
//! ([`open_cloud`]), signing out ([`sign_out`]) and the commands
//! `dropbox_login`, `dropbox_login_waiting`, `gdrive_login` and
//! `gdrive_login_ready`. See "Cloud accounts (Dropbox, Google Drive)".
//!
//! The WebDAV client drives its requests on a runtime of its own
//! (`Runtime::block_on`): it is only ever used on plain threads – the
//! blocking pool of the commands (`commands::run`), the mirror's worker,
//! the phone's sync and push threads – never inside an async task (a unit
//! test below runs it on Tauri's blocking pool).

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use keystead_core::settings::{CloudProvider, SyncCloud, SyncServer};
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::commands::{parse, run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::remote::dropbox::{self, DropboxRemote, Endpoints, Pkce};
use crate::remote::gdrive::{self, GdriveRemote};
use crate::remote::webdav::{self, WebDavConfig, WebDavRemote, WebDavTest};
use crate::remote::{self, Remote};
use crate::state::{log, Core};

/// `invalid_input:webdav_config`: the command's argument is not a server
/// configuration.
pub const BAD_CONFIG: &str = "webdav_config";
/// `skipped`: an automatic sync waits after a server error ([`Backoff`]).
#[cfg(mobile)]
pub const SKIPPED_BACKOFF: &str = "sync_backoff";

/// The pauses of automatic syncs after consecutive server errors: 1, 2, 5,
/// then 15 minutes.
pub const BACKOFF_STEPS: [Duration; 4] = [
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
];

/// The client's configuration of a stored server with its password.
pub fn config_of(server: &SyncServer, password: Zeroizing<String>) -> WebDavConfig {
    WebDavConfig {
        url: server.url.clone(),
        user: server.user.clone(),
        password,
        folder: server.folder.clone(),
        cert_sha256: server.cert_sha256.clone(),
    }
}

/// What is stored of `cfg` (no password); `cas` as the test found it.
pub fn server_of(cfg: &WebDavConfig, cas: bool) -> SyncServer {
    SyncServer {
        url: cfg.url.trim().to_owned(),
        user: cfg.user.clone(),
        folder: cfg.folder.clone(),
        cert_sha256: cfg.cert_sha256.clone(),
        cas,
    }
}

/// The key of the server's folder (`webdav:<normalized folder URL>`) – what
/// its password, the sync bases and the link are recorded for.
pub fn key_of(server: &SyncServer) -> AppResult<String> {
    Ok(webdav::key_of(&webdav::collection_url(
        &server.url,
        &server.folder,
    )?))
}

/// The server for a sync, with its stored password
/// (`unsupported:sync_credentials_missing` without one) and the `If-Match`
/// answer of its set-up (no probe write per sync).
pub fn open(core: &Core, server: &SyncServer) -> AppResult<WebDavRemote> {
    let key = key_of(server)?;
    let password = core.credentials().require(&core.data_dir(), &key)?;
    Ok(WebDavRemote::new(&config_of(server, password))?.with_cas(server.cas))
}

/// A server [`connect`] reached: what is stored of it, its folder's key
/// and what the connection test found.
#[derive(Debug)]
// The PC's `set_sync_server` needs the server only.
#[cfg_attr(not(any(mobile, test)), allow(dead_code))]
pub struct Connected {
    pub server: SyncServer,
    pub key: String,
    pub test: WebDavTest,
}

/// Connects to the server of `cfg`: the connection test must pass (its
/// error code otherwise – the folder missing, an untrusted certificate,
/// …), then the password is stored under the folder's key.
pub fn connect(core: &Core, cfg: &WebDavConfig) -> AppResult<Connected> {
    let test = WebDavRemote::test(cfg);
    if !test.ok {
        return Err(AppError::Code(test.code.clone().unwrap_or_else(|| {
            AppError::unsupported(remote::SERVER_ERROR).code()
        })));
    }
    let server = server_of(cfg, test.cas);
    let key = WebDavRemote::new(cfg)?.key();
    core.credentials()
        .store(&core.data_dir(), &key, &cfg.password)?;
    Ok(Connected { server, key, test })
}

/// The server password of a link or setting that is replaced or removed
/// is forgotten – unless the new one uses the same server folder (`keep`,
/// its key). Logged, never an error (a stale file only costs space).
pub fn forget_password(core: &Core, old_key: &str, keep: Option<&str>) {
    if keep == Some(old_key) {
        return;
    }
    if let Err(e) = core.credentials().remove(&core.data_dir(), old_key) {
        log(format_args!(
            "could not forget the server password: {}",
            e.code()
        ));
    }
}

/// Whether an error (its code) pauses the automatic syncs: the server
/// cannot be reached, failed, answered nonsense or refused the login (a
/// server that locks accounts after failed logins must not be asked every
/// minute), its certificate changed, the folder is gone. Errors of single
/// files do not.
pub fn backs_off(code: &str) -> bool {
    let Some(detail) = code.strip_prefix("unsupported:") else {
        return false;
    };
    [
        remote::OFFLINE,
        remote::smb::LOCAL_NETWORK,
        remote::SERVER_ERROR,
        remote::BAD_RESPONSE,
        remote::QUOTA,
        remote::AUTH_FAILED,
        remote::CERTIFICATE,
        remote::NOT_WEBDAV,
        remote::FOLDER_MISSING,
    ]
    .contains(&detail)
}

/// The pause of automatic syncs after server errors ([`BACKOFF_STEPS`];
/// a sync the user starts always runs). Both clocks: the monotonic one
/// stands still while a phone sleeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: usize,
    since: Option<(Instant, SystemTime)>,
}

impl Backoff {
    /// The current pause, `None` without one.
    pub fn delay(&self) -> Option<Duration> {
        let step = self.failures.checked_sub(1)?;
        Some(BACKOFF_STEPS[step.min(BACKOFF_STEPS.len() - 1)])
    }

    /// Whether an automatic sync may run at (`now`, `wall`).
    pub fn allows(&self, now: Instant, wall: SystemTime) -> bool {
        let (Some(delay), Some((at, at_wall))) = (self.delay(), self.since) else {
            return true;
        };
        let elapsed = now
            .saturating_duration_since(at)
            .max(wall.duration_since(at_wall).unwrap_or(Duration::ZERO));
        elapsed >= delay
    }

    /// [`Backoff::allows`] now.
    pub fn allows_now(&self) -> bool {
        self.allows(Instant::now(), SystemTime::now())
    }

    /// Records a sync's outcome: `None` (it reached the server) or an
    /// error code that does not pause ends the pause; one that does
    /// ([`backs_off`]) starts or lengthens it.
    pub fn record(&mut self, error: Option<&str>, now: Instant, wall: SystemTime) {
        match error {
            Some(code) if backs_off(code) => {
                self.failures += 1;
                self.since = Some((now, wall));
            }
            _ => self.reset(),
        }
    }

    /// [`Backoff::record`] now.
    pub fn record_now(&mut self, error: Option<&str>) {
        self.record(error, Instant::now(), SystemTime::now());
    }

    pub fn reset(&mut self) {
        *self = Backoff::default();
    }

    /// When the pause ends (ms since the epoch), `None` without one.
    pub fn retry_at_ms(&self) -> Option<i64> {
        let (delay, (_, wall)) = (self.delay()?, self.since?);
        let at = (wall + delay).duration_since(UNIX_EPOCH).ok()?;
        i64::try_from(at.as_millis()).ok()
    }

    /// Unit tests: the pause is over.
    #[cfg(test)]
    pub fn expire(&mut self) {
        if let Some((at, wall)) = self.since {
            let back = BACKOFF_STEPS[BACKOFF_STEPS.len() - 1];
            self.since = Some((
                at.checked_sub(back).unwrap_or(at),
                wall.checked_sub(back).unwrap_or(wall),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// The phone's link to a server
// ---------------------------------------------------------------------------

/// Where the phone's sync goes for a link.
#[cfg(any(mobile, test))]
pub enum LinkTarget {
    /// A cloud app's folder (Android: through the folder plugin): its tree
    /// URI.
    Folder(String),
    /// A server's folder or a cloud account's (WebDAV, Dropbox).
    Server(Box<crate::remote::folder::RemoteDirFolder<Box<dyn Remote>>>),
}

/// The target of `link` (a server's is opened with its stored password;
/// one whose folder key no longer matches the link is refused).
#[cfg(any(mobile, test))]
pub fn target(core: &Core, link: &crate::folder_sync::LinkState) -> AppResult<LinkTarget> {
    use crate::folder_sync::LinkKind;
    match link.kind {
        LinkKind::Folder => Ok(LinkTarget::Folder(link.tree_uri.clone())),
        LinkKind::Webdav => {
            let server = link
                .server
                .as_ref()
                .ok_or_else(|| AppError::invalid("server_link"))?;
            let remote = open(core, server)?;
            if remote.key() != link.tree_uri {
                return Err(AppError::invalid("server_link"));
            }
            let remote: Box<dyn Remote> = Box::new(remote);
            Ok(LinkTarget::Server(Box::new(
                crate::remote::folder::RemoteDirFolder::new(remote),
            )))
        }
        LinkKind::Smb => {
            let smb = link
                .smb
                .as_ref()
                .ok_or_else(|| AppError::invalid("server_link"))?;
            let password = core.credentials().require(&core.data_dir(), &smb.key())?;
            let remote = crate::remote::smb::SmbRemote::new(smb, password)?;
            if remote.key() != link.tree_uri {
                return Err(AppError::invalid("server_link"));
            }
            let remote: Box<dyn Remote> = Box::new(remote);
            Ok(LinkTarget::Server(Box::new(
                crate::remote::folder::RemoteDirFolder::new(remote),
            )))
        }
        LinkKind::Dropbox | LinkKind::Gdrive => {
            let cloud = link
                .cloud
                .as_ref()
                .ok_or_else(|| AppError::invalid("server_link"))?;
            let remote = open_cloud(core, cloud)?;
            if remote.key() != link.tree_uri {
                return Err(AppError::invalid("server_link"));
            }
            Ok(LinkTarget::Server(Box::new(
                crate::remote::folder::RemoteDirFolder::new(remote),
            )))
        }
    }
}

/// `link_smb` without the phone's glue: the connection test must pass
/// (its code otherwise – `unsupported:sync_server_folder_missing` for a
/// folder that is not there yet), then the password is stored under the
/// share's key and the new link returned.
#[cfg(any(mobile, test))]
pub fn link_smb(
    core: &Core,
    cfg: &crate::remote::smb::SmbConfig,
    now: i64,
) -> AppResult<crate::folder_sync::LinkState> {
    let smb = cfg.share()?;
    let test = crate::remote::smb::SmbRemote::test(cfg);
    if test.folder_missing {
        return Err(AppError::unsupported(remote::FOLDER_MISSING));
    }
    if let Some(code) = test.code {
        return Err(AppError::Code(code));
    }
    let key = smb.key();
    core.credentials()
        .store(&core.data_dir(), &key, &cfg.password)?;
    Ok(crate::folder_sync::LinkState::new_smb(
        key,
        smb,
        now,
        test.writable,
    ))
}

/// `link_webdav` without the phone's glue: connects ([`connect`]) and
/// returns the new link (the caller replaces the previous one and forgets
/// its password, [`forget_password`]).
#[cfg(any(mobile, test))]
pub fn link_server(
    core: &Core,
    cfg: &WebDavConfig,
    now: i64,
) -> AppResult<crate::folder_sync::LinkState> {
    let Connected { server, key, test } = connect(core, cfg)?;
    Ok(crate::folder_sync::LinkState::new_server(
        key,
        server,
        now,
        test.writable,
    ))
}

// ---------------------------------------------------------------------------
// Cloud accounts (stage 2d: Dropbox, Google Drive)
// ---------------------------------------------------------------------------

/// `unsupported:dropbox_login_expired`: no login waits for a code (none was
/// started here, or it is older than [`LOGIN_MAX_AGE`]) – start again.
pub const LOGIN_EXPIRED: &str = "dropbox_login_expired";
/// A started login waits this long for its code.
pub const LOGIN_MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// A waiting login is offered again – the same page, code and verifier –
/// while it has at least this long left: "Seite erneut öffnen" and a dialog
/// opened again continue it instead of throwing it away.
const LOGIN_REUSE_LEFT: Duration = Duration::from_secs(3 * 60);

/// The Dropbox login that waits for its code: its PKCE verifier (never
/// leaves the backend), its page and when it began. One at a time.
struct DropboxLogin {
    verifier: Zeroizing<String>,
    url: String,
    at: Instant,
}

static PENDING: Mutex<Option<DropboxLogin>> = Mutex::new(None);

fn pending() -> std::sync::MutexGuard<'static, Option<DropboxLogin>> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The sync methods this build can use besides a cloud folder
/// (`AppInfo.syncProviders`): WebDAV always, Dropbox with an app key,
/// Google Drive with this platform's client id and secret.
pub fn providers() -> Vec<&'static str> {
    let mut out = vec!["webdav"];
    if dropbox::app_key().is_ok() {
        out.push("dropbox");
    }
    if gdrive::client().is_ok() {
        out.push("gdrive");
    }
    out
}

/// Starts a Dropbox login – or offers the waiting one again (see
/// [`LOGIN_REUSE_LEFT`]): the page opens the returned URL in the browser,
/// the user allows Keystead's app folder and copies the code Dropbox shows.
pub fn dropbox_start_login() -> AppResult<String> {
    let app_key = dropbox::app_key()?;
    let mut pending = pending();
    if let Some(login) = pending
        .as_ref()
        .filter(|l| l.at.elapsed() + LOGIN_REUSE_LEFT < LOGIN_MAX_AGE)
    {
        return Ok(login.url.clone());
    }
    let pkce = Pkce::new()?;
    let url = dropbox::authorize_url(app_key, &pkce.challenge);
    *pending = Some(DropboxLogin {
        verifier: pkce.verifier,
        url: url.clone(),
        at: Instant::now(),
    });
    Ok(url)
}

/// Whether a Dropbox login waits for its code (the dialog opens at the code
/// field again).
pub fn dropbox_waiting() -> bool {
    pending()
        .as_ref()
        .is_some_and(|l| l.at.elapsed() < LOGIN_MAX_AGE)
}

/// Finishes the login with the code: the account (no secret); its refresh
/// token is stored under its key ("Server credentials"). A wrong code keeps
/// the login waiting (the user may paste again); a success ends it.
pub fn dropbox_finish_login(core: &Core, code: &str) -> AppResult<SyncCloud> {
    let app_key = dropbox::app_key()?;
    let verifier = match pending().as_ref() {
        Some(login) if login.at.elapsed() < LOGIN_MAX_AGE => login.verifier.clone(),
        _ => return Err(AppError::unsupported(LOGIN_EXPIRED)),
    };
    let connected = dropbox::connect(app_key, code, &verifier, &Endpoints::current())?;
    let cloud = SyncCloud {
        provider: CloudProvider::Dropbox,
        account_id: connected.account.account_id,
        name: connected.account.name,
        email: connected.account.email,
    };
    core.credentials().store(
        &core.data_dir(),
        &cloud_key(&cloud),
        &connected.refresh_token,
    )?;
    *pending() = None;
    Ok(cloud)
}

/// `unsupported:gdrive_login_expired`: no Google login waits (none was
/// started here, it is older than [`LOGIN_MAX_AGE`], its codes expired, or
/// it was finished).
pub const GDRIVE_LOGIN_EXPIRED: &str = "gdrive_login_expired";

/// How a Google login comes back (`remote::gdrive`): the PC's loopback
/// redirect, the phone's device code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdriveFlow {
    Loopback,
    Device,
}

/// This platform's way.
fn gdrive_flow() -> GdriveFlow {
    if cfg!(mobile) {
        GdriveFlow::Device
    } else {
        GdriveFlow::Loopback
    }
}

/// What the page shows of a waiting Google login (`gdrive_login`,
/// `gdrive_login_ready`). No secret: the user code is what the user types
/// at Google's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GdriveLoginState {
    /// Google allowed it: `link_gdrive` / `set_sync_gdrive` finish it.
    pub ready: bool,
    /// The page to open: the consent page (PC), or the one where the code
    /// is entered (phone, `google.com/device`).
    pub url: String,
    /// The code to enter there (the phone); `null` on the PC.
    pub user_code: Option<String>,
}

enum GdriveWait {
    /// The PC: the verifier, the loopback address, and the listener's code
    /// once it came (`Some(None)`: the user refused).
    Loopback {
        verifier: Zeroizing<String>,
        redirect: String,
        outcome: Option<Option<Zeroizing<String>>>,
    },
    /// The phone: the device code, when to ask Google next, and what it
    /// said.
    Device {
        device_code: Zeroizing<String>,
        interval: Duration,
        next_poll: Instant,
        expires: Instant,
        tokens: Option<gdrive::Tokens>,
        refused: bool,
    },
}

/// The Google login that waits. One at a time; a new start replaces it (the
/// PC's listener of the old one then stops).
struct GdriveLogin {
    /// Which start this is (the PC's listener and a poll check it).
    id: u64,
    flow: GdriveFlow,
    at: Instant,
    url: String,
    user_code: Option<String>,
    wait: GdriveWait,
}

impl GdriveLogin {
    fn left(&self) -> Duration {
        let age = LOGIN_MAX_AGE.saturating_sub(self.at.elapsed());
        match &self.wait {
            GdriveWait::Device { expires, .. } => {
                age.min(expires.saturating_duration_since(Instant::now()))
            }
            GdriveWait::Loopback { .. } => age,
        }
    }

    fn ready(&self) -> bool {
        match &self.wait {
            GdriveWait::Loopback { outcome, .. } => matches!(outcome, Some(Some(_))),
            GdriveWait::Device { tokens, .. } => tokens.is_some(),
        }
    }

    fn refused(&self) -> bool {
        match &self.wait {
            GdriveWait::Loopback { outcome, .. } => matches!(outcome, Some(None)),
            GdriveWait::Device { refused, .. } => *refused,
        }
    }

    fn state(&self) -> GdriveLoginState {
        GdriveLoginState {
            ready: self.ready(),
            url: self.url.clone(),
            user_code: self.user_code.clone(),
        }
    }
}

static GDRIVE_PENDING: Mutex<Option<GdriveLogin>> = Mutex::new(None);
static GDRIVE_STARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn gdrive_pending() -> std::sync::MutexGuard<'static, Option<GdriveLogin>> {
    GDRIVE_PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Starts a Google login of this platform's way ([`gdrive_start`]).
pub fn gdrive_start_login() -> AppResult<GdriveLoginState> {
    gdrive_start(gdrive_flow())
}

/// Starts a Google login – or offers the waiting one again (see
/// [`LOGIN_REUSE_LEFT`]); the page opens `url`. **Loopback** (PC): a
/// listener on a free loopback port, started here, answers the browser and
/// stops with the redirect of its state, after [`LOGIN_MAX_AGE`] or when
/// another login starts. **Device** (phone): Google gives a code the user
/// enters at its page; [`gdrive_check_login`] asks whether it was allowed.
pub fn gdrive_start(flow: GdriveFlow) -> AppResult<GdriveLoginState> {
    use std::sync::atomic::Ordering;
    let client = gdrive::client()?;
    if let Some(login) = gdrive_pending()
        .as_ref()
        .filter(|l| l.flow == flow && !l.refused() && l.left() > LOGIN_REUSE_LEFT)
    {
        return Ok(login.state());
    }
    let id = GDRIVE_STARTS.fetch_add(1, Ordering::Relaxed) + 1;
    let at = Instant::now();
    match flow {
        GdriveFlow::Loopback => {
            let login = gdrive::Login::new()?;
            let (listener, redirect) = gdrive::listen_loopback()?;
            let url =
                gdrive::authorize_url(&client.id, &redirect, &login.pkce.challenge, &login.state);
            let started = GdriveLogin {
                id,
                flow,
                at,
                url,
                user_code: None,
                wait: GdriveWait::Loopback {
                    verifier: login.pkce.verifier,
                    redirect,
                    outcome: None,
                },
            };
            let state = started.state();
            *gdrive_pending() = Some(started);
            let login_state = login.state;
            let spawned = std::thread::Builder::new()
                .name("keystead-google-login".into())
                .spawn(move || {
                    let current = || gdrive_pending().as_ref().is_some_and(|p| p.id == id);
                    let outcome =
                        gdrive::serve_loopback(listener, &login_state, at + LOGIN_MAX_AGE, current);
                    if let Some(outcome) = outcome {
                        if let Some(p) = gdrive_pending().as_mut().filter(|p| p.id == id) {
                            if let GdriveWait::Loopback { outcome: slot, .. } = &mut p.wait {
                                *slot = Some(outcome.ok());
                            }
                        }
                    }
                });
            if let Err(e) = spawned {
                *gdrive_pending() = None;
                return Err(AppError::io(format!("google login listener: {e}")));
            }
            Ok(state)
        }
        GdriveFlow::Device => {
            let device = gdrive::start_device_login(&client, &gdrive::Endpoints::current())?;
            let started = GdriveLogin {
                id,
                flow,
                at,
                url: device.verification_url,
                user_code: Some(device.user_code),
                wait: GdriveWait::Device {
                    device_code: device.device_code,
                    interval: device.interval,
                    // The first ask comes right away (the user is still
                    // typing: "pending"), then every interval.
                    next_poll: at,
                    expires: at + device.expires_in,
                    tokens: None,
                    refused: false,
                },
            };
            let state = started.state();
            *gdrive_pending() = Some(started);
            Ok(state)
        }
    }
}

/// The state of the waiting Google login; a device login asks Google first
/// when its interval has passed (outside the lock; no connection or a
/// server error just waits for the next ask). `invalid_input:gdrive_redirect`
/// when the user refused (the login ends), `unsupported:gdrive_login_expired`
/// when none waits.
pub fn gdrive_check_login() -> AppResult<GdriveLoginState> {
    let poll = {
        let mut pending = gdrive_pending();
        let Some(login) = pending.as_ref().filter(|l| !l.left().is_zero()) else {
            *pending = None;
            return Err(AppError::unsupported(GDRIVE_LOGIN_EXPIRED));
        };
        match &login.wait {
            GdriveWait::Device {
                device_code,
                next_poll,
                tokens: None,
                refused: false,
                ..
            } if Instant::now() >= *next_poll => Some((login.id, device_code.clone())),
            _ => None,
        }
    };
    if let Some((id, device_code)) = poll {
        let answer = gdrive::client().and_then(|client| {
            gdrive::poll_device_login(&client, &device_code, &gdrive::Endpoints::current())
        });
        let mut pending = gdrive_pending();
        if let Some(login) = pending.as_mut().filter(|l| l.id == id) {
            if let GdriveWait::Device {
                interval,
                next_poll,
                expires,
                tokens,
                refused,
                ..
            } = &mut login.wait
            {
                let now = Instant::now();
                match answer {
                    Ok(gdrive::DevicePoll::Pending) => {}
                    Ok(gdrive::DevicePoll::SlowDown) => *interval += Duration::from_secs(5),
                    Ok(gdrive::DevicePoll::Done(t)) => *tokens = Some(t),
                    Ok(gdrive::DevicePoll::Refused) => *refused = true,
                    Ok(gdrive::DevicePoll::Expired) => *expires = now,
                    Err(e)
                        if remote::has_code(&e, remote::OFFLINE)
                            || remote::has_code(&e, remote::SERVER_ERROR) => {}
                    Err(e) => {
                        *pending = None;
                        return Err(e);
                    }
                }
                *next_poll = now + *interval;
            }
        }
    }
    let mut pending = gdrive_pending();
    let Some(login) = pending.as_ref().filter(|l| !l.left().is_zero()) else {
        *pending = None;
        return Err(AppError::unsupported(GDRIVE_LOGIN_EXPIRED));
    };
    if login.refused() {
        *pending = None;
        return Err(AppError::invalid(gdrive::BAD_REDIRECT));
    }
    Ok(login.state())
}

/// Finishes the Google login Google allowed (a login not yet allowed keeps
/// waiting): the PC exchanges its code (once – the login ends either way),
/// the phone has its tokens already; then the folder "Keystead" is made if
/// there is none, the refresh token stored under the account's key and the
/// account returned (no secret).
pub fn gdrive_finish_login(core: &Core) -> AppResult<SyncCloud> {
    let client = gdrive::client()?;
    let endpoints = gdrive::Endpoints::current();
    let login = {
        let mut pending = gdrive_pending();
        match pending.as_ref() {
            Some(l) if l.ready() && !l.left().is_zero() => pending.take(),
            _ => None,
        }
    };
    let tokens = match login.map(|l| l.wait) {
        Some(GdriveWait::Loopback {
            verifier,
            redirect,
            outcome: Some(Some(code)),
        }) => gdrive::exchange_code(&client, &code, &verifier, &redirect, &endpoints)?,
        Some(GdriveWait::Device {
            tokens: Some(tokens),
            ..
        }) => tokens,
        _ => return Err(AppError::unsupported(GDRIVE_LOGIN_EXPIRED)),
    };
    let connected = gdrive::connect(&client, tokens, &endpoints)?;
    let cloud = SyncCloud {
        provider: CloudProvider::Gdrive,
        account_id: connected.account.account_id,
        name: connected.account.name,
        email: connected.account.email,
    };
    core.credentials().store(
        &core.data_dir(),
        &cloud_key(&cloud),
        &connected.refresh_token,
    )?;
    Ok(cloud)
}

/// What the bases, the link and the token of a cloud account are recorded
/// for (`dropbox:<account id>`, `gdrive:<permission id>`).
pub fn cloud_key(cloud: &SyncCloud) -> String {
    match cloud.provider {
        CloudProvider::Dropbox => dropbox::key_of(&cloud.account_id),
        CloudProvider::Gdrive => gdrive::key_of(&cloud.account_id),
    }
}

/// The cloud account for a sync, with its stored token
/// (`unsupported:sync_credentials_missing` without one).
pub fn open_cloud(core: &Core, cloud: &SyncCloud) -> AppResult<Box<dyn Remote>> {
    let token = core
        .credentials()
        .require(&core.data_dir(), &cloud_key(cloud))?;
    match cloud.provider {
        CloudProvider::Dropbox => Ok(Box::new(DropboxRemote::new(
            dropbox::app_key()?,
            &cloud.account_id,
            token,
            Endpoints::current(),
        )?)),
        CloudProvider::Gdrive => Ok(Box::new(GdriveRemote::new(
            gdrive::client()?,
            &cloud.account_id,
            token,
            gdrive::Endpoints::current(),
        )?)),
    }
}

/// "Abmelden": this device forgets the token. Dropbox is also told to
/// revoke it (best effort – offline, it just stays valid there until it is
/// revoked in the account's settings); Google is not – its revoke ends the
/// grant of the whole project, i.e. the access of every other device of
/// the account too (the user can remove Keystead's access in the Google
/// account's settings, "Drittanbieter-Apps").
pub fn sign_out(core: &Core, cloud: &SyncCloud) {
    if cloud.provider == CloudProvider::Dropbox {
        let revoked = (|| -> AppResult<()> {
            let token = core
                .credentials()
                .require(&core.data_dir(), &cloud_key(cloud))?;
            DropboxRemote::new(
                dropbox::app_key()?,
                &cloud.account_id,
                token,
                Endpoints::current(),
            )?
            .revoke()
        })();
        if let Err(e) = revoked {
            log(format_args!(
                "the provider did not revoke the token: {}",
                e.code()
            ));
        }
    }
    forget_password(core, &cloud_key(cloud), None);
}

/// `link_dropbox` / `link_gdrive` without the phone's glue: the new link of
/// `cloud`.
#[cfg(any(mobile, test))]
pub fn link_cloud(cloud: SyncCloud, now: i64) -> crate::folder_sync::LinkState {
    crate::folder_sync::LinkState::new_cloud(cloud_key(&cloud), cloud, now)
}

// ---------------------------------------------------------------------------
// Commands (both platforms)
// ---------------------------------------------------------------------------

/// `dropbox_login` → the URL of Dropbox's page where the user allows
/// Keystead's app folder (the page opens it in the browser; a waiting login
/// is offered again); `unsupported:dropbox_not_configured` in a build
/// without an app key.
#[tauri::command]
pub async fn dropbox_login(core: Shared<'_>) -> CmdResult<String> {
    run(&core, true, |_| dropbox_start_login()).await
}

/// `dropbox_login_waiting` → whether a Dropbox login waits for its code (the
/// dialog opens at the code field). Asked when the dialog opens – no
/// activity.
#[tauri::command]
pub async fn dropbox_login_waiting(core: Shared<'_>) -> CmdResult<bool> {
    run(&core, false, |_| Ok(dropbox_waiting())).await
}

/// `gdrive_login` → [`GdriveLoginState`]: Google's consent page (PC) or the
/// page and code of a device login (phone) – the page opens `url`, then asks
/// `gdrive_login_ready`; a waiting login is offered again.
/// `unsupported:gdrive_not_configured` in a build without this platform's
/// client.
#[tauri::command]
pub async fn gdrive_login(core: Shared<'_>) -> CmdResult<GdriveLoginState> {
    run(&core, true, |_| gdrive_start_login()).await
}

/// `gdrive_login_ready` → [`GdriveLoginState`] of the waiting login (`ready`:
/// Google allowed it, then `link_gdrive` / `set_sync_gdrive` finish it). The
/// page asks every second while its dialog waits, and when the dialog opens
/// (to continue a waiting login) – no activity, so the idle lock still
/// applies. Errors: see [`gdrive_check_login`].
#[tauri::command]
pub async fn gdrive_login_ready(core: Shared<'_>) -> CmdResult<GdriveLoginState> {
    run(&core, false, |_| gdrive_check_login()).await
}

/// `webdav_test(config)` → [`WebDavTest`]: the settings form's connection
/// test (server, certificate, login, folder). Nothing is stored; the
/// password is not echoed.
#[tauri::command]
pub async fn webdav_test(core: Shared<'_>, config: Value) -> CmdResult<WebDavTest> {
    run(&core, true, move |_| {
        let cfg: WebDavConfig = parse(config, BAD_CONFIG)?;
        Ok(WebDavRemote::test(&cfg))
    })
    .await
}

/// `webdav_create_folder(config)` → [`WebDavTest`]: "Ordner anlegen" – the
/// server's sync folder is created (one that exists is fine) and the
/// connection tested again. Never done by a sync.
#[tauri::command]
pub async fn webdav_create_folder(core: Shared<'_>, config: Value) -> CmdResult<WebDavTest> {
    run(&core, true, move |_| {
        let cfg: WebDavConfig = parse(config, BAD_CONFIG)?;
        WebDavRemote::new(&cfg)?.create_folder(&cfg)?;
        Ok(WebDavRemote::test(&cfg))
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use keystead_core::settings::Settings;
    use keystead_core::{KdfParams, VaultStore};

    use super::*;
    use crate::folder_sync::{self, LinkKind, LinkState, Outcome, RemoteFolder as _};
    use crate::remote::test_server::TestServer;

    fn cfg(server: &TestServer, password: &str) -> WebDavConfig {
        WebDavConfig {
            url: server.base.clone(),
            user: "alice".into(),
            password: Zeroizing::new(password.into()),
            folder: "Keystead".into(),
            cert_sha256: None,
        }
    }

    fn core(dir: &std::path::Path) -> Arc<Core> {
        Core::for_tests(VaultStore::new(dir.join("phone")), Settings::default())
    }

    #[test]
    fn the_backoff_grows_1_2_5_15_minutes_and_ends_with_a_success() {
        let t0 = Instant::now();
        let w0 = SystemTime::now();
        let mut b = Backoff::default();
        assert!(b.allows(t0, w0));
        assert_eq!(b.retry_at_ms(), None);
        let offline = AppError::unsupported(remote::OFFLINE).code();
        let minutes = |m: u64| Duration::from_secs(m * 60);
        for (n, expected) in [1, 2, 5, 15, 15, 15].into_iter().enumerate() {
            b.record(Some(&offline), t0, w0);
            assert_eq!(b.delay(), Some(minutes(expected)), "failure {}", n + 1);
            let just_before = minutes(expected) - Duration::from_secs(1);
            assert!(!b.allows(t0 + just_before, w0 + just_before));
            assert!(b.allows(t0 + minutes(expected), w0));
            // The wall clock counts too (the monotonic one stands still
            // while the phone sleeps).
            assert!(b.allows(t0, w0 + minutes(expected)));
        }
        let at = w0 + minutes(15);
        assert_eq!(
            b.retry_at_ms(),
            Some(at.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64)
        );
        // A sync that reached the server ends it – also one with an error
        // of a single file.
        b.record(None, t0, w0);
        assert_eq!(b, Backoff::default());
        for code in [
            remote::SERVER_ERROR,
            remote::AUTH_FAILED,
            remote::CERTIFICATE,
            remote::FOLDER_MISSING,
            remote::smb::LOCAL_NETWORK,
        ] {
            b.record(Some(&AppError::unsupported(code).code()), t0, w0);
            assert_eq!(b.delay(), Some(minutes(1)), "{code}");
            b.reset();
        }
        b.record(Some(&offline), t0, w0);
        b.record(
            Some(&AppError::unsupported(remote::TOO_LARGE).code()),
            t0,
            w0,
        );
        assert!(b.allows(t0, w0));
        for code in [
            AppError::unsupported(crate::credentials::MISSING).code(),
            AppError::invalid(remote::OFFLINE).code(),
            "io:disk".to_owned(),
        ] {
            assert!(!backs_off(&code), "{code}");
        }
        b.record(Some(&offline), t0, w0);
        b.expire();
        assert!(b.allows_now());
    }

    #[test]
    fn connecting_stores_the_password_and_the_server_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let core = core(dir.path());
        let server = TestServer::http("alice", "pw");
        // The folder is missing: nothing is stored.
        let e = connect(&core, &cfg(&server, "pw")).unwrap_err();
        assert_eq!(e.code(), "unsupported:sync_server_folder_missing");
        server.mkdir("Keystead");
        let e = connect(&core, &cfg(&server, "wrong")).unwrap_err();
        assert_eq!(e.code(), "unsupported:sync_auth_failed");
        assert!(!crate::credentials::dir(&core.data_dir()).exists());

        let Connected {
            server: stored,
            key,
            test,
        } = connect(&core, &cfg(&server, "pw")).unwrap();
        assert!(test.ok && test.writable && test.cas, "{test:?}");
        assert_eq!(key, format!("webdav:{}Keystead/", server.base));
        assert_eq!(key_of(&stored).unwrap(), key);
        assert_eq!(stored.url, server.base);
        assert_eq!(stored.user, "alice");
        assert_eq!(stored.folder, "Keystead");
        assert!(stored.cas, "the test server honours If-Match");
        assert_eq!(
            core.credentials()
                .require(&core.data_dir(), &key)
                .unwrap()
                .as_str(),
            "pw"
        );
        assert!(!serde_json::to_string(&stored).unwrap().contains("pw\""));
        // Opened for a sync with the stored password.
        let remote = open(&core, &stored).unwrap();
        assert!(remote.probe().unwrap().exists);
        core.credentials().remove(&core.data_dir(), &key).unwrap();
        assert_eq!(
            open(&core, &stored).unwrap_err().code(),
            "unsupported:sync_credentials_missing"
        );
    }

    #[test]
    fn the_client_runs_on_the_commands_blocking_pool() {
        // `commands::run` executes on Tauri's blocking pool – inside the
        // async runtime's context; the client's own runtime must work there.
        let server = TestServer::http("alice", "pw");
        server.mkdir("Keystead");
        let config = cfg(&server, "pw");
        let test =
            tauri::async_runtime::block_on(tauri::async_runtime::spawn_blocking(move || {
                WebDavRemote::test(&config)
            }))
            .unwrap();
        assert!(test.ok, "{test:?}");
    }

    #[test]
    fn a_dropbox_login_links_the_phone_and_signing_out_forgets_the_token() {
        use crate::remote::dropbox::fake::FakeDropbox;
        use crate::remote::dropbox::TEST_ENDPOINTS;

        let fake = FakeDropbox::new();
        TEST_ENDPOINTS.with(|e| *e.borrow_mut() = Some(fake.endpoints()));
        let dir = tempfile::tempdir().unwrap();
        let core = core(dir.path());
        // Nothing started: the code has no login to finish.
        *pending() = None;
        assert!(!dropbox_waiting());
        assert_eq!(
            dropbox_finish_login(&core, "x").unwrap_err().code(),
            "unsupported:dropbox_login_expired"
        );
        let url = dropbox_start_login().unwrap();
        assert!(
            url.starts_with("https://www.dropbox.com/oauth2/authorize?"),
            "{url}"
        );
        assert!(!url.contains("redirect_uri"));
        assert!(dropbox_waiting());
        // Opened again: the same login (the code the user copied still fits).
        assert_eq!(dropbox_start_login().unwrap(), url);
        // The fake knows its own verifier (the started one is random).
        pending().as_mut().unwrap().verifier = Zeroizing::new(fake.verifier());
        // A wrong code: refused, the login still waits for the right one.
        assert_eq!(
            dropbox_finish_login(&core, "falsch").unwrap_err().code(),
            "invalid_input:dropbox_code"
        );
        let cloud = dropbox_finish_login(&core, &fake.code()).unwrap();
        assert_eq!(cloud.account_id, "dbid:test-account");
        assert_eq!(cloud.email, "max@example.com");
        assert!(pending().is_none(), "a success ends the login");
        assert!(!dropbox_waiting());
        let key = cloud_key(&cloud);
        assert_eq!(key, "dropbox:dbid:test-account");
        assert!(core
            .credentials()
            .load(&core.data_dir(), &key)
            .unwrap()
            .is_some());
        assert!(!serde_json::to_string(&cloud).unwrap().contains("tok-"));

        // The PC's vault in the app folder; the phone links and imports it.
        let pc = VaultStore::new(dir.path().join("pc"));
        let v = pc
            .create_vault_with_params("Privat", "pc-pw", KdfParams::insecure_for_tests())
            .unwrap();
        let name = format!("{}.keystead", v.id());
        fake.write(&name, &fs::read(v.path()).unwrap());
        let mut link = link_cloud(cloud.clone(), 2);
        assert_eq!(link.kind, LinkKind::Dropbox);
        assert_eq!(link.tree_uri, key);
        assert!(link.writable);
        let LinkTarget::Server(folder) = target(&core, &link).unwrap() else {
            panic!("a Dropbox link goes to Dropbox");
        };
        assert!(folder_sync::sync_folder(&core, &*folder, &mut link, 3).unwrap());
        let result = link.files.iter().find(|f| f.file_name == name).unwrap();
        assert_eq!(result.outcome, Outcome::Imported);
        // A link whose account no longer has its key is refused.
        let mut forged = link.clone();
        forged.tree_uri = "dropbox:someone-else".into();
        assert_eq!(
            target(&core, &forged).err().unwrap().code(),
            "invalid_input:server_link"
        );

        // Signing out: the token goes (here and at Dropbox).
        sign_out(&core, &cloud);
        assert_eq!(
            core.credentials().load(&core.data_dir(), &key).unwrap(),
            None
        );
        assert_eq!(
            target(&core, &link).err().unwrap().code(),
            "unsupported:sync_credentials_missing"
        );
        TEST_ENDPOINTS.with(|e| *e.borrow_mut() = None);
    }

    /// The Google tests share the one waiting login: one at a time.
    static GOOGLE_TESTS: Mutex<()> = Mutex::new(());

    fn google_test() -> std::sync::MutexGuard<'static, ()> {
        GOOGLE_TESTS.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The verifier of the waiting Google loopback login (the fake knows its
    /// own; the started one is random).
    fn use_fake_verifier(verifier: &str) {
        match &mut gdrive_pending().as_mut().unwrap().wait {
            GdriveWait::Loopback { verifier: v, .. } => *v = Zeroizing::new(verifier.to_owned()),
            GdriveWait::Device { .. } => panic!("a loopback login"),
        }
    }

    /// The waiting device login asks Google at the next check.
    fn poll_now() {
        if let GdriveWait::Device { next_poll, .. } = &mut gdrive_pending().as_mut().unwrap().wait {
            *next_poll = Instant::now();
        }
    }

    #[test]
    fn a_google_login_on_the_pc_waits_for_its_redirect() {
        let _serial = google_test();
        use crate::remote::gdrive::fake::FakeDrive;
        use crate::remote::gdrive::TEST_ENDPOINTS;

        let fake = FakeDrive::new();
        TEST_ENDPOINTS.with(|e| *e.borrow_mut() = Some(fake.endpoints()));
        let dir = tempfile::tempdir().unwrap();
        let core = core(dir.path());
        *gdrive_pending() = None;
        assert_eq!(
            gdrive_check_login().unwrap_err().code(),
            "unsupported:gdrive_login_expired"
        );
        assert_eq!(
            gdrive_finish_login(&core).unwrap_err().code(),
            "unsupported:gdrive_login_expired"
        );
        let started = gdrive_start(GdriveFlow::Loopback).unwrap();
        assert_eq!(started.user_code, None);
        assert!(!started.ready);
        let url = reqwest::Url::parse(&started.url).unwrap();
        let q: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("accounts.google.com"));
        assert!(q["redirect_uri"].starts_with("http://127.0.0.1:"));
        assert_eq!(q["scope"], gdrive::SCOPE);
        // Opened again: the same login (same page, state and listener).
        assert_eq!(gdrive_start(GdriveFlow::Loopback).unwrap(), started);
        assert!(!gdrive_check_login().unwrap().ready, "nothing came yet");
        // Not yet allowed: finishing ends nothing.
        assert_eq!(
            gdrive_finish_login(&core).unwrap_err().code(),
            "unsupported:gdrive_login_expired"
        );
        assert!(gdrive_pending().is_some());
        // Google sends the browser back.
        use_fake_verifier(&fake.verifier());
        let mut browser = std::net::TcpStream::connect((
            "127.0.0.1",
            reqwest::Url::parse(&q["redirect_uri"])
                .unwrap()
                .port()
                .unwrap(),
        ))
        .unwrap();
        {
            use std::io::{Read as _, Write as _};
            write!(
                browser,
                "GET /?state={}&code={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                q["state"],
                fake.code()
            )
            .unwrap();
            let mut page = String::new();
            browser.read_to_string(&mut page).unwrap();
            assert!(page.contains("abgeschlossen"), "{page}");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !gdrive_check_login().unwrap().ready {
            assert!(Instant::now() < deadline, "the redirect never arrived");
            std::thread::sleep(Duration::from_millis(20));
        }
        let cloud = gdrive_finish_login(&core).unwrap();
        assert_eq!(cloud.provider, CloudProvider::Gdrive);
        assert_eq!(cloud.email, "max@example.com");
        assert!(gdrive_pending().is_none(), "a code is used once");
        assert_eq!(cloud_key(&cloud), "gdrive:perm-1");
        assert!(fake.has_folder());
        // A refused login ends.
        let started = gdrive_start(GdriveFlow::Loopback).unwrap();
        let url = reqwest::Url::parse(&started.url).unwrap();
        let q: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        let mut browser = std::net::TcpStream::connect((
            "127.0.0.1",
            reqwest::Url::parse(&q["redirect_uri"])
                .unwrap()
                .port()
                .unwrap(),
        ))
        .unwrap();
        {
            use std::io::{Read as _, Write as _};
            write!(
                browser,
                "GET /?state={}&error=access_denied HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                q["state"]
            )
            .unwrap();
            let mut page = String::new();
            browser.read_to_string(&mut page).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match gdrive_check_login() {
                Ok(state) => assert!(!state.ready),
                Err(e) => {
                    assert_eq!(e.code(), "invalid_input:gdrive_redirect");
                    break;
                }
            }
            assert!(Instant::now() < deadline, "the refusal never arrived");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gdrive_pending().is_none());
        TEST_ENDPOINTS.with(|e| *e.borrow_mut() = None);
    }

    #[test]
    fn a_google_login_on_the_phone_is_a_device_code_and_links_the_folder() {
        let _serial = google_test();
        use crate::remote::gdrive::fake::{Device, FakeDrive, USER_CODE};
        use crate::remote::gdrive::TEST_ENDPOINTS;

        let fake = FakeDrive::new();
        TEST_ENDPOINTS.with(|e| *e.borrow_mut() = Some(fake.endpoints()));
        let dir = tempfile::tempdir().unwrap();
        let core = core(dir.path());
        *gdrive_pending() = None;
        let started = gdrive_start(GdriveFlow::Device).unwrap();
        assert_eq!(started.user_code.as_deref(), Some(USER_CODE));
        assert_eq!(started.url, "https://www.google.com/device");
        assert!(!serde_json::to_string(&started)
            .unwrap()
            .contains("the-device-code"));
        // The first check asks Google ("pending"), the next waits its interval.
        assert!(!gdrive_check_login().unwrap().ready);
        assert!(!gdrive_check_login().unwrap().ready);
        assert_eq!(fake.device_polls(), 1);
        // Opened again: the same code.
        assert_eq!(gdrive_start(GdriveFlow::Device).unwrap(), started);
        // "Slow down" waits longer; then the user allows.
        fake.slow_down_next_poll();
        poll_now();
        assert!(!gdrive_check_login().unwrap().ready);
        fake.set_device(Device::Allowed);
        assert!(!gdrive_check_login().unwrap().ready, "within the interval");
        poll_now();
        assert!(gdrive_check_login().unwrap().ready);
        let cloud = gdrive_finish_login(&core).unwrap();
        assert_eq!(cloud.account_id, "perm-1");
        assert!(gdrive_pending().is_none());
        assert_eq!(fake.device(), Some(Device::Used));
        let key = cloud_key(&cloud);
        assert!(core
            .credentials()
            .load(&core.data_dir(), &key)
            .unwrap()
            .is_some());

        // The PC's vault in the Drive folder; the phone links and imports it.
        let pc = VaultStore::new(dir.path().join("pc"));
        let v = pc
            .create_vault_with_params("Privat", "pc-pw", KdfParams::insecure_for_tests())
            .unwrap();
        let name = format!("{}.keystead", v.id());
        fake.add_file(&name, &fs::read(v.path()).unwrap());
        let mut link = link_cloud(cloud.clone(), 2);
        assert_eq!(link.kind, LinkKind::Gdrive);
        assert_eq!(link.folder_name, "Google Drive");
        let LinkTarget::Server(folder) = target(&core, &link).unwrap() else {
            panic!("a Google Drive link goes to Google Drive");
        };
        assert!(folder_sync::sync_folder(&core, &*folder, &mut link, 3).unwrap());
        let result = link.files.iter().find(|f| f.file_name == name).unwrap();
        assert_eq!(result.outcome, Outcome::Imported);

        // A refused device login ends; an expired one too.
        gdrive_start(GdriveFlow::Device).unwrap();
        fake.set_device(Device::Refused);
        assert_eq!(
            gdrive_check_login().unwrap_err().code(),
            "invalid_input:gdrive_redirect"
        );
        assert!(gdrive_pending().is_none());
        gdrive_start(GdriveFlow::Device).unwrap();
        fake.set_device(Device::Expired);
        assert_eq!(
            gdrive_check_login().unwrap_err().code(),
            "unsupported:gdrive_login_expired"
        );

        // Signing out: this device forgets the token; Google is not told
        // (its revoke would end every other device's access too).
        assert_eq!(fake.refresh_tokens(), 1);
        sign_out(&core, &cloud);
        assert_eq!(fake.refresh_tokens(), 1);
        assert_eq!(
            core.credentials().load(&core.data_dir(), &key).unwrap(),
            None
        );
        TEST_ENDPOINTS.with(|e| *e.borrow_mut() = None);
    }

    #[test]
    fn an_smb_link_needs_a_share_that_answers_and_its_password() {
        let dir = tempfile::tempdir().unwrap();
        let core = core(dir.path());
        // A closed port: offline, nothing stored.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let cfg: crate::remote::smb::SmbConfig = serde_json::from_value(serde_json::json!({
            "server": format!("127.0.0.1:{port}"),
            "share": "Daten",
            "folder": "Keystead",
            "user": "max",
            "password": "pw",
        }))
        .unwrap();
        assert_eq!(
            link_smb(&core, &cfg, 1).unwrap_err().code(),
            "unsupported:sync_offline"
        );
        let smb = cfg.share().unwrap();
        assert_eq!(
            core.credentials()
                .load(&core.data_dir(), &smb.key())
                .unwrap(),
            None
        );
        // A link without its password: missing, not a wrong target.
        let link = crate::folder_sync::LinkState::new_smb(smb.key(), smb.clone(), 1, true);
        assert_eq!(link.kind, LinkKind::Smb);
        assert_eq!(link.folder_name, smb.label());
        assert!(link.folder_name.ends_with(r"\Daten\Keystead"));
        assert_eq!(
            target(&core, &link).err().unwrap().code(),
            "unsupported:sync_credentials_missing"
        );
        // Another share's key: refused.
        let mut forged = link.clone();
        forged.tree_uri = "smb://x@other/share/".into();
        core.credentials()
            .store(&core.data_dir(), &smb.key(), &Zeroizing::new("pw".into()))
            .unwrap();
        assert_eq!(
            target(&core, &forged).err().unwrap().code(),
            "invalid_input:server_link"
        );
        let json = serde_json::to_string(&link).unwrap();
        assert!(
            json.contains("\"kind\":\"smb\"") && !json.contains("pw"),
            "{json}"
        );
    }

    #[test]
    fn a_phone_linked_to_a_server_syncs_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::http("alice", "pw");
        server.mkdir("Keystead");
        // The PC's vault on the server.
        let pc = VaultStore::new(dir.path().join("pc"));
        let v = pc
            .create_vault_with_params("Privat", "pc-pw", KdfParams::insecure_for_tests())
            .unwrap();
        let name = format!("{}.keystead", v.id());
        server.put(&format!("Keystead/{name}"), &fs::read(v.path()).unwrap());

        let core = core(dir.path());
        // An earlier folder link: its target is the plugin's tree.
        let folder_link = LinkState::new("content://tree/x".into(), "X".into(), 1, true);
        assert!(matches!(
            target(&core, &folder_link).unwrap(),
            LinkTarget::Folder(tree) if tree == "content://tree/x"
        ));
        let mut link = link_server(&core, &cfg(&server, "pw"), 2).unwrap();
        assert_eq!(link.kind, LinkKind::Webdav);
        assert!(link.writable);
        assert_eq!(link.tree_uri, format!("webdav:{}Keystead/", server.base));
        let LinkTarget::Server(folder) = target(&core, &link).unwrap() else {
            panic!("a server link goes to the server");
        };
        assert!(folder.writable().unwrap());
        assert!(folder_sync::sync_folder(&core, &*folder, &mut link, 3).unwrap());
        let result = link.files.iter().find(|f| f.file_name == name).unwrap();
        assert_eq!(result.outcome, Outcome::Imported);
        assert_eq!(
            fs::read(core.store().vault_path(v.id())).unwrap(),
            fs::read(v.path()).unwrap()
        );

        // Another server: the first one's password is forgotten once the
        // new link is in place (not for the same server folder).
        let other = TestServer::http("alice", "pw2");
        other.mkdir("Keystead");
        let first_key = link.tree_uri.clone();
        let second = link_server(&core, &cfg(&other, "pw2"), 4).unwrap();
        assert_ne!(second.tree_uri, first_key);
        forget_password(&core, &first_key, Some(&first_key));
        assert!(core
            .credentials()
            .load(&core.data_dir(), &first_key)
            .unwrap()
            .is_some());
        forget_password(&core, &first_key, Some(&second.tree_uri));
        assert_eq!(
            core.credentials()
                .load(&core.data_dir(), &first_key)
                .unwrap(),
            None
        );
        assert!(core
            .credentials()
            .load(&core.data_dir(), &second.tree_uri)
            .unwrap()
            .is_some());
        // A link whose server no longer matches its key is refused.
        let mut forged = second.clone();
        forged.tree_uri = first_key;
        assert_eq!(
            target(&core, &forged).err().unwrap().code(),
            "invalid_input:server_link"
        );
    }
}
