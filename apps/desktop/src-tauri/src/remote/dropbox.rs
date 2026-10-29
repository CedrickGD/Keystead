//! Dropbox as a [`Remote`] (stage 2d): Keystead's own **app folder**
//! (`/Apps/Keystead` – the app sees nothing else of the account) through
//! Dropbox's HTTP API v2.
//!
//! * **Login**: OAuth 2 code flow with PKCE and no redirect – the user opens
//!   [`authorize_url`] in the browser, allows the app and copies the code
//!   Dropbox shows into Keystead; [`connect`] exchanges it for a short-lived
//!   access token and a **refresh token** (`token_access_type=offline`).
//!   No app secret: the app key is public ([`APP_KEY`], compiled in from
//!   `KEYSTEAD_DROPBOX_APP_KEY`; without one Dropbox is not offered). The
//!   refresh token is the secret – stored like a server password
//!   (`crate::credentials`, key [`key_of`]); the access token lives in
//!   memory and is renewed when Dropbox says it expired.
//! * **Paths**: the layout's relative paths (`remote::check_path`) at the
//!   app folder's root (`"/<path>"`). The ETag is the file's `rev`; writes
//!   are conditional (`files/upload` mode `update` with that rev for
//!   `IfMatch`, `add` for `IfNoneMatch`, `strict_conflict`, no autorename) –
//!   a conflict is `sync_remote_changed`, so `cas` is true.
//! * **Answers are untrusted**: JSON at most [`LIST_BODY_MAX`] per page,
//!   ≤ [`LIST_ENTRIES_MAX`] entries, names of one segment only (no `/`,
//!   `\`, NUL, `.`, `..`), the layout checks the names again; downloads
//!   capped like WebDAV's. Errors are codes (`super`), never Dropbox's
//!   texts (they could name a path): 401 after one renewal →
//!   `sync_auth_failed`, `not_found` → `sync_file_missing`, a conflict →
//!   `sync_remote_changed`, `insufficient_space` → `sync_server_quota`, no
//!   write permission → `sync_not_writable`, 429 and 5xx →
//!   `sync_server_error`, a malformed answer → `sync_server_response`.
//! * Transport, TLS (the system's roots / webpki roots, no pin) and the
//!   request ceilings: [`super::http`]. Never call it from an async task.

use std::sync::Mutex;

use base64::Engine as _;
use reqwest::header::{HeaderValue, CONTENT_TYPE};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use super::http::{Answer, Http};
use super::tls::Trust;
use super::{
    check_dir, check_path, Access, Precondition, Remote, RemoteMeta, AUTH_FAILED, BAD_RESPONSE,
    CHANGED, FILE_MISSING, NOT_WRITABLE, QUOTA, SERVER_ERROR, TOO_LARGE,
};
use crate::error::{AppError, AppResult};

/// The app key of Keystead's Dropbox app (public – PKCE, no secret), from
/// the build's environment; `None` until the app is registered.
pub const APP_KEY: Option<&str> = option_env!("KEYSTEAD_DROPBOX_APP_KEY");
/// Where the user allows the app.
const AUTHORIZE_URL: &str = "https://www.dropbox.com/oauth2/authorize";
/// `unsupported:dropbox_not_configured`: this build has no app key.
pub const NOT_CONFIGURED: &str = "dropbox_not_configured";
/// `invalid_input:dropbox_code`: not a code Dropbox gave (or it expired).
pub const BAD_CODE: &str = "dropbox_code";
/// A page of a listing (or another JSON answer) may be this large.
pub const LIST_BODY_MAX: u64 = 8 * 1024 * 1024;
/// A folder may list this many entries (pages included).
pub const LIST_ENTRIES_MAX: usize = 10_000;

/// The app key, `unsupported:dropbox_not_configured` without one (the unit
/// tests have one: the fake takes any).
pub fn app_key() -> AppResult<&'static str> {
    if cfg!(test) {
        return Ok("app-key");
    }
    APP_KEY
        .filter(|key| !key.is_empty())
        .ok_or_else(|| AppError::unsupported(NOT_CONFIGURED))
}

