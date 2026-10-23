//! "Kopie im Cloud-Ordner aktuell halten" – the desktop's side of the
//! two-way cloud-folder sync with the phone (Block 5: stage 2a, decision
//! E7; two-way since stage 2b). See "Cloud-folder mirror (desktop) and
//! phone link" in docs/ARCHITECTURE.md.
//!
//! With `Settings.mirrorFolder` set (a folder the user picked, typically in
//! the Nextcloud or OneDrive folder), the app keeps a copy of every vault
//! opened in it there, in the layout the phone reads and writes
//! (`folder_sync`): `<id>.keystead` and, with attachments,
//! `<id>.keystead.attachments/<attachment id>.bin` – and takes what the
//! phone wrote back.
//!
//! * The copies are the encrypted files byte for byte – the vault file as
//!   it is on disk, the blobs as they are; nothing is decrypted or
//!   re-encrypted, so the folder never holds plaintext. Never a `.bak`,
//!   `.lock`, export or other file of the data folder; the only temporary
//!   files are `format::write_atomic`'s hidden `.<name>.<hex>.tmp`, which
//!   the phone ignores.
//! * What to do with a vault ([`mirror_vault`]) is
//!   `sync_base::decide_pair` (`keystead_core::sync::decide`) on the heads
//!   of both copies and the vault's **sync base** for this folder
//!   (`sync_base.rs`, `<data>/sync/base/<id>.json`: the last head the PC
//!   and the folder agreed on): only the PC's copy changed → written
//!   (push); only the folder's → taken ([`pull`]); both → a **conflict**
//!   for the user (`sync_conflict.rs`; nothing is merged, neither copy is
//!   overwritten); the folder's copy went back below the base →
//!   `unsupported:mirror_remote_older`; it was removed after it was synced
//!   → not written again (`unsupported:mirror_remote_gone`; setting the
//!   folder again writes it). The same write with a raised revision is
//!   pulled (and refused by the checks), a lowered one is older.
//! * First pass after the update (no base for this folder yet, both
//!   copies there and different): before stage 2b only the PC wrote the
//!   folder, so the older copy is what both came from – it serves as the
//!   base (a newer PC copy is pushed, a newer folder copy pulled); the
//!   same revision with other contents is a conflict. A folder copy
//!   overwritten that way is kept as a conflict copy, in case it was
//!   another device's.
//! * A pull ([`pull`]): the folder's copy (at most
//!   `sync::VAULT_FILE_MAX_BYTES`, parsed and validated – KDF bounds
//!   included) and its blobs the PC lacks (blobs first), then under the
//!   state lock, the trial lock and the vault file's lock: the open vault's
//!   keys check it (`UnlockedVault::check_copy`) – a forged copy is
//!   rejected and overwritten with the PC's, one of another master
//!   password is not taken (`unsupported:mirror_other_password`), a genuine
//!   one replaces the file and the session is reloaded (`vault://changed`).
//!   A locked vault takes it on trial (`sync_trial.rs`: the copy that
//!   opened here is kept until the new one opens) – unless its KDF salt or
//!   parameters differ (another master password: not taken either).
//! * Order of a push: read the vault file, then list its blobs – every
//!   blob the read revision references exists at that point – copy the
//!   blobs the folder lacks (or whose size differs; a blob never changes
//!   under its id), read the folder's copy again (a copy that changed since
//!   the decision is left for the next pass), write the vault file, and
//!   only then remove the folder's blobs the data folder no longer has. The
//!   phone reads the vault file first and its blob folder afterwards, so it
//!   never gets a revision without the blobs it references.
//! * When: after every successful change of the open vault
//!   (`Core::mutate_with` – UI, browser extension, icon fetcher, import,
//!   master password, rename), when a vault is opened, when the monitor
//!   picks up a change another process made (the terminal UI), and when the
//!   folder is set. **Scans** ([`Job::Scan`]: on unlock, when the main
//!   window gets the focus, when the folder is set, and every
//!   [`SCAN_INTERVAL`] while the app runs) list the folder's
//!   `<id>.keystead` files: a vault of the PC that has a base is synced
//!   (also while locked), a vault the PC does not have is **offered**
//!   ([`FolderOffer`], `mirror://offers`; "Neuer Tresor im Cloud-Ordner –
//!   übernehmen?") – never imported on its own: `adopt_folder_vault` takes
//!   it (on trial), `dismiss_folder_vault` hides it
//!   (`<data>/sync/dismissed.json`). Vaults never opened in this app are
//!   not copied. Deleting a vault removes its copy (and its base).
//! * Errors (folder missing – a cloud drive not mounted –, no permission,
//!   a full disk) never block saving: the passes run on a thread of their
//!   own; an error is logged once per distinct code and announced as
//!   `mirror://status` (the UI shows a toast once per new error), the next
//!   change or scan tries again – a failed removal of a deleted vault's
//!   copy too (kept until it succeeds, while the app runs).
//! * A folder inside the data directory, or one that looks like a
//!   Keystead vaults folder, is refused – when it is set
//!   ([`check_setting`]) and before every pass.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime};

use keystead_core::format::{self, VaultFile, VAULT_EXTENSION};
use keystead_core::model::{now_ms, VaultInfo};
use keystead_core::settings::Settings;
use keystead_core::sync::{self, SyncBase, SyncDecision, SyncHead, VAULT_FILE_MAX_BYTES};
use keystead_core::vault::CopyCheck;
use keystead_core::{attachments, paths, Error as CoreError};
use serde::Serialize;

use crate::attachments::{is_dialog_path, is_inside};
use crate::commands::{run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::state::{log, Core};
use crate::sync_base::{self, sync_dir};
use crate::sync_conflict::{
    self, conflict_changed, install_resolved, set_record, ConflictRecord, Keep, CONFLICT_PASSWORD,
};
use crate::sync_trial::{
    begin_trial, remember_rejected, trial_lock, FileVersion, Trial, COPY_REJECTED,
};

/// `mirror://status` – payload [`MirrorStatus`].
pub const EVENT_STATUS: &str = "mirror://status";
/// `mirror://offers` – payload [`FolderOffer`]`[]`: the vaults in the
/// folder the PC does not have, whenever that list changes.
pub const EVENT_OFFERS: &str = "mirror://offers";

/// `invalid_input:mirror_folder` – not an absolute path as the folder
/// dialog gives it.
pub const FOLDER_INVALID: &str = "mirror_folder";
/// `invalid_input:mirror_folder_data_dir` – inside the data directory, or
/// a Keystead vaults folder.
pub const FOLDER_DATA_DIR: &str = "mirror_folder_data_dir";
/// `unsupported:mirror_folder_missing` – the folder does not exist (any
/// more), e.g. a cloud or network drive that is not mounted.
pub const FOLDER_MISSING: &str = "mirror_folder_missing";
/// `unsupported:mirror_other_password` – the folder's newer copy has
/// another master password (another KDF salt or parameters, or the open
/// vault's keys do not open it): the PC does not take it.
pub const OTHER_PASSWORD: &str = "mirror_other_password";
/// `unsupported:mirror_remote_older` – the folder's copy went back below
/// the revision both sides last agreed on (a restored backup, a cloud app
/// that has not fetched the newest file yet): neither taken nor replaced.
pub const REMOTE_OLDER: &str = "mirror_remote_older";
/// `unsupported:mirror_remote_gone` – the folder's copy was removed after
/// it was synced: it is not written again (set the folder again for that).
pub const REMOTE_GONE: &str = "mirror_remote_gone";
/// `unsupported:folder_not_linked` – no mirror folder is set.
pub const NOT_LINKED: &str = "folder_not_linked";
/// `invalid_input:vault_exists` – `adopt_folder_vault` for a vault the PC
/// has.
pub const VAULT_EXISTS: &str = "vault_exists";

/// Largest blob copied: the largest attachment plus the blob's own bytes
/// (the phone refuses anything larger).
const BLOB_MAX_BYTES: u64 = attachments::ATTACHMENT_MAX_BYTES + attachments::BLOB_OVERHEAD as u64;
/// How long the app's exit waits for passes still queued or running.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);
/// Without other jobs, the worker scans the folder this often (files whose
/// size and time did not change since are not read again).
pub const SCAN_INTERVAL: Duration = Duration::from_secs(20);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn io_err(path: &Path, e: std::io::Error) -> AppError {
    AppError::io(format!("{}: {e}", path.display()))
}

fn not_found(what: &str) -> AppError {
    CoreError::NotFound(what.to_owned()).into()
}

// ---------------------------------------------------------------------------
// The folder
// ---------------------------------------------------------------------------

/// True if `folder` looks like the vaults folder of a Keystead data
/// directory (this one's or another installation's, e.g. a portable copy):
/// it is called `vaults` and has a `settings.json` beside it, or it holds a
/// vault backup (`*.keystead.bak`) or lock file – the mirror itself never
/// writes either. Copies written there would replace that installation's
/// working files.
fn looks_like_vaults_folder(folder: &Path) -> bool {
    let named_vaults = folder
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("vaults"));
    if named_vaults
        && folder
            .parent()
            .is_some_and(|parent| parent.join("settings.json").is_file())
    {
        return true;
    }
    let Ok(entries) = fs::read_dir(folder) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        name.ends_with(".keystead.bak") || name.ends_with(".keystead.lock")
    })
}

/// Checks the mirror folder against the data directory `data_dir`:
/// * not of the folder dialog's shape (relative, `.`/`..`) →
///   `invalid_input:mirror_folder`;
/// * inside the data directory (this store's or the process's default
///   one), or a Keystead vaults folder → `invalid_input:mirror_folder_data_dir`;
/// * missing or not a folder → `unsupported:mirror_folder_missing` (never
///   created: a missing cloud drive must not turn into a local folder).
pub fn check_folder(folder: &Path, data_dir: &Path) -> AppResult<()> {
    if !is_dialog_path(folder) {
        return Err(AppError::invalid(FOLDER_INVALID));
    }
    if is_inside(folder, data_dir)
        || is_inside(folder, &paths::data_dir())
        || looks_like_vaults_folder(folder)
    {
        return Err(AppError::invalid(FOLDER_DATA_DIR));
    }
    if !folder.is_dir() {
        return Err(AppError::unsupported(FOLDER_MISSING));
    }
    Ok(())
}

/// `save_settings`: a changed `mirrorFolder` must pass [`check_folder`]
/// before the settings are saved (an unchanged one is not checked again –
/// a cloud drive that is offline right now must not block saving the
/// theme).
pub fn check_setting(core: &Core, settings: &Settings) -> AppResult<()> {
    let (old, data_dir) = {
        let st = core.state();
        (
            st.settings.mirror_folder.clone(),
            st.store.root().to_path_buf(),
        )
    };
    match &settings.mirror_folder {
        Some(folder) if old.as_ref() != Some(folder) => check_folder(Path::new(folder), &data_dir),
        _ => Ok(()),
    }
}

/// `<folder>/<id>.keystead`, for a valid vault id only (ids become file
/// names; ours never have anything else, but a path is built from it).
fn copy_path(folder: &Path, vault_id: &str) -> AppResult<PathBuf> {
    let name = format!("{vault_id}.{VAULT_EXTENSION}");
    if sync::vault_id_of_file_name(&name) != Some(vault_id) {
        return Err(not_found(&format!("vault {vault_id}")));
    }
    Ok(folder.join(name))
}

/// The ids of the vault files (`<id>.keystead`, nothing hidden, no
/// folders) in `dir`, sorted.
fn vault_ids_in(dir: &Path) -> AppResult<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(dir, e)),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(sync::vault_id_of_file_name) else {
            continue;
        };
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

/// The blob files of an attachments folder: `<id>.bin` names only (no
/// temporary files, nothing else a user or a cloud app put there).
fn blob_names(dir: &Path) -> AppResult<BTreeSet<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(io_err(dir, e)),
    };
    let mut names = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if attachments::blob_id_of_file_name(&name).is_some()
            && entry.file_type().is_ok_and(|t| t.is_file())
        {
            names.insert(name);
        }
    }
    Ok(names)
}

/// Reads a blob, at most [`BLOB_MAX_BYTES`] (+1: a larger one is refused
/// by the caller's shape check).
fn read_blob(path: &Path) -> AppResult<Vec<u8>> {
    let file = fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut bytes = Vec::new();
    file.take(BLOB_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, e))?;
    Ok(bytes)
}

/// Copies the blobs of `from` that `to` lacks (or whose size differs – a
/// blob never changes under its id) into `to` – the data folder's into the
/// cloud folder before a push, the cloud folder's into the data folder
/// before a pull. Only `<id>.bin` files of a blob's shape and size (the
/// cloud folder is untrusted input; the key is in the vault, so nothing is
/// decrypted here); others are skipped. Returns the blob names of `from`
/// and how many were copied.
fn copy_blobs(from: &Path, to: &Path) -> AppResult<(BTreeSet<String>, u32)> {
    let names = blob_names(from)?;
    let mut copied = 0;
    for name in &names {
        let source = from.join(name);
        let target = to.join(name);
        if file_len(&target).is_some_and(|len| Some(len) == file_len(&source)) {
            continue;
        }
        if file_len(&source).is_some_and(|len| len > BLOB_MAX_BYTES) {
            log(format_args!("not copied: {name} is too large for a blob"));
            continue;
        }
        let blob = match read_blob(&source) {
            Ok(blob) => blob,
            // Removed meanwhile (an attachment removed by a later save,
            // which queues the next pass).
            Err(_) if !source.exists() => continue,
            Err(e) => return Err(e),
        };
        if blob.len() as u64 > BLOB_MAX_BYTES || !attachments::looks_like_blob(&blob) {
            log(format_args!("not copied: {name} is no attachment blob"));
            continue;
        }
        format::write_atomic(&target, &blob, false)?;
        copied += 1;
    }
    Ok((names, copied))
}

