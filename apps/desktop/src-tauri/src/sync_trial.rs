//! Copies on trial (`<data>/sync/trial`): vault copies the cloud-folder
//! sync took from the folder and that have not opened on this device yet.
//! See "Copies on trial" in docs/ARCHITECTURE.md. Platform-neutral: the
//! phone's folder sync (`folder_sync.rs`) stores such copies, and every
//! master-password unlock on both platforms goes through [`unlock_copy`]
//! (`Core::unlock`).
//!
//! The plain header of a vault file – the revision included – is not
//! authenticated without the keys, so anyone who can write the folder
//! could raise it. A copy the sync stores is therefore **unconfirmed**
//! until it opens here ([`unlock_copy`]; or right away when the open
//! vault's keys check it, `UnlockedVault::check_copy`): the copy it
//! replaced is kept until then, and its revision – not the unconfirmed
//! one's – is what the folder's next copy has to beat. A copy that does not
//! open although its key did (forged or damaged) is rejected: the kept copy
//! comes back (or, without one, the copy goes), the vault's sync base goes
//! back with it (`sync_base::reset_to`) and that write of the folder's file
//! is not taken again ([`Trial::rejected`]). An unconfirmed copy never has
//! local edits: it is replaced or confirmed before anything is saved to it
//! (a save of the open vault ends the trial: [`after_save`]).
//!
//! A copy that **opens** is still only confirmed if it descends from the
//! copy it replaced ([`settle`]): its sync proof
//! (`keystead_core::sync::SyncProof`) must verify with the opened keys and
//! name the kept copy's head – while the vault was locked, nothing could
//! check that. If it does not, the kept copy comes back as this device's
//! state (opened with the same master key), the vault's base goes back to
//! it and that write of the folder's file is remembered
//! ([`Trial::diverged`]): the next sync finds the two in conflict and asks
//! the user – it is never taken on trial again. Only a kept copy the
//! opened master key cannot open (the master password was changed on the
//! other side meanwhile – the side with the old password never wins) is
//! not put back: it is kept as a conflict copy (`<data>/conflicts/`; on the
//! phone the next sync also writes it into the cloud folder) and the new
//! copy is confirmed – and the user is told (`sync://notice`,
//! [`NOTICE_KEPT_ASIDE`]). (The phone's sync no longer takes a copy with
//! another master password on trial over edits of its own: only over a
//! copy that is the sync base; otherwise it is a conflict.)
//!
//! A copy on trial with **another salt** that the typed password (or the
//! fingerprint's key) does not open, while it opens the kept copy, never
//! locks the user out: the kept copy comes back and opens, that write of
//! the folder's file becomes a conflict ([`Trial::diverged`]) and the user
//! is told ([`NOTICE_KEPT_OWN`]: the cloud copy needs another password,
//! this device kept its own state). The new password of a genuine change
//! opens the copy on trial as before.
//!
//! The unlock installs the session only if the vault file on disk is still
//! the write it opened (under the state lock, where a sync stores copies):
//! a copy a sync stored during the slow key derivation is opened again
//! (with the derived master key) and settled in turn – never confirmed in
//! place of the write that was opened.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use keystead_core::format::{self, VaultFile};
use keystead_core::model::{now_ms, VaultInfo};
use keystead_core::sync;
use keystead_core::vault::CopyCheck;
use keystead_core::{Error as CoreError, UnlockedVault, VaultStore};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::{log, Core};
use crate::sync_base::{self, sync_dir, valid_id};

/// Error code: this device's copy of a vault did not open although its key
/// did (forged or damaged), and there was no earlier copy to go back to –
/// it was removed; the folder's next copy brings the vault again.
pub const COPY_REJECTED: &str = "sync_copy_rejected";
/// Error code: the vault file changed again and again while it was being
/// unlocked (a sync stored newer copies meanwhile): unlock once more.
pub const UNLOCK_RACED: &str = "sync_unlock_raced";
/// `sync://notice` – payload [`SyncNotice`]: something an unlock did with a
/// copy on trial that the user has to know about (both platforms; the UI
/// shows it as a toast). Never only a log line.
pub const EVENT_NOTICE: &str = "sync://notice";
/// [`SyncNotice::kind`]: the copy this device had before (with edits the
/// folder's copy does not descend from, and another master password than
/// the one that opened) was put aside as a conflict copy – in
/// `<data>/conflicts/` and, on the phone, in the cloud folder too.
pub const NOTICE_KEPT_ASIDE: &str = "keptAside";
/// [`SyncNotice::kind`]: the folder's copy needs another master password
/// than the one typed (or the fingerprint's), which opened this device's
/// previous copy: that one was opened, the folder's copy becomes a
/// conflict (nothing is lost).
pub const NOTICE_KEPT_OWN: &str = "keptOwn";

/// A notice of the unlock about a copy on trial (`sync://notice`). No
/// secrets: the vault's id and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncNotice {
    pub kind: &'static str,
    pub vault_id: String,
    pub vault_name: String,
}

fn notify(core: &Core, kind: &'static str, vault: &UnlockedVault) {
    core.emit(
        EVENT_NOTICE,
        SyncNotice {
            kind,
            vault_id: vault.id().to_owned(),
            vault_name: vault.info().name,
        },
    );
}

/// One write of a vault file, from its plain header: the revision and the
/// payload's nonce (every write draws a fresh one). Not secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    pub revision: u64,
    pub nonce: String,
}

impl FileVersion {
    pub fn of(file: &VaultFile) -> Self {
        FileVersion {
            revision: file.revision,
            nonce: file.payload.nonce.clone(),
        }
    }

    pub fn of_session(vault: &UnlockedVault) -> Self {
        FileVersion {
            revision: vault.revision(),
            nonce: vault.payload_nonce().to_owned(),
        }
    }
}