#[cfg(test)]
thread_local! {
    /// The unit tests' fake for the code on this thread (the engines run
    /// on the test's thread).
    pub static TEST_ENDPOINTS: std::cell::RefCell<Option<Endpoints>> =
        const { std::cell::RefCell::new(None) };
}

/// What the bases, the link and the refresh token are recorded for.
pub fn key_of(account_id: &str) -> String {
    format!("dropbox:{account_id}")
}

/// Dropbox's two hosts (the unit tests point them at a fake).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// `https://api.dropboxapi.com/` – RPC calls and the token.
    pub api: Url,
    /// `https://content.dropboxapi.com/` – downloads and uploads.
    pub content: Url,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            api: Url::parse("https://api.dropboxapi.com/").expect("constant URL"),
            content: Url::parse("https://content.dropboxapi.com/").expect("constant URL"),
        }
    }
}

impl Endpoints {
    /// Dropbox's (the unit tests: their fake's).
    pub fn current() -> Self {
        #[cfg(test)]
        if let Some(endpoints) = TEST_ENDPOINTS.with(|e| e.borrow().clone()) {
            return endpoints;
        }
        Endpoints::default()
    }

    fn api(&self, route: &str) -> Url {
        self.api.join(route).expect("relative route")
    }
    fn content(&self, route: &str) -> Url {
        self.content.join(route).expect("relative route")
    }
}

/// One login's PKCE pair: the verifier stays in the backend, the challenge
/// goes into [`authorize_url`].
pub struct Pkce {
    pub verifier: Zeroizing<String>,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> AppResult<Self> {
        let random = keystead_core::crypto::random_array::<32>()?;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let verifier = Zeroizing::new(b64.encode(random));
        let challenge = b64.encode(Sha256::digest(verifier.as_bytes()));
        Ok(Pkce {
            verifier,
            challenge,
        })
    }
}

/// The page where the user allows Keystead's app folder (no redirect:
/// Dropbox shows the code to copy).
pub fn authorize_url(app_key: &str, challenge: &str) -> String {
    let mut url = Url::parse(AUTHORIZE_URL).expect("constant URL");
    url.query_pairs_mut()
        .append_pair("client_id", app_key)
        .append_pair("response_type", "code")
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("token_access_type", "offline");
    url.to_string()
}

/// Who the account is (shown in the settings; no secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropboxAccount {
    pub account_id: String,
    pub name: String,
    pub email: String,
}

/// What a code brought: the refresh token (the secret) and the account.
pub struct Connected {
    pub refresh_token: Zeroizing<String>,
    pub account: DropboxAccount,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connected")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// A form body (`application/x-www-form-urlencoded`, built by `Url`'s own
/// query serializer).
fn form(pairs: &[(&str, &str)]) -> Zeroizing<String> {
    let mut url = Url::parse("form:/").expect("constant URL");
    {
        let mut query = url.query_pairs_mut();
        for (name, value) in pairs {
            query.append_pair(name, value);
        }
    }
    Zeroizing::new(url.query().unwrap_or_default().to_owned())
}

fn post_form(http: &Http, url: Url, body: &Zeroizing<String>) -> AppResult<Answer> {
    let request = http
        .request(Method::POST, url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body.as_bytes().to_vec());
    http.send(request, body.len() as u64, LIST_BODY_MAX, BAD_RESPONSE)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> AppResult<T> {
    serde_json::from_slice(body).map_err(|_| AppError::unsupported(BAD_RESPONSE))
}

/// Exchanges the code the user copied for the tokens and asks whose
/// account it is.
pub fn connect(
    app_key: &str,
    code: &str,
    verifier: &str,
    endpoints: &Endpoints,
) -> AppResult<Connected> {
    let code = code.trim();
    if code.is_empty() || code.len() > 512 || !code.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(AppError::invalid(BAD_CODE));
    }
    let http = Http::new(Trust::System)?;
    let body = form(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", app_key),
        ("code_verifier", verifier),
    ]);
    let answer = post_form(&http, endpoints.api("oauth2/token"), &body)?;
    if answer.status == StatusCode::BAD_REQUEST {
        return Err(AppError::invalid(BAD_CODE));
    }
    if !answer.status.is_success() {
        return Err(rpc_error(&answer, Op::Read));
    }
    let tokens: TokenAnswer = parse(&answer.body)?;
    let refresh = tokens
        .refresh_token
        .map(Zeroizing::new)
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    let access = Zeroizing::new(tokens.access_token);
    let account = current_account(&http, endpoints, &access)?;
    Ok(Connected {
        refresh_token: refresh,
        account,
    })
}

