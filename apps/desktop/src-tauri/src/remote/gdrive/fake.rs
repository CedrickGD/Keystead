//! An in-process fake of Google's OAuth and Drive v3 for the unit tests:
//! the token endpoint (code + PKCE verifier, refresh tokens), revoke,
//! `about`, `files.list` with the queries [`super::GdriveRemote`] sends
//! (parent, name, folder type, not trashed) and paging, `alt=media`,
//! multipart create, media update, folder create and delete – and faults
//! (expired and revoked tokens, an error for the next write, a file of the
//! same name that another device makes just before ours, hostile names).
//! Plain HTTP on 127.0.0.1; `/oauth/…` and `/api/…` stand for the two
//! hosts.

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard};

use reqwest::Url;
use serde_json::{json, Value};

use super::{Endpoints, FOLDER_MIME, FOLDER_NAME};
use crate::remote::test_server::{read_request, respond};

const CODE: &str = "4/the-code-google-sent";
const VERIFIER: &str = "the-verifier";
const PERMISSION_ID: &str = "perm-1";

#[derive(Debug, Clone)]
struct File {
    name: String,
    parent: String,
    folder: bool,
    bytes: Vec<u8>,
    version: u64,
    created: u64,
}

#[derive(Default)]
struct State {
    refresh: BTreeSet<String>,
    access: BTreeSet<String>,
    next: u64,
    files: BTreeMap<String, File>,
    page_size: Option<usize>,
    fail_next_write: Option<(u16, String)>,
    before_create: Option<(String, Vec<u8>)>,
}

impl State {
    fn tick(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn token(&mut self) -> String {
        format!("tok-{}", self.tick())
    }

    fn insert(&mut self, name: &str, parent: &str, folder: bool, bytes: &[u8]) -> String {
        let id = format!("id{}", self.tick());
        let tick = self.tick();
        self.files.insert(
            id.clone(),
            File {
                name: name.to_owned(),
                parent: parent.to_owned(),
                folder,
                bytes: bytes.to_vec(),
                version: tick,
                created: tick,
            },
        );
        id
    }

    fn keystead(&self) -> Option<String> {
        self.files
            .iter()
            .filter(|(_, f)| f.parent == "root" && f.folder && f.name == FOLDER_NAME)
            .min_by_key(|(_, f)| f.created)
            .map(|(id, _)| id.clone())
    }

    fn remove(&mut self, id: &str) {
        self.files.remove(id);
        let children: Vec<String> = self
            .files
            .iter()
            .filter(|(_, f)| f.parent == id)
            .map(|(id, _)| id.clone())
            .collect();
        for child in children {
            self.remove(&child);
        }
    }
}

pub struct FakeDrive {
    base: String,
    state: Arc<Mutex<State>>,
}

fn created_time(tick: u64) -> String {
    format!(
        "2026-10-29T{:02}:{:02}:{:02}.000Z",
        tick / 3600 % 24,
        tick / 60 % 60,
        tick % 60
    )
}

fn meta(id: &str, f: &File) -> Value {
    let mut v = json!({
        "id": id,
        "name": f.name,
        "mimeType": if f.folder { FOLDER_MIME } else { "application/octet-stream" },
        "version": f.version.to_string(),
        "createdTime": created_time(f.created),
    });
    if !f.folder {
        v["size"] = json!(f.bytes.len().to_string());
    }
    v
}

fn form(body: &[u8]) -> BTreeMap<String, String> {
    let url = Url::parse(&format!("form:/?{}", String::from_utf8_lossy(body))).unwrap();
    url.query_pairs().into_owned().collect()
}

/// The clauses of a Drive query (`… and …`, quoted strings unescaped).
fn clauses(q: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in split_and(q) {
        let part = part.trim();
        if let Some(parent) = part.strip_suffix(" in parents") {
            out.push(("parent".into(), unquote(parent)));
        } else if let Some((field, value)) = part.split_once('=') {
            out.push((field.trim().into(), unquote(value.trim())));
        } else {
            panic!("unexpected clause {part}");
        }
    }
    out
}

fn split_and(q: &str) -> Vec<String> {
    let (mut parts, mut current, mut quoted, mut escaped) =
        (Vec::new(), String::new(), false, false);
    let mut chars = q.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if escaped {
            escaped = false;
        } else if c == '\\' && quoted {
            escaped = true;
        } else if c == '\'' {
            quoted = !quoted;
        } else if !quoted && current.ends_with(" and ") {
            current.truncate(current.len() - 5);
            parts.push(std::mem::take(&mut current));
        }
        let _ = chars.peek();
    }
    parts.push(current);
    parts
}

