//! Conflict copies of the two-way cloud-folder sync (Block 5 stage 2b).
//! When the user resolves a conflict, the losing side's vault file is not
//! dropped: it is kept as
//! `<data>/conflicts/<id>.keystead.conflict-<YYYYMMDD-HHMMSS>` (UTC; see
//! `keystead_core::sync::conflict_file_name`) on the side that resolved it
//! – never in the cloud folder, and not in `vaults/`, so no store lists it
//! as a vault. The user can import it again (file import of a
//! `.keystead` file). On the PC the losing side's blob folder is copied
//! beside it (`<copy>.attachments/<attachment id>.bin`), so attachments the
//! winning side no longer references are not lost with the hourly sweep.
//! Platform-neutral.

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::attachments;
use keystead_core::format;
use keystead_core::sync;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// A conflict of the two-way sync, for the UI (`sync_conflicts`,
/// `sync://conflict`): this side's copy of a vault and the folder's both
/// changed since they last agreed (`keystead_core::sync::decide` →
/// `Diverged`). No secrets: ids, names, revisions and times from the plain
/// headers.
// The PC records its own from work package 4 on.
#[cfg_attr(not(any(mobile, test)), expect(dead_code))]
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
}

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
/// blob files (`<attachment id>.bin`; nothing else) are copied into the
/// copy's own blob folder after the vault file; a blob folder that does not
/// exist is nothing to copy.
// Both sides' conflict resolution keeps the losing copy (stage 2b; the
// PC's mirror from work package 4 on).
#[cfg_attr(not(any(mobile, test)), expect(dead_code))]
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

/// Copies the blob files of `from` into `to` (created when there is one).
fn copy_blobs(from: &Path, to: &Path) -> AppResult<()> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if attachments::blob_id_of_file_name(name).is_none() || !entry.file_type()?.is_file() {
            continue;
        }
        fs::create_dir_all(to)?;
        fs::copy(entry.path(), to.join(name))?;
    }
    Ok(())
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

    #[test]
    fn a_conflict_copy_is_kept_with_its_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let blobs = dir.path().join("vault.keystead.attachments");
        fs::create_dir_all(&blobs).unwrap();
        let blob = "0b5d7c1e-8f2a-4d3b-9c6e-1a2b3c4d5e6f.bin";
        fs::write(blobs.join(blob), b"KSAT1 blob").unwrap();
        // Not blobs: left behind.
        fs::write(blobs.join(".x.bin.0123456789abcdef.tmp"), b"tmp").unwrap();
        fs::write(blobs.join("copy (1).bin"), b"copy").unwrap();
        fs::create_dir(blobs.join("sub.bin")).unwrap();

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
        assert_eq!(fs::read(copied.join(blob)).unwrap(), b"KSAT1 blob");
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
