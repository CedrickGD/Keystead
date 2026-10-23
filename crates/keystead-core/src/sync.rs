//! Vault files as opaque bytes: what a file import and the cloud-folder
//! sync of the mobile app (Block 5) need to know about a `.keystead` file
//! without its master password – which vault it is and how new it is – and
//! what the read-only phone does with a copy it finds in its cloud folder
//! ([`decide_copy_in`]). For the two-way sync (stage 2b): which write of a
//! vault file a copy is ([`head_of`]), the last state both sides agreed on
//! ([`SyncBase`]) and what to do with a pair of copies ([`decide`]).

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::format::{VaultFile, VAULT_EXTENSION};

/// The largest vault file a synced folder may hand over (bytes): 64 MiB.
/// The cloud folder is untrusted input on both sides – the phone copies no
/// larger file in, and the PC's mirror reads no larger copy of the folder
/// (it replaces it without reading it).
pub const VAULT_FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The public header of a vault file: no secrets, readable without the
/// master password. Two files with the same `id` are copies of one vault;
/// the higher `revision` is the newer one (`updated_at` is only the wall
/// clock of that save).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHeader {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub updated_at: i64,
    /// File format version (`format::FORMAT_VERSION*`).
    pub version: u32,
}

/// Reads the header of a vault file given as bytes, without decrypting
/// anything (`VaultFile::parse`: structure and encodings are validated, the
/// keys are not touched). A damaged or truncated Keystead file →
/// `Error::Corrupt`; some other kind of file (another program's export, a
/// legacy VaultX file, random bytes) or a format version this build does
/// not know → `Error::Unsupported`.
pub fn header_of(bytes: &[u8]) -> Result<VaultHeader> {
    VaultFile::parse(bytes).map(|file| VaultHeader::from(&file))
}

impl From<&VaultFile> for VaultHeader {
    fn from(file: &VaultFile) -> Self {
        VaultHeader {
            id: file.id.clone(),
            name: file.name.clone(),
            revision: file.revision,
            updated_at: file.updated_at,
            version: file.version,
        }
    }
}

/// What the read-only phone does with a copy of a vault it found in its
/// linked cloud folder (copy-in, Block 5 stage 2a). Only the PC writes the
/// folder's copy; the phone never writes back, so a revision counter is
/// enough to tell old from new (no divergence is possible yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyIn {
    /// The phone has no copy of this vault yet: add it.
    Import,
    /// The folder's copy is newer (higher revision): replace the phone's.
    Update,
    /// The same revision: nothing to do.
    UpToDate,
    /// The folder's copy is older than the phone's – the PC went back to
    /// an older file (a restored backup) or the cloud app has not fetched
    /// the newest one yet. The phone keeps its copy: an older revision is
    /// never copied in (a rollback), and nothing is ever written back.
    RemoteOlder,
    /// The phone's file at that place belongs to another vault (another
    /// id): it is never touched.
    OtherVault,
}

/// The copy-in decision for the folder's copy `remote` against the
/// phone's copy of that vault, `local` (`None`: the phone has none). Pure:
/// headers only, no file access.
pub fn decide_copy_in(local: Option<&VaultHeader>, remote: &VaultHeader) -> CopyIn {
    let Some(local) = local else {
        return CopyIn::Import;
    };
    if local.id != remote.id {
        return CopyIn::OtherVault;
    }
    match remote.revision.cmp(&local.revision) {
        Ordering::Greater => CopyIn::Update,
        Ordering::Equal => CopyIn::UpToDate,
        Ordering::Less => CopyIn::RemoteOlder,
    }
}

/// The vault id a vault file in a synced folder is named after:
/// `<id>.keystead` with an id of letters, digits, `-` and `_` (at most
/// 64), as the app names its vault files. Any other name → `None`: a
/// renamed copy, a cloud app's conflict copy (`… (1).keystead`), a hidden
/// temporary file (`.<id>.keystead.<hex>.tmp`) or a backup (`.bak`).
pub fn vault_id_of_file_name(name: &str) -> Option<&str> {
    let id = name.strip_suffix(&format!(".{VAULT_EXTENSION}"))?;
    crate::store::is_valid_id(id).then_some(id)
}

