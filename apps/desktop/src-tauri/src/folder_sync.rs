//! Cloud-folder sync of the phone (Block 5): copy-in since stage 2a,
//! two-way since stage 2b. See "Cloud-folder sync (phone)" under "Mobile
//! (Android)" in docs/ARCHITECTURE.md.
//!
//! The user links one folder of their own cloud (the Nextcloud or OneDrive
//! app on the phone offers it through the system picker); the PC keeps a
//! copy of each vault there – `<id>.keystead` plus its attachment folder
//! `<id>.keystead.attachments/<attachment id>.bin`. A sync reads that
//! folder, updates the phone's working copies in `<data>/vaults` and writes
//! the phone's newer copies back – only vault files, never blobs, and it
//! never removes or renames a file of the folder.
//!
//! * Every vault file is untrusted input: at most [`VAULT_FILE_MAX_BYTES`],
//!   named `<id>.keystead` after the id in its header, parsed and validated
//!   (`VaultFile::parse`) before anything is written.
//! * What to do with a vault is `keystead_core::sync::decide` on the heads
//!   of both copies (`sync::head_of`) and the vault's **sync base**
//!   (`sync_base.rs`: the last head the phone and this folder agreed on):
//!   only the folder's copy changed → pull (a new vault → import); only the
//!   phone's → push; both → a **conflict** for the user
//!   ([`resolve_conflict`]), nothing is merged and neither side is
//!   overwritten; the folder went back below the base → reported ("the
//!   PC's copy is older"), never taken; the folder's copy is gone after it
//!   was synced → the phone keeps its copy and does not write it again. Two
//!   copies of the same write with different revisions (a header changed
//!   by hand) are not "up to date": a raised revision is pulled (and then
//!   fails the checks below), a lowered one is "older".
//! * A push writes the phone's vault file through
//!   [`RemoteFolder::write_vault`], reads it back and compares the bytes (a
//!   second try, then `unsupported:sync_write_unverified`); only then the
//!   base becomes the pushed head. A link without write permission (a 2a
//!   link) reports `notWritable` and writes nothing.
//! * Copies on trial (`sync_trial.rs`, `<data>/sync/trial/`): the plain
//!   header – the revision included – is not authenticated without the
//!   keys, so anyone who can write the folder could raise it. A copy the
//!   sync stores is therefore **unconfirmed** until it opens on this phone
//!   (`sync_trial::unlock_copy`; or right away when the open vault's keys
//!   check it, `UnlockedVault::check_copy`): the copy it replaced is kept
//!   until then, and its revision – not the unconfirmed one's – is what the
//!   folder's next copy has to beat. A copy that does not open although its
//!   key did (forged or damaged) is rejected: the kept copy comes back (or,
//!   without one, the copy goes) and that write of the folder's file is not
//!   taken again (`Trial::rejected`). The open vault refuses a copy its
//!   keys prove forged without storing it – also as the other side of a
//!   conflict (no conflict is recorded for it). An unconfirmed copy is
//!   never pushed.
//! * Conflicts (`<data>/sync/conflicts.json`, [`SyncConflict`] per vault
//!   plus the two digests): recorded whenever a sync finds one, shown
//!   (`sync_conflicts`, `sync://conflict`) only once the open vault's keys
//!   checked the folder's copy; removed when a later sync finds the two
//!   sides agreeing again or the user resolves it.
//! * Attachments: blobs the phone lacks (or whose size differs – a blob
//!   never changes under its id) are stored **before** the phone's vault
//!   file is replaced, so a vault that references a new attachment never
//!   arrives without it. The folder's vault file is read first and its
//!   blob folder listed afterwards: the PC writes the blobs before the
//!   vault file, so every blob the copy that was read references is
//!   already there (a blob folder missing from the top-level listing, which
//!   was made before the vault file was read, is looked up again for a copy
//!   with attachments). Only
//!   names of the form `<id>.bin` with a blob's shape are taken
//!   (`attachments::looks_like_blob`; the key is in the vault, so nothing
//!   is decrypted here). Blobs the vault no longer references are left to
//!   the core's sweep (on unlock, and here after a reload of the open
//!   vault), which removes them once they are an hour old. The phone adds
//!   no attachments; a phone edit keeps the PC's (the payload still
//!   references them and their blobs stay in the folder).
//! * The open vault: the file is replaced under the state lock and reloaded
//!   (`reload_if_changed`) in the same step, so the monitor's own change
//!   check never sees it first. `vault://changed` follows; a new master
//!   password (`Error::KeyChanged`) locks with reason `keyChanged`.
//! * State: `<data>/sync/link.json` ([`LinkState`]: the folder's address
//!   and name, whether it may be written, when it was linked and last
//!   synced, the last error and what the last sync found per file – no
//!   secrets); copies arrive in `<data>/sync/incoming/` first and are
//!   removed right after reading; a push stages the file in
//!   `<data>/sync/outgoing/` for the plugin.
//!
//! The folder itself is behind [`RemoteFolder`]: on Android the Kotlin
//! plugin `KeysteadFolderPlugin` (`mobile::sync`), in the unit tests a
//! directory. Everything here is platform-neutral and tested on the host.

// Only the Android build reaches the sync through the folder plugin; the
// desktop compiles this module for its unit tests and iOS has no plugin yet.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use keystead_core::attachments;
use keystead_core::format::{self, VaultFile};
use keystead_core::model::{now_ms, VaultInfo};
use keystead_core::sync::{self, SyncBase, SyncDecision, SyncHead, VaultHeader};
use keystead_core::vault::CopyCheck;
use keystead_core::Error as CoreError;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::{log, Core, LockReason};
use crate::sync_base::{self, sync_dir};
use crate::sync_conflict::{self, SyncConflict};
use crate::sync_trial::{
    begin_trial, on_trial, remember_rejected, trial_lock, FileVersion, Trial, COPY_REJECTED,
};

/// Largest vault file copied in (bytes): 64 MiB.
pub use keystead_core::sync::VAULT_FILE_MAX_BYTES;
/// Largest blob: the largest attachment plus the blob's own bytes.
pub const BLOB_MAX_BYTES: u64 =
    attachments::ATTACHMENT_MAX_BYTES + attachments::BLOB_OVERHEAD as u64;

/// Error code: the folder cannot be read any more (the permission was
/// revoked, the cloud app removed, the folder deleted or moved). The user
/// has to pick it again.
pub const ACCESS_LOST: &str = "folder_access_lost";
/// Error code: a file is larger than its limit.
pub const TOO_LARGE: &str = "sync_file_too_large";
/// Error code: a vault file written to the folder did not read back equal
/// (twice) – the provider changed or cut it.
pub const WRITE_UNVERIFIED: &str = "sync_write_unverified";
/// Error code: the folder has no file of that name the provider lists, but
/// it would not create one under that name (it chose another, e.g.
/// `… (1).keystead`): nothing was written into a file of that name.
pub const NAME_TAKEN: &str = "sync_name_taken";
/// Error code: the app holds no write permission for the folder (pick it
/// again to grant it).
pub const NOT_WRITABLE: &str = "sync_not_writable";
/// Error code: "keep this side" for a conflict whose folder copy has
/// another master password – the side with the old password never wins.
pub const CONFLICT_PASSWORD: &str = "sync_conflict_password";
/// Error code: a copy changed since the conflict was recorded (or the
/// folder's is gone): it was looked at again; the UI fetches the list anew.
pub const CONFLICT_CHANGED: &str = "sync_conflict_changed";
/// Error code (per file): the phone's copy changed while the folder's was
/// about to replace it (a save meanwhile); the next sync looks again.
pub const LOCAL_CHANGED: &str = "sync_local_changed";
/// `sync://conflict` – payload `SyncConflict[]`: the open vault's checked
/// conflicts, whenever the recorded conflicts change.
pub const EVENT_CONFLICT: &str = "sync://conflict";

fn link_path(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("link.json")
}

fn incoming_dir(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("incoming")
}

/// `<data>/sync/outgoing`: a vault file waits here while the folder plugin
/// writes it into the folder (the only place it reads from).
pub fn outgoing_dir(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("outgoing")
}

fn conflicts_path(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("conflicts.json")
}

/// `<id>.keystead`.
fn file_name_of(id: &str) -> String {
    format!("{id}.{}", format::VAULT_EXTENSION)
}

/// `unsupported:folder_access_lost`.
pub fn access_lost() -> AppError {
    AppError::unsupported(ACCESS_LOST)
}

fn has_code(e: &AppError, detail: &str) -> bool {
    e.code() == format!("unsupported:{detail}")
}

fn is_access_lost(e: &AppError) -> bool {
    has_code(e, ACCESS_LOST)
}

fn is_too_large(e: &AppError) -> bool {
    has_code(e, TOO_LARGE)
}

