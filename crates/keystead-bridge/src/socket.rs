//! The local socket between native host and app (see "Local socket" in
//! docs/ARCHITECTURE.md).
//!
//! * Windows: named pipe `\\.\pipe\keystead-bridge-<USERNAME>`. The pipe gets a
//!   security descriptor whose owner is the current user's SID and whose DACL
//!   grants access to that SID only (best effort: if the SID cannot be
//!   determined, the default pipe security applies). Remote (network) clients
//!   are always rejected, and the first-instance flag makes creation fail if
//!   anyone else already owns the name. The pipe namespace is shared by all
//!   users of the machine and the name is predictable, so another user could
//!   create it first ("pipe squatting"): clients therefore check the owner of
//!   the pipe they connected to *before sending anything* and refuse pipes
//!   owned by anyone else (`PermissionDenied`); a server that finds the name
//!   taken does the same check to tell its own second instance
//!   (`AlreadyRunning`) from a hijacked name (`PermissionDenied`).
//! * Unix: `$XDG_RUNTIME_DIR/keystead-bridge.sock`, fallback
//!   `/tmp/keystead-bridge-<uid>.sock`, mode 0600. Both ends additionally
//!   check that the peer runs as the same user (`SO_PEERCRED`).
//! * `$KEYSTEAD_BRIDGE_SOCKET` overrides the endpoint (tests): a socket path on
//!   Unix, a pipe name (with or without `\\.\pipe\`) on Windows.

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    ConnectOptions, GenericFilePath, GenericNamespaced, Listener, ListenerOptions, Name, Stream,
};
use interprocess::ConnectWaitMode;

use crate::error::{Error, Result};
use crate::framing::{self, MAX_MESSAGE_SIZE};
use crate::protocol::{Request, Response};

/// Environment variable overriding the bridge endpoint.
pub const SOCKET_ENV: &str = "KEYSTEAD_BRIDGE_SOCKET";
/// Prefix of the Windows pipe name; the user name is appended.
pub const PIPE_NAME_PREFIX: &str = "keystead-bridge-";
/// File name of the Unix socket inside `$XDG_RUNTIME_DIR`.
pub const UNIX_SOCKET_NAME: &str = "keystead-bridge.sock";

/// How long a connection attempt may wait for a busy listener.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Where the bridge server listens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// A Unix domain socket file (on Windows: a `\\.\pipe\...` path).
    Path(PathBuf),
    /// A namespaced name: a named pipe name without the `\\.\pipe\` prefix
    /// on Windows (on Linux an abstract socket, which is never used by
    /// default because it has no file permissions).
    Namespaced(String),
}

impl Endpoint {
    /// `$KEYSTEAD_BRIDGE_SOCKET` if set, otherwise [`Endpoint::default_for_user`].
    pub fn resolve() -> Endpoint {
        match std::env::var_os(SOCKET_ENV) {
            Some(value) if !value.is_empty() => Endpoint::from_override(&value),
            _ => Endpoint::default_for_user(),
        }
    }

    /// Interprets an override value: a socket path on Unix, a pipe name
    /// (optionally with the `\\.\pipe\` prefix) on Windows.
    pub fn from_override(value: &OsStr) -> Endpoint {
        #[cfg(windows)]
        {
            let s = value.to_string_lossy();
            let name = strip_pipe_prefix(&s);
            Endpoint::Namespaced(name.to_owned())
        }
        #[cfg(not(windows))]
        {
            Endpoint::Path(PathBuf::from(value))
        }
    }