fn current_account(
    http: &Http,
    endpoints: &Endpoints,
    access: &Zeroizing<String>,
) -> AppResult<DropboxAccount> {
    let request = http
        .request(Method::POST, endpoints.api("2/users/get_current_account"))
        .bearer_auth(access.as_str())
        .header(CONTENT_TYPE, "application/json")
        .body("null");
    let answer = http.send(request, 4, LIST_BODY_MAX, BAD_RESPONSE)?;
    if !answer.status.is_success() {
        return Err(rpc_error(&answer, Op::Read));
    }
    let v: Value = parse(&answer.body)?;
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let account_id = text(&v["account_id"])
        .filter(|id| !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    Ok(DropboxAccount {
        account_id,
        name: text(&v["name"]["display_name"]).unwrap_or_default(),
        email: text(&v["email"]).unwrap_or_default(),
    })
}

/// What a call is, for the error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Read,
    Write,
}

/// The code of an answer that is not a success (Dropbox's own texts are
/// only matched, never passed on).
fn rpc_error(answer: &Answer, op: Op) -> AppError {
    let summary = serde_json::from_slice::<Value>(&answer.body)
        .ok()
        .and_then(|v| v["error_summary"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let code = match answer.status.as_u16() {
        401 => AUTH_FAILED,
        403 if op == Op::Write => NOT_WRITABLE,
        403 => AUTH_FAILED,
        409 if summary.contains("insufficient_space") => QUOTA,
        409 if summary.contains("conflict") => CHANGED,
        409 if summary.contains("not_found") => FILE_MISSING,
        409 if summary.contains("no_write_permission")
            || summary.contains("disallowed_name")
            || summary.contains("malformed_path") =>
        {
            NOT_WRITABLE
        }
        409 if summary.contains("too_large") => TOO_LARGE,
        400 => BAD_RESPONSE,
        _ => SERVER_ERROR,
    };
    AppError::unsupported(code)
}

/// The JSON of the `Dropbox-API-Arg` header (ASCII only, as Dropbox wants
/// it; the layout's paths are).
fn api_arg(v: &Value) -> AppResult<HeaderValue> {
    let text = v.to_string();
    if !text.is_ascii() {
        return Err(AppError::invalid(super::BAD_PATH));
    }
    HeaderValue::from_str(&text).map_err(|_| AppError::invalid(super::BAD_PATH))
}

/// A relative path (validated) as Dropbox's path in the app folder.
fn dropbox_path(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    }
}

/// A name of one segment, as the layout's are.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
}

/// Keystead's app folder on an account (see the module docs).
pub struct DropboxRemote {
    http: Http,
    endpoints: Endpoints,
    app_key: String,
    key: String,
    refresh: Zeroizing<String>,
    access: Mutex<Option<Zeroizing<String>>>,
}

impl std::fmt::Debug for DropboxRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropboxRemote").finish_non_exhaustive()
    }
}

impl DropboxRemote {
    /// The app folder of `account_id` with its refresh token (no request
    /// yet).
    pub fn new(
        app_key: &str,
        account_id: &str,
        refresh: Zeroizing<String>,
        endpoints: Endpoints,
    ) -> AppResult<Self> {
        Ok(DropboxRemote {
            http: Http::new(Trust::System)?,
            endpoints,
            app_key: app_key.to_owned(),
            key: key_of(account_id),
            refresh,
            access: Mutex::new(None),
        })
    }