/// What this device remembers about its copy of one vault beyond the file
/// itself (`<data>/sync/trial/<id>.json`, camelCase, no secrets; removed
/// once there is nothing to remember). A copy the sync stores is
/// **unconfirmed** until it opens on this device: the folder's plain header
/// – the revision included – is not authenticated without the keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    /// The copy in `vaults/` came from the folder and has not opened here
    /// yet: this write of it.
    #[serde(default)]
    pub unconfirmed: Option<FileVersion>,
    /// While `unconfirmed`: the revision of the copy it replaced – the last
    /// one that opened here (or was there before), kept beside this file as
    /// `<id>.keystead`. `None`: there was none (a new vault, a damaged
    /// copy).
    #[serde(default)]
    pub kept_revision: Option<u64>,
    /// A write of the folder's copy that did not open with the vault's keys
    /// (forged or damaged): not taken again.
    #[serde(default)]
    pub rejected: Option<FileVersion>,
    /// A write of the folder's copy that opened here but does not descend
    /// from the copy it replaced (its sync proof does not name it): the
    /// kept copy came back, and the two are a conflict for the user – this
    /// write is never taken on trial again ([`settle`]).
    #[serde(default)]
    pub diverged: Option<FileVersion>,
    /// The unconfirmed copy is one the user chose in a conflict ("take the
    /// other side's state"): it is confirmed when it opens, whatever its
    /// ancestry ([`settle`] does not put the kept copy back).
    #[serde(default)]
    pub chosen: bool,
}

/// Held while a trial file and the vault files it describes change. Taken
/// after the state lock, never the other way round.
static TRIAL: Mutex<()> = Mutex::new(());

pub fn trial_lock() -> MutexGuard<'static, ()> {
    TRIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn trial_dir(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("trial")
}

/// `<data>/sync/trial/<id>.json`.
pub fn trial_path(data_dir: &Path, id: &str) -> PathBuf {
    trial_dir(data_dir).join(format!("{id}.json"))
}

/// `<data>/sync/trial/<id>.keystead`: the copy an unconfirmed one replaced.
pub fn kept_path(data_dir: &Path, id: &str) -> PathBuf {
    trial_dir(data_dir).join(format!("{id}.{}", format::VAULT_EXTENSION))
}

fn remove_if_present(path: &Path) -> AppResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

impl Trial {
    /// Read leniently: none, or one that cannot be read, remembers nothing.
    pub fn load(data_dir: &Path, id: &str) -> Trial {
        match fs::read(trial_path(data_dir, id)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log(format_args!("a sync trial record is unreadable: {e}"));
                Trial::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Trial::default(),
            Err(e) => {
                log(format_args!("could not read a sync trial record: {e}"));
                Trial::default()
            }
        }
    }

    /// Writes it atomically – or, with nothing to remember, removes it; the
    /// kept copy goes once nothing refers to it.
    pub fn save(&self, data_dir: &Path, id: &str) -> AppResult<()> {
        if self.kept_revision.is_none() {
            remove_if_present(&kept_path(data_dir, id))?;
        }
        if *self == Trial::default() {
            return remove_if_present(&trial_path(data_dir, id));
        }
        fs::create_dir_all(trial_dir(data_dir))?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(CoreError::from)?;
        bytes.push(b'\n');
        format::write_atomic(&trial_path(data_dir, id), &bytes, false)?;
        Ok(())
    }
}

/// Before the sync replaces this device's copy of `id` at `path` with the
/// folder's copy `incoming` (unconfirmed): a confirmed copy in place is
/// kept, so an unlock can go back to it; an unconfirmed one is just
/// replaced (the copy kept before it stays). Call with the trial lock; the
/// caller then writes the copy and records the sync base for its head.
pub fn begin_trial(data_dir: &Path, id: &str, path: &Path, incoming: &VaultFile) -> AppResult<()> {
    let mut trial = Trial::load(data_dir, id);
    if trial.unconfirmed.is_none() {
        let current = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        trial.kept_revision = match current.and_then(|bytes| {
            let file = VaultFile::parse(&bytes).ok().filter(|f| f.id == id)?;
            Some((file.revision, bytes))
        }) {
            Some((revision, bytes)) => {
                fs::create_dir_all(trial_dir(data_dir))?;
                format::write_atomic(&kept_path(data_dir, id), &bytes, false)?;
                Some(revision)
            }
            // A damaged copy is nothing to go back to.
            None => None,
        };
    }
    trial.unconfirmed = Some(FileVersion::of(incoming));
    trial.rejected = None;
    trial.diverged = None;
    trial.chosen = false;
    trial.save(data_dir, id)
}

/// [`begin_trial`] for a copy the user chose in a conflict: confirmed when
/// it opens, without the ancestry check ([`Trial::chosen`]).
pub fn begin_chosen_trial(
    data_dir: &Path,
    id: &str,
    path: &Path,
    incoming: &VaultFile,
) -> AppResult<()> {
    begin_trial(data_dir, id, path, incoming)?;
    let mut trial = Trial::load(data_dir, id);
    trial.chosen = true;
    trial.save(data_dir, id)
}

/// The open vault `vault` just saved a new write of its own (an edit, a
/// recovery-key unlock): a copy on trial is no longer what the file holds,
/// so the trial ends (best effort). The copy it kept is not dropped unless
/// the saved write descends from it: otherwise it is kept as a conflict
/// copy in `<data>/conflicts/` first. Call with the state lock
/// (`Core::mutate_with`) or before the vault is installed, not with the
/// trial lock.
pub fn after_save(data_dir: &Path, vault: &UnlockedVault) {
    let id = vault.id();
    if !valid_id(id) {
        return;
    }
    let saved = FileVersion::of_session(vault);
    let _trial = trial_lock();
    let mut trial = Trial::load(data_dir, id);
    if trial.unconfirmed.is_none() || trial.unconfirmed.as_ref() == Some(&saved) {
        return;
    }
    if trial.kept_revision.is_some() {
        if let Ok(kept) = fs::read(kept_path(data_dir, id)) {
            let descends = VaultFile::parse(&kept)
                .is_ok_and(|file| vault.descends_from(&sync::head_of(&file)));
            if !descends {
                match crate::sync_conflict::keep_conflict_copy(data_dir, id, &kept, None, now_ms())
                {
                    Ok(path) => log(format_args!(
                        "a sync trial ended: kept its previous copy as {}",
                        path.display()
                    )),
                    // Then the trial (and its kept copy) stays.
                    Err(e) => {
                        log(format_args!("could not keep a trial's copy: {}", e.code()));
                        return;
                    }
                }
            }
        }
    }
    trial.unconfirmed = None;
    trial.kept_revision = None;
    trial.chosen = false;
    if let Err(e) = trial.save(data_dir, id) {
        log(format_args!("could not end a sync trial: {}", e.code()));
    }
}

