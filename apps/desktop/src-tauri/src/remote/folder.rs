//! The phone's sync engine (`folder_sync`) on a server: [`RemoteDirFolder`]
//! is a `folder_sync::RemoteFolder` over any [`Remote`].
//!
//! * An entry's `uri` is its path relative to the server's folder
//!   (`<name>` or `<attachment folder>/<name>`), its `etag` the server's.
//! * `copy_to` reads at most the caller's limit and writes it into the new
//!   file in `<data>/sync/incoming`.
//! * `write_vault_checked` is a conditional PUT – the precondition the
//!   engine verified (`If-Match` with the ETag of the copy it read again,
//!   `If-None-Match: *` for a copy that must not exist yet); a failed one
//!   is `unsupported:sync_remote_changed` and the engine looks again.
//! * `write_conflict_copy` creates the file only (`If-None-Match: *`): one
//!   of that name already there → `unsupported:sync_name_taken`.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use keystead_core::sync;

use super::{check_dir, has_code, Precondition, Remote, CHANGED, FOLDER_MISSING};
use crate::error::{AppError, AppResult};
use crate::folder_sync::{RemoteEntry, RemoteFolder, NAME_TAKEN};

/// A server's folder as the phone's sync sees it.
// Wired to the phone's link in a later step of stage 2c (WP4).
#[cfg_attr(not(test), expect(dead_code))]
pub struct RemoteDirFolder<R: Remote> {
    remote: R,
}

#[cfg_attr(not(test), expect(dead_code))]
impl<R: Remote> RemoteDirFolder<R> {
    pub fn new(remote: R) -> Self {
        RemoteDirFolder { remote }
    }

    pub fn remote(&self) -> &R {
        &self.remote
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

impl<R: Remote> RemoteFolder for RemoteDirFolder<R> {
    fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>> {
        let dir = parent.map_or("", |p| p.uri.as_str());
        check_dir(dir)?;
        Ok(self
            .remote
            .list(dir)?
            .into_iter()
            .map(|meta| RemoteEntry {
                uri: join(dir, &meta.name),
                name: meta.name,
                dir: meta.dir,
                size: meta.size,
                etag: meta.etag,
            })
            .collect())
    }

    fn copy_to(&self, entry: &RemoteEntry, target: &Path, max_bytes: u64) -> AppResult<u64> {
        let (bytes, _) = self.remote.get(&entry.uri, max_bytes)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        file.write_all(&bytes)?;
        Ok(bytes.len() as u64)
    }

    fn writable(&self) -> AppResult<bool> {
        let access = self.remote.probe()?;
        if !access.exists {
            return Err(AppError::unsupported(FOLDER_MISSING));
        }
        Ok(access.writable)
    }

    fn write_vault(&self, name: &str, bytes: &[u8]) -> AppResult<RemoteEntry> {
        self.write_vault_checked(name, bytes, &Precondition::None)
    }

    fn write_vault_checked(
        &self,
        name: &str,
        bytes: &[u8],
        pre: &Precondition,
    ) -> AppResult<RemoteEntry> {
        if sync::vault_id_of_file_name(name).is_none() {
            return Err(AppError::invalid("folder_write"));
        }
        let meta = self.remote.put(name, bytes, pre.clone())?;
        Ok(RemoteEntry {
            name: name.to_owned(),
            uri: name.to_owned(),
            dir: false,
            size: meta.size,
            etag: meta.etag,
        })
    }

    fn write_conflict_copy(&self, name: &str, bytes: &[u8]) -> AppResult<()> {
        if sync::vault_id_of_phone_conflict_copy(name).is_none() {
            return Err(AppError::invalid("folder_write"));
        }
        match self.remote.put(name, bytes, Precondition::IfNoneMatch) {
            Ok(_) => Ok(()),
            Err(e) if has_code(&e, CHANGED) => Err(AppError::unsupported(NAME_TAKEN)),
            Err(e) => Err(e),
        }
    }
}
