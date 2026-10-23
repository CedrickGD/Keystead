//! A thin WebDAV client – [`WebDavRemote`] is a [`Remote`] on any WebDAV
//! server (Nextcloud, ownCloud, a NAS, `rclone serve webdav`).
//!
//! * The folder ("collection") is the server URL plus the folder name the
//!   user gave (default `Keystead`): [`collection_url`]. Only `https://`
//!   (debug builds also `http://` to `localhost` and the Android emulator's
//!   host `10.0.2.2`); no user name or password in the URL
//!   (`invalid_input:server_url`). The key the bases are recorded for is
//!   `webdav:<normalized folder URL>` – never credentials.
//! * Requests: `PROPFIND` (depth 1 for a listing, 0 for the probe; exactly
//!   the properties the sync uses, [`dav_xml::PROPFIND_BODY`]), `GET` (at
//!   most the caller's limit: the `Content-Length` first, then the body is
//!   counted and the read stops one byte past it), `PUT` with `If-Match` /
//!   `If-None-Match: *`, `MKCOL` (an existing folder is fine), `DELETE`.
//!   Basic authentication on every request (only ever over TLS outside
//!   debug builds). No redirects are followed; connect 15 s, read 30 s, no
//!   total timeout (`tls::client`).
//! * The client is reqwest's async one on a small runtime of its own
//!   (`block_on`): the sync engines are blocking and run on threads of their
//!   own; reqwest's blocking client has a total timeout only, no read
//!   timeout. Never call it from an async task.
//! * Errors are codes ([`super`]): no connection / a timeout →
//!   `sync_offline`; 401 (403 on a read) → `sync_auth_failed`; 403 on a
//!   write → `sync_not_writable`; the folder missing →
//!   `sync_server_folder_missing` (never created by a sync – only by the
//!   user's "Ordner anlegen", [`WebDavRemote::create_folder`]); no
//!   multistatus, a redirect, 405 for `PROPFIND` → `sync_server_not_webdav`;
//!   412 → `sync_remote_changed`; 507 → `sync_server_quota`; 5xx →
//!   `sync_server_error`; a malformed or too large answer →
//!   `sync_server_response`; an untrusted certificate →
//!   `sync_server_certificate`. Never a URL in an error or the log.

use std::sync::OnceLock;

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use reqwest::header::{
    HeaderMap, HeaderValue, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, LOCATION,
};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::dav_xml::{self, DavEntry};
use super::tls::{self, CertInfo, Trust};
use super::{
    check_dir, check_path, Access, Precondition, Remote, RemoteMeta, AUTH_FAILED, BAD_RESPONSE,
    CERTIFICATE, CHANGED, FILE_MISSING, FOLDER_MISSING, NOT_WEBDAV, NOT_WRITABLE, OFFLINE, QUOTA,
    SERVER_ERROR, TOO_LARGE,
};
use crate::error::{AppError, AppResult};

/// `invalid_input:server_url`: not a URL the sync may use.
pub const BAD_URL: &str = "server_url";
/// `invalid_input:server_folder`: not a folder name the sync may use.
pub const BAD_FOLDER: &str = "server_folder";
/// The folder name the form suggests.
pub const DEFAULT_FOLDER: &str = "Keystead";
/// The file of the `If-Match` probe (never one of the layout's names).
const CAS_PROBE: &str = ".keystead-cas-probe";

/// Characters escaped in a path segment.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// The password straight into a [`Zeroizing`] string.
fn zeroizing<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(d).map(Zeroizing::new)
}

fn default_folder() -> String {
    DEFAULT_FOLDER.to_owned()
}

/// A WebDAV server as the user entered it.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavConfig {
    /// The server (`https://cloud.example`) or a WebDAV folder on it
    /// (`https://cloud.example/remote.php/dav/files/alice/`).
    pub url: String,
    pub user: String,
    /// Never serialized, never logged.
    #[serde(deserialize_with = "zeroizing")]
    pub password: Zeroizing<String>,
    /// The folder below `url` the sync uses (created on request only).
    #[serde(default = "default_folder")]
    pub folder: String,
    /// The SHA-256 (hex) of the leaf certificate the user trusted, if the
    /// server's is not trusted by the system.
    #[serde(default)]
    pub cert_sha256: Option<String>,
}

impl std::fmt::Debug for WebDavConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavConfig")
            .field("folder", &self.folder)
            .field("cert_sha256", &self.cert_sha256)
            .finish_non_exhaustive()
    }
}

impl WebDavConfig {
    fn trust(&self) -> AppResult<Trust> {
        match &self.cert_sha256 {
            None => Ok(Trust::System),
            Some(text) => tls::parse_sha256(text)
                .map(Trust::Pinned)
                .ok_or_else(|| AppError::invalid("cert_sha256")),
        }
    }
}

