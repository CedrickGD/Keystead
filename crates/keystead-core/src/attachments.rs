//! Encrypted file attachments of items ([`VaultData::attachments`]).
//!
//! The vault payload only holds the bookkeeping ([`AttachmentEntry`]: item,
//! name, size, date and the blob's own key); the file bytes are sealed into
//! one blob each in the side-car directory `<vault path>.attachments/`
//! (`vaults/<id>.keystead.attachments/<attachment id>.bin`).
//!
//! * Blob layout: 5 magic bytes `KSAT1`, a 24-byte XChaCha20-Poly1305 nonce,
//!   then the ciphertext (tag included) of the file bytes. The AAD
//!   `keystead:v1:attachment:<vault id>:<attachment id>` binds a blob to
//!   its vault and entry, so a blob moved to another slot fails to open.
//! * Every attachment has its own random 32-byte key stored (base64) in its
//!   entry: rotating the vault key re-encrypts no blob, and removing the
//!   entry makes the blob worthless – the file itself is deleted best
//!   effort afterwards ([`sweep`] catches what that misses).
//! * Entries of items that no longer exist are pruned on every save
//!   ([`prune`]); trashed items keep theirs.
//! * While the map is not empty the vault file carries format version 2
//!   ([`crate::format::version_for`]), so builds without the attachments
//!   code refuse the vault instead of dropping the keys.
//!
//! Pure helpers plus the file I/O of blobs; the vault methods live in
//! [`crate::UnlockedVault`] (`add_attachment`, `read_attachment`, …).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use zeroize::{Zeroize, Zeroizing};

use crate::crypto::{self, SecretKey, KEY_LEN, NONCE_LEN};
use crate::error::{Error, Result};
use crate::format;
use crate::model::{AttachmentEntry, AttachmentInfo, VaultData};

/// Largest file that can be attached (bytes): 20 MiB.
pub const ATTACHMENT_MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Maximum number of attachments per item.
pub const ATTACHMENTS_PER_ITEM_MAX: usize = 10;
/// Maximum length of an attachment name (characters).
pub const ATTACHMENT_NAME_MAX_CHARS: usize = 200;
/// Suffix of the side-car directory: `<vault path>.attachments`.
pub const DIR_SUFFIX: &str = ".attachments";
/// File extension of blobs (without dot).
pub const BLOB_EXTENSION: &str = "bin";
/// Unreferenced files younger than this are left alone by [`sweep`]:
/// another process (the TUI) may be in the middle of attaching.
pub const SWEEP_MIN_AGE: Duration = Duration::from_secs(60 * 60);

/// Magic bytes at the start of every blob.
const BLOB_MAGIC: &[u8; 5] = b"KSAT1";
/// Poly1305 tag length appended to the ciphertext.
const TAG_LEN: usize = 16;
/// Bytes a blob adds to the plaintext (magic, nonce, tag).
pub const BLOB_OVERHEAD: usize = BLOB_MAGIC.len() + NONCE_LEN + TAG_LEN;

/// `<vault path>.attachments`, e.g. `vaults/<id>.keystead.attachments`.
pub fn dir_for(vault_path: &Path) -> PathBuf {
    format::sibling(vault_path, DIR_SUFFIX)
}

/// `<dir>/<attachment id>.bin`.
pub fn blob_path(dir: &Path, attachment_id: &str) -> PathBuf {
    dir.join(format!("{attachment_id}.{BLOB_EXTENSION}"))
}

fn blob_aad(vault_id: &str, attachment_id: &str) -> Vec<u8> {
    format!("keystead:v1:attachment:{vault_id}:{attachment_id}").into_bytes()
}

/// Seals file bytes into a blob: magic, nonce, ciphertext.
pub fn seal_blob(
    key: &SecretKey,
    vault_id: &str,
    attachment_id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let sealed = crypto::seal(key, plaintext, &blob_aad(vault_id, attachment_id))?;
    let mut out = Vec::with_capacity(BLOB_MAGIC.len() + NONCE_LEN + sealed.ciphertext.len());
    out.extend_from_slice(BLOB_MAGIC);
    out.extend_from_slice(&sealed.nonce);
    out.extend_from_slice(&sealed.ciphertext);
    Ok(out)
}

/// Opens a blob written by [`seal_blob`]. `Error::Corrupt` if it is not a
/// blob, was tampered with, or belongs to another vault, attachment or key.
pub fn open_blob(
    key: &SecretKey,
    vault_id: &str,
    attachment_id: &str,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < BLOB_OVERHEAD || !blob.starts_with(BLOB_MAGIC) {
        return Err(Error::corrupt("not an attachment blob"));
    }
    let (nonce, ciphertext) = blob[BLOB_MAGIC.len()..].split_at(NONCE_LEN);
    crypto::open(key, nonce, ciphertext, &blob_aad(vault_id, attachment_id))
        .map_err(|_| Error::corrupt("attachment failed authentication"))
}

