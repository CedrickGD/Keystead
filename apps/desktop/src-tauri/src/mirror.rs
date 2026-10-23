//! "Kopie im Cloud-Ordner aktuell halten" – the desktop's side of the
//! phone's cloud-folder sync (Block 5, stage 2a, decision E7). See
//! "Cloud-folder mirror (desktop) and phone link" in docs/ARCHITECTURE.md.
//!
//! With `Settings.mirrorFolder` set (a folder the user picked, typically in
//! the Nextcloud or OneDrive folder), the app keeps a copy of every vault
//! opened in it there, in the layout the phone reads
//! (`folder_sync`): `<id>.keystead` and, with attachments,
//! `<id>.keystead.attachments/<attachment id>.bin`.
//!
//! * The copies are the encrypted files byte for byte – the vault file as
//!   it is on disk, the blobs as they are; nothing is decrypted or
//!   re-encrypted, so the folder never holds plaintext. Never a `.bak`,
//!   `.lock`, export or other file of the data folder; the only temporary
//!   files are `format::write_atomic`'s hidden `.<name>.<hex>.tmp`, which
//!   the phone ignores.
//! * When: after every successful change of the open vault
//!   (`Core::mutate_with` – UI, browser extension, icon fetcher, import,
//!   master password, rename), once when a vault is opened, when the
//!   monitor picks up a change another process made (the terminal UI), and
//!   when the folder is set. Deleting a vault removes its copy. Vaults that
//!   are never opened in this app are not copied.
//! * Order of one pass ([`mirror_vault`]): read the vault file, then list
//!   its blobs – every blob the read revision references exists at that
//!   point – copy the blobs the folder lacks (or whose size differs; a blob
//!   never changes under its id), write the vault file, and only then
//!   remove the folder's blobs the data folder no longer has. The phone
//!   reads the vault file first and its blob folder afterwards, so it never
//!   gets a revision without the blobs it references.
//! * Revisions: the folder's copy is replaced by a higher revision only.
//!   The same write (equal bytes) is left alone (no needless upload); a
//!   higher revision in the folder (another PC, a restored backup here) is
//!   never overwritten: `unsupported:mirror_newer`, nor is another write of
//!   the same revision: `unsupported:mirror_diverged`. Except a copy the
//!   open vault's keys prove forged or damaged (`UnlockedVault::check_copy`
//!   – e.g. a revision raised by hand): it is replaced. The folder's copy is
//!   untrusted input – one above `sync::VAULT_FILE_MAX_BYTES` is replaced
//!   without being read.
//! * Errors (folder missing – a cloud drive not mounted –, no permission,
//!   a full disk) never block saving: the passes run on a thread of their
//!   own; an error is logged once per distinct code and announced as
//!   `mirror://status` (the UI shows a toast once per new error), the next
//!   change tries again – a failed removal of a deleted vault's copy too
//!   (kept until it succeeds, while the app runs).
//! * A folder inside the data directory, or one that looks like a
//!   Keystead vaults folder, is refused – when it is set
//!   ([`check_setting`]) and before every pass.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use keystead_core::format::{self, VaultFile, VAULT_EXTENSION};
use keystead_core::model::now_ms;
use keystead_core::settings::Settings;
use keystead_core::sync::VAULT_FILE_MAX_BYTES;
use keystead_core::vault::CopyCheck;
use keystead_core::{attachments, paths, sync, Error as CoreError, VaultStore};
use serde::Serialize;

use crate::attachments::{is_dialog_path, is_inside};
use crate::commands::{run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::state::{log, Core};

/// `mirror://status` – payload [`MirrorStatus`].
pub const EVENT_STATUS: &str = "mirror://status";

/// `invalid_input:mirror_folder` – not an absolute path as the folder
/// dialog gives it.
pub const FOLDER_INVALID: &str = "mirror_folder";
/// `invalid_input:mirror_folder_data_dir` – inside the data directory, or
/// a Keystead vaults folder.
pub const FOLDER_DATA_DIR: &str = "mirror_folder_data_dir";
/// `unsupported:mirror_folder_missing` – the folder does not exist (any
/// more), e.g. a cloud or network drive that is not mounted.
pub const FOLDER_MISSING: &str = "mirror_folder_missing";
/// `unsupported:mirror_newer` – the folder holds a newer revision of the
/// vault; it is not replaced.
pub const MIRROR_NEWER: &str = "mirror_newer";
/// `unsupported:mirror_diverged` – the folder holds another write of the
/// same revision (another PC, a restored backup here); it is not replaced.
pub const MIRROR_DIVERGED: &str = "mirror_diverged";

/// Largest blob copied: the largest attachment plus the blob's own bytes
/// (the phone refuses anything larger).
const BLOB_MAX_BYTES: u64 = attachments::ATTACHMENT_MAX_BYTES + attachments::BLOB_OVERHEAD as u64;
/// How long the app's exit waits for passes still queued or running.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn io_err(path: &Path, e: std::io::Error) -> AppError {
    AppError::io(format!("{}: {e}", path.display()))
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

// ---------------------------------------------------------------------------
// One pass
// ---------------------------------------------------------------------------

/// What a pass did with the vault file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The folder's copy was missing, older or not readable: written.
    Written,
    /// The folder already holds this revision.
    UpToDate,
    /// The vault file does not exist (any more) – nothing to copy.
    Gone,
}

