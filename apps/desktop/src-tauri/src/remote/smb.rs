//! An SMB share – a Windows share, a NAS, Samba – as a [`Remote`]
//! (2026-11-05): the phone reaches the folder the PC keeps its copies in
//! (the PC mounts the share itself and uses it as its cloud folder). SMB
//! 2/3 in pure Rust (`smb2`), NTLM login (user, password, an optional
//! domain; an empty user works only where the server maps unknown users to
//! its guest), signing and encryption as the server asks. On Android 17+
//! the home network needs `ACCESS_LOCAL_NETWORK` (`mobile::sync` asks).
//!
//! * **Where**: the cloud folder's layout below `<share>/<folder>`
//!   (`folder` may be nested, `""` is the share's top).
//! * **Writes**: to a temporary name next to the file, then renamed over it
//!   (a reader never sees half a vault); "only if new" is a rename that
//!   refuses an existing name. SMB has no conditional write: `cas` is false
//!   – the ETag (change time and size) is compared right before an update
//!   or a delete, and the engines read the copy again before they write.
//! * **Answers are untrusted**: ≤ [`super::dropbox::LIST_ENTRIES_MAX`]
//!   entries, names of one segment only.
//! * **Time**: a connection gets [`CONNECT_LIMIT`], a request
//!   [`super::http::REQUEST_BASE`] plus its bytes at
//!   [`super::http::MIN_RATE`]; a lost connection is made again for the
//!   next request.
//! * **Errors keep their reason**: a failed request is noted as step, kind
//!   and the server's status (`login: AuthRequired (SessionSetup
//!   STATUS_LOGON_FAILURE)`) – in the log (logcat on the phone; not a
//!   missing file or a taken name, which the callers expect) and as the
//!   form test's `detail` (a file where the folder should be has none);
//!   never smb2's own text, which can name the user, the host or a path.
//!   "Offline" is only a server that cannot be dialled or named and a
//!   connection that broke while it was in use; one that took the
//!   connection and then closed it or fell silent before the share was
//!   open, or that leaves a request without an answer, is `smb_no_answer`.
//! * **Security**: NTLM does not prove the server's identity – meant for
//!   the user's own network; the vault files are encrypted anyway. The
//!   password is stored like a WebDAV password (`crate::credentials`).

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use keystead_core::crypto;
use serde::{Deserialize, Serialize};
use smb2::{ClientConfig, ErrorKind, RenameOptions, SmbClient, Tree};
use zeroize::Zeroizing;

use super::dropbox::LIST_ENTRIES_MAX;
use super::http::{MIN_RATE, REQUEST_BASE};
use super::{
    check_dir, check_path, Access, Precondition, Remote, RemoteMeta, AUTH_FAILED, BAD_RESPONSE,
    CHANGED, FILE_MISSING, FOLDER_MISSING, NOT_READABLE, NOT_WRITABLE, OFFLINE, QUOTA,
    SERVER_ERROR, TOO_LARGE,
};
use crate::error::{AppError, AppResult};
use crate::state::log;

/// `invalid_input:smb_config`: the form's server, share or folder is not one.
pub const BAD_CONFIG: &str = "smb_config";
/// `unsupported:smb_share_missing`: the server has no such share.
pub const SHARE_MISSING: &str = "smb_share_missing";
/// `unsupported:smb_local_network`: the phone may not reach the home network
/// (Android 17+ without `ACCESS_LOCAL_NETWORK` – its connections there time
/// out); `mobile::sync` says so instead of `sync_offline`.
pub const LOCAL_NETWORK: &str = "smb_local_network";
/// `unsupported:smb_share_denied`: the login worked, but this account may
/// not use the share.
pub const SHARE_DENIED: &str = "smb_share_denied";
/// `unsupported:smb_no_answer`: the server took the connection, then closed
/// it or fell silent before the share was open, or it left a request
/// without an answer – not "offline": something answers at that address.
pub const NO_ANSWER: &str = "smb_no_answer";
/// `unsupported:smb_not_a_folder`: the form's folder path names a file.
pub const NOT_A_FOLDER: &str = "smb_not_a_folder";
/// The most a connection (TCP, negotiation, login, share) may take – the
/// app's own cap, so that the form answers. smb2's limits for the parts
/// ([`CONNECT_TIMEOUT`] for the dial, then [`ANSWER_TIMEOUT`] of silence
/// per answer – a NAS may have to wake its disks) could add up to more.
pub const CONNECT_LIMIT: Duration = Duration::from_secs(75);
/// The dial's budget: the name lookup and the TCP connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long smb2 waits for an answer before it gives a request up (its
/// `RESPONSE_TIMEOUT`, the default of `ClientConfig::response_timeout`).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PORT: u16 = 445;

/// What the form sends (`smb_test`, `smb_create_folder`, `link_smb`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbConfig {
    /// `nas.local`, `192.168.1.10`, `nas:4455`, also `\\nas` or `smb://nas`.
    pub server: String,
    pub share: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub user: String,
    #[serde(default, deserialize_with = "super::webdav::zeroizing")]
    pub password: Zeroizing<String>,
    #[serde(default)]
    pub domain: String,
}

/// The share as the phone's link stores it (`link.json`) – no password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSmb {
    pub server: String,
    pub share: String,
    /// Below the share, `/`-separated; `""` = its top.
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub domain: String,
}

