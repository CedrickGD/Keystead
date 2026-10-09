//! The real local-socket server: start/stop, single instance, stale socket
//! cleanup, permissions, concurrency.

mod common;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common::*;
use keystead_bridge::socket::{self, Client, Endpoint};
use keystead_bridge::{
    start_server_at, BridgeError, ClientStore, Dispatcher, Error, Payload, Request, Response,
};
use serde_json::json;

/// A unique endpoint per test.
fn endpoint(dir: &tempfile::TempDir, name: &str) -> Endpoint {
    if cfg!(windows) {
        Endpoint::Namespaced(format!(
            "keystead-bridge-test-{}-{name}",
            std::process::id()
        ))
    } else {
        Endpoint::Path(dir.path().join(format!("{name}.sock")))
    }
}

fn dispatcher(dir: &tempfile::TempDir) -> (Arc<Dispatcher>, Arc<FakeBackend>) {
    let backend = Arc::new(FakeBackend::new());
    let store = ClientStore::open(dir.path().join("clients.json")).unwrap();
    (Arc::new(Dispatcher::new(backend.clone(), store)), backend)
}

fn status(client: &mut Client, id: &str) -> Response {
    client
        .request(&Request::new(id, Payload::Status))
        .expect("status request")
}

#[test]
fn serves_requests_and_refuses_a_second_instance() {
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "single");
    let (d, _) = dispatcher(&dir);
    let mut server = start_server_at(ep.clone(), d.clone()).unwrap();
    assert!(server.is_running());
    assert_eq!(server.endpoint(), &ep);

    let mut client = Client::connect(&ep).unwrap();
    for i in 0..3 {
        let r = status(&mut client, &format!("s{i}"));
        assert_eq!(r.id, format!("s{i}"));
        assert_eq!(r.data["appVersion"], "2.0.0-test");
    }

    // A second instance must not steal the endpoint.
    let second = start_server_at(ep.clone(), d.clone());
    assert!(
        matches!(second, Err(Error::AlreadyRunning(_))),
        "{second:?}"
    );
    // ... and the first one keeps working.
    assert!(status(&mut client, "after").ok);

    server.stop();
    assert!(!server.is_running());
    server.stop(); // idempotent
    #[cfg(unix)]
    if let Endpoint::Path(p) = &ep {
        assert!(!p.exists(), "socket file is removed on stop");
    }
    assert!(socket::connect(&ep).is_err());

    // The endpoint can be reused right away.
    let server = start_server_at(ep.clone(), d).unwrap();
    let mut client = Client::connect(&ep).unwrap();
    assert!(status(&mut client, "again").ok);
    drop(server); // Drop stops too.
    assert!(socket::connect(&ep).is_err());
}

#[test]
fn open_connections_are_closed_after_stop() {
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "stop");
    let (d, _) = dispatcher(&dir);
    let mut server = start_server_at(ep.clone(), d).unwrap();
    let mut client = Client::connect(&ep).unwrap();
    assert!(status(&mut client, "1").ok);
    server.stop();
    // The next request on the old connection is not processed.
    assert!(client.request(&Request::new("2", Payload::Status)).is_err());
}

#[cfg(unix)]
#[test]
fn stale_socket_file_is_replaced_and_permissions_are_private() {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stale.sock");
    // A crashed instance leaves its socket file behind.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());

    let (d, _) = dispatcher(&dir);
    let ep = Endpoint::Path(path.clone());
    let server = start_server_at(ep.clone(), d).unwrap();
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);

    let mut client = Client::connect(&ep).unwrap();
    assert!(status(&mut client, "1").ok);
    drop(server);
}

#[cfg(unix)]
#[test]
fn foreign_live_server_is_detected() {
    // Something else (here: a plain Unix listener) is live on the path.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.sock");
    let _other = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let (d, _) = dispatcher(&dir);
    let result = start_server_at(Endpoint::Path(path.clone()), d);
    assert!(
        matches!(result, Err(Error::AlreadyRunning(_))),
        "{result:?}"
    );
    assert!(path.exists(), "a live socket is never removed");
}

#[test]
fn pairing_blocks_only_its_own_connection() {
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "concurrent");
    let (d, backend) = dispatcher(&dir);
    let _server = start_server_at(ep.clone(), d.clone()).unwrap();

    thread::scope(|s| {
        let pairing = s.spawn(|| {
            let mut c = Client::connect(&ep).unwrap();
            c.request(&Request::new(
                "p",
                Payload::Pair {
                    client_name: "Vivaldi".into(),
                    code: "999999".into(),
                },
            ))
            .unwrap()
        });
        let req = backend.next_pairing(Duration::from_secs(10)).unwrap();
        // While the pairing waits, other connections are served.
        let mut other = Client::connect(&ep).unwrap();
        let r = status(&mut other, "s");
        assert_eq!(r.data["paired"], false);
        let r = other.request(&Request::new("l", Payload::Lock)).unwrap();
        assert_eq!(r, Response::error("l", BridgeError::NotPaired));

        assert!(d.respond_pairing(&req.request_id, true));
        let r = pairing.join().unwrap();
        assert!(r.ok);
        let cid = r.data["clientId"].as_str().unwrap();
        let tok = r.data["token"].as_str().unwrap();
        let r = other
            .request(&Request::new("l2", Payload::Lock).with_credentials(cid, tok))
            .unwrap();
        assert_eq!(r, Response::null("l2"));
    });
}

