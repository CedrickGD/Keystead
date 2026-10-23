//! Conflicts of the two-way cloud-folder sync (Block 5 stage 2b) – shared
//! by the phone's folder sync (`folder_sync.rs`) and the PC's mirror
//! (`mirror.rs`). Platform-neutral.
//!
//! * The record (`<data>/sync/conflicts.json`, [`SyncConflict`] per vault
//!   plus the digests of both writes and whether the open vault's keys
//!   checked the folder's copy): recorded whenever a sync finds both copies
//!   changed since the last agreed head (`keystead_core::sync::decide` →
//!   `Diverged`), shown (`sync_conflicts`, `sync://conflict`) only once
//!   checked, removed when a later sync finds the two sides agreeing again
//!   or the user resolves it ([`resolve_sync_conflict`]).
//! * Conflict copies: when the user resolves a conflict, the losing side's
//!   vault file is not dropped: it is kept as
//!   `<data>/conflicts/<id>.keystead.conflict-<YYYYMMDD-HHMMSS>` (UTC; see
//!   `keystead_core::sync::conflict_file_name`) on the side that resolved
//!   it – not in `vaults/`, so no store lists it as a vault. The user can
//!   import it again (file import of a `.keystead` file). The losing side's
//!   blobs are copied beside it (`<copy>.attachments/<attachment id>.bin`,
//!   only files of a blob's name, shape and size), so attachments the
//!   winning side no longer references are not lost with the hourly sweep.
//!   The phone – which has no import – also writes the losing side's vault
//!   file into the cloud folder as
//!   `<id>.keystead.conflict-<YYYYMMDD-HHMMSS>-phone`
//!   (`keystead_core::sync::phone_conflict_file_name`, never a vault file
//!   name), which the PC lists and offers to restore (`mirror.rs`).
//! * A resolution names the two writes the dialog showed (their digests):
//!   if the recorded conflict is about other writes by then, it is refused
//!   (`unsupported:sync_conflict_changed`).

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::attachments;
use keystead_core::format::{self, VaultFile};
use keystead_core::model::{now_ms, VaultInfo};
use keystead_core::sync::{self, SyncBase, SyncHead};
use keystead_core::vault::CopyCheck;
use keystead_core::Error as CoreError;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::commands::{run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::state::{log, Core, LockReason};
use crate::sync_base::{self, sync_dir};
use crate::sync_trial::{begin_chosen_trial, trial_lock, Trial};

/// Error code: "keep this side" for a conflict whose folder copy has
/// another master password – the side with the old password never wins.
pub const CONFLICT_PASSWORD: &str = "sync_conflict_password";
/// Error code: a copy changed since the conflict was recorded (or the
/// folder's is gone): it was looked at again; the UI fetches the list anew.
pub const CONFLICT_CHANGED: &str = "sync_conflict_changed";
/// `sync://conflict` – payload `SyncConflict[]`: the open vault's checked
/// conflicts, whenever the recorded conflicts change (both platforms).
pub const EVENT_CONFLICT: &str = "sync://conflict";

/// `unsupported:sync_conflict_changed`.
pub fn conflict_changed() -> AppError {
    AppError::unsupported(CONFLICT_CHANGED)
}

/// A conflict of the two-way sync, for the UI (`sync_conflicts`,
/// `sync://conflict`): this side's copy of a vault and the folder's both
/// changed since they last agreed (`keystead_core::sync::decide` →
/// `Diverged`). No secrets: ids, names, revisions and times from the plain
/// headers, and a count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub vault_id: String,
    /// This side's name of the vault.
    pub vault_name: String,
    /// This side's copy ("local") and the folder's ("remote").
    pub local_revision: u64,
    pub remote_revision: u64,
    pub local_updated_at: i64,
    pub remote_updated_at: i64,
    /// The folder's copy has another master password (KDF salt or
    /// parameters): this side's state cannot be kept – the side with the
    /// old password never wins.
    pub password_changed: bool,
    /// How many attachments only this side's copy has (by attachment id;
    /// both copies decrypted with the open vault's key) – what keeping the
    /// folder's copy drops (their blobs go with the conflict copy). Omitted
    /// (0) when unknown.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lost_attachments: u32,
    /// How many attachments only the folder's copy has – what keeping this
    /// side's copy drops (on the phone: attachments the PC added; their
    /// blobs go with the conflict copy). Omitted (0) when unknown.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub remote_only_attachments: u32,
    /// The heads' digests of this side's and the folder's copy: the dialog
    /// passes them back with its choice (`resolve_sync_conflict`), so a
    /// resolution never applies to writes the user did not see.
    pub local_digest: String,
    pub remote_digest: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

// ---------------------------------------------------------------------------
// The record (`<data>/sync/conflicts.json`)
// ---------------------------------------------------------------------------

/// A recorded conflict: what the UI gets (the two writes it is about
/// included) and whether it may be shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRecord {
    #[serde(flatten)]
    pub conflict: SyncConflict,
    /// The open vault's keys checked the folder's copy (not forged): only
    /// then the user is asked.
    #[serde(default)]
    pub verified: bool,
}