impl SmbConfig {
    /// The share, normalized and checked (`invalid_input:smb_config`).
    pub fn share(&self) -> AppResult<SyncSmb> {
        let bad = || AppError::invalid(BAD_CONFIG);
        let mut server = self.server.trim();
        for prefix in ["smb://", "\\\\", "//"] {
            // `get`, not a slice by bytes: a name such as `Büro-PC` has no
            // character boundary there (a slice would panic).
            if server
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            {
                server = &server[prefix.len()..];
            }
        }
        let server = server.trim_end_matches(['/', '\\']);
        if server.is_empty()
            || server.len() > 253
            || server
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '@'))
        {
            return Err(bad());
        }
        // `nas:` or a PC drive (`Z:`): no port – a wrong form, not "offline".
        if let Some(port) = port_of(server) {
            if !matches!(port.parse::<u16>(), Ok(p) if p > 0) {
                return Err(bad());
            }
        }
        let share = self.share.trim().trim_matches(['/', '\\']);
        if share.is_empty()
            || share.len() > 80
            || share.chars().any(|c| {
                matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
            })
        {
            return Err(bad());
        }
        let mut segments = Vec::new();
        for segment in self.folder.split(['/', '\\']).map(str::trim) {
            if segment.is_empty() {
                continue;
            }
            // A name ending in a dot (`.`, `..`, `Backup.`): SMB cannot carry
            // it as it is – the client would ask for another name.
            if segment.ends_with('.')
                || segment
                    .chars()
                    .any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
            {
                return Err(bad());
            }
            segments.push(segment);
        }
        let mut user = self.user.trim();
        let mut domain = self.domain.trim();
        // `NAS\max` or `.\max`, as Windows writes a login: the part in front
        // is the domain, the server knows the account by the name alone.
        if domain.is_empty() {
            if let Some((front, name)) = user.split_once('\\') {
                if !name.is_empty() && !name.contains('\\') {
                    domain = if front == "." { "" } else { front };
                    user = name;
                }
            }
        }
        if user.len() > 256 || domain.len() > 256 {
            return Err(bad());
        }
        Ok(SyncSmb {
            server: server.to_owned(),
            share: share.to_owned(),
            folder: segments.join("/"),
            user: user.to_owned(),
            domain: domain.to_owned(),
        })
    }
}

/// The port written after the server (`nas:4455`, `[fe80::1]:446`), as
/// [`SyncSmb::addr`] reads it; `None` without one (also a bare IPv6 address).
fn port_of(server: &str) -> Option<&str> {
    if let Some(rest) = server.strip_prefix('[') {
        rest.split_once("]:").map(|(_, port)| port)
    } else if server.matches(':').count() == 1 {
        server.split_once(':').map(|(_, port)| port)
    } else {
        None
    }
}

impl SyncSmb {
    /// What the bases, the link and the password are recorded for
    /// (`smb://user@server/share/folder`, in lower case – SMB names are not
    /// case-sensitive).
    pub fn key(&self) -> String {
        format!(
            "smb://{}@{}/{}/{}",
            self.user, self.server, self.share, self.folder
        )
        .to_lowercase()
    }

    /// `host:port` for the client (port 445 unless given).
    fn addr(&self) -> String {
        let has_port = if let Some(rest) = self.server.strip_prefix('[') {
            rest.contains("]:")
        } else {
            self.server.matches(':').count() == 1
        };
        if has_port {
            self.server.clone()
        } else if self.server.contains(':') && !self.server.starts_with('[') {
            format!("[{}]:{DEFAULT_PORT}", self.server)
        } else {
            format!("{}:{DEFAULT_PORT}", self.server)
        }
    }

    /// `\\server\share\folder`, as Windows shows it.
    pub fn label(&self) -> String {
        let mut out = format!("\\\\{}\\{}", self.server, self.share);
        for segment in self.folder.split('/').filter(|s| !s.is_empty()) {
            out.push('\\');
            out.push_str(segment);
        }
        out
    }
}

/// What the connection test found (`smb_test`). No secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbTest {
    /// The folder exists and can be used.
    pub ok: bool,
    /// The error code otherwise.
    pub code: Option<String>,
    /// The share is fine, the folder does not exist yet ("Ordner anlegen").
    pub folder_missing: bool,
    /// The user may write the folder (a test file could be made).
    pub writable: bool,
    /// What failed, technically ([`detail`]: step, kind, the server's
    /// status) – shown small below the error. No names, no secrets.
    pub detail: Option<String>,
}

/// What a request is for: it decides what "access denied" means and names
/// the step in the diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// TCP, negotiation and the login.
    Login,
    /// Opening the share.
    Share,
    List,
    Stat,
    Read,
    Write,
    Rename,
    Mkdir,
    Delete,
}

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Login => "login",
            Step::Share => "share",
            Step::List => "list",
            Step::Stat => "stat",
            Step::Read => "read",
            Step::Write => "write",
            Step::Rename => "rename",
            Step::Mkdir => "mkdir",
            Step::Delete => "delete",
        }
    }

    /// A change was asked for.
    fn writes(self) -> bool {
        matches!(
            self,
            Step::Write | Step::Rename | Step::Mkdir | Step::Delete
        )
    }

    /// Before the share is open.
    fn connecting(self) -> bool {
        matches!(self, Step::Login | Step::Share)
    }
}

