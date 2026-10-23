//! The sync base of each vault (two-way cloud-folder sync, Block 5 stage
//! 2b): the last head of the vault both this side and the linked folder
//! agreed on (`keystead_core::sync::SyncBase`), kept as
//! `<data>/sync/base/<id>.json` (camelCase, no secrets). Platform-neutral:
//! the phone's folder sync and the PC's mirror keep one each.
//!
//! * A base is recorded only for a head this side just wrote to the folder,
//!   or just read from it and took or found equal to its own copy.
//! * It belongs to one folder (`folder`: the phone's tree URI, the PC's
//!   mirror folder): a base recorded for another folder counts as none
//!   ([`load`]), so linking another folder starts without bases while
//!   linking the same one again keeps them.
//! * Read leniently: a missing, unreadable or damaged file is no base (the
//!   sync then treats differing copies as diverged – it never guesses).

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::format::{self, VaultFile};
use keystead_core::model::now_ms;
use keystead_core::sync::{self, RemoteProof, SyncBase, SyncDecision, SyncHead};
use keystead_core::vault::CopyCheck;
use keystead_core::{Error as CoreError, UnlockedVault};

use crate::error::{AppError, AppResult};
use crate::state::{log, Core};

/// `<data>/sync`: the sync's own files (link, bases, copies on trial).
pub fn sync_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sync")
}

/// A vault id as the app names its vault files – nothing else becomes a
/// file name under `<data>/sync` (an id may come from the page or the
/// folder).
pub fn valid_id(id: &str) -> bool {
    sync::vault_id_of_file_name(&format!("{id}.{}", format::VAULT_EXTENSION)) == Some(id)
}

fn base_dir(data_dir: &Path) -> PathBuf {
    sync_dir(data_dir).join("base")
}

/// `<data>/sync/base/<id>.json`.
pub fn base_path(data_dir: &Path, id: &str) -> PathBuf {
    base_dir(data_dir).join(format!("{id}.json"))
}

/// The base of vault `id` for `folder`, `None` without one – or with one
/// recorded for another folder.
pub fn load(data_dir: &Path, id: &str, folder: &str) -> Option<SyncBase> {
    read(data_dir, id).filter(|base| base.folder == folder)
}

/// The base of vault `id`, whatever folder it was recorded for. Lenient: a
/// missing or unreadable file is none (logged).
fn read(data_dir: &Path, id: &str) -> Option<SyncBase> {
    if !valid_id(id) {
        return None;
    }
    let bytes = match fs::read(base_path(data_dir, id)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log(format_args!("could not read a sync base: {e}"));
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(base) => Some(base),
        Err(e) => {
            log(format_args!("a sync base is unreadable: {e}"));
            None
        }
    }
}

/// Records `base` for vault `id` (atomic write).
pub fn save(data_dir: &Path, id: &str, base: &SyncBase) -> AppResult<()> {
    if !valid_id(id) {
        return Err(AppError::invalid("vault id"));
    }
    fs::create_dir_all(base_dir(data_dir))?;
    let mut bytes = serde_json::to_vec_pretty(base).map_err(CoreError::from)?;
    bytes.push(b'\n');
    format::write_atomic(&base_path(data_dir, id), &bytes, false)?;
    Ok(())
}