/// Remembers `version` of the folder's copy of `id` as rejected (best
/// effort). Call with the trial lock.
pub fn remember_rejected(data_dir: &Path, id: &str, version: FileVersion) {
    let mut trial = Trial::load(data_dir, id);
    trial.rejected = Some(version);
    if let Err(e) = trial.save(data_dir, id) {
        log(format_args!(
            "could not remember a rejected copy: {}",
            e.code()
        ));
    }
}

/// Whether this device's copy of vault `id` came from the folder and has
/// not opened here yet: its header (the salt included) is unproven, so the
/// fingerprint unlock's setup is not judged by it. Only if the vault file
/// on disk is that unconfirmed write: a record whose write never reached
/// the disk (the process ended between [`begin_trial`] and the write) is
/// stale – it is cleaned up when the file on disk is still the copy it
/// kept (or it kept none), and never counts.
// The PC asks before it pulls into a copy on trial (stage 2b).
#[cfg_attr(not(any(mobile, test)), expect(dead_code))]
pub fn on_trial(data_dir: &Path, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    let _trial = trial_lock();
    let mut trial = Trial::load(data_dir, id);
    let Some(unconfirmed) = trial.unconfirmed.clone() else {
        return false;
    };
    let on_disk = match fs::read(VaultStore::new(data_dir).vault_path(id)) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        // Cannot tell right now: as recorded.
        Err(_) => return true,
    };
    let version = on_disk
        .as_deref()
        .and_then(|bytes| VaultFile::parse(bytes).ok())
        .map(|file| FileVersion::of(&file));
    if version.as_ref() == Some(&unconfirmed) {
        return true;
    }
    // Stale. The kept copy goes only if nothing is lost with it: the file
    // on disk is that very copy, or there is none (otherwise a save ended
    // the trial without keeping it yet – `after_save` keeps it next time).
    let kept = trial
        .kept_revision
        .and_then(|_| fs::read(kept_path(data_dir, id)).ok());
    let nothing_lost = match (&kept, &on_disk) {
        (Some(kept), Some(disk)) => kept == disk,
        (Some(_), None) => false,
        (None, _) => trial.kept_revision.is_none(),
    };
    if nothing_lost {
        trial.unconfirmed = None;
        trial.kept_revision = None;
        trial.chosen = false;
        match trial.save(data_dir, id) {
            Ok(()) => log("removed a sync trial record whose copy never reached the disk"),
            Err(e) => log(format_args!(
                "could not remove a stale sync trial record: {}",
                e.code()
            )),
        }
    }
    false
}

/// Under the state lock, right before the unlock installs `vault` (vault
/// `id`, file `path`): whether the file on disk is still the write the
/// session opened – a sync may have stored another copy during the key
/// derivation (then `None`: the caller opens it again). If that write is
/// the unconfirmed copy ([`settle`] has checked it), it is confirmed and
/// the kept copy goes (`Some(true)`, best effort).
fn confirm_if_current(
    data_dir: &Path,
    path: &Path,
    id: &str,
    vault: &UnlockedVault,
) -> Option<bool> {
    let opened = FileVersion::of_session(vault);
    let _trial = trial_lock();
    let on_disk = VaultFile::read(path).ok().map(|f| FileVersion::of(&f));
    if on_disk.as_ref() != Some(&opened) {
        return None;
    }
    if !valid_id(id) {
        return Some(false);
    }
    let mut trial = Trial::load(data_dir, id);
    if trial.unconfirmed.as_ref() != Some(&opened) {
        return Some(false);
    }
    trial.unconfirmed = None;
    trial.kept_revision = None;
    trial.chosen = false;
    if let Err(e) = trial.save(data_dir, id) {
        log(format_args!(
            "could not confirm a synced copy: {}",
            e.code()
        ));
    }
    Some(true)
}