/// The code of an SMB error at `step`.
fn code(e: &smb2::Error, step: Step) -> &'static str {
    match e.kind() {
        ErrorKind::AuthRequired | ErrorKind::GuestSessionRefused => AUTH_FAILED,
        // Past the login the account was accepted: what is denied is the
        // share, the folder or the file – not the password.
        ErrorKind::AccessDenied => match step {
            Step::Login => AUTH_FAILED,
            Step::Share => SHARE_DENIED,
            step if step.writes() => NOT_WRITABLE,
            _ => NOT_READABLE,
        },
        ErrorKind::NotFound if step == Step::Share => SHARE_MISSING,
        ErrorKind::NotFound => FILE_MISSING,
        ErrorKind::AlreadyExists => CHANGED,
        ErrorKind::DiskFull => QUOTA,
        ErrorKind::ConnectionLost
        | ErrorKind::TimedOut
        | ErrorKind::SessionExpired
        | ErrorKind::Io => {
            // "Offline" is a server that cannot be dialled or named, and a
            // connection that broke while it was in use (the Wi-Fi, a NAS
            // going to sleep). A server that took the connection and then
            // closed it or fell silent before the share was open, or that
            // leaves a request without an answer, is there – something is
            // wrong with it or with what was sent. (A request that could
            // not even be sent in time says nothing about the server: a
            // dead link, offline.)
            let undialled = matches!(e, smb2::Error::ConnectFailed { .. } | smb2::Error::Io(_));
            let unanswered = matches!(e, smb2::Error::Timeout);
            if step.connecting() && !undialled || !step.connecting() && unanswered {
                NO_ANSWER
            } else {
                OFFLINE
            }
        }
        ErrorKind::InvalidData => BAD_RESPONSE,
        _ => SERVER_ERROR,
    }
}

/// The code of a failed login step (dial, negotiation, login) after
/// `elapsed`: [`code`], except for the name lookup running out of time.
/// smb2 reports that with the same error as a request without an answer
/// (`Error::Timeout`), but within the dial's budget ([`CONNECT_TIMEOUT`]) –
/// an unanswered request takes it at least [`ANSWER_TIMEOUT`] – and it is a
/// server that cannot be named: offline.
fn login_code(e: &smb2::Error, elapsed: Duration) -> &'static str {
    if matches!(e, smb2::Error::Timeout) && elapsed < ANSWER_TIMEOUT {
        OFFLINE
    } else {
        code(e, Step::Login)
    }
}

/// What failed, for the log and the form test's small print: the step,
/// smb2's kind and – where there is one – the command and the server's
/// status. Never the error's own text: it can name the user, the host or a
/// path.
fn detail(step: Step, e: &smb2::Error) -> String {
    let more = match e {
        smb2::Error::Protocol {
            status, command, ..
        } => format!(" ({command:?} {status})"),
        smb2::Error::ConnectFailed { attempts, .. } => {
            let tried: Vec<String> = attempts
                .iter()
                .map(|attempt| match attempt.error_kind {
                    Some(kind) => format!("{kind:?}"),
                    None => "no answer".to_owned(),
                })
                .collect();
            format!(" (TCP: {})", tried.join(", "))
        }
        smb2::Error::Io(err) => format!(" ({:?})", err.kind()),
        smb2::Error::SendFailed {
            command, source, ..
        } => format!(" ({command:?}, {:?})", source.kind()),
        smb2::Error::SendTimeout { command, .. }
        | smb2::Error::SignatureInvalid { command, .. } => format!(" ({command:?})"),
        _ => String::new(),
    };
    format!("{}: {:?}{more}", step.name(), e.kind())
}

/// A connection should be made again after this error.
fn drops_connection(e: &smb2::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::ConnectionLost | ErrorKind::TimedOut | ErrorKind::SessionExpired | ErrorKind::Io
    )
}

/// An open connection to the share.
struct Conn {
    client: SmbClient,
    tree: Tree,
}

/// The share's folder for a sync (see the module docs).
pub struct SmbRemote {
    share: SyncSmb,
    password: Zeroizing<String>,
    runtime: tokio::runtime::Runtime,
    conn: Mutex<Option<Conn>>,
    /// The last failure's [`detail`] (the form's test shows it).
    detail: Mutex<Option<String>>,
}

impl std::fmt::Debug for SmbRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbRemote")
            .field("share", &self.share.label())
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A request's time: the base plus its bytes at the minimum rate.
fn limit(bytes: u64) -> Duration {
    REQUEST_BASE + Duration::from_secs(bytes / MIN_RATE)
}

/// The ETag of a file: its change time and size.
fn etag(changed: u64, size: u64) -> String {
    format!("{changed:x}-{size:x}")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
}

