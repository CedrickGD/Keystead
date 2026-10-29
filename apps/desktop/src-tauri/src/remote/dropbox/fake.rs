//! An in-process fake of Dropbox's API for the unit tests: the token
//! endpoint (code + PKCE verifier, refresh tokens), `list_folder` with
//! paging, `download`, `upload` with the write modes, `create_folder_v2`,
//! `delete_v2`, the account and revoke – and faults (expired and revoked
//! tokens, an error for the next call, oversized and endless listings,
//! hostile names). Plain HTTP on 127.0.0.1; `/api/…` and `/content/…` stand
//! for the two hosts.

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard};

use reqwest::Url;
use serde_json::{json, Value};

use super::Endpoints;
use crate::remote::test_server::{read_request, respond};

const ACCOUNT: &str = "dbid:test-account";
const CODE: &str = "the-code-dropbox-showed";
const VERIFIER: &str = "the-verifier";

#[derive(Default)]
struct State {
    refresh: BTreeSet<String>,
    access: BTreeSet<String>,
    next_token: u64,
    token_requests: u32,
    /// Lower-case path → (name, bytes, rev).
    files: BTreeMap<String, (String, Vec<u8>, u64)>,
    folders: BTreeMap<String, String>,
    next_rev: u64,
    raw: Vec<Value>,
    page_size: usize,
    fail_next: Option<(u16, Value)>,
    oversize_next: bool,
    too_many: bool,
}

pub struct FakeDropbox {
    base: String,
    state: Arc<Mutex<State>>,
}

fn json_answer(stream: &mut impl std::io::Write, status: u16, v: &Value) {
    respond(
        stream,
        status,
        &[("Content-Type", "application/json".into())],
        v.to_string().as_bytes(),
    );
}

fn form(body: &[u8]) -> BTreeMap<String, String> {
    let url = Url::parse(&format!("form:/?{}", String::from_utf8_lossy(body))).unwrap();
    url.query_pairs().into_owned().collect()
}

fn parent(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(p, _)| p.to_owned())
        .unwrap_or_default()
}

fn name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

impl State {
    fn token(&mut self) -> String {
        self.next_token += 1;
        format!("tok-{}", self.next_token)
    }

    fn rev(&mut self) -> String {
        self.next_rev += 1;
        format!("{:x}", 0x1000 + self.next_rev)
    }
}

impl FakeDropbox {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            page_size: 1000,
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                let state = Arc::clone(&shared);
                std::thread::spawn(move || serve(&state, &mut stream));
            }
        });
        FakeDropbox { base, state }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn endpoints(&self) -> Endpoints {
        Endpoints {
            api: Url::parse(&format!("{}api/", self.base)).unwrap(),
            content: Url::parse(&format!("{}content/", self.base)).unwrap(),
        }
    }

    pub fn code(&self) -> String {
        CODE.into()
    }

    pub fn verifier(&self) -> String {
        VERIFIER.into()
    }

    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        let key = format!("/{path}").to_lowercase();
        self.state()
            .files
            .get(&key)
            .map(|(_, bytes, _)| bytes.clone())
    }

    /// Writes `path` directly, as another device (its folder appears).
    pub fn write(&self, path: &str, bytes: &[u8]) {
        let mut st = self.state();
        let full = format!("/{path}");
        let dir = parent(&full);
        if !dir.is_empty() {
            st.folders.insert(dir.to_lowercase(), name(&dir));
        }
        let rev = st.rev();
        st.files.insert(
            full.to_lowercase(),
            (
                name(&full),
                bytes.to_vec(),
                u64::from_str_radix(&rev, 16).unwrap(),
            ),
        );
    }

    pub fn token_requests(&self) -> u32 {
        self.state().token_requests
    }

    pub fn expire_access_tokens(&self) {
        self.state().access.clear();
    }

    pub fn revoke_refresh_tokens(&self) {
        self.state().refresh.clear();
    }

    pub fn add_raw_entry(&self, entry: Value) {
        self.state().raw.push(entry);
    }

    pub fn set_page_size(&self, n: usize) {
        self.state().page_size = n;
    }

    pub fn fail_next(&self, status: u16, body: Value) {
        self.state().fail_next = Some((status, body));
    }

    pub fn oversize_next_listing(&self) {
        self.state().oversize_next = true;
    }

    pub fn too_many_entries(&self) {
        self.state().too_many = true;
    }
}