/// The vault `vault` just opened as a copy on trial: confirmed below only
/// if it descends from the copy it replaced (see the module docs). Returns
/// the session to install – `vault`, or the kept copy's (opened with the
/// same master key) after it was put back.
fn settle(core: &Core, id: &str, vault: UnlockedVault) -> AppResult<UnlockedVault> {
    if !valid_id(id) {
        return Ok(vault);
    }
    let data_dir = core.data_dir();
    let store = core.store();
    let path = store.vault_path(id);
    let opened = FileVersion::of_session(&vault);
    let kept_bytes = {
        let _trial = trial_lock();
        let trial = Trial::load(&data_dir, id);
        if trial.unconfirmed.as_ref() != Some(&opened)
            || trial.kept_revision.is_none()
            || trial.chosen
        {
            // Not the copy on trial, nothing it replaced (a vault new to
            // this device), or one the user chose: nothing to descend from.
            return Ok(vault);
        }
        match fs::read(kept_path(&data_dir, id)) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(vault),
        }
    };
    let Some(kept) = VaultFile::parse(&kept_bytes).ok().filter(|f| f.id == id) else {
        return Ok(vault);
    };
    let kept_head = sync::head_of(&kept);
    if vault.descends_from(&kept_head) {
        return Ok(vault);
    }
    if vault.check_copy(&kept) != CopyCheck::Opens {
        // The kept copy has another master password (the one the user no
        // longer has to type): it never wins, but it is not dropped – it
        // is kept as a conflict copy (on the phone also written into the
        // cloud folder by the next sync, where the PC offers to restore
        // it), and the user is told.
        let kept_at =
            crate::sync_conflict::keep_conflict_copy(&data_dir, id, &kept_bytes, None, now_ms())?;
        log(format_args!(
            "a synced copy with a new master password does not descend from this device's: kept {}",
            kept_at.display()
        ));
        #[cfg(any(mobile, test))]
        crate::folder_sync::queue_folder_copy(&data_dir, id, &kept_at);
        notify(core, NOTICE_KEPT_ASIDE, &vault);
        return Ok(vault);
    }
    {
        let _st = core.state();
        let _trial = trial_lock();
        let on_disk = VaultFile::read(&path).ok().map(|f| FileVersion::of(&f));
        if on_disk.as_ref() != Some(&opened) {
            // Replaced meanwhile: the install check opens it again.
            return Ok(vault);
        }
        format::write_atomic(&path, &kept_bytes, false)?;
        let mut trial = Trial::load(&data_dir, id);
        trial.unconfirmed = None;
        trial.kept_revision = None;
        trial.diverged = Some(opened);
        trial.chosen = false;
        trial.save(&data_dir, id)?;
        if let Err(e) = sync_base::reset_to(&data_dir, id, Some(&kept_head)) {
            log(format_args!("could not reset a sync base: {}", e.code()));
        }
    }
    log(
        "a synced copy does not descend from this device's: kept this device's, a conflict follows",
    );
    let kek = vault.master_kek();
    drop(vault);
    Ok(store.unlock_with_master_kek(id, &kek)?)
}

/// What [`kept_copy_opens`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeptCopy {
    /// Not on trial, nothing kept, or the kept copy does not open either:
    /// the password is just wrong.
    DoesNotOpen,
    /// The kept copy opens and has the same KDF salt and parameters: the
    /// same key must have opened the copy on trial – its key wrapping is
    /// forged (or damaged).
    SameKey,
    /// The kept copy opens, but the copy on trial has another salt (or
    /// parameters): it needs another master password than the one this
    /// device's previous copy has. A genuine password change on the other
    /// side – or a copy someone put into the folder.
    OtherPassword,
}

/// An unlock of vault `id` with `open` failed with `wrong_password`: does
/// `open` (the typed password, or the fingerprint's master KEK) open the
/// copy the one on trial replaced? The kept copy is opened from a scratch
/// store (`<data>/sync/probe`, removed again) and dropped at once.
fn kept_copy_opens(
    core: &Core,
    id: &str,
    open: &impl Fn(&VaultStore) -> Result<UnlockedVault, CoreError>,
) -> KeptCopy {
    if !valid_id(id) {
        return KeptCopy::DoesNotOpen;
    }
    let data_dir = core.data_dir();
    let probe = sync_dir(&data_dir).join("probe");
    let (staged, same_key) = {
        let _trial = trial_lock();
        let trial = Trial::load(&data_dir, id);
        let (Some(unconfirmed), Some(_)) = (trial.unconfirmed, trial.kept_revision) else {
            return KeptCopy::DoesNotOpen;
        };
        let Ok(current) = VaultFile::read(&core.store().vault_path(id)) else {
            return KeptCopy::DoesNotOpen;
        };
        let Ok(kept_bytes) = fs::read(kept_path(&data_dir, id)) else {
            return KeptCopy::DoesNotOpen;
        };
        let Ok(kept) = VaultFile::parse(&kept_bytes) else {
            return KeptCopy::DoesNotOpen;
        };
        if FileVersion::of(&current) != unconfirmed || kept.id != id {
            return KeptCopy::DoesNotOpen;
        }
        let same_key =
            kept.kdf.salt == current.kdf.salt && kept.kdf_params() == current.kdf_params();
        let store = VaultStore::new(&probe);
        let staged = fs::create_dir_all(store.vaults_dir())
            .and_then(|()| fs::write(store.vault_path(id), &kept_bytes))
            .is_ok();
        (staged, same_key)
    };
    let opens = staged && open(&VaultStore::new(&probe)).is_ok();
    if let Err(e) = fs::remove_dir_all(&probe) {
        log(format_args!("could not remove the sync probe: {e}"));
    }
    match (opens, same_key) {
        (false, _) => KeptCopy::DoesNotOpen,
        (true, true) => KeptCopy::SameKey,
        (true, false) => KeptCopy::OtherPassword,
    }
}

/// What [`reject_unconfirmed`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fallback {
    /// The copy that failed is not one on trial (or changed meanwhile).
    NotOnTrial,
    /// The kept copy is back in place.
    Restored,
    /// There was no copy to go back to: the failed one was removed.
    Removed,
}

/// Why [`reject_unconfirmed`] goes back from the copy on trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reject {
    /// It is forged or damaged: never taken again ([`Trial::rejected`]).
    Forged,
    /// It needs another master password than the one that opens the kept
    /// copy: the kept copy is this device's state again and the two are a
    /// conflict for the user ([`Trial::diverged`]) – never on trial again.
    OtherPassword,
}

