//! Host relay against the real socket with explicit endpoints: launching the
//! app on demand and the browser message size limit.

use std::io::{self, Cursor};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::json;
use vaultx_bridge::framing::{read_frame, write_frame, MAX_BROWSER_MESSAGE_SIZE, MAX_MESSAGE_SIZE};
use vaultx_bridge::host::{run_with, AppConnector, LocalSocketConnector};
use vaultx_bridge::server::BridgeHandler;
use vaultx_bridge::socket::Endpoint;
use vaultx_bridge::{start_server_at, BridgeError, Payload, Request, Response, ServerHandle};

fn endpoint(dir: &tempfile::TempDir, name: &str) -> Endpoint {
    if cfg!(windows) {
        Endpoint::Namespaced(format!("vaultx-bridge-relay-{}-{name}", std::process::id()))
    } else {
        Endpoint::Path(dir.path().join(format!("{name}.sock")))
    }
}

/// Answers `search` with a reply of the requested size, everything else null.
struct SizedReplies;

impl BridgeHandler for SizedReplies {
    fn handle(&self, request: Request) -> Response {
        match request.payload {
            Payload::Search { query } => {
                let n: usize = query.parse().unwrap_or(0);
                Response::success(request.id, &"x".repeat(n))
            }
            _ => Response::null(request.id),
        }
    }
}

fn frames(messages: &[serde_json::Value]) -> Vec<u8> {
    let mut buf = Vec::new();
    for m in messages {
        write_frame(&mut buf, m.to_string().as_bytes(), MAX_MESSAGE_SIZE).unwrap();
    }
    buf
}

fn replies(output: &[u8]) -> Vec<Response> {
    let mut r = Cursor::new(output);
    let mut out = Vec::new();
    while let Some(f) = read_frame(&mut r, MAX_BROWSER_MESSAGE_SIZE).unwrap() {
        out.push(serde_json::from_slice(&f).unwrap());
    }
    out
}

/// Real socket connector whose "app launch" starts the server in-process
/// after a delay (like a real app needing a moment to start).
struct LaunchingConnector {
    inner: LocalSocketConnector,
    launches: Arc<AtomicUsize>,
    server: Arc<Mutex<Option<ServerHandle>>>,
}

impl AppConnector for LaunchingConnector {
    type Stream = <LocalSocketConnector as AppConnector>::Stream;

    fn connect(&mut self) -> io::Result<Self::Stream> {
        self.inner.connect()
    }

    fn launch_app(&mut self) -> io::Result<()> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let endpoint = self.inner.endpoint().clone();
        let slot = Arc::clone(&self.server);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let server = start_server_at(endpoint, Arc::new(SizedReplies)).unwrap();
            *slot.lock().unwrap() = Some(server);
        });
        Ok(())
    }

    fn launch_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }
}

#[test]
fn host_launches_the_app_when_it_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "launch");
    let launches = Arc::new(AtomicUsize::new(0));
    let server = Arc::new(Mutex::new(None));
    let connector = LaunchingConnector {
        inner: LocalSocketConnector::new(ep),
        launches: Arc::clone(&launches),
        server: Arc::clone(&server),
    };
    let input = frames(&[
        json!({"id": "1", "type": "status"}),
        json!({"id": "2", "type": "focus_app"}),
    ]);
    let mut output = Vec::new();
    assert_eq!(run_with(Cursor::new(input), &mut output, connector), 0);
    assert_eq!(
        replies(&output),
        vec![Response::null("1"), Response::null("2")]
    );
    assert_eq!(
        launches.load(Ordering::SeqCst),
        1,
        "launched once, then the connection is reused"
    );
    assert!(server.lock().unwrap().is_some());
}

#[test]
fn replies_over_one_mib_are_not_forwarded_to_the_browser() {
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "size");
    let _server = start_server_at(ep.clone(), Arc::new(SizedReplies)).unwrap();
    let fits = MAX_BROWSER_MESSAGE_SIZE - 100;
    let input = frames(&[
        json!({"id": "small", "type": "search", "query": fits.to_string()}),
        json!({"id": "big", "type": "search", "query": (MAX_BROWSER_MESSAGE_SIZE + 10).to_string()}),
        json!({"id": "after", "type": "status"}),
    ]);
    let mut output = Vec::new();
    let connector = LocalSocketConnector::new(ep).with_app_exe(dir.path().join("missing"));
    assert_eq!(run_with(Cursor::new(input), &mut output, connector), 0);
    let r = replies(&output);
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].data.as_str().unwrap().len(), fits);
    assert_eq!(r[1], Response::error("big", BridgeError::Internal));
    assert_eq!(r[2], Response::null("after"));
}

#[test]
fn missing_app_executable_yields_app_unavailable_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let connector = LocalSocketConnector::new(endpoint(&dir, "absent"))
        .with_app_exe(dir.path().join("no-such-binary"))
        .with_launch_timeout(Duration::from_millis(200));
    let input = frames(&[json!({"id": "x", "type": "status"})]);
    let mut output = Vec::new();
    let started = std::time::Instant::now();
    assert_eq!(run_with(Cursor::new(input), &mut output, connector), 0);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        replies(&output),
        vec![Response::error("x", BridgeError::AppUnavailable)]
    );
}