/// The name of a conflict copy of vault `id` made at `at_ms` (ms since the
/// epoch, UTC): `<id>.keystead.conflict-<YYYYMMDD-HHMMSS>`. Not a vault
/// file name ([`vault_id_of_file_name`] → `None`): a store or a sync never
/// mistakes it for a vault.
pub fn conflict_file_name(id: &str, at_ms: i64) -> String {
    // `YYYY-MM-DDTHH:MM:SS.mmmZ` → `YYYYMMDD-HHMMSS`.
    let iso = crate::util::format_iso8601_ms(at_ms);
    let seconds = iso.split('.').next().unwrap_or_default();
    let digits: String = seconds.chars().filter(char::is_ascii_digit).collect();
    let (date, time) = digits.split_at(digits.len().saturating_sub(6));
    format!("{id}.{VAULT_EXTENSION}.conflict-{date}-{time}")
}

// ---------------------------------------------------------------------------
// Two-way sync (stage 2b)
// ---------------------------------------------------------------------------

/// Which write of a vault file a copy is: its id, its revision and a digest
/// of the encrypted parts every write changes. Two copies with the same
/// digest are the same write (the payload is encrypted under a fresh nonce
/// each time, so equal ciphertexts are no coincidence); the revision alone
/// is not enough – two sides that each saved once both have the next one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncHead {
    pub id: String,
    pub revision: u64,
    /// Lowercase hex SHA-256, see [`head_of`].
    pub digest: String,
}

/// The head of `file`. The digest is the lowercase hex SHA-256 of
/// `"keystead-sync:1\0" ‖ payload.ciphertext ‖ "\0" ‖ wrappedKey.nonce ‖
/// "\0" ‖ wrappedKey.ciphertext` – the base64 strings as they are in the
/// file. The plain header (name, times, revision, KDF) is not part of it:
/// what tells two writes apart is the ciphertext, and a re-keyed vault (a
/// new master password or vault key) has a new key wrapping too.
pub fn head_of(file: &VaultFile) -> SyncHead {
    let mut hasher = Sha256::new();
    hasher.update(b"keystead-sync:1\0");
    hasher.update(file.payload.ciphertext.as_bytes());
    hasher.update(b"\0");
    hasher.update(file.wrapped_key.nonce.as_bytes());
    hasher.update(b"\0");
    hasher.update(file.wrapped_key.ciphertext.as_bytes());
    SyncHead {
        id: file.id.clone(),
        revision: file.revision,
        digest: data_encoding::HEXLOWER.encode(&hasher.finalize()),
    }
}

/// The last head of a vault both sides agreed on: one this side just wrote
/// to the folder, or just read from it and took (or found equal to its
/// own). Kept per vault and per side (`<data>/sync/base/<id>.json`, see
/// docs/ARCHITECTURE.md); no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncBase {
    pub revision: u64,
    pub digest: String,
    /// The folder this base belongs to (the phone's tree URI, the PC's
    /// mirror folder): a base recorded for another folder says nothing
    /// about this one.
    pub folder: String,
    /// When it was recorded (ms since the epoch).
    pub synced_at: i64,
}

impl SyncBase {
    /// The base for `head`, agreed on with `folder` at `synced_at`.
    pub fn new(head: &SyncHead, folder: &str, synced_at: i64) -> Self {
        SyncBase {
            revision: head.revision,
            digest: head.digest.clone(),
            folder: folder.to_owned(),
            synced_at,
        }
    }

    /// Whether `head` is the write this base records.
    fn is(&self, head: &SyncHead) -> bool {
        head.revision == self.revision && head.digest == self.digest
    }
}

/// What the two-way sync does with one vault ([`decide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    /// The two copies are different vaults (other ids): neither is touched.
    OtherVault,
    /// Only the folder's copy changed since the base: take it.
    PullRemote,
    /// Only this side's copy changed since the base: write it to the folder.
    PushLocal,
    /// Both copies are the same write (or there are none): nothing to do
    /// but to record the base.
    UpToDate,
    /// Both copies changed since the base – or there is no base and they
    /// differ: a conflict for the user to resolve; nothing is merged.
    Diverged,
    /// Only the folder has the vault: the phone imports it, the PC offers it.
    NewRemote,
    /// Only this side has the vault and it was never synced: write it to
    /// the folder.
    NewLocal,
    /// Only this side has the vault, but it was synced before: the folder's
    /// copy was removed. It is not written again.
    RemoteGone,
    /// The folder's copy is older than the base while this side's copy is
    /// unchanged (a restored backup, a cloud app that has not fetched the
    /// newest file yet): reported, never taken.
    RemoteOlder,
}

