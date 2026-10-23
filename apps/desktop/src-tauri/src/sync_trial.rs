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
//! local edits: it is replaced or confirmed before anything is saved to it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use keystead_core::format::{self, VaultFile};
use keystead_core::model::VaultInfo;
use keystead_core::sync;
use keystead_core::{Error as CoreError, UnlockedVault, VaultStore};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::{log, Core};
use crate::sync_base::{self, sync_dir, valid_id};

/// Error code: this device's copy of a vault did not open although its key
/// did (forged or damaged), and there was no earlier copy to go back to –
/// it was removed; the folder's next copy brings the vault again.
pub const COPY_REJECTED: &str = "sync_copy_rejected";

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

    fn of_session(vault: &UnlockedVault) -> Self {
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
// The PC's mirror pull stores copies on trial too (stage 2b).
#[cfg_attr(not(any(mobile, test)), expect(dead_code))]
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
    trial.save(data_dir, id)
}

/// Remembers `version` of the folder's copy of `id` as rejected (best
/// effort). Call with the trial lock.
// The PC's mirror pull rejects forged copies too (stage 2b).
#[cfg_attr(not(any(mobile, test)), expect(dead_code))]
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
/// fingerprint unlock's setup is not judged by it.
// The PC asks before it pulls into a copy on trial (stage 2b).
#[cfg_attr(not(any(mobile, test)), expect(dead_code))]
pub fn on_trial(data_dir: &Path, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    let _trial = trial_lock();
    Trial::load(data_dir, id).unconfirmed.is_some()
}

/// Vault `id` opened on this device as `opened` (with KDF salt `salt`): if
/// that is its unconfirmed copy, it is confirmed and the kept copy goes
/// (best effort) – and on the phone a fingerprint setup for another salt is
/// stale now. Another write (a sync stored a newer copy meanwhile) stays on
/// trial.
fn confirm(data_dir: &Path, id: &str, opened: &FileVersion, salt: &str) {
    if !valid_id(id) {
        return;
    }
    {
        let _trial = trial_lock();
        let mut trial = Trial::load(data_dir, id);
        if trial.unconfirmed.as_ref() != Some(opened) {
            return;
        }
        trial.unconfirmed = None;
        trial.kept_revision = None;
        if let Err(e) = trial.save(data_dir, id) {
            log(format_args!(
                "could not confirm a synced copy: {}",
                e.code()
            ));
        }
    }
    #[cfg(any(mobile, test))]
    crate::device_unlock::forget_if_stale(data_dir, id, salt);
    // The desktop has no fingerprint unlock.
    #[cfg(not(any(mobile, test)))]
    let _ = salt;
}

/// An unlock of vault `id` with `open` failed with `wrong_password`. If the
/// copy is on trial, has the same KDF salt and parameters as the kept copy,
/// and `open` opens the kept copy, the same key must have opened the copy
/// on trial: its key wrapping is forged (or damaged). The kept copy is
/// opened from a scratch store (`<data>/sync/probe`, removed again) and
/// dropped at once.
fn kept_copy_opens(
    core: &Core,
    id: &str,
    open: &impl Fn(&VaultStore) -> Result<UnlockedVault, CoreError>,
) -> bool {
    if !valid_id(id) {
        return false;
    }
    let data_dir = core.data_dir();
    let probe = sync_dir(&data_dir).join("probe");
    let staged = {
        let _trial = trial_lock();
        let trial = Trial::load(&data_dir, id);
        let (Some(unconfirmed), Some(_)) = (trial.unconfirmed, trial.kept_revision) else {
            return false;
        };
        let Ok(current) = VaultFile::read(&core.store().vault_path(id)) else {
            return false;
        };
        let Ok(kept_bytes) = fs::read(kept_path(&data_dir, id)) else {
            return false;
        };
        let Ok(kept) = VaultFile::parse(&kept_bytes) else {
            return false;
        };
        if FileVersion::of(&current) != unconfirmed
            || kept.id != id
            || kept.kdf.salt != current.kdf.salt
            || kept.kdf_params() != current.kdf_params()
        {
            return false;
        }
        let store = VaultStore::new(&probe);
        fs::create_dir_all(store.vaults_dir())
            .and_then(|()| fs::write(store.vault_path(id), &kept_bytes))
            .is_ok()
    };
    let opens = staged && open(&VaultStore::new(&probe)).is_ok();
    if let Err(e) = fs::remove_dir_all(&probe) {
        log(format_args!("could not remove the sync probe: {e}"));
    }
    opens
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

/// An unlock of vault `id` failed with `corrupt` – its key opened, its
/// payload did not (or the file is damaged) – or with a key wrapping
/// [`kept_copy_opens`] proved forged. If that is the unconfirmed copy, it
/// is rejected (never taken again) and the kept copy put back, or – with
/// none – removed. The vault's sync base follows: the kept copy's head, or
/// none.
fn reject_unconfirmed(core: &Core, id: &str) -> AppResult<Fallback> {
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
    trial.rejected = Some(version);
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
/// it is removed (`unsupported:sync_copy_rejected`). A vault that is not on
/// trial opens (or fails) as with a plain unlock.
pub fn unlock_copy(
    core: &Core,
    vault_id: &str,
    open: impl Fn(&VaultStore) -> Result<UnlockedVault, CoreError>,
) -> AppResult<VaultInfo> {
    let (epoch, store) = core.unlock_ticket();
    let vault = match open(&store) {
        Ok(vault) => vault,
        Err(e) => {
            let forged = match &e {
                CoreError::Corrupt(_) => true,
                CoreError::WrongPassword => kept_copy_opens(core, vault_id, &open),
                _ => false,
            };
            if !forged {
                return Err(e.into());
            }
            match reject_unconfirmed(core, vault_id)? {
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
    confirm(
        &core.data_dir(),
        vault_id,
        &FileVersion::of_session(&vault),
        vault.kdf_salt(),
    );
    core.install_vault_since(vault, epoch)
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
}
