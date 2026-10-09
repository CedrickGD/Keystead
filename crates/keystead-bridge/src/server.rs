//! The bridge server run by the desktop app: one accept thread plus one
//! thread per connection (the native host keeps a single connection open
//! while the browser port lives). Requests on a connection are answered in
//! order.

use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{Listener, Stream};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::framing::{self, MAX_MESSAGE_SIZE};
use crate::protocol::{BridgeError, Request, Response};
use crate::socket::{self, Endpoint};
use crate::util::log;

/// Upper bound of simultaneously served connections (each one is a thread).
const MAX_CONNECTIONS: usize = 32;
/// How long [`ServerHandle::stop`] waits for the accept thread.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// Answers bridge requests. Implemented by
/// [`Dispatcher`](crate::dispatcher::Dispatcher); called concurrently from
/// several connection threads.
pub trait BridgeHandler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> Response;
}

#[derive(Default)]
struct Shared {
    stopping: AtomicBool,
    connections: AtomicUsize,
}

/// A running bridge server. Stops on [`ServerHandle::stop`] or when dropped.
pub struct ServerHandle {
    endpoint: Endpoint,
    shared: Arc<Shared>,
    accept_thread: Option<JoinHandle<()>>,
    /// Disconnected when the accept thread has finished.
    finished: Option<mpsc::Receiver<()>>,
}

impl std::fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerHandle")
            .field("endpoint", &self.endpoint)
            .field("running", &self.is_running())
            .finish()
    }
}

/// Starts the server on [`Endpoint::resolve`] (`$KEYSTEAD_BRIDGE_SOCKET` or
/// the per-user default).
///
/// Fails with [`Error::AlreadyRunning`] if another live server (e.g. a
/// second app instance) already listens there – it is never displaced.
pub fn start_server(handler: Arc<dyn BridgeHandler>) -> Result<ServerHandle> {
    start_server_at(Endpoint::resolve(), handler)
}

/// Starts the server on an explicit endpoint.
pub fn start_server_at(
    endpoint: Endpoint,
    handler: Arc<dyn BridgeHandler>,
) -> Result<ServerHandle> {
    let listener = socket::bind(&endpoint)?;
    let shared = Arc::new(Shared::default());
    let (finished_tx, finished_rx) = mpsc::channel::<()>();
    let thread_shared = Arc::clone(&shared);
    let accept_thread = thread::Builder::new()
        .name("keystead-bridge-accept".into())
        .spawn(move || {
            accept_loop(&listener, &handler, &thread_shared);
            // Drop the listener (removes the Unix socket file) before
            // signalling completion.
            drop(listener);
            drop(finished_tx);
        })
        .map_err(Error::Io)?;
    Ok(ServerHandle {
        endpoint,
        shared,
        accept_thread: Some(accept_thread),
        finished: Some(finished_rx),
    })
}