fn conflict_changed() -> AppError {
    AppError::unsupported(CONFLICT_CHANGED)
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

    /// Whether the app may also write the folder (its persisted permission
    /// includes write). A folder that cannot be read any more →
    /// [`access_lost`].
    fn writable(&self) -> AppResult<bool>;

    /// Writes `bytes` as the top-level file `name` (always `<id>.keystead`):
    /// the file of that name is truncated and rewritten, or created when
    /// the folder has none. Never another file, never a blob; nothing is
    /// removed or renamed. Returns the file's entry (for the read-back).
    /// Errors: `unsupported:sync_not_writable`, `unsupported:sync_name_taken`,
    /// `unsupported:sync_write_unverified` (the size read back differs),
    /// [`access_lost`].
    fn write_vault(&self, name: &str, bytes: &[u8]) -> AppResult<RemoteEntry>;
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
    /// Could not be read from the folder, written on the phone or written
    /// to the folder (`error` says which).
    Failed,
    /// Synced before, no longer in the folder: the phone keeps its copy
    /// (and does not write it again).
    Missing,
    /// This write of the folder's copy did not open with the vault's keys
    /// (forged or damaged): not taken again – the phone keeps the last copy
    /// that opened, if it has one (see [`Trial`]).
    Rejected,
    /// The phone's copy was newer (or new): written to the folder.
    Pushed,
    /// Both the phone's and the folder's copy changed since they last
    /// agreed: nothing was written; the user decides (`sync_conflicts`).
    Conflict,
    /// The phone's copy is newer (or new), but the app may not write the
    /// folder (a link of stage 2a): pick the folder again.
    NotWritable,
    /// No folder is linked: the vault is only on the phone (it is written
    /// to the folder once a writable one is linked).
    LocalOnly,
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
    /// The folder copy's revision (after a push: the pushed one; for a
    /// vault the folder does not have: the phone's).
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
    /// password no longer opens it (kept until a newer revision arrives);
    /// for a `conflict`: the folder's copy has another master password.
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
    /// persisted permission for).
    pub tree_uri: String,
    /// The folder's display name.
    pub folder_name: String,
    /// When it was linked (ms since the epoch).
    pub linked_at: i64,
    /// The app may write the folder too (as of the last sync or the
    /// link); a link of stage 2a holds read permission only.
    #[serde(default)]
    pub writable: bool,
    /// The sync bases of the vaults the folder had were seeded from the
    /// results of stage 2a ([`seed_bases`]; a link made since needs none).
    #[serde(default)]
    pub bases_seeded: bool,
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
    pub fn new(tree_uri: String, folder_name: String, linked_at: i64, writable: bool) -> Self {
        LinkState {
            tree_uri,
            folder_name,
            linked_at,
            writable,
            bases_seeded: true,
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

    /// Removes `link.json`, the recorded conflicts (they belong to that
    /// folder) and leftovers of an interrupted copy; the vault copies and
    /// their sync bases stay (linking the same folder again keeps them).
    pub fn remove(data_dir: &Path) -> AppResult<()> {
        match fs::remove_file(link_path(data_dir)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        clear_conflicts(data_dir)?;
        let _ = fs::remove_dir_all(incoming_dir(data_dir));
        let _ = fs::remove_dir_all(outgoing_dir(data_dir));
        Ok(())
    }
}

/// The sync state for the UI (`folder_status`, `sync://status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    pub linked: bool,
    /// The app may write the linked folder (`false` without one).
    pub writable: bool,
    /// A sync is running right now.
    pub syncing: bool,
    pub folder_name: Option<String>,
    pub linked_at: Option<i64>,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
    /// Per vault file the last sync found; without a link: the phone's
    /// vaults, `localOnly` ([`local_only_results`]).
    pub files: Vec<FileResult>,
}

impl FolderStatus {
    pub fn of(link: Option<&LinkState>) -> Self {
        match link {
            None => FolderStatus {
                linked: false,
                writable: false,
                syncing: false,
                folder_name: None,
                linked_at: None,
                last_sync_at: None,
                last_error: None,
                files: Vec::new(),
            },
            Some(link) => FolderStatus {
                linked: true,
                writable: link.writable,
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

/// Without a linked folder: every vault on the phone, `localOnly`.
pub fn local_only_results(core: &Core) -> Vec<FileResult> {
    let store = core.store();
    local_vault_ids(&store.vaults_dir())
        .into_iter()
        .filter_map(|id| {
            let file = VaultFile::read(&store.vault_path(&id)).ok()?;
            (file.id == id).then(|| {
                FileResult::new(&file_name_of(&id), Outcome::LocalOnly)
                    .with_header(&VaultHeader::from(&file))
            })
        })
        .collect()
}

/// The ids of the vault files in `vaults_dir` (by name; nothing is read).
fn local_vault_ids(vaults_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(vaults_dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name();
            sync::vault_id_of_file_name(name.to_str()?).map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids
}

/// Stage 2a → 2b, once per link: the phone was read-only, so a vault the
/// last 2a sync found imported, updated or up to date at the revision the
/// phone's copy still has is the folder's copy – its head becomes the
/// vault's base (unless it has one for this folder already). Without it,
/// the first edit on either side would count as a conflict. Returns whether
/// the link changed (then the caller saves it).
pub fn seed_bases(core: &Core, link: &mut LinkState) -> bool {
    if link.bases_seeded {
        return false;
    }
    let data_dir = core.data_dir();
    let store = core.store();
    for result in &link.files {
        if !matches!(
            result.outcome,
            Outcome::Imported | Outcome::Updated | Outcome::UpToDate
        ) {
            continue;
        }
        let (Some(id), Some(revision)) = (result.vault_id.as_deref(), result.revision) else {
            continue;
        };
        if result.file_name != file_name_of(id)
            || sync_base::load(&data_dir, id, &link.tree_uri).is_some()
        {
            continue;
        }
        let Ok(file) = VaultFile::read(&store.vault_path(id)) else {
            continue;
        };
        if file.id != id || file.revision != revision {
            continue;
        }
        let base = SyncBase::new(&sync::head_of(&file), &link.tree_uri, now_ms());
        if let Err(e) = sync_base::save(&data_dir, id, &base) {
            log(format_args!("could not seed a sync base: {}", e.code()));
        }
    }
    link.bases_seeded = true;
    true
}

// ---------------------------------------------------------------------------
// Conflicts (`<data>/sync/conflicts.json`)
// ---------------------------------------------------------------------------

/// A recorded conflict: what the UI gets plus the two writes it is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConflictRecord {
    #[serde(flatten)]
    conflict: SyncConflict,
    /// The heads' digests of the phone's and the folder's copy.
    local_digest: String,
    remote_digest: String,
    /// The open vault's keys checked the folder's copy (not forged): only
    /// then the user is asked.
    #[serde(default)]
    verified: bool,
}

/// Read leniently: a missing or damaged file holds none.
fn load_records(data_dir: &Path) -> Vec<ConflictRecord> {
    match fs::read(conflicts_path(data_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log(format_args!("the sync conflicts are unreadable: {e}"));
            Vec::new()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            log(format_args!("could not read the sync conflicts: {e}"));
            Vec::new()
        }
    }
}

fn save_records(data_dir: &Path, records: &[ConflictRecord]) -> AppResult<()> {
    if records.is_empty() {
        return clear_conflicts(data_dir);
    }
    fs::create_dir_all(sync_dir(data_dir))?;
    let mut bytes = serde_json::to_vec_pretty(records).map_err(CoreError::from)?;
    bytes.push(b'\n');
    format::write_atomic(&conflicts_path(data_dir), &bytes, false)?;
    Ok(())
}

/// Forgets every recorded conflict (the folder was unlinked or another one
/// linked).
pub fn clear_conflicts(data_dir: &Path) -> AppResult<()> {
    match fs::remove_file(conflicts_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Records (`Some`) or forgets (`None`) the conflict of vault `id`; a
/// change is announced as `sync://conflict`. Call without the state lock.
fn set_record(core: &Core, id: &str, record: Option<ConflictRecord>) {
    let data_dir = core.data_dir();
    let before = load_records(&data_dir);
    let mut records: Vec<ConflictRecord> = before
        .iter()
        .filter(|r| r.conflict.vault_id != id)
        .cloned()
        .collect();
    records.extend(record);
    records.sort_by(|a, b| a.conflict.vault_id.cmp(&b.conflict.vault_id));
    if records == before {
        return;
    }
    if let Err(e) = save_records(&data_dir, &records) {
        log(format_args!(
            "could not record the sync conflicts: {}",
            e.code()
        ));
    }
    emit_conflicts(core);
}

/// The checked conflicts of vault `id`.
fn conflicts_of(data_dir: &Path, id: &str) -> Vec<SyncConflict> {
    load_records(data_dir)
        .into_iter()
        .filter(|r| r.verified && r.conflict.vault_id == id)
        .map(|r| r.conflict)
        .collect()
}

/// Announces the open vault's checked conflicts (`sync://conflict`; an
/// empty list without one). Call without the state lock.
pub fn emit_conflicts(core: &Core) {
    let conflicts = match core.read(|v| v.id().to_owned()) {
        Ok(id) => conflicts_of(&core.data_dir(), &id),
        Err(_) => Vec::new(),
    };
    core.emit(EVENT_CONFLICT, conflicts);
}

/// `sync_conflicts(pageVaultId)`: the conflicts of the page's vault the
/// open vault's keys checked (`locked` for another vault).
pub fn conflicts_for_page(
    core: &Core,
    page_vault_id: Option<&str>,
) -> AppResult<Vec<SyncConflict>> {
    let id = core.state().page_vault(page_vault_id)?.id().to_owned();
    Ok(conflicts_of(&core.data_dir(), &id))
}

// ---------------------------------------------------------------------------
// The sync
// ---------------------------------------------------------------------------

/// Reads the linked folder, updates the phone's vault copies and writes
/// the phone's newer ones back (see the module docs). Runs one at a time
/// (the caller holds the sync guard). `link` gets the result: on success
/// `last_sync_at = now`, no error, `writable` and the per-file results; on
/// failure only `last_error` (the previous results stay). Returns whether a
/// vault was added or replaced on the phone.
pub fn sync_folder(
    core: &Core,
    folder: &dyn RemoteFolder,
    link: &mut LinkState,
    now: i64,
) -> AppResult<bool> {
    match run_sync(core, folder, link, None) {
        Ok((files, writable)) => {
            let changed = files
                .iter()
                .any(|f| matches!(f.outcome, Outcome::Imported | Outcome::Updated));
            link.files = files;
            link.writable = writable;
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

/// [`sync_folder`] for vault `id` only – the push after an edit on the
/// phone (it reads the folder's copy first: a push never goes over a copy
/// it has not compared), and the check after an unlock. Only that vault's
/// entry of `link.files` changes.
pub fn sync_vault(
    core: &Core,
    folder: &dyn RemoteFolder,
    link: &mut LinkState,
    id: &str,
    now: i64,
) -> AppResult<()> {
    if !sync_base::valid_id(id) {
        return Err(AppError::invalid("vault id"));
    }
    match run_sync(core, folder, link, Some(id)) {
        Ok((files, writable)) => {
            let name = file_name_of(id);
            link.files.retain(|f| f.file_name != name);
            link.files.extend(files);
            link.files.sort_by(|a, b| a.file_name.cmp(&b.file_name));
            link.writable = writable;
            link.last_sync_at = Some(now);
            link.last_error = None;
            Ok(())
        }
        Err(e) => {
            link.last_error = Some(e.code());
            Err(e)
        }
    }
}

/// One sync's view of things.
struct Ctx<'a> {
    core: &'a Core,
    folder: &'a dyn RemoteFolder,
    /// The folder's address: what the bases are recorded for.
    tree: &'a str,
    writable: bool,
    incoming: &'a Path,
    data_dir: PathBuf,
}

/// The per-file results (of every file, or of vault `only`'s) and whether
/// the folder may be written.
fn run_sync(
    core: &Core,
    folder: &dyn RemoteFolder,
    link: &LinkState,
    only: Option<&str>,
) -> AppResult<(Vec<FileResult>, bool)> {
    let data_dir = core.data_dir();
    let incoming = incoming_dir(&data_dir);
    // Leftovers of an interrupted sync (the process was killed mid-copy).
    let _ = fs::remove_dir_all(&incoming);
    let writable = folder.writable()?;
    fs::create_dir_all(&incoming)?;
    let ctx = Ctx {
        core,
        folder,
        tree: &link.tree_uri,
        writable,
        incoming: &incoming,
        data_dir,
    };
    let result = sync_entries(&ctx, &link.files, only);
    let _ = fs::remove_dir_all(&incoming);
    result.map(|files| (files, writable))
}

fn sync_entries(
    ctx: &Ctx<'_>,
    previous: &[FileResult],
    only: Option<&str>,
) -> AppResult<Vec<FileResult>> {
    let top = ctx.folder.list(None)?;
    let only_name = only.map(file_name_of);
    let mut vault_files: Vec<&RemoteEntry> = top
        .iter()
        .filter(|e| !e.dir && !e.name.starts_with('.') && e.name.ends_with(".keystead"))
        .filter(|e| only_name.as_ref().is_none_or(|name| &e.name == name))
        .collect();
    vault_files.sort_by(|a, b| a.name.cmp(&b.name));

    let mut results = Vec::with_capacity(vault_files.len());
    for entry in vault_files {
        let before = previous.iter().find(|r| r.file_name == entry.name);
        results.push(sync_vault_file(ctx, &top, entry, before)?);
    }

    // The phone's vaults the folder does not have: new ones are written
    // there, ones synced before stay the phone's ("missing").
    let present: BTreeSet<&str> = top
        .iter()
        .filter(|e| !e.dir)
        .map(|e| e.name.as_str())
        .collect();
    let local_ids = match only {
        Some(id) => vec![id.to_owned()],
        None => local_vault_ids(&ctx.core.store().vaults_dir()),
    };
    for id in local_ids {
        let name = file_name_of(&id);
        if present.contains(name.as_str()) {
            continue;
        }
        let before = previous.iter().find(|r| r.file_name == name);
        if let Some(result) = sync_local_only(ctx, &id, before)? {
            results.push(result);
        }
    }

    // Vaults synced before that are neither in the folder nor on the phone
    // any more (a copy that was rejected and had nothing to go back to).
    let gone: Vec<FileResult> = previous
        .iter()
        .filter(|r| {
            r.vault_id.is_some()
                && only_name.as_ref().is_none_or(|name| &r.file_name == name)
                && !results.iter().any(|f| f.file_name == r.file_name)
        })
        .cloned()
        .collect();
    for mut missing in gone {
        missing.outcome = Outcome::Missing;
        missing.error = None;
        missing.attachments_copied = 0;
        missing.attachments_failed = 0;
        missing.password_changed = false;
        results.push(missing);
    }
    results.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(results)
}

/// Whether a result says the folder had that vault (so a phone copy without
/// a base is no new vault of the phone's).
fn seen_in_folder(result: &FileResult) -> bool {
    matches!(
        result.outcome,
        Outcome::Imported
            | Outcome::Updated
            | Outcome::UpToDate
            | Outcome::PcOlder
            | Outcome::Missing
            | Outcome::Rejected
            | Outcome::Conflict
            | Outcome::Pushed
    )
}

/// Vault `id` of the phone, which the folder does not have: written there
/// if it never was synced (and the folder may be written), kept as it is
/// otherwise. `None`: the phone has no readable copy either.
fn sync_local_only(
    ctx: &Ctx<'_>,
    id: &str,
    before: Option<&FileResult>,
) -> AppResult<Option<FileResult>> {
    let name = file_name_of(id);
    let path = ctx.core.store().vault_path(id);
    let Ok(bytes) = fs::read(&path) else {
        return Ok(None);
    };
    let Some(file) = VaultFile::parse(&bytes).ok().filter(|f| f.id == id) else {
        return Ok(None);
    };
    let head = sync::head_of(&file);
    let mut result =
        FileResult::new(&name, Outcome::Missing).with_header(&VaultHeader::from(&file));
    let base = sync_base::load(&ctx.data_dir, id, ctx.tree);
    // A copy the folder had (a 2a vault without a base, a copy on trial)
    // is never written back once the folder's file is gone.
    let from_folder = before
        .is_some_and(|b| b.vault_id.as_deref() == Some(id) && seen_in_folder(b))
        || on_trial(&ctx.data_dir, id);
    let decision = if base.is_none() && from_folder {
        SyncDecision::RemoteGone
    } else {
        sync::decide(Some(&head), None, base.as_ref())
    };
    if decision == SyncDecision::NewLocal {
        push_into(ctx, &mut result, &name, id, &bytes, &head)?;
    }
    set_record(ctx.core, id, None);
    Ok(Some(result))
}

/// The push of the phone's copy `bytes` (head `head`) as `name`, into
/// `result`: `pushed`, `notWritable` or `failed` with the error. Only a
/// folder that cannot be read any more is an error.
fn push_into(
    ctx: &Ctx<'_>,
    result: &mut FileResult,
    name: &str,
    id: &str,
    bytes: &[u8],
    head: &SyncHead,
) -> AppResult<()> {
    if !ctx.writable {
        result.outcome = Outcome::NotWritable;
        return Ok(());
    }
    match push(ctx, name, id, bytes, head)? {
        None => {
            result.outcome = Outcome::Pushed;
            result.revision = Some(head.revision);
        }
        Some(e) => {
            log(format_args!(
                "could not write a vault to the folder: {}",
                e.code()
            ));
            result.outcome = Outcome::Failed;
            result.error = Some(e.code());
        }
    }
    Ok(())
}

/// Writes `bytes` (the phone's copy of vault `id`, head `head`) as `name`
/// into the folder, reads it back and compares; a second try if it differs.
/// Verified → the base becomes `head`. `Some(error)`: not written (or not
/// verified); only a folder that cannot be read any more is an error.
fn push(
    ctx: &Ctx<'_>,
    name: &str,
    id: &str,
    bytes: &[u8],
    head: &SyncHead,
) -> AppResult<Option<AppError>> {
    let mut last = None;
    for _ in 0..2 {
        match write_and_verify(ctx, name, bytes) {
            Ok(()) => {
                let base = SyncBase::new(head, ctx.tree, now_ms());
                if let Err(e) = sync_base::save(&ctx.data_dir, id, &base) {
                    log(format_args!("could not record the sync base: {}", e.code()));
                }
                return Ok(None);
            }
            Err(e) if is_access_lost(&e) => return Err(e),
            Err(e) if has_code(&e, WRITE_UNVERIFIED) => last = Some(e),
            Err(e) => return Ok(Some(e)),
        }
    }
    Ok(Some(last.unwrap_or_else(|| {
        AppError::unsupported(WRITE_UNVERIFIED)
    })))
}

fn write_and_verify(ctx: &Ctx<'_>, name: &str, bytes: &[u8]) -> AppResult<()> {
    let entry = ctx.folder.write_vault(name, bytes)?;
    match fetch(ctx.folder, &entry, ctx.incoming, VAULT_FILE_MAX_BYTES) {
        Ok(back) if back == bytes => Ok(()),
        Err(e) if is_access_lost(&e) => Err(e),
        Ok(_) | Err(_) => Err(AppError::unsupported(WRITE_UNVERIFIED)),
    }
}

/// What one vault of the folder gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// The phone has no (readable) copy: the folder's is stored.
    Import,
    /// Only the folder's copy changed: it replaces the phone's.
    Pull,
    /// Only the phone's copy changed: it is written to the folder.
    Push,
    UpToDate,
    /// Both changed: recorded for the user.
    Conflict,
    /// The folder's copy is older: kept out.
    RemoteOlder,
    OtherVault,
}

/// `sync::decide` for two copies that are both there – except that the
/// same write with another revision (a header changed by hand) is not "up
/// to date": a raised revision is pulled (and fails the checks of a copy on
/// trial), a lowered one is older.
fn plan_for(local: &SyncHead, remote: &SyncHead, base: Option<&SyncBase>) -> Plan {
    if local.id == remote.id && local.digest == remote.digest && local.revision != remote.revision {
        return if remote.revision > local.revision {
            Plan::Pull
        } else {
            Plan::RemoteOlder
        };
    }
    match sync::decide(Some(local), Some(remote), base) {
        SyncDecision::OtherVault => Plan::OtherVault,
        SyncDecision::PullRemote => Plan::Pull,
        SyncDecision::PushLocal => Plan::Push,
        SyncDecision::UpToDate => Plan::UpToDate,
        SyncDecision::RemoteOlder => Plan::RemoteOlder,
        // Both copies are there: only `Diverged` is left.
        SyncDecision::Diverged
        | SyncDecision::NewRemote
        | SyncDecision::NewLocal
        | SyncDecision::RemoteGone => Plan::Conflict,
    }
}

/// One vault file of the folder. Only a folder that cannot be read any
/// more is an error (it ends the sync); everything else is the file's
/// result.
fn sync_vault_file(
    ctx: &Ctx<'_>,
    top: &[RemoteEntry],
    entry: &RemoteEntry,
    before: Option<&FileResult>,
) -> AppResult<FileResult> {
    let (core, folder) = (ctx.core, ctx.folder);
    let name = entry.name.as_str();
    let Some(id) = sync::vault_id_of_file_name(name) else {
        return Ok(FileResult::new(name, Outcome::Misnamed));
    };
    if entry.size.is_some_and(|size| size > VAULT_FILE_MAX_BYTES) {
        return Ok(FileResult::new(name, Outcome::TooLarge));
    }
    let bytes = match fetch(folder, entry, ctx.incoming, VAULT_FILE_MAX_BYTES) {
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
    let data_dir = &ctx.data_dir;
    let remote_version = FileVersion::of(&remote_file);
    let remote_head = sync::head_of(&remote_file);
    let trial = {
        let _trial = trial_lock();
        Trial::load(data_dir, id)
    };
    // This write did not open here before: not again.
    if trial.rejected.as_ref() == Some(&remote_version) {
        return Ok(FileResult::new(name, Outcome::Rejected).with_header(&remote));
    }

    let local_path = core.store().vault_path(id);
    let (local, damaged) = match fs::read(&local_path) {
        Ok(local_bytes) => match VaultFile::parse(&local_bytes) {
            Ok(file) => (Some((file, local_bytes)), false),
            Err(_) => (None, true),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, false),
        // The phone's copy cannot be read (it came from the folder or was
        // pushed there): the folder's replaces it.
        Err(_) => (None, true),
    };
    let local_head = local.as_ref().map(|(file, _)| sync::head_of(file));
    let plan = match (&trial.unconfirmed, &local, &local_head) {
        (_, None, _) | (_, _, None) => Plan::Import,
        // The phone's copy has not opened yet: its revision proves nothing
        // (anyone who can write the folder could have raised it) – the
        // folder's copy has to beat the last copy that opened here. It has
        // no edits of the phone's, so nothing is pushed or in conflict.
        (Some(_), Some((file, _)), _) if file.id == id => {
            if FileVersion::of(file) == remote_version {
                Plan::UpToDate
            } else if trial
                .kept_revision
                .is_none_or(|kept| remote.revision > kept)
            {
                Plan::Pull
            } else {
                Plan::RemoteOlder
            }
        }
        (_, Some(_), Some(local_head)) => {
            let base = sync_base::load(data_dir, id, ctx.tree);
            plan_for(local_head, &remote_head, base.as_ref())
        }
    };
    let mut result = FileResult::new(
        name,
        match plan {
            Plan::Import if damaged => Outcome::Updated,
            Plan::Import => Outcome::Imported,
            Plan::Pull => Outcome::Updated,
            Plan::Push => Outcome::Pushed,
            Plan::UpToDate => Outcome::UpToDate,
            Plan::Conflict => Outcome::Conflict,
            Plan::RemoteOlder => Outcome::PcOlder,
            Plan::OtherVault => Outcome::OtherVault,
        },
    )
    .with_header(&remote);
    if plan == Plan::OtherVault {
        return Ok(result);
    }

    // Attachments first: the vault file must not arrive before its blobs.
    let with_attachments = remote_file.version >= format::FORMAT_VERSION_ATTACHMENTS;
    let (copied, attachment_failures) =
        copy_attachments(folder, top, id, &local_path, ctx.incoming, with_attachments)?;
    result.attachments_copied = copied;
    result.attachments_failed = attachment_failures;

    match plan {
        Plan::Import | Plan::Pull => {
            result.password_changed = local
                .as_ref()
                .is_some_and(|(local, _)| local.kdf.salt != remote_file.kdf.salt);
            match install_copy(
                core,
                ctx.tree,
                &local_path,
                id,
                &bytes,
                &remote_file,
                local_head.as_ref(),
            ) {
                Ok(Stored::Yes) => {
                    // A new master password (another salt): the fingerprint
                    // unlock's sealed KEK opens nothing any more. Not for a
                    // copy on trial – its salt is unproven until it opens
                    // (`confirm` decides then).
                    if !on_trial(data_dir, id) {
                        crate::device_unlock::forget_if_stale(data_dir, id, &remote_file.kdf.salt);
                    }
                }
                Ok(Stored::Rejected) => {
                    log("a synced vault copy is forged or damaged: not taken");
                    result.outcome = Outcome::Rejected;
                    result.password_changed = false;
                }
                Ok(Stored::LocalChanged) => {
                    result.outcome = Outcome::Failed;
                    result.error = Some(AppError::unsupported(LOCAL_CHANGED).code());
                    result.password_changed = false;
                }
                Err(e) => {
                    log(format_args!("could not store the vault copy: {}", e.code()));
                    result.outcome = Outcome::Failed;
                    result.error = Some(e.code());
                    result.password_changed = false;
                }
            }
        }
        Plan::UpToDate => {
            // Still the revision whose new password the last sync reported.
            result.password_changed = before.is_some_and(|b| {
                b.password_changed
                    && b.vault_id.as_deref() == Some(id)
                    && b.revision == result.revision
            });
            // Both sides agree on this write: it is the base.
            let agreed = sync_base::load(data_dir, id, ctx.tree).is_some_and(|b| {
                b.revision == remote_head.revision && b.digest == remote_head.digest
            });
            if !agreed {
                let base = SyncBase::new(&remote_head, ctx.tree, now_ms());
                if let Err(e) = sync_base::save(data_dir, id, &base) {
                    log(format_args!("could not record the sync base: {}", e.code()));
                }
            }
        }
        Plan::Push => {
            if let (Some((_, local_bytes)), Some(local_head)) = (&local, &local_head) {
                push_into(ctx, &mut result, name, id, local_bytes, local_head)?;
            }
        }
        Plan::Conflict => {
            if let (Some((local_file, _)), Some(local_head)) = (&local, &local_head) {
                let outcome =
                    record_conflict(ctx, id, local_file, local_head, &remote_file, &remote_head);
                result.outcome = outcome;
                result.password_changed =
                    outcome == Outcome::Conflict && password_changed(local_file, &remote_file);
            }
        }
        Plan::RemoteOlder | Plan::OtherVault => {}
    }
    if result.outcome != Outcome::Conflict {
        set_record(core, id, None);
    }
    Ok(result)
}

/// The folder's copy has another master password than the phone's (another
/// key-derivation salt or parameters).
fn password_changed(local: &VaultFile, remote: &VaultFile) -> bool {
    local.kdf.salt != remote.kdf.salt || local.kdf_params() != remote.kdf_params()
}

/// Both copies of vault `id` changed: recorded for the user – unless the
/// open vault's keys prove the folder's copy forged (then it is rejected
/// like any forged copy and no conflict is recorded). Nothing is written to
/// either copy. Returns the file's outcome.
fn record_conflict(
    ctx: &Ctx<'_>,
    id: &str,
    local: &VaultFile,
    local_head: &SyncHead,
    remote: &VaultFile,
    remote_head: &SyncHead,
) -> Outcome {
    let check = {
        let st = ctx.core.state();
        st.vault
            .as_ref()
            .filter(|vault| vault.id() == id)
            .map(|vault| vault.check_copy(remote))
    };
    if check == Some(CopyCheck::Forged) {
        log("the folder's copy of a vault in conflict is forged or damaged: not taken");
        {
            let _trial = trial_lock();
            remember_rejected(&ctx.data_dir, id, FileVersion::of(remote));
        }
        set_record(ctx.core, id, None);
        return Outcome::Rejected;
    }
    // A check of the same two writes in an earlier session still counts.
    let checked_before = load_records(&ctx.data_dir).into_iter().any(|r| {
        r.conflict.vault_id == id
            && r.verified
            && r.local_digest == local_head.digest
            && r.remote_digest == remote_head.digest
    });
    let record = ConflictRecord {
        conflict: SyncConflict {
            vault_id: id.to_owned(),
            vault_name: local.name.clone(),
            local_revision: local.revision,
            remote_revision: remote.revision,
            local_updated_at: local.updated_at,
            remote_updated_at: remote.updated_at,
            password_changed: password_changed(local, remote),
        },
        local_digest: local_head.digest.clone(),
        remote_digest: remote_head.digest.clone(),
        verified: check.is_some() || checked_before,
    };
    set_record(ctx.core, id, Some(record));
    Outcome::Conflict
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
/// phone lacks. `top` is the folder's top level as listed before the vault
/// file was read; `with_attachments`: the copy that was read references
/// attachments (format version 2). Returns (copied, failed); only a folder
/// that cannot be read any more is an error.
fn copy_attachments(
    folder: &dyn RemoteFolder,
    top: &[RemoteEntry],
    id: &str,
    vault_path: &Path,
    incoming: &Path,
    with_attachments: bool,
) -> AppResult<(u32, u32)> {
    let dir_name = format!("{id}.keystead{}", attachments::DIR_SUFFIX);
    let listed = top.iter().find(|e| e.dir && e.name == dir_name).cloned();
    let remote_dir = match listed {
        Some(dir) => dir,
        // The blob folder may be newer than that listing (the PC added the
        // vault's first attachment meanwhile – it writes the blobs before
        // the vault file): a copy with attachments looks again.
        None if with_attachments => match folder.list(None) {
            Ok(fresh) => match fresh.into_iter().find(|e| e.dir && e.name == dir_name) {
                Some(dir) => dir,
                None => return Ok((0, 0)),
            },
            Err(e) if is_access_lost(&e) => return Err(e),
            Err(e) => {
                log(format_args!("could not list the folder: {}", e.code()));
                return Ok((0, 1));
            }
        },
        None => return Ok((0, 0)),
    };
    let entries = match folder.list(Some(&remote_dir)) {
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

/// What [`install_copy`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    Yes,
    /// The open vault's keys prove the copy forged or damaged: nothing was
    /// written, the write is remembered as rejected.
    Rejected,
    /// The phone's copy is no longer the one the decision was made on (a
    /// save meanwhile): nothing was written.
    LocalChanged,
}

/// Replaces the phone's copy of vault `id` with `bytes` (already
/// validated; `remote` is their header) – if the phone's copy is still
/// `expected` (its head when the sync decided; `None`: none or unreadable).
/// If that vault is open, its keys check the copy first
/// ([`UnlockedVault::check_copy`]): a forged one is refused, a genuine one
/// is stored confirmed, one of another master password unconfirmed; then
/// the vault is reloaded under the same state lock: the UI gets
/// `vault://changed`; a new master password closes it with reason
/// `keyChanged`. Without the open vault the copy is stored unconfirmed, the
/// copy that opened before kept ([`Trial`]). A stored copy becomes the
/// vault's sync base for the folder `tree` (best effort).
fn install_copy(
    core: &Core,
    tree: &str,
    path: &Path,
    id: &str,
    bytes: &[u8],
    remote: &VaultFile,
    expected: Option<&SyncHead>,
) -> AppResult<Stored> {
    let data_dir = core.data_dir();
    let (changed, closed) = {
        let mut st = core.state();
        let _trial = trial_lock();
        // An unlock may have rejected this very write since the sync looked.
        if Trial::load(&data_dir, id).rejected == Some(FileVersion::of(remote)) {
            return Ok(Stored::Rejected);
        }
        // An edit on the phone since the sync read its copy is never
        // overwritten (the push after it looks again).
        let on_disk = VaultFile::read(path).ok().map(|f| sync::head_of(&f));
        if on_disk.as_ref() != expected {
            return Ok(Stored::LocalChanged);
        }
        let check = st
            .vault
            .as_ref()
            .filter(|vault| vault.id() == id)
            .map(|vault| vault.check_copy(remote));
        match check {
            Some(CopyCheck::Forged) => {
                remember_rejected(&data_dir, id, FileVersion::of(remote));
                return Ok(Stored::Rejected);
            }
            // Genuine: nothing to keep, nothing on trial.
            Some(CopyCheck::Opens) => Trial::default().save(&data_dir, id)?,
            Some(CopyCheck::OtherPassword) | None => begin_trial(&data_dir, id, path, remote)?,
        }
        format::write_atomic(path, bytes, false)?;
        let base = SyncBase::new(&sync::head_of(remote), tree, now_ms());
        if let Err(e) = sync_base::save(&data_dir, id, &base) {
            log(format_args!("could not record the sync base: {}", e.code()));
        }
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
    Ok(Stored::Yes)
}

// ---------------------------------------------------------------------------
// Resolving a conflict
// ---------------------------------------------------------------------------

/// Which side of a conflict the user keeps (`resolve_sync_conflict`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The phone's copy: it is saved again above the folder's revision and
    /// written to the folder.
    Local,
    /// The folder's copy: it replaces the phone's.
    Remote,
}

impl Keep {
    /// `"local"` / `"remote"`; anything else → `invalid_input:keep`.
    pub fn parse(value: &str) -> AppResult<Keep> {
        match value {
            "local" => Ok(Keep::Local),
            "remote" => Ok(Keep::Remote),
            _ => Err(AppError::invalid("keep")),
        }
    }
}

/// Resolves the recorded conflict of vault `vault_id` – which must be the
/// open vault and the page's (`locked` otherwise) – by keeping one side.
/// The other side's vault file is kept as a conflict copy in
/// `<data>/conflicts/` (`sync_conflict::keep_conflict_copy`; never in the
/// folder). Both copies must still be the ones the conflict is about, and
/// the open vault's keys must not prove the folder's copy forged:
/// otherwise `unsupported:sync_conflict_changed` (the vault was synced
/// again) or `unsupported:sync_copy_rejected`.
///
/// * [`Keep::Remote`]: the folder's copy (blobs first) replaces the phone's
///   and the open session is replaced by it; a copy with another master
///   password is stored on trial and the vault locks (`keyChanged`).
/// * [`Keep::Local`]: refused for a folder copy with another master password
///   (`unsupported:sync_conflict_password`: the side with the old password
///   never wins); the open vault is saved again as a revision above the
///   folder's (`save_superseding`), the folder's head becomes the base and
///   the vault is pushed.
///
/// Returns the vault's info afterwards; `link` gets that vault's result.
pub fn resolve_conflict(
    core: &Core,
    folder: &dyn RemoteFolder,
    link: &mut LinkState,
    vault_id: &str,
    page_vault_id: Option<&str>,
    keep: Keep,
    now: i64,
) -> AppResult<VaultInfo> {
    {
        let st = core.state();
        if st.page_vault(page_vault_id)?.id() != vault_id {
            return Err(AppError::Locked);
        }
    }
    let data_dir = core.data_dir();
    let record = load_records(&data_dir)
        .into_iter()
        .find(|r| r.verified && r.conflict.vault_id == vault_id)
        .ok_or_else(|| AppError::Code("not_found".into()))?;
    if keep == Keep::Local && record.conflict.password_changed {
        return Err(AppError::unsupported(CONFLICT_PASSWORD));
    }
    let incoming = incoming_dir(&data_dir);
    let _ = fs::remove_dir_all(&incoming);
    fs::create_dir_all(&incoming)?;
    let resolved = resolve_in(core, folder, link, &record, keep, &incoming, now);
    let _ = fs::remove_dir_all(&incoming);
    match resolved {
        Ok(info) => {
            // The vault's result in the link (and, for "keep the phone's",
            // the push).
            if let Err(e) = sync_vault(core, folder, link, vault_id, now) {
                log(format_args!(
                    "sync after a resolved conflict failed: {}",
                    e.code()
                ));
            }
            Ok(info)
        }
        Err(e) => {
            if has_code(&e, CONFLICT_CHANGED) {
                // Look again: the conflict is recorded anew, or gone.
                if let Err(e) = sync_vault(core, folder, link, vault_id, now) {
                    log(format_args!(
                        "sync after a changed conflict failed: {}",
                        e.code()
                    ));
                }
            }
            Err(e)
        }
    }
}

fn resolve_in(
    core: &Core,
    folder: &dyn RemoteFolder,
    link: &mut LinkState,
    record: &ConflictRecord,
    keep: Keep,
    incoming: &Path,
    now: i64,
) -> AppResult<VaultInfo> {
    let id = record.conflict.vault_id.as_str();
    let data_dir = core.data_dir();
    let name = file_name_of(id);
    link.writable = folder.writable()?;
    let top = folder.list(None)?;
    let entry = top
        .iter()
        .find(|e| !e.dir && e.name == name)
        .ok_or_else(conflict_changed)?;
    let remote_bytes = fetch(folder, entry, incoming, VAULT_FILE_MAX_BYTES)?;
    let remote = VaultFile::parse(&remote_bytes).map_err(|_| conflict_changed())?;
    let remote_head = sync::head_of(&remote);
    if remote.id != id || remote_head.digest != record.remote_digest {
        return Err(conflict_changed());
    }
    let local_path = core.store().vault_path(id);
    let local_bytes = fs::read(&local_path)?;
    let local = VaultFile::parse(&local_bytes)?;
    if sync::head_of(&local).digest != record.local_digest {
        return Err(conflict_changed());
    }

    // The open vault's keys check the folder's copy (again: the record may
    // be from an earlier session).
    let check = {
        let st = core.state();
        let vault = st
            .vault
            .as_ref()
            .filter(|v| v.id() == id)
            .ok_or(AppError::Locked)?;
        vault.check_copy(&remote)
    };
    match check {
        CopyCheck::Forged => {
            {
                let _trial = trial_lock();
                remember_rejected(&data_dir, id, FileVersion::of(&remote));
            }
            set_record(core, id, None);
            return Err(AppError::unsupported(COPY_REJECTED));
        }
        CopyCheck::OtherPassword if keep == Keep::Local => {
            return Err(AppError::unsupported(CONFLICT_PASSWORD));
        }
        CopyCheck::Opens | CopyCheck::OtherPassword => {}
    }

    match keep {
        Keep::Local => {
            sync_conflict::keep_conflict_copy(&data_dir, id, &remote_bytes, None, now)?;
            let info = {
                let mut st = core.state();
                let vault = st
                    .vault
                    .as_mut()
                    .filter(|v| v.id() == id)
                    .ok_or(AppError::Locked)?;
                // Above the folder's revision: every other device takes it.
                vault.save_superseding(remote.revision)?;
                vault.info()
            };
            // The folder's copy is what the phone's now supersedes: the
            // next sync pushes.
            sync_base::save(
                &data_dir,
                id,
                &SyncBase::new(&remote_head, &link.tree_uri, now_ms()),
            )?;
            set_record(core, id, None);
            Ok(info)
        }
        Keep::Remote => {
            sync_conflict::keep_conflict_copy(&data_dir, id, &local_bytes, None, now)?;
            let with_attachments = remote.version >= format::FORMAT_VERSION_ATTACHMENTS;
            copy_attachments(folder, &top, id, &local_path, incoming, with_attachments)?;
            let info = install_resolved(
                core,
                &link.tree_uri,
                id,
                &local_path,
                &record.local_digest,
                &remote_bytes,
                &remote,
                check,
            )?;
            set_record(core, id, None);
            match info {
                Some(info) => Ok(info),
                None => core
                    .store()
                    .list_vaults()?
                    .into_iter()
                    .find(|v| v.id == id)
                    .ok_or_else(|| AppError::Code("not_found".into())),
            }
        }
    }
}

/// "Keep the folder's copy": stores `bytes` (`remote`, checked as `check`
/// by the open vault) in place of the phone's copy – if that is still the
/// write `local_digest` – and replaces the open session with one of the new
/// copy (same master password), or locks the vault (`keyChanged`: another
/// master password; the copy stays on trial until it opens). Returns the
/// new session's info, `None` after the lock.
#[allow(clippy::too_many_arguments)]
fn install_resolved(
    core: &Core,
    tree: &str,
    id: &str,
    path: &Path,
    local_digest: &str,
    bytes: &[u8],
    remote: &VaultFile,
    check: CopyCheck,
) -> AppResult<Option<VaultInfo>> {
    let data_dir = core.data_dir();
    let (info, closed) = {
        let mut st = core.state();
        let _trial = trial_lock();
        let kek = st
            .vault
            .as_ref()
            .filter(|v| v.id() == id)
            .ok_or(AppError::Locked)?
            .master_kek();
        let on_disk = VaultFile::read(path).ok().map(|f| sync::head_of(&f).digest);
        if on_disk.as_deref() != Some(local_digest) {
            return Err(conflict_changed());
        }
        match check {
            CopyCheck::Opens => Trial::default().save(&data_dir, id)?,
            _ => begin_trial(&data_dir, id, path, remote)?,
        }
        format::write_atomic(path, bytes, false)?;
        let base = SyncBase::new(&sync::head_of(remote), tree, now_ms());
        if let Err(e) = sync_base::save(&data_dir, id, &base) {
            log(format_args!("could not record the sync base: {}", e.code()));
        }
        let fresh = match check {
            CopyCheck::Opens => st
                .store
                .unlock_with_master_kek(id, &kek)
                .inspect_err(|e| {
                    log(format_args!("could not open the kept copy: {}", e.code()));
                })
                .ok(),
            _ => None,
        };
        match fresh {
            Some(fresh) => {
                // Blobs the kept copy no longer references (an hour old).
                fresh.sweep_attachments();
                let info = fresh.info();
                // The old session is dropped (and wiped) here.
                st.vault = Some(fresh);
                (Some(info), None)
            }
            None => {
                core.begin_lock();
                (None, st.vault.take())
            }
        }
    };
    match closed {
        Some(closed) => {
            core.finish_lock(Some(closed), Some(LockReason::KeyChanged));
        }
        None => core.emit_changed(),
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;

    use keystead_core::model::{ItemType, VaultInfo, VaultItem};
    use keystead_core::settings::Settings;
    use keystead_core::{KdfParams, UnlockedVault, VaultStore};

    use super::*;
    use crate::state::{EVENT_CHANGED, EVENT_LOCKED};
    use crate::sync_trial::{kept_path, trial_path, unlock_copy};

    /// A folder on disk standing in for the cloud folder; records the
    /// order of the copies and whether the phone's file `probe` existed at
    /// each. `after_list` runs once, right after the first top-level
    /// listing (the PC writing meanwhile).
    struct DirFolder {
        root: PathBuf,
        copies: RefCell<Vec<String>>,
        probe: Option<PathBuf>,
        probed: RefCell<Vec<bool>>,
        lost: bool,
        after_list: RefCell<Option<Box<dyn FnOnce()>>>,
        /// The app holds write permission (a 2a link: false).
        writable: Cell<bool>,
        /// The names written, in order.
        writes: RefCell<Vec<String>>,
        /// This many of the next writes store the bytes cut short (a
        /// provider that mangles the upload).
        mangle: Cell<u32>,
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
                after_list: RefCell::new(None),
                writable: Cell::new(true),
                writes: RefCell::new(Vec::new()),
                mangle: Cell::new(0),
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
            if parent.is_none() {
                if let Some(hook) = self.after_list.borrow_mut().take() {
                    hook();
                }
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

        fn writable(&self) -> AppResult<bool> {
            if self.lost {
                return Err(access_lost());
            }
            Ok(self.writable.get())
        }

        fn write_vault(&self, name: &str, bytes: &[u8]) -> AppResult<RemoteEntry> {
            if self.lost {
                return Err(access_lost());
            }
            if !self.writable.get() {
                return Err(AppError::unsupported(NOT_WRITABLE));
            }
            // Only top-level vault files, never a blob.
            assert!(sync::vault_id_of_file_name(name).is_some(), "{name}");
            self.writes.borrow_mut().push(name.to_owned());
            let path = self.root.join(name);
            if self.mangle.get() > 0 {
                self.mangle.set(self.mangle.get() - 1);
                fs::write(&path, &bytes[..bytes.len() / 2])?;
            } else {
                fs::write(&path, bytes)?;
            }
            Ok(RemoteEntry {
                name: name.to_owned(),
                uri: path.display().to_string(),
                dir: false,
                size: Some(fs::metadata(&path)?.len()),
            })
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
            mirror_files(v.path(), &self.folder);
        }
    }

    /// The vault file at `vault` and its blobs into `folder` (blobs first).
    fn mirror_files(vault: &Path, folder: &Path) {
        fs::create_dir_all(folder).unwrap();
        let file_name = vault.file_name().unwrap().to_owned();
        let blobs = attachments::dir_for(vault);
        if blobs.is_dir() {
            let target = folder.join(blobs.file_name().unwrap());
            fs::create_dir_all(&target).unwrap();
            for blob in fs::read_dir(&blobs).unwrap() {
                let blob = blob.unwrap();
                fs::copy(blob.path(), target.join(blob.file_name())).unwrap();
            }
        }
        fs::copy(vault, folder.join(file_name)).unwrap();
    }

    /// The phone's unlock with the PC's master password.
    fn open_on_phone(core: &Core, id: &str) -> AppResult<VaultInfo> {
        unlock_copy(core, id, |s| s.unlock(id, "pc-pw"))
    }

    /// Someone with write access to the folder raises the revision of `v`'s
    /// copy there (the rest stays the real file).
    fn forge(pc: &Pc, v: &UnlockedVault, revision: u64) {
        let target = pc.folder.join(v.path().file_name().unwrap());
        let mut file = VaultFile::read(&target).unwrap();
        file.revision = revision;
        fs::write(&target, file.to_json().unwrap()).unwrap();
    }

    fn phone(dir: &Path) -> Arc<Core> {
        let settings = Settings {
            browser_integration: false,
            ..Settings::default()
        };
        Core::for_tests(VaultStore::new(dir.join("phone")), settings)
    }

    fn link() -> LinkState {
        LinkState::new("content://tree/cloud".into(), "Keystead".into(), 1, true)
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
                "basesSeeded",
                "files",
                "folderName",
                "lastError",
                "lastSyncAt",
                "linkedAt",
                "treeUri",
                "writable"
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
        // A link of stage 2a: read only, its bases not seeded yet.
        let loaded = LinkState::load(dir.path()).unwrap();
        let mut legacy = LinkState::new("u".into(), "F".into(), 5, false);
        legacy.bases_seeded = false;
        assert_eq!(loaded, legacy);
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
        // It opens on the phone: from now on its revision counts (a copy
        // that never opened here proves nothing, see `Trial`).
        open_on_phone(&core, v.id()).unwrap();
        core.lock(None);

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
        // The phone changed nothing: nothing was written to the folder.
        assert!(folder.writes.borrow().is_empty());
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
        let vault_id = v.id().to_owned();
        let unlock =
            |password: &str| unlock_copy(&core, &vault_id, |s| s.unlock(&vault_id, password));
        unlock("pc-pw").unwrap();
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
        // Its keys could not check it (another master password): on trial
        // until the new password opens it, the copy before it kept – and
        // until then its salt proves nothing: the fingerprint setup stays.
        assert!(kept_path(&core.data_dir(), &vault_id).is_file());
        assert!(fingerprint_set_up());
        assert_eq!(unlock("pc-pw").unwrap_err().code(), "wrong_password");
        assert!(kept_path(&core.data_dir(), &vault_id).is_file());
        assert!(fingerprint_set_up());
        unlock("neu").unwrap();
        assert!(
            !trial_path(&core.data_dir(), &vault_id).exists(),
            "confirmed"
        );
        assert!(!kept_path(&core.data_dir(), &vault_id).exists());
        // Now the new password is proven: the fingerprint's KEK opens
        // nothing any more – reset.
        assert!(!fingerprint_set_up());

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

    #[test]
    fn a_forged_copy_never_replaces_the_last_copy_that_opened() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        note(&mut v, "eins");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        open_on_phone(&core, v.id()).unwrap();
        let kek = core.read(|o| o.master_kek()).unwrap();
        core.lock(None);
        let data_dir = core.data_dir();
        let local = core.store().vault_path(v.id());
        let good = fs::read(&local).unwrap();
        // The stored copy is the vault's sync base for this folder.
        let (vault_id, tree) = (v.id().to_owned(), link.tree_uri.clone());
        let base_of = |data_dir: &Path| {
            sync_base::load(data_dir, &vault_id, &tree).map(|b| (b.revision, b.digest))
        };
        let head = |bytes: &[u8]| {
            let head = sync::head_of(&VaultFile::parse(bytes).unwrap());
            Some((head.revision, head.digest))
        };
        assert_eq!(base_of(&data_dir), head(&good));

        // A raised revision: stored, but on trial – the copy before it kept;
        // the base is its head until it fails to open.
        forge(&pc, &v, u64::MAX);
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Updated);
        assert_ne!(fs::read(&local).unwrap(), good);
        assert_eq!(fs::read(kept_path(&data_dir, v.id())).unwrap(), good);
        assert_eq!(base_of(&data_dir).unwrap().0, u64::MAX);

        // The unlock (here the fingerprint's KEK) finds it broken and opens
        // the copy before it instead.
        let id = v.id().to_owned();
        let info = unlock_copy(&core, &id, |s| s.unlock_with_master_kek(&id, &kek)).unwrap();
        assert_eq!(info.id, id);
        assert_eq!(
            core.read(|o| (o.revision(), o.items().len())).unwrap(),
            (v.revision(), 1)
        );
        assert_eq!(fs::read(&local).unwrap(), good);
        assert!(!kept_path(&data_dir, &id).exists());
        assert_eq!(base_of(&data_dir), head(&good), "back with the kept copy");
        core.lock(None);

        // That write is never taken again ...
        sync_folder(&core, &folder, &mut link, 3).unwrap();
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::Rejected);
        assert_eq!(fs::read(&local).unwrap(), good);
        // ... and the PC's next genuine revision – far below the forged one
        // – still arrives.
        note(&mut v, "zwei");
        pc.mirror(&v);
        sync_folder(&core, &folder, &mut link, 4).unwrap();
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::Updated);
        open_on_phone(&core, &id).unwrap();
        assert_eq!(core.read(|o| o.items().len()).unwrap(), 2);
        assert!(
            !trial_path(&data_dir, &id).exists(),
            "nothing left to remember"
        );
    }

    #[test]
    fn a_forged_key_wrapping_is_proved_by_the_copy_that_opened() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        note(&mut v, "eins");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        open_on_phone(&core, v.id()).unwrap();
        core.lock(None);
        let data_dir = core.data_dir();
        let id = v.id().to_owned();
        let local = core.store().vault_path(&id);
        let good = fs::read(&local).unwrap();

        // Same salt, a wrapped key that opens with nothing, a raised
        // revision: the right password gets `wrong_password` from it.
        let target = pc.folder.join(v.path().file_name().unwrap());
        let mut file = VaultFile::read(&target).unwrap();
        let c = &mut file.wrapped_key.ciphertext;
        let first = if c.starts_with('A') { "B" } else { "A" };
        c.replace_range(0..1, first);
        file.revision = 99;
        fs::write(&target, file.to_json().unwrap()).unwrap();
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::Updated);
        assert!(on_trial(&data_dir, &id));

        // A wrong password stays a wrong password; nothing changes.
        let err = unlock_copy(&core, &id, |s| s.unlock(&id, "falsch")).unwrap_err();
        assert_eq!(err.code(), "wrong_password");
        assert!(on_trial(&data_dir, &id));
        assert!(!sync_dir(&data_dir).join("probe").exists());

        // The right one opens the kept copy: the copy on trial is forged.
        open_on_phone(&core, &id).unwrap();
        assert_eq!(
            core.read(|o| (o.revision(), o.items().len())).unwrap(),
            (v.revision(), 1)
        );
        assert_eq!(fs::read(&local).unwrap(), good);
        assert!(!on_trial(&data_dir, &id));
        assert!(!sync_dir(&data_dir).join("probe").exists());
        core.lock(None);
        sync_folder(&core, &folder, &mut link, 3).unwrap();
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::Rejected);
    }

    #[test]
    fn an_unconfirmed_copy_does_not_raise_the_revision_the_phone_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        open_on_phone(&core, v.id()).unwrap();
        core.lock(None);
        let data_dir = core.data_dir();
        let confirmed = fs::read(core.store().vault_path(v.id())).unwrap();

        forge(&pc, &v, 1_000);
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        // Before anyone unlocks, the PC saves: revision 2 has to beat the
        // copy that opened (1), not the forged 1000.
        note(&mut v, "neu");
        pc.mirror(&v);
        sync_folder(&core, &folder, &mut link, 3).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Updated);
        assert_eq!(
            fs::read(kept_path(&data_dir, v.id())).unwrap(),
            confirmed,
            "still the copy that opened"
        );
        // The same write again: nothing to do.
        sync_folder(&core, &folder, &mut link, 4).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::UpToDate);
        open_on_phone(&core, v.id()).unwrap();
        assert_eq!(
            core.read(|o| (o.revision(), o.items().len())).unwrap(),
            (2, 1)
        );
        assert!(!trial_path(&data_dir, v.id()).exists());
        assert!(!kept_path(&data_dir, v.id()).exists());
    }

    #[test]
    fn the_open_vault_refuses_a_copy_its_keys_prove_forged() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let v = pc.create("Privat");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        open_on_phone(&core, v.id()).unwrap();
        let local = core.store().vault_path(v.id());
        let before = fs::read(&local).unwrap();

        forge(&pc, &v, 7);
        for now in [2, 3] {
            assert!(!sync_folder(&core, &folder, &mut link, now).unwrap());
            assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Rejected);
            assert_eq!(fs::read(&local).unwrap(), before, "nothing written");
        }
        assert_eq!(core.read(|o| o.revision()).unwrap(), 1, "still open");
        assert!(core.emitted().is_empty());
        assert!(!kept_path(&core.data_dir(), v.id()).exists());
    }

    #[test]
    fn a_forged_first_copy_is_removed_once_it_fails_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        pc.mirror(&v);
        forge(&pc, &v, 50);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Imported);

        // No copy before it to go back to: it goes.
        let err = open_on_phone(&core, v.id()).unwrap_err();
        assert_eq!(err.code(), "unsupported:sync_copy_rejected");
        assert!(core.store().list_vaults().unwrap().is_empty());
        assert!(core.state().vault.is_none());
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Rejected);
        assert!(core.store().list_vaults().unwrap().is_empty());

        // The PC's next copy brings the vault.
        note(&mut v, "neu");
        pc.mirror(&v);
        sync_folder(&core, &folder, &mut link, 3).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Imported);
        open_on_phone(&core, v.id()).unwrap();
        assert_eq!(core.read(|o| o.items().len()).unwrap(), 1);
    }

    #[test]
    fn a_blob_folder_newer_than_the_listing_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();

        // The PC adds the vault's first attachment and mirrors it right
        // after the phone's next sync listed the folder – before it reads
        // the vault file.
        let item = v.save_item(VaultItem::new(ItemType::Note, "n")).unwrap();
        let attachment = v
            .add_attachment(&item.id, "scan.png", b"\x89PNG scan")
            .unwrap();
        let (vault, cloud) = (v.path().to_path_buf(), pc.folder.clone());
        *folder.after_list.borrow_mut() = Some(Box::new(move || mirror_files(&vault, &cloud)));
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        let result = outcome_of(&link, v.id());
        assert_eq!(result.outcome, Outcome::Updated);
        assert_eq!(
            (result.attachments_copied, result.attachments_failed),
            (1, 0)
        );
        let opened = core.store().unlock(v.id(), "pc-pw").unwrap();
        assert_eq!(
            &*opened.read_attachment(&attachment.id).unwrap(),
            b"\x89PNG scan"
        );
    }

    // -----------------------------------------------------------------------
    // Two-way (stage 2b)
    // -----------------------------------------------------------------------

    /// An edit on the phone (the open vault).
    fn edit_on_phone(core: &Core, name: &str) {
        core.mutate(|v| v.save_item(VaultItem::new(ItemType::Note, name)))
            .unwrap();
    }

    fn names_of(v: &UnlockedVault) -> Vec<String> {
        let mut names: Vec<String> = v.items().iter().map(|i| i.name.clone()).collect();
        names.sort();
        names
    }

    /// Opens the folder's copy of vault `id` (with its blobs) from a
    /// scratch store under `dir`.
    fn open_folder_copy(dir: &Path, folder: &Path, id: &str, password: &str) -> UnlockedVault {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let store =
            VaultStore::new(dir.join(format!("probe-{}", NEXT.fetch_add(1, Ordering::Relaxed))));
        mirror_files(&folder.join(format!("{id}.keystead")), &store.vaults_dir());
        store.unlock(id, password).unwrap()
    }

    /// The PC takes the folder's copy of `v` (what its mirror pull does).
    fn pc_takes_folder_copy(pc: &Pc, v: &mut UnlockedVault) {
        fs::copy(pc.folder.join(v.path().file_name().unwrap()), v.path()).unwrap();
        assert!(v.reload_if_changed().unwrap());
    }

    fn base_head(core: &Core, link: &LinkState, id: &str) -> Option<(u64, String)> {
        sync_base::load(&core.data_dir(), id, &link.tree_uri).map(|b| (b.revision, b.digest))
    }

    fn head_of_path(path: &Path) -> (u64, String) {
        let head = sync::head_of(&VaultFile::read(path).unwrap());
        (head.revision, head.digest)
    }

    fn conflict_events(core: &Core) -> Vec<serde_json::Value> {
        core.emitted()
            .into_iter()
            .filter(|(name, _)| name == EVENT_CONFLICT)
            .map(|(_, payload)| payload)
            .collect()
    }

    /// The PC's vault "Privat" with one note, in the folder; the phone
    /// imported it and has it open.
    struct TwoSides {
        dir: tempfile::TempDir,
        pc: Pc,
        v: UnlockedVault,
        folder: DirFolder,
        core: Arc<Core>,
        link: LinkState,
        id: String,
    }

    fn two_sides() -> TwoSides {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        note(&mut v, "vom PC");
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        let id = v.id().to_owned();
        open_on_phone(&core, &id).unwrap();
        TwoSides {
            dir,
            pc,
            v,
            folder,
            core,
            link,
            id,
        }
    }

    impl TwoSides {
        fn remote_path(&self) -> PathBuf {
            self.pc.folder.join(format!("{}.keystead", self.id))
        }

        fn local_path(&self) -> PathBuf {
            self.core.store().vault_path(&self.id)
        }

        fn sync(&mut self, now: i64) -> FileResult {
            sync_folder(&self.core, &self.folder, &mut self.link, now).unwrap();
            outcome_of(&self.link, &self.id).clone()
        }

        fn push(&mut self, now: i64) -> FileResult {
            sync_vault(&self.core, &self.folder, &mut self.link, &self.id, now).unwrap();
            outcome_of(&self.link, &self.id).clone()
        }

        fn resolve(&mut self, keep: Keep) -> AppResult<VaultInfo> {
            resolve_conflict(
                &self.core,
                &self.folder,
                &mut self.link,
                &self.id,
                Some(&self.id),
                keep,
                1_792_764_309_000,
            )
        }

        fn conflicts(&self) -> Vec<SyncConflict> {
            conflicts_for_page(&self.core, Some(&self.id)).unwrap()
        }

        fn conflicts_file(&self) -> PathBuf {
            self.dir.path().join("phone/sync/conflicts.json")
        }

        /// Both sides edit after the last agreed state.
        fn diverge(&mut self) {
            edit_on_phone(&self.core, "vom Handy");
            note(&mut self.v, "noch mal vom PC");
            self.pc.mirror(&self.v);
        }
    }

    #[test]
    fn a_phone_edit_is_pushed_verified_and_the_pc_takes_it() {
        let mut s = two_sides();
        assert_eq!(
            base_head(&s.core, &s.link, &s.id),
            Some(head_of_path(&s.local_path()))
        );

        edit_on_phone(&s.core, "vom Handy");
        let result = s.push(2);
        assert_eq!(result.outcome, Outcome::Pushed);
        assert_eq!(result.revision, Some(3));
        assert_eq!(*s.folder.writes.borrow(), [format!("{}.keystead", s.id)]);
        // Written, read back and equal.
        assert_eq!(
            fs::read(s.remote_path()).unwrap(),
            fs::read(s.local_path()).unwrap()
        );
        assert_eq!(
            base_head(&s.core, &s.link, &s.id),
            Some(head_of_path(&s.local_path()))
        );
        assert!(!s.dir.path().join("phone/sync/incoming").exists());

        // The PC takes it (its pull) and edits again; the open vault on the
        // phone gets that at once.
        pc_takes_folder_copy(&s.pc, &mut s.v);
        assert_eq!(names_of(&s.v), ["vom Handy", "vom PC"]);
        note(&mut s.v, "wieder vom PC");
        s.pc.mirror(&s.v);
        let before = s.core.emitted().len();
        assert_eq!(s.sync(3).outcome, Outcome::Updated);
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 3);
        assert!(s.core.emitted()[before..]
            .iter()
            .any(|(e, _)| e == EVENT_CHANGED));
        assert_eq!(
            base_head(&s.core, &s.link, &s.id),
            Some(head_of_path(&s.remote_path()))
        );
        // Nothing to do then: up to date, nothing written.
        s.folder.writes.borrow_mut().clear();
        assert_eq!(s.sync(4).outcome, Outcome::UpToDate);
        assert_eq!(s.push(5).outcome, Outcome::UpToDate);
        assert!(s.folder.writes.borrow().is_empty());

        // Locked: the next copy is pulled on trial.
        s.core.lock(None);
        note(&mut s.v, "noch eins");
        s.pc.mirror(&s.v);
        assert_eq!(s.sync(6).outcome, Outcome::Updated);
        assert!(on_trial(&s.core.data_dir(), &s.id));
        open_on_phone(&s.core, &s.id).unwrap();
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 4);
        assert!(!on_trial(&s.core.data_dir(), &s.id));
        assert!(conflict_events(&s.core).is_empty(), "never a conflict");
    }

    #[test]
    fn a_lost_update_is_a_conflict_and_neither_side_is_overwritten() {
        let mut s = two_sides();
        s.diverge();
        let local = fs::read(s.local_path()).unwrap();
        let remote = fs::read(s.remote_path()).unwrap();
        let result = s.sync(2);
        assert_eq!(result.outcome, Outcome::Conflict);
        assert!(!result.password_changed);
        assert_eq!(fs::read(s.local_path()).unwrap(), local);
        assert_eq!(fs::read(s.remote_path()).unwrap(), remote);
        assert!(s.folder.writes.borrow().is_empty());
        // The open vault's keys checked the folder's copy: the user is asked.
        let conflicts = s.conflicts();
        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(
            (c.vault_id.as_str(), c.vault_name.as_str()),
            (s.id.as_str(), "Privat")
        );
        assert_eq!((c.local_revision, c.remote_revision), (3, 3));
        assert!(!c.password_changed);
        let events = conflict_events(&s.core);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0][0]["vaultId"], s.id.as_str());
        assert_eq!(events[0][0]["localRevision"], 3);
        // Still the same conflict: no new event, nothing written.
        assert_eq!(s.sync(3).outcome, Outcome::Conflict);
        assert_eq!(s.push(4).outcome, Outcome::Conflict);
        assert_eq!(conflict_events(&s.core).len(), 1);
        assert!(s.folder.writes.borrow().is_empty());
        // Another vault's page gets `locked`.
        assert_eq!(
            conflicts_for_page(&s.core, Some("other"))
                .unwrap_err()
                .code(),
            "locked"
        );
        // camelCase, these keys (the UI's plus the two digests and the check).
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(s.conflicts_file()).unwrap()).unwrap();
        let mut keys: Vec<_> = stored[0].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "localDigest",
                "localRevision",
                "localUpdatedAt",
                "passwordChanged",
                "remoteDigest",
                "remoteRevision",
                "remoteUpdatedAt",
                "vaultId",
                "vaultName",
                "verified"
            ]
        );

        // The folder gets the phone's state after all (resolved on the PC):
        // both agree again, the conflict goes.
        fs::copy(s.local_path(), s.remote_path()).unwrap();
        assert_eq!(s.sync(5).outcome, Outcome::UpToDate);
        assert!(s.conflicts().is_empty());
        assert_eq!(
            conflict_events(&s.core).last().unwrap(),
            &serde_json::json!([])
        );
    }

    #[test]
    fn a_conflict_found_while_locked_is_shown_once_the_vault_checked_it() {
        let mut s = two_sides();
        edit_on_phone(&s.core, "vom Handy");
        s.core.lock(None);
        note(&mut s.v, "vom PC 2");
        s.pc.mirror(&s.v);
        assert_eq!(s.sync(2).outcome, Outcome::Conflict);
        assert!(s.conflicts_file().is_file());
        // Not yet checked by the vault's keys: nothing to ask.
        open_on_phone(&s.core, &s.id).unwrap();
        assert!(s.conflicts().is_empty());
        // The sync after the unlock (the phone's `request_mirror`) checks it.
        assert_eq!(s.push(3).outcome, Outcome::Conflict);
        assert_eq!(s.conflicts().len(), 1);
        // A later session keeps that check.
        s.core.lock(None);
        assert_eq!(s.sync(4).outcome, Outcome::Conflict);
        open_on_phone(&s.core, &s.id).unwrap();
        assert_eq!(s.conflicts().len(), 1);
    }

    #[test]
    fn keeping_the_phone_state_supersedes_the_folder_copy() {
        let mut s = two_sides();
        s.diverge();
        s.sync(2);
        let remote = fs::read(s.remote_path()).unwrap();

        let info = s.resolve(Keep::Local).unwrap();
        assert_eq!(info.id, s.id);
        // The folder's copy is kept as a conflict copy on the phone.
        let copies = sync_conflict::list(&s.core.data_dir(), Some(&s.id)).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0].path).unwrap(), remote);
        assert!(copies[0]
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".keystead.conflict-20261023-140509"));
        // The phone's state, above the folder's revision, is in the folder.
        assert_eq!(s.core.read(|o| o.revision()).unwrap(), 4);
        assert_eq!(
            fs::read(s.remote_path()).unwrap(),
            fs::read(s.local_path()).unwrap()
        );
        assert_eq!(outcome_of(&s.link, &s.id).outcome, Outcome::Pushed);
        assert_eq!(
            base_head(&s.core, &s.link, &s.id),
            Some(head_of_path(&s.local_path()))
        );
        assert!(s.conflicts().is_empty());
        assert!(!s.conflicts_file().exists());
        // The PC takes it: a higher revision than its own.
        pc_takes_folder_copy(&s.pc, &mut s.v);
        assert_eq!(names_of(&s.v), ["vom Handy", "vom PC"]);
        // Nothing left to resolve.
        assert_eq!(s.resolve(Keep::Local).unwrap_err().code(), "not_found");
    }

    #[test]
    fn keeping_the_pc_state_replaces_the_phone_copy() {
        let mut s = two_sides();
        s.diverge();
        s.sync(2);
        let local = fs::read(s.local_path()).unwrap();
        let before = s.core.emitted().len();

        let info = s.resolve(Keep::Remote).unwrap();
        assert_eq!(info.id, s.id);
        let copies = sync_conflict::list(&s.core.data_dir(), Some(&s.id)).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0].path).unwrap(), local);
        // The PC's state is the phone's now – also in the open session.
        assert_eq!(
            fs::read(s.local_path()).unwrap(),
            fs::read(s.remote_path()).unwrap()
        );
        assert_eq!(
            s.core.read(|o| (o.revision(), names_of(o))).unwrap(),
            (3, vec!["noch mal vom PC".to_owned(), "vom PC".to_owned()])
        );
        assert!(s.core.emitted()[before..]
            .iter()
            .any(|(e, _)| e == EVENT_CHANGED));
        assert_eq!(
            base_head(&s.core, &s.link, &s.id),
            Some(head_of_path(&s.remote_path()))
        );
        assert_eq!(outcome_of(&s.link, &s.id).outcome, Outcome::UpToDate);
        assert!(s.conflicts().is_empty());
        assert!(s.folder.writes.borrow().is_empty(), "nothing written");
        assert!(!on_trial(&s.core.data_dir(), &s.id));
        // The phone goes on from there: its next edit is pushed.
        edit_on_phone(&s.core, "danach");
        assert_eq!(s.push(3).outcome, Outcome::Pushed);
    }

    #[test]
    fn a_resolution_needs_the_copies_it_was_about() {
        let mut s = two_sides();
        s.diverge();
        s.sync(2);
        // The PC saved again since the conflict was found.
        note(&mut s.v, "ganz neu vom PC");
        s.pc.mirror(&s.v);
        let local = fs::read(s.local_path()).unwrap();
        assert_eq!(
            s.resolve(Keep::Remote).unwrap_err().code(),
            "unsupported:sync_conflict_changed"
        );
        assert_eq!(fs::read(s.local_path()).unwrap(), local, "nothing taken");
        assert!(sync_conflict::list(&s.core.data_dir(), None)
            .unwrap()
            .is_empty());
        // Recorded again for the new copy.
        let conflicts = s.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].remote_revision, 4);
        // Only the page's own open vault.
        let err = resolve_conflict(
            &s.core,
            &s.folder,
            &mut s.link,
            &s.id,
            Some("other"),
            Keep::Remote,
            1,
        )
        .unwrap_err();
        assert_eq!(err.code(), "locked");
        assert_eq!(Keep::parse("local").unwrap(), Keep::Local);
        assert_eq!(Keep::parse("remote").unwrap(), Keep::Remote);
        assert_eq!(
            Keep::parse("both").unwrap_err().code(),
            "invalid_input:keep"
        );
    }

    #[test]
    fn a_forged_folder_copy_is_rejected_without_a_conflict() {
        let mut s = two_sides();
        edit_on_phone(&s.core, "vom Handy");
        let local = fs::read(s.local_path()).unwrap();
        // Someone with write access changes the folder's copy: a raised
        // revision and a payload that does not open.
        let mut file = VaultFile::read(&s.remote_path()).unwrap();
        file.revision += 5;
        let c = &mut file.payload.ciphertext;
        let first = if c.starts_with('A') { "B" } else { "A" };
        c.replace_range(0..1, first);
        fs::write(s.remote_path(), file.to_json().unwrap()).unwrap();

        assert_eq!(s.sync(2).outcome, Outcome::Rejected);
        assert!(s.conflicts().is_empty());
        assert!(!s.conflicts_file().exists());
        assert!(conflict_events(&s.core).is_empty());
        assert_eq!(fs::read(s.local_path()).unwrap(), local);
        assert!(
            s.folder.writes.borrow().is_empty(),
            "not pushed over it either"
        );
        // That write is not looked at again.
        s.core.lock(None);
        assert_eq!(s.sync(3).outcome, Outcome::Rejected);
        assert!(!s.conflicts_file().exists());
    }

    #[test]
    fn a_new_master_password_on_the_pc_against_phone_edits() {
        let mut s = two_sides();
        edit_on_phone(&s.core, "vom Handy");
        s.v.change_master_password("pc-pw", "neu").unwrap();
        s.pc.mirror(&s.v);
        let result = s.sync(2);
        assert_eq!(result.outcome, Outcome::Conflict);
        assert!(result.password_changed);
        let conflicts = s.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].password_changed);

        // The side with the old password never wins.
        assert_eq!(
            s.resolve(Keep::Local).unwrap_err().code(),
            "unsupported:sync_conflict_password"
        );
        assert!(sync_conflict::list(&s.core.data_dir(), None)
            .unwrap()
            .is_empty());
        let local = fs::read(s.local_path()).unwrap();
        // The PC's copy: stored on trial, the vault locks (`keyChanged`).
        s.resolve(Keep::Remote).unwrap();
        assert!(s.core.state().vault.is_none());
        assert!(s.core.emitted().contains(&(
            EVENT_LOCKED.to_owned(),
            serde_json::json!({ "reason": "keyChanged" })
        )));
        let copies = sync_conflict::list(&s.core.data_dir(), Some(&s.id)).unwrap();
        assert_eq!(fs::read(&copies[0].path).unwrap(), local);
        assert_eq!(
            fs::read(s.local_path()).unwrap(),
            fs::read(s.remote_path()).unwrap()
        );
        assert!(on_trial(&s.core.data_dir(), &s.id));
        assert!(!s.conflicts_file().exists());
        let id = s.id.clone();
        unlock_copy(&s.core, &id, |st| st.unlock(&id, "neu")).unwrap();
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 1);
    }

    #[test]
    fn a_vault_made_on_the_phone_goes_to_the_folder_once_one_can_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let v = core
            .store()
            .create_vault_with_params("Handy", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let id = v.id().to_owned();
        // No folder yet: only on the phone.
        let local_only = local_only_results(&core);
        assert_eq!(local_only.len(), 1);
        assert_eq!(local_only[0].outcome, Outcome::LocalOnly);
        assert_eq!(local_only[0].vault_name.as_deref(), Some("Handy"));

        // A folder the app may only read (a 2a link): nothing written.
        folder.writable.set(false);
        let mut link = LinkState::new("content://tree/cloud".into(), "K".into(), 1, true);
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        assert!(!link.writable);
        assert!(!FolderStatus::of(Some(&link)).writable);
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::NotWritable);
        assert!(folder.writes.borrow().is_empty());
        assert_eq!(fs::read_dir(&pc.folder).unwrap().count(), 0);
        assert_eq!(base_head(&core, &link, &id), None);

        // Writable: pushed as a new vault.
        folder.writable.set(true);
        sync_folder(&core, &folder, &mut link, 2).unwrap();
        assert!(link.writable && FolderStatus::of(Some(&link)).writable);
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::Pushed);
        let remote = pc.folder.join(format!("{id}.keystead"));
        assert_eq!(fs::read(&remote).unwrap(), fs::read(v.path()).unwrap());
        assert_eq!(base_head(&core, &link, &id), Some(head_of_path(v.path())));
        // From now on it is a vault like any other.
        sync_folder(&core, &folder, &mut link, 3).unwrap();
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::UpToDate);
        assert_eq!(folder.writes.borrow().len(), 1);
    }

    #[test]
    fn a_read_only_link_reports_edits_and_writes_nothing() {
        let mut s = two_sides();
        s.folder.writable.set(false);
        let remote = fs::read(s.remote_path()).unwrap();
        edit_on_phone(&s.core, "vom Handy");
        assert_eq!(s.push(2).outcome, Outcome::NotWritable);
        assert_eq!(s.sync(3).outcome, Outcome::NotWritable);
        assert!(!s.link.writable);
        assert_eq!(fs::read(s.remote_path()).unwrap(), remote);
        assert!(s.folder.writes.borrow().is_empty());
        // Picked again with write permission: pushed.
        s.folder.writable.set(true);
        assert_eq!(s.sync(4).outcome, Outcome::Pushed);
    }

    #[test]
    fn a_vault_removed_from_the_folder_is_not_written_again() {
        let mut s = two_sides();
        edit_on_phone(&s.core, "vom Handy");
        fs::remove_file(s.remote_path()).unwrap();
        assert_eq!(s.push(2).outcome, Outcome::Missing);
        assert_eq!(s.sync(3).outcome, Outcome::Missing);
        assert!(s.folder.writes.borrow().is_empty());
        assert!(!s.remote_path().exists());
        // Also a vault of stage 2a without a base: the folder had it.
        fs::remove_file(sync_base::base_path(&s.core.data_dir(), &s.id)).unwrap();
        assert_eq!(s.sync(4).outcome, Outcome::Missing);
        assert!(!s.remote_path().exists());
        assert!(s.local_path().is_file(), "the phone keeps it");
    }

    #[test]
    fn a_write_that_does_not_read_back_is_retried_then_reported() {
        let mut s = two_sides();
        let base = base_head(&s.core, &s.link, &s.id);
        edit_on_phone(&s.core, "eins");
        // Mangled once: the second write is verified.
        s.folder.mangle.set(1);
        assert_eq!(s.push(2).outcome, Outcome::Pushed);
        assert_eq!(s.folder.writes.borrow().len(), 2);
        assert_eq!(
            fs::read(s.remote_path()).unwrap(),
            fs::read(s.local_path()).unwrap()
        );
        let pushed = base_head(&s.core, &s.link, &s.id);
        assert_ne!(pushed, base);

        // Mangled twice: reported, the base stays the last verified push.
        edit_on_phone(&s.core, "zwei");
        s.folder.mangle.set(2);
        let result = s.push(3);
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(
            result.error.as_deref(),
            Some("unsupported:sync_write_unverified")
        );
        assert_eq!(s.folder.writes.borrow().len(), 4);
        assert_eq!(base_head(&s.core, &s.link, &s.id), pushed);
    }

    #[test]
    fn the_bases_of_a_2a_link_are_seeded_once() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let v = pc.create("Privat");
        let w = pc.create("Arbeit");
        pc.mirror(&v);
        pc.mirror(&w);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        // As stage 2a left it: no bases, the link not seeded; for "Arbeit"
        // the phone's copy is another revision than the result says.
        for id in [v.id(), w.id()] {
            sync_base::remove(&core.data_dir(), id).unwrap();
        }
        link.bases_seeded = false;
        link.files
            .iter_mut()
            .find(|f| f.vault_id.as_deref() == Some(w.id()))
            .unwrap()
            .revision = Some(9);

        assert!(seed_bases(&core, &mut link));
        assert!(link.bases_seeded);
        assert_eq!(
            base_head(&core, &link, v.id()),
            Some(head_of_path(&core.store().vault_path(v.id())))
        );
        assert_eq!(base_head(&core, &link, w.id()), None);
        assert!(!seed_bases(&core, &mut link), "once");
        // A seeded vault's edit on the phone is a push, not a conflict.
        open_on_phone(&core, v.id()).unwrap();
        edit_on_phone(&core, "vom Handy");
        sync_vault(&core, &folder, &mut link, v.id(), 2).unwrap();
        assert_eq!(outcome_of(&link, v.id()).outcome, Outcome::Pushed);
        // A new link needs no seeding.
        let mut fresh = LinkState::new("t".into(), "n".into(), 1, true);
        assert!(!seed_bases(&core, &mut fresh));
    }

    #[test]
    fn attachments_the_pc_added_survive_a_phone_edit() {
        let dir = tempfile::tempdir().unwrap();
        let pc = Pc::new(dir.path());
        let mut v = pc.create("Privat");
        let item = v
            .save_item(VaultItem::new(ItemType::Login, "Bank"))
            .unwrap();
        let attachment = v
            .add_attachment(&item.id, "vertrag.pdf", b"%PDF-1.7")
            .unwrap();
        pc.mirror(&v);
        let folder = DirFolder::new(&pc.folder);
        let core = phone(dir.path());
        let mut link = link();
        sync_folder(&core, &folder, &mut link, 1).unwrap();
        let id = v.id().to_owned();
        open_on_phone(&core, &id).unwrap();
        edit_on_phone(&core, "vom Handy");
        sync_vault(&core, &folder, &mut link, &id, 2).unwrap();
        assert_eq!(outcome_of(&link, &id).outcome, Outcome::Pushed);

        // Only the vault file was written; the blob is still in the folder,
        // and the pushed copy still references it.
        assert_eq!(*folder.writes.borrow(), [format!("{id}.keystead")]);
        let blobs = pc
            .folder
            .join(attachments::dir_for(v.path()).file_name().unwrap());
        assert_eq!(fs::read_dir(&blobs).unwrap().count(), 1);
        let theirs = open_folder_copy(dir.path(), &pc.folder, &id, "pc-pw");
        assert_eq!(names_of(&theirs), ["Bank", "vom Handy"]);
        assert_eq!(
            &*theirs.read_attachment(&attachment.id).unwrap(),
            b"%PDF-1.7"
        );
        // The PC takes it and keeps its attachment.
        pc_takes_folder_copy(&pc, &mut v);
        assert_eq!(v.attachments_of(&item.id).len(), 1);
    }
}
