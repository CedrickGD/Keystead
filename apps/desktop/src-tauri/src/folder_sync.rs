//! Cloud-folder sync of the read-only phone (Block 5, stage 2a): copy-in
//! only. See "Cloud-folder sync (phone)" under "Mobile (Android)" in
//! docs/ARCHITECTURE.md.
//!
//! The user links one folder of their own cloud (the Nextcloud or OneDrive
//! app on the phone offers it through the system picker); the PC keeps a
//! copy of each vault there – `<id>.keystead` plus its attachment folder
//! `<id>.keystead.attachments/<attachment id>.bin`. A sync reads that
//! folder and updates the phone's working copies in `<data>/vaults`; it
//! never writes to the folder (the phone is read-only, decision E4).
//!
//! * Every vault file is untrusted input: at most [`VAULT_FILE_MAX_BYTES`],
//!   named `<id>.keystead` after the id in its header, parsed and validated
//!   (`VaultFile::parse`) before anything is written; then
//!   `keystead_core::sync::decide_copy_in` against the phone's copy: a new
//!   vault is added, a higher revision replaces the phone's copy
//!   atomically, an equal one changes nothing, a lower one is kept out
//!   (reported as "the PC's copy is older"; never written back).
//! * Attachments: blobs the phone lacks (or whose size differs – a blob
//!   never changes under its id) are stored **before** the phone's vault
//!   file is replaced, so a vault that references a new attachment never
//!   arrives without it. The folder's vault file is read first and its
//!   blob folder listed afterwards: the PC writes the blobs before the
//!   vault file, so every blob the copy that was read references is
//!   already there. Only
//!   names of the form `<id>.bin` with a blob's shape are taken
//!   (`attachments::looks_like_blob`; the key is in the vault, so nothing
//!   is decrypted here). Blobs the vault no longer references are left to
//!   the core's sweep (on unlock, and here after a reload of the open
//!   vault), which removes them once they are an hour old.
//! * The open vault: the file is replaced under the state lock and reloaded
//!   (`reload_if_changed`) in the same step, so the monitor's own change
//!   check never sees it first. `vault://changed` follows; a new master
//!   password (`Error::KeyChanged`) locks with reason `keyChanged`.
//! * State: `<data>/sync/link.json` ([`LinkState`]: the folder's address
//!   and name, when it was linked and last synced, the last error and what
//!   the last sync found per file – no secrets); copies arrive in
//!   `<data>/sync/incoming/` first and are removed right after reading.
//!
//! The folder itself is behind [`RemoteFolder`]: on Android the Kotlin
//! plugin `KeysteadFolderPlugin` (`mobile::sync`), in the unit tests a
//! directory. Everything here is platform-neutral and tested on the host.

#![cfg_attr(not(mobile), allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use keystead_core::attachments;
use keystead_core::format::{self, VaultFile};
use keystead_core::sync::{self, CopyIn, VaultHeader};
use keystead_core::Error as CoreError;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::{log, Core, LockReason};

/// Largest vault file copied in (bytes): 64 MiB.
pub const VAULT_FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest blob: the largest attachment plus the blob's own bytes.
pub const BLOB_MAX_BYTES: u64 =
    attachments::ATTACHMENT_MAX_BYTES + attachments::BLOB_OVERHEAD as u64;

/// Error code: the folder cannot be read any more (the permission was
/// revoked, the cloud app removed, the folder deleted or moved). The user
/// has to pick it again.
pub const ACCESS_LOST: &str = "folder_access_lost";
/// Error code: a file is larger than its limit.
pub const TOO_LARGE: &str = "sync_file_too_large";

/// `<data>/sync`.
pub fn sync_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sync")
}

fn link_path(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("link.json")
}

fn incoming_dir(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("incoming")
}

/// `unsupported:folder_access_lost`.
pub fn access_lost() -> AppError {
    AppError::unsupported(ACCESS_LOST)
}

fn is_access_lost(e: &AppError) -> bool {
    e.code() == format!("unsupported:{ACCESS_LOST}")
}

fn is_too_large(e: &AppError) -> bool {
    e.code() == format!("unsupported:{TOO_LARGE}")
}

// ---------------------------------------------------------------------------
// The folder
// ---------------------------------------------------------------------------

/// One entry of the linked folder (or of a folder inside it).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    /// The entry's address for the folder backend (Android: a `content://`
    /// document URI inside the granted tree).
    pub uri: String,
    #[serde(default)]
    pub dir: bool,
    /// Size in bytes, if the provider knows it.
    #[serde(default)]
    pub size: Option<u64>,
}

/// The linked folder as the sync sees it.
pub trait RemoteFolder {
    /// The entries at the folder's top level (`parent: None`) or inside
    /// the folder `parent`. A folder that cannot be read any more →
    /// [`access_lost`].
    fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>>;

    /// Copies the file `entry` into the new file `target` (it must not
    /// exist; always in `<data>/sync/incoming`), at most `max_bytes` –
    /// more → `unsupported:sync_file_too_large` and no file. Returns the
    /// number of bytes copied.
    fn copy_to(&self, entry: &RemoteEntry, target: &Path, max_bytes: u64) -> AppResult<u64>;
}