/// An unlock of vault `id` failed with `corrupt` – its key opened, its
/// payload did not (or the file is damaged) – or with a key wrapping
/// [`kept_copy_opens`] proved forged, or with a password that opens the
/// kept copy but not the copy on trial (`why`). If that is the unconfirmed
/// copy, the kept copy is put back – or, with none, the copy is removed –
/// and that write is remembered as rejected or diverged. The vault's sync
/// base follows: the kept copy's head, or none.
fn reject_unconfirmed(core: &Core, id: &str, why: Reject) -> AppResult<Fallback> {
    if !valid_id(id) {
        return Ok(Fallback::NotOnTrial);
    }
    let data_dir = core.data_dir();
    let path = core.store().vault_path(id);
    // Under the state lock: no sync stores a copy meanwhile.
    let st = core.state();
    if st.vault.as_ref().is_some_and(|vault| vault.id() == id) {
        return Ok(Fallback::NotOnTrial);
    }
    let _trial = trial_lock();
    let mut trial = Trial::load(&data_dir, id);
    let Some(version) = trial.unconfirmed.clone() else {
        return Ok(Fallback::NotOnTrial);
    };
    // Still that copy? (A sync may have stored a newer one since the
    // unlock read it.)
    if let Ok(file) = VaultFile::read(&path) {
        if FileVersion::of(&file) != version {
            return Ok(Fallback::NotOnTrial);
        }
    }
    let kept = trial
        .kept_revision
        .and_then(|_| fs::read(kept_path(&data_dir, id)).ok());
    let (fallback, head) = match kept {
        Some(bytes) => {
            format::write_atomic(&path, &bytes, false)?;
            let head = VaultFile::parse(&bytes).ok().map(|f| sync::head_of(&f));
            (Fallback::Restored, head)
        }
        None => {
            remove_if_present(&path)?;
            (Fallback::Removed, None)
        }
    };
    trial.unconfirmed = None;
    trial.kept_revision = None;
    match why {
        Reject::Forged => trial.rejected = Some(version),
        Reject::OtherPassword => trial.diverged = Some(version),
    }
    trial.chosen = false;
    trial.save(&data_dir, id)?;
    if let Err(e) = sync_base::reset_to(&data_dir, id, head.as_ref()) {
        log(format_args!("could not reset a sync base: {}", e.code()));
    }
    Ok(fallback)
}