/// The folder's copy of a vault file as [`mirror_vault`] sees it.
enum FolderCopy {
    Missing,
    /// Larger than [`VAULT_FILE_MAX_BYTES`] (not read), or not readable.
    Unusable,
    Bytes(Vec<u8>),
}

/// Reads the folder's copy at `path` – untrusted input: one above
/// [`VAULT_FILE_MAX_BYTES`] is not read (its size says so first, so a cloud
/// drive's placeholder is not downloaded for it).
fn read_copy(path: &Path) -> FolderCopy {
    match fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FolderCopy::Missing,
        Ok(meta) if meta.is_file() && meta.len() <= VAULT_FILE_MAX_BYTES => {}
        _ => return FolderCopy::Unusable,
    }
    let Ok(file) = fs::File::open(path) else {
        return FolderCopy::Unusable;
    };
    let mut bytes = Vec::new();
    match file.take(VAULT_FILE_MAX_BYTES + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 <= VAULT_FILE_MAX_BYTES => FolderCopy::Bytes(bytes),
        _ => FolderCopy::Unusable,
    }
}

fn file_len(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

// ---------------------------------------------------------------------------
// One pass
// ---------------------------------------------------------------------------

/// What a pass did with the vault file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The PC's copy was written to the folder (it was missing, older,
    /// unreadable or forged there, or only the PC's copy changed).
    Written,
    /// Both copies are the same write (the base is recorded).
    UpToDate,
    /// The vault file does not exist (any more) – nothing to copy.
    Gone,
    /// The folder's copy replaced the PC's.
    Pulled,
    /// Both copies changed: recorded for the user, nothing written.
    Conflict,
    /// A copy changed while the pass looked (a save meanwhile, the phone
    /// wrote the folder): nothing written, the next pass looks again.
    Changed,
    /// The PC's copy came from the folder and has not opened here yet: it
    /// is never written back, and only a copy newer than the one that
    /// opened here last is taken.
    OnTrial,
}

/// The result of [`mirror_vault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub outcome: Outcome,
    /// Blobs copied into the folder (push) or into the data folder (pull).
    pub blobs_copied: u32,
    pub blobs_removed: u32,
}

impl Report {
    fn of(outcome: Outcome) -> Report {
        Report {
            outcome,
            blobs_copied: 0,
            blobs_removed: 0,
        }
    }
}

/// The PC's side of one pass over one vault.
struct Pass<'a> {
    core: &'a Core,
    data_dir: PathBuf,
    /// The folder as set (`Settings.mirrorFolder`): what bases are recorded
    /// for.
    folder: &'a str,
    id: &'a str,
    local_path: PathBuf,
    target: PathBuf,
    local_bytes: Vec<u8>,
    local: VaultFile,
    local_head: SyncHead,
}

/// The folder's copy as a pass found it.
enum Remote {
    Missing,
    /// Too large, unreadable, damaged, or another vault's file under this
    /// vault's name: replaced like a missing one.
    Garbage,
    Copy(Box<VaultFile>, Vec<u8>),
}

/// Syncs vault `vault_id` of the PC with its copy in `folder` (see the
/// module docs): writes the PC's copy, takes the folder's, records a
/// conflict or reports why neither happens (`unsupported:mirror_remote_older`,
/// `unsupported:mirror_remote_gone`, `unsupported:mirror_other_password`).
/// The open vault's session checks the folder's copy where that matters.
pub fn mirror_vault(core: &Core, folder: &str, vault_id: &str) -> AppResult<Report> {
    let data_dir = core.data_dir();
    let folder_path = Path::new(folder);
    let local_path = core.store().vault_path(vault_id);
    let target = copy_path(folder_path, vault_id)?;
    // 1. The vault file as it is on disk (saves replace it atomically, so
    //    this is one whole revision).
    let local_bytes = match fs::read(&local_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Report::of(Outcome::Gone)),
        Err(e) => return Err(io_err(&local_path, e)),
    };
    let local = VaultFile::parse(&local_bytes)?;
    if local.id != vault_id {
        return Err(CoreError::Corrupt("vault file of another vault".into()).into());
    }
    check_folder(folder_path, &data_dir)?;
    let local_head = sync::head_of(&local);
    let trial = {
        let _trial = trial_lock();
        Trial::load(&data_dir, vault_id)
    };
    let on_trial = trial.unconfirmed.as_ref() == Some(&FileVersion::of(&local));
    let remote = match read_copy(&target) {
        FolderCopy::Missing => Remote::Missing,
        FolderCopy::Unusable => Remote::Garbage,
        FolderCopy::Bytes(bytes) => match VaultFile::parse(&bytes) {
            Ok(file) if file.id == vault_id => Remote::Copy(Box::new(file), bytes),
            _ => Remote::Garbage,
        },
    };
    let pass = Pass {
        core,
        data_dir,
        folder,
        id: vault_id,
        local_path,
        target,
        local_bytes,
        local,
        local_head,
    };

    let (file, bytes) = match remote {
        // A copy that has not opened here is never written to the folder.
        Remote::Missing | Remote::Garbage if on_trial => return Ok(Report::of(Outcome::OnTrial)),
        Remote::Missing => {
            let base = sync_base::load(&pass.data_dir, vault_id, folder);
            return match sync::decide(Some(&pass.local_head), None, base.as_ref()) {
                SyncDecision::NewLocal => push(&pass, None, None),
                _ => Err(AppError::unsupported(REMOTE_GONE)),
            };
        }
        Remote::Garbage => {
            log("replacing a damaged copy in the cloud folder");
            return push(&pass, None, None);
        }
        Remote::Copy(file, bytes) => (*file, bytes),
    };
    let remote_head = sync::head_of(&file);
    // A write of the folder's file that did not open here (forged or
    // damaged): replaced with the PC's.
    if trial.rejected.as_ref() == Some(&FileVersion::of(&file)) {
        if on_trial {
            return Ok(Report::of(Outcome::OnTrial));
        }
        log("replacing a rejected copy in the cloud folder");
        return push(&pass, Some(&bytes), None);
    }

    let (decision, migrated) = if on_trial {
        // Its revision proves nothing yet: the folder's copy has to beat
        // the copy that opened here last. It has no edits of the PC's.
        let decision = if FileVersion::of(&file) == FileVersion::of(&pass.local) {
            SyncDecision::UpToDate
        } else if trial.kept_revision.is_none_or(|kept| file.revision > kept) {
            SyncDecision::PullRemote
        } else {
            return Ok(Report::of(Outcome::OnTrial));
        };
        (decision, false)
    } else {
        match sync_base::load(&pass.data_dir, vault_id, folder) {
            Some(base) => (
                sync_base::decide_pair(&pass.local_head, &remote_head, Some(&base)),
                false,
            ),
            None => {
                let base = first_base(&pass.local_head, &remote_head, folder);
                (
                    sync_base::decide_pair(&pass.local_head, &remote_head, base.as_ref()),
                    true,
                )
            }
        }
    };
    match decision {
        SyncDecision::UpToDate => up_to_date(&pass, &remote_head, on_trial),
        SyncDecision::PushLocal => {
            // Without a base, the folder's older copy may still be another
            // device's write – unless it is the PC's own previous one (its
            // `.bak`): kept as a conflict copy before it is replaced.
            let keep = migrated && !is_own_backup(&pass.local_path, &remote_head);
            push(&pass, Some(&bytes), keep.then_some(&bytes[..]))
        }
        SyncDecision::PullRemote => pull(&pass, &file, &bytes, &remote_head),
        SyncDecision::RemoteOlder => Err(AppError::unsupported(REMOTE_OLDER)),
        // Both copies are there: only `Diverged` is left.
        SyncDecision::Diverged
        | SyncDecision::OtherVault
        | SyncDecision::NewRemote
        | SyncDecision::NewLocal
        | SyncDecision::RemoteGone => conflict(&pass, &file, &bytes, &remote_head),
    }
}

/// The base of a vault the PC has never synced with this folder although
/// both copies are there (the first pass after the update to stage 2b, or
/// a folder that already held copies): before 2b only the PC wrote the
/// folder, so the older copy is the write both came from. The same
/// revision with other contents has none (a conflict).
fn first_base(local: &SyncHead, remote: &SyncHead, folder: &str) -> Option<SyncBase> {
    let older = match local.revision.cmp(&remote.revision) {
        std::cmp::Ordering::Less => local,
        std::cmp::Ordering::Greater => remote,
        std::cmp::Ordering::Equal => return None,
    };
    Some(SyncBase::new(older, folder, now_ms()))
}

/// Whether `head` is the write in the `.bak` beside the PC's vault file at
/// `path` (the revision before the current one).
fn is_own_backup(path: &Path, head: &SyncHead) -> bool {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    VaultFile::read(Path::new(&bak)).is_ok_and(|f| sync::head_of(&f) == *head)
}

/// Records `head` as the vault's base for the pass's folder (best effort).
fn record_base(pass: &Pass<'_>, head: &SyncHead) {
    let agreed = sync_base::load(&pass.data_dir, pass.id, pass.folder)
        .is_some_and(|b| b.revision == head.revision && b.digest == head.digest);
    if agreed {
        return;
    }
    let base = SyncBase::new(head, pass.folder, now_ms());
    if let Err(e) = sync_base::save(&pass.data_dir, pass.id, &base) {
        log(format_args!("could not record the sync base: {}", e.code()));
    }
}

/// Both copies are the same write: it is the base. The blobs the folder
/// lacks are copied (and ones the vault no longer has removed), as with a
/// push – not for a copy on trial.
fn up_to_date(pass: &Pass<'_>, remote_head: &SyncHead, on_trial: bool) -> AppResult<Report> {
    record_base(pass, remote_head);
    set_record(pass.core, pass.id, None);
    if on_trial {
        return Ok(Report::of(Outcome::UpToDate));
    }
    let (blobs_copied, blobs_removed) = sync_blobs_out(pass, || Ok(true))?;
    Ok(Report {
        outcome: Outcome::UpToDate,
        blobs_copied,
        blobs_removed,
    })
}

/// The blobs of the PC's copy into the folder, then `write` (the vault
/// file; `false`: nothing was written, no blob is removed), then the
/// folder's blobs the data folder no longer has are removed. Returns
/// (copied, removed).
fn sync_blobs_out(
    pass: &Pass<'_>,
    write: impl FnOnce() -> AppResult<bool>,
) -> AppResult<(u32, u32)> {
    let local_dir = attachments::dir_for(&pass.local_path);
    let mirror_dir = attachments::dir_for(&pass.target);
    // 2. The blobs – listed after reading the vault file, so all the ones
    //    it references are there – before the vault file.
    let (local_blobs, copied) = copy_blobs(&local_dir, &mirror_dir)?;
    // 3. The vault file.
    if !write()? {
        return Ok((copied, 0));
    }
    // 4. Blobs the data folder no longer has (their attachments are gone
    //    from the revision just written). Only `<id>.bin` files.
    let mut removed = 0;
    for name in blob_names(&mirror_dir)?.difference(&local_blobs) {
        let path = mirror_dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => log(format_args!("could not remove a mirrored blob: {e}")),
        }
    }
    if local_blobs.is_empty() {
        // Only succeeds if nothing else is left in it.
        let _ = fs::remove_dir(&mirror_dir);
    }
    Ok((copied, removed))
}

/// Writes the PC's copy to the folder (blobs first) and records its head
/// as the base. `seen`: the folder's copy the decision was made on (`None`:
/// missing or unusable) – if it changed meanwhile (the phone wrote it),
/// nothing is written (`Changed`). `keep`: the folder's copy is kept as a
/// conflict copy before it is replaced.
fn push(pass: &Pass<'_>, seen: Option<&[u8]>, keep: Option<&[u8]>) -> AppResult<Report> {
    let mut changed = false;
    let (blobs_copied, blobs_removed) = sync_blobs_out(pass, || {
        let still = match (read_copy(&pass.target), seen) {
            (FolderCopy::Bytes(now), Some(seen)) => now == seen,
            (FolderCopy::Bytes(now), None) => {
                VaultFile::parse(&now).map_or(true, |f| f.id != pass.id)
            }
            (FolderCopy::Missing | FolderCopy::Unusable, Some(_)) => false,
            (FolderCopy::Missing | FolderCopy::Unusable, None) => true,
        };
        if !still {
            changed = true;
            return Ok(false);
        }
        if let Some(old) = keep {
            let at = now_ms();
            match sync_conflict::keep_conflict_copy(&pass.data_dir, pass.id, old, None, at) {
                Ok(path) => log(format_args!(
                    "kept the cloud folder's previous copy: {}",
                    path.display()
                )),
                Err(e) => return Err(e),
            }
        }
        format::write_atomic(&pass.target, &pass.local_bytes, false)?;
        Ok(true)
    })?;
    if changed {
        return Ok(Report::of(Outcome::Changed));
    }
    record_base(pass, &pass.local_head);
    set_record(pass.core, pass.id, None);
    Ok(Report {
        outcome: Outcome::Written,
        blobs_copied,
        blobs_removed,
    })
}

/// What [`install_pulled`] did.
enum Installed {
    Yes {
        reloaded: bool,
    },
    /// The open vault's keys prove the copy forged: not taken.
    Forged,
    /// The PC's copy changed since the decision (a save meanwhile).
    LocalChanged,
}

