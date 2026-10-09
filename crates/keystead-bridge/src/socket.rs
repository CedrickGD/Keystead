//! The local socket between native host and app (see "Local socket" in
//! docs/ARCHITECTURE.md).
//!
//! * Windows: named pipe `\\.\pipe\keystead-bridge-<USERNAME>`. The pipe gets a
//!   DACL that grants access to the current user's SID only (best effort: if
//!   the SID cannot be determined, the default pipe security applies). Remote
//!   (network) clients are always rejected, and the first-instance flag makes
//!   creation fail if anyone else already owns the name.
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

/// Connects to the bridge server at `endpoint`. On Unix the connection is
/// refused (`PermissionDenied`) if the server runs as another user.
pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    let stream = connect_unverified(endpoint, CONNECT_TIMEOUT)?;
    if !peer_is_current_user(&stream) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the bridge endpoint {endpoint} is served by another user"),
        ));
    }
    Ok(stream)
}

fn connect_unverified(endpoint: &Endpoint, timeout: Duration) -> io::Result<Stream> {
    ConnectOptions::new()
        .name(endpoint.name()?)
        .wait_mode(ConnectWaitMode::Timeout(timeout))
        .connect_sync()
}

/// True if the peer of `stream` runs as the current user. Where the OS
/// cannot tell, the endpoint's access control (file mode / pipe DACL) is
/// relied upon and `true` is returned.
pub(crate) fn peer_is_current_user(stream: &Stream) -> bool {
    #[cfg(unix)]
    {
        match stream.peer_creds() {
            Ok(creds) => creds.euid().is_none_or(|uid| uid == current_euid()),
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = stream;
        true
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
/// server already listens on `endpoint`; a stale Unix socket file left by a
/// crashed instance is removed first.
pub(crate) fn bind(endpoint: &Endpoint) -> Result<Listener> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Path(path) => prepare_unix_path(endpoint, path)?,
        _ => {
            if poke(endpoint).is_some() {
                return Err(Error::AlreadyRunning(endpoint.to_string()));
            }
        }
    }

    create_listener(endpoint).map_err(|e| {
        if name_taken(&e) {
            Error::AlreadyRunning(endpoint.to_string())
        } else {
            Error::Io(e)
        }
    })
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

/// Handles an existing file at the socket path: a live server → error, a
/// stale socket of ours → removed, anything else → error (never delete
/// files that are not our sockets).
#[cfg(unix)]
fn prepare_unix_path(endpoint: &Endpoint, path: &std::path::Path) -> Result<()> {
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
    if poke(endpoint).is_some() {
        return Err(Error::AlreadyRunning(endpoint.to_string()));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Opens a throwaway connection to `endpoint` (used to wake up a blocking
/// `accept` when the server stops).
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
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// A security descriptor whose protected DACL grants full access to the
    /// current user's SID and nobody else.
    pub(super) fn current_user_only_security_descriptor() -> io::Result<SecurityDescriptor> {
        let sid = current_user_sid()?;
        let sddl = U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID contains NUL"))?;
        SecurityDescriptor::deserialize(&sddl)
    }

    struct TokenHandle(HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by OpenProcessToken and is
            // closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The string SID (`S-1-5-21-...`) of the user running this process.
    fn current_user_sid() -> io::Result<String> {
        let mut raw: HANDLE = ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo handle; `raw` is a valid
        // out pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = TokenHandle(raw);

        let mut len = 0u32;
        // SAFETY: size query with a null buffer of length 0; expected to fail
        // with ERROR_INSUFFICIENT_BUFFER and report the needed size.
        unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        // u64 elements keep the buffer suitably aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` provides at least `len` writable bytes.
        let ok = unsafe {
            GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation(TokenUser) filled the buffer with a
        // TOKEN_USER whose SID pointer points into the same buffer.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };

        let mut wide: *mut u16 = ptr::null_mut();
        // SAFETY: `sid` is valid while `buf` lives; `wide` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(sid, &mut wide) } == 0 || wide.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string
        // allocated with LocalAlloc, freed right after copying.
        let text = unsafe { U16CStr::from_ptr_str(wide) }.to_string_lossy();
        unsafe { LocalFree(wide.cast()) };
        Ok(text)
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