/// Unlocks vault `vault_id` and makes it the open vault
/// (`Core::install_vault_since`; a lock meanwhile wins) – `open` is the
/// master password's or the fingerprint's way to open it from the store,
/// called once more after a fallback. A copy on trial that opens is
/// confirmed. One that does not although its key opened (`corrupt`: forged
/// or damaged), or whose key wrapping the kept copy proves forged
/// ([`kept_copy_opens`]: same salt, and the kept copy opens), is rejected:
/// the copy it replaced is put back and opened instead, or – without one –
/// it is removed (`unsupported:sync_copy_rejected`). One with another salt
/// that `open` does not open while it opens the kept copy (the password of
/// this device's previous copy – a copy in the folder with a password the
/// user does not know must not lock them out) is not taken either: the kept
/// copy is put back and opened, the folder's write becomes a conflict
/// ([`Trial::diverged`]) and the user is told ([`NOTICE_KEPT_OWN`]). The
/// new password of a genuine change still opens the copy on trial. A vault
/// that is not on trial opens (or fails) as with a plain unlock.
pub fn unlock_copy(
    core: &Core,
    vault_id: &str,
    open: impl Fn(&VaultStore) -> Result<UnlockedVault, CoreError>,
) -> AppResult<VaultInfo> {
    let (epoch, store) = core.unlock_ticket();
    let mut vault = match open(&store) {
        Ok(vault) => vault,
        Err(e) => {
            let why = match &e {
                CoreError::Corrupt(_) => Some(Reject::Forged),
                CoreError::WrongPassword => match kept_copy_opens(core, vault_id, &open) {
                    KeptCopy::SameKey => Some(Reject::Forged),
                    KeptCopy::OtherPassword => Some(Reject::OtherPassword),
                    KeptCopy::DoesNotOpen => None,
                },
                _ => None,
            };
            let Some(why) = why else {
                return Err(e.into());
            };
            match reject_unconfirmed(core, vault_id, why)? {
                Fallback::Restored if why == Reject::OtherPassword => {
                    log("a synced vault copy needs another master password: this device's previous copy is back, a conflict follows");
                    let vault = open(&store)?;
                    notify(core, NOTICE_KEPT_OWN, &vault);
                    vault
                }
                Fallback::Restored => {
                    log("a synced vault copy did not open: the previous copy is back");
                    open(&store)?
                }
                Fallback::Removed => {
                    log("a synced vault copy did not open and was removed");
                    return Err(AppError::unsupported(COPY_REJECTED));
                }
                Fallback::NotOnTrial => return Err(e.into()),
            }
        }
    };
    let data_dir = core.data_dir();
    let path = store.vault_path(vault_id);
    for _ in 0..3 {
        vault = settle(core, vault_id, vault)?;
        let salt = vault.kdf_salt().to_owned();
        let mut confirmed = false;
        let installed = core.install_vault_checked(vault, epoch, |v| {
            match confirm_if_current(&data_dir, &path, vault_id, v) {
                Some(c) => {
                    confirmed = c;
                    true
                }
                None => false,
            }
        })?;
        match installed {
            Ok(info) => {
                if confirmed {
                    // A new salt: the fingerprint unlock's sealed KEK is stale.
                    #[cfg(any(mobile, test))]
                    crate::device_unlock::forget_if_stale(&data_dir, vault_id, &salt);
                }
                #[cfg(not(any(mobile, test)))]
                let _ = salt;
                return Ok(info);
            }
            Err(stale) => {
                // A sync stored another copy while the key was derived: that
                // one is opened (with the derived master key) and settled.
                let kek = stale.master_kek();
                drop(stale);
                vault = store.unlock_with_master_kek(vault_id, &kek)?;
            }
        }
    }
    Err(AppError::unsupported(UNLOCK_RACED))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keystead_core::model::{ItemType, VaultItem};
    use keystead_core::settings::Settings;
    use keystead_core::sync::{SyncBase, SyncHead};
    use keystead_core::KdfParams;

    use super::*;

    const TREE: &str = "content://tree/cloud";

    /// A device with one vault (password `pw`) that opened here, and the
    /// "folder side" store that writes newer copies of it.
    struct Setup {
        _dir: tempfile::TempDir,
        core: Arc<Core>,
        other: UnlockedVault,
        id: String,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let other_store = VaultStore::new(dir.path().join("other"));
        let mut other = other_store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        other
            .save_item(VaultItem::new(ItemType::Note, "eins"))
            .unwrap();
        let store = VaultStore::new(dir.path().join("device"));
        fs::create_dir_all(store.vaults_dir()).unwrap();
        fs::copy(other.path(), store.vault_path(other.id())).unwrap();
        let core = Core::for_tests(
            store,
            Settings {
                browser_integration: false,
                ..Settings::default()
            },
        );
        let id = other.id().to_owned();
        Setup {
            _dir: dir,
            core,
            other,
            id,
        }
    }

    /// Stores `bytes` as the device's copy of the vault the way a sync does
    /// without the open vault: on trial, the base set to its head.
    fn store_on_trial(s: &Setup, bytes: &[u8]) {
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let file = VaultFile::parse(bytes).unwrap();
        let _trial = trial_lock();
        begin_trial(&data_dir, &s.id, &path, &file).unwrap();
        fs::write(&path, bytes).unwrap();
        sync_base::save(
            &data_dir,
            &s.id,
            &SyncBase::new(&sync::head_of(&file), TREE, 1),
        )
        .unwrap();
    }

    fn base_head(s: &Setup) -> Option<(u64, String)> {
        sync_base::load(&s.core.data_dir(), &s.id, TREE).map(|b| (b.revision, b.digest))
    }

    fn head_of_bytes(bytes: &[u8]) -> SyncHead {
        sync::head_of(&VaultFile::parse(bytes).unwrap())
    }

    fn unlock(s: &Setup, password: &str) -> AppResult<VaultInfo> {
        unlock_copy(&s.core, &s.id, |store| store.unlock(&s.id, password))
    }

    #[test]
    fn a_plain_unlock_is_unchanged() {
        let s = setup();
        let data_dir = s.core.data_dir();
        // Not on trial: wrong passwords and corrupt files fail as always,
        // nothing is created under `sync/`.
        assert_eq!(unlock(&s, "falsch").unwrap_err().code(), "wrong_password");
        assert_eq!(
            unlock_copy(&s.core, "unknown", |st| st.unlock("unknown", "pw"))
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert!(!on_trial(&data_dir, &s.id));
        let info = unlock(&s, "pw").unwrap();
        assert_eq!(info.id, s.id);
        assert!(!sync_dir(&data_dir).exists());
        s.core.lock(None);
        // `Core::unlock` takes the same path on every platform.
        assert_eq!(s.core.unlock(&s.id, "pw").unwrap().id, s.id);
        assert!(!sync_dir(&data_dir).exists());
    }

    #[test]
    fn a_copy_on_trial_keeps_the_copy_that_opened_until_it_opens() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();

        // Two newer copies arrive before the device unlocks: the copy that
        // opened is kept (not the first unconfirmed one).
        s.other
            .save_item(VaultItem::new(ItemType::Note, "zwei"))
            .unwrap();
        store_on_trial(&s, &fs::read(s.other.path()).unwrap());
        s.other
            .save_item(VaultItem::new(ItemType::Note, "drei"))
            .unwrap();
        let newest = fs::read(s.other.path()).unwrap();
        store_on_trial(&s, &newest);
        assert!(on_trial(&data_dir, &s.id));
        assert_eq!(fs::read(kept_path(&data_dir, &s.id)).unwrap(), confirmed);
        let trial = Trial::load(&data_dir, &s.id);
        assert_eq!(trial.kept_revision, Some(2));
        assert_eq!(trial.unconfirmed.unwrap().revision, 4);

        // It opens: confirmed, the kept copy goes, the base stays its head.
        unlock(&s, "pw").unwrap();
        assert!(!on_trial(&data_dir, &s.id));
        assert!(!trial_path(&data_dir, &s.id).exists());
        assert!(!kept_path(&data_dir, &s.id).exists());
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 3);
        let head = head_of_bytes(&newest);
        assert_eq!(base_head(&s), Some((head.revision, head.digest)));
    }

    #[test]
    fn a_forged_copy_on_trial_brings_back_the_kept_copy_and_its_base() {
        let s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();

        // A raised revision with a payload that does not open.
        let mut forged = VaultFile::read(&path).unwrap();
        forged.revision = 99;
        let c = &mut forged.payload.ciphertext;
        let first = if c.starts_with('A') { "B" } else { "A" };
        c.replace_range(0..1, first);
        let forged_bytes = forged.to_json().unwrap();
        store_on_trial(&s, &forged_bytes);
        assert_eq!(base_head(&s).unwrap().0, 99);

        // The unlock finds it broken: the kept copy is back and opens, the
        // write is remembered as rejected, the base is the kept copy's head.
        unlock(&s, "pw").unwrap();
        assert_eq!(fs::read(&path).unwrap(), confirmed);
        let trial = Trial::load(&data_dir, &s.id);
        assert_eq!(trial.unconfirmed, None);
        assert_eq!(trial.rejected, Some(FileVersion::of(&forged)));
        assert!(!kept_path(&data_dir, &s.id).exists());
        let head = head_of_bytes(&confirmed);
        assert_eq!(base_head(&s), Some((head.revision, head.digest)));
        // The next copy that goes on trial clears the rejection.
        s.core.lock(None);
        store_on_trial(&s, &confirmed);
        assert_eq!(Trial::load(&data_dir, &s.id).rejected, None);
    }

    #[test]
    fn a_forged_first_copy_is_removed_with_its_base() {
        let s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        // The device never had this vault: the forged copy is the first.
        let mut forged = VaultFile::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        forged.revision = 50;
        let c = &mut forged.payload.ciphertext;
        let first = if c.starts_with('A') { "B" } else { "A" };
        c.replace_range(0..1, first);
        store_on_trial(&s, &forged.to_json().unwrap());
        assert_eq!(Trial::load(&data_dir, &s.id).kept_revision, None);

        let err = unlock(&s, "pw").unwrap_err();
        assert_eq!(err.code(), format!("unsupported:{COPY_REJECTED}"));
        assert!(!path.exists());
        assert_eq!(base_head(&s), None, "no copy left: no base");
        assert!(!sync_base::base_path(&data_dir, &s.id).exists());
        assert!(s.core.state().vault.is_none());
    }

    #[test]
    fn a_forged_key_wrapping_is_proved_by_the_kept_copy() {
        let s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();
        let mut forged = VaultFile::read(&path).unwrap();
        forged.revision = 7;
        let c = &mut forged.wrapped_key.ciphertext;
        let first = if c.starts_with('A') { "B" } else { "A" };
        c.replace_range(0..1, first);
        store_on_trial(&s, &forged.to_json().unwrap());

        // A wrong password stays a wrong password.
        assert_eq!(unlock(&s, "falsch").unwrap_err().code(), "wrong_password");
        assert!(on_trial(&data_dir, &s.id));
        assert!(!sync_dir(&data_dir).join("probe").exists());
        // The right one opens the kept copy: the copy on trial is forged.
        unlock(&s, "pw").unwrap();
        assert_eq!(fs::read(&path).unwrap(), confirmed);
        assert!(!on_trial(&data_dir, &s.id));
        assert!(!sync_dir(&data_dir).join("probe").exists());
        let head = head_of_bytes(&confirmed);
        assert_eq!(base_head(&s), Some((head.revision, head.digest)));
    }

    #[test]
    fn invalid_ids_are_never_file_names() {
        let s = setup();
        let data_dir = s.core.data_dir();
        for id in ["../x", "a/b", ""] {
            assert!(!on_trial(&data_dir, id));
            assert!(unlock_copy(&s.core, id, |st| st.unlock(id, "pw")).is_err());
        }
        assert!(!sync_dir(&data_dir).exists());
    }

    /// Review H1: a sync stores a newer copy on trial while an unlock
    /// derives its key. The unlock does not install the write it opened
    /// (and confirm the trial for it): it opens the one now on disk and
    /// settles that.
    #[test]
    fn a_copy_stored_during_the_unlock_is_the_one_installed() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        s.other
            .save_item(VaultItem::new(ItemType::Note, "zwei"))
            .unwrap();
        let newer = fs::read(s.other.path()).unwrap();
        let stored = std::cell::Cell::new(false);
        let info = unlock_copy(&s.core, &s.id, |store| {
            let opened = store.unlock(&s.id, "pw");
            if !stored.replace(true) {
                // The sync, during the key derivation.
                store_on_trial(&s, &newer);
            }
            opened
        })
        .unwrap();
        assert_eq!(info.id, s.id);
        assert_eq!(fs::read(&path).unwrap(), newer);
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 2);
        assert_eq!(
            s.core.read(FileVersion::of_session).unwrap(),
            FileVersion::of(&VaultFile::parse(&newer).unwrap())
        );
        assert!(!on_trial(&data_dir, &s.id), "it descends: confirmed");
        // An edit saves on top of it (no conflict, nothing on trial).
        s.core
            .mutate(|v| v.save_item(VaultItem::new(ItemType::Note, "drei")))
            .unwrap();
        assert_eq!(Trial::load(&data_dir, &s.id), Trial::default());
    }

    /// A save of the open vault ends whatever trial is recorded for it.
    #[test]
    fn a_save_of_the_open_vault_ends_the_trial() {
        let s = setup();
        let data_dir = s.core.data_dir();
        unlock(&s, "pw").unwrap();
        let stale = Trial {
            unconfirmed: Some(FileVersion {
                revision: 50,
                nonce: "AAAA".into(),
            }),
            kept_revision: Some(1),
            ..Trial::default()
        };
        stale.save(&data_dir, &s.id).unwrap();
        // The copy that trial kept (not an ancestor of what is saved now).
        let kept = b"{ an older copy }".to_vec();
        fs::write(kept_path(&data_dir, &s.id), &kept).unwrap();
        // Not what the file holds: never "on trial" – but the record and
        // its kept copy stay (the kept copy is not the file on disk).
        assert!(!on_trial(&data_dir, &s.id));
        assert!(Trial::load(&data_dir, &s.id).unconfirmed.is_some());
        assert!(kept_path(&data_dir, &s.id).exists());
        s.core
            .mutate(|v| v.save_item(VaultItem::new(ItemType::Note, "neu")))
            .unwrap();
        assert!(!on_trial(&data_dir, &s.id));
        assert!(!kept_path(&data_dir, &s.id).exists());
        // Never dropped: kept as a conflict copy.
        let copies = crate::sync_conflict::list(&data_dir, Some(&s.id)).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0].path).unwrap(), kept);
    }

    /// A copy stored on trial while locked that opens but does not descend
    /// from the copy it replaced: the kept copy comes back (opened with the
    /// same master key), the base with it, and that write is remembered as
    /// diverged – the sync records the conflict.
    #[test]
    fn a_copy_on_trial_that_does_not_descend_brings_back_the_kept_copy() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();
        // A write of the other side whose proof is gone (a 2.0.x writer).
        s.other
            .save_item(VaultItem::new(ItemType::Note, "zwei"))
            .unwrap();
        let mut stripped = VaultFile::read(s.other.path()).unwrap();
        stripped.extra.0.remove(sync::SYNC_PROOF_KEY);
        let stripped_bytes = stripped.to_json().unwrap();
        store_on_trial(&s, &stripped_bytes);

        let info = unlock(&s, "pw").unwrap();
        assert_eq!(info.id, s.id);
        assert_eq!(fs::read(&path).unwrap(), confirmed);
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 1);
        let trial = Trial::load(&data_dir, &s.id);
        assert_eq!(trial.unconfirmed, None);
        assert_eq!(trial.diverged, Some(FileVersion::of(&stripped)));
        assert!(!kept_path(&data_dir, &s.id).exists());
        let head = head_of_bytes(&confirmed);
        assert_eq!(base_head(&s), Some((head.revision, head.digest)));
    }

    /// … unless the kept copy has another master password (the other side
    /// changed it): the copy is confirmed, the kept copy kept as a conflict
    /// copy – never dropped.
    #[test]
    fn a_kept_copy_with_the_old_password_is_kept_as_a_conflict_copy() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();
        s.other.change_master_password("pw", "neu").unwrap();
        let mut stripped = VaultFile::read(s.other.path()).unwrap();
        stripped.extra.0.remove(sync::SYNC_PROOF_KEY);
        let stripped_bytes = stripped.to_json().unwrap();
        store_on_trial(&s, &stripped_bytes);

        unlock(&s, "neu").unwrap();
        assert_eq!(fs::read(&path).unwrap(), stripped_bytes);
        assert!(!on_trial(&data_dir, &s.id));
        let copies = crate::sync_conflict::list(&data_dir, Some(&s.id)).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0].path).unwrap(), confirmed);
        // Review M1: never only a log line – the user is told, and the
        // phone's next sync writes the copy into the folder.
        assert!(s.core.emitted().contains(&(
            EVENT_NOTICE.to_owned(),
            serde_json::json!({ "kind": NOTICE_KEPT_ASIDE, "vaultId": s.id, "vaultName": "Privat" })
        )));
        let outbox = fs::read_to_string(sync_dir(&data_dir).join("outbox.json")).unwrap();
        let name = copies[0].path.file_name().unwrap().to_str().unwrap();
        assert!(outbox.contains(name), "{outbox}");
    }

    /// Review M2: a copy on trial with a password of someone else's (a new
    /// salt, a higher revision) never locks the user out: the password of
    /// this device's previous copy – typed or the fingerprint's key – opens
    /// that copy, the folder's write becomes a conflict, the user is told.
    #[test]
    fn a_copy_that_needs_another_password_never_locks_the_user_out() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();
        let kek = s.other.master_kek();
        s.other.change_master_password("pw", "fremd").unwrap();
        let foreign = fs::read(s.other.path()).unwrap();
        let foreign_version = FileVersion::of(&VaultFile::parse(&foreign).unwrap());
        store_on_trial(&s, &foreign);

        // A password that opens neither stays a wrong password.
        assert_eq!(unlock(&s, "falsch").unwrap_err().code(), "wrong_password");
        assert!(on_trial(&data_dir, &s.id));
        // The real one opens this device's copy.
        let info = unlock(&s, "pw").unwrap();
        assert_eq!(info.id, s.id);
        assert_eq!(fs::read(&path).unwrap(), confirmed);
        assert_eq!(s.core.read(|o| o.items().len()).unwrap(), 1);
        let trial = Trial::load(&data_dir, &s.id);
        assert_eq!(trial.unconfirmed, None);
        assert_eq!(trial.diverged, Some(foreign_version.clone()));
        assert_eq!(trial.rejected, None, "a conflict, not a rejection");
        assert!(!kept_path(&data_dir, &s.id).exists());
        let head = head_of_bytes(&confirmed);
        assert_eq!(base_head(&s), Some((head.revision, head.digest)));
        assert!(s.core.emitted().contains(&(
            EVENT_NOTICE.to_owned(),
            serde_json::json!({ "kind": NOTICE_KEPT_OWN, "vaultId": s.id, "vaultName": "Privat" })
        )));
        assert!(!sync_dir(&data_dir).join("probe").exists());

        // The same with the fingerprint's key (the master KEK).
        s.core.lock(None);
        store_on_trial(&s, &foreign);
        unlock_copy(&s.core, &s.id, |st| st.unlock_with_master_kek(&s.id, &kek)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), confirmed);
        assert_eq!(
            Trial::load(&data_dir, &s.id).diverged,
            Some(foreign_version)
        );

        // A genuine new password still opens the copy on trial.
        s.core.lock(None);
        store_on_trial(&s, &foreign);
        unlock(&s, "fremd").unwrap();
        assert_eq!(fs::read(&path).unwrap(), foreign);
        assert!(!on_trial(&data_dir, &s.id));
    }

    /// Review L3: the process ended between `begin_trial` and the write of
    /// the copy. The record is stale: never "on trial", and cleaned up
    /// (the kept copy is the file on disk – nothing is lost).
    #[test]
    fn a_trial_record_whose_copy_never_reached_the_disk_is_stale() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        let confirmed = fs::read(&path).unwrap();
        s.other
            .save_item(VaultItem::new(ItemType::Note, "zwei"))
            .unwrap();
        let newer = VaultFile::read(s.other.path()).unwrap();
        {
            let _trial = trial_lock();
            begin_trial(&data_dir, &s.id, &path, &newer).unwrap();
        }
        assert!(kept_path(&data_dir, &s.id).exists());
        assert!(!on_trial(&data_dir, &s.id));
        assert_eq!(Trial::load(&data_dir, &s.id), Trial::default());
        assert!(!kept_path(&data_dir, &s.id).exists());
        assert_eq!(fs::read(&path).unwrap(), confirmed);

        // A first copy that never arrived (nothing kept): stale as well.
        fs::remove_file(&path).unwrap();
        {
            let _trial = trial_lock();
            begin_trial(&data_dir, &s.id, &path, &newer).unwrap();
        }
        assert!(!on_trial(&data_dir, &s.id));
        assert_eq!(Trial::load(&data_dir, &s.id), Trial::default());
    }

    /// A copy the user chose in a conflict is confirmed whatever its
    /// ancestry.
    #[test]
    fn a_chosen_copy_is_confirmed_without_the_ancestry_check() {
        let mut s = setup();
        let data_dir = s.core.data_dir();
        let path = s.core.store().vault_path(&s.id);
        s.other
            .save_item(VaultItem::new(ItemType::Note, "zwei"))
            .unwrap();
        let mut stripped = VaultFile::read(s.other.path()).unwrap();
        stripped.extra.0.remove(sync::SYNC_PROOF_KEY);
        let bytes = stripped.to_json().unwrap();
        {
            let _trial = trial_lock();
            begin_chosen_trial(&data_dir, &s.id, &path, &stripped).unwrap();
            fs::write(&path, &bytes).unwrap();
        }
        unlock(&s, "pw").unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(Trial::load(&data_dir, &s.id), Trial::default());
    }
}
