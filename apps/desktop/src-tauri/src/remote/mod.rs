//! The sync's I/O seam to a storage server (stage 2c, see "Remote backends"
//! in docs/ARCHITECTURE.md).
//!
//! Both sync engines keep their logic – the phone's `folder_sync` and the
//! PC's `mirror` – and reach a server through one small trait, [`Remote`]:
//! a folder ("collection") holding the same layout as the cloud folder,
//! `<id>.keystead`, `<id>.keystead.attachments/<attachment id>.bin` and the
//! phone's conflict copies. Adapters put the engines on top of it: the
//! phone's [`folder::RemoteDirFolder`] (a `folder_sync::RemoteFolder`) and
//! the PC's `mirror::RemoteStore` (a `mirror::MirrorStore`).
//!
//! * Paths are relative to the collection and validated
//!   ([`check_path`]): one or two segments of exactly the names the layout
//!   has – nothing else is ever read, written or removed.
//! * Writes are conditional ([`Precondition`]): `If-Match` with the ETag a
//!   read saw, `If-None-Match: *` for a file that must not exist yet. A
//!   precondition that fails is `unsupported:sync_remote_changed`
//!   ([`CHANGED`]) – the engines look again. A server that ignores the
//!   preconditions loses the atomicity only: the engines still read the
//!   copy again before a write and the authenticated ancestry (sync proof)
//!   turns a write that went over another one into a conflict on the other
//!   side, never into a silent pull.
//! * Errors are codes (`unsupported:<code>`), never URLs or credentials.
//!
//! Backends: [`webdav::WebDavRemote`] (any WebDAV server: Nextcloud,
//! ownCloud, a NAS, `rclone serve webdav`) and, in the unit tests,
//! `mem::MemRemote`.

// Wired to the phone's link (WP4) and the desktop's mirror (WP5) in later
// steps of stage 2c; until then much of it is reached by the unit tests only.
#![cfg_attr(not(test), expect(dead_code))]

use std::sync::Arc;

use keystead_core::{attachments, sync};
use serde::Serialize;

use crate::error::{AppError, AppResult};

pub mod dav_xml;
#[cfg(any(mobile, test))]
pub mod folder;
#[cfg(test)]
pub mod mem;
#[cfg(test)]
mod test_server;
pub mod tls;
pub mod webdav;

/// Error code: the server cannot be reached (DNS, connection refused, a
/// timeout, no network). Ends a sync; nothing changes.
pub const OFFLINE: &str = "sync_offline";
/// Error code: the server refused the user name or password (HTTP 401).
pub const AUTH_FAILED: &str = "sync_auth_failed";
/// Error code: the server does not let the user write there (HTTP 403 on a
/// write). Same code as the phone's folder without write permission.
pub const NOT_WRITABLE: &str = "sync_not_writable";
/// Error code: the server's folder (collection) does not exist. It is
/// never created by a sync – only by the user's "Ordner anlegen".
pub const FOLDER_MISSING: &str = "sync_server_folder_missing";
/// Error code: the address is no WebDAV folder (no multistatus answer, a
/// redirect, HTTP 405 for PROPFIND).
pub const NOT_WEBDAV: &str = "sync_server_not_webdav";
/// Error code: the server's storage is full (HTTP 507).
pub const QUOTA: &str = "sync_server_quota";
/// Error code: the server failed (HTTP 5xx, an unexpected status).
pub const SERVER_ERROR: &str = "sync_server_error";
/// Error code: the server's answer is malformed or too large.
pub const BAD_RESPONSE: &str = "sync_server_response";
/// Error code: the server's TLS certificate is not trusted (or no longer
/// the one the user confirmed).
pub const CERTIFICATE: &str = "sync_server_certificate";
/// Error code: a write's precondition failed (HTTP 412) – the file changed
/// since it was read (or exists although it should not). The same code as
/// the phone's `folder_sync::REMOTE_CHANGED`.
pub const CHANGED: &str = "sync_remote_changed";
/// Error code: the file (or the attachment folder) does not exist.
pub const FILE_MISSING: &str = "sync_file_missing";
/// Error code: a file is larger than the caller's limit (the same code as
/// `folder_sync::TOO_LARGE`).
pub const TOO_LARGE: &str = "sync_file_too_large";
/// `invalid_input:<detail>` for a path outside the layout.
pub const BAD_PATH: &str = "remote_path";