// ---------------------------------------------------------------------------
// Link state (`<data>/sync/link.json`)
// ---------------------------------------------------------------------------

/// What happened to one vault file of the folder in the last sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// A vault the phone did not have: added.
    Imported,
    /// A newer revision: the phone's copy was replaced.
    Updated,
    /// The phone already has this revision.
    UpToDate,
    /// The PC's copy is older than the phone's: the phone keeps its copy.
    PcOlder,
    /// The phone's file at that place is another vault: left alone.
    OtherVault,
    /// Not named `<id>.keystead` after its own id (a renamed copy, a cloud
    /// app's conflict copy): skipped.
    Misnamed,
    /// Not a vault file this app can read (damaged, half uploaded, another
    /// program's file or a newer format): skipped.
    Unreadable,
    /// Larger than [`VAULT_FILE_MAX_BYTES`]: skipped.
    TooLarge,
    /// Could not be read from the folder or written on the phone.
    Failed,
    /// Synced before, no longer in the folder: the phone keeps its copy.
    Missing,
}

/// One vault file of the folder and what the last sync did with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub file_name: String,
    /// The vault id and name from the file's header (when it was readable).
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub vault_name: Option<String>,
    /// The folder copy's revision.
    #[serde(default)]
    pub revision: Option<u64>,
    pub outcome: Outcome,
    /// The error code behind `unreadable` / `failed`.
    #[serde(default)]
    pub error: Option<String>,
    /// Attachments copied in this sync, and those that could not be.
    #[serde(default)]
    pub attachments_copied: u32,
    #[serde(default)]
    pub attachments_failed: u32,
    /// The master password was changed on the PC: the copy that arrived
    /// has another key-derivation salt than the phone's before, so the old
    /// password no longer opens it (kept until a newer revision arrives).
    #[serde(default)]
    pub password_changed: bool,
}

impl FileResult {
    fn new(file_name: &str, outcome: Outcome) -> Self {
        FileResult {
            file_name: file_name.to_owned(),
            vault_id: None,
            vault_name: None,
            revision: None,
            outcome,
            error: None,
            attachments_copied: 0,
            attachments_failed: 0,
            password_changed: false,
        }
    }

    fn with_header(mut self, header: &VaultHeader) -> Self {
        self.vault_id = Some(header.id.clone());
        self.vault_name = Some(header.name.clone());
        self.revision = Some(header.revision);
        self
    }
}

/// The linked folder (`<data>/sync/link.json`). No secrets: the folder's
/// address and name, times, and the vault ids, names and revisions the
/// last sync found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// The folder's address (Android: the tree URI the app holds a
    /// persisted read permission for).
    pub tree_uri: String,
    /// The folder's display name.
    pub folder_name: String,
    /// When it was linked (ms since the epoch).
    pub linked_at: i64,
    /// The last sync that could read the folder (ms since the epoch).
    #[serde(default)]
    pub last_sync_at: Option<i64>,
    /// The error code of the last sync attempt, `None` after a sync that
    /// read the folder.
    #[serde(default)]
    pub last_error: Option<String>,
    /// What the last sync that read the folder found, by file name.
    #[serde(default)]
    pub files: Vec<FileResult>,
}

impl LinkState {
    pub fn new(tree_uri: String, folder_name: String, linked_at: i64) -> Self {
        LinkState {
            tree_uri,
            folder_name,
            linked_at,
            last_sync_at: None,
            last_error: None,
            files: Vec::new(),
        }
    }

    /// The link of the data directory, `None` without one. A file that
    /// cannot be read counts as no link (logged): the user links again.
    pub fn load(data_dir: &Path) -> Option<LinkState> {
        let path = link_path(data_dir);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                log(format_args!("could not read the folder link: {e}"));
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(link) => Some(link),
            Err(e) => {
                log(format_args!("the folder link is unreadable: {e}"));
                None
            }
        }
    }

    /// Writes `link.json` atomically.
    pub fn save(&self, data_dir: &Path) -> AppResult<()> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(CoreError::from)?;
        bytes.push(b'\n');
        format::write_atomic(&link_path(data_dir), &bytes, false)?;
        Ok(())
    }

    /// Removes `link.json` (and leftovers of an interrupted copy); the
    /// vault copies stay.
    pub fn remove(data_dir: &Path) -> AppResult<()> {
        match fs::remove_file(link_path(data_dir)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let _ = fs::remove_dir_all(incoming_dir(data_dir));
        Ok(())
    }
}

/// The sync state for the UI (`folder_status`, `sync://status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    pub linked: bool,
    /// A sync is running right now.
    pub syncing: bool,
    pub folder_name: Option<String>,
    pub linked_at: Option<i64>,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
    pub files: Vec<FileResult>,
}

