//! Google Drive as a [`Remote`] (stage 2d): a folder "Keystead" in the
//! user's My Drive that Keystead made itself, with the scope `drive.file`
//! (the app sees only files it created – nothing else of the Drive).
//!
//! * **Login, PC**: OAuth 2 code flow with PKCE and a random `state`; the
//!   redirect goes to a loopback listener (`http://127.0.0.1:<port>`, a
//!   "Desktop app" client). The code is exchanged only with the verifier and
//!   only for the waiting `state`, so a redirect another page fakes is
//!   refused.
//! * **Login, phone**: the device flow (a "TVs and Limited Input devices"
//!   client – Google allows `drive.file` there): Keystead shows a code, the
//!   user enters it at the page Google names (`google.com/device`) and
//!   allows the folder, Keystead asks the token endpoint until it answers.
//!   No redirect into the app (Google no longer supports an Android app's
//!   own URI scheme, nor a loopback redirect for Android clients).
//! * Client ids and secrets from the build ([`DESKTOP_CLIENT_ID`],
//!   [`DEVICE_CLIENT_ID`]; the secrets of installed apps are not
//!   confidential, Google says so); without both of a platform Google Drive
//!   is not offered. The refresh token is stored like a server password,
//!   key [`key_of`] (the Drive user's `permissionId`). Signing out only
//!   forgets it here: Google's revoke would end the access of **every**
//!   device of the account (it drops the grant of the whole project).
//! * **Layout**: the folder's files are the cloud folder's names; an
//!   attachment folder is a sub-folder. Drive allows several files of one
//!   name in a folder: the **oldest** (by `createdTime`) is the one, later
//!   ones are ignored, and a create that turns out to be such a later one
//!   is removed again and reported as `sync_remote_changed` (the next look
//!   reads the older); a delete removes every file of the name. Drive has no
//!   `If-Match` on uploads: `cas` is false –
//!   the engines read the copy again right before a write (as for rclone).
//!   The ETag is the file's `version`, compared right before an update
//!   or a delete. The folder is looked up by name in My Drive at each
//!   [`Remote::probe`] (a trashed or moved one is missing, never empty).
//! * **Answers are untrusted**: JSON at most [`super::dropbox::LIST_BODY_MAX`]
//!   per page, ≤ [`super::dropbox::LIST_ENTRIES_MAX`] entries and
//!   [`super::dropbox::LIST_PAGES_MAX`] pages, names of one segment only;
//!   errors are codes, never Google's texts.
//! * Transport, TLS and the request ceilings: [`super::http`].

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::dropbox::{Pkce, LIST_BODY_MAX, LIST_ENTRIES_MAX, LIST_PAGES_MAX};
use super::http::{Answer, Http};
use super::tls::Trust;
use super::{
    check_dir, check_path, Access, Precondition, Remote, RemoteMeta, AUTH_FAILED, BAD_RESPONSE,
    CHANGED, FILE_MISSING, FOLDER_MISSING, NOT_WRITABLE, QUOTA, SERVER_ERROR, TOO_LARGE,
};
use crate::error::{AppError, AppResult};

/// The client id of Keystead's "Desktop app" OAuth client (the PC; from the
/// build's environment); `None` until the owner registered it.
pub const DESKTOP_CLIENT_ID: Option<&str> = option_env!("KEYSTEAD_GDRIVE_DESKTOP_CLIENT_ID");
/// Its secret – not confidential for an installed app (Google's own word);
/// Google wants it with the code.
pub const DESKTOP_CLIENT_SECRET: Option<&str> =
    option_env!("KEYSTEAD_GDRIVE_DESKTOP_CLIENT_SECRET");
/// The client id of the "TVs and Limited Input devices" client (the phone's
/// device flow).
pub const DEVICE_CLIENT_ID: Option<&str> = option_env!("KEYSTEAD_GDRIVE_DEVICE_CLIENT_ID");
/// Its secret (not confidential either; Google wants it with each poll).
pub const DEVICE_CLIENT_SECRET: Option<&str> = option_env!("KEYSTEAD_GDRIVE_DEVICE_CLIENT_SECRET");
/// The device flow's grant type.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// The scope: only the files Keystead made.
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
/// The folder in My Drive.
pub const FOLDER_NAME: &str = "Keystead";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// `unsupported:gdrive_not_configured`: this build has no client id.
pub const NOT_CONFIGURED: &str = "gdrive_not_configured";
/// `invalid_input:gdrive_redirect`: Google did not confirm the login (the
/// user refused, another `state`, no code, an expired or used code).
pub const BAD_REDIRECT: &str = "gdrive_redirect";

/// The OAuth client of this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    /// Not confidential (an installed app's), but Google wants it.
    pub secret: String,
}

/// This build's client – the PC's "Desktop app", the phone's device client
/// –, `unsupported:gdrive_not_configured` without its id and secret (the
/// unit tests have one: the fake takes any).
pub fn client() -> AppResult<Client> {
    if cfg!(test) {
        return Ok(Client {
            id: "client-id".into(),
            secret: "client-secret".into(),
        });
    }
    let configured = |v: Option<&str>| v.filter(|v| !v.is_empty()).map(str::to_owned);
    let (id, secret) = if cfg!(mobile) {
        (DEVICE_CLIENT_ID, DEVICE_CLIENT_SECRET)
    } else {
        (DESKTOP_CLIENT_ID, DESKTOP_CLIENT_SECRET)
    };
    match (configured(id), configured(secret)) {
        (Some(id), Some(secret)) => Ok(Client { id, secret }),
        _ => Err(AppError::unsupported(NOT_CONFIGURED)),
    }
}

/// What the bases, the link and the refresh token are recorded for.
pub fn key_of(account_id: &str) -> String {
    format!("gdrive:{account_id}")
}

/// Google's hosts (the unit tests point them at a fake).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// `https://oauth2.googleapis.com/` – the token, the device code.
    pub oauth: Url,
    /// `https://www.googleapis.com/` – Drive (files, uploads, about).
    pub api: Url,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            oauth: Url::parse("https://oauth2.googleapis.com/").expect("constant URL"),
            api: Url::parse("https://www.googleapis.com/").expect("constant URL"),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// The unit tests' fake for the code on this thread.
    pub static TEST_ENDPOINTS: std::cell::RefCell<Option<Endpoints>> =
        const { std::cell::RefCell::new(None) };
}