impl SmbRemote {
    pub fn new(share: &SyncSmb, password: Zeroizing<String>) -> AppResult<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(AppError::from)?;
        Ok(SmbRemote {
            share: share.clone(),
            password,
            runtime,
            conn: Mutex::new(None),
            detail: Mutex::new(None),
        })
    }

    /// Notes a failure: a line in the log (logcat on the phone, tag
    /// `RustStdoutStderr`) and the detail the form's test shows.
    fn note(&self, detail: String) {
        log(format_args!("smb {detail}"));
        *lock(&self.detail) = Some(detail);
    }

    /// An SMB error at `step` as the app's error ([`code`]), noted.
    fn failed(&self, step: Step, e: &smb2::Error) -> AppError {
        self.failed_as(step, e, code(e, step))
    }

    /// [`Self::failed`] with the code given. Noted – except a missing file
    /// or name, which the callers expect and handle (only kept as the
    /// detail, not logged).
    fn failed_as(&self, step: Step, e: &smb2::Error, code: &'static str) -> AppError {
        let expected = !step.connecting()
            && matches!(e.kind(), ErrorKind::NotFound | ErrorKind::AlreadyExists);
        if expected {
            *lock(&self.detail) = Some(detail(step, e));
        } else {
            self.note(detail(step, e));
        }
        AppError::unsupported(code)
    }

    /// The last failure's detail (taken).
    fn take_detail(&self) -> Option<String> {
        lock(&self.detail).take()
    }

    /// The share path of `rel` (a path of the layout, `""` = the folder).
    fn path(&self, rel: &str) -> String {
        match (self.share.folder.is_empty(), rel.is_empty()) {
            (true, _) => rel.to_owned(),
            (false, true) => self.share.folder.clone(),
            (false, false) => format!("{}/{rel}", self.share.folder),
        }
    }

    async fn connect(&self) -> AppResult<Conn> {
        let config = ClientConfig {
            addr: self.share.addr(),
            timeout: CONNECT_TIMEOUT,
            username: self.share.user.clone(),
            password: self.password.as_str().to_owned(),
            domain: self.share.domain.clone(),
            auto_reconnect: false,
            ..Default::default()
        };
        let started = Instant::now();
        let connecting = async {
            let mut client = SmbClient::connect(config)
                .await
                .map_err(|e| self.failed_as(Step::Login, &e, login_code(&e, started.elapsed())))?;
            let tree = client
                .connect_share(&self.share.share)
                .await
                .map_err(|e| self.failed(Step::Share, &e))?;
            Ok(Conn { client, tree })
        };
        match tokio::time::timeout(CONNECT_LIMIT, connecting).await {
            Ok(connected) => connected,
            // A server that cannot be dialled fails earlier
            // ([`CONNECT_TIMEOUT`]): this one took the connection and did
            // not get through the login and the share in time.
            Err(_) => {
                self.note(format!(
                    "login: no answer within {} s",
                    CONNECT_LIMIT.as_secs()
                ));
                Err(AppError::unsupported(NO_ANSWER))
            }
        }
    }

    /// Runs `f` on the connection (made first if there is none) within the
    /// time of a request of `bytes`; an SMB error becomes its code at
    /// `step` ([`code`]) and is noted, a lost connection is dropped.
    fn run<T>(
        &self,
        step: Step,
        bytes: u64,
        f: impl AsyncFnOnce(&mut Conn) -> smb2::Result<T>,
    ) -> AppResult<T> {
        let mut guard = lock(&self.conn);
        self.runtime.block_on(async {
            if guard.is_none() {
                *guard = Some(self.connect().await?);
            }
            let outcome = {
                let conn = guard.as_mut().expect("connected above");
                tokio::time::timeout(limit(bytes), f(conn)).await
            };
            match outcome {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(e)) => {
                    if drops_connection(&e) {
                        *guard = None;
                    }
                    Err(self.failed(step, &e))
                }
                // The app's own limit for a request ran out on a
                // connection that stood: no answer, not "offline".
                Err(_) => {
                    *guard = None;
                    self.note(format!(
                        "{}: no answer within {} s",
                        step.name(),
                        limit(bytes).as_secs()
                    ));
                    Err(AppError::unsupported(NO_ANSWER))
                }
            }
        })
    }

    /// The ETag and size of `path` (a share path) and whether it is a
    /// folder; `None` if there is none.
    fn stat(&self, path: &str) -> AppResult<Option<(String, u64, bool)>> {
        self.run(Step::Stat, 0, async |c| {
            match c.client.stat(&mut c.tree, path).await {
                Ok(info) => Ok(Some((
                    etag(info.changed.0, info.size),
                    info.size,
                    info.is_directory,
                ))),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        })
    }

    /// Makes the folder below the share, segment by segment ("Ordner
    /// anlegen"; never done by a sync).
    pub fn create_folder(&self) -> AppResult<()> {
        let mut path = String::new();
        for segment in self.share.folder.split('/').filter(|s| !s.is_empty()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            let dir = path.clone();
            let existed = self.run(Step::Mkdir, 0, async |c| {
                match c.client.create_directory(&c.tree, &dir).await {
                    Ok(()) => Ok(false),
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(true),
                    Err(e) => Err(e),
                }
            })?;
            // The name is taken – by a folder, or by a file in the way. Only
            // the proof of a file stops here: a folder whose attributes the
            // account may not read is still a folder to go through, and
            // anything else shows at the next segment or in the test.
            if existed && matches!(self.stat(&path), Ok(Some((_, _, false)))) {
                return Err(AppError::unsupported(NOT_A_FOLDER));
            }
        }
        Ok(())
    }

    /// Removes a file a failed write may have left behind (best effort):
    /// only while the connection still stands – not worth a new dial –, and
    /// the failure that led here stays the noted one.
    fn remove_leftover(&self, path: &str) {
        let standing = lock(&self.conn).is_some();
        if !standing {
            return;
        }
        let noted = self.take_detail();
        let _ = self.run(Step::Delete, 0, async |c| {
            c.client.delete_file(&c.tree, path).await
        });
        *lock(&self.detail) = noted;
    }

    /// The connection test of the form: share, login, folder, write access
    /// (a test file is made and removed). A failed request carries its
    /// [`SmbTest::detail`].
    pub fn test(cfg: &SmbConfig) -> SmbTest {
        let fail = |e: AppError, detail: Option<String>| SmbTest {
            code: Some(e.code()),
            detail,
            ..SmbTest::default()
        };
        let share = match cfg.share() {
            Ok(share) => share,
            Err(e) => return fail(e, None),
        };
        let remote = match SmbRemote::new(&share, cfg.password.clone()) {
            Ok(remote) => remote,
            Err(e) => return fail(e, None),
        };
        // `None`: the folder is not there yet.
        let tested = remote.probe().and_then(|access| {
            if access.exists {
                remote.write_test().map(Some)
            } else {
                Ok(None)
            }
        });
        match tested {
            Ok(None) => SmbTest {
                folder_missing: true,
                ..SmbTest::default()
            },
            Ok(Some(writable)) => SmbTest {
                ok: true,
                writable,
                ..SmbTest::default()
            },
            Err(e) => fail(e, remote.take_detail()),
        }
    }

    /// Whether the folder can be written: a test file is made and removed
    /// again. `Ok(false)`: the server refuses the account writes there –
    /// any other failure (the connection, a full disk) is that error, not
    /// "read-only".
    fn write_test(&self) -> AppResult<bool> {
        let name = format!(
            ".keystead-write-test-{}",
            crypto::b64_encode(&crypto::random_array::<6>()?).replace(['+', '/', '='], "x")
        );
        let path = self.path(&name);
        if let Err(e) = self.run(Step::Write, 4, async |c| {
            c.client.write_file(&c.tree, &path, b"test").await
        }) {
            // The server may have made the file before it refused the
            // write or the write failed: not left behind.
            self.remove_leftover(&path);
            return if super::has_code(&e, NOT_WRITABLE) {
                Ok(false)
            } else {
                Err(e)
            };
        }
        // The write proved the access; removing the file is best effort
        // (its failure is noted in the log).
        let _ = self.run(Step::Delete, 0, async |c| {
            c.client.delete_file(&c.tree, &path).await
        });
        Ok(true)
    }

    /// A temporary name next to `path` (a share path).
    fn temp_of(path: &str) -> AppResult<String> {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let tag = crypto::b64_encode(&crypto::random_array::<6>()?).replace(['+', '/', '='], "x");
        Ok(if dir.is_empty() {
            format!(".{name}.{tag}.tmp")
        } else {
            format!("{dir}/.{name}.{tag}.tmp")
        })
    }
}