/// Whether `e` is `unsupported:<code>`.
pub fn has_code(e: &AppError, code: &str) -> bool {
    matches!(e, AppError::Code(c) if c.strip_prefix("unsupported:") == Some(code))
}

/// Errors that end a whole sync (the folder or server cannot be used right
/// now) instead of failing one file: `folder_access_lost`, `sync_offline`,
/// `sync_auth_failed`, `sync_server_certificate`.
pub fn ends_sync(e: &AppError) -> bool {
    ["folder_access_lost", OFFLINE, AUTH_FAILED, CERTIFICATE]
        .iter()
        .any(|code| has_code(e, code))
}

/// One entry of a listing (a file or a folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMeta {
    /// The entry's name (one segment, decoded).
    pub name: String,
    pub dir: bool,
    pub size: Option<u64>,
    /// A strong ETag as the server sent it (quotes included); `None` when
    /// the server sends none (or only a weak one, which `If-Match` cannot
    /// use).
    pub etag: Option<String>,
    /// Last modification (ms since the epoch), if the server says.
    pub modified: Option<i64>,
}

/// The condition of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// Unconditional (blobs: one never changes under its id).
    None,
    /// Only over the file with this ETag (`If-Match`).
    IfMatch(String),
    /// Only if no file of that name exists (`If-None-Match: *`).
    IfNoneMatch,
}

/// What a probe of the server's folder found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    /// The folder exists.
    pub exists: bool,
    /// The user may write it (as far as the server says).
    pub writable: bool,
    /// The server honours `If-Match` (a write with a wrong ETag failed).
    pub cas: bool,
}

/// A folder on a server in the cloud-folder layout. Blocking; every call is
/// one or a few requests. Paths are relative to the folder ([`check_path`]).
pub trait Remote: Send + Sync {
    /// What the sync bases and the link are recorded for (e.g.
    /// `webdav:<normalized folder URL>`) – never credentials.
    fn key(&self) -> String;
    /// Whether the folder exists and may be written (and whether the server
    /// honours preconditions).
    fn probe(&self) -> AppResult<Access>;
    /// The entries of `dir` (`""`: the folder's top level, or an attachment
    /// folder), depth 1. The folder missing → [`FOLDER_MISSING`], an
    /// attachment folder missing → [`FILE_MISSING`].
    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>>;
    /// The file `path` – at most `max` bytes ([`TOO_LARGE`] otherwise) – and
    /// its ETag. Missing → [`FILE_MISSING`].
    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)>;
    /// Writes the file `path` if `pre` holds ([`CHANGED`] otherwise).
    /// Returns its entry as the server reported it (the new ETag, if sent).
    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta>;
    /// Creates the attachment folder `dir` (one that exists is fine).
    fn mkcol(&self, dir: &str) -> AppResult<()>;
    /// Removes the file (or attachment folder) `path` if `pre` holds.
    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()>;
}

impl<R: Remote + ?Sized> Remote for Arc<R> {
    fn key(&self) -> String {
        (**self).key()
    }
    fn probe(&self) -> AppResult<Access> {
        (**self).probe()
    }
    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>> {
        (**self).list(dir)
    }
    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)> {
        (**self).get(path, max)
    }
    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta> {
        (**self).put(path, bytes, pre)
    }
    fn mkcol(&self, dir: &str) -> AppResult<()> {
        (**self).mkcol(dir)
    }
    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()> {
        (**self).delete(path, pre)
    }
}

/// `<id>.keystead.attachments` → `id`.
pub fn blob_dir_id(name: &str) -> Option<&str> {
    sync::vault_id_of_file_name(name.strip_suffix(attachments::DIR_SUFFIX)?)
}