impl Endpoints {
    /// Google's (the unit tests: their fake's).
    pub fn current() -> Self {
        #[cfg(test)]
        if let Some(endpoints) = TEST_ENDPOINTS.with(|e| e.borrow().clone()) {
            return endpoints;
        }
        Endpoints::default()
    }

    fn oauth(&self, route: &str) -> Url {
        self.oauth.join(route).expect("relative route")
    }

    fn api(&self, route: &str) -> Url {
        self.api.join(route).expect("relative route")
    }
}

/// One login: the PKCE verifier and the `state`, both random; only the
/// challenge and the state go into [`authorize_url`].
pub struct Login {
    pub pkce: Pkce,
    pub state: String,
}

impl Login {
    pub fn new() -> AppResult<Self> {
        let state = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(keystead_core::crypto::random_array::<16>()?);
        Ok(Login {
            pkce: Pkce::new()?,
            state,
        })
    }
}

/// Google's consent page for the folder (offline: a refresh token).
pub fn authorize_url(client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    let mut url = Url::parse(AUTHORIZE_URL).expect("constant URL");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "select_account consent");
    url.to_string()
}

/// The `state` a redirect carries (none: not one of Keystead's).
pub fn state_of_redirect(redirect: &str) -> Option<String> {
    let url = Url::parse(redirect).ok()?;
    let state = url.query_pairs().find(|(name, _)| name == "state")?.1;
    Some(state.into_owned())
}

/// The code of a redirect for the waiting `state` (`invalid_input:gdrive_redirect`
/// otherwise – an error answer such as "access denied", another state, no
/// code).
pub fn code_of_redirect(redirect: &str, state: &str) -> AppResult<Zeroizing<String>> {
    let url = Url::parse(redirect).map_err(|_| AppError::invalid(BAD_REDIRECT))?;
    let pairs: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
    if pairs.get("state").map(String::as_str) != Some(state) || pairs.contains_key("error") {
        return Err(AppError::invalid(BAD_REDIRECT));
    }
    pairs
        .get("code")
        .filter(|c| !c.is_empty() && c.len() <= 1024 && c.bytes().all(|b| b.is_ascii_graphic()))
        .map(|c| Zeroizing::new(c.clone()))
        .ok_or_else(|| AppError::invalid(BAD_REDIRECT))
}

/// The PC's redirect: a listener on a free loopback port and its address
/// (`http://127.0.0.1:<port>/`, what the Desktop client allows).
#[cfg_attr(mobile, allow(dead_code))]
pub fn listen_loopback() -> AppResult<(std::net::TcpListener, String)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| AppError::io(format!("loopback listener: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| AppError::io(format!("loopback listener: {e}")))?
        .port();
    Ok((listener, format!("http://127.0.0.1:{port}/")))
}

/// Answers the browser on the loopback listener until the redirect of the
/// login with `state` comes (its code, or `invalid_input:gdrive_redirect`
/// when the user refused), `waiting` says no or `until` passes (`None`).
/// Other requests (another state, the favicon) get a 404 and are otherwise
/// ignored; a request head is at most 8 KiB and 5 s in all (and no longer
/// than `waiting` and `until` allow).
#[cfg_attr(mobile, allow(dead_code))]
pub fn serve_loopback(
    listener: std::net::TcpListener,
    state: &str,
    until: std::time::Instant,
    waiting: impl Fn() -> bool,
) -> Option<AppResult<Zeroizing<String>>> {
    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};
    if listener.set_nonblocking(true).is_err() {
        return None;
    }
    while waiting() && Instant::now() < until {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let deadline = (Instant::now() + Duration::from_secs(5)).min(until);
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while head.len() < 8 * 1024 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !waiting() {
                break;
            }
            let _ = stream.set_read_timeout(Some(left));
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
        let head = String::from_utf8_lossy(&head);
        let target = head
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_default();
        let redirect = format!("http://127.0.0.1{target}");
        let ours = state_of_redirect(&redirect).as_deref() == Some(state);
        let outcome = ours.then(|| code_of_redirect(&redirect, state));
        let (status, page) = match &outcome {
            Some(Ok(_)) => ("200 OK", LOOPBACK_DONE),
            Some(Err(_)) => ("200 OK", LOOPBACK_REFUSED),
            None => ("404 Not Found", ""),
        };
        let answer = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{page}",
            page.len()
        );
        let _ = stream.write_all(answer.as_bytes());
        let _ = stream.flush();
        if outcome.is_some() {
            return outcome;
        }
    }
    None
}

#[cfg_attr(mobile, allow(dead_code))]
const LOOPBACK_DONE: &str = "<!doctype html><meta charset=utf-8><title>Keystead</title><body style=\"font-family:system-ui;margin:3em\"><h1>Keystead</h1><p>Anmeldung bei Google abgeschlossen – du kannst dieses Fenster schließen und zu Keystead zurückkehren.</p><p>Signed in to Google – you can close this window and return to Keystead.</p>";
#[cfg_attr(mobile, allow(dead_code))]
const LOOPBACK_REFUSED: &str = "<!doctype html><meta charset=utf-8><title>Keystead</title><body style=\"font-family:system-ui;margin:3em\"><h1>Keystead</h1><p>Die Anmeldung wurde nicht erlaubt. Starte sie in Keystead erneut.</p><p>The sign-in was not allowed. Start it again in Keystead.</p>";

/// Who the Drive user is (shown in the settings; no secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdriveAccount {
    /// Drive's `permissionId` – stable for the user.
    pub account_id: String,
    pub name: String,
    pub email: String,
}

/// What a code brought: the refresh token (the secret) and the account.
pub struct Connected {
    pub refresh_token: Zeroizing<String>,
    pub account: GdriveAccount,
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

/// What the token endpoint gave a login: the refresh token (the secret) and
/// a first access token.
pub struct Tokens {
    pub refresh: Zeroizing<String>,
    pub access: Zeroizing<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens").finish_non_exhaustive()
    }
}

fn tokens_of(answer: &Answer) -> AppResult<Tokens> {
    let tokens: TokenAnswer = parse(&answer.body)?;
    let refresh = tokens
        .refresh_token
        .filter(|t| !t.is_empty())
        .map(Zeroizing::new)
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    Ok(Tokens {
        refresh,
        access: Zeroizing::new(tokens.access_token),
    })
}