impl FolderStatus {
    pub fn of(link: Option<&LinkState>) -> Self {
        match link {
            None => FolderStatus {
                linked: false,
                syncing: false,
                folder_name: None,
                linked_at: None,
                last_sync_at: None,
                last_error: None,
                files: Vec::new(),
            },
            Some(link) => FolderStatus {
                linked: true,
                syncing: false,
                folder_name: Some(link.folder_name.clone()),
                linked_at: Some(link.linked_at),
                last_sync_at: link.last_sync_at,
                last_error: link.last_error.clone(),
                files: link.files.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The sync
// ---------------------------------------------------------------------------

/// Reads the linked folder and updates the phone's vault copies (see the
/// module docs). Runs one at a time (the caller holds the sync guard).
/// `link` gets the result: on success `last_sync_at = now`, no error and
/// the per-file results; on failure only `last_error` (the previous results
/// stay). Returns whether a vault was added or replaced.
pub fn sync_folder(
    core: &Core,
    folder: &dyn RemoteFolder,
    link: &mut LinkState,
    now: i64,
) -> AppResult<bool> {
    match run_sync(core, folder, &link.files) {
        Ok(files) => {
            let changed = files
                .iter()
                .any(|f| matches!(f.outcome, Outcome::Imported | Outcome::Updated));
            link.files = files;
            link.last_sync_at = Some(now);
            link.last_error = None;
            Ok(changed)
        }
        Err(e) => {
            link.last_error = Some(e.code());
            Err(e)
        }
    }
}

fn run_sync(
    core: &Core,
    folder: &dyn RemoteFolder,
    previous: &[FileResult],
) -> AppResult<Vec<FileResult>> {
    let data_dir = core.data_dir();
    let incoming = incoming_dir(&data_dir);
    // Leftovers of an interrupted sync (the process was killed mid-copy).
    let _ = fs::remove_dir_all(&incoming);
    fs::create_dir_all(&incoming)?;
    let result = sync_entries(core, folder, previous, &incoming);
    let _ = fs::remove_dir_all(&incoming);
    result
}

fn sync_entries(
    core: &Core,
    folder: &dyn RemoteFolder,
    previous: &[FileResult],
    incoming: &Path,
) -> AppResult<Vec<FileResult>> {
    let top = folder.list(None)?;
    let mut vault_files: Vec<&RemoteEntry> = top
        .iter()
        .filter(|e| !e.dir && !e.name.starts_with('.') && e.name.ends_with(".keystead"))
        .collect();
    vault_files.sort_by(|a, b| a.name.cmp(&b.name));

    let mut results = Vec::with_capacity(vault_files.len());
    for entry in vault_files {
        let before = previous.iter().find(|r| r.file_name == entry.name);
        results.push(sync_vault_file(
            core, folder, &top, entry, before, incoming,
        )?);
    }
    // Vaults synced before whose file is gone: the phone keeps its copy.
    let gone: Vec<&FileResult> = previous
        .iter()
        .filter(|r| r.vault_id.is_some() && !results.iter().any(|f| f.file_name == r.file_name))
        .collect();
    for gone in gone {
        let mut missing = gone.clone();
        missing.outcome = Outcome::Missing;
        missing.error = None;
        missing.attachments_copied = 0;
        missing.attachments_failed = 0;
        missing.password_changed = false;
        results.push(missing);
    }
    Ok(results)
}

/// One vault file of the folder. Only a folder that cannot be read any
/// more is an error (it ends the sync); everything else is the file's
/// result.
fn sync_vault_file(
    core: &Core,
    folder: &dyn RemoteFolder,
    top: &[RemoteEntry],
    entry: &RemoteEntry,
    before: Option<&FileResult>,
    incoming: &Path,
) -> AppResult<FileResult> {
    let name = entry.name.as_str();
    let Some(id) = sync::vault_id_of_file_name(name) else {
        return Ok(FileResult::new(name, Outcome::Misnamed));
    };
    if entry.size.is_some_and(|size| size > VAULT_FILE_MAX_BYTES) {
        return Ok(FileResult::new(name, Outcome::TooLarge));
    }
    let bytes = match fetch(folder, entry, incoming, VAULT_FILE_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(e) if is_access_lost(&e) => return Err(e),
        Err(e) if is_too_large(&e) => return Ok(FileResult::new(name, Outcome::TooLarge)),
        Err(e) => return Ok(failed(name, Outcome::Failed, &e)),
    };
    let remote_file = match VaultFile::parse(&bytes) {
        Ok(file) => file,
        Err(e) => return Ok(failed(name, Outcome::Unreadable, &AppError::from(e))),
    };
    let remote = VaultHeader::from(&remote_file);
    if remote.id != id {
        return Ok(FileResult::new(name, Outcome::Misnamed).with_header(&remote));
    }

    let local_path = core.store().vault_path(id);
    let (local_file, damaged) = match VaultFile::read(&local_path) {
        Ok(file) => (Some(file), false),
        Err(CoreError::NotFound(_)) => (None, false),
        // The phone's copy cannot be read (it only ever is a copy): the
        // folder's replaces it.
        Err(_) => (None, true),
    };
    let local = local_file.as_ref().map(VaultHeader::from);
    let decision = sync::decide_copy_in(local.as_ref(), &remote);
    let mut result = FileResult::new(
        name,
        match decision {
            CopyIn::Import if damaged => Outcome::Updated,
            CopyIn::Import => Outcome::Imported,
            CopyIn::Update => Outcome::Updated,
            CopyIn::UpToDate => Outcome::UpToDate,
            CopyIn::RemoteOlder => Outcome::PcOlder,
            CopyIn::OtherVault => Outcome::OtherVault,
        },
    )
    .with_header(&remote);
    if decision == CopyIn::OtherVault {
        return Ok(result);
    }

    // Attachments first: the vault file must not arrive before its blobs.
    let (copied, attachment_failures) = copy_attachments(folder, top, id, &local_path, incoming)?;
    result.attachments_copied = copied;
    result.attachments_failed = attachment_failures;

    match decision {
        CopyIn::Import | CopyIn::Update => {
            result.password_changed = local_file
                .as_ref()
                .is_some_and(|local| local.kdf.salt != remote_file.kdf.salt);
            if let Err(e) = install_copy(core, &local_path, id, &bytes) {
                log(format_args!("could not store the vault copy: {}", e.code()));
                result.outcome = Outcome::Failed;
                result.error = Some(e.code());
                result.password_changed = false;
            } else {
                // A new master password (another salt): the fingerprint
                // unlock's sealed KEK opens nothing any more.
                crate::device_unlock::forget_if_stale(&core.data_dir(), id, &remote_file.kdf.salt);
            }
        }
        // Still the revision whose new password the last sync reported.
        CopyIn::UpToDate => {
            result.password_changed = before.is_some_and(|b| {
                b.password_changed
                    && b.vault_id.as_deref() == Some(id)
                    && b.revision == result.revision
            });
        }
        CopyIn::RemoteOlder | CopyIn::OtherVault => {}
    }
    Ok(result)
}

fn failed(name: &str, outcome: Outcome, e: &AppError) -> FileResult {
    let mut result = FileResult::new(name, outcome);
    result.error = Some(e.code());
    result
}

/// Copies `entry` into a new file in `incoming`, reads it and removes it.
fn fetch(
    folder: &dyn RemoteFolder,
    entry: &RemoteEntry,
    incoming: &Path,
    max_bytes: u64,
) -> AppResult<Vec<u8>> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let target = incoming.join(format!("{}.part", NEXT.fetch_add(1, Ordering::Relaxed)));
    let copied = folder.copy_to(entry, &target, max_bytes);
    let read = copied.and_then(|_| fs::read(&target).map_err(AppError::from));
    let _ = fs::remove_file(&target);
    let bytes = read?;
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::unsupported(TOO_LARGE));
    }
    Ok(bytes)
}

/// Copies the blobs of `<id>.keystead.attachments` in the folder that the
/// phone lacks. Returns (copied, failed); only a folder that cannot be read
/// any more is an error.
fn copy_attachments(
    folder: &dyn RemoteFolder,
    top: &[RemoteEntry],
    id: &str,
    vault_path: &Path,
    incoming: &Path,
) -> AppResult<(u32, u32)> {
    let dir_name = format!("{id}.keystead{}", attachments::DIR_SUFFIX);
    let Some(remote_dir) = top.iter().find(|e| e.dir && e.name == dir_name) else {
        return Ok((0, 0));
    };
    let entries = match folder.list(Some(remote_dir)) {
        Ok(entries) => entries,
        Err(e) if is_access_lost(&e) => return Err(e),
        Err(e) => {
            log(format_args!("could not list the attachments: {}", e.code()));
            return Ok((0, 1));
        }
    };
    let local_dir = attachments::dir_for(vault_path);
    let (mut copied, mut failures) = (0, 0);
    for entry in entries.iter().filter(|e| !e.dir) {
        let Some(attachment_id) = attachments::blob_id_of_file_name(&entry.name) else {
            continue;
        };
        let local = attachments::blob_path(&local_dir, attachment_id);
        let local_size = fs::metadata(&local)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len());
        // A blob never changes under its id: only a missing one, or one of
        // another size (an interrupted copy), is fetched.
        if local_size.is_some() && (entry.size.is_none() || entry.size == local_size) {
            continue;
        }
        if entry.size.is_some_and(|size| size > BLOB_MAX_BYTES) {
            failures += 1;
            continue;
        }
        match fetch(folder, entry, incoming, BLOB_MAX_BYTES) {
            Ok(bytes) if attachments::looks_like_blob(&bytes) => {
                match format::write_atomic(&local, &bytes, false) {
                    Ok(()) => copied += 1,
                    Err(e) => {
                        log(format_args!("could not store an attachment: {}", e.code()));
                        failures += 1;
                    }
                }
            }
            Ok(_) => failures += 1,
            Err(e) if is_access_lost(&e) => return Err(e),
            Err(_) => failures += 1,
        }
    }
    Ok((copied, failures))
}