/// Takes the folder's copy `file` (`bytes`, head `remote_head`) in place of
/// the PC's: its blobs the PC lacks first, then [`install_pulled`]. A copy
/// the open vault proves forged is overwritten with the PC's.
fn pull(
    pass: &Pass<'_>,
    file: &VaultFile,
    bytes: &[u8],
    remote_head: &SyncHead,
) -> AppResult<Report> {
    let (_, blobs_copied) = copy_blobs(
        &attachments::dir_for(&pass.target),
        &attachments::dir_for(&pass.local_path),
    )?;
    match install_pulled(pass, file, bytes, remote_head)? {
        Installed::Yes { reloaded } => {
            if reloaded {
                pass.core.emit_changed();
            }
            set_record(pass.core, pass.id, None);
            Ok(Report {
                outcome: Outcome::Pulled,
                blobs_copied,
                blobs_removed: 0,
            })
        }
        Installed::Forged => {
            log("the cloud folder's copy is forged or damaged: replaced");
            push(pass, Some(bytes), None)
        }
        Installed::LocalChanged => Ok(Report::of(Outcome::Changed)),
    }
}

/// Replaces the PC's vault file with the folder's copy under the state
/// lock, the trial lock and the vault file's lock (no save of this app or
/// of the terminal UI interleaves) – if the PC's file is still the one the
/// decision was made on. The open vault checks it first and is reloaded;
/// a locked vault takes it on trial. Another master password is never
/// taken. The folder's head becomes the base.
fn install_pulled(
    pass: &Pass<'_>,
    file: &VaultFile,
    bytes: &[u8],
    remote_head: &SyncHead,
) -> AppResult<Installed> {
    let data_dir = &pass.data_dir;
    let mut st = pass.core.state();
    let _trial = trial_lock();
    let _file_lock = format::lock_vault_file(&pass.local_path)?;
    let on_disk = VaultFile::read(&pass.local_path)
        .ok()
        .map(|f| sync::head_of(&f));
    if on_disk.as_ref() != Some(&pass.local_head) {
        return Ok(Installed::LocalChanged);
    }
    let check = st
        .vault
        .as_ref()
        .filter(|vault| vault.id() == pass.id)
        .map(|vault| vault.check_copy(file));
    match check {
        Some(CopyCheck::Forged) => {
            remember_rejected(data_dir, pass.id, FileVersion::of(file));
            return Ok(Installed::Forged);
        }
        Some(CopyCheck::OtherPassword) => return Err(AppError::unsupported(OTHER_PASSWORD)),
        // Genuine: nothing to keep, nothing on trial.
        Some(CopyCheck::Opens) => Trial::default().save(data_dir, pass.id)?,
        None => {
            // Nothing proves a copy while the vault is locked: on trial –
            // but never one of another master password.
            if sync_conflict::password_changed(&pass.local, file) {
                return Err(AppError::unsupported(OTHER_PASSWORD));
            }
            begin_trial(data_dir, pass.id, &pass.local_path, file)?;
        }
    }
    format::write_atomic(&pass.local_path, bytes, true)?;
    record_base(pass, remote_head);
    let reloaded = match st.vault.as_mut() {
        Some(vault) if vault.id() == pass.id => match vault.reload_if_changed() {
            Ok(changed) => {
                if changed {
                    // Blobs the new revision no longer references (an hour old).
                    vault.sweep_attachments();
                }
                changed
            }
            Err(e) => {
                log(format_args!(
                    "could not reload the pulled vault: {}",
                    e.code()
                ));
                false
            }
        },
        _ => false,
    };
    Ok(Installed::Yes { reloaded })
}

/// Both copies changed: recorded for the user (`sync://conflict` once the
/// open vault's keys checked the folder's copy) – unless those keys prove
/// it forged: then it is rejected and overwritten with the PC's, as
/// before stage 2b.
fn conflict(
    pass: &Pass<'_>,
    file: &VaultFile,
    bytes: &[u8],
    remote_head: &SyncHead,
) -> AppResult<Report> {
    let (check, lost) = {
        let st = pass.core.state();
        match st.vault.as_ref().filter(|vault| vault.id() == pass.id) {
            Some(vault) => (
                Some(vault.check_copy(file)),
                vault.attachments_missing_in_copy(file),
            ),
            None => (None, None),
        }
    };
    if check == Some(CopyCheck::Forged) {
        {
            let _trial = trial_lock();
            remember_rejected(&pass.data_dir, pass.id, FileVersion::of(file));
        }
        log("the cloud folder's copy of a vault in conflict is forged or damaged: replaced");
        return push(pass, Some(bytes), None);
    }
    sync_conflict::record_conflict(
        pass.core,
        pass.id,
        (&pass.local, &pass.local_head),
        (file, remote_head),
        check.is_some(),
        lost.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
    );
    Ok(Report::of(Outcome::Conflict))
}

/// Removes the copy of the deleted vault `vault_id` from `folder`: its
/// vault file (only if it is that vault's) and its blobs. Nothing happens
/// while the vault still exists in `data_dir`. Returns whether a vault
/// file was removed.
pub fn remove_vault(data_dir: &Path, folder: &Path, vault_id: &str) -> AppResult<bool> {
    let target = copy_path(folder, vault_id)?;
    if keystead_core::VaultStore::new(data_dir)
        .vault_path(vault_id)
        .exists()
    {
        return Ok(false);
    }
    check_folder(folder, data_dir)?;
    // Only a copy whose header says it is that vault's (one too large to
    // read is left, like a damaged one).
    let removed = match read_copy(&target) {
        FolderCopy::Bytes(bytes) if VaultFile::parse(&bytes).is_ok_and(|c| c.id == vault_id) => {
            fs::remove_file(&target).map_err(|e| io_err(&target, e))?;
            true
        }
        _ => false,
    };
    let mirror_dir = attachments::dir_for(&target);
    for name in blob_names(&mirror_dir)? {
        let path = mirror_dir.join(&name);
        if let Err(e) = fs::remove_file(&path) {
            log(format_args!("could not remove a mirrored blob: {e}"));
        }
    }
    let _ = fs::remove_dir(&mirror_dir);
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Scans and offers
// ---------------------------------------------------------------------------

/// A vault in the folder the PC does not have (e.g. created on the phone):
/// offered, never imported on its own (`folder_offers`, `mirror://offers`).
/// From its plain header – no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderOffer {
    pub vault_id: String,
    pub name: String,
    pub revision: u64,
    pub updated_at: i64,
}

/// Size and modification time of a file (`None`: missing).
type Stamp = Option<(u64, Option<SystemTime>)>;

fn stamp(path: &Path) -> Stamp {
    fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| (m.len(), m.modified().ok()))
}

/// What the last scan found for one file of the folder, while neither it
/// nor the PC's copy changed (size and time).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    remote: Stamp,
    local: Stamp,
    offer: Option<FolderOffer>,
}

/// `<data>/sync/dismissed.json`: the ids of folder vaults the user does not
/// want offered (a JSON array, sorted).
fn dismissed_path(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("dismissed.json")
}

/// Read leniently: missing or damaged → none.
fn load_dismissed(data_dir: &Path) -> BTreeSet<String> {
    match fs::read(dismissed_path(data_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log(format_args!(
                "the dismissed folder vaults are unreadable: {e}"
            ));
            BTreeSet::new()
        }),
        Err(_) => BTreeSet::new(),
    }
}

fn save_dismissed(data_dir: &Path, ids: &BTreeSet<String>) -> AppResult<()> {
    let path = dismissed_path(data_dir);
    if ids.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    fs::create_dir_all(sync_dir(data_dir))?;
    let mut bytes = serde_json::to_vec_pretty(ids).map_err(CoreError::from)?;
    bytes.push(b'\n');
    format::write_atomic(&path, &bytes, false)?;
    Ok(())
}

/// The offer for the folder's copy at `path` of vault `id`: its header,
/// if it is a readable copy of that vault.
fn read_offer(path: &Path, id: &str) -> Option<FolderOffer> {
    let FolderCopy::Bytes(bytes) = read_copy(path) else {
        return None;
    };
    let file = VaultFile::parse(&bytes).ok().filter(|f| f.id == id)?;
    Some(FolderOffer {
        vault_id: file.id,
        name: file.name,
        revision: file.revision,
        updated_at: file.updated_at,
    })
}

/// Lists the folder's vault files: the PC's vaults that were synced with
/// this folder before (they have a base) are synced (also while locked –
/// a pull then stores the copy on trial); the others are left to the next
/// opening of that vault. Vaults the PC does not have are offered unless
/// dismissed (or deleted here with a removal still pending). `cached`:
/// files whose size and time – and those of the PC's copy – did not change
/// since the last scan are not read again (the periodic scan).
fn scan(core: &Core, folder: &str, cached: bool, results: &mut Results) {
    let tracker = core.mirror();
    let data_dir = core.data_dir();
    let folder_path = Path::new(folder);
    if let Err(e) = check_folder(folder_path, &data_dir) {
        results.add(Err(e));
        return;
    }
    let ids = match vault_ids_in(folder_path) {
        Ok(ids) => ids,
        Err(e) => {
            results.add(Err(e));
            return;
        }
    };
    let store = core.store();
    let dismissed = load_dismissed(&data_dir);
    let pending_removals = lock(&tracker.failed_removals).clone();
    let previous = if cached {
        lock(&tracker.seen).clone()
    } else {
        BTreeMap::new()
    };
    let mut seen = BTreeMap::new();
    let mut offers = Vec::new();
    for id in ids {
        let Ok(remote_path) = copy_path(folder_path, &id) else {
            continue;
        };
        let local_path = store.vault_path(&id);
        let stamps = |offer: Option<FolderOffer>| Seen {
            remote: stamp(&remote_path),
            local: stamp(&local_path),
            offer,
        };
        let unchanged = previous
            .get(&id)
            .filter(|p: &&Seen| p.remote == stamp(&remote_path) && p.local == stamp(&local_path));
        if local_path.exists() {
            if sync_base::load(&data_dir, &id, folder).is_none() {
                continue;
            }
            if let Some(p) = unchanged {
                seen.insert(id, p.clone());
                continue;
            }
            let result = mirror_vault(core, folder, &id);
            if result.as_ref().is_ok_and(|r| r.outcome != Outcome::Changed) {
                seen.insert(id, stamps(None));
            }
            results.add(result);
            continue;
        }
        if dismissed.contains(&id) || pending_removals.contains(&id) {
            continue;
        }
        let offer = match unchanged {
            Some(p) => p.offer.clone(),
            None => read_offer(&remote_path, &id),
        };
        if let Some(offer) = offer {
            seen.insert(id, stamps(Some(offer.clone())));
            offers.push(offer);
        }
    }
    *lock(&tracker.seen) = seen;
    tracker.set_offers(core, offers);
}

/// `adopt_folder_vault`: takes vault `vault_id` from the folder – it must
/// be one the PC does not have (`invalid_input:vault_exists`) – as a copy
/// on trial without a kept copy (it opens with its own master password,
/// or is removed if it does not although its key does): its blobs first,
/// then the vault file; its head becomes the base. Returns its info.
pub fn adopt(core: &Core, vault_id: &str) -> AppResult<VaultInfo> {
    if !sync_base::valid_id(vault_id) {
        return Err(not_found(&format!("vault {vault_id}")));
    }
    let folder = core
        .state()
        .settings
        .mirror_folder
        .clone()
        .ok_or_else(|| AppError::unsupported(NOT_LINKED))?;
    let tracker = core.mirror();
    let _pass = lock(&tracker.pass);
    let data_dir = core.data_dir();
    let folder_path = Path::new(&folder);
    check_folder(folder_path, &data_dir)?;
    let store = core.store();
    let path = store.vault_path(vault_id);
    if path.exists() {
        return Err(AppError::invalid(VAULT_EXISTS));
    }
    let target = copy_path(folder_path, vault_id)?;
    let FolderCopy::Bytes(bytes) = read_copy(&target) else {
        return Err(not_found(&format!("vault {vault_id}")));
    };
    let file = VaultFile::parse(&bytes)?;
    if file.id != vault_id {
        return Err(not_found(&format!("vault {vault_id}")));
    }
    copy_blobs(&attachments::dir_for(&target), &attachments::dir_for(&path))?;
    {
        let _st = core.state();
        let _trial = trial_lock();
        if path.exists() {
            return Err(AppError::invalid(VAULT_EXISTS));
        }
        // Nothing to go back to: there was no copy here.
        begin_trial(&data_dir, vault_id, &path, &file)?;
        format::write_atomic(&path, &bytes, false)?;
        let base = SyncBase::new(&sync::head_of(&file), &folder, now_ms());
        if let Err(e) = sync_base::save(&data_dir, vault_id, &base) {
            log(format_args!("could not record the sync base: {}", e.code()));
        }
    }
    let mut dismissed = load_dismissed(&data_dir);
    if dismissed.remove(vault_id) {
        if let Err(e) = save_dismissed(&data_dir, &dismissed) {
            log(format_args!(
                "could not save the dismissed vaults: {}",
                e.code()
            ));
        }
    }
    tracker.drop_offer(core, vault_id);
    store
        .list_vaults()?
        .into_iter()
        .find(|v| v.id == vault_id)
        .ok_or_else(|| not_found(&format!("vault {vault_id}")))
}