/// Decodes the base64 key of an entry. `Error::Corrupt` if it is not a
/// 32-byte key.
pub(crate) fn decode_key(entry: &AttachmentEntry) -> Result<SecretKey> {
    let mut raw = Zeroizing::new(crypto::b64_decode(&entry.key)?);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    if raw.len() != KEY_LEN {
        raw.zeroize();
        return Err(Error::corrupt("attachment key has wrong length"));
    }
    key.copy_from_slice(&raw);
    Ok(key)
}

/// Trims an attachment name and checks it is non-empty
/// (`attachment_name_required`) and at most [`ATTACHMENT_NAME_MAX_CHARS`]
/// characters (`attachment_name_too_long`).
pub fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid("attachment_name_required"));
    }
    if trimmed.chars().count() > ATTACHMENT_NAME_MAX_CHARS {
        return Err(Error::invalid("attachment_name_too_long"));
    }
    Ok(trimmed.to_owned())
}

/// The attachments of `item_id`, oldest first.
pub fn of_item(data: &VaultData, item_id: &str) -> Vec<AttachmentInfo> {
    let mut out: Vec<AttachmentInfo> = data
        .attachments
        .iter()
        .filter(|(_, e)| e.item_id == item_id)
        .map(|(id, e)| e.info(id))
        .collect();
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Removes the entries of items that no longer exist (trashed items keep
/// theirs). Returns the ids of the removed entries – their blobs are
/// worthless now and can be deleted.
pub fn prune(data: &mut VaultData) -> Vec<String> {
    if data.attachments.is_empty() {
        return Vec::new();
    }
    let items: BTreeSet<&str> = data.items.iter().map(|i| i.id.as_str()).collect();
    let orphaned: Vec<String> = data
        .attachments
        .iter()
        .filter(|(_, e)| !items.contains(e.item_id.as_str()))
        .map(|(id, _)| id.clone())
        .collect();
    for id in &orphaned {
        if let Some(mut entry) = data.attachments.remove(id) {
            wipe_entry(&mut entry);
        }
    }
    orphaned
}

/// Overwrites the keys and names of the attachment map before it is freed.
pub(crate) fn wipe(data: &mut VaultData) {
    for (_, mut entry) in std::mem::take(&mut data.attachments) {
        wipe_entry(&mut entry);
    }
}

/// Overwrites the key and the name of one entry.
pub(crate) fn wipe_entry(entry: &mut AttachmentEntry) {
    entry.key.zeroize();
    entry.name.zeroize();
}

/// Removes the blob files of `ids` (best effort; a missing file is fine).
pub(crate) fn remove_blobs(dir: &Path, ids: &[String]) {
    for id in ids {
        let _ = fs::remove_file(blob_path(dir, id));
    }
}

/// Deletes blobs (`*.bin`) no entry of `data` references and leftover
/// temporary files (`*.tmp`) in `dir`, but only those whose modification
/// time is at least [`SWEEP_MIN_AGE`] old – a younger file may belong to an
/// attachment another process is adding right now. Best effort: a missing
/// directory or an unreadable file counts as nothing to do. Returns how
/// many files were removed.
pub fn sweep(dir: &Path, data: &VaultData) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let referenced = match path.extension().and_then(|e| e.to_str()) {
            Some(BLOB_EXTENSION) => path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|id| data.attachments.contains_key(id)),
            Some("tmp") => false,
            _ => continue,
        };
        if referenced || !path.is_file() {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= SWEEP_MIN_AGE);
        if old_enough && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ItemType, VaultItem};

    fn entry(item_id: &str, name: &str, created_at: i64) -> AttachmentEntry {
        AttachmentEntry {
            item_id: item_id.into(),
            name: name.into(),
            size: 3,
            created_at,
            key: crypto::b64_encode(&[7u8; KEY_LEN]),
        }
    }

    #[test]
    fn blob_round_trip_and_tamper() {
        let key = crypto::random_key().unwrap();
        let blob = seal_blob(&key, "vault", "att", b"file bytes").unwrap();
        assert!(blob.starts_with(BLOB_MAGIC));
        assert_eq!(blob.len(), b"file bytes".len() + BLOB_OVERHEAD);
        let plain = open_blob(&key, "vault", "att", &blob).unwrap();
        assert_eq!(plain.as_slice(), b"file bytes");

        assert!(matches!(
            open_blob(&key, "vault", "other", &blob),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            open_blob(&key, "other", "att", &blob),
            Err(Error::Corrupt(_))
        ));
        let other = crypto::random_key().unwrap();
        assert!(matches!(
            open_blob(&other, "vault", "att", &blob),
            Err(Error::Corrupt(_))
        ));
        let mut flipped = blob.clone();
        flipped[BLOB_OVERHEAD - TAG_LEN] ^= 1;
        assert!(matches!(
            open_blob(&key, "vault", "att", &flipped),
            Err(Error::Corrupt(_))
        ));
        let mut magic = blob.clone();
        magic[0] = b'X';
        assert!(matches!(
            open_blob(&key, "vault", "att", &magic),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            open_blob(&key, "vault", "att", &blob[..BLOB_OVERHEAD - 1]),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            open_blob(&key, "vault", "att", b""),
            Err(Error::Corrupt(_))
        ));
        // An empty file is a valid attachment.
        let empty = seal_blob(&key, "vault", "att", b"").unwrap();
        assert_eq!(empty.len(), BLOB_OVERHEAD);
        assert!(open_blob(&key, "vault", "att", &empty).unwrap().is_empty());
    }

    #[test]
    fn keys_are_decoded_and_checked() {
        let key = crypto::random_key().unwrap();
        let e = AttachmentEntry {
            key: crypto::b64_encode(key.as_slice()),
            ..Default::default()
        };
        assert_eq!(*decode_key(&e).unwrap(), *key);
        let short = AttachmentEntry {
            key: crypto::b64_encode(&[1u8; 16]),
            ..Default::default()
        };
        assert!(matches!(decode_key(&short), Err(Error::Corrupt(_))));
        let garbage = AttachmentEntry {
            key: "!!".into(),
            ..Default::default()
        };
        assert!(matches!(decode_key(&garbage), Err(Error::Corrupt(_))));
    }

    #[test]
    fn names() {
        assert_eq!(validate_name("  scan.png ").unwrap(), "scan.png");
        assert_eq!(
            validate_name("").unwrap_err().code(),
            "invalid_input:attachment_name_required"
        );
        assert_eq!(
            validate_name("   ").unwrap_err().code(),
            "invalid_input:attachment_name_required"
        );
        let long = "ä".repeat(ATTACHMENT_NAME_MAX_CHARS);
        assert_eq!(validate_name(&long).unwrap(), long);
        assert_eq!(
            validate_name(&format!("{long}x")).unwrap_err().code(),
            "invalid_input:attachment_name_too_long"
        );
    }

    #[test]
    fn paths() {
        let dir = dir_for(Path::new("vaults/abc.keystead"));
        assert_eq!(dir, PathBuf::from("vaults/abc.keystead.attachments"));
        assert_eq!(
            blob_path(&dir, "a1"),
            PathBuf::from("vaults/abc.keystead.attachments/a1.bin")
        );
    }

    #[test]
    fn of_item_is_oldest_first_and_prune_keeps_the_trash() {
        let mut trashed = VaultItem::new(ItemType::Note, "t");
        trashed.id = "t".into();
        trashed.deleted_at = Some(1);
        let mut live = VaultItem::new(ItemType::Note, "l");
        live.id = "l".into();
        let mut data = VaultData {
            items: vec![trashed, live],
            ..Default::default()
        };
        data.attachments
            .insert("b".into(), entry("l", "second", 20));
        data.attachments.insert("a".into(), entry("l", "first", 10));
        data.attachments
            .insert("c".into(), entry("l", "same time", 20));
        data.attachments
            .insert("t1".into(), entry("t", "trashed", 5));
        data.attachments
            .insert("gone".into(), entry("missing", "orphan", 1));
        let names: Vec<String> = of_item(&data, "l").into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["first", "second", "same time"]);
        assert_eq!(of_item(&data, "t").len(), 1);
        assert!(of_item(&data, "nobody").is_empty());

        assert_eq!(prune(&mut data), ["gone"]);
        assert_eq!(data.attachments.len(), 4);
        assert!(prune(&mut data).is_empty());
        data.items.clear();
        let mut gone = prune(&mut data);
        gone.sort();
        assert_eq!(gone, ["a", "b", "c", "t1"]);
        assert!(data.attachments.is_empty());
    }

    #[test]
    fn wipe_blanks_keys_and_names() {
        let mut data = VaultData::default();
        data.attachments
            .insert("a".into(), entry("l", "secret.pdf", 1));
        let mut e = entry("l", "secret.pdf", 1);
        wipe_entry(&mut e);
        assert!(e.key.is_empty() && e.name.is_empty());
        wipe(&mut data);
        assert!(data.attachments.is_empty());
    }

    #[test]
    fn sweep_spares_young_and_referenced_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("v.keystead.attachments");
        assert_eq!(sweep(&dir, &VaultData::default()), 0, "no directory");
        fs::create_dir(&dir).unwrap();
        let mut data = VaultData::default();
        data.attachments
            .insert("kept".into(), entry("l", "kept", 1));
        let old = |name: &str| {
            let p = dir.join(name);
            fs::write(&p, b"x").unwrap();
            let f = fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::now() - SWEEP_MIN_AGE - Duration::from_secs(60))
                .unwrap();
            p
        };
        let kept = old("kept.bin");
        let orphan = old("orphan.bin");
        let stale_tmp = old(".orphan.bin.0123456789abcdef.tmp");
        let other = old("notes.txt");
        let fresh = dir.join("fresh.bin");
        fs::write(&fresh, b"x").unwrap();
        fs::create_dir(dir.join("sub.bin")).unwrap();

        assert_eq!(sweep(&dir, &data), 2);
        assert!(kept.exists() && fresh.exists() && other.exists());
        assert!(!orphan.exists() && !stale_tmp.exists());
        assert!(dir.join("sub.bin").is_dir());
        assert_eq!(sweep(&dir, &data), 0);
    }
}