/// Replaces the phone's copy of vault `id` with `bytes` (already
/// validated). If that vault is open, it is reloaded under the same state
/// lock: the UI gets `vault://changed`; a new master password closes it
/// with reason `keyChanged`.
fn install_copy(core: &Core, path: &Path, id: &str, bytes: &[u8]) -> AppResult<()> {
    let (changed, closed) = {
        let mut st = core.state();
        format::write_atomic(path, bytes, false)?;
        let reload = match st.vault.as_mut() {
            Some(vault) if vault.id() == id => {
                Some(vault.reload_if_changed().inspect(|&changed| {
                    if changed {
                        // Blobs the new revision no longer references (an hour old).
                        vault.sweep_attachments();
                    }
                }))
            }
            _ => None,
        };
        match reload {
            None | Some(Ok(false)) => (false, None),
            Some(Ok(true)) => (true, None),
            Some(Err(CoreError::KeyChanged)) => {
                // Like `Core::lock`, under the state lock: no unlock in
                // flight opens the old session again.
                core.begin_lock();
                (false, st.vault.take())
            }
            Some(Err(e)) => {
                log(format_args!(
                    "could not reload the synced vault: {}",
                    e.code()
                ));
                (false, None)
            }
        }
    };
    if closed.is_some() {
        core.finish_lock(closed, Some(LockReason::KeyChanged));
    }
    if changed {
        core.emit_changed();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use keystead_core::model::{ItemType, VaultItem};
    use keystead_core::settings::Settings;
    use keystead_core::{KdfParams, UnlockedVault, VaultStore};

    use super::*;
    use crate::state::{EVENT_CHANGED, EVENT_LOCKED};

    /// A folder on disk standing in for the cloud folder; records the
    /// order of the copies and whether the phone's file `probe` existed at
    /// each.
    struct DirFolder {
        root: PathBuf,
        copies: RefCell<Vec<String>>,
        probe: Option<PathBuf>,
        probed: RefCell<Vec<bool>>,
        lost: bool,
    }

    impl DirFolder {
        fn new(root: &Path) -> Self {
            fs::create_dir_all(root).unwrap();
            DirFolder {
                root: root.to_owned(),
                copies: RefCell::new(Vec::new()),
                probe: None,
                probed: RefCell::new(Vec::new()),
                lost: false,
            }
        }
    }

    impl RemoteFolder for DirFolder {
        fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>> {
            if self.lost {
                return Err(access_lost());
            }
            let dir = parent.map_or(self.root.clone(), |p| PathBuf::from(&p.uri));
            let mut out = Vec::new();
            for entry in fs::read_dir(dir).map_err(|_| access_lost())? {
                let entry = entry.unwrap();
                let meta = entry.metadata().unwrap();
                out.push(RemoteEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    uri: entry.path().display().to_string(),
                    dir: meta.is_dir(),
                    size: meta.is_file().then_some(meta.len()),
                });
            }
            Ok(out)
        }

        fn copy_to(&self, entry: &RemoteEntry, target: &Path, max_bytes: u64) -> AppResult<u64> {
            if self.lost {
                return Err(access_lost());
            }
            assert!(!target.exists(), "copy_to into an existing file");
            assert!(target.parent().unwrap().ends_with("sync/incoming"));
            self.copies.borrow_mut().push(entry.name.clone());
            if let Some(probe) = &self.probe {
                self.probed.borrow_mut().push(probe.exists());
            }
            let bytes = fs::read(&entry.uri)?;
            if bytes.len() as u64 > max_bytes {
                return Err(AppError::unsupported(TOO_LARGE));
            }
            fs::write(target, &bytes)?;
            Ok(bytes.len() as u64)
        }
    }

    /// The PC's side: a store of its own, mirrored into the folder like the
    /// desktop's "keep a copy in the cloud folder" does.
    struct Pc {
        store: VaultStore,
        folder: PathBuf,
    }

    impl Pc {
        fn new(dir: &Path) -> Self {
            Pc {
                store: VaultStore::new(dir.join("pc")),
                folder: dir.join("cloud"),
            }
        }

        fn create(&self, name: &str) -> UnlockedVault {
            self.store
                .create_vault_with_params(name, "pc-pw", KdfParams::insecure_for_tests())
                .unwrap()
        }

        /// Copies the vault file and its blobs into the folder.
        fn mirror(&self, v: &UnlockedVault) {
            fs::create_dir_all(&self.folder).unwrap();
            let file_name = v.path().file_name().unwrap().to_owned();
            let blobs = attachments::dir_for(v.path());
            if blobs.is_dir() {
                let target = self.folder.join(blobs.file_name().unwrap());
                fs::create_dir_all(&target).unwrap();
                for blob in fs::read_dir(&blobs).unwrap() {
                    let blob = blob.unwrap();
                    fs::copy(blob.path(), target.join(blob.file_name())).unwrap();
                }
            }
            fs::copy(v.path(), self.folder.join(file_name)).unwrap();
        }
    }

    fn phone(dir: &Path) -> Arc<Core> {
        let settings = Settings {
            browser_integration: false,
            ..Settings::default()
        };
        Core::for_tests(VaultStore::new(dir.join("phone")), settings)
    }

    fn link() -> LinkState {
        LinkState::new("content://tree/cloud".into(), "Keystead".into(), 1)
    }

    fn note(v: &mut UnlockedVault, name: &str) {
        v.save_item(VaultItem::new(ItemType::Note, name)).unwrap();
    }

    fn outcome_of<'a>(link: &'a LinkState, id: &str) -> &'a FileResult {
        link.files
            .iter()
            .find(|f| f.vault_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no result for {id}: {:?}", link.files))
    }

    #[test]
    fn link_json_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(LinkState::load(dir.path()), None, "no link yet");

        let mut link = link();
        link.last_sync_at = Some(42);
        link.last_error = Some("unsupported:folder_access_lost".into());
        let mut file = FileResult::new("a.keystead", Outcome::Updated);
        file.vault_id = Some("a".into());
        file.vault_name = Some("Privat".into());
        file.revision = Some(7);
        file.attachments_copied = 2;
        file.password_changed = true;
        link.files.push(file);
        link.save(dir.path()).unwrap();
        assert_eq!(LinkState::load(dir.path()), Some(link.clone()));
        assert!(dir.path().join("sync").join("link.json").is_file());

        // camelCase, no secrets: only these keys.
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("sync/link.json")).unwrap()).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "files",
                "folderName",
                "lastError",
                "lastSyncAt",
                "linkedAt",
                "treeUri"
            ]
        );
        assert_eq!(json["files"][0]["outcome"], "updated");
        assert_eq!(json["files"][0]["passwordChanged"], true);

        // Lenient: optional fields may be missing, unknown ones are ignored.
        fs::write(
            dir.path().join("sync/link.json"),
            r#"{"treeUri":"u","folderName":"F","linkedAt":5,"future":1}"#,
        )
        .unwrap();
        let loaded = LinkState::load(dir.path()).unwrap();
        assert_eq!(loaded, LinkState::new("u".into(), "F".into(), 5));
        // Damaged: no link (the user links again).
        fs::write(dir.path().join("sync/link.json"), b"{ not json").unwrap();
        assert_eq!(LinkState::load(dir.path()), None);

        LinkState::remove(dir.path()).unwrap();
        assert_eq!(LinkState::load(dir.path()), None);
        LinkState::remove(dir.path()).unwrap();
    }

    #[test]
    fn a_new_vault_arrives_with_its_attachments_first() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        note(&mut v, "n");
        let item = v
            .save_item(VaultItem::new(ItemType::Login, "Bank"))
            .unwrap();
        v.add_attachment(&item.id, "scan.png", b"\x89PNG not really")
            .unwrap();
        v.add_attachment(&item.id, "vertrag.pdf", b"%PDF-1.7")
            .unwrap();
        pc.mirror(&v);
        let mut folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        folder.probe = Some(core.store().vault_path(v.id()));

        let mut link = link();
        assert!(sync_folder(&core, &folder, &mut link, 100).unwrap());
        assert_eq!(link.last_sync_at, Some(100));
        assert_eq!(link.last_error, None);
        let result = outcome_of(&link, v.id());
        assert_eq!(result.outcome, Outcome::Imported);
        assert_eq!(result.vault_name.as_deref(), Some("Privat"));
        assert_eq!(result.revision, Some(v.revision()));
        assert_eq!(result.attachments_copied, 2);
        assert_eq!(result.attachments_failed, 0);
        assert!(!result.password_changed);

        // The folder's vault file is read first, then its blobs are listed
        // (the PC writes the blobs first, so every blob that copy references
        // is there) – and the phone's vault file is written only after the
        // blobs.
        let copies = folder.copies.borrow().clone();
        assert_eq!(copies.len(), 3);
        assert!(copies[0].ends_with(".keystead"), "{copies:?}");
        assert!(
            copies[1..].iter().all(|c| c.ends_with(".bin")),
            "{copies:?}"
        );
        assert_eq!(*folder.probed.borrow(), [false, false, false]);
        assert!(core.store().vault_path(v.id()).is_file());
        // The phone lists and opens it, attachments included; nothing is
        // left in `incoming`.
        let vaults = core.store().list_vaults().unwrap();
        assert_eq!(vaults.len(), 1);
        assert_eq!(vaults[0].id, v.id());
        let opened = core.store().unlock(v.id(), "pc-pw").unwrap();
        let ids: Vec<String> = opened
            .attachments_of(&item.id)
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids.len(), 2);
        for id in &ids {
            assert!(!opened.read_attachment(id).unwrap().is_empty());
        }
        assert!(!dir.path().join("phone/sync/incoming").exists());

        // Nothing new: up to date, nothing copied again.
        folder.copies.borrow_mut().clear();
        assert!(!sync_folder(&core, &folder, &mut link, 200).unwrap());
        let result = outcome_of(&link, v.id());
        assert_eq!(result.outcome, Outcome::UpToDate);
        assert_eq!(result.attachments_copied, 0);
        assert_eq!(
            *folder.copies.borrow(),
            [v.path().file_name().unwrap().to_string_lossy().into_owned()]
        );
        assert!(core.emitted().is_empty(), "no vault was open");
    }

    #[test]
    fn a_newer_revision_replaces_the_phone_copy_and_an_older_one_never_does() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();

        // The PC saves twice: the phone gets revision 3.
        note(&mut v, "one");
        note(&mut v, "two");
        let newest = fs::read(v.path()).unwrap();
        pc.mirror(&v);
        assert!(sync_folder(&core, &folder, &mut link, 2).unwrap());
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Updated);
        let local = core.store().vault_path(v.id());
        assert_eq!(fs::read(&local).unwrap(), newest);
        let bak = PathBuf::from(format!("{}.bak", local.display()));
        assert!(!bak.exists(), "no backup of an old copy");

        // The folder goes back to revision 1 (a restored backup on the PC):
        // the phone keeps revision 3 and says so.
        let old = pc.store.vault_path(v.id());
        let mut first = VaultFile::read(&old).unwrap();
        first.revision = 1;
        fs::write(
            pc.folder.join(old.file_name().unwrap()),
            first.to_json().unwrap(),
        )
        .unwrap();
        assert!(!sync_folder(&core, &folder, &mut link, 3).unwrap());
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::PcOlder);
        assert_eq!(fs::read(&local).unwrap(), newest);
        // Nothing was written to the folder, ever.
        let names: Vec<_> = fs::read_dir(&pc.folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [old.file_name().unwrap().to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn the_open_vault_is_reloaded_or_locked_when_its_password_changed() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        core.unlock(v.id(), "pc-pw").unwrap();
        // Fingerprint unlock set up on the phone (its sealed KEK belongs
        // to this salt).
        let fingerprint = crate::device_unlock::Blob {
            version: 1,
            vault_id: v.id().to_owned(),
            kdf_salt: core.read(|o| o.kdf_salt().to_owned()).unwrap(),
            sealed: "sealed".into(),
            created_at: 1,
        };
        crate::device_unlock::store_blob(&core.data_dir(), &fingerprint).unwrap();
        let vault_id = v.id().to_owned();
        let fingerprint_set_up = || {
            crate::device_unlock::load_blob(&core.data_dir(), &vault_id)
                .unwrap()
                .is_some()
        };

        // A new item on the PC: the open vault shows it at once.
        note(&mut v, "neu am PC");
        pc.mirror(&v);
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(core.read(|o| o.items().len()).unwrap(), 1);
        assert!(fingerprint_set_up(), "same master password");
        assert_eq!(
            core.emitted(),
            [(EVENT_CHANGED.to_owned(), serde_json::json!({}))]
        );

        // The master password changed on the PC: the session ends with
        // reason `keyChanged`, the result says so.
        v.change_master_password("pc-pw", "neu").unwrap();
        pc.mirror(&v);
        sync_folder(&core, &folder, &mut link, 3).unwrap();
        assert!(core.state().vault.is_none(), "still open with the old key");
        let events = core.emitted();
        assert_eq!(
            events.last().unwrap(),
            &(
                EVENT_LOCKED.to_owned(),
                serde_json::json!({ "reason": "keyChanged" })
            )
        );
        let result = outcome_of(&link, v.id());
        assert_eq!(result.outcome, Outcome::Updated);
        assert!(result.password_changed);
        // The fingerprint's KEK opens nothing any more: reset.
        assert!(!fingerprint_set_up());
        assert_eq!(
            core.unlock(v.id(), "pc-pw").unwrap_err().code(),
            "wrong_password"
        );
        core.unlock(v.id(), "neu").unwrap();

        // The flag stays while that revision is the newest ...
        sync_folder(&core, &folder, &mut link, 4).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::UpToDate);
        assert!(outcome_of(&link, v.id()).password_changed);
        // ... and goes with the next one; a locked vault gets no event.
        core.lock(None);
        let before = core.emitted().len();
        note(&mut v, "noch eins");
        pc.mirror(&v);
        sync_folder(&core, &folder, &mut link, 5).unwrap();
        assert!(!outcome_of(&link, v.id()).password_changed);
        assert_eq!(core.emitted().len(), before);
    }

    #[test]
    fn files_that_are_not_this_vault_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let v = pc.create("Privat");
        let w = pc.create("Arbeit");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let bytes = fs::read(v.path()).unwrap();
        // A renamed copy, another vault's content under this one's name, a
        // cloud app's conflict copy, half an upload, a foreign file, a
        // hidden temporary file and a backup.
        fs::write(pc.folder.join("Privat.keystead"), &bytes).unwrap();
        fs::write(
            pc.folder.join(format!("{}.keystead", w.id())),
            fs::read(v.path()).unwrap(),
        )
        .unwrap();
        fs::write(pc.folder.join(format!("{} (1).keystead", v.id())), &bytes).unwrap();
        let half = "0b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f";
        fs::write(
            pc.folder.join(format!("{half}.keystead")),
            &bytes[..bytes.len() / 2],
        )
        .unwrap();
        let foreign = "11111111-2222-3333-4444-555555555555";
        fs::write(pc.folder.join(format!("{foreign}.keystead")), b"PK\x03\x04").unwrap();
        fs::write(
            pc.folder
                .join(format!(".{}.keystead.0123456789abcdef.tmp", v.id())),
            &bytes,
        )
        .unwrap();
        fs::write(pc.folder.join(format!("{}.keystead.bak", v.id())), &bytes).unwrap();
        fs::create_dir(pc.folder.join("Unterordner.keystead")).unwrap();

        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        let by_name = |name: String| {
            link.files
                .iter()
                .find(|f| f.file_name == name)
                .unwrap_or_else(|| panic!("{name}: {:?}", link.files))
                .clone()
        };
        assert_eq!(
            by_name(format!("{}.keystead", v.id())).outcome,
            Outcome::Imported
        );
        assert_eq!(by_name("Privat.keystead".into()).outcome, Outcome::Misnamed);
        let swapped = by_name(format!("{}.keystead", w.id()));
        assert_eq!(swapped.outcome, Outcome::Misnamed);
        assert_eq!(swapped.vault_id.as_deref(), Some(v.id()));
        assert_eq!(
            by_name(format!("{} (1).keystead", v.id())).outcome,
            Outcome::Misnamed
        );
        let half = by_name(format!("{half}.keystead"));
        assert_eq!(half.outcome, Outcome::Unreadable);
        assert!(
            half.error.as_deref().unwrap().starts_with("corrupt:"),
            "{half:?}"
        );
        let foreign = by_name(format!("{foreign}.keystead"));
        assert_eq!(foreign.outcome, Outcome::Unreadable);
        assert!(foreign
            .error
            .as_deref()
            .unwrap()
            .starts_with("unsupported:"));
        assert_eq!(
            link.files.len(),
            6,
            "temporary file, backup and folder are not listed"
        );
        // Only the real vault is on the phone.
        let vaults = core.store().list_vaults().unwrap();
        assert_eq!(vaults.len(), 1);
        assert_eq!(vaults[0].id, v.id());
    }

    #[test]
    fn another_vault_at_the_place_and_damaged_copies() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let v = pc.create("Privat");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let local = core.store().vault_path(v.id());

        // The phone's file at that place is another vault: never replaced.
        let other = core
            .store()
            .create_vault_with_params("Fremd", "x", KdfParams::insecure_for_tests())
            .unwrap();
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::copy(other.path(), &local).unwrap();
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::OtherVault);
        assert_eq!(fs::read(&local).unwrap(), fs::read(other.path()).unwrap());

        // A damaged copy on the phone is replaced (it is only ever a copy).
        fs::write(&local, b"{ damaged").unwrap();
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Updated);
        assert_eq!(fs::read(&local).unwrap(), fs::read(v.path()).unwrap());
    }

    #[test]
    fn too_large_files_and_bad_blobs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        let item = v.save_item(VaultItem::new(ItemType::Note, "n")).unwrap();
        v.add_attachment(&item.id, "a.txt", b"hello").unwrap();
        pc.mirror(&v);
        let blobs = pc
            .folder
            .join(attachments::dir_for(v.path()).file_name().unwrap());
        // Not a blob, a temporary file and a misnamed copy next to the real one.
        fs::write(
            blobs.join("0b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f.bin"),
            b"not a blob",
        )
        .unwrap();
        fs::write(blobs.join(".x.bin.0123456789abcdef.tmp"), b"KSAT1").unwrap();
        fs::write(blobs.join("copy (1).bin"), b"KSAT1").unwrap();
        // A vault file over the limit (by its listed size).
        let big = "22222222-3333-4444-5555-666666666666";
        let file = fs::File::create(pc.folder.join(format!("{big}.keystead"))).unwrap();
        file.set_len(VAULT_FILE_MAX_BYTES + 1).unwrap();

        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        let result = outcome_of(&link, v.id());
        assert_eq!(result.outcome, Outcome::Imported);
        assert_eq!(
            (result.attachments_copied, result.attachments_failed),
            (1, 1)
        );
        let big = link
            .files
            .iter()
            .find(|f| f.file_name == format!("{big}.keystead"))
            .unwrap();
        assert_eq!(big.outcome, Outcome::TooLarge);
        assert!(
            !folder.copies.borrow().iter().any(|c| c.starts_with("2222")),
            "not even read"
        );
        let local_blobs = attachments::dir_for(&core.store().vault_path(v.id()));
        assert_eq!(fs::read_dir(local_blobs).unwrap().count(), 1);

        // A blob that was cut short on the phone is fetched again.
        let local_blob = fs::read_dir(attachments::dir_for(&core.store().vault_path(v.id())))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(&local_blob, b"KSAT1").unwrap();
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(outcome_of(&link, v.id()).attachments_copied, 1);
        assert!(fs::read(&local_blob).unwrap().len() > 5);
    }

    #[test]
    fn a_lost_folder_keeps_the_copies_and_reports_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let v = pc.create("Privat");
        let w = pc.create("Arbeit");
        pc.mirror(&v);
        pc.mirror(&w);
        let mut folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();

        // The PC stops mirroring "Arbeit": the phone keeps it.
        fs::remove_file(pc.folder.join(w.path().file_name().unwrap())).unwrap();
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(outcome_of(&link, w.id()).outcome, Outcome::Missing);
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::UpToDate);
        assert_eq!(core.store().list_vaults().unwrap().len(), 2);

        // The permission is gone: an error, the last results stay.
        folder.lost = true;
        let err = sync_folder(&core, &folder, &mut link, 3).unwrap_err();
        assert_eq!(err.code(), "unsupported:folder_access_lost");
        assert_eq!(
            link.last_error.as_deref(),
            Some("unsupported:folder_access_lost")
        );
        assert_eq!(link.last_sync_at, Some(2));
        assert_eq!(link.files.len(), 2);
        assert_eq!(core.store().list_vaults().unwrap().len(), 2);

        // Readable again: the error is cleared.
        folder.lost = false;
        sync_folder(&core, &folder, &mut link, 4).unwrap();
        assert_eq!(link.last_error, None);
        assert_eq!(FolderStatus::of(Some(&link)).last_sync_at, Some(4));
        assert!(!FolderStatus::of(None).linked);
    }
}