/// `dismiss_folder_vault`: vault `vault_id` of the folder is not offered
/// any more (`<data>/sync/dismissed.json`; `adopt_folder_vault` still takes
/// it).
pub fn dismiss(core: &Core, vault_id: &str) -> AppResult<()> {
    if !sync_base::valid_id(vault_id) {
        return Err(not_found(&format!("vault {vault_id}")));
    }
    let data_dir = core.data_dir();
    let mut dismissed = load_dismissed(&data_dir);
    if dismissed.insert(vault_id.to_owned()) {
        save_dismissed(&data_dir, &dismissed)?;
    }
    core.mirror().drop_offer(core, vault_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolving a conflict
// ---------------------------------------------------------------------------

/// `resolve_sync_conflict` on the PC: resolves the recorded conflict of
/// vault `vault_id` – which must be the open vault and the page's
/// (`locked` otherwise) – by keeping one side; the other side's vault file
/// and blob folder are kept as a conflict copy in `<data>/conflicts/`
/// (`sync_conflict::keep_conflict_copy`; never in the folder). Both copies
/// must still be the ones the conflict is about
/// (`unsupported:sync_conflict_changed` otherwise; the vault is looked at
/// again), and the open vault's keys must not prove the folder's copy
/// forged (`unsupported:sync_copy_rejected`; it is replaced with the PC's).
///
/// * [`Keep::Remote`]: the folder's copy (blobs first) replaces the PC's and
///   the open session is replaced by it; one with another master password
///   is stored on trial and the vault locks (`keyChanged`).
/// * [`Keep::Local`]: refused for a folder copy with another master
///   password (`unsupported:sync_conflict_password`); the open vault is
///   saved again as a revision above the folder's (`save_superseding`), the
///   folder's head becomes the base and the vault is written to the folder.
pub fn resolve_conflict(
    core: &Core,
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
    let record = sync_conflict::verified_record(&core.data_dir(), vault_id)?;
    if keep == Keep::Local && record.conflict.password_changed {
        return Err(AppError::unsupported(CONFLICT_PASSWORD));
    }
    let folder = core
        .state()
        .settings
        .mirror_folder
        .clone()
        .ok_or_else(|| AppError::unsupported(NOT_LINKED))?;
    let tracker = core.mirror();
    let result = {
        let _pass = lock(&tracker.pass);
        resolve_in(core, &folder, &record, keep, now)
    };
    if let Err(e) = &result {
        let code = e.code();
        if code == conflict_changed().code() || code == AppError::unsupported(COPY_REJECTED).code()
        {
            // Look again: the conflict is recorded anew or gone; a forged
            // copy is replaced.
            tracker.request(Job::Vault(vault_id.to_owned()));
        }
    }
    result
}

fn resolve_in(
    core: &Core,
    folder: &str,
    record: &ConflictRecord,
    keep: Keep,
    now: i64,
) -> AppResult<VaultInfo> {
    let id = record.conflict.vault_id.as_str();
    let data_dir = core.data_dir();
    let folder_path = Path::new(folder);
    check_folder(folder_path, &data_dir)?;
    let target = copy_path(folder_path, id)?;
    let FolderCopy::Bytes(remote_bytes) = read_copy(&target) else {
        return Err(conflict_changed());
    };
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
        st.vault
            .as_ref()
            .filter(|v| v.id() == id)
            .ok_or(AppError::Locked)?
            .check_copy(&remote)
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
            let remote_blobs = attachments::dir_for(&target);
            sync_conflict::keep_conflict_copy(
                &data_dir,
                id,
                &remote_bytes,
                Some(&remote_blobs),
                now,
            )?;
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
            // The folder's copy is what the PC's now supersedes: a push.
            sync_base::save(
                &data_dir,
                id,
                &SyncBase::new(&remote_head, folder, now_ms()),
            )?;
            set_record(core, id, None);
            match mirror_vault(core, folder, id) {
                Ok(report) if report.outcome == Outcome::Written => {}
                Ok(report) => {
                    log(format_args!(
                        "resolved conflict not pushed: {:?}",
                        report.outcome
                    ));
                    core.mirror().request(Job::Vault(id.to_owned()));
                }
                Err(e) => {
                    log(format_args!(
                        "could not push the resolved vault: {}",
                        e.code()
                    ));
                    core.mirror().request(Job::Vault(id.to_owned()));
                }
            }
            Ok(info)
        }
        Keep::Remote => {
            let local_blobs = attachments::dir_for(&local_path);
            sync_conflict::keep_conflict_copy(
                &data_dir,
                id,
                &local_bytes,
                Some(&local_blobs),
                now,
            )?;
            copy_blobs(&attachments::dir_for(&target), &local_blobs)?;
            let info = install_resolved(
                core,
                folder,
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
                    .ok_or_else(|| not_found(&format!("vault {id}"))),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The worker and its state
// ---------------------------------------------------------------------------

/// What the worker is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Sync this vault (after a change, on unlock).
    Vault(String),
    /// This vault was deleted: remove its copy.
    Removed(String),
    /// Look through the folder (unlock, window focus, folder set): every
    /// file is read again.
    Scan,
}

impl Job {
    fn vault_id(&self) -> Option<&str> {
        match self {
            Job::Vault(id) | Job::Removed(id) => Some(id),
            Job::Scan => None,
        }
    }
}

/// `mirror_status`, `mirror://status`. No secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorStatus {
    /// The folder (`Settings.mirrorFolder`); `null` = off.
    pub folder: Option<String>,
    /// When a pass last succeeded in this folder (ms; since the app started
    /// or the folder was set).
    pub last_mirrored_at: Option<i64>,
    /// When a pass last took the folder's copy of a vault (ms; likewise).
    pub last_pulled_at: Option<i64>,
    /// The error code of the last pass, `null` after a success.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Progress {
    last_mirrored_at: Option<i64>,
    last_pulled_at: Option<i64>,
    last_error: Option<String>,
}

/// What the passes of one round found.
#[derive(Debug, Default)]
struct Results {
    ok: bool,
    pulled: bool,
    error: Option<String>,
}

impl Results {
    fn add(&mut self, result: AppResult<Report>) {
        match result {
            Ok(report) => {
                self.ok |= report.outcome != Outcome::Gone;
                self.pulled |= report.outcome == Outcome::Pulled;
            }
            Err(e) => {
                self.error.get_or_insert(e.code());
            }
        }
    }
}

/// The mirror's part of [`Core`]: the worker's queue and what the last
/// passes found.
#[derive(Default)]
pub struct MirrorTracker {
    queue: Mutex<Option<Sender<Job>>>,
    progress: Mutex<Progress>,
    /// Held for each round of the worker and for the commands that change
    /// the folder or a vault from it (adopt, resolve): one at a time. Taken
    /// before the state lock.
    pass: Mutex<()>,
    /// Deleted vaults whose copy could not be removed yet: tried again with
    /// every pass (while the app runs).
    failed_removals: Mutex<BTreeSet<String>>,
    /// The vaults the folder offers ([`FolderOffer`]).
    offers: Mutex<Vec<FolderOffer>>,
    /// What the last scan found per file ([`Seen`]).
    seen: Mutex<BTreeMap<String, Seen>>,
    /// The folder was set (or turned off): the next round starts over.
    relinked: AtomicBool,
    /// The worker runs (`spawn`); `shutdown` waits for it.
    running: AtomicBool,
    /// Set by the worker once it has done its last job after `shutdown`.
    finished: Arc<(Mutex<bool>, Condvar)>,
    /// Unit tests: jobs asked for while no worker runs.
    #[cfg(test)]
    recorded: Mutex<Vec<Job>>,
}

impl MirrorTracker {
    /// Queues a job for the worker (never blocks; without a worker – unit
    /// tests – it is recorded).
    pub fn request(&self, job: Job) {
        let queue = lock(&self.queue);
        let Some(tx) = queue.as_ref() else {
            #[cfg(test)]
            lock(&self.recorded).push(job);
            return;
        };
        let _ = tx.send(job);
    }

    /// The jobs asked for so far without a worker (unit tests).
    #[cfg(test)]
    pub fn take_recorded(&self) -> Vec<Job> {
        std::mem::take(&mut *lock(&self.recorded))
    }

    fn reset(&self) {
        *lock(&self.progress) = Progress::default();
    }

    /// The status for `mirror_status` / `mirror://status`. Takes the state
    /// lock (never while `progress` is held).
    pub fn status(&self, core: &Core) -> MirrorStatus {
        let folder = core.state().settings.mirror_folder.clone();
        let progress = lock(&self.progress).clone();
        MirrorStatus {
            folder,
            last_mirrored_at: progress.last_mirrored_at,
            last_pulled_at: progress.last_pulled_at,
            last_error: progress.last_error,
        }
    }

    /// The vaults the folder offers (none while the mirror is off).
    pub fn offers(&self, core: &Core) -> Vec<FolderOffer> {
        if core.state().settings.mirror_folder.is_none() {
            return Vec::new();
        }
        lock(&self.offers).clone()
    }

    /// Replaces the offers; a change is announced as `mirror://offers`.
    fn set_offers(&self, core: &Core, offers: Vec<FolderOffer>) {
        let changed = {
            let mut current = lock(&self.offers);
            let changed = *current != offers;
            *current = offers;
            changed
        };
        if changed {
            core.emit(EVENT_OFFERS, self.offers(core));
        }
    }

    /// Takes vault `id` off the offers (adopted or dismissed).
    fn drop_offer(&self, core: &Core, id: &str) {
        let offers: Vec<FolderOffer> = lock(&self.offers)
            .iter()
            .filter(|o| o.vault_id != id)
            .cloned()
            .collect();
        lock(&self.seen).remove(id);
        self.set_offers(core, offers);
    }

    /// Records the results of the passes for `folder`; returns whether the
    /// status changed. A result for a folder that is no longer the
    /// setting's is dropped.
    fn record(&self, core: &Core, folder: &str, results: Results) -> bool {
        if core.state().settings.mirror_folder.as_deref() != Some(folder) {
            return false;
        }
        let mut progress = lock(&self.progress);
        let before = progress.clone();
        if let Some(code) = &results.error {
            if progress.last_error.as_ref() != Some(code) {
                log(format_args!(
                    "could not mirror into the cloud folder: {code}"
                ));
            }
        }
        let now = now_ms();
        if results.ok {
            progress.last_mirrored_at = Some(now);
        }
        if results.pulled {
            progress.last_pulled_at = Some(now);
        }
        if results.ok || results.error.is_some() {
            progress.last_error = results.error;
        }
        *progress != before
    }

    /// App exit: lets the worker finish what is queued and waits for it (at
    /// most [`SHUTDOWN_WAIT`] – a hanging network drive must not keep the
    /// app alive).
    pub fn shutdown(&self) {
        let queue = lock(&self.queue).take();
        drop(queue);
        if !self.running.load(Ordering::SeqCst) {
            return;
        }
        let (done, signal) = &*self.finished;
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        let mut finished = lock(done);
        while !*finished {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                log("the cloud-folder mirror did not finish before the exit");
                return;
            }
            finished = signal
                .wait_timeout(finished, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

/// Starts the worker thread (desktop setup).
pub fn spawn(core: &Arc<Core>) {
    let (tx, rx) = mpsc::channel();
    let tracker = core.mirror();
    *lock(&tracker.queue) = Some(tx);
    tracker.running.store(true, Ordering::SeqCst);
    let finished = Arc::clone(&tracker.finished);
    let weak = Arc::downgrade(core);
    let result = std::thread::Builder::new()
        .name("keystead-mirror".into())
        .spawn(move || {
            work(&weak, &rx);
            let (done, signal) = &*finished;
            *lock(done) = true;
            signal.notify_all();
        });
    if let Err(e) = result {
        log(format_args!("could not start the cloud-folder mirror: {e}"));
        *lock(&tracker.queue) = None;
        tracker.running.store(false, Ordering::SeqCst);
    }
}

fn work(core: &Weak<Core>, rx: &Receiver<Job>) {
    loop {
        let (jobs, periodic) = match rx.recv_timeout(SCAN_INTERVAL) {
            // Everything queued meanwhile goes into the same round.
            Ok(job) => (std::iter::once(job).chain(rx.try_iter()).collect(), false),
            Err(RecvTimeoutError::Timeout) => (Vec::new(), true),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Some(core) = core.upgrade() else {
            return;
        };
        if periodic {
            process_round(&core, Vec::new(), Some(true));
        } else {
            process(&core, jobs);
        }
    }
}

/// Runs `jobs` (the last one per vault; a scan after them) against the
/// folder of the settings and announces a changed status. Without a folder
/// nothing happens.
pub(crate) fn process(core: &Core, jobs: Vec<Job>) {
    process_round(core, jobs, None);
}

/// [`process`]; `scan`: `Some(cached)` scans in any case (the periodic
/// scan: `Some(true)`, files unchanged since the last scan are not read).
fn process_round(core: &Core, jobs: Vec<Job>, scan_anyway: Option<bool>) {
    let tracker = core.mirror();
    let _pass = lock(&tracker.pass);
    let (folder, data_dir) = {
        let st = core.state();
        (
            st.settings.mirror_folder.clone(),
            st.store.root().to_path_buf(),
        )
    };
    if tracker.relinked.swap(false, Ordering::SeqCst) {
        start_over(core, folder.as_deref());
    }
    let Some(folder) = folder else {
        return;
    };
    // Removals that failed before come first (the drive is back, the cloud
    // app let go of the file); the last job per vault wins.
    let retries: Vec<Job> = lock(&tracker.failed_removals)
        .iter()
        .map(|id| Job::Removed(id.clone()))
        .collect();
    let mut scan_cached = scan_anyway;
    let mut last: Vec<Job> = Vec::new();
    for job in retries.into_iter().chain(jobs) {
        if job == Job::Scan {
            scan_cached = Some(false);
            continue;
        }
        last.retain(|j| j.vault_id() != job.vault_id());
        last.push(job);
    }
    let mut results = Results::default();
    for job in &last {
        match job {
            Job::Vault(id) => results.add(mirror_vault(core, &folder, id)),
            Job::Removed(id) => {
                let removed = remove_vault(&data_dir, Path::new(&folder), id);
                let mut failed = lock(&tracker.failed_removals);
                if removed.is_ok() {
                    failed.remove(id);
                } else {
                    failed.insert(id.clone());
                }
                drop(failed);
                if removed.is_ok() {
                    // Nothing left to agree on.
                    if let Err(e) = sync_base::remove(&data_dir, id) {
                        log(format_args!("could not remove a sync base: {}", e.code()));
                    }
                    set_record(core, id, None);
                }
                results.add(removed.map(|_| Report::of(Outcome::Written)));
            }
            Job::Scan => {}
        }
    }
    if let Some(cached) = scan_cached {
        scan(core, &folder, cached, &mut results);
    }
    if tracker.record(core, &folder, results) {
        core.emit(EVENT_STATUS, tracker.status(core));
    }
}

/// The first round after the folder was set (or turned off): the
/// conflicts, offers and scan results of the previous one are forgotten;
/// in the new folder, a vault whose copy is missing although it has a base
/// there (it was removed) is written again – setting the folder is how the
/// user asks for that.
fn start_over(core: &Core, folder: Option<&str>) {
    let tracker = core.mirror();
    let data_dir = core.data_dir();
    if let Err(e) = sync_conflict::clear_conflicts(&data_dir) {
        log(format_args!("could not forget the conflicts: {}", e.code()));
    }
    sync_conflict::emit_conflicts(core);
    lock(&tracker.seen).clear();
    tracker.set_offers(core, Vec::new());
    let Some(folder) = folder else {
        return;
    };
    let Ok(local_ids) = vault_ids_in(&core.store().vaults_dir()) else {
        return;
    };
    for id in local_ids {
        let gone = copy_path(Path::new(folder), &id).is_ok_and(|p| !p.exists());
        if gone && sync_base::load(&data_dir, &id, folder).is_some() {
            if let Err(e) = sync_base::remove(&data_dir, &id) {
                log(format_args!("could not remove a sync base: {}", e.code()));
            }
        }
    }
}

/// `mirrorFolder` was changed (set, replaced or turned off): the status
/// starts over, the next round forgets the conflicts and offers of the
/// previous folder, and the open vault is synced with the new folder right
/// away, followed by a scan. The copies in a previous folder stay there;
/// removals that failed are tried again (in a folder without that copy
/// they have nothing to do).
pub fn folder_changed(core: &Core) {
    let tracker = core.mirror();
    tracker.reset();
    tracker.relinked.store(true, Ordering::SeqCst);
    let open = core.state().vault.as_ref().map(|v| v.id().to_owned());
    if let Some(id) = open {
        tracker.request(Job::Vault(id));
    }
    let retries: Vec<String> = lock(&tracker.failed_removals).iter().cloned().collect();
    for id in retries {
        tracker.request(Job::Removed(id));
    }
    tracker.request(Job::Scan);
    core.emit(EVENT_STATUS, tracker.status(core));
}

// ---------------------------------------------------------------------------
// Commands (desktop)
// ---------------------------------------------------------------------------

/// `mirror_status` → [`MirrorStatus`]. Polled: not user activity.
#[tauri::command]
pub async fn mirror_status(core: Shared<'_>) -> CmdResult<MirrorStatus> {
    run(&core, false, |c| Ok(c.mirror().status(c))).await
}

/// `folder_offers` → [`FolderOffer`]`[]`: the vaults in the folder the PC
/// does not have (as the last scan found them). No I/O; not user activity.
#[tauri::command]
pub async fn folder_offers(core: Shared<'_>) -> CmdResult<Vec<FolderOffer>> {
    run(&core, false, |c| Ok(c.mirror().offers(c))).await
}

/// `adopt_folder_vault(vaultId)` → `VaultInfo`: takes an offered vault
/// from the folder ([`adopt`]).
#[tauri::command]
pub async fn adopt_folder_vault(core: Shared<'_>, vault_id: String) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| adopt(c, &vault_id)).await
}

/// `dismiss_folder_vault(vaultId)`: the vault is not offered any more
/// ([`dismiss`]).
#[tauri::command]
pub async fn dismiss_folder_vault(core: Shared<'_>, vault_id: String) -> CmdResult<()> {
    run(&core, true, move |c| dismiss(c, &vault_id)).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keystead_core::model::{ItemType, VaultItem};
    use keystead_core::{KdfParams, UnlockedVault, VaultStore};

    use super::*;
    use crate::folder_sync::{self, LinkState, Outcome as PhoneOutcome, RemoteEntry, RemoteFolder};
    use crate::state::test_support::core_with_open_vault;
    use crate::state::EVENT_CHANGED;
    use crate::sync_conflict::EVENT_CONFLICT;
    use crate::sync_trial::kept_path;

    /// 2026-10-23 14:05:09 UTC.
    const AT: i64 = 1_792_764_309_000;

    /// A desktop core with an open vault (password `master`) and the mirror
    /// folder `<dir>/cloud` (created).
    fn pc(dir: &Path) -> (Arc<Core>, PathBuf) {
        let cloud = dir.join("cloud");
        fs::create_dir_all(&cloud).unwrap();
        let settings = Settings {
            mirror_folder: Some(cloud.to_string_lossy().into_owned()),
            ..Settings::default()
        };
        (core_with_open_vault(dir, settings), cloud)
    }

    fn folder_of(core: &Core) -> String {
        core.state().settings.mirror_folder.clone().unwrap()
    }

    /// One pass over vault `id` with the folder of the settings.
    fn pass(core: &Core, id: &str) -> AppResult<Report> {
        mirror_vault(core, &folder_of(core), id)
    }

    fn open_id(core: &Core) -> String {
        core.state().vault.as_ref().unwrap().id().to_owned()
    }

    fn vault_path(core: &Core) -> PathBuf {
        core.state().vault.as_ref().unwrap().path().to_path_buf()
    }

    fn vault_path_of(core: &Core, id: &str) -> PathBuf {
        core.store().vault_path(id)
    }

    /// `<path><suffix>` (`x.keystead` → `x.keystead.bak`).
    fn sibling(path: &Path, suffix: &str) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        PathBuf::from(s)
    }

    fn note(core: &Core, name: &str) -> String {
        core.mutate(|v| v.save_item(VaultItem::new(ItemType::Note, name)))
            .unwrap()
            .id
    }

    fn names(core: &Core) -> Vec<String> {
        let mut names: Vec<String> = core
            .read(|v| v.items().iter().map(|i| i.name.clone()).collect())
            .unwrap();
        names.sort();
        names
    }

    /// Runs the jobs the hooks queued (no worker in unit tests).
    fn run_queued(core: &Core) {
        let jobs = core.mirror().take_recorded();
        process(core, jobs);
    }

    fn scan_now(core: &Core) {
        process(core, vec![Job::Scan]);
    }

    /// Every file below `dir`, as relative paths with `/`.
    fn tree(dir: &Path) -> BTreeSet<String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.is_dir() {
                    out.insert(format!("{rel}/"));
                    walk(root, &path, out);
                } else {
                    out.insert(rel);
                }
            }
        }
        let mut out = BTreeSet::new();
        if dir.is_dir() {
            walk(dir, dir, &mut out);
        }
        out
    }

    fn events(core: &Core, name: &str) -> Vec<serde_json::Value> {
        core.emitted()
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, payload)| payload)
            .collect()
    }

    fn status_events(core: &Core) -> Vec<serde_json::Value> {
        events(core, EVENT_STATUS)
    }

    fn head_of_path(path: &Path) -> SyncHead {
        sync::head_of(&VaultFile::read(path).unwrap())
    }

    fn base_of(core: &Core, id: &str) -> Option<(u64, String)> {
        sync_base::load(&core.data_dir(), id, &folder_of(core)).map(|b| (b.revision, b.digest))
    }

    fn pair(head: &SyncHead) -> Option<(u64, String)> {
        Some((head.revision, head.digest.clone()))
    }

    /// Another device with the PC's vault (same master password): a copy of
    /// the PC's vault file in a store of its own, opened.
    fn other_device(dir: &Path, core: &Core, id: &str) -> UnlockedVault {
        let store = VaultStore::new(dir.join("other-device"));
        fs::create_dir_all(store.vaults_dir()).unwrap();
        fs::copy(vault_path_of(core, id), store.vault_path(id)).unwrap();
        store.unlock(id, "master").unwrap()
    }

    /// Writes `v`'s vault file and blobs into `folder` (blobs first), as
    /// the phone or another PC does.
    fn write_to_folder(v: &UnlockedVault, folder: &Path) {
        let blobs = attachments::dir_for(v.path());
        if blobs.is_dir() {
            let target = folder.join(blobs.file_name().unwrap());
            fs::create_dir_all(&target).unwrap();
            for blob in fs::read_dir(&blobs).unwrap() {
                let blob = blob.unwrap();
                fs::copy(blob.path(), target.join(blob.file_name())).unwrap();
            }
        }
        fs::copy(v.path(), folder.join(v.path().file_name().unwrap())).unwrap();
    }

    fn add_note(v: &mut UnlockedVault, name: &str) -> String {
        v.save_item(VaultItem::new(ItemType::Note, name))
            .unwrap()
            .id
    }

    // -----------------------------------------------------------------
    // Pushing (as before stage 2b)
    // -----------------------------------------------------------------

    #[test]
    fn a_save_mirrors_the_vault_file_and_its_blobs_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let item = note(&core, "Ausweis");
        // A second save leaves a `.bak` in the data folder; an attachment a blob.
        let attachment = core
            .mutate(|v| v.add_attachment(&item, "scan.png", b"\x89PNG fake scan"))
            .unwrap();
        let jobs = core.mirror().take_recorded();
        assert!(
            jobs.iter().all(|j| *j == Job::Vault(id.clone())),
            "{jobs:?}"
        );
        assert!(!jobs.is_empty());
        process(&core, jobs);

        let local = vault_path(&core);
        assert!(
            sibling(&local, ".bak").exists(),
            "the data folder has a backup"
        );
        let blob = format!("{}.bin", attachment.id);
        assert_eq!(
            tree(&cloud),
            BTreeSet::from([
                format!("{id}.keystead"),
                format!("{id}.keystead.attachments/"),
                format!("{id}.keystead.attachments/{blob}"),
            ])
        );
        // Byte for byte: the encrypted files, nothing decrypted.
        assert_eq!(
            fs::read(cloud.join(format!("{id}.keystead"))).unwrap(),
            fs::read(&local).unwrap()
        );
        assert_eq!(
            fs::read(cloud.join(format!("{id}.keystead.attachments/{blob}"))).unwrap(),
            fs::read(attachments::dir_for(&local).join(&blob)).unwrap()
        );
        let copy = fs::read_to_string(cloud.join(format!("{id}.keystead"))).unwrap();
        assert!(!copy.contains("Ausweis") && !copy.contains("scan.png"));
        // The written head is the base.
        assert_eq!(base_of(&core, &id), pair(&head_of_path(&local)));

        // The status says so, once.
        let events = status_events(&core);
        assert_eq!(events.len(), 1);
        assert!(events[0]["lastMirroredAt"].is_i64());
        assert!(events[0]["lastPulledAt"].is_null());
        assert!(events[0]["lastError"].is_null());
        assert_eq!(events[0]["folder"], cloud.to_string_lossy().as_ref());
    }

    #[test]
    fn unlocking_and_changes_ask_for_a_pass_and_equal_revisions_are_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        // Opening a vault (UI, extension, recovery key, a new vault): a
        // pass for it and a scan of the folder.
        let other = core
            .store()
            .create_vault_with_params("Arbeit", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let other_id = other.id().to_owned();
        core.install_vault(other).unwrap();
        assert_eq!(
            core.mirror().take_recorded(),
            vec![Job::Vault(other_id.clone()), Job::Scan]
        );
        core.lock(None);
        core.unlock(&id, "master").unwrap();
        assert_eq!(
            core.mirror().take_recorded(),
            vec![Job::Vault(id.clone()), Job::Scan]
        );

        let target = cloud.join(format!("{id}.keystead"));
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Written);
        // The same revision again: left alone (no upload by the cloud app).
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::UpToDate);
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
        // A new revision replaces it.
        note(&core, "Neu");
        run_queued(&core);
        let copy = VaultFile::read(&target).unwrap();
        assert_eq!(copy.revision, core.read(|v| v.revision()).unwrap());
        // Vaults never opened here are not copied.
        assert!(!cloud.join(format!("{other_id}.keystead")).exists());
    }

    #[test]
    fn removed_attachments_leave_the_folder_after_the_new_vault_file() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let item = note(&core, "Vertrag");
        let a = core
            .mutate(|v| v.add_attachment(&item, "a.pdf", b"%PDF a"))
            .unwrap();
        let b = core
            .mutate(|v| v.add_attachment(&item, "b.pdf", b"%PDF b"))
            .unwrap();
        run_queued(&core);
        let mirror_dir = cloud.join(format!("{id}.keystead.attachments"));
        // Something the user put there is not the mirror's business.
        fs::write(mirror_dir.join("notiz.txt"), b"mine").unwrap();
        assert_eq!(blob_names(&mirror_dir).unwrap().len(), 2);

        core.mutate(|v| v.remove_attachment(&a.id)).unwrap();
        run_queued(&core);
        assert_eq!(
            blob_names(&mirror_dir).unwrap(),
            BTreeSet::from([format!("{}.bin", b.id)])
        );
        assert!(mirror_dir.join("notiz.txt").exists());

        // The last one goes too; the folder stays for the user's file.
        core.mutate(|v| v.remove_attachment(&b.id)).unwrap();
        run_queued(&core);
        assert!(blob_names(&mirror_dir).unwrap().is_empty());
        assert!(mirror_dir.join("notiz.txt").exists());
        fs::remove_file(mirror_dir.join("notiz.txt")).unwrap();
        note(&core, "noch einer");
        run_queued(&core);
        assert!(!mirror_dir.exists(), "an empty blob folder is removed");
    }

    #[test]
    fn a_blob_that_cannot_be_written_keeps_the_old_vault_file() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        run_queued(&core);
        note(&core, "x");
        run_queued(&core);
        let target = cloud.join(format!("{id}.keystead"));
        let before = fs::read(&target).unwrap();
        // The blob folder's place is taken by a file: no blob can be stored.
        fs::write(
            cloud.join(format!("{id}.keystead.attachments")),
            b"in the way",
        )
        .unwrap();
        let item = note(&core, "Anhang");
        core.mutate(|v| v.add_attachment(&item, "a.txt", b"a"))
            .unwrap();
        run_queued(&core);
        assert_eq!(
            fs::read(&target).unwrap(),
            before,
            "vault file before its blobs"
        );
        let status = core.mirror().status(&core);
        assert!(status.last_error.as_deref().unwrap().starts_with("io:"));
    }

    #[test]
    fn a_missing_folder_is_reported_once_and_never_blocks_saving() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        fs::remove_dir_all(&cloud).unwrap();
        note(&core, "eins");
        run_queued(&core);
        note(&core, "zwei");
        run_queued(&core);
        scan_now(&core);
        // Saving went on; the folder was not created on its own.
        assert_eq!(core.read(|v| v.items().len()).unwrap(), 2);
        assert!(!cloud.exists());
        let events = status_events(&core);
        assert_eq!(
            events.len(),
            1,
            "one announcement per new error: {events:?}"
        );
        assert_eq!(events[0]["lastError"], "unsupported:mirror_folder_missing");
        assert!(events[0]["lastMirroredAt"].is_null());

        // The drive is back: the next change gets through, the error is gone.
        fs::create_dir_all(&cloud).unwrap();
        note(&core, "drei");
        run_queued(&core);
        let events = status_events(&core);
        assert_eq!(events.len(), 2);
        assert!(events[1]["lastError"].is_null());
        assert!(events[1]["lastMirroredAt"].is_i64());
        assert!(cloud.join(format!("{}.keystead", open_id(&core))).exists());
    }

    #[test]
    fn the_data_directory_and_vaults_folders_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (core, _cloud) = pc(dir.path());
        let data_dir = core.data_dir();
        let vaults = data_dir.join("vaults");
        let refused = |folder: &Path| check_folder(folder, &data_dir).unwrap_err().code();
        assert_eq!(refused(&data_dir), "invalid_input:mirror_folder_data_dir");
        assert_eq!(refused(&vaults), "invalid_input:mirror_folder_data_dir");
        assert_eq!(
            refused(&data_dir.join("sub").join("does-not-exist")),
            "invalid_input:mirror_folder_data_dir"
        );
        // Another installation's vaults folder (a portable copy).
        let other = dir.path().join("stick").join("Keystead-Data");
        fs::create_dir_all(other.join("vaults")).unwrap();
        fs::write(other.join("settings.json"), b"{}").unwrap();
        assert_eq!(
            refused(&other.join("vaults")),
            "invalid_input:mirror_folder_data_dir"
        );
        let with_backup = dir.path().join("copied-vaults");
        fs::create_dir_all(&with_backup).unwrap();
        fs::write(with_backup.join("x.keystead.bak"), b"{}").unwrap();
        assert_eq!(
            refused(&with_backup),
            "invalid_input:mirror_folder_data_dir"
        );
        // Not the dialog's shape; missing.
        assert_eq!(
            refused(Path::new("relative/cloud")),
            "invalid_input:mirror_folder"
        );
        assert_eq!(
            refused(&dir.path().join("cloud").join("..").join("cloud")),
            "invalid_input:mirror_folder"
        );
        assert_eq!(
            refused(&dir.path().join("nowhere")),
            "unsupported:mirror_folder_missing"
        );
        // A plain folder is fine.
        check_folder(&dir.path().join("cloud"), &data_dir).unwrap();

        // `save_settings` checks a changed folder before anything is saved …
        let into_vaults = Settings {
            mirror_folder: Some(vaults.to_string_lossy().into_owned()),
            ..core.state().settings.clone()
        };
        assert_eq!(
            check_setting(&core, &into_vaults).unwrap_err().code(),
            "invalid_input:mirror_folder_data_dir"
        );
        // … but not an unchanged one (an offline drive must not block the theme).
        fs::remove_dir_all(dir.path().join("cloud")).unwrap();
        let unchanged = core.state().settings.clone();
        check_setting(&core, &unchanged).unwrap();
        let off = Settings {
            mirror_folder: None,
            ..unchanged
        };
        check_setting(&core, &off).unwrap();

        // A pass never writes into the data directory, whatever the settings say.
        core.state().settings.mirror_folder = Some(vaults.to_string_lossy().into_owned());
        note(&core, "x");
        let before = tree(&vaults);
        run_queued(&core);
        scan_now(&core);
        assert_eq!(tree(&vaults), before);
        assert_eq!(
            core.mirror().status(&core).last_error.as_deref(),
            Some("invalid_input:mirror_folder_data_dir")
        );
    }

    #[test]
    fn an_oversized_copy_is_replaced_without_being_read() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let target = cloud.join(format!("{id}.keystead"));
        // A sparse file just above the limit (nothing to read on disk).
        let big = fs::File::create(&target).unwrap();
        big.set_len(VAULT_FILE_MAX_BYTES + 1).unwrap();
        drop(big);
        assert!(matches!(read_copy(&target), FolderCopy::Unusable));
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Written);
        assert_eq!(
            fs::read(&target).unwrap(),
            fs::read(vault_path(&core)).unwrap()
        );

        // A deleted vault's oversized copy is left alone (like a damaged one).
        let other = core
            .store()
            .create_vault_with_params("Arbeit", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let other_id = other.id().to_owned();
        drop(other);
        core.store().delete_vault(&other_id, "pw").unwrap();
        let other_target = cloud.join(format!("{other_id}.keystead"));
        let big = fs::File::create(&other_target).unwrap();
        big.set_len(VAULT_FILE_MAX_BYTES + 1).unwrap();
        drop(big);
        assert!(!remove_vault(&core.data_dir(), &cloud, &other_id).unwrap());
        assert!(other_target.exists());
        // … and never offered.
        scan_now(&core);
        assert!(core.mirror().offers(&core).is_empty());
    }

    #[test]
    fn a_failed_removal_is_tried_again_with_the_next_pass() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "x");
        run_queued(&core);
        let copy = cloud.join(format!("{id}.keystead"));
        assert!(copy.exists());
        assert!(base_of(&core, &id).is_some());
        // Another vault keeps the app busy afterwards.
        let other = core
            .store()
            .create_vault_with_params("Arbeit", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let other_id = other.id().to_owned();

        // The drive is gone while the vault is deleted: the removal fails.
        core.lock(None);
        core.store().delete_vault(&id, "master").unwrap();
        let away = dir.path().join("cloud-away");
        fs::rename(&cloud, &away).unwrap();
        process(&core, vec![Job::Removed(id.clone())]);
        assert_eq!(
            core.mirror().status(&core).last_error.as_deref(),
            Some("unsupported:mirror_folder_missing")
        );
        // Its copy is no offer meanwhile (the removal is pending).
        fs::rename(&away, &cloud).unwrap();
        process_round(&core, Vec::new(), Some(false));
        // (the round above already retried the removal)
        assert!(!copy.exists());
        assert!(core.mirror().offers(&core).is_empty());

        // The next pass – for another vault – has nothing left to do.
        core.install_vault(other).unwrap();
        run_queued(&core);
        assert!(!copy.exists(), "the deleted vault's copy is gone");
        assert!(cloud.join(format!("{other_id}.keystead")).exists());
        assert!(core.mirror().status(&core).last_error.is_none());
        assert!(lock(&core.mirror().failed_removals).is_empty());
        assert_eq!(base_of(&core, &id), None, "its base went with it");
    }

    #[test]
    fn deleting_a_vault_removes_its_copy_only() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let item = note(&core, "Anhang");
        core.mutate(|v| v.add_attachment(&item, "a.txt", b"a"))
            .unwrap();
        run_queued(&core);
        // Another vault's copy and a file of the user's.
        let other = core
            .store()
            .create_vault_with_params("Arbeit", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let other_id = other.id().to_owned();
        drop(other);
        pass(&core, &other_id).unwrap();
        fs::write(cloud.join("Einkaufsliste.txt"), b"Milch").unwrap();

        // Not while the vault still exists.
        assert!(!remove_vault(&core.data_dir(), &cloud, &id).unwrap());
        assert!(cloud.join(format!("{id}.keystead")).exists());

        core.lock(None);
        core.store().delete_vault(&id, "master").unwrap();
        process(
            &core,
            vec![Job::Vault(id.clone()), Job::Removed(id.clone())],
        );
        assert_eq!(
            tree(&cloud),
            BTreeSet::from([
                "Einkaufsliste.txt".to_owned(),
                format!("{other_id}.keystead"),
            ])
        );
        assert!(core.mirror().status(&core).last_error.is_none());
    }

    #[test]
    fn a_new_folder_starts_over_with_the_open_vault() {
        let dir = tempfile::tempdir().unwrap();
        let (core, _cloud) = pc(dir.path());
        let id = open_id(&core);
        core.mirror().take_recorded();
        core.state().settings.mirror_folder = None;
        process(&core, vec![Job::Vault(id.clone()), Job::Scan]);
        assert!(core.emitted().is_empty(), "off: nothing happens");

        let second = dir.path().join("OneDrive").join("Keystead");
        fs::create_dir_all(&second).unwrap();
        core.state().settings.mirror_folder = Some(second.to_string_lossy().into_owned());
        folder_changed(&core);
        assert_eq!(
            core.mirror().take_recorded(),
            vec![Job::Vault(id.clone()), Job::Scan]
        );
        let events = status_events(&core);
        assert_eq!(
            events.last().unwrap()["folder"],
            second.to_string_lossy().as_ref()
        );
        process(&core, vec![Job::Vault(id.clone()), Job::Scan]);
        assert!(second.join(format!("{id}.keystead")).exists());
    }

    #[test]
    fn a_scan_without_a_folder_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        // A vault of another device in the old folder.
        let store = VaultStore::new(dir.path().join("phone"));
        let v = store
            .create_vault_with_params("Handy", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        write_to_folder(&v, &cloud);
        core.state().settings.mirror_folder = None;
        scan_now(&core);
        process_round(&core, Vec::new(), Some(true));
        assert!(core.emitted().is_empty());
        assert!(core.mirror().offers(&core).is_empty());
        assert!(!sync_dir(&core.data_dir()).exists());
        assert_eq!(tree(&cloud).len(), 1, "nothing written there");
    }

    #[test]
    fn the_worker_copies_in_the_background_and_finishes_before_the_exit() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        spawn(&core);
        note(&core, "eins");
        // The exit waits for what is queued (bounded).
        core.shutdown();
        let copy = VaultFile::read(&cloud.join(format!("{id}.keystead"))).unwrap();
        assert_eq!(copy.revision, 2);
        assert!(
            core.mirror().take_recorded().is_empty(),
            "all went to the worker"
        );
        let events = status_events(&core);
        assert_eq!(events.len(), 1);
        assert!(events[0]["lastError"].is_null());
    }

    // -----------------------------------------------------------------
    // Pulling (stage 2b)
    // -----------------------------------------------------------------

    #[test]
    fn the_open_vault_takes_a_newer_copy_from_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "PC");
        run_queued(&core);
        // The phone (same master password) edits and adds nothing but an
        // item; its copy keeps the PC's blobs referenced.
        let mut phone = other_device(dir.path(), &core, &id);
        let item = add_note(&mut phone, "Handy");
        let attachment = phone.add_attachment(&item, "foto.jpg", b"JPEG").unwrap();
        write_to_folder(&phone, &cloud);
        let remote = cloud.join(format!("{id}.keystead"));
        let remote_head = head_of_path(&remote);

        scan_now(&core);
        assert_eq!(names(&core), ["Handy", "PC"]);
        assert_eq!(
            fs::read(vault_path(&core)).unwrap(),
            fs::read(&remote).unwrap()
        );
        // Its blob arrived first and opens with the session.
        assert_eq!(
            &*core
                .read(|v| v.read_attachment(&attachment.id))
                .unwrap()
                .unwrap(),
            b"JPEG"
        );
        // Genuine: nothing on trial; the folder's head is the base; the
        // previous revision is the `.bak`.
        assert_eq!(Trial::load(&core.data_dir(), &id), Trial::default());
        assert_eq!(base_of(&core, &id), pair(&remote_head));
        assert!(sibling(&vault_path(&core), ".bak").exists());
        assert_eq!(events(&core, EVENT_CHANGED).len(), 1);
        let status = core.mirror().status(&core);
        assert!(status.last_pulled_at.is_some() && status.last_error.is_none());
        // Nothing to do on the next scan, and the next save is pushed.
        scan_now(&core);
        assert_eq!(events(&core, EVENT_CHANGED).len(), 1);
        note(&core, "PC 2");
        run_queued(&core);
        assert_eq!(
            fs::read(&remote).unwrap(),
            fs::read(vault_path(&core)).unwrap()
        );
    }

    #[test]
    fn a_locked_vault_takes_a_newer_copy_on_trial() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "PC");
        run_queued(&core);
        let confirmed = fs::read(vault_path(&core)).unwrap();
        let path = vault_path(&core);
        core.lock(None);
        let mut phone = other_device(dir.path(), &core, &id);
        add_note(&mut phone, "Handy");
        write_to_folder(&phone, &cloud);

        scan_now(&core);
        let data_dir = core.data_dir();
        assert_eq!(fs::read(&path).unwrap(), fs::read(phone.path()).unwrap());
        // On trial: the copy that opened here is kept until this one opens.
        let trial = Trial::load(&data_dir, &id);
        assert_eq!(trial.unconfirmed.unwrap().revision, 3);
        assert_eq!(trial.kept_revision, Some(2));
        assert_eq!(fs::read(kept_path(&data_dir, &id)).unwrap(), confirmed);
        assert_eq!(base_of(&core, &id), pair(&head_of_path(phone.path())));
        // A copy on trial is never written to the folder.
        fs::remove_file(cloud.join(format!("{id}.keystead"))).unwrap();
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::OnTrial);
        assert!(!cloud.join(format!("{id}.keystead")).exists());
        write_to_folder(&phone, &cloud);

        // It opens: confirmed.
        core.unlock(&id, "master").unwrap();
        assert_eq!(names(&core), ["Handy", "PC"]);
        assert_eq!(Trial::load(&data_dir, &id), Trial::default());
        run_queued(&core);
        assert!(core.mirror().status(&core).last_error.is_none());
    }

    #[test]
    fn a_forged_copy_is_replaced_and_never_taken() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "eins");
        run_queued(&core);
        let target = cloud.join(format!("{id}.keystead"));
        // Someone with access to the folder raised the revision by hand:
        // the payload (its AAD binds the revision) no longer authenticates.
        let forge = |revision: u64| {
            let mut forged = VaultFile::read(&target).unwrap();
            forged.revision = revision;
            fs::write(&target, forged.to_json().unwrap()).unwrap();
        };
        forge(u64::MAX);
        // The open vault's keys prove it: replaced with the PC's copy.
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Written);
        assert_eq!(
            fs::read(&target).unwrap(),
            fs::read(vault_path(&core)).unwrap()
        );
        assert!(core.mirror().take_recorded().is_empty());

        // While locked nothing proves it: it is taken on trial – and
        // rejected when it does not open; the PC's copy replaces it then.
        let confirmed = fs::read(vault_path(&core)).unwrap();
        let path = vault_path(&core);
        core.lock(None);
        forge(999);
        scan_now(&core);
        assert_eq!(VaultFile::read(&path).unwrap().revision, 999);
        core.unlock(&id, "master").unwrap();
        assert_eq!(fs::read(&path).unwrap(), confirmed);
        run_queued(&core);
        assert_eq!(fs::read(&target).unwrap(), confirmed);
        assert!(core.mirror().status(&core).last_error.is_none());
        assert!(events(&core, EVENT_CONFLICT)
            .iter()
            .all(|e| e.as_array().unwrap().is_empty()));
    }

    #[test]
    fn a_copy_with_another_master_password_is_not_taken() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "eins");
        run_queued(&core);
        let local = fs::read(vault_path(&core)).unwrap();
        // Another PC changed the master password and saved.
        let mut other = other_device(dir.path(), &core, &id);
        other.change_master_password("master", "neu").unwrap();
        add_note(&mut other, "anderer PC");
        write_to_folder(&other, &cloud);

        // Open: the session's keys do not open it.
        assert_eq!(
            pass(&core, &id).unwrap_err().code(),
            "unsupported:mirror_other_password"
        );
        // Locked: its salt differs.
        let path = vault_path(&core);
        core.lock(None);
        scan_now(&core);
        assert_eq!(
            core.mirror().status(&core).last_error.as_deref(),
            Some("unsupported:mirror_other_password")
        );
        assert_eq!(fs::read(&path).unwrap(), local, "never pulled");
        assert_eq!(Trial::load(&core.data_dir(), &id), Trial::default());
        assert_eq!(
            fs::read(cloud.join(format!("{id}.keystead"))).unwrap(),
            fs::read(other.path()).unwrap(),
            "nor overwritten"
        );
    }

    #[test]
    fn a_folder_copy_that_went_back_or_away_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let target = cloud.join(format!("{id}.keystead"));
        note(&core, "eins");
        run_queued(&core);
        let older = fs::read(&target).unwrap();
        note(&core, "zwei");
        run_queued(&core);
        // The cloud app brought back the previous revision.
        fs::write(&target, &older).unwrap();
        assert_eq!(
            pass(&core, &id).unwrap_err().code(),
            "unsupported:mirror_remote_older"
        );
        assert_eq!(fs::read(&target).unwrap(), older, "not replaced");
        // Removed after it was synced: not written again …
        fs::remove_file(&target).unwrap();
        assert_eq!(
            pass(&core, &id).unwrap_err().code(),
            "unsupported:mirror_remote_gone"
        );
        assert!(!target.exists());
        // … until the folder is set again.
        folder_changed(&core);
        run_queued(&core);
        assert_eq!(
            fs::read(&target).unwrap(),
            fs::read(vault_path(&core)).unwrap()
        );
        assert!(core.mirror().status(&core).last_error.is_none());
    }

    #[test]
    fn the_first_pass_after_the_update_needs_no_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let data_dir = core.data_dir();
        let target = cloud.join(format!("{id}.keystead"));
        let path = vault_path(&core);
        // Stage 2a mirrored every save; there are no bases yet.
        let forget_base = || sync_base::remove(&data_dir, &id).unwrap();
        note(&core, "eins");
        run_queued(&core);
        forget_base();

        // The same file: its head becomes the base.
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::UpToDate);
        assert_eq!(base_of(&core, &id), pair(&head_of_path(&path)));

        // The folder has the PC's previous revision (its `.bak`): pushed.
        forget_base();
        let previous = fs::read(&target).unwrap();
        core.mirror().take_recorded();
        note(&core, "zwei");
        assert_eq!(
            fs::read(sibling(&path, ".bak")).unwrap(),
            previous,
            "the folder has the backup's revision"
        );
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Written);
        assert_eq!(fs::read(&target).unwrap(), fs::read(&path).unwrap());
        assert!(sync_conflict::list(&data_dir, Some(&id))
            .unwrap()
            .is_empty());

        // Older than that: pushed, the folder's copy kept just in case.
        forget_base();
        let two_back = fs::read(&target).unwrap();
        note(&core, "drei");
        note(&core, "vier");
        core.mirror().take_recorded();
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Written);
        let kept = sync_conflict::list(&data_dir, Some(&id)).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read(&kept[0].path).unwrap(), two_back);

        // Newer in the folder (the phone pushed first): pulled.
        forget_base();
        let mut phone = other_device(dir.path(), &core, &id);
        add_note(&mut phone, "Handy");
        write_to_folder(&phone, &cloud);
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Pulled);
        assert!(names(&core).contains(&"Handy".to_owned()));

        // The same revision with other contents: a conflict, nothing written.
        forget_base();
        add_note(&mut phone, "Handy 2");
        write_to_folder(&phone, &cloud);
        note(&core, "PC 5");
        core.mirror().take_recorded();
        let folder_copy = fs::read(&target).unwrap();
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Conflict);
        assert_eq!(fs::read(&target).unwrap(), folder_copy);
        assert_eq!(
            sync_conflict::conflicts_for_page(&core, Some(&id))
                .unwrap()
                .len(),
            1
        );
    }

    // -----------------------------------------------------------------
    // Offers
    // -----------------------------------------------------------------

    #[test]
    fn a_vault_from_the_folder_is_offered_dismissed_or_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let pc_id = open_id(&core);
        run_queued(&core);
        // The phone made a vault (with an attachment) and wrote it there.
        let phone_store = VaultStore::new(dir.path().join("phone"));
        let mut made = phone_store
            .create_vault_with_params("Familie", "handy-pw", KdfParams::insecure_for_tests())
            .unwrap();
        let item = add_note(&mut made, "WLAN");
        let attachment = made.add_attachment(&item, "router.png", b"PNG").unwrap();
        write_to_folder(&made, &cloud);
        let id = made.id().to_owned();
        // A cloud app's conflict copy is never offered.
        fs::copy(made.path(), cloud.join("Familie (1).keystead")).unwrap();

        scan_now(&core);
        let offer = serde_json::json!([{
            "vaultId": id,
            "name": "Familie",
            "revision": 3,
            "updatedAt": made.info().updated_at,
        }]);
        assert_eq!(events(&core, EVENT_OFFERS), std::slice::from_ref(&offer));
        assert_eq!(
            serde_json::to_value(core.mirror().offers(&core)).unwrap(),
            offer
        );
        // Never imported on its own.
        assert!(!vault_path_of(&core, &id).exists());
        // The same again: no new announcement (the periodic scan too).
        scan_now(&core);
        process_round(&core, Vec::new(), Some(true));
        assert_eq!(events(&core, EVENT_OFFERS).len(), 1);

        // Dismissed: gone, also after the next scan.
        dismiss(&core, &id).unwrap();
        assert_eq!(
            events(&core, EVENT_OFFERS).last().unwrap(),
            &serde_json::json!([])
        );
        let dismissed = sync_dir(&core.data_dir()).join("dismissed.json");
        let ids: Vec<String> = serde_json::from_slice(&fs::read(&dismissed).unwrap()).unwrap();
        assert_eq!(ids, std::slice::from_ref(&id));
        scan_now(&core);
        assert!(core.mirror().offers(&core).is_empty());

        // Adopted after all: blobs first, on trial without a kept copy,
        // the folder's head as the base.
        let info = adopt(&core, &id).unwrap();
        assert_eq!(info.name, "Familie");
        let path = vault_path_of(&core, &id);
        assert_eq!(fs::read(&path).unwrap(), fs::read(made.path()).unwrap());
        let trial = Trial::load(&core.data_dir(), &id);
        assert_eq!(trial.unconfirmed.unwrap().revision, 3);
        assert_eq!(trial.kept_revision, None);
        assert_eq!(base_of(&core, &id), pair(&head_of_path(made.path())));
        assert!(!dismissed.exists(), "no longer dismissed");
        assert_eq!(
            adopt(&core, &id).unwrap_err().code(),
            "invalid_input:vault_exists"
        );
        assert_eq!(adopt(&core, "unbekannt").unwrap_err().code(), "not_found");
        assert_eq!(adopt(&core, "../x").unwrap_err().code(), "not_found");
        scan_now(&core);
        assert!(core.mirror().offers(&core).is_empty());
        assert!(core.mirror().status(&core).last_error.is_none());

        // It opens with its own master password; its attachment is there.
        core.unlock(&id, "handy-pw").unwrap();
        assert_eq!(
            &*core
                .read(|v| v.read_attachment(&attachment.id))
                .unwrap()
                .unwrap(),
            b"PNG"
        );
        assert_eq!(Trial::load(&core.data_dir(), &id), Trial::default());
        // The PC's own vault is no offer; with the mirror off there are none.
        assert!(core
            .mirror()
            .offers(&core)
            .iter()
            .all(|o| o.vault_id != pc_id));
        core.state().settings.mirror_folder = None;
        assert!(core.mirror().offers(&core).is_empty());
        assert_eq!(
            adopt(&core, &id).unwrap_err().code(),
            "unsupported:folder_not_linked"
        );
    }

    // -----------------------------------------------------------------
    // Conflicts
    // -----------------------------------------------------------------

    /// Both sides changed since the last push: the phone wrote the folder,
    /// the PC added an attachment. Returns the phone's vault.
    fn conflict_setup(dir: &Path, core: &Core, cloud: &Path) -> UnlockedVault {
        let id = open_id(core);
        let item = note(core, "PC");
        run_queued(core);
        let mut phone = other_device(dir, core, &id);
        add_note(&mut phone, "Handy");
        write_to_folder(&phone, cloud);
        core.mutate(|v| v.add_attachment(&item, "nur-am-pc.pdf", b"%PDF"))
            .unwrap();
        run_queued(core);
        phone
    }

    #[test]
    fn both_changed_is_a_conflict_and_neither_copy_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let phone = conflict_setup(dir.path(), &core, &cloud);
        let target = cloud.join(format!("{id}.keystead"));
        assert_eq!(fs::read(&target).unwrap(), fs::read(phone.path()).unwrap());
        assert!(names(&core).contains(&"PC".to_owned()));
        assert!(!names(&core).contains(&"Handy".to_owned()));

        let conflicts = events(&core, EVENT_CONFLICT);
        let local = VaultFile::read(&vault_path(&core)).unwrap();
        let expected = serde_json::json!([{
            "vaultId": id,
            "vaultName": "Test",
            "localRevision": local.revision,
            "remoteRevision": phone.revision(),
            "localUpdatedAt": local.updated_at,
            "remoteUpdatedAt": phone.info().updated_at,
            "passwordChanged": false,
            "lostAttachments": 1,
        }]);
        assert_eq!(conflicts.last().unwrap(), &expected);
        assert_eq!(
            serde_json::to_value(sync_conflict::conflicts_for_page(&core, Some(&id)).unwrap())
                .unwrap(),
            expected
        );
        // Not an error of the mirror.
        assert!(core.mirror().status(&core).last_error.is_none());
        // Locked and found again: still shown after the next unlock.
        core.lock(None);
        scan_now(&core);
        core.unlock(&id, "master").unwrap();
        run_queued(&core);
        assert_eq!(
            sync_conflict::conflicts_for_page(&core, Some(&id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(fs::read(&target).unwrap(), fs::read(phone.path()).unwrap());
    }

    #[test]
    fn keeping_the_pc_state_supersedes_the_folder_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let phone = conflict_setup(dir.path(), &core, &cloud);
        let remote_bytes = fs::read(phone.path()).unwrap();
        let data_dir = core.data_dir();

        // Not the page's vault; an unknown keep value.
        assert_eq!(
            resolve_conflict(&core, &id, Some("other"), Keep::Local, AT)
                .unwrap_err()
                .code(),
            "locked"
        );
        let before = core.read(|v| v.revision()).unwrap();
        let info = resolve_conflict(&core, &id, Some(&id), Keep::Local, AT).unwrap();
        assert_eq!(info.id, id);
        // Above both revisions, written to the folder, the base is its head.
        let revision = core.read(|v| v.revision()).unwrap();
        assert_eq!(revision, before.max(phone.revision()) + 1);
        let path = vault_path(&core);
        let target = cloud.join(format!("{id}.keystead"));
        assert_eq!(fs::read(&target).unwrap(), fs::read(&path).unwrap());
        assert_eq!(base_of(&core, &id), pair(&head_of_path(&path)));
        assert!(!names(&core).contains(&"Handy".to_owned()));
        // The folder's copy (and its blob folder) is the conflict copy.
        let kept = sync_conflict::list(&data_dir, Some(&id)).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].path.file_name().unwrap().to_string_lossy(),
            format!("{id}.keystead.conflict-20261023-140509")
        );
        assert_eq!(fs::read(&kept[0].path).unwrap(), remote_bytes);
        // The conflict is gone.
        assert_eq!(
            events(&core, EVENT_CONFLICT).last().unwrap(),
            &serde_json::json!([])
        );
        assert_eq!(
            resolve_conflict(&core, &id, Some(&id), Keep::Local, AT)
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert!(core.mirror().take_recorded().is_empty());
    }

    #[test]
    fn keeping_the_folder_state_replaces_the_pc_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let phone = conflict_setup(dir.path(), &core, &cloud);
        let data_dir = core.data_dir();
        let path = vault_path(&core);
        let local_bytes = fs::read(&path).unwrap();
        let pc_blobs = blob_names(&attachments::dir_for(&path)).unwrap();
        assert_eq!(pc_blobs.len(), 1);

        let info = resolve_conflict(&core, &id, Some(&id), Keep::Remote, AT).unwrap();
        assert_eq!(info.id, id);
        assert_eq!(core.read(|v| v.revision()).unwrap(), phone.revision());
        assert_eq!(fs::read(&path).unwrap(), fs::read(phone.path()).unwrap());
        assert_eq!(names(&core), ["Handy", "PC"]);
        assert_eq!(base_of(&core, &id), pair(&head_of_path(phone.path())));
        // The PC's copy and its blobs are the conflict copy.
        let kept = sync_conflict::list(&data_dir, Some(&id)).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read(&kept[0].path).unwrap(), local_bytes);
        assert_eq!(
            blob_names(&attachments::dir_for(&kept[0].path)).unwrap(),
            pc_blobs
        );
        // In step with the folder now.
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::UpToDate);
        assert!(sync_conflict::conflicts_for_page(&core, Some(&id))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_resolution_needs_the_copies_it_was_about() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let mut phone = conflict_setup(dir.path(), &core, &cloud);
        // The phone wrote again meanwhile.
        add_note(&mut phone, "Handy 2");
        write_to_folder(&phone, &cloud);
        assert_eq!(
            resolve_conflict(&core, &id, Some(&id), Keep::Remote, AT)
                .unwrap_err()
                .code(),
            "unsupported:sync_conflict_changed"
        );
        // It is looked at again.
        assert_eq!(core.mirror().take_recorded(), vec![Job::Vault(id.clone())]);
        process(&core, vec![Job::Vault(id.clone())]);
        let conflicts = sync_conflict::conflicts_for_page(&core, Some(&id)).unwrap();
        assert_eq!(conflicts[0].remote_revision, phone.revision());
        // Nothing was kept or changed.
        assert!(sync_conflict::list(&core.data_dir(), Some(&id))
            .unwrap()
            .is_empty());
        resolve_conflict(&core, &id, Some(&id), Keep::Remote, AT).unwrap();
        assert!(names(&core).contains(&"Handy 2".to_owned()));
    }

    // -----------------------------------------------------------------
    // With the phone
    // -----------------------------------------------------------------

    /// The cloud folder as the phone's `folder_sync` sees it: a directory
    /// it may write.
    struct SharedDir(PathBuf);

    impl RemoteFolder for SharedDir {
        fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>> {
            let dir = parent.map_or(self.0.clone(), |p| PathBuf::from(&p.uri));
            let mut out = Vec::new();
            for entry in fs::read_dir(&dir)?.flatten() {
                let meta = entry.metadata()?;
                out.push(RemoteEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    uri: entry.path().to_string_lossy().into_owned(),
                    dir: meta.is_dir(),
                    size: meta.is_file().then_some(meta.len()),
                });
            }
            Ok(out)
        }
        fn copy_to(&self, entry: &RemoteEntry, target: &Path, _max: u64) -> AppResult<u64> {
            Ok(fs::copy(&entry.uri, target)?)
        }
        fn writable(&self) -> AppResult<bool> {
            Ok(true)
        }
        fn write_vault(&self, name: &str, bytes: &[u8]) -> AppResult<RemoteEntry> {
            let path = self.0.join(name);
            fs::write(&path, bytes)?;
            Ok(RemoteEntry {
                name: name.to_owned(),
                uri: path.to_string_lossy().into_owned(),
                dir: false,
                size: Some(bytes.len() as u64),
            })
        }
    }

    fn phone_core(dir: &Path) -> Arc<Core> {
        Core::for_tests(
            VaultStore::new(dir.join("phone")),
            Settings {
                browser_integration: false,
                ..Settings::default()
            },
        )
    }

    fn phone_note(phone: &Core, name: &str) {
        phone
            .mutate(|v| v.save_item(VaultItem::new(ItemType::Note, name)))
            .unwrap();
    }

    /// The phone's, the folder's and the PC's copy are the same write, and
    /// both sides' bases record it.
    fn assert_converged(core: &Core, phone: &Core, link: &LinkState, cloud: &Path, id: &str) {
        let pc_head = head_of_path(&vault_path_of(core, id));
        let folder_head = head_of_path(&cloud.join(format!("{id}.keystead")));
        let phone_head = head_of_path(&vault_path_of(phone, id));
        assert_eq!(pc_head, folder_head);
        assert_eq!(phone_head, folder_head);
        assert_eq!(base_of(core, id), pair(&folder_head));
        let phone_base = sync_base::load(&phone.data_dir(), id, &link.tree_uri).unwrap();
        assert_eq!(
            (phone_base.revision, phone_base.digest),
            (folder_head.revision, folder_head.digest)
        );
        assert_eq!(names(core), names(phone));
    }

    fn phone_outcome(link: &LinkState, id: &str) -> PhoneOutcome {
        link.files
            .iter()
            .find(|f| f.vault_id.as_deref() == Some(id))
            .unwrap()
            .outcome
    }

    #[test]
    fn edits_go_back_and_forth_with_the_phone() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "vom PC");
        run_queued(&core);
        let folder = SharedDir(cloud.clone());

        // The phone takes the PC's copy and opens it.
        let phone = phone_core(dir.path());
        let mut link = LinkState::new("content://tree/cloud".into(), "Keystead".into(), 1, true);
        folder_sync::sync_folder(&phone, &folder, &mut link, 2).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Imported);
        phone.unlock(&id, "master").unwrap();

        // The phone edits and pushes; the PC's scan takes it.
        phone_note(&phone, "vom Handy");
        folder_sync::sync_vault(&phone, &folder, &mut link, &id, 3).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Pushed);
        scan_now(&core);
        assert_eq!(names(&core), ["vom Handy", "vom PC"]);
        assert_converged(&core, &phone, &link, &cloud, &id);

        // The PC edits and mirrors; the phone takes it.
        note(&core, "wieder PC");
        run_queued(&core);
        folder_sync::sync_vault(&phone, &folder, &mut link, &id, 4).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Updated);
        assert_converged(&core, &phone, &link, &cloud, &id);
        assert!(core.mirror().status(&core).last_error.is_none());
    }

    #[test]
    fn both_edited_is_a_conflict_on_the_side_that_comes_second() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        assert_eq!(pass(&core, &id).unwrap().outcome, Outcome::Written);
        let folder = SharedDir(cloud.clone());
        let phone = phone_core(dir.path());
        let mut link = LinkState::new("content://tree/cloud".into(), "Keystead".into(), 1, true);
        folder_sync::sync_folder(&phone, &folder, &mut link, 2).unwrap();
        phone.unlock(&id, "master").unwrap();

        // Round 1: both edit, the PC writes first – the phone finds the
        // conflict and keeps its own state; the PC takes it.
        phone_note(&phone, "Handy 1");
        note(&core, "PC 1");
        run_queued(&core);
        folder_sync::sync_vault(&phone, &folder, &mut link, &id, 3).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Conflict);
        assert_eq!(
            sync_conflict::conflicts_for_page(&phone, Some(&id))
                .unwrap()
                .len(),
            1
        );
        folder_sync::resolve_conflict(&phone, &folder, &mut link, &id, Some(&id), Keep::Local, 4)
            .unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Pushed);
        scan_now(&core);
        assert_converged(&core, &phone, &link, &cloud, &id);
        assert_eq!(names(&core), ["Handy 1"]);
        assert!(sync_conflict::conflicts_for_page(&core, Some(&id))
            .unwrap()
            .is_empty());

        // Round 2: both edit, the phone writes first – the PC finds the
        // conflict and takes the phone's state.
        phone_note(&phone, "Handy 2");
        folder_sync::sync_vault(&phone, &folder, &mut link, &id, 5).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Pushed);
        note(&core, "PC 2");
        run_queued(&core);
        let conflicts = sync_conflict::conflicts_for_page(&core, Some(&id)).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].lost_attachments, 0);
        resolve_conflict(&core, &id, Some(&id), Keep::Remote, AT).unwrap();
        folder_sync::sync_vault(&phone, &folder, &mut link, &id, 6).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::UpToDate);
        assert_converged(&core, &phone, &link, &cloud, &id);
        assert_eq!(names(&core), ["Handy 1", "Handy 2"]);
        // Each side kept the state it gave up.
        assert_eq!(
            sync_conflict::list(&core.data_dir(), Some(&id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            sync_conflict::list(&phone.data_dir(), Some(&id))
                .unwrap()
                .len(),
            1
        );
        // From here on, edits flow again.
        note(&core, "PC 3");
        run_queued(&core);
        folder_sync::sync_vault(&phone, &folder, &mut link, &id, 7).unwrap();
        assert_eq!(phone_outcome(&link, &id), PhoneOutcome::Updated);
        assert_converged(&core, &phone, &link, &cloud, &id);
    }

    /// The phone's side of the contract with a read-only link (stage 2a):
    /// it takes what the mirror wrote – the vault with its attachment, then
    /// a newer revision.
    #[test]
    fn the_phone_takes_what_the_mirror_wrote() {
        struct ReadOnly(PathBuf);
        impl RemoteFolder for ReadOnly {
            fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>> {
                SharedDir(self.0.clone()).list(parent)
            }
            fn copy_to(&self, entry: &RemoteEntry, target: &Path, max: u64) -> AppResult<u64> {
                SharedDir(self.0.clone()).copy_to(entry, target, max)
            }
            fn writable(&self) -> AppResult<bool> {
                Ok(false)
            }
            fn write_vault(&self, _name: &str, _bytes: &[u8]) -> AppResult<RemoteEntry> {
                Err(AppError::unsupported(folder_sync::NOT_WRITABLE))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        let item = note(&core, "Ausweis");
        let attachment = core
            .mutate(|v| v.add_attachment(&item, "scan.png", b"\x89PNG scan"))
            .unwrap();
        run_queued(&core);

        let phone = phone_core(dir.path());
        let mut link = LinkState::new("content://tree".into(), "Keystead".into(), 1, false);
        assert!(folder_sync::sync_folder(&phone, &ReadOnly(cloud.clone()), &mut link, 2).unwrap());
        assert_eq!(link.files.len(), 1, "{:?}", link.files);
        assert_eq!(link.files[0].outcome, PhoneOutcome::Imported);
        assert_eq!(link.files[0].attachments_copied, 1);
        let vault = phone.store().unlock(&id, "master").unwrap();
        assert_eq!(
            &*vault.read_attachment(&attachment.id).unwrap(),
            b"\x89PNG scan"
        );

        note(&core, "Neu am PC");
        run_queued(&core);
        assert!(folder_sync::sync_folder(&phone, &ReadOnly(cloud), &mut link, 3).unwrap());
        assert_eq!(link.files[0].outcome, PhoneOutcome::Updated);
        let vault = phone.store().unlock(&id, "master").unwrap();
        assert_eq!(vault.items().len(), 2);
    }
}