/// Removes the base of vault `id` (none is fine).
pub fn remove(data_dir: &Path, id: &str) -> AppResult<()> {
    if !valid_id(id) {
        return Ok(());
    }
    match fs::remove_file(base_path(data_dir, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// `sync::decide` for this side's copy `local` and the folder's `remote`
/// of one vault (both there), the same on both sides. The ancestry comes
/// from the open vault when it is this one: this side's verified lineage
/// (if the session is that write; else what this side's own file claims)
/// and the folder copy's proof verified with the session's master key
/// ([`remote_proof`]). While the vault is locked the folder's proof cannot
/// be checked (`Unchecked`: a pull only goes on trial). `extra_local`: more
/// of this side's own earlier writes (the PC's `.bak`).
pub fn decide_copies(
    core: &Core,
    local: &VaultFile,
    remote: &VaultFile,
    base: Option<&SyncBase>,
    extra_local: &[String],
) -> SyncDecision {
    let local_head = sync::head_of(local);
    let remote_head = sync::head_of(remote);
    let (mut local_ancestors, verified, unchecked) = {
        let st = core.state();
        match st.vault.as_ref().filter(|v| v.id() == local.id) {
            Some(vault) => {
                let ancestors = if vault.head() == local_head {
                    vault.lineage().to_vec()
                } else {
                    sync::claimed_ancestors(local)
                };
                match remote_proof(vault, remote) {
                    Ok(verified) => (ancestors, Some(verified), false),
                    Err(unchecked) => (ancestors, None, unchecked),
                }
            }
            None => (sync::claimed_ancestors(local), None, true),
        }
    };
    local_ancestors.extend(extra_local.iter().cloned());
    let proof = match (&verified, unchecked) {
        (Some(ancestors), _) => RemoteProof::Verified(ancestors),
        (None, true) => RemoteProof::Unchecked,
        (None, false) => RemoteProof::Unproven,
    };
    sync::decide(
        Some(&local_head),
        Some(&remote_head),
        base,
        &local_ancestors,
        proof,
    )
}

/// The folder copy `remote`'s verified ancestry with the open `vault`'s
/// keys; `Err(true)` if the session cannot tell (another master password:
/// unchecked), `Err(false)` if it can and there is none (no proof, one that
/// does not verify, or a copy its keys prove forged).
pub fn remote_proof(vault: &UnlockedVault, remote: &VaultFile) -> Result<Vec<String>, bool> {
    match vault.verified_lineage_of(remote) {
        Some(ancestors) => Ok(ancestors),
        None => Err(vault.check_copy(remote) == CopyCheck::OtherPassword),
    }
}

/// Whether the folder copy `remote` descends from `from` as far as the open
/// `vault`'s keys can prove it: a higher revision and `from` in its
/// verified ancestry.
pub fn descends(vault: &UnlockedVault, remote: &VaultFile, from: &SyncHead) -> bool {
    remote.revision > from.revision
        && vault
            .verified_lineage_of(remote)
            .is_some_and(|ancestors| ancestors.contains(&from.digest))
}

/// After this side went back from a copy it had taken from the folder
/// (a copy on trial that did not open): the base of vault `id` becomes
/// `head` – the copy now in place – for the folder it was recorded for, or
/// goes (`head` `None`: no copy left). Without a base nothing is recorded:
/// there is no folder to record it for.
pub fn reset_to(data_dir: &Path, id: &str, head: Option<&SyncHead>) -> AppResult<()> {
    let Some(base) = read(data_dir, id) else {
        return remove(data_dir, id);
    };
    match head {
        Some(head) => save(data_dir, id, &SyncBase::new(head, &base.folder, now_ms())),
        None => remove(data_dir, id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "29a9e366-e89d-4f3f-8c15-814a2200224b";
    const TREE: &str = "content://tree/cloud";

    fn head(revision: u64, digest: &str) -> SyncHead {
        SyncHead {
            id: ID.into(),
            revision,
            digest: digest.into(),
        }
    }

    #[test]
    fn round_trip_per_folder() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert_eq!(load(data, ID, TREE), None, "none yet");

        let base = SyncBase::new(&head(7, "d7"), TREE, 42);
        save(data, ID, &base).unwrap();
        assert!(data.join("sync/base").join(format!("{ID}.json")).is_file());
        assert_eq!(load(data, ID, TREE), Some(base.clone()));
        // Recorded for another folder: none for this one.
        assert_eq!(load(data, ID, "content://tree/other"), None);
        // Another vault has none.
        assert_eq!(load(data, "other-vault", TREE), None);

        // camelCase, no secrets: only these keys.
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(base_path(data, ID)).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "revision": 7, "digest": "d7", "folder": TREE, "syncedAt": 42 })
        );

        // Replaced by the next one; removed (twice is fine).
        let next = SyncBase::new(&head(8, "d8"), TREE, 43);
        save(data, ID, &next).unwrap();
        assert_eq!(load(data, ID, TREE), Some(next));
        remove(data, ID).unwrap();
        assert_eq!(load(data, ID, TREE), None);
        remove(data, ID).unwrap();
    }

    #[test]
    fn lenient_read_and_invalid_ids() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        fs::create_dir_all(base_dir(data)).unwrap();
        // Unknown keys are ignored.
        fs::write(
            base_path(data, ID),
            format!(
                r#"{{"revision":3,"digest":"d","folder":"{TREE}","syncedAt":1,"future":true}}"#
            ),
        )
        .unwrap();
        assert_eq!(load(data, ID, TREE).unwrap().revision, 3);
        // Damaged or incomplete: no base.
        for bad in [&b"{ not json"[..], b"", br#"{"revision":3}"#, b"[1,2]"] {
            fs::write(base_path(data, ID), bad).unwrap();
            assert_eq!(load(data, ID, TREE), None, "{bad:?}");
        }
        // Nothing but a vault id becomes a file name.
        for id in ["../x", "a/b", "", "x.json"] {
            assert_eq!(load(data, id, TREE), None);
            assert!(save(data, id, &SyncBase::new(&head(1, "d"), TREE, 1)).is_err());
            remove(data, id).unwrap();
        }
        assert!(!data.join("x.json").exists());
    }

    #[test]
    fn copies_are_decided_with_the_open_vaults_keys() {
        use keystead_core::settings::Settings;
        use keystead_core::{KdfParams, VaultStore};

        let dir = tempfile::tempdir().unwrap();
        // The other device: writes the folder's copies.
        let other_store = VaultStore::new(dir.path().join("other"));
        let mut other = other_store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let store = VaultStore::new(dir.path().join("here"));
        fs::create_dir_all(store.vaults_dir()).unwrap();
        fs::copy(other.path(), store.vault_path(other.id())).unwrap();
        let core = Core::for_tests(store.clone(), Settings::default());
        let local = VaultFile::read(&store.vault_path(other.id())).unwrap();
        let base = SyncBase::new(&sync::head_of(&local), TREE, 1);
        other.save().unwrap();
        other.save().unwrap();
        let newer = VaultFile::read(other.path()).unwrap();

        // Locked: the newer copy can only go on trial.
        assert_eq!(
            decide_copies(&core, &local, &newer, Some(&base), &[]),
            SyncDecision::PullUnverified
        );
        // Open: its proof holds the base – a pull.
        core.state().vault = Some(store.unlock(other.id(), "pw").unwrap());
        assert_eq!(
            decide_copies(&core, &local, &newer, Some(&base), &[]),
            SyncDecision::PullRemote
        );
        // A raised revision of the same write is not the same write and
        // proves nothing.
        let mut raised = local.clone();
        raised.revision = 9;
        assert_eq!(
            decide_copies(&core, &local, &raised, Some(&base), &[]),
            SyncDecision::Diverged
        );
        // Its proof stripped (a 2.0.x writer): a conflict, not a pull.
        let mut bare = newer.clone();
        bare.extra.0.remove(sync::SYNC_PROOF_KEY);
        assert_eq!(
            decide_copies(&core, &local, &bare, Some(&base), &[]),
            SyncDecision::Diverged
        );
        let session = core.state().vault.take().unwrap();
        assert!(descends(&session, &newer, &sync::head_of(&local)));
        assert!(!descends(&session, &bare, &sync::head_of(&local)));
        assert!(!descends(&session, &local, &sync::head_of(&local)));
        // Another master password: unchecked even while open.
        other.change_master_password("pw", "neu").unwrap();
        let rekeyed = VaultFile::read(other.path()).unwrap();
        assert_eq!(remote_proof(&session, &rekeyed), Err(true));
        assert_eq!(remote_proof(&session, &bare), Err(false));
        core.state().vault = Some(session);
        assert_eq!(
            decide_copies(&core, &local, &rekeyed, Some(&base), &[]),
            SyncDecision::PullUnverified
        );
    }

    #[test]
    fn reset_keeps_the_folder_or_removes_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        // Without a base nothing is recorded.
        reset_to(data, ID, Some(&head(3, "d3"))).unwrap();
        assert!(!base_path(data, ID).exists());

        save(data, ID, &SyncBase::new(&head(9, "d9"), TREE, 1)).unwrap();
        reset_to(data, ID, Some(&head(3, "d3"))).unwrap();
        let base = load(data, ID, TREE).unwrap();
        assert_eq!((base.revision, base.digest.as_str()), (3, "d3"));
        assert!(base.synced_at > 1);

        // Another folder's base keeps its folder.
        save(data, ID, &SyncBase::new(&head(9, "d9"), "C:\\Cloud", 1)).unwrap();
        reset_to(data, ID, Some(&head(4, "d4"))).unwrap();
        assert_eq!(load(data, ID, "C:\\Cloud").unwrap().revision, 4);

        reset_to(data, ID, None).unwrap();
        assert!(!base_path(data, ID).exists());
    }
}