fn unquote(text: &str) -> String {
    let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) else {
        return text.to_owned();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// The metadata and the content of a `multipart/related` upload.
fn multipart(content_type: &str, body: &[u8]) -> (Value, Vec<u8>) {
    let boundary = content_type.split_once("boundary=").unwrap().1;
    let delimiter = format!("\r\n--{boundary}");
    let first_head = find(body, b"\r\n\r\n", 0).unwrap() + 4;
    let first_end = find(body, delimiter.as_bytes(), first_head).unwrap();
    let metadata: Value = serde_json::from_slice(&body[first_head..first_end]).unwrap();
    let second_head = find(body, b"\r\n\r\n", first_end + delimiter.len()).unwrap() + 4;
    let second_end = find(body, format!("{delimiter}--").as_bytes(), second_head).unwrap();
    (metadata, body[second_head..second_end].to_vec())
}

impl FakeDrive {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                let state = Arc::clone(&shared);
                std::thread::spawn(move || serve(&state, &mut stream));
            }
        });
        FakeDrive { base, state }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn endpoints(&self) -> Endpoints {
        Endpoints {
            oauth: Url::parse(&format!("{}oauth/", self.base)).unwrap(),
            api: Url::parse(&format!("{}api/", self.base)).unwrap(),
        }
    }

    pub fn code(&self) -> String {
        CODE.into()
    }

    pub fn verifier(&self) -> String {
        VERIFIER.into()
    }

    pub fn has_folder(&self) -> bool {
        self.state().keystead().is_some()
    }

    /// A file of Keystead's folder, as another device wrote it (the folder
    /// appears).
    pub fn add_file(&self, name: &str, bytes: &[u8]) {
        let mut st = self.state();
        let folder = match st.keystead() {
            Some(id) => id,
            None => st.insert(FOLDER_NAME, "root", true, b""),
        };
        st.insert(name, &folder, false, bytes);
    }

    /// The (oldest) file `name` of Keystead's folder.
    pub fn file(&self, name: &str) -> Option<Vec<u8>> {
        let st = self.state();
        let folder = st.keystead()?;
        st.files
            .values()
            .filter(|f| f.parent == folder && !f.folder && f.name == name)
            .min_by_key(|f| f.created)
            .map(|f| f.bytes.clone())
    }

    /// How many files of Keystead's folder are called `name`.
    pub fn count(&self, name: &str) -> usize {
        let st = self.state();
        let Some(folder) = st.keystead() else {
            return 0;
        };
        st.files
            .values()
            .filter(|f| f.parent == folder && f.name == name)
            .count()
    }

    pub fn remove_folder(&self) {
        let mut st = self.state();
        if let Some(id) = st.keystead() {
            st.remove(&id);
        }
    }

    pub fn expire_access_tokens(&self) {
        self.state().access.clear();
    }

    pub fn revoke_refresh_tokens(&self) {
        self.state().refresh.clear();
    }

    pub fn refresh_tokens(&self) -> usize {
        self.state().refresh.len()
    }

    pub fn set_page_size(&self, n: usize) {
        self.state().page_size = Some(n);
    }

    /// The next write (create, update, delete) fails with Google's error
    /// `reason`.
    pub fn fail_next(&self, status: u16, reason: &str) {
        self.state().fail_next_write = Some((status, reason.to_owned()));
    }

    /// Another device makes `name` just before the next create of that name
    /// lands (it is older).
    pub fn before_create(&self, name: &str, bytes: &[u8]) {
        self.state().before_create = Some((name.to_owned(), bytes.to_vec()));
    }
}

fn error(status: u16, reason: &str) -> Value {
    json!({"error": {"code": status, "message": "Google's text", "errors": [{"reason": reason}]}})
}