impl ServerHandle {
    /// The endpoint the server listens on.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// True while the accept loop runs.
    pub fn is_running(&self) -> bool {
        self.accept_thread
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    /// Stops accepting connections and releases the endpoint. Connections
    /// that are still open are closed (unanswered) on their next request, so
    /// the native host reconnects – to a restarted server, if any. Requests
    /// already being handled complete normally. Idempotent.
    pub fn stop(&mut self) {
        let Some(thread) = self.accept_thread.take() else {
            return;
        };
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wake the blocking accept() by connecting to ourselves.
        let wake = socket::poke(&self.endpoint);
        let done = match self.finished.take() {
            Some(rx) => !matches!(
                rx.recv_timeout(STOP_WAIT),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            None => true,
        };
        drop(wake);
        if done {
            let _ = thread.join();
        } else {
            // Only possible if the endpoint was hijacked or deleted behind
            // our back; the thread exits with the next connection.
            log("bridge accept thread did not stop in time; detaching it");
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Decrements the connection counter when a connection thread ends.
struct ConnectionSlot(Arc<Shared>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

fn accept_loop(listener: &Listener, handler: &Arc<dyn BridgeHandler>, shared: &Arc<Shared>) {
    // Consecutive accept failures; only the first few are logged.
    let mut failures = 0u32;
    loop {
        let accepted = listener.accept();
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        let stream: Stream = match accepted {
            Ok(stream) => {
                failures = 0;
                stream
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                if failures <= 3 {
                    log(format_args!("bridge accept failed: {e}"));
                }
                // Avoid a busy loop on persistent errors (e.g. out of fds).
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        if !socket::peer_is_current_user(&stream) {
            log("rejected a bridge connection from another user");
            continue;
        }
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            log("too many bridge connections; rejecting");
            continue;
        }
        // Moved into the thread; frees the slot when the thread ends (or
        // right away if it cannot be spawned).
        let slot = ConnectionSlot(Arc::clone(shared));
        let handler = Arc::clone(handler);
        let spawned = thread::Builder::new()
            .name("keystead-bridge-conn".into())
            .spawn(move || serve_connection(stream, handler.as_ref(), &slot.0.stopping));
        if let Err(e) = spawned {
            log(format_args!(
                "could not spawn a bridge connection thread: {e}"
            ));
        }
    }
}

/// Serves one connection until EOF, an I/O error or server stop.
fn serve_connection<S: Read + Write>(
    mut stream: S,
    handler: &dyn BridgeHandler,
    stopping: &AtomicBool,
) {
    loop {
        let frame = match framing::read_frame(&mut stream, MAX_MESSAGE_SIZE) {
            Ok(Some(frame)) => Zeroizing::new(frame),
            Ok(None) => return,
            Err(Error::FrameTooLarge { .. }) => {
                // The stream cannot be resynchronised: answer and close.
                let _ = send(
                    &mut stream,
                    &Response::error("", BridgeError::InvalidRequest),
                );
                return;
            }
            Err(_) => return,
        };
        if stopping.load(Ordering::SeqCst) {
            // Close without processing or answering: the host treats this as
            // a lost connection and retries on a fresh one, which reaches a
            // restarted server transparently (or reports `app_unavailable`).
            return;
        }
        let response = handle_frame(handler, &frame);
        if send(&mut stream, &response).is_err() {
            return;
        }
    }
}

/// Parses one request frame and lets `handler` answer it. Invalid input
/// yields `invalid_request`, a panicking handler `internal` (in builds that
/// unwind). The response always carries the request's id.
pub fn handle_frame(handler: &dyn BridgeHandler, frame: &[u8]) -> Response {
    let request = match Request::parse(frame) {
        Ok(request) => request,
        Err(invalid) => return invalid,
    };
    let id = request.id.clone();
    let mut response = panic::catch_unwind(AssertUnwindSafe(|| handler.handle(request)))
        .unwrap_or_else(|_| {
            log("bridge request handler panicked");
            Response::error(id.clone(), BridgeError::Internal)
        });
    response.id = id;
    response
}

fn send<W: Write>(stream: &mut W, response: &Response) -> Result<()> {
    let bytes = Zeroizing::new(response.to_bytes());
    if bytes.len() > MAX_MESSAGE_SIZE {
        log("bridge response exceeds the message size limit");
        let fallback = Response::error(response.id.clone(), BridgeError::Internal);
        return framing::write_frame(stream, &fallback.to_bytes(), MAX_MESSAGE_SIZE);
    }
    framing::write_frame(stream, &bytes, MAX_MESSAGE_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Payload;
    use std::io::{self, Cursor};

    struct Echo;

    impl BridgeHandler for Echo {
        fn handle(&self, request: Request) -> Response {
            match request.payload {
                Payload::Search { query } if query == "panic" => panic!("handler bug"),
                Payload::Search { query } => Response::success("wrong-id", &query),
                _ => Response::null(request.id),
            }
        }
    }

    /// In-memory duplex stream: reads from `input`, collects writes.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn frames(messages: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in messages {
            framing::write_frame(&mut buf, m, MAX_MESSAGE_SIZE).unwrap();
        }
        buf
    }

    fn responses(output: &[u8]) -> Vec<Response> {
        let mut r = Cursor::new(output);
        let mut out = Vec::new();
        while let Some(frame) = framing::read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap() {
            out.push(serde_json::from_slice(&frame).unwrap());
        }
        out
    }

    #[test]
    fn connection_answers_in_order_and_survives_garbage() {
        let input = frames(&[
            br#"{"id":"1","type":"status"}"#,
            b"not json",
            br#"{"id":"3","type":"no_such_type"}"#,
            br#"{"id":"4","type":"search"}"#,
            br#"{"id":"5","type":"search","query":"x"}"#,
        ]);
        let mut conn = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        serve_connection(&mut conn, &Echo, &AtomicBool::new(false));
        let r = responses(&conn.output);
        assert_eq!(r.len(), 5);
        assert_eq!(r[0], Response::null("1"));
        assert_eq!(r[1], Response::error("", BridgeError::InvalidRequest));
        assert_eq!(r[2], Response::error("3", BridgeError::InvalidRequest));
        assert_eq!(r[3], Response::error("4", BridgeError::InvalidRequest));
        // The handler's wrong id is corrected.
        assert_eq!(r[4], Response::success("5", &"x"));
    }

    #[test]
    fn panicking_handler_yields_internal() {
        let r = handle_frame(&Echo, br#"{"id":"p","type":"search","query":"panic"}"#);
        assert_eq!(r, Response::error("p", BridgeError::Internal));
    }

    #[test]
    fn oversized_frame_closes_connection() {
        let mut input = u32::try_from(MAX_MESSAGE_SIZE + 1)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        input.extend_from_slice(&frames(&[br#"{"id":"1","type":"status"}"#]));
        let mut conn = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        serve_connection(&mut conn, &Echo, &AtomicBool::new(false));
        assert_eq!(
            responses(&conn.output),
            vec![Response::error("", BridgeError::InvalidRequest)]
        );
    }

    #[test]
    fn stopped_server_closes_without_answering() {
        let input = frames(&[
            br#"{"id":"1","type":"status"}"#,
            br#"{"id":"2","type":"status"}"#,
        ]);
        let mut conn = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        serve_connection(&mut conn, &Echo, &AtomicBool::new(true));
        assert!(conn.output.is_empty());
    }
}
