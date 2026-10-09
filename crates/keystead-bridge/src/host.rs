//! Native messaging host mode: `Keystead.exe chrome-extension://<id>/
//! [--parent-window=N]` relays every frame from the browser (stdin) to the
//! running app over the local socket and the app's reply back (stdout).
//!
//! * One connection to the app is kept open and re-established on failure.
//! * If the app is not reachable, it is started (`<exe> --background`,
//!   detached) and the connection is retried for up to 8 s; otherwise the
//!   browser gets `{ "id", "ok": false, "error": "app_unavailable" }`.
//! * stdout carries protocol frames only; diagnostics go to stderr (shown in
//!   the browser's log). Message contents are never logged.
//! * EOF on stdin (the browser closed the port) ends the host with exit code 0.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use zeroize::Zeroizing;

use crate::error::Error;
use crate::framing::{self, MAX_BROWSER_MESSAGE_SIZE, MAX_MESSAGE_SIZE};
use crate::protocol::{extract_id, BridgeError, Response, PAIRING_TIMEOUT};
use crate::socket::{self, Endpoint};
use crate::util::log;

/// Environment variable naming the executable to launch instead of the
/// current one (tests, development builds).
pub const APP_EXE_ENV: &str = "KEYSTEAD_APP_EXE";
/// Argument that makes the app start hidden in the tray.
pub const BACKGROUND_ARG: &str = "--background";
/// How long the host keeps retrying after launching the app.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(8);
/// How long to wait for the app's reply (a `pair` request legitimately
/// blocks for up to 120 s). Only enforced where the OS supports socket
/// timeouts (Unix).
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(PAIRING_TIMEOUT.as_secs() + 30);
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
/// After a launch attempt failed, further requests within this period only
/// try to connect once instead of launching (and waiting) again.
const RELAUNCH_BACKOFF: Duration = Duration::from_secs(30);

const ORIGIN_PREFIX: &str = "chrome-extension://";

/// True if the process was started by the browser as native messaging host
/// (some argument is the caller origin `chrome-extension://...`).
pub fn is_host_invocation<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|a| a.as_ref().to_string_lossy().starts_with(ORIGIN_PREFIX))
}

/// The arguments Chrome passes to a native host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostArgs {
    /// Caller origin, e.g. `chrome-extension://imfn.../`.
    pub origin: Option<String>,
    /// Windows only: handle of the calling browser window
    /// (`--parent-window=N`). Informational.
    pub parent_window: Option<u64>,
}

impl HostArgs {
    /// Parses the arguments (without the program name). Unknown arguments
    /// are ignored.
    pub fn parse<I, S>(args: I) -> HostArgs
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut parsed = HostArgs::default();
        for arg in args {
            let arg = arg.as_ref().to_string_lossy();
            if arg.starts_with(ORIGIN_PREFIX) {
                parsed.origin = Some(arg.into_owned());
            } else if let Some(n) = arg.strip_prefix("--parent-window=") {
                parsed.parent_window = n.trim().parse().ok();
            }
        }
        parsed
    }
}

/// How the host reaches (and if necessary starts) the app.
pub trait AppConnector {
    type Stream: Read + Write;
    /// Connects to the running app.
    fn connect(&mut self) -> io::Result<Self::Stream>;
    /// Starts the app in the background without waiting for it.
    fn launch_app(&mut self) -> io::Result<()>;
    /// How long to keep retrying after [`AppConnector::launch_app`].
    fn launch_timeout(&self) -> Duration {
        LAUNCH_TIMEOUT
    }
    /// Pause between connection attempts after a launch.
    fn retry_interval(&self) -> Duration {
        RETRY_INTERVAL
    }
}

impl<C: AppConnector + ?Sized> AppConnector for &mut C {
    type Stream = C::Stream;
    fn connect(&mut self) -> io::Result<Self::Stream> {
        (**self).connect()
    }
    fn launch_app(&mut self) -> io::Result<()> {
        (**self).launch_app()
    }
    fn launch_timeout(&self) -> Duration {
        (**self).launch_timeout()
    }
    fn retry_interval(&self) -> Duration {
        (**self).retry_interval()
    }
}