    /// The per-user default endpoint of this platform.
    pub fn default_for_user() -> Endpoint {
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap_or_default();
            Endpoint::Namespaced(format!("{PIPE_NAME_PREFIX}{}", sanitize_pipe_part(&user)))
        }
        #[cfg(unix)]
        {
            Endpoint::Path(default_unix_path(
                std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
                current_euid(),
            ))
        }
    }

    pub(crate) fn name(&self) -> io::Result<Name<'_>> {
        match self {
            Endpoint::Path(path) => path.as_path().to_fs_name::<GenericFilePath>(),
            Endpoint::Namespaced(name) => name.as_str().to_ns_name::<GenericNamespaced>(),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Path(path) => write!(f, "{}", path.display()),
            Endpoint::Namespaced(name) if cfg!(windows) => write!(f, r"\\.\pipe\{name}"),
            Endpoint::Namespaced(name) => write!(f, "@{name}"),
        }
    }
}

#[cfg(any(windows, test))]
fn strip_pipe_prefix(s: &str) -> &str {
    const PREFIX: &str = r"\\.\pipe\";
    match s.get(..PREFIX.len()) {
        Some(head) if head.eq_ignore_ascii_case(PREFIX) => &s[PREFIX.len()..],
        _ => s,
    }
}

/// Pipe names may contain anything but backslashes; control characters are
/// replaced as well to keep the name printable.
#[cfg(any(windows, test))]
fn sanitize_pipe_part(s: &str) -> String {
    let cleaned: String = s
        .trim()
        .chars()
        .map(|c| {
            if c == '\\' || c == '/' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "default".to_owned()
    } else {
        cleaned
    }
}

/// `$XDG_RUNTIME_DIR/keystead-bridge.sock` if that directory is usable and the
/// path fits into `sockaddr_un`, else `/tmp/keystead-bridge-<uid>.sock`.
#[cfg(unix)]
fn default_unix_path(runtime_dir: Option<PathBuf>, uid: u32) -> PathBuf {
    // sun_path is 104 (macOS) to 108 (Linux) bytes including the NUL.
    const MAX_SOCKET_PATH: usize = 100;
    if let Some(dir) = runtime_dir.filter(|d| d.is_absolute() && d.is_dir()) {
        let path = dir.join(UNIX_SOCKET_NAME);
        if path.as_os_str().len() <= MAX_SOCKET_PATH {
            return path;
        }
    }
    PathBuf::from(format!("/tmp/keystead-bridge-{uid}.sock"))
}

#[cfg(unix)]
pub(crate) fn current_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

// ---------------------------------------------------------------------------
// Client side
// ---------------------------------------------------------------------------

/// Connects to the bridge server at `endpoint`. The connection is refused
/// (`PermissionDenied`) unless the server provably runs as the current user
/// (Unix: peer uid; Windows: owner of the named pipe). The check happens
/// before anything is sent, so a server squatting on the endpoint never sees
/// a request (and, on Windows, cannot impersonate us: that needs data read
/// from the pipe first).
pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    connect_verified(endpoint, CONNECT_TIMEOUT, server_is_current_user)
}

/// [`connect`] with an explicit server check (`verify`); any error of the
/// check counts as "not ours" (fail closed).
fn connect_verified(
    endpoint: &Endpoint,
    timeout: Duration,
    verify: impl FnOnce(&Stream) -> io::Result<bool>,
) -> io::Result<Stream> {
    let stream = connect_unverified(endpoint, timeout)?;
    match verify(&stream) {
        Ok(true) => Ok(stream),
        Ok(false) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the bridge endpoint {endpoint} is served by another user"),
        )),
        Err(e) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("could not verify who serves the bridge endpoint {endpoint}: {e}"),
        )),
    }
}

fn connect_unverified(endpoint: &Endpoint, timeout: Duration) -> io::Result<Stream> {
    ConnectOptions::new()
        .name(endpoint.name()?)
        .wait_mode(ConnectWaitMode::Timeout(timeout))
        .connect_sync()
}

/// Client side: true if the *server* end of `stream` belongs to the current
/// user. Unix: the peer's effective uid (where the OS cannot tell, the
/// socket file's mode 0600 is relied upon). Windows: the owner SID of the
/// named pipe must be the current user's SID; any API failure is an error
/// (callers treat it as "not ours").
pub(crate) fn server_is_current_user(stream: &Stream) -> io::Result<bool> {
    #[cfg(unix)]
    {
        Ok(peer_uid_is_current_user(stream))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsHandle, AsRawHandle};
        let Stream::NamedPipe(inner) = stream;
        win::pipe_owner_is_current_user(inner.as_handle().as_raw_handle().cast())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = stream;
        Ok(true)
    }
}

