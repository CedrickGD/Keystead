//! An in-process WebDAV server for the unit tests (the pattern of
//! `update.rs`'s status server): HTTP/1.1 on 127.0.0.1, plain or TLS with
//! a certificate of its own (rcgen), Basic authentication, the methods the
//! client uses with ETags and preconditions – and hostile answers
//! ([`Hostile`]): an oversized body, a DOCTYPE bomb, hrefs of another host,
//! with `..` or an encoded slash, a redirect, any status.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};

use percent_encoding::percent_decode_str;

/// What the server does wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hostile {
    None,
    /// `PROPFIND` answers 207 with a 9 MiB body.
    OversizedPropfind,
    /// `PROPFIND` answers with a DTD (a "billion laughs").
    DocTypeBomb,
    /// The listing's entries name another host.
    ForeignHref,
    /// A listing entry with `..`.
    DotDotHref,
    /// A listing entry with an encoded `/`.
    EncodedSlashHref,
    /// Every request to a path outside this one: `302` to it.
    Redirect(String),
    /// Every request: this status, no body.
    Status(u16),
    /// `GET` sends this many bytes without a length (until it closes).
    EndlessGet(usize),
    /// Preconditions are ignored.
    IgnoreIfMatch,
}

struct State {
    user: String,
    password: String,
    files: BTreeMap<String, (Vec<u8>, u64)>,
    dirs: BTreeSet<String>,
    next: u64,
    hostile: Hostile,
    log: Vec<String>,
}

/// A running server. Its root folder is `/dav/` ([`TestServer::base`]).
pub struct TestServer {
    /// `http(s)://127.0.0.1:<port>/dav/`.
    pub base: String,
    /// The TLS certificate (DER), if it speaks TLS.
    pub cert_der: Option<Vec<u8>>,
    state: Arc<Mutex<State>>,
}

impl TestServer {
    /// Plain HTTP (allowed to 127.0.0.1 in debug builds).
    pub fn http(user: &str, password: &str) -> TestServer {
        Self::start(user, password, None)
    }

    /// TLS with a self-signed certificate for 127.0.0.1.
    pub fn https(user: &str, password: &str) -> TestServer {
        let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        let cert = certified.cert.der().to_vec();
        let key = certified.signing_key.serialize_der();
        Self::start(user, password, Some((cert, key)))
    }

    fn start(user: &str, password: &str, tls: Option<(Vec<u8>, Vec<u8>)>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let scheme = if tls.is_some() { "https" } else { "http" };
        let state = Arc::new(Mutex::new(State {
            user: user.to_owned(),
            password: password.to_owned(),
            files: BTreeMap::new(),
            dirs: BTreeSet::from(["/dav".to_owned()]),
            next: 0,
            hostile: Hostile::None,
            log: Vec::new(),
        }));
        let config = tls.as_ref().map(|(cert, key)| {
            let provider = super::tls::provider();
            Arc::new(
                rustls::ServerConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .with_no_client_auth()
                    .with_single_cert(
                        vec![rustls::pki_types::CertificateDer::from(cert.clone())],
                        rustls::pki_types::PrivateKeyDer::Pkcs8(key.clone().into()),
                    )
                    .unwrap(),
            )
        });
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let state = Arc::clone(&shared);
                let config = config.clone();
                std::thread::spawn(move || {
                    let _ = match config {
                        Some(config) => {
                            let Ok(conn) = rustls::ServerConnection::new(config) else {
                                return;
                            };
                            let mut tls = rustls::StreamOwned::new(conn, stream);
                            serve(&state, &mut tls)
                        }
                        None => {
                            let mut plain: TcpStream = stream;
                            serve(&state, &mut plain)
                        }
                    };
                });
            }
        });
        TestServer {
            base: format!("{scheme}://{addr}/dav/"),
            cert_der: tls.map(|(cert, _)| cert),
            state,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_hostile(&self, hostile: Hostile) {
        self.state().hostile = hostile;
    }

    /// Creates the folder `/dav/<path>`.
    pub fn mkdir(&self, path: &str) {
        self.state()
            .dirs
            .insert(format!("/dav/{}", path.trim_matches('/')));
    }

    /// The file `/dav/<path>`.
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.state()
            .files
            .get(&format!("/dav/{path}"))
            .map(|(b, _)| b.clone())
    }

    /// Writes `/dav/<path>` directly.
    pub fn put(&self, path: &str, bytes: &[u8]) {
        let mut st = self.state();
        st.next += 1;
        let etag = st.next;
        st.files
            .insert(format!("/dav/{path}"), (bytes.to_vec(), etag));
    }

    /// The requests so far (`METHOD /path`), and forgets them.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.state().log)
    }
}

struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut impl Read) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}

fn respond(stream: &mut impl Write, status: u16, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// `user:password` in Base64, as Basic authentication sends it.
fn basic(user: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{user}:{password}");
    let mut out = String::new();
    for chunk in input.as_bytes().chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    format!("Basic {out}")
}

fn etag(n: u64) -> String {
    format!("\"v{n}\"")
}

fn propstat_file(href: &str, bytes: &[u8], tag: u64) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>{}</d:getcontentlength><d:getetag>{}</d:getetag><d:getlastmodified>Fri, 23 Oct 2026 14:05:09 GMT</d:getlastmodified></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
        bytes.len(),
        etag(tag).replace('"', "&quot;")
    )
}

fn propstat_dir(href: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype><oc:permissions>RGDNVCK</oc:permissions></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

fn multistatus(inner: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">{inner}</d:multistatus>"#
    )
    .into_bytes()
}