#[test]
fn pairing_is_withdrawn_when_the_requester_hangs_up() {
    // The extension cancelled the pairing (or the browser closed): the native
    // host and with it the connection are gone. The request must disappear
    // and must not be approvable anymore (nobody would get the token).
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "hangup");
    let (d, backend) = dispatcher(&dir);
    let _server = start_server_at(ep.clone(), d.clone()).unwrap();

    let mut stream = socket::connect(&ep).unwrap();
    keystead_bridge::framing::write_frame(
        &mut stream,
        json!({"id": "p", "type": "pair", "clientName": "Chrome", "code": "123456"})
            .to_string()
            .as_bytes(),
        keystead_bridge::framing::MAX_MESSAGE_SIZE,
    )
    .unwrap();
    let req = backend.next_pairing(Duration::from_secs(10)).unwrap();
    assert_eq!(d.pending_pairings().len(), 1);
    drop(stream);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !d.pending_pairings().is_empty() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(d.pending_pairings().is_empty(), "request withdrawn");
    assert!(!d.respond_pairing(&req.request_id, true));
    assert!(d.clients().is_empty(), "no client was paired");
    assert_eq!(
        *backend.closed_pairings.lock().unwrap(),
        vec![req.request_id.clone()],
        "the app was told to close its dialog"
    );
}

#[test]
fn superseded_pairing_is_reported_closed() {
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "supersede");
    let (d, backend) = dispatcher(&dir);
    let _server = start_server_at(ep.clone(), d.clone()).unwrap();
    thread::scope(|s| {
        let pair = |code: &'static str| {
            let ep = ep.clone();
            move || {
                Client::connect(&ep)
                    .unwrap()
                    .request(&Request::new(
                        code,
                        Payload::Pair {
                            client_name: "Edge".into(),
                            code: code.into(),
                        },
                    ))
                    .unwrap()
            }
        };
        let first = s.spawn(pair("111111"));
        let old = backend.next_pairing(Duration::from_secs(10)).unwrap();
        let second = s.spawn(pair("222222"));
        let new = backend.next_pairing(Duration::from_secs(10)).unwrap();
        assert_eq!(
            first.join().unwrap(),
            Response::error("111111", BridgeError::PairingDenied)
        );
        assert_eq!(
            *backend.closed_pairings.lock().unwrap(),
            vec![old.request_id.clone()]
        );
        assert!(d.respond_pairing(&new.request_id, true));
        assert!(second.join().unwrap().ok);
    });
}

#[test]
fn raw_garbage_on_the_socket_gets_invalid_request() {
    use keystead_bridge::framing::{read_frame, write_frame, MAX_MESSAGE_SIZE};
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint(&dir, "garbage");
    let (d, _) = dispatcher(&dir);
    let _server = start_server_at(ep.clone(), d).unwrap();
    let mut stream = socket::connect(&ep).unwrap();
    write_frame(&mut stream, b"{nope", MAX_MESSAGE_SIZE).unwrap();
    let reply = read_frame(&mut stream, MAX_MESSAGE_SIZE).unwrap().unwrap();
    let r: Response = serde_json::from_slice(&reply).unwrap();
    assert_eq!(r, Response::error("", BridgeError::InvalidRequest));
    // The connection is still usable.
    write_frame(
        &mut stream,
        json!({"id": "ok", "type": "status"}).to_string().as_bytes(),
        MAX_MESSAGE_SIZE,
    )
    .unwrap();
    let reply = read_frame(&mut stream, MAX_MESSAGE_SIZE).unwrap().unwrap();
    let r: Response = serde_json::from_slice(&reply).unwrap();
    assert!(r.ok && r.id == "ok");
    // An oversized frame closes the connection after an error reply.
    stream.write_all(&u32::MAX.to_le_bytes()).unwrap();
    let reply = read_frame(&mut stream, MAX_MESSAGE_SIZE).unwrap().unwrap();
    let r: Response = serde_json::from_slice(&reply).unwrap();
    assert_eq!(r.error_code(), Some(BridgeError::InvalidRequest));
    assert!(read_frame(&mut stream, MAX_MESSAGE_SIZE).unwrap().is_none());
}