    fn lock_access(&self) -> std::sync::MutexGuard<'_, Option<Zeroizing<String>>> {
        self.access
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// An access token: the one in memory, or a fresh one from the refresh
    /// token (a revoked one: `sync_auth_failed`).
    fn token(&self, renew: bool) -> AppResult<Zeroizing<String>> {
        if !renew {
            if let Some(token) = self.lock_access().clone() {
                return Ok(token);
            }
        }
        let body = form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", self.refresh.as_str()),
            ("client_id", &self.app_key),
        ]);
        let answer = post_form(&self.http, self.endpoints.api("oauth2/token"), &body)?;
        if matches!(answer.status.as_u16(), 400 | 401) {
            return Err(AppError::unsupported(AUTH_FAILED));
        }
        if !answer.status.is_success() {
            return Err(rpc_error(&answer, Op::Read));
        }
        let tokens: TokenAnswer = parse(&answer.body)?;
        let token = Zeroizing::new(tokens.access_token);
        *self.lock_access() = Some(token.clone());
        Ok(token)
    }

    /// Sends the request `build` makes for an access token; an expired or
    /// invalid one is renewed once.
    fn call(
        &self,
        build: impl Fn(&str) -> (reqwest::RequestBuilder, u64),
        max: u64,
        too_large: &str,
    ) -> AppResult<Answer> {
        let mut renew = false;
        loop {
            let token = self.token(renew)?;
            let (request, upload) = build(&token);
            let answer = self.http.send(request, upload, max, too_large)?;
            if answer.status == StatusCode::UNAUTHORIZED && !renew {
                *self.lock_access() = None;
                renew = true;
                continue;
            }
            return Ok(answer);
        }
    }

    /// An RPC call on the API host with a JSON argument.
    fn rpc(&self, route: &str, arg: &Value, op: Op) -> AppResult<Value> {
        let body = arg.to_string();
        let url = self.endpoints.api(route);
        let answer = self.call(
            |token| {
                let request = self
                    .http
                    .request(Method::POST, url.clone())
                    .bearer_auth(token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone());
                (request, body.len() as u64)
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if !answer.status.is_success() {
            return Err(rpc_error(&answer, op));
        }
        parse(&answer.body)
    }

    /// Gives the refresh token back ("Abmelden"): Dropbox forgets it.
    pub fn revoke(&self) -> AppResult<()> {
        let url = self.endpoints.api("2/auth/token/revoke");
        let answer = self.call(
            |token| {
                let request = self
                    .http
                    .request(Method::POST, url.clone())
                    .bearer_auth(token)
                    .header(CONTENT_TYPE, "application/json")
                    .body("null");
                (request, 4)
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if answer.status.is_success() || answer.status == StatusCode::UNAUTHORIZED {
            return Ok(());
        }
        Err(rpc_error(&answer, Op::Read))
    }

    fn entry_meta(entry: &Value) -> Option<RemoteMeta> {
        let name = entry["name"].as_str()?;
        if !valid_name(name) {
            return None;
        }
        match entry[".tag"].as_str()? {
            "file" => Some(RemoteMeta {
                name: name.to_owned(),
                dir: false,
                size: entry["size"].as_u64(),
                etag: entry["rev"].as_str().map(str::to_owned),
                modified: None,
            }),
            "folder" => Some(RemoteMeta {
                name: name.to_owned(),
                dir: true,
                size: None,
                etag: None,
                modified: None,
            }),
            _ => None,
        }
    }

    fn file_meta(path: &str, v: &Value, size: u64) -> RemoteMeta {
        RemoteMeta {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            dir: false,
            size: Some(size),
            etag: v["rev"].as_str().map(str::to_owned),
            modified: None,
        }
    }
}

impl Remote for DropboxRemote {
    fn key(&self) -> String {
        self.key.clone()
    }

    /// The app folder always exists (Dropbox makes it with the first
    /// file); one small listing proves the token and the connection.
    fn probe(&self) -> AppResult<Access> {
        self.rpc(
            "2/files/list_folder",
            &json!({"path": "", "recursive": false, "include_deleted": false, "limit": 1}),
            Op::Read,
        )?;
        Ok(Access {
            exists: true,
            writable: true,
            cas: true,
        })
    }

    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>> {
        check_dir(dir)?;
        let mut page = self.rpc(
            "2/files/list_folder",
            &json!({"path": dropbox_path(dir), "recursive": false, "include_deleted": false}),
            Op::Read,
        )?;
        let mut out = Vec::new();
        let mut seen = 0usize;
        loop {
            let entries = page["entries"]
                .as_array()
                .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
            seen += entries.len();
            if seen > LIST_ENTRIES_MAX {
                return Err(AppError::unsupported(BAD_RESPONSE));
            }
            out.extend(entries.iter().filter_map(Self::entry_meta));
            if page["has_more"].as_bool() != Some(true) {
                return Ok(out);
            }
            let cursor = page["cursor"]
                .as_str()
                .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?
                .to_owned();
            page = self.rpc(
                "2/files/list_folder/continue",
                &json!({ "cursor": cursor }),
                Op::Read,
            )?;
        }
    }

    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)> {
        check_path(path)?;
        let arg = api_arg(&json!({ "path": dropbox_path(path) }))?;
        let url = self.endpoints.content("2/files/download");
        let answer = self.call(
            |token| {
                let request = self
                    .http
                    .request(Method::POST, url.clone())
                    .bearer_auth(token)
                    .header("Dropbox-API-Arg", arg.clone());
                (request, 0)
            },
            max,
            TOO_LARGE,
        )?;
        if !answer.status.is_success() {
            return Err(rpc_error(&answer, Op::Read));
        }
        let rev = answer
            .headers
            .get("dropbox-api-result")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| serde_json::from_str::<Value>(v).ok())
            .and_then(|v| v["rev"].as_str().map(str::to_owned));
        Ok((answer.body, rev))
    }

    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta> {
        check_path(path)?;
        let mode = match &pre {
            Precondition::None => json!("overwrite"),
            Precondition::IfMatch(rev) => json!({".tag": "update", "update": rev}),
            Precondition::IfNoneMatch => json!("add"),
        };
        let arg = api_arg(&json!({
            "path": dropbox_path(path),
            "mode": mode,
            "autorename": false,
            "mute": true,
            "strict_conflict": true,
        }))?;
        let url = self.endpoints.content("2/files/upload");
        let answer = self.call(
            |token| {
                let request = self
                    .http
                    .request(Method::POST, url.clone())
                    .bearer_auth(token)
                    .header("Dropbox-API-Arg", arg.clone())
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .body(bytes.to_vec());
                (request, bytes.len() as u64)
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if !answer.status.is_success() {
            return Err(rpc_error(&answer, Op::Write));
        }
        let v: Value = parse(&answer.body)?;
        Ok(Self::file_meta(path, &v, bytes.len() as u64))
    }

    fn mkcol(&self, dir: &str) -> AppResult<()> {
        check_dir(dir)?;
        if dir.is_empty() {
            return Err(AppError::invalid(super::BAD_PATH));
        }
        match self.rpc(
            "2/files/create_folder_v2",
            &json!({"path": dropbox_path(dir), "autorename": false}),
            Op::Write,
        ) {
            Ok(_) => Ok(()),
            // It exists.
            Err(e) if super::has_code(&e, CHANGED) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()> {
        check_path(path)?;
        let mut arg = json!({ "path": dropbox_path(path) });
        if let Precondition::IfMatch(rev) = &pre {
            arg["parent_rev"] = json!(rev);
        }
        self.rpc("2/files/delete_v2", &arg, Op::Write).map(|_| ())
    }
}

#[cfg(test)]
pub mod fake;

#[cfg(test)]
mod tests {
    use super::fake::FakeDropbox;
    use super::*;
    use crate::remote::has_code;

    const ID: &str = "0f5b2c1e-aaaa-4bbb-8ccc-1234567890ab";

    fn vault() -> String {
        format!("{ID}.keystead")
    }

    fn remote(fake: &FakeDropbox) -> DropboxRemote {
        let connected =
            connect("app-key", &fake.code(), &fake.verifier(), &fake.endpoints()).unwrap();
        DropboxRemote::new(
            "app-key",
            &connected.account.account_id,
            connected.refresh_token,
            fake.endpoints(),
        )
        .unwrap()
    }

    #[test]
    fn pkce_and_the_authorize_url() {
        let pkce = Pkce::new().unwrap();
        assert_eq!(pkce.verifier.len(), 43);
        assert!(pkce
            .verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(pkce.verifier.as_bytes()));
        assert_eq!(pkce.challenge, expected);
        assert_ne!(Pkce::new().unwrap().verifier, pkce.verifier);
        let url = Url::parse(&authorize_url("abc123", &pkce.challenge)).unwrap();
        assert_eq!(url.host_str(), Some("www.dropbox.com"));
        let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["client_id"], "abc123");
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["code_challenge"], pkce.challenge);
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["token_access_type"], "offline");
        assert!(!pairs.contains_key("redirect_uri"));
        assert_eq!(key_of("dbid:AA1"), "dropbox:dbid:AA1");
    }

    #[test]
    fn the_code_becomes_a_refresh_token_and_an_account() {
        let fake = FakeDropbox::new();
        let connected =
            connect("app-key", &fake.code(), &fake.verifier(), &fake.endpoints()).unwrap();
        assert_eq!(connected.account.account_id, "dbid:test-account");
        assert_eq!(connected.account.email, "max@example.com");
        assert_eq!(connected.account.name, "Max Muster");
        assert!(!format!("{connected:?}").contains(connected.refresh_token.as_str()));
        // A wrong code, a wrong verifier, garbage: refused, nothing kept.
        for (code, verifier) in [
            ("nope".to_owned(), fake.verifier()),
            (fake.code(), "other".to_owned()),
        ] {
            let e = connect("app-key", &code, &verifier, &fake.endpoints()).unwrap_err();
            assert_eq!(e.code(), "invalid_input:dropbox_code");
        }
        for bad in ["", " ", "a b", "ä"] {
            let e = connect("app-key", bad, &fake.verifier(), &fake.endpoints()).unwrap_err();
            assert_eq!(e.code(), "invalid_input:dropbox_code", "{bad:?}");
        }
    }

    #[test]
    fn files_go_up_and_down_with_revs_as_etags() {
        let fake = FakeDropbox::new();
        let remote = remote(&fake);
        assert!(remote.probe().unwrap().cas);
        assert!(remote.list("").unwrap().is_empty());
        let first = remote
            .put(&vault(), b"v1", Precondition::IfNoneMatch)
            .unwrap();
        let rev1 = first.etag.clone().unwrap();
        let (bytes, rev) = remote.get(&vault(), 100).unwrap();
        assert_eq!(
            (bytes.as_slice(), rev.as_deref()),
            (&b"v1"[..], Some(rev1.as_str()))
        );
        // Conditional writes: a stale rev or a create over a file are
        // conflicts – nothing written.
        let e = remote
            .put(&vault(), b"x", Precondition::IfMatch("stale".into()))
            .unwrap_err();
        assert!(has_code(&e, CHANGED), "{e:?}");
        let e = remote
            .put(&vault(), b"x", Precondition::IfNoneMatch)
            .unwrap_err();
        assert!(has_code(&e, CHANGED), "{e:?}");
        let second = remote
            .put(&vault(), b"v2", Precondition::IfMatch(rev1.clone()))
            .unwrap();
        assert_ne!(second.etag, Some(rev1));
        assert_eq!(fake.file(&vault()).unwrap(), b"v2");
        // The listing, an attachment folder, its blob, removals.
        let blobs = crate::remote::blob_dir_name(ID);
        remote.mkcol(&blobs).unwrap();
        remote.mkcol(&blobs).unwrap();
        let blob = format!("{blobs}/{ID}.bin");
        remote.put(&blob, b"blob", Precondition::None).unwrap();
        let mut names: Vec<_> = remote
            .list("")
            .unwrap()
            .into_iter()
            .map(|m| (m.name, m.dir))
            .collect();
        names.sort();
        assert_eq!(names, [(vault(), false), (blobs.clone(), true)]);
        assert_eq!(remote.list(&blobs).unwrap().len(), 1);
        remote.delete(&blob, Precondition::None).unwrap();
        assert!(has_code(
            &remote.delete(&blob, Precondition::None).unwrap_err(),
            FILE_MISSING
        ));
        assert!(has_code(&remote.get(&blob, 100).unwrap_err(), FILE_MISSING));
        // Too large for the caller: refused.
        assert!(has_code(&remote.get(&vault(), 1).unwrap_err(), TOO_LARGE));
        // Only the layout's paths.
        for bad in ["../x", "/abs", "a/b/c", "x.txt"] {
            assert!(remote.get(bad, 10).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_expired_token_is_renewed_once_a_revoked_one_fails() {
        let fake = FakeDropbox::new();
        let remote = remote(&fake);
        remote.list("").unwrap();
        let before = fake.token_requests();
        fake.expire_access_tokens();
        remote.list("").unwrap();
        assert_eq!(fake.token_requests(), before + 1);
        remote.list("").unwrap();
        assert_eq!(fake.token_requests(), before + 1, "the new token is kept");
        fake.revoke_refresh_tokens();
        fake.expire_access_tokens();
        let e = remote.list("").unwrap_err();
        assert!(has_code(&e, AUTH_FAILED), "{e:?}");
    }

    #[test]
    fn hostile_and_failing_answers() {
        let fake = FakeDropbox::new();
        let remote = remote(&fake);
        remote.put(&vault(), b"v1", Precondition::None).unwrap();
        // Names that are no single segment are dropped; paging is followed.
        fake.add_raw_entry(json!({".tag": "file", "name": "../evil", "rev": "1", "size": 1}));
        fake.add_raw_entry(json!({".tag": "file", "name": "a/b", "rev": "1", "size": 1}));
        fake.set_page_size(1);
        let names: Vec<_> = remote
            .list("")
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, [vault()]);
        fake.set_page_size(1000);
        for (status, body, code) in [
            (
                429,
                json!({"error_summary": "too_many_requests/"}),
                SERVER_ERROR,
            ),
            (500, json!({}), SERVER_ERROR),
            (
                409,
                json!({"error_summary": "path/insufficient_space/.."}),
                QUOTA,
            ),
            (
                409,
                json!({"error_summary": "path/no_write_permission/.."}),
                NOT_WRITABLE,
            ),
        ] {
            fake.fail_next(status, body);
            let e = remote.put(&vault(), b"x", Precondition::None).unwrap_err();
            assert!(has_code(&e, code), "{status}: {e:?}");
        }
        fake.oversize_next_listing();
        let e = remote.list("").unwrap_err();
        assert!(has_code(&e, BAD_RESPONSE), "{e:?}");
        fake.too_many_entries();
        let e = remote.list("").unwrap_err();
        assert!(has_code(&e, BAD_RESPONSE), "{e:?}");
    }

    #[test]
    fn both_engines_sync_through_dropbox() {
        use std::fs;

        use keystead_core::settings::Settings;
        use keystead_core::{KdfParams, VaultStore};

        use crate::folder_sync::{self, LinkState, Outcome};
        use crate::remote::folder::RemoteDirFolder;
        use crate::state::Core;

        let dir = tempfile::tempdir().unwrap();
        let fake = FakeDropbox::new();
        // The PC's vault in the app folder.
        let pc = VaultStore::new(dir.path().join("pc"));
        let v = pc
            .create_vault_with_params("Privat", "pc-pw", KdfParams::insecure_for_tests())
            .unwrap();
        let name = format!("{}.keystead", v.id());
        let up = remote(&fake);
        up.put(
            &name,
            &fs::read(v.path()).unwrap(),
            Precondition::IfNoneMatch,
        )
        .unwrap();
        // The phone imports it through the same account.
        let phone = Core::for_tests(
            VaultStore::new(dir.path().join("phone")),
            Settings::default(),
        );
        let folder = RemoteDirFolder::new(remote(&fake));
        let mut link = LinkState::new(key_of("dbid:test-account"), "Dropbox".into(), 1, true);
        assert!(folder_sync::sync_folder(&phone, &folder, &mut link, 2).unwrap());
        let result = link.files.iter().find(|f| f.file_name == name).unwrap();
        assert_eq!(result.outcome, Outcome::Imported);
        assert_eq!(
            fs::read(phone.store().vault_path(v.id())).unwrap(),
            fs::read(v.path()).unwrap()
        );
    }
}