/// What the connection test found (`webdav_test`). No secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavTest {
    /// The folder exists and can be used.
    pub ok: bool,
    /// The error code otherwise.
    pub code: Option<String>,
    /// The server's certificate is not trusted: its facts, for the user to
    /// confirm (`certSha256`).
    pub certificate: Option<CertInfo>,
    /// The URL the server points to instead (a redirect to https on the
    /// same host, or Nextcloud's WebDAV folder for a bare host name).
    pub suggested_url: Option<String>,
    /// The server is fine, the folder does not exist yet ("Ordner anlegen").
    pub folder_missing: bool,
    /// The user may write the folder (as far as the server says).
    pub writable: bool,
    /// The server honours `If-Match` (an atomic check before each write).
    pub cas: bool,
}

/// Whether `url` may be used: https, or (debug builds) http to this
/// machine or the Android emulator's host; no credentials, query or
/// fragment in it.
fn check_url(url: &Url) -> AppResult<()> {
    let bad = || AppError::invalid(BAD_URL);
    let local_http = cfg!(debug_assertions)
        && url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "10.0.2.2")
        );
    if !(url.scheme() == "https" || local_http) {
        return Err(bad());
    }
    if url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(bad());
    }
    Ok(())
}

/// The server URL as entered, checked and with a trailing `/`.
pub fn base_url(url: &str) -> AppResult<Url> {
    let mut url = Url::parse(url.trim()).map_err(|_| AppError::invalid(BAD_URL))?;
    check_url(&url)?;
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

/// The sync's folder: `url` (checked, [`base_url`]) plus the folder name
/// `folder` (segments separated by `/`; none empty, `.`, `..`, with `\` or
/// control characters), ending with `/`.
pub fn collection_url(url: &str, folder: &str) -> AppResult<Url> {
    let mut collection = base_url(url)?;
    let mut path = collection.path().to_owned();
    for segment in folder.split('/').filter(|s| !s.is_empty()) {
        if segment == "."
            || segment == ".."
            || segment.len() > 255
            || segment.contains('\\')
            || segment.chars().any(char::is_control)
        {
            return Err(AppError::invalid(BAD_FOLDER));
        }
        path.push_str(&utf8_percent_encode(segment, SEGMENT).to_string());
        path.push('/');
    }
    collection.set_path(&path);
    check_url(&collection)?;
    Ok(collection)
}

/// The key of a server folder: `webdav:<normalized folder URL>`.
pub fn key_of(collection: &Url) -> String {
    format!("webdav:{collection}")
}

/// Nextcloud's WebDAV folder of `user` on the server of `base` – for a
/// bare host name (`https://cloud.example`), where the folder is
/// `/remote.php/dav/files/<user>/`.
pub fn nextcloud_url(base: &Url, user: &str) -> Option<Url> {
    if base.path() != "/" || user.is_empty() {
        return None;
    }
    let mut url = base.clone();
    url.set_path(&format!(
        "/remote.php/dav/files/{}/",
        utf8_percent_encode(user, SEGMENT)
    ));
    Some(url)
}

/// The code of a failed request (no answer): a TLS error anywhere in its
/// chain (`io::Error`s wrap it, nested) decides; otherwise the server
/// could not be reached.
fn network_error(e: &reqwest::Error) -> AppError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = source {
        match err.downcast_ref::<rustls::Error>() {
            Some(rustls::Error::InvalidCertificate(_)) => {
                return AppError::unsupported(CERTIFICATE)
            }
            Some(_) => return AppError::unsupported(SERVER_ERROR),
            None => {}
        }
        source = match err
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
        {
            Some(inner) => Some(inner as &(dyn std::error::Error + 'static)),
            None => err.source(),
        };
    }
    AppError::unsupported(OFFLINE)
}

/// What a request is, for the status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// A read of the folder itself (`PROPFIND` of the top level, the probe).
    Folder,
    /// A read of a file or attachment folder.
    Read,
    /// A write of a top-level file.
    WriteTop,
    /// A write in an attachment folder (or of one).
    WriteSub,
}

/// The code of an answer that is not a success.
fn status_error(status: StatusCode, op: Op) -> AppError {
    let code = match status.as_u16() {
        401 => AUTH_FAILED,
        403 if matches!(op, Op::WriteTop | Op::WriteSub) => NOT_WRITABLE,
        403 => AUTH_FAILED,
        404 | 409 => match op {
            Op::Folder | Op::WriteTop => FOLDER_MISSING,
            Op::Read | Op::WriteSub => FILE_MISSING,
        },
        412 => CHANGED,
        413 => TOO_LARGE,
        507 => QUOTA,
        300..=399 | 405 | 501 if matches!(op, Op::Folder) => NOT_WEBDAV,
        _ => SERVER_ERROR,
    };
    AppError::unsupported(code)
}

/// A strong ETag header.
fn etag_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .and_then(dav_xml::strong_etag)
}