fn conflicts_path(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("conflicts.json")
}

/// Read leniently: a missing or damaged file holds none.
pub fn load_records(data_dir: &Path) -> Vec<ConflictRecord> {
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

/// Forgets every recorded conflict (the folder was unlinked, another one
/// linked or – PC – the mirror folder changed).
pub fn clear_conflicts(data_dir: &Path) -> AppResult<()> {
    match fs::remove_file(conflicts_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Records (`Some`) or forgets (`None`) the conflict of vault `id`; a
/// change is announced as `sync://conflict`. Call without the state lock.
pub fn set_record(core: &Core, id: &str, record: Option<ConflictRecord>) {
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

/// Records the conflict of vault `id` between this side's copy `local` and
/// the folder's `remote` (each with its head). `checked_now`: the open
/// vault's keys just checked the folder's copy (not forged); a check of the
/// same two writes in an earlier session still counts. `attachments`: the
/// counts `lost_attachments` and `remote_only_attachments` of
/// [`SyncConflict`] (kept from that earlier check when not checked now).
/// Call without the state lock.
pub fn record_conflict(
    core: &Core,
    id: &str,
    (local, local_head): (&VaultFile, &SyncHead),
    (remote, remote_head): (&VaultFile, &SyncHead),
    checked_now: bool,
    attachments: (u32, u32),
) {
    let previous = load_records(&core.data_dir()).into_iter().find(|r| {
        r.conflict.vault_id == id
            && r.verified
            && r.conflict.local_digest == local_head.digest
            && r.conflict.remote_digest == remote_head.digest
    });
    let (lost_attachments, remote_only_attachments) = match (&previous, checked_now) {
        (Some(previous), false) => (
            previous.conflict.lost_attachments,
            previous.conflict.remote_only_attachments,
        ),
        _ => attachments,
    };
    let record = ConflictRecord {
        conflict: SyncConflict {
            vault_id: id.to_owned(),
            vault_name: local.name.clone(),
            local_revision: local.revision,
            remote_revision: remote.revision,
            local_updated_at: local.updated_at,
            remote_updated_at: remote.updated_at,
            password_changed: password_changed(local, remote),
            lost_attachments,
            remote_only_attachments,
            local_digest: local_head.digest.clone(),
            remote_digest: remote_head.digest.clone(),
        },
        verified: checked_now || previous.is_some(),
    };
    set_record(core, id, Some(record));
}

/// The checked conflicts of vault `id`.
fn conflicts_of(data_dir: &Path, id: &str) -> Vec<SyncConflict> {
    load_records(data_dir)
        .into_iter()
        .filter(|r| r.verified && r.conflict.vault_id == id)
        .map(|r| r.conflict)
        .collect()
}

/// The checked record of vault `id` (`not_found` without one): what a
/// resolution works on. `seen`: the digests (this side's, the folder's)
/// of the two writes the dialog showed – a record about other writes is
/// `unsupported:sync_conflict_changed`.
pub fn verified_record(data_dir: &Path, id: &str, seen: (&str, &str)) -> AppResult<ConflictRecord> {
    let record = load_records(data_dir)
        .into_iter()
        .find(|r| r.verified && r.conflict.vault_id == id)
        .ok_or_else(|| AppError::Code("not_found".into()))?;
    if record.conflict.local_digest != seen.0 || record.conflict.remote_digest != seen.1 {
        return Err(conflict_changed());
    }
    Ok(record)
}

/// Unit tests: a resolution of the recorded conflict of `id` as it is.
#[cfg(test)]
pub fn recorded_digests(data_dir: &Path, id: &str) -> (String, String) {
    load_records(data_dir)
        .into_iter()
        .find(|r| r.conflict.vault_id == id)
        .map(|r| (r.conflict.local_digest, r.conflict.remote_digest))
        .unwrap_or_default()
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

/// The folder's copy has another master password than this side's (another
/// key-derivation salt or parameters).
pub fn password_changed(local: &VaultFile, remote: &VaultFile) -> bool {
    local.kdf.salt != remote.kdf.salt || local.kdf_params() != remote.kdf_params()
}

// ---------------------------------------------------------------------------
// Resolving
// ---------------------------------------------------------------------------

/// Which side of a conflict the user keeps (`resolve_sync_conflict`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// This side's copy: it is saved again above the folder's revision and
    /// written to the folder.
    Local,
    /// The folder's copy: it replaces this side's.
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

/// "Keep the folder's copy": stores `bytes` (`remote`, checked as `check`
/// by the open vault) in place of this side's copy at `path` – if that is
/// still the write `local_digest` – and replaces the open session with one
/// of the new copy (same master password), or locks the vault
/// (`keyChanged`: another master password; the copy stays on trial until
/// it opens). The folder's head becomes the base for `folder` (the phone's
/// tree URI, the PC's mirror folder). Returns the new session's info,
/// `None` after the lock.
#[allow(clippy::too_many_arguments)]
pub fn install_resolved(
    core: &Core,
    folder: &str,
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
            // The user chose it: confirmed when it opens, whatever its
            // ancestry.
            _ => begin_chosen_trial(&data_dir, id, path, remote)?,
        }
        format::write_atomic(path, bytes, false)?;
        let base = SyncBase::new(&sync::head_of(remote), folder, now_ms());
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

/// `sync_conflicts(pageVaultId)` → `SyncConflict[]`: the conflicts of the
/// page's open vault that its keys checked (`locked` for another vault).
/// No I/O on the folder. Both platforms.
#[tauri::command]
pub async fn sync_conflicts(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<Vec<SyncConflict>> {
    run(&core, false, move |c| {
        conflicts_for_page(c, page_vault_id.as_deref())
    })
    .await
}

/// `resolve_sync_conflict(vaultId, pageVaultId, keep: "local"|"remote",
/// localDigest, remoteDigest)` → `VaultInfo`: keeps one side of the open
/// vault's conflict – the one about the two writes the dialog showed
/// (`unsupported:sync_conflict_changed` otherwise); the other is kept as a
/// conflict copy in `<data>/conflicts/` (the phone also writes it into the
/// cloud folder). Phone: `mobile::sync::resolve`
/// (`folder_sync::resolve_conflict`); PC: `mirror::resolve_conflict`.
/// Keeping the folder's copy may lock the vault (`keyChanged`: another
/// master password).
#[tauri::command]
pub async fn resolve_sync_conflict(
    app: AppHandle,
    core: Shared<'_>,
    vault_id: String,
    page_vault_id: Option<String>,
    keep: String,
    local_digest: String,
    remote_digest: String,
) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| {
        let keep = Keep::parse(&keep)?;
        let seen = (local_digest.as_str(), remote_digest.as_str());
        #[cfg(mobile)]
        let resolved =
            crate::mobile::sync::resolve(&app, c, &vault_id, page_vault_id.as_deref(), keep, seen);
        #[cfg(desktop)]
        let resolved = {
            let _ = &app;
            crate::mirror::resolve_conflict(
                c,
                &vault_id,
                page_vault_id.as_deref(),
                keep,
                seen,
                now_ms(),
            )
        };
        resolved
    })
    .await
}

// ---------------------------------------------------------------------------
// Conflict copies (`<data>/conflicts`)
// ---------------------------------------------------------------------------

/// `<data>/conflicts`.
pub fn conflicts_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("conflicts")
}

/// One conflict copy in `<data>/conflicts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictCopy {
    /// The vault it is a copy of (from its file name).
    pub vault_id: String,
    pub path: PathBuf,
}

/// Keeps `bytes` – the losing side's vault file of vault `id` – as a
/// conflict copy made at `at_ms` and returns its path. A copy of the same
/// second already there is not replaced: the new one gets `-2`, `-3`, … at
/// the end of its name. With `blobs` (the losing side's blob folder), its
/// blob files (`<attachment id>.bin` of a blob's shape and size; nothing
/// else) are copied into the copy's own blob folder after the vault file
/// ([`copy_blobs`]); a blob folder that does not exist is nothing to copy.
pub fn keep_conflict_copy(
    data_dir: &Path,
    id: &str,
    bytes: &[u8],
    blobs: Option<&Path>,
    at_ms: i64,
) -> AppResult<PathBuf> {
    if !crate::sync_base::valid_id(id) {
        return Err(AppError::invalid("vault id"));
    }
    let dir = conflicts_dir(data_dir);
    fs::create_dir_all(&dir)?;
    let name = sync::conflict_file_name(id, at_ms);
    let path = (1..)
        .map(|n| match n {
            1 => dir.join(&name),
            n => dir.join(format!("{name}-{n}")),
        })
        .find(|path| !path.exists() && !attachments::dir_for(path).exists())
        .expect("an unused name");
    format::write_atomic(&path, bytes, false)?;
    if let Some(blobs) = blobs {
        copy_blobs(blobs, &attachments::dir_for(&path))?;
    }
    Ok(path)
}

/// The largest blob: the largest attachment plus the blob's own bytes.
pub const BLOB_MAX_BYTES: u64 =
    attachments::ATTACHMENT_MAX_BYTES + attachments::BLOB_OVERHEAD as u64;

/// At most this many blobs go with one conflict copy: 1,000 items with
/// the most attachments one item may have
/// (`attachments::ATTACHMENTS_PER_ITEM_MAX`) – far beyond a real vault.
pub const CONFLICT_BLOBS_MAX_COUNT: u32 = 1_000 * attachments::ATTACHMENTS_PER_ITEM_MAX as u32;
/// At most this many bytes of blobs go with one conflict copy: 100 of the
/// largest attachments (`attachments::ATTACHMENT_MAX_BYTES`, ~2 GiB).
pub const CONFLICT_BLOBS_MAX_BYTES: u64 = 100 * BLOB_MAX_BYTES;
/// Error code: the blob folder of a conflict copy holds more blobs (or
/// bytes) than [`CONFLICT_BLOBS_MAX_COUNT`] / [`CONFLICT_BLOBS_MAX_BYTES`]:
/// the copy stopped there (the vault file and the blobs copied so far
/// stay) – nothing fills the disk.
pub const CONFLICT_BLOBS_TOO_MANY: &str = "sync_conflict_blobs_too_many";

/// Copies the blob files of `from` into `to` (created when there is one):
/// only `<id>.bin` files of at most [`BLOB_MAX_BYTES`] that look like a
/// blob (`attachments::looks_like_blob`) – a folder the cloud app or the
/// user filled with other things fills no disk here – and at most
/// [`CONFLICT_BLOBS_MAX_COUNT`] blobs of [`CONFLICT_BLOBS_MAX_BYTES`]
/// together: beyond that it stops with
/// `unsupported:sync_conflict_blobs_too_many`. Returns how many.
pub fn copy_blobs(from: &Path, to: &Path) -> AppResult<u32> {
    copy_blobs_bounded(from, to, CONFLICT_BLOBS_MAX_COUNT, CONFLICT_BLOBS_MAX_BYTES)
}

fn copy_blobs_bounded(from: &Path, to: &Path, max_count: u32, max_bytes: u64) -> AppResult<u32> {
    use std::io::Read as _;
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut copied = 0;
    let mut total: u64 = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if attachments::blob_id_of_file_name(name).is_none() || !entry.file_type()?.is_file() {
            continue;
        }
        let size = entry.metadata()?.len();
        if size > BLOB_MAX_BYTES {
            log(format_args!("not kept: {name} is too large for a blob"));
            continue;
        }
        if copied >= max_count || total.saturating_add(size) > max_bytes {
            log(format_args!(
                "a conflict copy's blobs stopped after {copied} ({total} bytes): too many"
            ));
            return Err(AppError::unsupported(CONFLICT_BLOBS_TOO_MANY));
        }
        let mut blob = Vec::new();
        fs::File::open(entry.path())?
            .take(BLOB_MAX_BYTES + 1)
            .read_to_end(&mut blob)?;
        if blob.len() as u64 > BLOB_MAX_BYTES || !attachments::looks_like_blob(&blob) {
            log(format_args!("not kept: {name} is no attachment blob"));
            continue;
        }
        fs::create_dir_all(to)?;
        format::write_atomic(&to.join(name), &blob, false)?;
        copied += 1;
        total += blob.len() as u64;
    }
    Ok(copied)
}

/// The conflict copies in `<data>/conflicts`, oldest first (by name: the
/// time is in it), of vault `id` only or – `None` – of every vault. Blob
/// folders, temporary files and anything else are not listed.
// The conflict dialog says where the copies are (stage 2b).
#[cfg_attr(not(test), expect(dead_code))]
pub fn list(data_dir: &Path, id: Option<&str>) -> AppResult<Vec<ConflictCopy>> {
    let entries = match fs::read_dir(conflicts_dir(data_dir)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let marker = format!(".{}.conflict-", format::VAULT_EXTENSION);
    let mut copies = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some((vault_id, _)) = name.split_once(&marker) else {
            continue;
        };
        if !crate::sync_base::valid_id(vault_id) || id.is_some_and(|id| id != vault_id) {
            continue;
        }
        copies.push(ConflictCopy {
            vault_id: vault_id.to_owned(),
            path: entry.path(),
        });
    }
    copies.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(copies)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "29a9e366-e89d-4f3f-8c15-814a2200224b";
    /// 2026-10-23 14:05:09 UTC.
    const AT: i64 = 1_792_764_309_000;

    /// Bytes of a blob's shape (magic, room for nonce and tag).
    fn blob_bytes() -> Vec<u8> {
        let mut bytes = b"KSAT1".to_vec();
        bytes.resize(attachments::BLOB_OVERHEAD + 10, 7);
        bytes
    }

    #[test]
    fn a_conflict_copy_is_kept_with_its_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let blobs = dir.path().join("vault.keystead.attachments");
        fs::create_dir_all(&blobs).unwrap();
        let blob = "0b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f.bin";
        fs::write(blobs.join(blob), blob_bytes()).unwrap();
        // Not blobs: left behind.
        fs::write(blobs.join(".x.bin.0123456789abcdef.tmp"), b"tmp").unwrap();
        fs::write(blobs.join("copy (1).bin"), b"copy").unwrap();
        fs::create_dir(blobs.join("sub.bin")).unwrap();
        // Named like a blob, but not one (wrong shape; too large).
        fs::write(
            blobs.join("1b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f.bin"),
            b"not a blob at all, but long enough to pass the size check....",
        )
        .unwrap();
        let huge =
            fs::File::create(blobs.join("2b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f.bin")).unwrap();
        huge.set_len(BLOB_MAX_BYTES + 1).unwrap();
        drop(huge);

        let path = keep_conflict_copy(&data, ID, b"vault bytes", Some(&blobs), AT).unwrap();
        assert_eq!(
            path,
            data.join("conflicts")
                .join(format!("{ID}.keystead.conflict-20261023-140509"))
        );
        assert_eq!(fs::read(&path).unwrap(), b"vault bytes");
        let copied = attachments::dir_for(&path);
        let names: Vec<String> = fs::read_dir(&copied)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [blob]);
        assert_eq!(fs::read(copied.join(blob)).unwrap(), blob_bytes());
        // Never in `vaults/`.
        assert!(!data.join("vaults").exists());

        // The same second again: a second copy, the first one untouched.
        let second = keep_conflict_copy(&data, ID, b"other bytes", None, AT).unwrap();
        assert_eq!(
            second.file_name().unwrap().to_string_lossy(),
            format!("{ID}.keystead.conflict-20261023-140509-2")
        );
        assert_eq!(fs::read(&path).unwrap(), b"vault bytes");
        assert!(!attachments::dir_for(&second).exists(), "no blobs given");
        // A blob folder that does not exist is nothing to copy.
        let third =
            keep_conflict_copy(&data, ID, b"x", Some(&dir.path().join("missing")), AT).unwrap();
        assert!(third.is_file());
        assert!(!attachments::dir_for(&third).exists());

        // Nothing but a vault id names a file.
        assert!(keep_conflict_copy(&data, "../x", b"x", None, AT).is_err());
    }

    /// Review L5: a blob folder with more than the bound stops the copy
    /// with an error instead of filling the disk.
    #[test]
    fn too_many_blobs_stop_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        fs::create_dir_all(&from).unwrap();
        for n in 0..5 {
            fs::write(
                from.join(format!("{n}b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f.bin")),
                blob_bytes(),
            )
            .unwrap();
        }
        let size = blob_bytes().len() as u64;
        // Within both bounds: all of them.
        let to = dir.path().join("all");
        assert_eq!(copy_blobs_bounded(&from, &to, 5, 5 * size).unwrap(), 5);
        // One too many by count, or by bytes: stopped and reported.
        for (count, bytes) in [(4, 5 * size), (5, 4 * size + 1)] {
            let to = dir.path().join(format!("cut-{count}-{bytes}"));
            let err = copy_blobs_bounded(&from, &to, count, bytes).unwrap_err();
            assert_eq!(err.code(), format!("unsupported:{CONFLICT_BLOBS_TOO_MANY}"));
            assert_eq!(fs::read_dir(&to).unwrap().count(), 4);
        }
        // The real bounds follow the attachment limits.
        assert_eq!(CONFLICT_BLOBS_MAX_COUNT, 10_000);
        const { assert!(CONFLICT_BLOBS_MAX_BYTES > 2_000_000_000) };
    }

    #[test]
    fn conflict_copies_are_listed_per_vault() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert_eq!(list(data, None).unwrap(), []);
        let other = "11111111-2222-3333-4444-555555555555";
        let later = keep_conflict_copy(data, ID, b"b", None, AT + 60_000).unwrap();
        let first = keep_conflict_copy(data, ID, b"a", None, AT).unwrap();
        let theirs = keep_conflict_copy(data, other, b"c", None, AT).unwrap();
        // Blob folders and anything else are not copies.
        fs::create_dir_all(attachments::dir_for(&first)).unwrap();
        fs::write(conflicts_dir(data).join("notes.txt"), b"x").unwrap();
        fs::write(
            conflicts_dir(data).join(format!("{ID}.keystead")),
            b"not a conflict copy",
        )
        .unwrap();

        let all = list(data, None).unwrap();
        let paths: Vec<&PathBuf> = all.iter().map(|c| &c.path).collect();
        assert_eq!(paths, [&theirs, &first, &later]);
        assert_eq!(all[0].vault_id, other);
        let mine = list(data, Some(ID)).unwrap();
        assert_eq!(
            mine.iter().map(|c| &c.path).collect::<Vec<_>>(),
            [&first, &later]
        );
        assert!(mine.iter().all(|c| c.vault_id == ID));
    }
}