fn serve(state: &Mutex<State>, stream: &mut std::net::TcpStream) {
    let Some(request) = read_request(stream) else {
        return;
    };
    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
    let path = request.path.as_str();
    if path == "/api/oauth2/token" {
        let f = form(&request.body);
        match f.get("grant_type").map(String::as_str) {
            Some("authorization_code")
                if f.get("code").map(String::as_str) == Some(CODE)
                    && f.get("code_verifier").map(String::as_str) == Some(VERIFIER) =>
            {
                let (refresh, access) = (st.token(), st.token());
                st.refresh.insert(refresh.clone());
                st.access.insert(access.clone());
                let v = json!({"access_token": access, "token_type": "bearer", "expires_in": 14400,
                    "refresh_token": refresh, "account_id": ACCOUNT, "uid": "1"});
                drop(st);
                json_answer(stream, 200, &v);
            }
            Some("refresh_token")
                if f.get("refresh_token")
                    .is_some_and(|t| st.refresh.contains(t)) =>
            {
                st.token_requests += 1;
                let access = st.token();
                st.access.insert(access.clone());
                drop(st);
                json_answer(
                    stream,
                    200,
                    &json!({"access_token": access, "expires_in": 14400}),
                );
            }
            _ => {
                drop(st);
                json_answer(stream, 400, &json!({"error": "invalid_grant"}));
            }
        }
        return;
    }
    let token = request
        .headers
        .get("authorization")
        .and_then(|a| a.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !st.access.contains(token) {
        drop(st);
        json_answer(
            stream,
            401,
            &json!({"error_summary": "expired_access_token/", "error": {".tag": "expired_access_token"}}),
        );
        return;
    }
    if let Some((status, body)) = st.fail_next.take() {
        drop(st);
        json_answer(stream, status, &body);
        return;
    }
    let arg: Value = request
        .headers
        .get("dropbox-api-arg")
        .map(|a| serde_json::from_str(a).unwrap())
        .unwrap_or_else(|| serde_json::from_slice(&request.body).unwrap_or(Value::Null));
    let target = arg["path"].as_str().unwrap_or_default().to_owned();
    let key = target.to_lowercase();
    let not_found = json!({"error_summary": "path/not_found/..", "error": {".tag": "path"}});
    let conflict = json!({"error_summary": "path/conflict/file/..", "error": {".tag": "path"}});
    let (status, body, extra): (u16, Vec<u8>, Option<(String, String)>) = match path {
        "/api/2/users/get_current_account" => (
            200,
            json!({"account_id": ACCOUNT, "name": {"display_name": "Max Muster"}, "email": "max@example.com"})
                .to_string()
                .into_bytes(),
            None,
        ),
        "/api/2/auth/token/revoke" => {
            st.access.remove(token);
            (200, b"null".to_vec(), None)
        }
        "/api/2/files/list_folder" | "/api/2/files/list_folder/continue" => {
            let (dir, offset) = if path.ends_with("continue") {
                let cursor = arg["cursor"].as_str().unwrap_or_default();
                let (dir, offset) = cursor.rsplit_once('|').unwrap();
                (dir.to_owned(), offset.parse::<usize>().unwrap())
            } else {
                (key.clone(), 0)
            };
            if !dir.is_empty() && !st.folders.contains_key(&dir) {
                (409, not_found.to_string().into_bytes(), None)
            } else {
                let mut entries: Vec<Value> = st
                    .folders
                    .iter()
                    .filter(|(p, _)| parent(p) == dir)
                    .map(|(_, n)| json!({".tag": "folder", "name": n}))
                    .chain(st.files.iter().filter(|(p, _)| parent(p) == dir).map(
                        |(_, (n, bytes, rev))| {
                            json!({".tag": "file", "name": n, "rev": format!("{rev:x}"), "size": bytes.len()})
                        },
                    ))
                    .collect();
                if dir.is_empty() {
                    entries.extend(st.raw.iter().cloned());
                }
                // Every page of every listing from now on (the test's last).
                if st.too_many {
                    entries = (0..10_001)
                        .map(|i| json!({".tag": "file", "name": format!("f{i}"), "rev": "1", "size": 1}))
                        .collect();
                }
                let page: Vec<Value> = entries.iter().skip(offset).take(st.page_size).cloned().collect();
                let more = offset + page.len() < entries.len();
                let mut text = json!({"entries": page, "cursor": format!("{dir}|{}", offset + st.page_size),
                    "has_more": more})
                .to_string();
                if st.oversize_next {
                    st.oversize_next = false;
                    text.push_str(&" ".repeat(9 * 1024 * 1024));
                }
                (200, text.into_bytes(), None)
            }
        }
        "/content/2/files/download" => match st.files.get(&key) {
            Some((n, bytes, rev)) => {
                let result = json!({"name": n, "rev": format!("{rev:x}"), "size": bytes.len()});
                (200, bytes.clone(), Some(("Dropbox-API-Result".into(), result.to_string())))
            }
            None => (409, not_found.to_string().into_bytes(), None),
        },
        "/content/2/files/upload" => {
            let current = st.files.get(&key).map(|(_, _, rev)| format!("{rev:x}"));
            let allowed = match &arg["mode"] {
                Value::String(m) if m == "overwrite" => true,
                Value::String(m) if m == "add" => current.is_none(),
                m if m[".tag"] == "update" => current.as_deref() == m["update"].as_str(),
                _ => false,
            };
            if allowed {
                let rev = st.rev();
                let dir = parent(&target);
                if !dir.is_empty() {
                    st.folders.entry(dir.to_lowercase()).or_insert_with(|| name(&dir));
                }
                st.files.insert(
                    key.clone(),
                    (name(&target), request.body.clone(), u64::from_str_radix(&rev, 16).unwrap()),
                );
                (
                    200,
                    json!({"name": name(&target), "rev": rev, "size": request.body.len()})
                        .to_string()
                        .into_bytes(),
                    None,
                )
            } else {
                (409, conflict.to_string().into_bytes(), None)
            }
        }
        "/api/2/files/create_folder_v2" => {
            if st.folders.contains_key(&key) {
                (
                    409,
                    json!({"error_summary": "path/conflict/folder/.."}).to_string().into_bytes(),
                    None,
                )
            } else {
                st.folders.insert(key.clone(), name(&target));
                (200, json!({"metadata": {"name": name(&target)}}).to_string().into_bytes(), None)
            }
        }
        "/api/2/files/delete_v2" => {
            if let Some((_, _, rev)) = st.files.get(&key) {
                if arg["parent_rev"].as_str().is_some_and(|p| p != format!("{rev:x}")) {
                    (409, conflict.to_string().into_bytes(), None)
                } else {
                    st.files.remove(&key);
                    (200, json!({"metadata": {}}).to_string().into_bytes(), None)
                }
            } else if st.folders.remove(&key).is_some() {
                let prefix = format!("{key}/");
                st.files.retain(|p, _| !p.starts_with(&prefix));
                (200, json!({"metadata": {}}).to_string().into_bytes(), None)
            } else {
                (
                    409,
                    json!({"error_summary": "path_lookup/not_found/.."}).to_string().into_bytes(),
                    None,
                )
            }
        }
        _ => (404, Vec::new(), None),
    };
    drop(st);
    let mut headers = vec![("Content-Type", "application/json".to_owned())];
    if let Some((_, value)) = &extra {
        headers.push(("Dropbox-API-Result", value.clone()));
    }
    respond(stream, status, &headers, &body);
}
