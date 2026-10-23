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

use keystead_core::format;
use keystead_core::model::now_ms;
use keystead_core::sync::{self, SyncBase, SyncDecision, SyncHead};
use keystead_core::Error as CoreError;

use crate::error::{AppError, AppResult};
use crate::state::log;

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

/// `sync::decide` for two copies of one vault that are both there – with
/// one rule on top, the same on both sides: the same write with another
/// revision (a header changed by hand) is not "up to date": a raised
/// revision is pulled (and then fails the checks of a copy on trial or the
/// open vault's `check_copy`), a lowered one is older.
pub fn decide_pair(local: &SyncHead, remote: &SyncHead, base: Option<&SyncBase>) -> SyncDecision {
    if local.id == remote.id && local.digest == remote.digest && local.revision != remote.revision {
        return if remote.revision > local.revision {
            SyncDecision::PullRemote
        } else {
            SyncDecision::RemoteOlder
        };
    }
    sync::decide(Some(local), Some(remote), base)
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
    fn a_pair_with_the_same_write_under_another_revision() {
        let base = SyncBase::new(&head(5, "d5"), TREE, 1);
        let local = head(5, "d5");
        // A raised revision of the same write: pulled (the checks refuse it).
        assert_eq!(
            decide_pair(&local, &head(9, "d5"), Some(&base)),
            SyncDecision::PullRemote
        );
        assert_eq!(
            decide_pair(&local, &head(9, "d5"), None),
            SyncDecision::PullRemote
        );
        // A lowered one: older.
        assert_eq!(
            decide_pair(&local, &head(2, "d5"), Some(&base)),
            SyncDecision::RemoteOlder
        );
        // Everything else is `decide`'s.
        assert_eq!(
            decide_pair(&local, &head(5, "d5"), Some(&base)),
            SyncDecision::UpToDate
        );
        assert_eq!(
            decide_pair(&local, &head(6, "d6"), Some(&base)),
            SyncDecision::PullRemote
        );
        assert_eq!(
            decide_pair(&head(6, "l6"), &head(6, "d6"), Some(&base)),
            SyncDecision::Diverged
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