/// The `error` of an OAuth answer (`invalid_grant`, `authorization_pending`,
/// …; only matched, never shown).
fn oauth_error(answer: &Answer) -> String {
    serde_json::from_slice::<Value>(&answer.body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Exchanges a code of the loopback login (with the verifier and the
/// redirect it came to) for the tokens. A code Google does not take
/// (`invalid_grant`: expired, used) is `invalid_input:gdrive_redirect`; a
/// client Google does not know is `unsupported:gdrive_not_configured`.
pub fn exchange_code(
    client: &Client,
    code: &str,
    verifier: &str,
    redirect: &str,
    endpoints: &Endpoints,
) -> AppResult<Tokens> {
    let http = Http::new(Trust::System)?;
    let body = form(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client.id.as_str()),
        ("client_secret", client.secret.as_str()),
        ("code_verifier", verifier),
        ("redirect_uri", redirect),
    ]);
    let answer = post_form(&http, endpoints.oauth("token"), &body)?;
    if answer.status.is_success() {
        return tokens_of(&answer);
    }
    match (answer.status.as_u16(), oauth_error(&answer).as_str()) {
        (400, "invalid_grant") => Err(AppError::invalid(BAD_REDIRECT)),
        (400 | 401, _) => Err(AppError::unsupported(NOT_CONFIGURED)),
        _ => Err(api_error(&answer, Op::Read)),
    }
}

/// A started device login (the phone): the code the user enters at
/// `verification_url`, and what Keystead asks the token endpoint with.
pub struct DeviceLogin {
    pub device_code: Zeroizing<String>,
    pub user_code: String,
    pub verification_url: String,
    /// How long the codes are valid.
    pub expires_in: Duration,
    /// How long to wait between two polls.
    pub interval: Duration,
}

impl std::fmt::Debug for DeviceLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLogin")
            .field("user_code", &self.user_code)
            .field("verification_url", &self.verification_url)
            .finish_non_exhaustive()
    }
}

/// Starts a device login. The answer is checked: a code of at most 64
/// plain characters, an `https` page on google.com (the page opens it).
pub fn start_device_login(client: &Client, endpoints: &Endpoints) -> AppResult<DeviceLogin> {
    let http = Http::new(Trust::System)?;
    let body = form(&[("client_id", client.id.as_str()), ("scope", SCOPE)]);
    let answer = post_form(&http, endpoints.oauth("device/code"), &body)?;
    if !answer.status.is_success() {
        return Err(match answer.status.as_u16() {
            400 | 401 | 403 => AppError::unsupported(NOT_CONFIGURED),
            _ => api_error(&answer, Op::Read),
        });
    }
    let v: Value = parse(&answer.body)?;
    let text = |key: &str| v[key].as_str().map(str::to_owned);
    let device_code = text("device_code")
        .filter(|c| !c.is_empty() && c.len() <= 1024)
        .map(Zeroizing::new)
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    let user_code = text("user_code")
        .filter(|c| {
            !c.is_empty() && c.len() <= 64 && c.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        })
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    let verification_url = text("verification_url")
        .or_else(|| text("verification_uri"))
        .filter(|u| {
            Url::parse(u).is_ok_and(|url| {
                url.scheme() == "https"
                    && url
                        .host_str()
                        .is_some_and(|h| h == "google.com" || h.ends_with(".google.com"))
            })
        })
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    let seconds = |key: &str, default: u64, max: u64| {
        Duration::from_secs(v[key].as_u64().unwrap_or(default).clamp(1, max))
    };
    Ok(DeviceLogin {
        device_code,
        user_code,
        verification_url,
        expires_in: seconds("expires_in", 1800, 3600),
        interval: seconds("interval", 5, 60),
    })
}

/// What a poll of a device login found.
#[derive(Debug)]
pub enum DevicePoll {
    /// The user has not allowed it yet.
    Pending,
    /// Too often: wait longer.
    SlowDown,
    /// Allowed: the tokens.
    Done(Tokens),
    /// The user refused.
    Refused,
    /// The codes expired.
    Expired,
}

/// Asks the token endpoint whether the user allowed the device login.
pub fn poll_device_login(
    client: &Client,
    device_code: &str,
    endpoints: &Endpoints,
) -> AppResult<DevicePoll> {
    let http = Http::new(Trust::System)?;
    let body = form(&[
        ("client_id", client.id.as_str()),
        ("client_secret", client.secret.as_str()),
        ("device_code", device_code),
        ("grant_type", DEVICE_GRANT),
    ]);
    let answer = post_form(&http, endpoints.oauth("token"), &body)?;
    if answer.status.is_success() {
        return tokens_of(&answer).map(DevicePoll::Done);
    }
    Ok(match oauth_error(&answer).as_str() {
        "authorization_pending" => DevicePoll::Pending,
        "slow_down" => DevicePoll::SlowDown,
        "access_denied" => DevicePoll::Refused,
        "expired_token" | "invalid_grant" => DevicePoll::Expired,
        "invalid_client" | "unauthorized_client" => {
            return Err(AppError::unsupported(NOT_CONFIGURED))
        }
        _ => return Err(api_error(&answer, Op::Read)),
    })
}

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

/// What a call is, for the error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Read,
    Write,
}