fn serve(state: &Mutex<State>, stream: &mut std::net::TcpStream) {
    let Some(request) = read_request(stream) else {
        return;
    };
    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
    let url = Url::parse(&format!("http://fake{}", request.path)).unwrap();
    let query: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
    let path = url.path().to_owned();
    let (status, body): (u16, Value) = 'answer: {
        if path == "/oauth/token" {
            let f = form(&request.body);
            break 'answer match f.get("grant_type").map(String::as_str) {
                Some("authorization_code")
                    if f.get("code").map(String::as_str) == Some(CODE)
                        && f.get("code_verifier").map(String::as_str) == Some(VERIFIER)
                        && f.contains_key("redirect_uri") =>
                {
                    let (refresh, access) = (st.token(), st.token());
                    st.refresh.insert(refresh.clone());
                    st.access.insert(access.clone());
                    (
                        200,
                        json!({"access_token": access, "expires_in": 3599, "refresh_token": refresh,
                        "scope": super::SCOPE, "token_type": "Bearer"}),
                    )
                }
                Some("refresh_token")
                    if f.get("refresh_token")
                        .is_some_and(|t| st.refresh.contains(t)) =>
                {
                    let access = st.token();
                    st.access.insert(access.clone());
                    (200, json!({"access_token": access, "expires_in": 3599}))
                }
                _ => (400, json!({"error": "invalid_grant"})),
            };
        }
        if path == "/oauth/revoke" {
            let token = form(&request.body).remove("token").unwrap_or_default();
            st.refresh.remove(&token);
            break 'answer (200, json!({}));
        }
        let token = request
            .headers
            .get("authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .unwrap_or_default();
        if !st.access.contains(token) {
            break 'answer (401, error(401, "authError"));
        }
        if request.method != "GET" {
            if let Some((status, reason)) = st.fail_next_write.take() {
                break 'answer (status, error(status, &reason));
            }
        }
        let file_id = path
            .strip_prefix("/api/drive/v3/files/")
            .or_else(|| path.strip_prefix("/api/upload/drive/v3/files/"))
            .map(str::to_owned);
        match (request.method.as_str(), path.as_str(), file_id) {
            ("GET", "/api/drive/v3/about", _) => (
                200,
                json!({"user": {"permissionId": PERMISSION_ID, "displayName": "Max Muster",
                    "emailAddress": "max@example.com"}}),
            ),
            ("GET", "/api/drive/v3/files", _) => {
                let wanted = clauses(&query["q"]);
                let mut hits: Vec<(&String, &File)> = st
                    .files
                    .iter()
                    .filter(|(_, f)| {
                        wanted.iter().all(|(field, value)| match field.as_str() {
                            "parent" => &f.parent == value,
                            "name" => &f.name == value,
                            "mimeType" => (value == FOLDER_MIME) == f.folder,
                            "trashed" => value == "false",
                            other => panic!("unexpected field {other}"),
                        })
                    })
                    .collect();
                assert_eq!(
                    query.get("orderBy").map(String::as_str),
                    Some("createdTime")
                );
                hits.sort_by_key(|(_, f)| f.created);
                let size = st
                    .page_size
                    .unwrap_or_else(|| query["pageSize"].parse().unwrap());
                let offset: usize = query
                    .get("pageToken")
                    .map(|t| t.parse().unwrap())
                    .unwrap_or(0);
                let page: Vec<Value> = hits
                    .iter()
                    .skip(offset)
                    .take(size)
                    .map(|(id, f)| meta(id, f))
                    .collect();
                let mut v = json!({"files": page});
                if offset + size < hits.len() {
                    v["nextPageToken"] = json!((offset + size).to_string());
                }
                (200, v)
            }
            ("GET", _, Some(id)) if query.get("alt").map(String::as_str) == Some("media") => {
                match st.files.get(&id) {
                    Some(f) if !f.folder => {
                        let bytes = f.bytes.clone();
                        drop(st);
                        respond(stream, 200, &[], &bytes);
                        return;
                    }
                    _ => (404, error(404, "notFound")),
                }
            }
            ("POST", "/api/drive/v3/files", _) => {
                let v: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(v["mimeType"], FOLDER_MIME);
                let parent = v["parents"][0].as_str().unwrap().to_owned();
                let id = st.insert(v["name"].as_str().unwrap(), &parent, true, b"");
                (200, json!({"id": id}))
            }
            ("POST", "/api/upload/drive/v3/files", _) => {
                assert_eq!(
                    query.get("uploadType").map(String::as_str),
                    Some("multipart")
                );
                let content_type = request
                    .headers
                    .get("content-type")
                    .cloned()
                    .unwrap_or_default();
                let (metadata, bytes) = multipart(&content_type, &request.body);
                let name = metadata["name"].as_str().unwrap().to_owned();
                let parent = metadata["parents"][0].as_str().unwrap().to_owned();
                if !st.files.get(&parent).is_some_and(|f| f.folder) {
                    break 'answer (404, error(404, "notFound"));
                }
                if let Some((other, other_bytes)) = st.before_create.take() {
                    if other == name {
                        st.insert(&name, &parent, false, &other_bytes);
                    } else {
                        st.before_create = Some((other, other_bytes));
                    }
                }
                let id = st.insert(&name, &parent, false, &bytes);
                (200, meta(&id, &st.files[&id]))
            }
            ("PATCH", _, Some(id)) => {
                assert_eq!(query.get("uploadType").map(String::as_str), Some("media"));
                let tick = st.tick();
                match st.files.get_mut(&id) {
                    Some(f) if !f.folder => {
                        f.bytes = request.body.clone();
                        f.version = tick;
                        let f = f.clone();
                        (200, meta(&id, &f))
                    }
                    _ => (404, error(404, "notFound")),
                }
            }
            ("DELETE", _, Some(id)) => {
                if st.files.contains_key(&id) {
                    st.remove(&id);
                    drop(st);
                    respond(stream, 204, &[], b"");
                    return;
                }
                (404, error(404, "notFound"))
            }
            _ => (404, error(404, "notFound")),
        }
    };
    drop(st);
    respond(
        stream,
        status,
        &[("Content-Type", "application/json".to_owned())],
        body.to_string().as_bytes(),
    );
}
