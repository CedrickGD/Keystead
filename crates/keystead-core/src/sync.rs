//! Vault files as opaque bytes: what a file import and the cloud-folder
//! sync of the mobile app (Block 5) need to know about a `.keystead` file
//! without its master password – which vault it is and how new it is.

use serde::Serialize;

use crate::error::Result;
use crate::format::VaultFile;

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
    let file = VaultFile::parse(bytes)?;
    Ok(VaultHeader {
        id: file.id,
        name: file.name,
        revision: file.revision,
        updated_at: file.updated_at,
        version: file.version,
    })
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