/// The code of an answer that is not a success (Google's texts are only
/// matched, never passed on).
fn api_error(answer: &Answer, op: Op) -> AppError {
    let reason = serde_json::from_slice::<Value>(&answer.body)
        .ok()
        .and_then(|v| {
            v["error"]["errors"][0]["reason"]
                .as_str()
                .or_else(|| v["error"]["status"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let code = match answer.status.as_u16() {
        401 => AUTH_FAILED,
        403 if reason.contains("storageQuotaExceeded") || reason.contains("quotaExceeded") => QUOTA,
        403 if reason.contains("rateLimit") || reason.contains("RateLimit") => SERVER_ERROR,
        403 if op == Op::Write => NOT_WRITABLE,
        403 => AUTH_FAILED,
        404 => FILE_MISSING,
        400 => BAD_RESPONSE,
        _ => SERVER_ERROR,
    };
    AppError::unsupported(code)
}

/// With the tokens of a login: asks whose Drive it is and makes the folder
/// if there is none yet (the app's own – it is never made by a sync
/// otherwise).
pub fn connect(client: &Client, tokens: Tokens, endpoints: &Endpoints) -> AppResult<Connected> {
    let http = Http::new(Trust::System)?;
    let Tokens { refresh, access } = tokens;
    let request = http
        .request(
            Method::GET,
            endpoints.api("drive/v3/about?fields=user(permissionId,displayName,emailAddress)"),
        )
        .bearer_auth(access.as_str());
    let answer = http.send(request, 0, LIST_BODY_MAX, BAD_RESPONSE)?;
    if !answer.status.is_success() {
        return Err(api_error(&answer, Op::Read));
    }
    let v: Value = parse(&answer.body)?;
    let user = &v["user"];
    let account_id = user["permissionId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?
        .to_owned();
    let remote = GdriveRemote::new(
        client.clone(),
        &account_id,
        refresh.clone(),
        endpoints.clone(),
    )?;
    *remote.lock_access() = Some(access);
    remote.ensure_folder()?;
    Ok(Connected {
        refresh_token: refresh,
        account: GdriveAccount {
            account_id,
            name: user["displayName"].as_str().unwrap_or_default().to_owned(),
            email: user["emailAddress"].as_str().unwrap_or_default().to_owned(),
        },
    })
}

/// A file of a listing.
#[derive(Debug, Clone)]
struct DriveFile {
    id: String,
    name: String,
    dir: bool,
    size: Option<u64>,
    version: Option<String>,
    created: String,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
}

/// A name inside a Drive query (`'…'`, backslash and quote escaped).
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Keystead's folder on a Drive (see the module docs).
pub struct GdriveRemote {
    http: Http,
    endpoints: Endpoints,
    client: Client,
    key: String,
    refresh: Zeroizing<String>,
    access: Mutex<Option<Zeroizing<String>>>,
    /// The folder's id, once found.
    folder: Mutex<Option<String>>,
}

impl std::fmt::Debug for GdriveRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GdriveRemote").finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl GdriveRemote {
    pub fn new(
        client: Client,
        account_id: &str,
        refresh: Zeroizing<String>,
        endpoints: Endpoints,
    ) -> AppResult<Self> {
        Ok(GdriveRemote {
            http: Http::new(Trust::System)?,
            endpoints,
            client,
            key: key_of(account_id),
            refresh,
            access: Mutex::new(None),
            folder: Mutex::new(None),
        })
    }

    fn lock_access(&self) -> MutexGuard<'_, Option<Zeroizing<String>>> {
        lock(&self.access)
    }

    fn token(&self, renew: bool) -> AppResult<Zeroizing<String>> {
        if !renew {
            if let Some(token) = self.lock_access().clone() {
                return Ok(token);
            }
        }
        let body = form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", self.refresh.as_str()),
            ("client_id", self.client.id.as_str()),
            ("client_secret", self.client.secret.as_str()),
        ]);
        let answer = post_form(&self.http, self.endpoints.oauth("token"), &body)?;
        if matches!(answer.status.as_u16(), 400 | 401) {
            return Err(AppError::unsupported(AUTH_FAILED));
        }
        if !answer.status.is_success() {
            return Err(api_error(&answer, Op::Read));
        }
        let tokens: TokenAnswer = parse(&answer.body)?;
        let token = Zeroizing::new(tokens.access_token);
        *self.lock_access() = Some(token.clone());
        Ok(token)
    }

    /// Sends the request `build` makes for an access token; an expired one
    /// is renewed once.
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

    fn get_json(&self, url: Url, op: Op) -> AppResult<Value> {
        let answer = self.call(
            |token| {
                (
                    self.http
                        .request(Method::GET, url.clone())
                        .bearer_auth(token),
                    0,
                )
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if !answer.status.is_success() {
            return Err(api_error(&answer, op));
        }
        parse(&answer.body)
    }

    fn send_json(&self, method: Method, url: Url, body: &Value, op: Op) -> AppResult<Value> {
        let text = body.to_string();
        let answer = self.call(
            |token| {
                let request = self
                    .http
                    .request(method.clone(), url.clone())
                    .bearer_auth(token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(text.clone());
                (request, text.len() as u64)
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if !answer.status.is_success() {
            return Err(api_error(&answer, op));
        }
        if answer.body.is_empty() {
            return Ok(Value::Null);
        }
        parse(&answer.body)
    }

    /// The children of folder `parent` matching `extra` (a query part), the
    /// oldest first; at most [`LIST_ENTRIES_MAX`].
    fn children(&self, parent: &str, extra: &str) -> AppResult<Vec<DriveFile>> {
        let mut query = format!("{} in parents and trashed=false", quoted(parent));
        if !extra.is_empty() {
            query.push_str(" and ");
            query.push_str(extra);
        }
        let mut out = Vec::new();
        let mut seen = 0usize;
        let mut page_token: Option<String> = None;
        for page_number in 0.. {
            if page_number >= LIST_PAGES_MAX {
                return Err(AppError::unsupported(BAD_RESPONSE));
            }
            let mut url = self.endpoints.api("drive/v3/files");
            {
                let mut pairs = url.query_pairs_mut();
                pairs
                    .append_pair("q", &query)
                    .append_pair(
                        "fields",
                        "nextPageToken,files(id,name,mimeType,size,version,createdTime)",
                    )
                    .append_pair("orderBy", "createdTime")
                    .append_pair("pageSize", "1000")
                    .append_pair("spaces", "drive");
                if let Some(token) = &page_token {
                    pairs.append_pair("pageToken", token);
                }
            }
            let page = self.get_json(url, Op::Read)?;
            let files = page["files"]
                .as_array()
                .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
            seen += files.len();
            if seen > LIST_ENTRIES_MAX {
                return Err(AppError::unsupported(BAD_RESPONSE));
            }
            for f in files {
                let (Some(id), Some(name)) = (f["id"].as_str(), f["name"].as_str()) else {
                    continue;
                };
                out.push(DriveFile {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    dir: f["mimeType"].as_str() == Some(FOLDER_MIME),
                    size: f["size"].as_str().and_then(|s| s.parse().ok()),
                    version: f["version"].as_str().map(str::to_owned),
                    created: f["createdTime"].as_str().unwrap_or_default().to_owned(),
                });
            }
            match page["nextPageToken"].as_str() {
                // The same page again: an answer that never ends.
                Some(token) if page_token.as_deref() == Some(token) => {
                    return Err(AppError::unsupported(BAD_RESPONSE))
                }
                Some(token) if !token.is_empty() => page_token = Some(token.to_owned()),
                _ => break,
            }
        }
        // The oldest of a name counts (Drive allows several).
        out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Keystead's folder in My Drive (`None` if there is none).
    fn find_folder(&self) -> AppResult<Option<String>> {
        if let Some(id) = lock(&self.folder).clone() {
            return Ok(Some(id));
        }
        let found = self
            .children(
                "root",
                &format!(
                    "name={} and mimeType={}",
                    quoted(FOLDER_NAME),
                    quoted(FOLDER_MIME)
                ),
            )?
            .into_iter()
            .next()
            .map(|f| f.id);
        if let Some(id) = &found {
            *lock(&self.folder) = Some(id.clone());
        }
        Ok(found)
    }

    fn folder_id(&self) -> AppResult<String> {
        self.find_folder()?
            .ok_or_else(|| AppError::unsupported(FOLDER_MISSING))
    }

    /// Makes Keystead's folder if there is none (at the login only).
    pub fn ensure_folder(&self) -> AppResult<String> {
        if let Some(id) = self.find_folder()? {
            return Ok(id);
        }
        self.create_folder_in("root", FOLDER_NAME)?;
        *lock(&self.folder) = None;
        self.folder_id()
    }

    fn create_folder_in(&self, parent: &str, name: &str) -> AppResult<String> {
        let v = self.send_json(
            Method::POST,
            self.endpoints.api("drive/v3/files?fields=id"),
            &json!({"name": name, "mimeType": FOLDER_MIME, "parents": [parent]}),
            Op::Write,
        )?;
        v["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))
    }

    /// The folder `dir` ("" = Keystead's, else an attachment folder in it).
    fn dir_id(&self, dir: &str) -> AppResult<Option<String>> {
        let root = self.folder_id()?;
        if dir.is_empty() {
            return Ok(Some(root));
        }
        Ok(self
            .children(
                &root,
                &format!("name={} and mimeType={}", quoted(dir), quoted(FOLDER_MIME)),
            )?
            .into_iter()
            .next()
            .map(|f| f.id))
    }

    /// The files at `path`, the oldest (the one) first.
    fn files_at(&self, path: &str) -> AppResult<Vec<DriveFile>> {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let Some(parent) = self.dir_id(dir)? else {
            return Ok(Vec::new());
        };
        Ok(self
            .children(&parent, &format!("name={}", quoted(name)))?
            .into_iter()
            .filter(|f| !f.dir)
            .collect())
    }

    /// The file at `path` (the oldest of its name).
    fn file(&self, path: &str) -> AppResult<Option<DriveFile>> {
        Ok(self.files_at(path)?.into_iter().next())
    }

    fn meta(path: &str, f: &DriveFile) -> RemoteMeta {
        RemoteMeta {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            dir: f.dir,
            size: f.size,
            etag: f.version.clone(),
            modified: None,
        }
    }

    /// Uploads `bytes` as a new file `name` in `parent` (multipart).
    fn create_file(&self, parent: &str, name: &str, bytes: &[u8]) -> AppResult<DriveFile> {
        let boundary = format!(
            "keystead-{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(keystead_core::crypto::random_array::<12>()?)
        );
        let metadata = json!({"name": name, "parents": [parent]}).to_string();
        let mut body = Vec::with_capacity(bytes.len() + 512);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let url = self.endpoints.api(
            "upload/drive/v3/files?uploadType=multipart&fields=id,name,size,version,createdTime",
        );
        let answer = self.call(
            |token| {
                let request = self
                    .http
                    .request(Method::POST, url.clone())
                    .bearer_auth(token)
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/related; boundary={boundary}"),
                    )
                    .body(body.clone());
                (request, body.len() as u64)
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if !answer.status.is_success() {
            return Err(api_error(&answer, Op::Write));
        }
        let v: Value = parse(&answer.body)?;
        Ok(DriveFile {
            id: v["id"]
                .as_str()
                .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?
                .to_owned(),
            name: name.to_owned(),
            dir: false,
            size: Some(bytes.len() as u64),
            version: v["version"].as_str().map(str::to_owned),
            created: v["createdTime"].as_str().unwrap_or_default().to_owned(),
        })
    }

    fn delete_id(&self, id: &str) -> AppResult<()> {
        let url = self.endpoints.api(&format!("drive/v3/files/{id}"));
        let answer = self.call(
            |token| {
                (
                    self.http
                        .request(Method::DELETE, url.clone())
                        .bearer_auth(token),
                    0,
                )
            },
            LIST_BODY_MAX,
            BAD_RESPONSE,
        )?;
        if answer.status.is_success() {
            Ok(())
        } else {
            Err(api_error(&answer, Op::Write))
        }
    }
}

impl Remote for GdriveRemote {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn probe(&self) -> AppResult<Access> {
        // Each pass looks for the folder again (it may have been trashed).
        *lock(&self.folder) = None;
        Ok(Access {
            exists: self.find_folder()?.is_some(),
            writable: true,
            cas: false,
        })
    }

    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>> {
        check_dir(dir)?;
        let Some(id) = self.dir_id(dir)? else {
            return Err(AppError::unsupported(FILE_MISSING));
        };
        let files = self.children(&id, "")?;
        if dir.is_empty() && files.is_empty() {
            // Gone since the pass began, or really empty?
            *lock(&self.folder) = None;
            self.folder_id()?;
        }
        let mut seen = std::collections::BTreeSet::new();
        Ok(files
            .into_iter()
            .filter(|f| valid_name(&f.name) && seen.insert(f.name.clone()))
            .map(|f| Self::meta(&f.name, &f))
            .collect())
    }

    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)> {
        check_path(path)?;
        let Some(file) = self.file(path)? else {
            return Err(AppError::unsupported(FILE_MISSING));
        };
        let url = self
            .endpoints
            .api(&format!("drive/v3/files/{}?alt=media", file.id));
        let answer = self.call(
            |token| {
                (
                    self.http
                        .request(Method::GET, url.clone())
                        .bearer_auth(token),
                    0,
                )
            },
            max,
            TOO_LARGE,
        )?;
        if !answer.status.is_success() {
            return Err(api_error(&answer, Op::Read));
        }
        Ok((answer.body, file.version))
    }

    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta> {
        check_path(path)?;
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        match self.file(path)? {
            Some(_) if pre == Precondition::IfNoneMatch => Err(AppError::unsupported(CHANGED)),
            Some(file) if matches!(&pre, Precondition::IfMatch(v) if file.version.as_deref() != Some(v.as_str())) => {
                Err(AppError::unsupported(CHANGED))
            }
            Some(file) => {
                let url = self.endpoints.api(&format!(
                    "upload/drive/v3/files/{}?uploadType=media&fields=id,size,version,createdTime",
                    file.id
                ));
                let answer = self.call(
                    |token| {
                        let request = self
                            .http
                            .request(Method::PATCH, url.clone())
                            .bearer_auth(token)
                            .header(CONTENT_TYPE, "application/octet-stream")
                            .body(bytes.to_vec());
                        (request, bytes.len() as u64)
                    },
                    LIST_BODY_MAX,
                    BAD_RESPONSE,
                )?;
                if !answer.status.is_success() {
                    return Err(api_error(&answer, Op::Write));
                }
                let v: Value = parse(&answer.body)?;
                Ok(RemoteMeta {
                    name: name.to_owned(),
                    dir: false,
                    size: Some(bytes.len() as u64),
                    etag: v["version"].as_str().map(str::to_owned),
                    modified: None,
                })
            }
            None if matches!(pre, Precondition::IfMatch(_)) => Err(AppError::unsupported(CHANGED)),
            None => {
                let parent = match self.dir_id(dir)? {
                    Some(parent) => parent,
                    None if !dir.is_empty() => {
                        self.mkcol(dir)?;
                        self.dir_id(dir)?
                            .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?
                    }
                    None => return Err(AppError::unsupported(FOLDER_MISSING)),
                };
                let created = self.create_file(&parent, name, bytes)?;
                // Another device made one of this name first: ours goes again,
                // the next look reads the older.
                let oldest = self
                    .children(&parent, &format!("name={}", quoted(name)))?
                    .into_iter()
                    .find(|f| !f.dir);
                if oldest.as_ref().is_some_and(|f| f.id != created.id) {
                    let _ = self.delete_id(&created.id);
                    return Err(AppError::unsupported(CHANGED));
                }
                Ok(Self::meta(path, &created))
            }
        }
    }

    fn mkcol(&self, dir: &str) -> AppResult<()> {
        check_dir(dir)?;
        if dir.is_empty() {
            return Err(AppError::invalid(super::BAD_PATH));
        }
        if self.dir_id(dir)?.is_some() {
            return Ok(());
        }
        let root = self.folder_id()?;
        let created = self.create_folder_in(&root, dir)?;
        // A folder of this name made meanwhile elsewhere: the oldest counts.
        let oldest = self
            .children(
                &root,
                &format!("name={} and mimeType={}", quoted(dir), quoted(FOLDER_MIME)),
            )?
            .into_iter()
            .next();
        if oldest.as_ref().is_some_and(|f| f.id != created) {
            let _ = self.delete_id(&created);
        }
        Ok(())
    }

    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()> {
        check_path(path)?;
        if super::blob_dir_id(path).is_some() {
            return match self.dir_id(path)? {
                Some(id) => self.delete_id(&id),
                None => Err(AppError::unsupported(FILE_MISSING)),
            };
        }
        let files = self.files_at(path)?;
        let Some(file) = files.first() else {
            return Err(AppError::unsupported(FILE_MISSING));
        };
        if let Precondition::IfMatch(version) = &pre {
            if file.version.as_deref() != Some(version.as_str()) {
                return Err(AppError::unsupported(CHANGED));
            }
        }
        // The later files of the name first (ignored, but they would become
        // "the" file once the oldest is gone); a failure leaves the oldest,
        // so the engine's next try finds it unchanged.
        for later in &files[1..] {
            self.delete_id(&later.id)?;
        }
        self.delete_id(&file.id)
    }
}

#[cfg(test)]
pub mod fake;

#[cfg(test)]
mod tests {
    use super::fake::FakeDrive;
    use super::*;
    use crate::remote::has_code;

    const ID: &str = "0f5b2c1e-aaaa-4bbb-8ccc-1234567890ab";

    fn vault() -> String {
        format!("{ID}.keystead")
    }

    fn signed_in(fake: &FakeDrive) -> GdriveRemote {
        let tokens = exchange_code(
            &client().unwrap(),
            &fake.code(),
            &fake.verifier(),
            "http://127.0.0.1:1/",
            &fake.endpoints(),
        )
        .unwrap();
        let connected = connect(&client().unwrap(), tokens, &fake.endpoints()).unwrap();
        GdriveRemote::new(
            client().unwrap(),
            &connected.account.account_id,
            connected.refresh_token,
            fake.endpoints(),
        )
        .unwrap()
    }

    const LOOPBACK: &str = "http://127.0.0.1:1234/";

    #[test]
    fn the_login_url_and_the_redirect() {
        let login = Login::new().unwrap();
        let url = Url::parse(&authorize_url(
            "cid",
            LOOPBACK,
            &login.pkce.challenge,
            &login.state,
        ))
        .unwrap();
        assert_eq!(url.host_str(), Some("accounts.google.com"));
        let pairs: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["scope"], SCOPE);
        assert_eq!(pairs["redirect_uri"], LOOPBACK);
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["state"], login.state);
        assert_eq!(pairs["access_type"], "offline");
        assert_eq!(pairs["prompt"], "select_account consent");
        // Only the waiting state's code.
        let good = format!("{LOOPBACK}?state={}&code=4/abc", login.state);
        assert_eq!(
            state_of_redirect(&good).as_deref(),
            Some(login.state.as_str())
        );
        assert_eq!(
            code_of_redirect(&good, &login.state).unwrap().as_str(),
            "4/abc"
        );
        for bad in [
            format!("{LOOPBACK}?state=other&code=4/abc"),
            format!("{LOOPBACK}?state={}&error=access_denied", login.state),
            format!("{LOOPBACK}?state={}", login.state),
            "nonsense".to_owned(),
        ] {
            assert_eq!(
                code_of_redirect(&bad, &login.state).unwrap_err().code(),
                "invalid_input:gdrive_redirect",
                "{bad}"
            );
        }
    }

    #[test]
    fn the_code_makes_the_folder_and_files_go_up_and_down() {
        let fake = FakeDrive::new();
        let remote = signed_in(&fake);
        assert!(fake.has_folder(), "the login made Keystead's folder");
        assert!(!remote.probe().unwrap().cas);
        assert!(remote.list("").unwrap().is_empty());
        let first = remote
            .put(&vault(), b"v1", Precondition::IfNoneMatch)
            .unwrap();
        let (bytes, version) = remote.get(&vault(), 100).unwrap();
        assert_eq!(bytes, b"v1");
        assert_eq!(version, first.etag);
        // A create over a file, a stale delete: conflicts.
        assert!(has_code(
            &remote
                .put(&vault(), b"x", Precondition::IfNoneMatch)
                .unwrap_err(),
            CHANGED
        ));
        remote.put(&vault(), b"v2", Precondition::None).unwrap();
        assert_eq!(remote.get(&vault(), 100).unwrap().0, b"v2");
        assert!(has_code(
            &remote
                .delete(&vault(), Precondition::IfMatch("stale".into()))
                .unwrap_err(),
            CHANGED
        ));
        // Attachments.
        let blobs = crate::remote::blob_dir_name(ID);
        remote.mkcol(&blobs).unwrap();
        remote.mkcol(&blobs).unwrap();
        let blob = format!("{blobs}/{ID}.bin");
        remote.put(&blob, b"blob", Precondition::None).unwrap();
        assert_eq!(remote.list(&blobs).unwrap().len(), 1);
        let mut names: Vec<_> = remote
            .list("")
            .unwrap()
            .into_iter()
            .map(|m| (m.name, m.dir))
            .collect();
        names.sort();
        assert_eq!(names, [(vault(), false), (blobs.clone(), true)]);
        remote.delete(&blob, Precondition::None).unwrap();
        assert!(has_code(&remote.get(&blob, 10).unwrap_err(), FILE_MISSING));
        remote.delete(&blobs, Precondition::None).unwrap();
        assert!(has_code(&remote.list(&blobs).unwrap_err(), FILE_MISSING));
        assert!(has_code(&remote.get(&vault(), 1).unwrap_err(), TOO_LARGE));
    }

    #[test]
    fn several_files_of_one_name_the_oldest_counts() {
        let fake = FakeDrive::new();
        let remote = signed_in(&fake);
        // Another device made the file first.
        fake.add_file(&vault(), b"older");
        fake.add_file(&vault(), b"newer");
        assert_eq!(remote.get(&vault(), 100).unwrap().0, b"older");
        assert_eq!(remote.list("").unwrap().len(), 1);
        // A create that loses the race is removed again.
        let fake2 = FakeDrive::new();
        let remote2 = signed_in(&fake2);
        fake2.before_create(&vault(), b"first");
        assert!(has_code(
            &remote2
                .put(&vault(), b"mine", Precondition::IfNoneMatch)
                .unwrap_err(),
            CHANGED
        ));
        assert_eq!(fake2.count(&vault()), 1);
        assert_eq!(remote2.get(&vault(), 100).unwrap().0, b"first");
    }

    #[test]
    fn expired_tokens_are_renewed_and_errors_are_codes() {
        let fake = FakeDrive::new();
        let remote = signed_in(&fake);
        remote.list("").unwrap();
        fake.expire_access_tokens();
        remote.list("").unwrap();
        fake.revoke_refresh_tokens();
        fake.expire_access_tokens();
        assert!(has_code(&remote.list("").unwrap_err(), AUTH_FAILED));
        let fake = FakeDrive::new();
        let remote = signed_in(&fake);
        for (status, reason, code) in [
            (403, "storageQuotaExceeded", QUOTA),
            (403, "userRateLimitExceeded", SERVER_ERROR),
            (403, "insufficientFilePermissions", NOT_WRITABLE),
            (500, "backendError", SERVER_ERROR),
        ] {
            fake.fail_next(status, reason);
            let e = remote.put(&vault(), b"x", Precondition::None).unwrap_err();
            assert!(has_code(&e, code), "{reason}: {e:?}");
        }
        // Hostile names are left out.
        fake.add_file("../evil", b"x");
        fake.add_file("a\\b", b"x");
        assert!(remote
            .list("")
            .unwrap()
            .iter()
            .all(|m| valid_name(&m.name) && !m.name.contains('\\')));
        // The folder gone: missing at the next pass – and within a pass
        // an empty listing is checked.
        assert!(remote.probe().unwrap().exists);
        fake.remove_folder();
        assert!(has_code(&remote.list("").unwrap_err(), FOLDER_MISSING));
        assert!(!remote.probe().unwrap().exists);
        assert!(has_code(&remote.list("").unwrap_err(), FOLDER_MISSING));
        // Another device made the folder again: found by name.
        fake.add_file(&vault(), b"x");
        assert!(remote.probe().unwrap().exists);
        assert_eq!(remote.list("").unwrap().len(), 1);
    }

    #[test]
    fn the_phone_login_is_a_device_code() {
        use super::fake::{Device, USER_CODE};
        let fake = FakeDrive::new();
        let client = client().unwrap();
        let login = start_device_login(&client, &fake.endpoints()).unwrap();
        assert_eq!(login.user_code, USER_CODE);
        assert_eq!(login.verification_url, "https://www.google.com/device");
        assert_eq!(login.interval, Duration::from_secs(5));
        assert!(!format!("{login:?}").contains("the-device-code"));
        let poll = |fake: &FakeDrive| {
            poll_device_login(&client, &login.device_code, &fake.endpoints()).unwrap()
        };
        assert!(matches!(poll(&fake), DevicePoll::Pending));
        fake.slow_down_next_poll();
        assert!(matches!(poll(&fake), DevicePoll::SlowDown));
        fake.set_device(Device::Allowed);
        let DevicePoll::Done(tokens) = poll(&fake) else {
            panic!("allowed: the tokens");
        };
        let connected = connect(&client, tokens, &fake.endpoints()).unwrap();
        assert_eq!(connected.account.account_id, "perm-1");
        assert!(fake.has_folder());
        // The code is used once.
        assert!(matches!(poll(&fake), DevicePoll::Expired));
        fake.set_device(Device::Refused);
        assert!(matches!(poll(&fake), DevicePoll::Refused));
        fake.set_device(Device::Expired);
        assert!(matches!(poll(&fake), DevicePoll::Expired));
        fake.fail_next_token(401, "invalid_client");
        assert_eq!(
            poll_device_login(&client, &login.device_code, &fake.endpoints())
                .unwrap_err()
                .code(),
            "unsupported:gdrive_not_configured"
        );
        // A page the answer names must be Google's, over https.
        for url in [
            "https://evil.example/device",
            "http://www.google.com/device",
            "javascript:alert(1)",
        ] {
            fake.set_device_url(url);
            assert!(
                has_code(
                    &start_device_login(&client, &fake.endpoints()).unwrap_err(),
                    BAD_RESPONSE
                ),
                "{url}"
            );
        }
    }

    #[test]
    fn a_code_google_refuses_and_a_client_it_does_not_know() {
        let fake = FakeDrive::new();
        let client = client().unwrap();
        let exchange = || {
            exchange_code(
                &client,
                &fake.code(),
                &fake.verifier(),
                LOOPBACK,
                &fake.endpoints(),
            )
        };
        for (status, error, code) in [
            (400, "invalid_grant", "invalid_input:gdrive_redirect"),
            (401, "invalid_client", "unsupported:gdrive_not_configured"),
            (
                400,
                "redirect_uri_mismatch",
                "unsupported:gdrive_not_configured",
            ),
            (500, "backend", "unsupported:sync_server_error"),
        ] {
            fake.fail_next_token(status, error);
            assert_eq!(exchange().unwrap_err().code(), code, "{error}");
        }
        assert!(exchange().is_ok());
        // A wrong code or verifier: Google's invalid_grant.
        assert_eq!(
            exchange_code(
                &client,
                "4/other",
                &fake.verifier(),
                LOOPBACK,
                &fake.endpoints()
            )
            .unwrap_err()
            .code(),
            "invalid_input:gdrive_redirect"
        );
    }

    #[test]
    fn a_delete_takes_every_file_of_the_name_and_endless_listings_end() {
        let fake = FakeDrive::new();
        let remote = signed_in(&fake);
        fake.add_file(&vault(), b"older");
        fake.add_file(&vault(), b"newer");
        let version = remote.get(&vault(), 100).unwrap().1.unwrap();
        remote
            .delete(&vault(), Precondition::IfMatch(version))
            .unwrap();
        assert_eq!(fake.count(&vault()), 0, "no newer copy comes back");
        assert!(has_code(
            &remote.get(&vault(), 100).unwrap_err(),
            FILE_MISSING
        ));
        fake.endless_pages();
        assert!(has_code(&remote.list("").unwrap_err(), BAD_RESPONSE));
    }

    #[test]
    fn long_listings_come_in_pages_and_stale_versions_conflict() {
        let fake = FakeDrive::new();
        let remote = signed_in(&fake);
        fake.set_page_size(1);
        let blobs = crate::remote::blob_dir_name(ID);
        remote
            .put(&vault(), b"v1", Precondition::IfNoneMatch)
            .unwrap();
        remote.mkcol(&blobs).unwrap();
        assert_eq!(remote.list("").unwrap().len(), 2);
        let read = remote.get(&vault(), 100).unwrap().1.unwrap();
        // Another device updates it: the version read is stale.
        remote.put(&vault(), b"v2", Precondition::None).unwrap();
        assert!(has_code(
            &remote
                .put(&vault(), b"v3", Precondition::IfMatch(read.clone()))
                .unwrap_err(),
            CHANGED
        ));
        let now = remote.get(&vault(), 100).unwrap().1.unwrap();
        remote
            .put(&vault(), b"v3", Precondition::IfMatch(now))
            .unwrap();
        assert_eq!(fake.file(&vault()).unwrap(), b"v3");
        // An update or delete of a file that went: changed, not created.
        remote.delete(&vault(), Precondition::None).unwrap();
        assert!(has_code(
            &remote
                .put(&vault(), b"v4", Precondition::IfMatch(read))
                .unwrap_err(),
            CHANGED
        ));
        assert_eq!(fake.count(&vault()), 0);
    }

    /// A browser's GET of `redirect` + `rest`: the whole answer.
    fn browse(redirect: &str, rest: &str) -> String {
        use std::io::{Read as _, Write as _};
        let url = Url::parse(redirect).unwrap();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        write!(stream, "GET /{rest} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn the_pc_login_waits_on_the_loopback() {
        let login = Login::new().unwrap();
        let (listener, redirect) = listen_loopback().unwrap();
        let state = login.state.clone();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let waiter = std::thread::spawn(move || serve_loopback(listener, &state, until, || true));
        // Another login's redirect and the favicon are not ours.
        assert!(browse(&redirect, "?state=other&code=x").starts_with("HTTP/1.1 404"));
        assert!(browse(&redirect, "favicon.ico").starts_with("HTTP/1.1 404"));
        let page = browse(
            &redirect,
            &format!("?state={}&code=4/the-code", login.state),
        );
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("abgeschlossen"));
        let code = waiter.join().unwrap().unwrap().unwrap();
        assert_eq!(code.as_str(), "4/the-code");

        // Refused: the error, and the wait ends.
        let (listener, redirect) = listen_loopback().unwrap();
        let state = login.state.clone();
        let waiter = std::thread::spawn(move || serve_loopback(listener, &state, until, || true));
        let page = browse(
            &redirect,
            &format!("?state={}&error=access_denied", login.state),
        );
        assert!(page.contains("nicht erlaubt"));
        assert_eq!(
            waiter.join().unwrap().unwrap().unwrap_err().code(),
            "invalid_input:gdrive_redirect"
        );
        // A login given up: the wait ends without a request.
        let (listener, _) = listen_loopback().unwrap();
        assert!(serve_loopback(listener, "s", until, || false).is_none());
    }
}