/// The result of [`mirror_vault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub outcome: Outcome,
    pub blobs_copied: u32,
    pub blobs_removed: u32,
}

/// `<folder>/<id>.keystead`, for a valid vault id only (ids become file
/// names; ours never have anything else, but a path is built from it).
fn copy_path(folder: &Path, vault_id: &str) -> AppResult<PathBuf> {
    let name = format!("{vault_id}.{VAULT_EXTENSION}");
    if sync::vault_id_of_file_name(&name) != Some(vault_id) {
        return Err(CoreError::NotFound(format!("vault {vault_id}")).into());
    }
    Ok(folder.join(name))
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

/// Reads a blob of the data folder, at most [`BLOB_MAX_BYTES`].
fn read_blob(path: &Path) -> AppResult<Vec<u8>> {
    let file = fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut bytes = Vec::new();
    file.take(BLOB_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, e))?;
    Ok(bytes)
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

/// Copies vault `vault_id` of the data directory `data_dir` into `folder`
/// (see the module docs for the order and the revision rule). `check`
/// asks the open vault's session about the folder's copy (`None`: that
/// vault is not open – nothing can be proven).
pub fn mirror_vault(
    data_dir: &Path,
    folder: &Path,
    vault_id: &str,
    check: &dyn Fn(&VaultFile) -> Option<CopyCheck>,
) -> AppResult<Report> {
    let local = VaultStore::new(data_dir).vault_path(vault_id);
    let target = copy_path(folder, vault_id)?;
    // 1. The vault file as it is on disk (saves replace it atomically, so
    //    this is one whole revision).
    let bytes = match fs::read(&local) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Report {
                outcome: Outcome::Gone,
                blobs_copied: 0,
                blobs_removed: 0,
            })
        }
        Err(e) => return Err(io_err(&local, e)),
    };
    let file = VaultFile::parse(&bytes)?;
    if file.id != vault_id {
        return Err(CoreError::Corrupt("vault file of another vault".into()).into());
    }
    check_folder(folder, data_dir)?;

    let write_vault = match read_copy(&target) {
        FolderCopy::Bytes(copy_bytes) => match VaultFile::parse(&copy_bytes) {
            Ok(copy) if copy.id == vault_id && copy.revision >= file.revision => {
                if copy_bytes == bytes {
                    false
                } else if check(&copy) == Some(CopyCheck::Forged) {
                    // Its header was changed by hand (or it is damaged):
                    // the session's keys show it cannot be genuine.
                    log("replacing a forged or damaged copy in the cloud folder");
                    true
                } else if copy.revision > file.revision {
                    return Err(AppError::unsupported(MIRROR_NEWER));
                } else {
                    return Err(AppError::unsupported(MIRROR_DIVERGED));
                }
            }
            // Older, damaged (half a download) or another vault's file
            // under this vault's name: replaced.
            _ => true,
        },
        // Missing, too large or unreadable: replaced.
        FolderCopy::Missing | FolderCopy::Unusable => true,
    };

    // 2. The blobs – listed after reading the vault file, so all the ones
    //    it references are there – before the vault file.
    let local_dir = attachments::dir_for(&local);
    let mirror_dir = attachments::dir_for(&target);
    let local_blobs = blob_names(&local_dir)?;
    let mut blobs_copied = 0;
    for name in &local_blobs {
        let source = local_dir.join(name);
        let copy = mirror_dir.join(name);
        if file_len(&copy).is_some_and(|len| Some(len) == file_len(&source)) {
            continue;
        }
        let blob = match read_blob(&source) {
            Ok(blob) => blob,
            // Removed meanwhile (an attachment removed by a later save,
            // which queues the next pass).
            Err(_) if !source.exists() => continue,
            Err(e) => return Err(e),
        };
        if !attachments::looks_like_blob(&blob) {
            log(format_args!("not mirrored: {name} is no attachment blob"));
            continue;
        }
        format::write_atomic(&copy, &blob, false)?;
        blobs_copied += 1;
    }

    // 3. The vault file.
    if write_vault {
        format::write_atomic(&target, &bytes, false)?;
    }

    // 4. Blobs the data folder no longer has (their attachments are gone
    //    from the revision just written). Only `<id>.bin` files.
    let mut blobs_removed = 0;
    for name in blob_names(&mirror_dir)?.difference(&local_blobs) {
        let path = mirror_dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => blobs_removed += 1,
            Err(e) => log(format_args!("could not remove a mirrored blob: {e}")),
        }
    }
    if local_blobs.is_empty() {
        // Only succeeds if nothing else is left in it.
        let _ = fs::remove_dir(&mirror_dir);
    }
    Ok(Report {
        outcome: if write_vault {
            Outcome::Written
        } else {
            Outcome::UpToDate
        },
        blobs_copied,
        blobs_removed,
    })
}