/// The real connector: the bridge local socket plus launching
/// `<exe> --background`.
#[derive(Debug, Clone)]
pub struct LocalSocketConnector {
    endpoint: Endpoint,
    app_exe: Option<PathBuf>,
    launch_timeout: Duration,
    reply_timeout: Duration,
}

impl LocalSocketConnector {
    /// Connects to `endpoint`, launches the current executable.
    pub fn new(endpoint: Endpoint) -> LocalSocketConnector {
        LocalSocketConnector {
            endpoint,
            app_exe: None,
            launch_timeout: LAUNCH_TIMEOUT,
            reply_timeout: REPLY_TIMEOUT,
        }
    }

    /// [`Endpoint::resolve`] and `$KEYSTEAD_APP_EXE` (if set) as launcher.
    pub fn from_env() -> LocalSocketConnector {
        let mut connector = LocalSocketConnector::new(Endpoint::resolve());
        connector.app_exe = std::env::var_os(APP_EXE_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        connector
    }

    /// Launch this executable instead of the current one.
    pub fn with_app_exe(mut self, exe: impl Into<PathBuf>) -> LocalSocketConnector {
        self.app_exe = Some(exe.into());
        self
    }

    pub fn with_launch_timeout(mut self, timeout: Duration) -> LocalSocketConnector {
        self.launch_timeout = timeout;
        self
    }

    pub fn with_reply_timeout(mut self, timeout: Duration) -> LocalSocketConnector {
        self.reply_timeout = timeout;
        self
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl AppConnector for LocalSocketConnector {
    type Stream = interprocess::local_socket::Stream;

    fn connect(&mut self) -> io::Result<Self::Stream> {
        // A named pipe is briefly "busy" while the server replaces the
        // instance it just handed out; retry that a few times instead of
        // treating the app as absent (which would launch it again).
        let mut attempt = 0;
        let stream = loop {
            match socket::connect(&self.endpoint) {
                Ok(stream) => break stream,
                Err(e) if is_busy(&e) && attempt < 5 => {
                    attempt += 1;
                    thread::sleep(Duration::from_millis(40));
                }
                Err(e) => return Err(e),
            }
        };
        // Best effort: Windows named pipes do not support timeouts.
        let _ = stream.set_recv_timeout(Some(self.reply_timeout));
        let _ = stream.set_send_timeout(Some(Duration::from_secs(10)));
        Ok(stream)
    }

    fn launch_app(&mut self) -> io::Result<()> {
        let exe = match &self.app_exe {
            Some(exe) => exe.clone(),
            None => std::env::current_exe()?,
        };
        log(format_args!("starting {} {BACKGROUND_ARG}", exe.display()));
        spawn_detached(&exe)
    }

    fn launch_timeout(&self) -> Duration {
        self.launch_timeout
    }
}

fn is_busy(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ResourceBusy | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Starts `exe --background` fully detached from the host: no inherited
/// stdio (stdout is the browser pipe!), own process group, and on Windows
/// no console and – if the browser's job object allows it – outside the job,
/// so the app survives when the browser ends the host.
fn spawn_detached(exe: &Path) -> io::Result<()> {
    let mut command = Command::new(exe);
    command
        .arg(BACKGROUND_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = exe.parent().filter(|d| d.is_dir()) {
        command.current_dir(dir);
    }

    #[cfg(unix)]
    let child = {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        command.spawn()?
    };

    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };
        win::make_stdio_uninheritable();
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        command.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            Ok(child) => child,
            // The job object does not allow breakaway: start inside it.
            Err(_) => {
                command.creation_flags(base);
                command.spawn()?
            }
        }
    };

    #[cfg(not(any(unix, windows)))]
    let child = command.spawn()?;

    // Reap the child if it exits while the host still runs (no zombies).
    let mut child = child;
    let _ = thread::Builder::new()
        .name("keystead-host-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

#[cfg(windows)]
mod win {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};

    /// Clears the inherit flag of our stdio handles. Rust's `Command`
    /// creates the child with handle inheritance enabled, which would leak
    /// the browser's stdin/stdout pipes into the long-running app and keep
    /// the browser from noticing that the host exited.
    pub(super) fn make_stdio_uninheritable() {
        let handles = [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ];
        for handle in handles {
            if !handle.is_null() {
                // SAFETY: the handle belongs to this process (or is invalid,
                // in which case the call just fails); only a flag changes.
                unsafe { SetHandleInformation(handle.cast(), HANDLE_FLAG_INHERIT, 0) };
            }
        }
    }
}

/// Entry point of native host mode. Returns the process exit code.
pub fn run() -> i32 {
    let args = HostArgs::parse(std::env::args_os().skip(1));
    log(format_args!(
        "native host started (caller: {})",
        args.origin.as_deref().unwrap_or("unknown")
    ));
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_with(
        stdin.lock(),
        stdout.lock(),
        LocalSocketConnector::from_env(),
    )
}

/// The relay loop on arbitrary streams (testable). Returns 0 on a clean EOF
/// of `input`, 1 if reading the browser or writing to it failed.
pub fn run_with<R, W, C>(mut input: R, mut output: W, connector: C) -> i32
where
    R: Read,
    W: Write,
    C: AppConnector,
{
    let mut relay = Relay::new(connector);
    loop {
        let frame = match framing::read_frame(&mut input, MAX_MESSAGE_SIZE) {
            Ok(Some(frame)) => Zeroizing::new(frame),
            Ok(None) => return 0,
            Err(Error::FrameTooLarge { len, max }) => {
                log(format_args!(
                    "message from the browser too large ({len} > {max} bytes)"
                ));
                let reply = Response::error("", BridgeError::InvalidRequest).to_bytes();
                let _ = framing::write_frame(&mut output, &reply, MAX_BROWSER_MESSAGE_SIZE);
                return 1;
            }
            Err(e) => {
                log(format_args!("reading from the browser failed: {e}"));
                return 1;
            }
        };
        let reply = relay.forward(&frame);
        if let Err(e) = framing::write_frame(&mut output, &reply, MAX_BROWSER_MESSAGE_SIZE) {
            log(format_args!("writing to the browser failed: {e}"));
            return 1;
        }
    }
}

enum ExchangeError {
    /// The request could not be delivered, or the connection closed before
    /// any reply arrived – safe to retry on a fresh connection.
    Retryable(io::Error),
    /// No reply in time, a broken reply or one for another request.
    Fatal(BridgeError),
}

struct Relay<C: AppConnector> {
    connector: C,
    conn: Option<C::Stream>,
    last_failed_launch: Option<Instant>,
}

impl<C: AppConnector> Relay<C> {
    fn new(connector: C) -> Self {
        Relay {
            connector,
            conn: None,
            last_failed_launch: None,
        }
    }

    /// Forwards one browser frame; always returns a reply frame for the
    /// browser (the app's reply or an error with the request's id).
    fn forward(&mut self, frame: &[u8]) -> Zeroizing<Vec<u8>> {
        let Some(id) = extract_id(frame) else {
            // Not even an id: the app could not answer it meaningfully either.
            return error_reply("", BridgeError::InvalidRequest);
        };
        match self.exchange(frame, &id) {
            Ok(reply) if reply.len() <= MAX_BROWSER_MESSAGE_SIZE => reply,
            Ok(_) => {
                log("reply of the app exceeds the browser's 1 MiB limit");
                error_reply(&id, BridgeError::Internal)
            }
            Err(code) => error_reply(&id, code),
        }
    }

    fn exchange(&mut self, frame: &[u8], id: &str) -> Result<Zeroizing<Vec<u8>>, BridgeError> {
        if let Some(mut conn) = self.conn.take() {
            match exchange_on(&mut conn, frame, id) {
                Ok(reply) => {
                    self.conn = Some(conn);
                    return Ok(reply);
                }
                Err(ExchangeError::Retryable(e)) => {
                    log(format_args!(
                        "connection to the app lost ({e}); reconnecting"
                    ));
                }
                Err(ExchangeError::Fatal(code)) => return Err(code),
            }
        }
        let mut conn = self.connect_or_launch()?;
        match exchange_on(&mut conn, frame, id) {
            Ok(reply) => {
                self.conn = Some(conn);
                Ok(reply)
            }
            Err(ExchangeError::Retryable(e)) => {
                log(format_args!("the app closed the connection ({e})"));
                Err(BridgeError::AppUnavailable)
            }
            Err(ExchangeError::Fatal(code)) => Err(code),
        }
    }

    fn connect_or_launch(&mut self) -> Result<C::Stream, BridgeError> {
        match self.connector.connect() {
            Ok(conn) => {
                self.last_failed_launch = None;
                return Ok(conn);
            }
            Err(e) => log(format_args!("the app is not reachable ({e})")),
        }
        if self
            .last_failed_launch
            .is_some_and(|t| t.elapsed() < RELAUNCH_BACKOFF)
        {
            return Err(BridgeError::AppUnavailable);
        }
        if let Err(e) = self.connector.launch_app() {
            log(format_args!("could not start the app: {e}"));
            self.last_failed_launch = Some(Instant::now());
            return Err(BridgeError::AppUnavailable);
        }
        let deadline = Instant::now() + self.connector.launch_timeout();
        loop {
            thread::sleep(self.connector.retry_interval());
            match self.connector.connect() {
                Ok(conn) => {
                    self.last_failed_launch = None;
                    return Ok(conn);
                }
                Err(e) if Instant::now() >= deadline => {
                    log(format_args!("the app did not become reachable: {e}"));
                    self.last_failed_launch = Some(Instant::now());
                    return Err(BridgeError::AppUnavailable);
                }
                Err(_) => {}
            }
        }
    }
}

/// Sends one request and reads its reply.
fn exchange_on<S: Read + Write>(
    conn: &mut S,
    frame: &[u8],
    id: &str,
) -> Result<Zeroizing<Vec<u8>>, ExchangeError> {
    if let Err(e) = framing::write_frame(conn, frame, MAX_MESSAGE_SIZE) {
        return Err(match e {
            Error::Io(e) => ExchangeError::Retryable(e),
            _ => ExchangeError::Fatal(BridgeError::InvalidRequest),
        });
    }
    let reply = match framing::read_frame(conn, MAX_MESSAGE_SIZE) {
        Ok(Some(reply)) => Zeroizing::new(reply),
        Ok(None) => {
            return Err(ExchangeError::Retryable(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            )))
        }
        Err(Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
            ) =>
        {
            return Err(ExchangeError::Retryable(e))
        }
        Err(e) => {
            log(format_args!("no valid reply from the app: {e}"));
            return Err(ExchangeError::Fatal(BridgeError::AppUnavailable));
        }
    };
    if extract_id(&reply).as_deref() != Some(id) {
        log("the app answered with a mismatching id");
        return Err(ExchangeError::Fatal(BridgeError::Internal));
    }
    Ok(reply)
}

fn error_reply(id: &str, code: BridgeError) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(Response::error(id, code).to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn host_invocation_detection() {
        assert!(is_host_invocation(["chrome-extension://abc/"]));
        assert!(is_host_invocation([
            "chrome-extension://abc/",
            "--parent-window=1234"
        ]));
        assert!(is_host_invocation([
            "--parent-window=0",
            "chrome-extension://x/"
        ]));
        assert!(!is_host_invocation(["--background"]));
        assert!(!is_host_invocation(Vec::<String>::new()));
    }

    #[test]
    fn host_args_parsing() {
        let a = HostArgs::parse(["chrome-extension://abc/", "--parent-window=4567"]);
        assert_eq!(a.origin.as_deref(), Some("chrome-extension://abc/"));
        assert_eq!(a.parent_window, Some(4567));
        let a = HostArgs::parse(["--parent-window=x", "other"]);
        assert_eq!(a, HostArgs::default());
    }

    /// A connector whose app is never reachable; counts launches.
    struct Unreachable {
        launches: usize,
        launch_fails: bool,
    }

    impl AppConnector for Unreachable {
        type Stream = Cursor<Vec<u8>>;
        fn connect(&mut self) -> io::Result<Self::Stream> {
            Err(io::Error::new(io::ErrorKind::NotFound, "no app"))
        }
        fn launch_app(&mut self) -> io::Result<()> {
            self.launches += 1;
            if self.launch_fails {
                Err(io::Error::new(io::ErrorKind::NotFound, "no exe"))
            } else {
                Ok(())
            }
        }
        fn launch_timeout(&self) -> Duration {
            Duration::from_millis(50)
        }
        fn retry_interval(&self) -> Duration {
            Duration::from_millis(10)
        }
    }

    fn frames(messages: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in messages {
            framing::write_frame(&mut buf, m, MAX_MESSAGE_SIZE).unwrap();
        }
        buf
    }

    fn replies(output: &[u8]) -> Vec<Response> {
        let mut r = Cursor::new(output);
        let mut out = Vec::new();
        while let Some(f) = framing::read_frame(&mut r, MAX_BROWSER_MESSAGE_SIZE).unwrap() {
            out.push(serde_json::from_slice(&f).unwrap());
        }
        out
    }

    #[test]
    fn unreachable_app_yields_app_unavailable_with_backoff() {
        let input = frames(&[
            br#"{"id":"a","type":"status"}"#,
            br#"{"id":"b","type":"status"}"#,
            b"garbage",
        ]);
        let mut output = Vec::new();
        let mut connector = Unreachable {
            launches: 0,
            launch_fails: false,
        };
        let code = run_with(Cursor::new(input), &mut output, &mut connector);
        assert_eq!(code, 0);
        assert_eq!(
            replies(&output),
            vec![
                Response::error("a", BridgeError::AppUnavailable),
                Response::error("b", BridgeError::AppUnavailable),
                Response::error("", BridgeError::InvalidRequest),
            ]
        );
        // The second request did not launch again (backoff).
        assert_eq!(connector.launches, 1);
    }

    #[test]
    fn failing_launch_is_reported() {
        let input = frames(&[br#"{"id":"a","type":"focus_app"}"#]);
        let mut output = Vec::new();
        let mut connector = Unreachable {
            launches: 0,
            launch_fails: true,
        };
        assert_eq!(run_with(Cursor::new(input), &mut output, &mut connector), 0);
        assert_eq!(
            replies(&output),
            vec![Response::error("a", BridgeError::AppUnavailable)]
        );
    }

    #[test]
    fn oversized_browser_message_ends_the_host() {
        let input = u32::try_from(MAX_MESSAGE_SIZE + 1).unwrap().to_le_bytes();
        let mut output = Vec::new();
        let connector = Unreachable {
            launches: 0,
            launch_fails: false,
        };
        assert_eq!(run_with(Cursor::new(input), &mut output, connector), 1);
        assert_eq!(
            replies(&output),
            vec![Response::error("", BridgeError::InvalidRequest)]
        );
    }

    #[test]
    fn truncated_browser_message_ends_the_host() {
        let mut input = 50u32.to_le_bytes().to_vec();
        input.extend_from_slice(b"{\"id\"");
        let mut output = Vec::new();
        let connector = Unreachable {
            launches: 0,
            launch_fails: false,
        };
        assert_eq!(run_with(Cursor::new(input), &mut output, connector), 1);
        assert!(output.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn launch_runs_the_configured_exe_with_background_flag() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("args.txt");
        let script = dir.path().join("fake-keystead.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" > '{}.tmp'\nmv '{0}.tmp' '{0}'\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut connector = LocalSocketConnector::new(Endpoint::Path(dir.path().join("none.sock")))
            .with_app_exe(&script);
        connector.launch_app().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !out.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().trim(),
            BACKGROUND_ARG
        );
    }
}