impl Remote for SmbRemote {
    fn key(&self) -> String {
        self.share.key()
    }

    fn probe(&self) -> AppResult<Access> {
        let folder = self.path("");
        // `None`: the path names a file ("Ordner anlegen" could not help).
        let found = self.run(Step::List, 0, async |c| {
            match c.client.list_directory(&mut c.tree, &folder).await {
                Ok(_) => Ok(Some(true)),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(Some(false)),
                Err(e) if e.kind() == ErrorKind::NotADirectory => Ok(None),
                Err(e) => Err(e),
            }
        })?;
        let exists = found.ok_or_else(|| AppError::unsupported(NOT_A_FOLDER))?;
        Ok(Access {
            exists,
            writable: true,
            cas: false,
        })
    }

    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>> {
        check_dir(dir)?;
        let path = self.path(dir);
        let entries = self
            .run(Step::List, 0, async |c| {
                c.client.list_directory(&mut c.tree, &path).await
            })
            .map_err(|e| {
                if dir.is_empty() && super::has_code(&e, FILE_MISSING) {
                    AppError::unsupported(FOLDER_MISSING)
                } else {
                    e
                }
            })?;
        if entries.len() > LIST_ENTRIES_MAX {
            return Err(AppError::unsupported(BAD_RESPONSE));
        }
        Ok(entries
            .into_iter()
            .filter(|e| valid_name(&e.name))
            .map(|e| RemoteMeta {
                dir: e.is_directory,
                size: (!e.is_directory).then_some(e.size),
                etag: Some(etag(e.changed.0, e.size)),
                modified: None,
                name: e.name,
            })
            .collect())
    }

    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)> {
        check_path(path)?;
        let share_path = self.path(path);
        let Some((before, size, false)) = self.stat(&share_path)? else {
            return Err(AppError::unsupported(FILE_MISSING));
        };
        if size > max {
            return Err(AppError::unsupported(TOO_LARGE));
        }
        let bytes = self.run(Step::Read, size, async |c| {
            c.client.read_file_pipelined(&mut c.tree, &share_path).await
        })?;
        if bytes.len() as u64 > max {
            return Err(AppError::unsupported(TOO_LARGE));
        }
        // Written meanwhile: the bytes may be of either version.
        let after = self.stat(&share_path)?.map(|(etag, _, _)| etag);
        if after.as_deref() != Some(before.as_str()) {
            return Err(AppError::unsupported(CHANGED));
        }
        Ok((bytes, Some(before)))
    }

    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta> {
        check_path(path)?;
        let share_path = self.path(path);
        if let Some((dir, _)) = path.rsplit_once('/') {
            self.mkcol(dir)?;
        }
        let current = self.stat(&share_path)?;
        match (&pre, &current) {
            (Precondition::IfNoneMatch, Some(_)) => return Err(AppError::unsupported(CHANGED)),
            (Precondition::IfMatch(_), None) => return Err(AppError::unsupported(CHANGED)),
            (Precondition::IfMatch(want), Some((have, _, _))) if want != have => {
                return Err(AppError::unsupported(CHANGED))
            }
            _ => {}
        }
        let temp = Self::temp_of(&share_path)?;
        let written = match self.run(Step::Write, bytes.len() as u64, async |c| {
            c.client.write_file(&c.tree, &temp, bytes).await
        }) {
            Ok(written) => written,
            // The server may have made the temp file before the write
            // failed (a full volume): not left behind while it can be
            // removed. The copy in the folder is untouched.
            Err(e) => {
                self.remove_leftover(&temp);
                return Err(e);
            }
        };
        // Not all of it arrived: half a vault must never replace the copy.
        if written != bytes.len() as u64 {
            self.note(format!("write: {written} of {} bytes written", bytes.len()));
            self.remove_leftover(&temp);
            return Err(AppError::unsupported(SERVER_ERROR));
        }
        let replace = pre != Precondition::IfNoneMatch;
        let renamed = self.run(Step::Rename, 0, async |c| {
            c.client
                .rename_with(
                    &c.tree,
                    &temp,
                    &share_path,
                    RenameOptions::new().replace_if_exists(replace),
                )
                .await
        });
        if let Err(e) = renamed {
            self.remove_leftover(&temp);
            return Err(e);
        }
        let (etag, size, _) = self
            .stat(&share_path)?
            .ok_or_else(|| AppError::unsupported(CHANGED))?;
        Ok(RemoteMeta {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            dir: false,
            size: Some(size),
            etag: Some(etag),
            modified: None,
        })
    }

    fn mkcol(&self, dir: &str) -> AppResult<()> {
        check_dir(dir)?;
        if dir.is_empty() {
            return Err(AppError::invalid(super::BAD_PATH));
        }
        let path = self.path(dir);
        self.run(Step::Mkdir, 0, async |c| {
            match c.client.create_directory(&c.tree, &path).await {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
                other => other,
            }
        })
    }

    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()> {
        check_path(path)?;
        let share_path = self.path(path);
        if super::blob_dir_id(path).is_some() {
            for entry in self.list(path)? {
                if !entry.dir {
                    let file = format!("{share_path}/{}", entry.name);
                    self.run(Step::Delete, 0, async |c| {
                        c.client.delete_file(&c.tree, &file).await
                    })?;
                }
            }
            return self.run(Step::Delete, 0, async |c| {
                c.client.delete_directory(&c.tree, &share_path).await
            });
        }
        let Some((have, _, _)) = self.stat(&share_path)? else {
            return Err(AppError::unsupported(FILE_MISSING));
        };
        if let Precondition::IfMatch(want) = &pre {
            if *want != have {
                return Err(AppError::unsupported(CHANGED));
            }
        }
        self.run(Step::Delete, 0, async |c| {
            c.client.delete_file(&c.tree, &share_path).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(server: &str, share: &str, folder: &str) -> SmbConfig {
        SmbConfig {
            server: server.into(),
            share: share.into(),
            folder: folder.into(),
            user: "Max".into(),
            password: Zeroizing::new("pw".into()),
            domain: String::new(),
        }
    }

    #[test]
    fn the_form_is_normalized_and_checked() {
        let s = config("  \\\\NAS.local\\ ", "/Daten/", " Backups\\Keystead/ ")
            .share()
            .unwrap();
        assert_eq!(
            (s.server.as_str(), s.share.as_str(), s.folder.as_str()),
            ("NAS.local", "Daten", "Backups/Keystead")
        );
        assert_eq!(s.key(), "smb://max@nas.local/daten/backups/keystead");
        assert_eq!(s.label(), "\\\\NAS.local\\Daten\\Backups\\Keystead");
        assert_eq!(s.addr(), "NAS.local:445");
        assert_eq!(
            config("smb://192.168.1.10:4455", "x", "")
                .share()
                .unwrap()
                .addr(),
            "192.168.1.10:4455"
        );
        assert_eq!(
            config("fe80::1", "x", "").share().unwrap().addr(),
            "[fe80::1]:445"
        );
        assert_eq!(
            config("[fe80::1]:446", "x", "").share().unwrap().addr(),
            "[fe80::1]:446"
        );
        assert_eq!(config("nas", "x", "").share().unwrap().folder, "");
        assert_eq!(
            config(
                "192.168.2.201",
                "personal_folder",
                "Personal Data/VaultSync"
            )
            .share()
            .unwrap()
            .label(),
            "\\\\192.168.2.201\\personal_folder\\Personal Data\\VaultSync"
        );
        for (server, share, folder) in [
            ("", "x", ""),
            ("nas host", "x", ""),
            ("user@nas", "x", ""),
            ("nas/x", "x", ""),
            ("Z:", "Personal Data", "VaultSync"),
            ("nas:", "x", ""),
            ("nas:0", "x", ""),
            ("nas:99999", "x", ""),
            ("nas:44a", "x", ""),
            ("[fe80::1]:", "x", ""),
            ("nas", "", ""),
            ("nas", "a:b", ""),
            ("nas", "x", "a/../b"),
            ("nas", "x", "a/./b"),
            ("nas", "x", "a*b"),
            // SMB cannot carry a name that ends in a dot as it is.
            ("nas", "x", "Backup."),
            ("nas", "x", "a/b./c"),
        ] {
            assert_eq!(
                config(server, share, folder).share().unwrap_err().code(),
                "invalid_input:smb_config",
                "{server} {share} {folder}"
            );
        }
        // A name with a letter of more than one byte where a prefix would
        // end is a name like any other (cutting it by bytes panicked).
        for (typed, server) in [
            ("Büro-PC", "Büro-PC"),
            ("\\\\Büro-PC\\", "Büro-PC"),
            ("nas-büro", "nas-büro"),
            ("smb://Büro-PC", "Büro-PC"),
            ("ünas", "ünas"),
        ] {
            assert_eq!(config(typed, "x", "").share().unwrap().server, server);
        }
    }

    #[test]
    fn a_windows_style_login_is_split_into_domain_and_user() {
        let login = |user: &str, domain: &str| {
            let mut cfg = config("nas", "x", "");
            cfg.user = user.into();
            cfg.domain = domain.into();
            let s = cfg.share().unwrap();
            (s.user, s.domain)
        };
        let pair = |user: &str, domain: &str| (user.to_owned(), domain.to_owned());
        assert_eq!(login("NAS\\max", ""), pair("max", "NAS"));
        assert_eq!(login(" .\\max ", ""), pair("max", ""));
        assert_eq!(login("Cedrick Grabe", ""), pair("Cedrick Grabe", ""));
        assert_eq!(login("max@example.org", ""), pair("max@example.org", ""));
        // A domain of its own wins; odd names stay as typed.
        assert_eq!(login("NAS\\max", "FIRMA"), pair("NAS\\max", "FIRMA"));
        assert_eq!(login("a\\b\\c", ""), pair("a\\b\\c", ""));
        assert_eq!(login("NAS\\", ""), pair("NAS\\", ""));
        // The key follows the split (the password is stored under it).
        let mut cfg = config("nas", "x", "");
        cfg.user = "NAS\\Max".into();
        assert_eq!(cfg.share().unwrap().key(), "smb://max@nas/x/");
    }

    #[test]
    fn errors_keep_their_step_and_reason() {
        use smb2::types::status::NtStatus;
        use smb2::types::Command;
        let status = |status, command| smb2::Error::Protocol { status, command };
        let denied = |command| status(NtStatus::ACCESS_DENIED, command);

        // "Access denied" is a wrong password only at the login.
        assert_eq!(
            code(&denied(Command::SessionSetup), Step::Login),
            AUTH_FAILED
        );
        assert_eq!(
            code(&denied(Command::TreeConnect), Step::Share),
            SHARE_DENIED
        );
        for step in [Step::List, Step::Stat, Step::Read] {
            assert_eq!(code(&denied(Command::Create), step), NOT_READABLE);
        }
        for step in [Step::Write, Step::Rename, Step::Mkdir, Step::Delete] {
            assert_eq!(code(&denied(Command::Create), step), NOT_WRITABLE);
        }
        let logon = status(NtStatus::LOGON_FAILURE, Command::SessionSetup);
        assert_eq!(code(&logon, Step::Login), AUTH_FAILED);
        assert_eq!(
            detail(Step::Login, &logon),
            "login: AuthRequired (SessionSetup STATUS_LOGON_FAILURE)"
        );
        let no_share = status(NtStatus::BAD_NETWORK_NAME, Command::TreeConnect);
        assert_eq!(code(&no_share, Step::Share), SHARE_MISSING);
        let no_file = status(NtStatus::OBJECT_NAME_NOT_FOUND, Command::Create);
        assert_eq!(code(&no_file, Step::Stat), FILE_MISSING);
        assert_eq!(
            code(&status(NtStatus::DISK_FULL, Command::Write), Step::Write),
            QUOTA
        );

        // "Offline" is only a server that cannot be dialled or named …
        let refused = smb2::Error::ConnectFailed {
            host: "nas".into(),
            attempts: vec![smb2::transport::ConnectAttempt {
                addr: "192.168.1.10:445".parse().unwrap(),
                error_kind: Some(std::io::ErrorKind::ConnectionRefused),
            }],
        };
        assert_eq!(code(&refused, Step::Login), OFFLINE);
        // … and the detail names neither the host nor the address.
        assert_eq!(
            detail(Step::Login, &refused),
            "login: ConnectionLost (TCP: ConnectionRefused)"
        );
        let unnamed = smb2::Error::Io(std::io::Error::other("lookup"));
        assert_eq!(code(&unnamed, Step::Login), OFFLINE);
        // … one that took the connection and then closed it or fell silent
        // before the share was open is there:
        for e in [smb2::Error::Disconnected, smb2::Error::Timeout] {
            assert_eq!(code(&e, Step::Login), NO_ANSWER, "{e}");
            assert_eq!(code(&e, Step::Share), NO_ANSWER, "{e}");
        }
        // Later a connection that broke is one that was lost (the Wi-Fi, a
        // NAS going to sleep) – the sync ends and tries again –, and a
        // request without an answer is still no "offline".
        for step in [Step::List, Step::Read, Step::Write] {
            assert_eq!(code(&smb2::Error::Disconnected, step), OFFLINE);
            assert_eq!(code(&smb2::Error::Timeout, step), NO_ANSWER);
        }
        // The name lookup running out of time looks like an unanswered
        // request, but comes within the dial's budget: no such server.
        let secs = Duration::from_secs;
        assert_eq!(login_code(&smb2::Error::Timeout, secs(15)), OFFLINE);
        assert_eq!(login_code(&smb2::Error::Timeout, secs(31)), NO_ANSWER);
        assert_eq!(login_code(&smb2::Error::Disconnected, secs(1)), NO_ANSWER);
        assert_eq!(login_code(&refused, secs(1)), OFFLINE);
        assert_eq!(login_code(&logon, secs(1)), AUTH_FAILED);
        assert_eq!(
            detail(Step::Share, &smb2::Error::Disconnected),
            "share: ConnectionLost"
        );
        // A guest answer to a named login names no user in the detail.
        let guest = smb2::Error::GuestSessionRefused {
            username: "Max".into(),
            anonymous: false,
        };
        assert_eq!(code(&guest, Step::Login), AUTH_FAILED);
        assert_eq!(detail(Step::Login, &guest), "login: GuestSessionRefused");
    }

    #[test]
    fn a_server_that_hangs_up_is_not_offline() {
        // Something listens and takes the connection, then closes it at
        // once – no SMB server, but not "unreachable" either.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepting = std::thread::spawn(move || {
            for stream in listener.incoming().take(1) {
                drop(stream);
            }
        });
        let test = SmbRemote::test(&config(&format!("127.0.0.1:{port}"), "x", ""));
        accepting.join().unwrap();
        assert!(!test.ok);
        assert_eq!(test.code.as_deref(), Some("unsupported:smb_no_answer"));
        let detail = test.detail.expect("a detail");
        assert!(detail.starts_with("login: "), "{detail}");
        assert!(!detail.contains("127.0.0.1"), "{detail}");
    }

    #[test]
    fn paths_below_the_folder_and_temporary_names() {
        let remote = SmbRemote::new(
            &config("nas", "x", "a/b").share().unwrap(),
            Zeroizing::default(),
        )
        .unwrap();
        assert_eq!(remote.path(""), "a/b");
        assert_eq!(remote.path("v.keystead"), "a/b/v.keystead");
        let top = SmbRemote::new(
            &config("nas", "x", "").share().unwrap(),
            Zeroizing::default(),
        )
        .unwrap();
        assert_eq!(top.path(""), "");
        assert_eq!(top.path("v.keystead"), "v.keystead");
        let temp = SmbRemote::temp_of("a/b/v.keystead").unwrap();
        assert!(
            temp.starts_with("a/b/.v.keystead.") && temp.ends_with(".tmp"),
            "{temp}"
        );
        assert!(!format!("{remote:?}").contains("pw"));
    }

    #[test]
    fn an_unreachable_server_is_offline() {
        // A closed local port: the connection is refused at once.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let test = SmbRemote::test(&config(&format!("127.0.0.1:{port}"), "x", ""));
        assert!(!test.ok);
        assert_eq!(test.code.as_deref(), Some("unsupported:sync_offline"));
        // The detail says what the dial did – without the address.
        let detail = test.detail.expect("a detail");
        assert!(
            detail.starts_with("login: ConnectionLost (TCP: "),
            "{detail}"
        );
        assert!(!detail.contains("127.0.0.1"), "{detail}");
    }

    /// Against a real share: `KEYSTEAD_TEST_SMB_SERVER`, `_SHARE`, `_USER`,
    /// `_PASSWORD` (and `_FOLDER`, default `keystead-test`).
    #[test]
    #[ignore]
    fn against_a_real_share() {
        let env = |k: &str| std::env::var(format!("KEYSTEAD_TEST_SMB_{k}")).unwrap_or_default();
        let mut cfg = config(&env("SERVER"), &env("SHARE"), &env("FOLDER"));
        if cfg.folder.is_empty() {
            cfg.folder = "keystead-test".into();
        }
        cfg.user = env("USER");
        cfg.password = Zeroizing::new(env("PASSWORD"));
        let share = cfg.share().unwrap();
        let remote = SmbRemote::new(&share, cfg.password.clone()).unwrap();
        remote.create_folder().unwrap();
        let test = SmbRemote::test(&cfg);
        assert!(test.ok && test.writable, "{test:?}");
        let id = "0f5b2c1e-aaaa-4bbb-8ccc-1234567890ab";
        let vault = format!("{id}.keystead");
        let _ = remote.delete(&vault, Precondition::None);
        let first = remote
            .put(&vault, b"v1", Precondition::IfNoneMatch)
            .unwrap();
        assert!(super::super::has_code(
            &remote
                .put(&vault, b"x", Precondition::IfNoneMatch)
                .unwrap_err(),
            CHANGED
        ));
        let (bytes, tag) = remote.get(&vault, 100).unwrap();
        assert_eq!(
            (bytes.as_slice(), tag),
            (b"v1".as_slice(), first.etag.clone())
        );
        let second = remote
            .put(
                &vault,
                b"v2",
                Precondition::IfMatch(first.etag.clone().unwrap()),
            )
            .unwrap();
        assert!(super::super::has_code(
            &remote
                .put(&vault, b"v3", Precondition::IfMatch(first.etag.unwrap()))
                .unwrap_err(),
            CHANGED
        ));
        assert_eq!(
            remote
                .list("")
                .unwrap()
                .iter()
                .filter(|m| m.name == vault)
                .count(),
            1
        );
        let blobs = crate::remote::blob_dir_name(id);
        remote
            .put(&format!("{blobs}/{id}.bin"), b"blob", Precondition::None)
            .unwrap();
        assert_eq!(remote.list(&blobs).unwrap().len(), 1);
        remote.delete(&blobs, Precondition::None).unwrap();
        remote
            .delete(&vault, Precondition::IfMatch(second.etag.unwrap()))
            .unwrap();
        assert!(super::super::has_code(
            &remote.get(&vault, 10).unwrap_err(),
            FILE_MISSING
        ));
    }
}