/// Server side: true if the *client* of an accepted connection runs as the
/// current user. Unix: the peer's effective uid (where the OS cannot tell,
/// the socket file's mode 0600 is relied upon). Windows: the pipe's DACL
/// already grants access to the current user's SID only (with the default
/// security of the fallback path other users get read access at most, which
/// cannot send a request), so every accepted client is the current user.
pub(crate) fn client_is_current_user(stream: &Stream) -> bool {
    #[cfg(unix)]
    {
        peer_uid_is_current_user(stream)
    }
    #[cfg(not(unix))]
    {
        let _ = stream;
        true
    }
}

#[cfg(unix)]
fn peer_uid_is_current_user(stream: &Stream) -> bool {
    match stream.peer_creds() {
        Ok(creds) => creds.euid().is_none_or(|uid| uid == current_euid()),
        Err(_) => true,
    }
}

/// True if the peer of `stream` has closed its end. Never blocks and never
/// consumes data (unread bytes count as "still connected"). Used while a
/// request (`pair`) waits for the user and nobody reads the connection; must
/// not be called while another thread reads from `stream`.
pub(crate) fn peer_hung_up(stream: &Stream) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::{AsFd, AsRawFd};
        let Stream::UdSocket(inner) = stream;
        let fd = inner.as_fd().as_raw_fd();
        let mut byte = 0u8;
        // SAFETY: `fd` is a valid socket owned by `stream` for the duration
        // of the call; the buffer is one writable byte.
        let n = unsafe {
            libc::recv(
                fd,
                (&mut byte as *mut u8).cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        match n {
            0 => true, // orderly shutdown by the peer
            n if n > 0 => false,
            _ => !matches!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsHandle, AsRawHandle};
        use windows_sys::Win32::System::Pipes::PeekNamedPipe;
        let Stream::NamedPipe(inner) = stream;
        let handle = inner.as_handle().as_raw_handle();
        let mut available = 0u32;
        // SAFETY: the handle is a valid pipe handle owned by `stream`; all
        // optional out-pointers but `available` are null.
        let ok = unsafe {
            PeekNamedPipe(
                handle.cast(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        // Fails with ERROR_BROKEN_PIPE / ERROR_PIPE_NOT_CONNECTED once the
        // client has closed its end.
        ok == 0
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = stream;
        false
    }
}

/// A simple synchronous client: one connection, one request at a time.
/// (The native host uses its own relay; this is for tools and tests.)
#[derive(Debug)]
pub struct Client {
    stream: Stream,
}

impl Client {
    /// Connects to `endpoint` (see [`connect`]).
    pub fn connect(endpoint: &Endpoint) -> io::Result<Client> {
        Ok(Client {
            stream: connect(endpoint)?,
        })
    }

    /// Sends `request` and waits for the response.
    pub fn request(&mut self, request: &Request) -> Result<Response> {
        framing::write_json(&mut self.stream, request, MAX_MESSAGE_SIZE)?;
        let frame = framing::read_frame(&mut self.stream, MAX_MESSAGE_SIZE)?.ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the bridge server closed the connection",
            ))
        })?;
        let frame = zeroize::Zeroizing::new(frame);
        Ok(serde_json::from_slice(&frame)?)
    }
}

// ---------------------------------------------------------------------------
// Server side
// ---------------------------------------------------------------------------

/// Creates the listener. Fails with [`Error::AlreadyRunning`] if a live
/// server of the current user already listens on `endpoint`, and with a
/// `PermissionDenied` I/O error if the endpoint is held by another user
/// (possible hijack, see the module docs). A stale Unix socket file left by a
/// crashed instance is removed first.
pub(crate) fn bind(endpoint: &Endpoint) -> Result<Listener> {
    bind_with(endpoint, server_is_current_user)
}