/// Removes the copy of the deleted vault `vault_id` from `folder`: its
/// vault file (only if it is that vault's) and its blobs. Nothing happens
/// while the vault still exists in `data_dir`. Returns whether a vault
/// file was removed.
pub fn remove_vault(data_dir: &Path, folder: &Path, vault_id: &str) -> AppResult<bool> {
    let target = copy_path(folder, vault_id)?;
    if VaultStore::new(data_dir).vault_path(vault_id).exists() {
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
// The worker and its state
// ---------------------------------------------------------------------------

/// What the worker is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Copy this vault (after a change, on unlock).
    Vault(String),
    /// This vault was deleted: remove its copy.
    Removed(String),
}

impl Job {
    fn vault_id(&self) -> &str {
        match self {
            Job::Vault(id) | Job::Removed(id) => id,
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
    /// The error code of the last pass, `null` after a success.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Progress {
    last_mirrored_at: Option<i64>,
    last_error: Option<String>,
}

/// The mirror's part of [`Core`]: the worker's queue and what the last
/// passes found.
#[derive(Default)]
pub struct MirrorTracker {
    queue: Mutex<Option<Sender<Job>>>,
    progress: Mutex<Progress>,
    /// Deleted vaults whose copy could not be removed yet: tried again with
    /// every pass (while the app runs).
    failed_removals: Mutex<BTreeSet<String>>,
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
            last_error: progress.last_error,
        }
    }

    /// Records the result of the passes for `folder`; returns whether the
    /// status changed. A result for a folder that is no longer the
    /// setting's is dropped.
    fn record(&self, core: &Core, folder: &str, ok: bool, error: Option<String>) -> bool {
        if core.state().settings.mirror_folder.as_deref() != Some(folder) {
            return false;
        }
        let mut progress = lock(&self.progress);
        let before = progress.clone();
        if let Some(code) = &error {
            if progress.last_error.as_ref() != Some(code) {
                log(format_args!(
                    "could not mirror into the cloud folder: {code}"
                ));
            }
        }
        if ok {
            progress.last_mirrored_at = Some(now_ms());
        }
        if ok || error.is_some() {
            progress.last_error = error;
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
    while let Ok(job) = rx.recv() {
        // Everything queued meanwhile goes into the same pass.
        let jobs: Vec<Job> = std::iter::once(job).chain(rx.try_iter()).collect();
        let Some(core) = core.upgrade() else {
            return;
        };
        process(&core, jobs);
    }
}

/// Runs `jobs` (the last one per vault) against the folder of the
/// settings and announces a changed status. Without a folder nothing
/// happens.
pub(crate) fn process(core: &Core, jobs: Vec<Job>) {
    let (folder, data_dir) = {
        let st = core.state();
        (
            st.settings.mirror_folder.clone(),
            st.store.root().to_path_buf(),
        )
    };
    let Some(folder) = folder else {
        return;
    };
    let tracker = core.mirror();
    // Removals that failed before come first (the drive is back, the cloud
    // app let go of the file); the last job per vault wins.
    let retries: Vec<Job> = lock(&tracker.failed_removals)
        .iter()
        .map(|id| Job::Removed(id.clone()))
        .collect();
    let mut last: Vec<Job> = Vec::new();
    for job in retries.into_iter().chain(jobs) {
        last.retain(|j| j.vault_id() != job.vault_id());
        last.push(job);
    }
    // The open vault's session checks a copy in the folder that would
    // block the pass (a higher revision, or another write of the same).
    let check = |copy: &VaultFile| {
        let st = core.state();
        st.vault
            .as_ref()
            .filter(|vault| vault.id() == copy.id)
            .map(|vault| vault.check_copy(copy))
    };
    let (mut ok, mut error) = (false, None);
    for job in &last {
        let result = match job {
            Job::Vault(id) => mirror_vault(&data_dir, Path::new(&folder), id, &check)
                .map(|r| r.outcome != Outcome::Gone),
            Job::Removed(id) => {
                let removed = remove_vault(&data_dir, Path::new(&folder), id).map(|_| true);
                let mut failed = lock(&tracker.failed_removals);
                if removed.is_ok() {
                    failed.remove(id);
                } else {
                    failed.insert(id.clone());
                }
                removed
            }
        };
        match result {
            Ok(done) => ok |= done,
            Err(e) => {
                error.get_or_insert(e.code());
            }
        }
    }
    if tracker.record(core, &folder, ok, error) {
        core.emit(EVENT_STATUS, tracker.status(core));
    }
}

/// `mirrorFolder` was changed (set, replaced or turned off): the status
/// starts over, and the open vault is copied into the new folder right
/// away. The copies in a previous folder stay there; removals that failed
/// are tried again (in a folder without that copy they have nothing to do).
pub fn folder_changed(core: &Core) {
    let tracker = core.mirror();
    tracker.reset();
    let open = core.state().vault.as_ref().map(|v| v.id().to_owned());
    if let Some(id) = open {
        tracker.request(Job::Vault(id));
    }
    let retries: Vec<String> = lock(&tracker.failed_removals).iter().cloned().collect();
    for id in retries {
        tracker.request(Job::Removed(id));
    }
    core.emit(EVENT_STATUS, tracker.status(core));
}

/// `mirror_status` → [`MirrorStatus`]. Polled: not user activity.
#[tauri::command]
pub async fn mirror_status(core: Shared<'_>) -> CmdResult<MirrorStatus> {
    run(&core, false, |c| Ok(c.mirror().status(c))).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keystead_core::model::{ItemType, VaultItem};
    use keystead_core::KdfParams;

    use super::*;
    use crate::folder_sync::{self, LinkState, Outcome as PhoneOutcome, RemoteEntry, RemoteFolder};
    use crate::state::test_support::core_with_open_vault;

    /// A desktop core with an open vault and the mirror folder `<dir>/cloud`
    /// (created).
    fn pc(dir: &Path) -> (Arc<Core>, PathBuf) {
        let cloud = dir.join("cloud");
        fs::create_dir_all(&cloud).unwrap();
        let settings = Settings {
            mirror_folder: Some(cloud.to_string_lossy().into_owned()),
            ..Settings::default()
        };
        (core_with_open_vault(dir, settings), cloud)
    }

    fn open_id(core: &Core) -> String {
        core.state().vault.as_ref().unwrap().id().to_owned()
    }

    /// `mirror_vault`'s check without an open session.
    fn no_session(_: &VaultFile) -> Option<CopyCheck> {
        None
    }

    fn vault_path(core: &Core) -> PathBuf {
        core.state().vault.as_ref().unwrap().path().to_path_buf()
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

    /// Runs the jobs the hooks queued (no worker in unit tests).
    fn run_queued(core: &Core) {
        let jobs = core.mirror().take_recorded();
        process(core, jobs);
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

    fn status_events(core: &Core) -> Vec<serde_json::Value> {
        core.emitted()
            .into_iter()
            .filter(|(name, _)| name == EVENT_STATUS)
            .map(|(_, payload)| payload)
            .collect()
    }

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

        // The status says so, once.
        let events = status_events(&core);
        assert_eq!(events.len(), 1);
        assert!(events[0]["lastMirroredAt"].is_i64());
        assert!(events[0]["lastError"].is_null());
        assert_eq!(events[0]["folder"], cloud.to_string_lossy().as_ref());
    }

    #[test]
    fn unlocking_and_changes_ask_for_a_pass_and_equal_revisions_are_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        // Opening a vault (UI, extension, recovery key, a new vault).
        let other = core
            .store()
            .create_vault_with_params("Arbeit", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let other_id = other.id().to_owned();
        core.install_vault(other).unwrap();
        assert_eq!(
            core.mirror().take_recorded(),
            vec![Job::Vault(other_id.clone())]
        );
        core.lock(None);
        core.unlock(&id, "master").unwrap();
        assert_eq!(core.mirror().take_recorded(), vec![Job::Vault(id.clone())]);

        let target = cloud.join(format!("{id}.keystead"));
        assert_eq!(
            mirror_vault(&core.data_dir(), &cloud, &id, &no_session)
                .unwrap()
                .outcome,
            Outcome::Written
        );
        // The same revision again: left alone (no upload by the cloud app).
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            mirror_vault(&core.data_dir(), &cloud, &id, &no_session)
                .unwrap()
                .outcome,
            Outcome::UpToDate
        );
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
        assert_eq!(tree(&vaults), before);
        assert_eq!(
            core.mirror().status(&core).last_error.as_deref(),
            Some("invalid_input:mirror_folder_data_dir")
        );
    }

    #[test]
    fn a_newer_copy_in_the_folder_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "eins");
        note(&core, "zwei");
        run_queued(&core);
        let target = cloud.join(format!("{id}.keystead"));
        let newer = fs::read(&target).unwrap();
        // The PC goes back to an older revision (a restored backup).
        let bak = sibling(&vault_path(&core), ".bak");
        core.lock(None);
        fs::copy(&bak, vault_path_of(&core, &id)).unwrap();
        let err = mirror_vault(&core.data_dir(), &cloud, &id, &no_session).unwrap_err();
        assert_eq!(err.code(), "unsupported:mirror_newer");
        assert_eq!(fs::read(&target).unwrap(), newer);
    }

    fn vault_path_of(core: &Core, id: &str) -> PathBuf {
        core.store().vault_path(id)
    }

    #[test]
    fn another_write_of_the_same_revision_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "eins");
        note(&core, "zwei");
        run_queued(&core);
        let target = cloud.join(format!("{id}.keystead"));
        let folder_copy = fs::read(&target).unwrap();
        // Back to the previous revision (a restored backup), then a save of
        // other content: the same revision number as the folder's copy.
        let bak = sibling(&vault_path(&core), ".bak");
        core.lock(None);
        fs::copy(&bak, vault_path_of(&core, &id)).unwrap();
        core.unlock(&id, "master").unwrap();
        run_queued(&core);
        assert_eq!(
            core.mirror().status(&core).last_error.as_deref(),
            Some("unsupported:mirror_newer")
        );
        note(&core, "anders");
        run_queued(&core);
        assert_eq!(
            VaultFile::read(&target).unwrap().revision,
            core.read(|v| v.revision()).unwrap()
        );
        assert_eq!(fs::read(&target).unwrap(), folder_copy, "not replaced");
        assert_eq!(
            core.mirror().status(&core).last_error.as_deref(),
            Some("unsupported:mirror_diverged")
        );
        // The next save is newer than the folder's copy again.
        note(&core, "weiter");
        run_queued(&core);
        assert_eq!(
            fs::read(&target).unwrap(),
            fs::read(vault_path(&core)).unwrap()
        );
        assert!(core.mirror().status(&core).last_error.is_none());
    }

    #[test]
    fn a_copy_the_open_vault_proves_forged_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (core, cloud) = pc(dir.path());
        let id = open_id(&core);
        note(&core, "eins");
        run_queued(&core);
        let target = cloud.join(format!("{id}.keystead"));
        // Someone with access to the folder raised the revision by hand:
        // the payload (its AAD binds the revision) no longer authenticates.
        let mut forged = VaultFile::read(&target).unwrap();
        forged.revision = u64::MAX;
        fs::write(&target, forged.to_json().unwrap()).unwrap();
        // Without the open vault nothing proves it: never overwritten.
        let err = mirror_vault(&core.data_dir(), &cloud, &id, &no_session).unwrap_err();
        assert_eq!(err.code(), "unsupported:mirror_newer");
        // With it, the next save replaces it.
        note(&core, "zwei");
        run_queued(&core);
        assert_eq!(
            fs::read(&target).unwrap(),
            fs::read(vault_path(&core)).unwrap()
        );
        assert!(core.mirror().status(&core).last_error.is_none());

        // A genuinely newer copy (another PC with the same password wrote
        // it into the folder) is still never replaced.
        let other_pc = VaultStore::new(dir.path().join("other-pc"));
        let other_path = other_pc.vault_path(&id);
        fs::create_dir_all(other_path.parent().unwrap()).unwrap();
        fs::copy(vault_path(&core), &other_path).unwrap();
        let mut other = other_pc.unlock(&id, "master").unwrap();
        other
            .save_item(VaultItem::new(ItemType::Note, "anderer PC"))
            .unwrap();
        let newer = fs::read(other.path()).unwrap();
        fs::write(&target, &newer).unwrap();
        assert_eq!(
            mirror_vault(&core.data_dir(), &cloud, &id, &|copy| {
                core.read(|v| v.check_copy(copy)).ok()
            })
            .unwrap_err()
            .code(),
            "unsupported:mirror_newer"
        );
        assert_eq!(fs::read(&target).unwrap(), newer);
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
        assert_eq!(
            mirror_vault(&core.data_dir(), &cloud, &id, &no_session)
                .unwrap()
                .outcome,
            Outcome::Written
        );
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

        // The drive is back: the next pass – for another vault – removes it.
        fs::rename(&away, &cloud).unwrap();
        core.install_vault(other).unwrap();
        run_queued(&core);
        assert!(!copy.exists(), "the deleted vault's copy is gone");
        assert!(cloud.join(format!("{other_id}.keystead")).exists());
        assert!(core.mirror().status(&core).last_error.is_none());
        assert!(lock(&core.mirror().failed_removals).is_empty());
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
        mirror_vault(&core.data_dir(), &cloud, &other_id, &no_session).unwrap();
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
        process(&core, vec![Job::Vault(id.clone())]);
        assert!(status_events(&core).is_empty(), "off: nothing happens");

        let second = dir.path().join("OneDrive").join("Keystead");
        fs::create_dir_all(&second).unwrap();
        core.state().settings.mirror_folder = Some(second.to_string_lossy().into_owned());
        folder_changed(&core);
        assert_eq!(core.mirror().take_recorded(), vec![Job::Vault(id.clone())]);
        let events = status_events(&core);
        assert_eq!(
            events.last().unwrap()["folder"],
            second.to_string_lossy().as_ref()
        );
        process(&core, vec![Job::Vault(id.clone())]);
        assert!(second.join(format!("{id}.keystead")).exists());
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

    /// The phone's side of the contract: `folder_sync` takes what the
    /// mirror wrote – the vault with its attachment, then a newer revision.
    #[test]
    fn the_phone_takes_what_the_mirror_wrote() {
        struct Dir(PathBuf);
        impl RemoteFolder for Dir {
            fn list(&self, parent: Option<&RemoteEntry>) -> AppResult<Vec<RemoteEntry>> {
                let dir = parent.map_or(self.0.clone(), |p| PathBuf::from(&p.uri));
                let mut out = Vec::new();
                for entry in fs::read_dir(&dir)?.flatten() {
                    let meta = entry.metadata()?;
                    out.push(RemoteEntry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        uri: entry.path().to_string_lossy().into_owned(),
                        dir: meta.is_dir(),
                        size: Some(meta.len()),
                    });
                }
                Ok(out)
            }
            fn copy_to(&self, entry: &RemoteEntry, target: &Path, _max: u64) -> AppResult<u64> {
                Ok(fs::copy(&entry.uri, target)?)
            }
            // A read-only link: the phone only takes what the PC wrote.
            fn writable(&self) -> AppResult<bool> {
                Ok(false)
            }
            fn write_vault(&self, _name: &str, _bytes: &[u8]) -> AppResult<RemoteEntry> {
                Err(crate::error::AppError::unsupported(
                    folder_sync::NOT_WRITABLE,
                ))
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

        let phone = Core::for_tests(
            VaultStore::new(dir.path().join("phone")),
            Settings::default(),
        );
        let mut link = LinkState::new("content://tree".into(), "Keystead".into(), 1, false);
        assert!(folder_sync::sync_folder(&phone, &Dir(cloud.clone()), &mut link, 2).unwrap());
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
        assert!(folder_sync::sync_folder(&phone, &Dir(cloud), &mut link, 3).unwrap());
        assert_eq!(link.files[0].outcome, PhoneOutcome::Updated);
        let vault = phone.store().unlock(&id, "master").unwrap();
        assert_eq!(vault.items().len(), 2);
    }
}