/// A WebDAV folder (see the module docs).
pub struct WebDavRemote {
    collection: Url,
    key: String,
    user: String,
    password: Zeroizing<String>,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
    /// The `If-Match` probe's answer, once known.
    cas: OnceLock<bool>,
}

impl std::fmt::Debug for WebDavRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavRemote").finish_non_exhaustive()
    }
}

impl WebDavRemote {
    /// The folder of `cfg` (no request yet).
    pub fn new(cfg: &WebDavConfig) -> AppResult<Self> {
        let collection = collection_url(&cfg.url, &cfg.folder)?;
        let client = tls::client(cfg.trust()?)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(AppError::from)?;
        Ok(WebDavRemote {
            key: key_of(&collection),
            collection,
            user: cfg.user.clone(),
            password: cfg.password.clone(),
            client,
            runtime,
            cas: OnceLock::new(),
        })
    }

    /// `path` (relative, validated by the caller) in the folder; `dir`: as
    /// a folder (trailing `/`).
    fn url(&self, path: &str, dir: bool) -> Url {
        let mut url = self.collection.clone();
        let mut full = url.path().to_owned();
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            full.push_str(&utf8_percent_encode(segment, SEGMENT).to_string());
            full.push('/');
        }
        if !dir && full.ends_with('/') && !path.is_empty() {
            full.pop();
        }
        url.set_path(&full);
        url
    }

    /// Sends a request and returns the answer's status, headers and – for
    /// a success – its body (at most `max` bytes, `too_large` beyond).
    fn send(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        max: u64,
        too_large: &str,
    ) -> AppResult<(StatusCode, HeaderMap, Vec<u8>)> {
        let mut request = self
            .client
            .request(method, url)
            .headers(headers)
            .basic_auth(&self.user, Some(self.password.as_str()));
        if let Some(body) = body {
            request = request.body(body);
        }
        self.runtime.block_on(async move {
            let mut response = request.send().await.map_err(|e| network_error(&e))?;
            let status = response.status();
            let headers = response.headers().clone();
            let mut bytes = Vec::new();
            if status.is_success() {
                if response.content_length().is_some_and(|n| n > max) {
                    return Err(AppError::unsupported(too_large));
                }
                while let Some(chunk) = response.chunk().await.map_err(|e| network_error(&e))? {
                    if (bytes.len() + chunk.len()) as u64 > max {
                        return Err(AppError::unsupported(too_large));
                    }
                    bytes.extend_from_slice(&chunk);
                }
            }
            Ok((status, headers, bytes))
        })
    }

    /// `PROPFIND` of `url` (`depth` 0 or 1): the status and, for 207, the
    /// entries.
    fn propfind(&self, url: Url, depth: u8) -> AppResult<(StatusCode, Option<Vec<DavEntry>>)> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Depth",
            HeaderValue::from_static(if depth == 0 { "0" } else { "1" }),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        );
        let method = Method::from_bytes(b"PROPFIND").map_err(|_| AppError::invalid("method"))?;
        let (status, _, body) = self.send(
            method,
            url,
            headers,
            Some(dav_xml::PROPFIND_BODY.as_bytes().to_vec()),
            dav_xml::BODY_MAX as u64,
            BAD_RESPONSE,
        )?;
        if status == StatusCode::MULTI_STATUS {
            return Ok((status, Some(dav_xml::parse_multistatus(&body)?)));
        }
        Ok((status, None))
    }

    /// The entries of the folder `url` (depth 1), the folder's own first.
    fn list_url(&self, url: &Url, op: Op) -> AppResult<(DavEntry, Vec<RemoteMeta>)> {
        let (status, entries) = self.propfind(url.clone(), 1)?;
        let Some(entries) = entries else {
            if status.is_success() {
                return Err(AppError::unsupported(NOT_WEBDAV));
            }
            return Err(status_error(status, op));
        };
        let mut own = None;
        let mut children = Vec::new();
        for entry in entries {
            match dav_xml::child_name(&entry.href, url)? {
                None => own = Some(entry),
                Some(name) => children.push(RemoteMeta {
                    name,
                    dir: entry.collection,
                    size: if entry.collection { None } else { entry.size },
                    etag: entry.etag.as_deref().and_then(dav_xml::strong_etag),
                    modified: entry.modified,
                }),
            }
        }
        let own = own.ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
        if !own.collection {
            return Err(AppError::unsupported(NOT_WEBDAV));
        }
        Ok((own, children))
    }

    /// Whether the server honours `If-Match`: a `PUT` of the probe file
    /// with an ETag no file has must fail (412); one that goes through is
    /// removed again.
    fn cas_probe(&self) -> AppResult<bool> {
        let mut url = self.collection.clone();
        url.set_path(&format!("{}{CAS_PROBE}", self.collection.path()));
        let mut headers = HeaderMap::new();
        headers.insert(
            IF_MATCH,
            HeaderValue::from_static("\"keystead-no-such-version\""),
        );
        let (status, _, _) = self.send(
            Method::PUT,
            url.clone(),
            headers,
            Some(Vec::new()),
            0,
            BAD_RESPONSE,
        )?;
        if status == StatusCode::PRECONDITION_FAILED {
            return Ok(true);
        }
        if status.is_success() {
            let _ = self.send(Method::DELETE, url, HeaderMap::new(), None, 0, BAD_RESPONSE);
        }
        Ok(false)
    }

    /// The folder itself (`PROPFIND` depth 0): `None` if it does not exist.
    fn folder_entry(&self) -> AppResult<Option<DavEntry>> {
        let (status, entries) = self.propfind(self.collection.clone(), 0)?;
        match entries {
            Some(entries) => {
                let own = entries
                    .into_iter()
                    .find(|e| {
                        dav_xml::child_name(&e.href, &self.collection).is_ok_and(|n| n.is_none())
                    })
                    .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
                if !own.collection {
                    return Err(AppError::unsupported(NOT_WEBDAV));
                }
                Ok(Some(own))
            }
            None if status == StatusCode::NOT_FOUND => Ok(None),
            None if status.is_success() => Err(AppError::unsupported(NOT_WEBDAV)),
            None => Err(status_error(status, Op::Folder)),
        }
    }

    /// "Ordner anlegen": creates the folder (and the folders of its name
    /// above it, below the server URL). One that exists is fine.
    pub fn create_folder(&self, cfg: &WebDavConfig) -> AppResult<()> {
        let mut url = base_url(&cfg.url)?;
        let mut path = url.path().to_owned();
        for segment in cfg.folder.split('/').filter(|s| !s.is_empty()) {
            path.push_str(&utf8_percent_encode(segment, SEGMENT).to_string());
            path.push('/');
            url.set_path(&path);
            let method = Method::from_bytes(b"MKCOL").map_err(|_| AppError::invalid("method"))?;
            let (status, _, _) =
                self.send(method, url.clone(), HeaderMap::new(), None, 0, BAD_RESPONSE)?;
            if !(status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED) {
                return Err(status_error(status, Op::WriteTop));
            }
        }
        Ok(())
    }

    /// The connection test of the settings form (`webdav_test`): the
    /// server URL, the certificate, the credentials, the folder.
    pub fn test(cfg: &WebDavConfig) -> WebDavTest {
        let mut result = WebDavTest::default();
        let fail = |mut result: WebDavTest, e: AppError| {
            result.code = Some(e.code());
            result
        };
        let base = match base_url(&cfg.url) {
            Ok(base) => base,
            Err(e) => return fail(result, e),
        };
        let remote = match WebDavRemote::new(cfg) {
            Ok(remote) => remote,
            Err(e) => return fail(result, e),
        };
        // The server URL itself: a WebDAV folder?
        let (status, headers, _) = match remote.propfind_raw(&base) {
            Ok(answer) => answer,
            Err(e) => {
                // Untrusted, or no longer the confirmed one: its facts, for
                // the user to confirm (again).
                if super::has_code(&e, CERTIFICATE) {
                    result.certificate = tls::fetch_certificate(&base).ok();
                }
                return fail(result, e);
            }
        };
        if status != StatusCode::MULTI_STATUS {
            // Where it may be instead: the redirect's target (same host), or
            // Nextcloud's folder for a bare host name – if it answers.
            let mut candidates = Vec::new();
            if status.is_redirection() {
                candidates.extend(
                    headers
                        .get(LOCATION)
                        .and_then(|l| l.to_str().ok())
                        .and_then(|l| base.join(l).ok())
                        .filter(|l| l.host() == base.host() && check_url(l).is_ok()),
                );
            }
            if status != StatusCode::UNAUTHORIZED {
                candidates.extend(nextcloud_url(&base, &cfg.user));
            }
            result.suggested_url = candidates
                .into_iter()
                .find(|url| {
                    remote
                        .propfind_raw(url)
                        .is_ok_and(|(s, _, _)| s == StatusCode::MULTI_STATUS)
                })
                .map(|url| url.to_string());
            let e = if status.is_success() || status.is_redirection() {
                AppError::unsupported(NOT_WEBDAV)
            } else {
                status_error(status, Op::Folder)
            };
            return fail(result, e);
        }
        match remote.probe() {
            Ok(access) if access.exists => {
                result.ok = true;
                result.writable = access.writable;
                result.cas = access.cas;
                result
            }
            Ok(_) => {
                result.folder_missing = true;
                fail(result, AppError::unsupported(FOLDER_MISSING))
            }
            Err(e) => fail(result, e),
        }
    }

    /// `PROPFIND` depth 0 of `url` as it answers (no redirect followed).
    fn propfind_raw(&self, url: &Url) -> AppResult<(StatusCode, HeaderMap, Vec<u8>)> {
        let mut headers = HeaderMap::new();
        headers.insert("Depth", HeaderValue::from_static("0"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        );
        let method = Method::from_bytes(b"PROPFIND").map_err(|_| AppError::invalid("method"))?;
        self.send(
            method,
            url.clone(),
            headers,
            Some(dav_xml::PROPFIND_BODY.as_bytes().to_vec()),
            dav_xml::BODY_MAX as u64,
            BAD_RESPONSE,
        )
    }
}