/// `<id>.keystead.attachments` of vault `id`.
pub fn blob_dir_name(id: &str) -> String {
    format!(
        "{id}.{}{}",
        keystead_core::format::VAULT_EXTENSION,
        attachments::DIR_SUFFIX
    )
}

/// Whether `path` is one of the layout's paths: `<id>.keystead`, a phone
/// conflict copy, an attachment folder `<id>.keystead.attachments`, or a
/// blob `<id>.keystead.attachments/<attachment id>.bin`.
pub fn valid_path(path: &str) -> bool {
    match path.split_once('/') {
        None => {
            sync::vault_id_of_file_name(path).is_some()
                || sync::vault_id_of_phone_conflict_copy(path).is_some()
                || blob_dir_id(path).is_some()
        }
        Some((dir, name)) => {
            blob_dir_id(dir).is_some() && attachments::blob_id_of_file_name(name).is_some()
        }
    }
}

/// Whether `dir` may be listed: the top level (`""`) or an attachment
/// folder.
pub fn valid_dir(dir: &str) -> bool {
    dir.is_empty() || blob_dir_id(dir).is_some()
}

/// [`valid_path`] as an error (`invalid_input:remote_path`).
pub fn check_path(path: &str) -> AppResult<()> {
    if valid_path(path) {
        Ok(())
    } else {
        Err(AppError::invalid(BAD_PATH))
    }
}

/// [`valid_dir`] as an error (`invalid_input:remote_path`).
pub fn check_dir(dir: &str) -> AppResult<()> {
    if valid_dir(dir) {
        Ok(())
    } else {
        Err(AppError::invalid(BAD_PATH))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0f5b2c1e-aaaa-4bbb-8ccc-1234567890ab";

    #[test]
    fn only_the_layouts_paths_are_valid() {
        let blob_dir = blob_dir_name(ID);
        assert_eq!(blob_dir_id(&blob_dir), Some(ID));
        for ok in [
            format!("{ID}.keystead"),
            format!("{ID}.keystead.conflict-20261023-140509-phone"),
            blob_dir.clone(),
            format!("{blob_dir}/{ID}.bin"),
        ] {
            assert!(valid_path(&ok), "{ok}");
        }
        for bad in [
            String::new(),
            "/".into(),
            "..".into(),
            format!("../{ID}.keystead"),
            format!("/{ID}.keystead"),
            format!("{ID}.keystead/"),
            format!("{ID}.keystead.bak"),
            format!("{ID}.keystead.conflict-20261023-140509"),
            format!("{blob_dir}/x/{ID}.bin"),
            format!("{blob_dir}/../{ID}.keystead"),
            format!("{blob_dir}/{ID}.bin.tmp"),
            format!("{ID}.keystead/{ID}.bin"),
            format!("a\\{ID}.keystead"),
            format!("{ID}.keystead\0"),
            ".keystead-cas-probe".into(),
            "settings.json".into(),
        ] {
            assert!(!valid_path(&bad), "{bad:?}");
        }
        assert!(valid_dir(""));
        assert!(valid_dir(&blob_dir));
        assert!(!valid_dir(&format!("{ID}.keystead")));
        assert!(!valid_dir("x"));
        assert_eq!(
            check_path("x").unwrap_err().code(),
            "invalid_input:remote_path"
        );
    }

    #[test]
    fn the_codes_that_end_a_sync() {
        for code in ["folder_access_lost", OFFLINE, AUTH_FAILED, CERTIFICATE] {
            assert!(ends_sync(&AppError::unsupported(code)), "{code}");
        }
        for code in [CHANGED, FILE_MISSING, TOO_LARGE, SERVER_ERROR, QUOTA] {
            assert!(!ends_sync(&AppError::unsupported(code)), "{code}");
        }
        assert!(!ends_sync(&AppError::invalid(OFFLINE)));
        assert!(has_code(&AppError::unsupported(CHANGED), CHANGED));
    }
}