fn serve(state: &Mutex<State>, stream: &mut (impl Read + Write)) -> Option<()> {
    let request = read_request(stream)?;
    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
    st.log.push(format!("{} {}", request.method, request.path));
    match st.hostile.clone() {
        Hostile::Status(status) => {
            respond(stream, status, &[], b"");
            return Some(());
        }
        Hostile::Redirect(location) if !request.path.starts_with(&location) => {
            respond(stream, 302, &[("Location", location)], b"");
            return Some(());
        }
        _ => {}
    }
    if request.headers.get("authorization") != Some(&basic(&st.user, &st.password)) {
        respond(stream, 401, &[("WWW-Authenticate", "Basic".into())], b"");
        return Some(());
    }
    let decoded = percent_decode_str(&request.path).decode_utf8().ok()?;
    let is_dir_request = decoded.ends_with('/');
    let path = decoded.trim_end_matches('/').to_owned();
    if !path.starts_with("/dav") {
        respond(stream, 404, &[], b"");
        return Some(());
    }
    let parent = path.rsplit_once('/').map(|(p, _)| p.to_owned())?;
    let precondition_holds = |st: &State| {
        if st.hostile == Hostile::IgnoreIfMatch {
            return true;
        }
        let current = st.files.get(&path).map(|(_, tag)| etag(*tag));
        if let Some(want) = request.headers.get("if-match") {
            return current.as_ref() == Some(want);
        }
        if request.headers.get("if-none-match").map(String::as_str) == Some("*") {
            return current.is_none();
        }
        true
    };
    let endless = match st.hostile {
        Hostile::EndlessGet(n) => Some(n),
        _ => None,
    };
    match request.method.as_str() {
        "PROPFIND" => {
            let depth = request.headers.get("depth").cloned().unwrap_or_default();
            let body = match st.hostile {
                Hostile::OversizedPropfind => {
                    let mut body = multistatus("");
                    body.resize(9 * 1024 * 1024, b' ');
                    body
                }
                Hostile::DocTypeBomb => {
                    let mut body = br#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol1 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;"><!ENTITY lol2 "&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;">]>"#.to_vec();
                    body.extend(multistatus(
                        "<d:response><d:href>&lol2;</d:href></d:response>",
                    ));
                    body
                }
                _ if st.dirs.contains(&path) => {
                    let mut inner = propstat_dir(&format!("{path}/"));
                    if depth == "1" {
                        let prefix = format!("{path}/");
                        let child_href = |name: &str| match &st.hostile {
                            Hostile::ForeignHref => format!("https://evil.example{prefix}{name}"),
                            Hostile::DotDotHref => format!("{prefix}../{name}"),
                            Hostile::EncodedSlashHref => format!("{prefix}x%2F{name}"),
                            _ => format!("{prefix}{name}"),
                        };
                        for dir in &st.dirs {
                            if let Some(name) =
                                dir.strip_prefix(&prefix).filter(|n| !n.contains('/'))
                            {
                                inner.push_str(&propstat_dir(&format!("{}/", child_href(name))));
                            }
                        }
                        for (file, (bytes, tag)) in &st.files {
                            if let Some(name) =
                                file.strip_prefix(&prefix).filter(|n| !n.contains('/'))
                            {
                                inner.push_str(&propstat_file(&child_href(name), bytes, *tag));
                            }
                        }
                    }
                    multistatus(&inner)
                }
                _ => match st.files.get(&path) {
                    Some((bytes, tag)) => multistatus(&propstat_file(&path, bytes, *tag)),
                    None => {
                        respond(stream, 404, &[], b"");
                        return Some(());
                    }
                },
            };
            respond(
                stream,
                207,
                &[("Content-Type", "application/xml".into())],
                &body,
            );
        }
        "GET" => match st.files.get(&path) {
            Some(_) if endless.is_some() => {
                let n = endless.unwrap_or_default();
                drop(st);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(&vec![b'x'; n]);
                let _ = stream.flush();
            }
            Some((bytes, tag)) => respond(stream, 200, &[("ETag", etag(*tag))], bytes),
            None => respond(stream, 404, &[], b""),
        },
        "PUT" => {
            if st.dirs.contains(&path) || is_dir_request {
                respond(stream, 405, &[], b"");
            } else if !st.dirs.contains(&parent) {
                respond(stream, 409, &[], b"");
            } else if !precondition_holds(&st) {
                respond(stream, 412, &[], b"");
            } else {
                st.next += 1;
                let tag = st.next;
                let created = st.files.insert(path, (request.body, tag)).is_none();
                // A body with 201, as `rclone serve webdav` and Apache send.
                respond(
                    stream,
                    if created { 201 } else { 204 },
                    &[("ETag", etag(tag))],
                    if created { b"Created" } else { b"" },
                );
            }
        }
        "MKCOL" => {
            if st.dirs.contains(&path) || st.files.contains_key(&path) {
                respond(stream, 405, &[], b"");
            } else if !st.dirs.contains(&parent) {
                respond(stream, 409, &[], b"");
            } else {
                st.dirs.insert(path);
                respond(stream, 201, &[], b"Created");
            }
        }
        "DELETE" => {
            if st.dirs.remove(&path) {
                let prefix = format!("{path}/");
                st.files.retain(|p, _| !p.starts_with(&prefix));
                st.dirs.retain(|p| !p.starts_with(&prefix));
                respond(stream, 204, &[], b"");
            } else if !st.files.contains_key(&path) {
                respond(stream, 404, &[], b"");
            } else if !precondition_holds(&st) {
                respond(stream, 412, &[], b"");
            } else {
                st.files.remove(&path);
                respond(stream, 204, &[], b"");
            }
        }
        _ => respond(stream, 405, &[], b""),
    }
    Some(())
}