impl Remote for WebDavRemote {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn probe(&self) -> AppResult<Access> {
        let Some(own) = self.folder_entry()? else {
            return Ok(Access {
                exists: false,
                writable: false,
                cas: false,
            });
        };
        // Nextcloud/ownCloud say what the user may do there ("C": create
        // files); other servers say nothing – a write will tell.
        let writable = own.permissions.as_deref().is_none_or(|p| p.contains('C'));
        let cas = match self.cas.get() {
            Some(cas) => *cas,
            None if writable => {
                let cas = self.cas_probe()?;
                let _ = self.cas.set(cas);
                cas
            }
            None => false,
        };
        Ok(Access {
            exists: true,
            writable,
            cas,
        })
    }

    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>> {
        check_dir(dir)?;
        let op = if dir.is_empty() { Op::Folder } else { Op::Read };
        let url = self.url(dir, true);
        Ok(self.list_url(&url, op)?.1)
    }

    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)> {
        check_path(path)?;
        let (status, headers, body) = self.send(
            Method::GET,
            self.url(path, false),
            HeaderMap::new(),
            None,
            max,
            TOO_LARGE,
        )?;
        if status != StatusCode::OK {
            return Err(status_error(status, Op::Read));
        }
        Ok((body, etag_of(&headers)))
    }

    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta> {
        check_path(path)?;
        let mut headers = HeaderMap::new();
        match &pre {
            Precondition::None => {}
            Precondition::IfMatch(etag) => {
                let value =
                    HeaderValue::from_str(etag).map_err(|_| AppError::unsupported(BAD_RESPONSE))?;
                headers.insert(IF_MATCH, value);
            }
            Precondition::IfNoneMatch => {
                headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
            }
        }
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        let op = if path.contains('/') {
            Op::WriteSub
        } else {
            Op::WriteTop
        };
        let (status, answer, _) = self.send(
            Method::PUT,
            self.url(path, false),
            headers,
            Some(bytes.to_vec()),
            64 * 1024,
            BAD_RESPONSE,
        )?;
        if !status.is_success() {
            return Err(status_error(status, op));
        }
        Ok(RemoteMeta {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            dir: false,
            size: Some(bytes.len() as u64),
            etag: etag_of(&answer),
            modified: None,
        })
    }

    fn mkcol(&self, dir: &str) -> AppResult<()> {
        check_dir(dir)?;
        if dir.is_empty() {
            return Err(AppError::invalid(super::BAD_PATH));
        }
        let method = Method::from_bytes(b"MKCOL").map_err(|_| AppError::invalid("method"))?;
        let (status, _, _) = self.send(
            method,
            self.url(dir, true),
            HeaderMap::new(),
            None,
            64 * 1024,
            BAD_RESPONSE,
        )?;
        // 405: it exists.
        if status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        Err(status_error(status, Op::WriteTop))
    }

    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()> {
        check_path(path)?;
        let dir = super::blob_dir_id(path).is_some();
        let mut headers = HeaderMap::new();
        if let Precondition::IfMatch(etag) = &pre {
            let value =
                HeaderValue::from_str(etag).map_err(|_| AppError::unsupported(BAD_RESPONSE))?;
            headers.insert(IF_MATCH, value);
        }
        let (status, _, _) = self.send(
            Method::DELETE,
            self.url(path, dir),
            headers,
            None,
            64 * 1024,
            BAD_RESPONSE,
        )?;
        if status.is_success() {
            return Ok(());
        }
        Err(status_error(status, Op::WriteSub))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::test_server::{Hostile, TestServer};
    use crate::remote::{blob_dir_name, has_code};

    const ID: &str = "abc";

    fn cfg(url: &str, password: &str) -> WebDavConfig {
        WebDavConfig {
            url: url.to_owned(),
            user: "alice".to_owned(),
            password: Zeroizing::new(password.to_owned()),
            folder: DEFAULT_FOLDER.to_owned(),
            cert_sha256: None,
        }
    }

    fn code(e: AppError) -> String {
        e.code()
    }

    #[test]
    fn urls_keys_and_folders() {
        let url = collection_url(
            "https://Cloud.Example:443/remote.php/dav/files/alice",
            "Keystead",
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://cloud.example/remote.php/dav/files/alice/Keystead/"
        );
        assert_eq!(
            key_of(&url),
            "webdav:https://cloud.example/remote.php/dav/files/alice/Keystead/"
        );
        assert_eq!(
            collection_url("https://nas.local:5006/", "Meine Tresore/2026")
                .unwrap()
                .as_str(),
            "https://nas.local:5006/Meine%20Tresore/2026/"
        );
        assert_eq!(
            collection_url("https://nas.local/dav/", "")
                .unwrap()
                .as_str(),
            "https://nas.local/dav/"
        );
        for bad in [
            "http://cloud.example/",
            "https://alice:pw@cloud.example/",
            "https://alice@cloud.example/",
            "https://cloud.example/?x=1",
            "https://cloud.example/#x",
            "ftp://cloud.example/",
            "file:///C:/Users",
            "mailto:a@b.c",
            "cloud.example",
            "",
        ] {
            assert_eq!(
                code(collection_url(bad, "Keystead").unwrap_err()),
                "invalid_input:server_url",
                "{bad}"
            );
        }
        for bad in ["..", "a/../b", "a\\b", "x\u{0}"] {
            assert_eq!(
                code(collection_url("https://nas.local/", bad).unwrap_err()),
                "invalid_input:server_folder",
                "{bad:?}"
            );
        }
        // Debug builds (these tests): http to this machine and the
        // emulator's host only.
        for local in [
            "http://127.0.0.1:8080/",
            "http://localhost/",
            "http://10.0.2.2:8443/",
        ] {
            assert!(collection_url(local, "Keystead").is_ok(), "{local}");
        }
        // Nextcloud's folder for a bare host name.
        let base = base_url("https://cloud.example").unwrap();
        assert_eq!(
            nextcloud_url(&base, "alice m").unwrap().as_str(),
            "https://cloud.example/remote.php/dav/files/alice%20m/"
        );
        assert_eq!(
            nextcloud_url(&base_url("https://cloud.example/dav").unwrap(), "a"),
            None
        );
        // No password in the debug output.
        assert!(!format!("{:?}", cfg("https://x/", "geheim")).contains("geheim"));
    }

    #[test]
    fn files_folders_etags_and_preconditions() {
        let server = TestServer::http("alice", "pw");
        server.mkdir("Keystead");
        let remote = WebDavRemote::new(&cfg(&server.base, "pw")).unwrap();
        let access = remote.probe().unwrap();
        assert_eq!(
            access,
            Access {
                exists: true,
                writable: true,
                cas: true
            }
        );
        assert_eq!(server.file("Keystead/.keystead-cas-probe"), None);
        assert!(remote.list("").unwrap().is_empty());

        let name = format!("{ID}.keystead");
        let first = remote
            .put(&name, b"one", Precondition::IfNoneMatch)
            .unwrap();
        let etag = first.etag.clone().unwrap();
        let listed = remote.list("").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, name);
        assert_eq!(listed[0].size, Some(3));
        assert_eq!(listed[0].etag.as_deref(), Some(etag.as_str()));
        assert!(listed[0].modified.is_some());
        assert_eq!(
            remote.get(&name, 100).unwrap(),
            (b"one".to_vec(), Some(etag.clone()))
        );
        assert!(has_code(
            &remote
                .put(&name, b"x", Precondition::IfNoneMatch)
                .unwrap_err(),
            CHANGED
        ));
        assert!(has_code(
            &remote
                .put(&name, b"x", Precondition::IfMatch("\"other\"".into()))
                .unwrap_err(),
            CHANGED
        ));
        let second = remote
            .put(&name, b"two", Precondition::IfMatch(etag))
            .unwrap();
        assert_ne!(second.etag, first.etag);
        assert_eq!(server.file(&format!("Keystead/{name}")).unwrap(), b"two");
        // At most the limit.
        assert!(has_code(&remote.get(&name, 2).unwrap_err(), TOO_LARGE));
        assert!(has_code(
            &remote.get("zzz.keystead", 10).unwrap_err(),
            FILE_MISSING
        ));

        // Blobs: their folder first.
        let dir = blob_dir_name(ID);
        let blob = format!("{dir}/b1.bin");
        assert!(has_code(
            &remote.put(&blob, b"blob", Precondition::None).unwrap_err(),
            FILE_MISSING
        ));
        assert!(has_code(&remote.list(&dir).unwrap_err(), FILE_MISSING));
        remote.mkcol(&dir).unwrap();
        remote.mkcol(&dir).unwrap();
        remote.put(&blob, b"blob", Precondition::None).unwrap();
        let top = remote.list("").unwrap();
        assert!(top
            .iter()
            .any(|m| m.dir && m.name == dir && m.size.is_none()));
        assert_eq!(remote.list(&dir).unwrap()[0].name, "b1.bin");
        remote.delete(&blob, Precondition::None).unwrap();
        remote.delete(&dir, Precondition::None).unwrap();
        assert!(has_code(
            &remote.delete(&blob, Precondition::None).unwrap_err(),
            FILE_MISSING
        ));
        let now = remote.list("").unwrap()[0].etag.clone().unwrap();
        assert!(has_code(
            &remote
                .delete(&name, Precondition::IfMatch("\"old\"".into()))
                .unwrap_err(),
            CHANGED
        ));
        remote.delete(&name, Precondition::IfMatch(now)).unwrap();
        // Only the layout's paths.
        assert_eq!(
            code(remote.get("../secret", 10).unwrap_err()),
            "invalid_input:remote_path"
        );
        assert_eq!(
            code(
                remote
                    .put(".keystead-cas-probe", b"", Precondition::None)
                    .unwrap_err()
            ),
            "invalid_input:remote_path"
        );
        assert_eq!(
            code(remote.list("x").unwrap_err()),
            "invalid_input:remote_path"
        );

        // A wrong password.
        let wrong = WebDavRemote::new(&cfg(&server.base, "falsch")).unwrap();
        assert!(has_code(&wrong.list("").unwrap_err(), AUTH_FAILED));
        assert!(has_code(&wrong.probe().unwrap_err(), AUTH_FAILED));
    }

    #[test]
    fn the_connection_test_and_the_missing_folder() {
        let server = TestServer::http("alice", "pw");
        let config = cfg(&server.base, "pw");
        let remote = WebDavRemote::new(&config).unwrap();
        assert!(!remote.probe().unwrap().exists);
        assert!(has_code(&remote.list("").unwrap_err(), FOLDER_MISSING));
        assert!(has_code(
            &remote
                .put("abc.keystead", b"x", Precondition::None)
                .unwrap_err(),
            FOLDER_MISSING
        ));
        let test = WebDavRemote::test(&config);
        assert!(!test.ok);
        assert!(test.folder_missing);
        assert_eq!(
            test.code.as_deref(),
            Some("unsupported:sync_server_folder_missing")
        );
        // "Ordner anlegen" (nested names too).
        remote.create_folder(&config).unwrap();
        remote.create_folder(&config).unwrap();
        let test = WebDavRemote::test(&config);
        assert_eq!(
            test,
            WebDavTest {
                ok: true,
                writable: true,
                cas: true,
                ..WebDavTest::default()
            }
        );
        let mut nested = config.clone();
        nested.folder = "a/b".into();
        WebDavRemote::new(&nested)
            .unwrap()
            .create_folder(&nested)
            .unwrap();
        assert!(WebDavRemote::test(&nested).ok);

        // A wrong password, a server that is not there.
        let test = WebDavRemote::test(&cfg(&server.base, "falsch"));
        assert_eq!(test.code.as_deref(), Some("unsupported:sync_auth_failed"));
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = format!("http://{}/dav/", closed.local_addr().unwrap());
        drop(closed);
        let test = WebDavRemote::test(&cfg(&gone, "pw"));
        assert_eq!(test.code.as_deref(), Some("unsupported:sync_offline"));
        assert!(has_code(
            &WebDavRemote::new(&cfg(&gone, "pw"))
                .unwrap()
                .list("")
                .unwrap_err(),
            OFFLINE
        ));
        // A URL that is not allowed.
        let test = WebDavRemote::test(&cfg("http://cloud.example/", "pw"));
        assert_eq!(test.code.as_deref(), Some("invalid_input:server_url"));

        // A redirect to the server's WebDAV folder: suggested.
        let root = server.base.trim_end_matches("dav/").to_owned();
        server.set_hostile(Hostile::Redirect("/dav/".into()));
        let test = WebDavRemote::test(&cfg(&format!("{root}old/"), "pw"));
        assert_eq!(
            test.code.as_deref(),
            Some("unsupported:sync_server_not_webdav")
        );
        assert_eq!(test.suggested_url.as_deref(), Some(server.base.as_str()));
        // To another host: not suggested.
        server.set_hostile(Hostile::Redirect("https://evil.example/dav/".into()));
        let test = WebDavRemote::test(&config);
        assert_eq!(test.suggested_url, None);
        assert!(has_code(&remote.list("").unwrap_err(), NOT_WEBDAV));
    }

    #[test]
    fn hostile_answers() {
        let server = TestServer::http("alice", "pw");
        server.mkdir("Keystead");
        server.put("Keystead/abc.keystead", b"vault");
        let remote = WebDavRemote::new(&cfg(&server.base, "pw")).unwrap();
        assert_eq!(remote.list("").unwrap().len(), 1);
        for (hostile, expected) in [
            (Hostile::OversizedPropfind, BAD_RESPONSE),
            (Hostile::DocTypeBomb, BAD_RESPONSE),
            (Hostile::ForeignHref, BAD_RESPONSE),
            (Hostile::DotDotHref, BAD_RESPONSE),
            (Hostile::EncodedSlashHref, BAD_RESPONSE),
            (Hostile::Status(401), AUTH_FAILED),
            (Hostile::Status(405), NOT_WEBDAV),
            (Hostile::Status(500), SERVER_ERROR),
            (Hostile::Status(507), QUOTA),
            (Hostile::Redirect("/elsewhere/".into()), NOT_WEBDAV),
        ] {
            server.set_hostile(hostile.clone());
            let e = remote.list("").unwrap_err();
            assert!(has_code(&e, expected), "{hostile:?}: {}", e.code());
        }
        server.set_hostile(Hostile::Status(507));
        assert!(has_code(
            &remote
                .put("abc.keystead", b"x", Precondition::None)
                .unwrap_err(),
            QUOTA
        ));
        server.set_hostile(Hostile::Status(403));
        assert!(has_code(
            &remote
                .put("abc.keystead", b"x", Precondition::None)
                .unwrap_err(),
            NOT_WRITABLE
        ));
        assert!(has_code(
            &remote.get("abc.keystead", 100).unwrap_err(),
            AUTH_FAILED
        ));
        // A body without a length that goes on: cut at the limit.
        server.set_hostile(Hostile::EndlessGet(64 * 1024));
        assert!(has_code(
            &remote.get("abc.keystead", 1000).unwrap_err(),
            TOO_LARGE
        ));
        // A server that ignores If-Match: no CAS (the probe file goes again).
        server.set_hostile(Hostile::IgnoreIfMatch);
        let fresh = WebDavRemote::new(&cfg(&server.base, "pw")).unwrap();
        assert!(!fresh.probe().unwrap().cas);
        assert_eq!(server.file("Keystead/.keystead-cas-probe"), None);
        server.set_hostile(Hostile::None);
        assert_eq!(remote.list("").unwrap().len(), 1);
    }

    #[test]
    fn an_own_certificate_is_trusted_only_once_confirmed() {
        let server = TestServer::https("alice", "pw");
        server.mkdir("Keystead");
        let der = server.cert_der.clone().unwrap();
        let sha = tls::hex(&tls::sha256_of(&der));
        // Not trusted by the system: its facts, for the user to confirm.
        let mut config = cfg(&server.base, "pw");
        let test = WebDavRemote::test(&config);
        assert_eq!(
            test.code.as_deref(),
            Some("unsupported:sync_server_certificate")
        );
        let cert = test.certificate.unwrap();
        assert_eq!(cert.sha256, sha);
        assert!(server.take_log().is_empty(), "nothing was sent");
        // Confirmed: it works.
        config.cert_sha256 = Some(sha.to_uppercase());
        let test = WebDavRemote::test(&config);
        assert!(test.ok, "{test:?}");
        let remote = WebDavRemote::new(&config).unwrap();
        remote
            .put("abc.keystead", b"vault", Precondition::IfNoneMatch)
            .unwrap();
        assert_eq!(remote.get("abc.keystead", 100).unwrap().0, b"vault");
        // Another certificate than the confirmed one: refused.
        config.cert_sha256 = Some("00".repeat(32));
        let other = WebDavRemote::new(&config).unwrap();
        assert!(has_code(&other.list("").unwrap_err(), CERTIFICATE));
        let test = WebDavRemote::test(&config);
        assert_eq!(
            test.code.as_deref(),
            Some("unsupported:sync_server_certificate")
        );
        // The one it has now, to confirm again.
        assert_eq!(test.certificate.unwrap().sha256, sha);
        // A malformed fingerprint.
        config.cert_sha256 = Some("xyz".into());
        assert_eq!(
            code(WebDavRemote::new(&config).unwrap_err()),
            "invalid_input:cert_sha256"
        );
    }
}
