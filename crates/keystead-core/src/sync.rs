//! Vault files as opaque bytes: what a file import and the cloud-folder
//! sync of the mobile app (Block 5) need to know about a `.keystead` file
//! without its master password – which vault it is and how new it is – and
//! what the read-only phone does with a copy it finds in its cloud folder
//! ([`decide_copy_in`]).

use std::cmp::Ordering;

use serde::Serialize;

use crate::error::Result;
use crate::format::{VaultFile, VAULT_EXTENSION};

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
}