/// [`bind`] with an explicit check of an existing server (`verify`, see
/// [`server_is_current_user`]).
fn bind_with(
    endpoint: &Endpoint,
    verify: impl Fn(&Stream) -> io::Result<bool>,
) -> Result<Listener> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Path(path) => prepare_unix_path(endpoint, path, &verify)?,
        _ => {
            if let Some(stream) = poke(endpoint) {
                return Err(occupied(endpoint, verify(&stream)));
            }
        }
    }

    create_listener(endpoint).map_err(|e| {
        if !name_taken(&e) {
            return Error::Io(e);
        }
        // Someone owns the name although nobody answered the probe above:
        // look again who it is before calling it a second instance.
        match poke(endpoint) {
            Some(stream) => occupied(endpoint, verify(&stream)),
            None if cfg!(windows) => {
                let error = io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "the bridge endpoint {endpoint} exists but its owner could not be verified (possibly another user)"
                    ),
                );
                crate::util::log(format_args!("WARNING: {error}"));
                Error::Io(error)
            }
            None => Error::AlreadyRunning(endpoint.to_string()),
        }
    })
}

/// The error for a live server found at `endpoint`, given the result of
/// checking whether it runs as the current user: our own second instance →
/// [`Error::AlreadyRunning`]; anyone else (or an unverifiable server) → a
/// `PermissionDenied` I/O error, logged loudly.
fn occupied(endpoint: &Endpoint, ours: io::Result<bool>) -> Error {
    let reason = match ours {
        Ok(true) => return Error::AlreadyRunning(endpoint.to_string()),
        Ok(false) => "is owned by another user".to_owned(),
        Err(e) => format!("is held by a server that could not be verified ({e})"),
    };
    let error = io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("the bridge endpoint {endpoint} {reason} (possible hijack); the browser bridge stays off"),
    );
    crate::util::log(format_args!("WARNING: {error}"));
    Error::Io(error)
}

fn name_taken(e: &io::Error) -> bool {
    match e.kind() {
        io::ErrorKind::AddrInUse => true,
        // CreateNamedPipe with FILE_FLAG_FIRST_PIPE_INSTANCE fails with
        // ERROR_ACCESS_DENIED if any process already owns the pipe name.
        io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy => cfg!(windows),
        _ => false,
    }
}

fn listener_options(endpoint: &Endpoint) -> io::Result<ListenerOptions<'_>> {
    // reclaim_name: the socket file is unlinked when the listener is dropped.
    Ok(ListenerOptions::new()
        .name(endpoint.name()?)
        .reclaim_name(true))
}