/// The decision for this side's copy `local` and the folder's copy
/// `remote` of one vault (`None`: that side has none), given the `base` of
/// that vault (`None`: never synced with this folder). Pure: heads only.
///
/// Top to bottom: no copies → up to date; only the folder's → new remote;
/// only this side's → new local without a base, remote gone with one;
/// other ids → other vault; equal digests → up to date; no base → diverged;
/// local is the base and the remote's revision is below it → remote older;
/// local is the base and the remote's revision above it → pull; the remote
/// is the base and the local revision above it → push; anything else
/// (both changed, the same revision with another digest, …) → diverged.
pub fn decide(
    local: Option<&SyncHead>,
    remote: Option<&SyncHead>,
    base: Option<&SyncBase>,
) -> SyncDecision {
    let (local, remote) = match (local, remote) {
        (None, None) => return SyncDecision::UpToDate,
        (None, Some(_)) => return SyncDecision::NewRemote,
        (Some(_), None) if base.is_none() => return SyncDecision::NewLocal,
        (Some(_), None) => return SyncDecision::RemoteGone,
        (Some(local), Some(remote)) => (local, remote),
    };
    if local.id != remote.id {
        return SyncDecision::OtherVault;
    }
    if local.digest == remote.digest {
        return SyncDecision::UpToDate;
    }
    let Some(base) = base else {
        return SyncDecision::Diverged;
    };
    if base.is(local) && remote.revision < base.revision {
        SyncDecision::RemoteOlder
    } else if base.is(local) && remote.revision > base.revision {
        SyncDecision::PullRemote
    } else if base.is(remote) && local.revision > base.revision {
        SyncDecision::PushLocal
    } else {
        SyncDecision::Diverged
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::error::Error;
    use crate::model::{ItemType, VaultItem};
    use crate::{KdfParams, VaultStore};

    fn fixture(name: &str) -> Vec<u8> {
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn header_of_a_real_vault_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultStore::new(dir.path());
        let mut v = store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let bytes = fs::read(v.path()).unwrap();
        let h = header_of(&bytes).unwrap();
        assert_eq!(h.id, v.id());
        assert_eq!(h.name, "Privat");
        assert_eq!(h.revision, 1);
        assert_eq!(h.updated_at, v.info().updated_at);
        assert_eq!(h.version, crate::format::FORMAT_VERSION);

        // A save bumps the revision; a BOM is tolerated like everywhere.
        v.save_item(VaultItem::new(ItemType::Note, "n")).unwrap();
        let mut with_bom = b"\xef\xbb\xbf".to_vec();
        with_bom.extend(fs::read(v.path()).unwrap());
        let h2 = header_of(&with_bom).unwrap();
        assert_eq!(h2.id, h.id);
        assert_eq!(h2.revision, 2);
        assert!(h2.updated_at >= h.updated_at);

        // camelCase for the UI.
        let json = serde_json::to_value(&h2).unwrap();
        assert_eq!(json["updatedAt"], h2.updated_at);
        assert_eq!(json.as_object().unwrap().len(), 5);
    }

    #[test]
    fn damaged_vault_files_are_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultStore::new(dir.path());
        let v = store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let bytes = fs::read(v.path()).unwrap();
        // Truncated (half a download).
        let err = header_of(&bytes[..bytes.len() / 2]).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{err:?}");
        // A header field that fails validation.
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["payload"]["nonce"] = "AAAA".into();
        let err = header_of(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{err:?}");
        // Missing version.
        json["payload"]["nonce"] = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
            ["payload"]["nonce"]
            .clone();
        json.as_object_mut().unwrap().remove("version");
        let err = header_of(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{err:?}");
        // A future format version is not corrupt but unsupported.
        json["version"] = 99.into();
        let err = header_of(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    }

    fn header(id: &str, revision: u64) -> VaultHeader {
        VaultHeader {
            id: id.into(),
            name: "Privat".into(),
            revision,
            updated_at: 1_700_000_000_000,
            version: crate::format::FORMAT_VERSION,
        }
    }

    #[test]
    fn copy_in_decisions() {
        let remote = header("a", 7);
        // No copy on the phone yet: added.
        assert_eq!(decide_copy_in(None, &remote), CopyIn::Import);
        // Newer in the folder: replaced; equal: nothing; older: kept.
        assert_eq!(
            decide_copy_in(Some(&header("a", 6)), &remote),
            CopyIn::Update
        );
        assert_eq!(
            decide_copy_in(Some(&header("a", 1)), &remote),
            CopyIn::Update
        );
        assert_eq!(
            decide_copy_in(Some(&header("a", 7)), &remote),
            CopyIn::UpToDate
        );
        assert_eq!(
            decide_copy_in(Some(&header("a", 8)), &remote),
            CopyIn::RemoteOlder
        );
        assert_eq!(
            decide_copy_in(Some(&header("a", u64::MAX)), &remote),
            CopyIn::RemoteOlder
        );
        // Another vault at that place is never replaced, whatever its revision.
        for revision in [1, 7, 9] {
            assert_eq!(
                decide_copy_in(Some(&header("b", revision)), &remote),
                CopyIn::OtherVault
            );
        }
        // Only id and revision count: a renamed vault or another clock is still an update.
        let mut renamed = header("a", 8);
        renamed.name = "Familie".into();
        renamed.updated_at = 0;
        assert_eq!(
            decide_copy_in(Some(&header("a", 7)), &renamed),
            CopyIn::Update
        );
    }

    #[test]
    fn copy_in_of_real_files_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultStore::new(dir.path());
        let mut v = store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let phone = header_of(&fs::read(v.path()).unwrap()).unwrap();
        v.save_item(VaultItem::new(ItemType::Note, "n")).unwrap();
        let pc = fs::read(v.path()).unwrap();
        let remote = header_of(&pc).unwrap();
        assert_eq!(decide_copy_in(Some(&phone), &remote), CopyIn::Update);
        assert_eq!(decide_copy_in(Some(&remote), &phone), CopyIn::RemoteOlder);
        // Garbage never gets as far as a decision.
        assert!(header_of(&pc[..pc.len() - 10]).is_err());
        assert!(header_of(b"PK a zip").is_err());
    }

    #[test]
    fn vault_ids_of_file_names() {
        let id = "29a9e366-e89d-4f3f-8c15-814a2200224b";
        assert_eq!(vault_id_of_file_name(&format!("{id}.keystead")), Some(id));
        assert_eq!(
            vault_id_of_file_name("legacy_id_1.keystead"),
            Some("legacy_id_1")
        );
        for name in [
            "Privat.keystead.bak",
            ".keystead",
            "Privat (1).keystead",
            "Kopie von Privat.keystead",
            ".29a9e366.keystead.0123456789abcdef.tmp",
            "29a9e366.keystead.lock",
            "29a9e366.KEYSTEAD",
            "29a9e366.keystead.attachments",
            "../29a9e366.keystead",
            "a/b.keystead",
        ] {
            assert_eq!(vault_id_of_file_name(name), None, "{name}");
        }
        assert_eq!(
            vault_id_of_file_name(&format!("{}.keystead", "x".repeat(65))),
            None
        );
    }

    #[test]
    fn foreign_files_are_unsupported() {
        for garbage in [
            &b""[..],
            b"not a vault at all",
            b"\x00\x01\x02\xff",
            b"[1,2,3]",
            b"{\"format\":\"bitwarden\",\"items\":[]}",
        ] {
            let err = header_of(garbage).unwrap_err();
            assert!(matches!(err, Error::Unsupported(_)), "{garbage:?}: {err:?}");
        }
        // The legacy VaultX 1.x fixtures are JSON, but not Keystead vaults.
        for name in [
            "legacy_v1.json",
            "legacy_v1_empty.json",
            "legacy_v2.json",
            "legacy_recovery.json",
            "legacy_single_entry.json",
            "legacy_accounts.json",
            "legacy_accounts_single.json",
        ] {
            let err = header_of(&fixture(name)).unwrap_err();
            assert!(matches!(err, Error::Unsupported(_)), "{name}: {err:?}");
        }
    }

    #[test]
    fn conflict_file_names() {
        let id = "29a9e366-e89d-4f3f-8c15-814a2200224b";
        // 2026-10-23 14:05:09.999 UTC.
        let name = conflict_file_name(id, 1_792_764_309_999);
        assert_eq!(name, format!("{id}.keystead.conflict-20261023-140509"));
        assert_eq!(vault_id_of_file_name(&name), None, "never a vault file");
        assert_eq!(
            conflict_file_name("a", 0),
            "a.keystead.conflict-19700101-000000"
        );
    }

    fn head(id: &str, revision: u64, digest: &str) -> SyncHead {
        SyncHead {
            id: id.into(),
            revision,
            digest: digest.into(),
        }
    }

    fn base(revision: u64, digest: &str) -> SyncBase {
        SyncBase::new(&head("a", revision, digest), "content://tree/cloud", 1)
    }

    #[test]
    fn sync_decisions() {
        use SyncDecision::*;
        let b = base(5, "b5");
        let at_base = head("a", 5, "b5");

        // Missing copies.
        assert_eq!(decide(None, None, None), UpToDate);
        assert_eq!(decide(None, None, Some(&b)), UpToDate);
        assert_eq!(decide(None, Some(&at_base), None), NewRemote);
        assert_eq!(decide(None, Some(&at_base), Some(&b)), NewRemote);
        assert_eq!(decide(Some(&at_base), None, None), NewLocal);
        assert_eq!(decide(Some(&at_base), None, Some(&b)), RemoteGone);
        assert_eq!(
            decide(Some(&head("a", 9, "l9")), None, Some(&b)),
            RemoteGone,
            "edited here, removed there: still not written again"
        );

        // Another vault, whatever the base or digests say.
        let other = head("b", 5, "b5");
        for base in [None, Some(&b)] {
            assert_eq!(decide(Some(&at_base), Some(&other), base), OtherVault);
            assert_eq!(decide(Some(&other), Some(&at_base), base), OtherVault);
        }

        // The same write on both sides.
        assert_eq!(decide(Some(&at_base), Some(&at_base), Some(&b)), UpToDate);
        let both = head("a", 9, "x9");
        assert_eq!(decide(Some(&both), Some(&both), None), UpToDate);
        assert_eq!(
            decide(Some(&both), Some(&both), Some(&b)),
            UpToDate,
            "equal copies beyond the base: the base is refreshed"
        );

        // No base and different copies: never guessed.
        assert_eq!(
            decide(Some(&at_base), Some(&head("a", 9, "r9")), None),
            Diverged
        );
        assert_eq!(
            decide(Some(&head("a", 9, "l9")), Some(&at_base), None),
            Diverged
        );

        // Only the folder changed: pull; only this side: push.
        let remote_new = head("a", 6, "r6");
        assert_eq!(
            decide(Some(&at_base), Some(&remote_new), Some(&b)),
            PullRemote
        );
        assert_eq!(
            decide(Some(&at_base), Some(&head("a", u64::MAX, "rx")), Some(&b)),
            PullRemote
        );
        let local_new = head("a", 6, "l6");
        assert_eq!(
            decide(Some(&local_new), Some(&at_base), Some(&b)),
            PushLocal
        );

        // The folder went back below the base while this side is unchanged.
        let remote_old = head("a", 3, "r3");
        assert_eq!(
            decide(Some(&at_base), Some(&remote_old), Some(&b)),
            RemoteOlder
        );

        // Both changed: the lost-update case.
        assert_eq!(
            decide(Some(&local_new), Some(&remote_new), Some(&b)),
            Diverged
        );
        assert_eq!(
            decide(Some(&head("a", 7, "l7")), Some(&remote_new), Some(&b)),
            Diverged
        );
        // This side changed and the folder went back: not a push over it.
        assert_eq!(
            decide(Some(&local_new), Some(&remote_old), Some(&b)),
            Diverged
        );
        // The same revision with another digest: one side rewrote it.
        assert_eq!(
            decide(Some(&at_base), Some(&head("a", 5, "r5")), Some(&b)),
            Diverged
        );
        assert_eq!(
            decide(Some(&head("a", 5, "l5")), Some(&at_base), Some(&b)),
            Diverged
        );
        // This side went back below the base (a restored backup here).
        assert_eq!(
            decide(Some(&head("a", 4, "l4")), Some(&at_base), Some(&b)),
            Diverged
        );
        // A base that is neither copy.
        let stale = base(2, "b2");
        assert_eq!(
            decide(Some(&at_base), Some(&remote_new), Some(&stale)),
            Diverged
        );
        // The base's digest counts, not only its revision.
        let same_revision = base(5, "other");
        assert_eq!(
            decide(Some(&at_base), Some(&remote_new), Some(&same_revision)),
            Diverged
        );
        assert_eq!(
            decide(Some(&local_new), Some(&at_base), Some(&same_revision)),
            Diverged
        );
    }

    #[test]
    fn heads_of_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultStore::new(dir.path());
        let mut v = store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let first = VaultFile::read(v.path()).unwrap();
        let h1 = head_of(&first);
        assert_eq!(h1.id, v.id());
        assert_eq!(h1.revision, 1);
        assert_eq!(h1.digest.len(), 64);
        assert!(h1
            .digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        // Stable: the same file, read again or serialised and parsed, has
        // the same head.
        assert_eq!(head_of(&VaultFile::read(v.path()).unwrap()), h1);
        assert_eq!(
            head_of(&VaultFile::parse(&first.to_json().unwrap()).unwrap()),
            h1
        );
        // The plain header is not part of the digest.
        let mut renamed = first.clone();
        renamed.name = "Familie".into();
        renamed.updated_at += 1000;
        assert_eq!(head_of(&renamed).digest, h1.digest);

        // Every save is another write, even of unchanged data.
        v.save().unwrap();
        let h2 = head_of(&VaultFile::read(v.path()).unwrap());
        assert_eq!(h2.revision, 2);
        assert_ne!(h2.digest, h1.digest);
        note_and_check(&mut v, &h2);

        // A new master password changes the key wrapping too.
        let before = head_of(&VaultFile::read(v.path()).unwrap());
        v.change_master_password("pw", "neu").unwrap();
        let after = head_of(&VaultFile::read(v.path()).unwrap());
        assert_ne!(after.digest, before.digest);

        // camelCase, these keys only.
        let json = serde_json::to_value(&after).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert_eq!(json["revision"], after.revision);
        let json = serde_json::to_value(SyncBase::new(&after, "F", 7)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "revision": after.revision,
                "digest": after.digest,
                "folder": "F",
                "syncedAt": 7
            })
        );
    }

    fn note_and_check(v: &mut crate::UnlockedVault, previous: &SyncHead) {
        v.save_item(VaultItem::new(ItemType::Note, "n")).unwrap();
        let h = head_of(&VaultFile::read(v.path()).unwrap());
        assert_eq!(h.revision, previous.revision + 1);
        assert_ne!(h.digest, previous.digest);
    }

    #[test]
    fn digests_see_each_encrypted_part() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultStore::new(dir.path());
        let v = store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let file = VaultFile::read(v.path()).unwrap();
        let digest = head_of(&file).digest;
        let flip = |s: &mut String| {
            let first = if s.starts_with('A') { "B" } else { "A" };
            s.replace_range(0..1, first);
        };

        // Only the payload's ciphertext changed.
        let mut payload = file.clone();
        flip(&mut payload.payload.ciphertext);
        assert_ne!(head_of(&payload).digest, digest);
        // Only the wrapped key changed (nonce or ciphertext).
        let mut nonce = file.clone();
        flip(&mut nonce.wrapped_key.nonce);
        assert_ne!(head_of(&nonce).digest, digest);
        let mut wrapped = file.clone();
        flip(&mut wrapped.wrapped_key.ciphertext);
        assert_ne!(head_of(&wrapped).digest, digest);
        assert_ne!(head_of(&nonce).digest, head_of(&wrapped).digest);
        // The separators keep the parts apart: moving bytes from one field
        // to the next is another digest.
        let mut shifted = file.clone();
        let moved = shifted.payload.ciphertext.pop().unwrap();
        shifted.wrapped_key.nonce.insert(0, moved);
        assert_ne!(head_of(&shifted).digest, digest);

        // The revision is in the head, not in the digest.
        let mut raised = file.clone();
        raised.revision = 99;
        let h = head_of(&raised);
        assert_eq!(h.revision, 99);
        assert_eq!(h.digest, digest);
    }
}