#[cfg(unix)]
fn create_listener(endpoint: &Endpoint) -> io::Result<Listener> {
    use interprocess::os::unix::local_socket::ListenerOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    match listener_options(endpoint)?.mode(0o600).create_sync() {
        // Platforms that cannot fchmod a socket before bind (e.g. macOS):
        // bind normally and restrict the file right after. The directory
        // ($XDG_RUNTIME_DIR is 0700) and the peer-uid check cover the gap.
        Err(e) if e.kind() == io::ErrorKind::Unsupported => {
            let listener = listener_options(endpoint)?.create_sync()?;
            if let Endpoint::Path(path) = endpoint {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(listener)
        }
        other => other,
    }
}

#[cfg(windows)]
fn create_listener(endpoint: &Endpoint) -> io::Result<Listener> {
    use interprocess::os::windows::local_socket::ListenerOptionsExt;

    let options = listener_options(endpoint)?;
    match win::current_user_only_security_descriptor() {
        Ok(sd) => options.security_descriptor(sd).create_sync(),
        Err(e) => {
            crate::util::log(format_args!(
                "could not restrict the bridge pipe to the current user ({e}); using default pipe security"
            ));
            options.create_sync()
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn create_listener(endpoint: &Endpoint) -> io::Result<Listener> {
    listener_options(endpoint)?.create_sync()
}

/// Handles an existing file at the socket path: a live server → error (see
/// [`occupied`]), a stale socket of ours → removed, anything else → error
/// (never delete files that are not our sockets).
#[cfg(unix)]
fn prepare_unix_path(
    endpoint: &Endpoint,
    path: &std::path::Path,
    verify: &dyn Fn(&Stream) -> io::Result<bool>,
) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::Io(e)),
    };
    if !meta.file_type().is_socket() {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists and is not a socket", path.display()),
        )));
    }
    if meta.uid() != current_euid() {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} belongs to another user", path.display()),
        )));
    }
    if let Some(stream) = poke(endpoint) {
        return Err(occupied(endpoint, verify(&stream)));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Opens a throwaway connection to `endpoint` without checking who serves it
/// (used to wake up a blocking `accept` when the server stops, and by
/// [`bind`], which checks the server itself). Nothing is ever sent on it.
pub(crate) fn poke(endpoint: &Endpoint) -> Option<Stream> {
    connect_unverified(endpoint, Duration::from_millis(500)).ok()
}

#[cfg(windows)]
mod win {
    //! Windows security helpers (FFI).

    use std::io;
    use std::ptr;

    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::{U16CStr, U16CString};
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, IsValidSid, TokenUser, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// A security descriptor owned by the current user's SID whose protected
    /// DACL grants full access to that SID and nobody else. The explicit
    /// owner matters: clients verify it (see `pipe_owner_is_current_user`),
    /// and the default owner of an elevated administrator's objects would be
    /// the Administrators group instead.
    pub(super) fn current_user_only_security_descriptor() -> io::Result<SecurityDescriptor> {
        let sid = CurrentUser::query()?.sid_string()?;
        let sddl = U16CString::from_str(format!("O:{sid}D:P(A;;GA;;;{sid})"))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID contains NUL"))?;
        SecurityDescriptor::deserialize(&sddl)
    }

    /// True if the named pipe behind `handle` (an open client end) is owned
    /// by the current user. Only the owner of a pipe name's first instance
    /// can create further instances, and nobody but an administrator can
    /// make another user the owner of an object, so another (non-admin) user
    /// squatting on the name always fails this check. Errors of the Win32
    /// calls are returned; callers treat them as "not ours".
    pub(super) fn pipe_owner_is_current_user(handle: HANDLE) -> io::Result<bool> {
        let user = CurrentUser::query()?;
        let mut owner: PSID = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `handle` is a valid pipe handle (opened with GENERIC_READ,
        // which includes READ_CONTROL) borrowed for the call; the out
        // pointers are valid; the unused ones are null as documented.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _descriptor = LocalBox(descriptor.cast());
        // SAFETY: `owner` is null or points into `descriptor`, which lives
        // until the end of this function; `user.sid()` points into `user`.
        let same = !owner.is_null()
            && unsafe { IsValidSid(owner) } != 0
            && unsafe { EqualSid(owner, user.sid()) } != 0;
        Ok(same)
    }

    /// Memory allocated by the system with `LocalAlloc`; freed on drop.
    struct LocalBox(*mut core::ffi::c_void);

    impl Drop for LocalBox {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer was allocated by the system with
                // LocalAlloc for us and is freed exactly once.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    struct TokenHandle(HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by OpenProcessToken and is
            // closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The `TOKEN_USER` of this process (its user SID).
    struct CurrentUser {
        /// u64 elements keep the buffer suitably aligned for TOKEN_USER.
        buf: Vec<u64>,
    }

    impl CurrentUser {
        fn query() -> io::Result<CurrentUser> {
            let mut raw: HANDLE = ptr::null_mut();
            // SAFETY: GetCurrentProcess returns a pseudo handle; `raw` is a
            // valid out pointer.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = TokenHandle(raw);

            let mut len = 0u32;
            // SAFETY: size query with a null buffer of length 0; expected to
            // fail with ERROR_INSUFFICIENT_BUFFER and report the needed size.
            unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut len) };
            if len == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: `buf` provides at least `len` writable bytes.
            let ok = unsafe {
                GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let user = CurrentUser { buf };
            // SAFETY: the SID pointer was set by GetTokenInformation.
            if user.sid().is_null() || unsafe { IsValidSid(user.sid()) } == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the process token has no valid user SID",
                ));
            }
            Ok(user)
        }

        /// The user SID; valid while `self` lives.
        fn sid(&self) -> PSID {
            // SAFETY: GetTokenInformation(TokenUser) filled the buffer with a
            // TOKEN_USER whose SID pointer points into the same buffer.
            unsafe { (*self.buf.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }

        /// The string SID (`S-1-5-21-...`).
        fn sid_string(&self) -> io::Result<String> {
            let mut wide: *mut u16 = ptr::null_mut();
            // SAFETY: `sid()` is valid while `self` lives; `wide` is a valid
            // out pointer.
            if unsafe { ConvertSidToStringSidW(self.sid(), &mut wide) } == 0 || wide.is_null() {
                return Err(io::Error::last_os_error());
            }
            let wide = LocalBox(wide.cast());
            // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string
            // allocated with LocalAlloc (freed by `wide` after copying).
            Ok(unsafe { U16CStr::from_ptr_str(wide.0.cast::<u16>()) }.to_string_lossy())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_helpers() {
        assert_eq!(strip_pipe_prefix(r"\\.\pipe\abc"), "abc");
        assert_eq!(strip_pipe_prefix(r"\\.\PIPE\abc"), "abc");
        assert_eq!(strip_pipe_prefix("abc"), "abc");
        assert_eq!(strip_pipe_prefix("ä"), "ä");
        assert_eq!(sanitize_pipe_part(r"DOM\bob"), "DOM_bob");
        assert_eq!(sanitize_pipe_part("  "), "default");
        assert_eq!(sanitize_pipe_part("Max Müller"), "Max Müller");
    }

    #[cfg(unix)]
    #[test]
    fn unix_default_path() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            default_unix_path(Some(dir.path().to_path_buf()), 1000),
            dir.path().join(UNIX_SOCKET_NAME)
        );
        assert_eq!(
            default_unix_path(None, 1000),
            PathBuf::from("/tmp/keystead-bridge-1000.sock")
        );
        assert_eq!(
            default_unix_path(Some(PathBuf::from("relative")), 7),
            PathBuf::from("/tmp/keystead-bridge-7.sock")
        );
        let long = dir.path().join("x".repeat(120));
        std::fs::create_dir(&long).unwrap();
        assert_eq!(
            default_unix_path(Some(long), 7),
            PathBuf::from("/tmp/keystead-bridge-7.sock")
        );
    }

    #[cfg(unix)]
    #[test]
    fn override_is_a_path_on_unix() {
        assert_eq!(
            Endpoint::from_override(OsStr::new("/run/x.sock")),
            Endpoint::Path(PathBuf::from("/run/x.sock"))
        );
        assert_eq!(Endpoint::Path(PathBuf::from("/a/b")).to_string(), "/a/b");
    }

    fn is_permission_denied(err: &Error) -> bool {
        matches!(err, Error::Io(e) if e.kind() == io::ErrorKind::PermissionDenied)
    }

    #[test]
    fn occupied_endpoint_errors() {
        let ep = Endpoint::Namespaced("x".into());
        assert!(matches!(occupied(&ep, Ok(true)), Error::AlreadyRunning(_)));
        let foreign = occupied(&ep, Ok(false));
        assert!(is_permission_denied(&foreign), "{foreign:?}");
        assert!(foreign.to_string().contains("another user"), "{foreign}");
        let unverified = occupied(&ep, Err(io::Error::other("api failed")));
        assert!(is_permission_denied(&unverified), "{unverified:?}");
        assert!(unverified.code().starts_with("io:"));
    }

    /// Accepts `n` connections on `listener` and returns how many bytes
    /// each client sent before closing.
    fn count_received(listener: Listener, n: usize) -> std::thread::JoinHandle<Vec<usize>> {
        std::thread::spawn(move || {
            (0..n)
                .map(|_| {
                    let mut stream = listener.accept().unwrap();
                    let mut buf = Vec::new();
                    let _ = io::Read::read_to_end(&mut stream, &mut buf);
                    buf.len()
                })
                .collect()
        })
    }

    #[cfg(unix)]
    #[test]
    fn client_refuses_unverified_servers_before_sending_anything() {
        let dir = tempfile::tempdir().unwrap();
        let ep = Endpoint::Path(dir.path().join("verify.sock"));
        let server = count_received(bind(&ep).unwrap(), 3);
        let timeout = Duration::from_secs(2);

        let err = connect_verified(&ep, timeout, |_| Ok(false)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("another user"), "{err}");
        // Fail closed: an error of the check counts as "not ours".
        let err = connect_verified(&ep, timeout, |_| Err(io::Error::other("no SID"))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        // The real check accepts our own server.
        let mut client = Client::connect(&ep).unwrap();
        framing::write_frame(&mut client.stream, b"hi", MAX_MESSAGE_SIZE).unwrap();
        drop(client);
        // The refused connections never carried a byte.
        assert_eq!(server.join().unwrap(), vec![0, 0, 6]);
    }

    #[cfg(unix)]
    #[test]
    fn bind_tells_a_second_instance_from_a_foreign_server() {
        let dir = tempfile::tempdir().unwrap();
        let ep = Endpoint::Path(dir.path().join("bind.sock"));
        let _listener = bind(&ep).unwrap();
        assert!(matches!(bind(&ep).unwrap_err(), Error::AlreadyRunning(_)));
        assert!(matches!(
            bind_with(&ep, |_| Ok(true)).unwrap_err(),
            Error::AlreadyRunning(_)
        ));
        assert!(is_permission_denied(
            &bind_with(&ep, |_| Ok(false)).unwrap_err()
        ));
        assert!(is_permission_denied(
            &bind_with(&ep, |_| Err(io::Error::other("x"))).unwrap_err()
        ));
        // The live server's socket file was left alone.
        assert!(matches!(bind(&ep).unwrap_err(), Error::AlreadyRunning(_)));
    }

    /// Namespaced endpoints (abstract sockets on Linux, named pipes on
    /// Windows) have no file permissions: the probe of `bind` decides.
    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn bind_checks_the_owner_of_a_namespaced_endpoint() {
        let ep = Endpoint::Namespaced(format!("keystead-test-{}", uuid::Uuid::new_v4()));
        let listener = bind(&ep).unwrap();
        assert!(matches!(bind(&ep).unwrap_err(), Error::AlreadyRunning(_)));
        assert!(is_permission_denied(
            &bind_with(&ep, |_| Ok(false)).unwrap_err()
        ));
        drop(listener);
    }

    /// Windows: the pipe is owned by the current user (explicit owner in
    /// the security descriptor), so our own client accepts it and the owner
    /// check itself succeeds.
    #[cfg(windows)]
    #[test]
    fn windows_client_accepts_its_own_pipe() {
        let ep = Endpoint::Namespaced(format!("keystead-test-{}", uuid::Uuid::new_v4()));
        let server = count_received(bind(&ep).unwrap(), 1);
        let stream = connect(&ep).unwrap();
        assert!(server_is_current_user(&stream).unwrap());
        drop(stream);
        assert_eq!(server.join().unwrap(), vec![0]);
    }

    #[cfg(unix)]
    #[test]
    fn bind_refuses_non_socket_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain-file");
        std::fs::write(&path, b"keep me").unwrap();
        let err = bind(&Endpoint::Path(path.clone())).unwrap_err();
        assert!(matches!(err, Error::Io(ref e) if e.kind() == io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
    }
}
