//! Vault files as opaque bytes: what a file import and the cloud-folder
//! sync of the mobile app (Block 5) need to know about a `.keystead` file
//! without its master password – which vault it is and how new it is. For
//! the two-way sync (stage 2b): which write of a vault file a copy is
//! ([`head_of`]), the last state both sides agreed on ([`SyncBase`]), the
//! authenticated ancestry a write carries ([`SyncProof`]) and what to do
//! with a pair of copies ([`decide`]).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{self, SecretKey};
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

/// The end of the name of a conflict copy the phone writes into the cloud
/// folder (see [`phone_conflict_file_name`]).
pub const PHONE_CONFLICT_SUFFIX: &str = "-phone";

/// The name of the conflict copy of vault `id` the phone writes into the
/// cloud folder when it resolves a conflict (made at `at_ms`, UTC), so the
/// PC can offer to restore it: `<id>.keystead.conflict-<YYYYMMDD-HHMMSS>-phone`.
/// Not a vault file name ([`vault_id_of_file_name`] → `None`): no sync takes
/// it for the vault; [`vault_id_of_phone_conflict_copy`] recognises it.
pub fn phone_conflict_file_name(id: &str, at_ms: i64) -> String {
    format!("{}{PHONE_CONFLICT_SUFFIX}", conflict_file_name(id, at_ms))
}

/// The vault id of a conflict copy the phone wrote into the cloud folder:
/// `<id>.keystead.conflict-<8 digits>-<6 digits>-phone` with a valid id
/// (see [`phone_conflict_file_name`]). Any other name → `None`.
pub fn vault_id_of_phone_conflict_copy(name: &str) -> Option<&str> {
    let rest = name.strip_suffix(PHONE_CONFLICT_SUFFIX)?;
    let (id, stamp) = rest.split_once(&format!(".{VAULT_EXTENSION}.conflict-"))?;
    let (date, time) = stamp.split_once('-')?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    (crate::store::is_valid_id(id) && digits(date, 8) && digits(time, 6)).then_some(id)
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
/// file. The plain header (name, times, revision, KDF, the sync proof) is
/// not part of it: what tells two writes apart is the ciphertext, and a
/// re-keyed vault (a new master password or vault key) has a new key
/// wrapping too.
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
    pub fn is(&self, head: &SyncHead) -> bool {
        head.revision == self.revision && head.digest == self.digest
    }
}

// ---------------------------------------------------------------------------
// The sync proof: authenticated ancestry
// ---------------------------------------------------------------------------

/// The header key of the sync proof (an optional header extension, outside
/// the payload's associated data). Builds that do not know it keep it as an
/// unknown header key (`VaultFile::extra`) and write it back unchanged –
/// after their next save it no longer verifies (it binds the digest of the
/// write that made it), which counts as "no proof".
pub const SYNC_PROOF_KEY: &str = "syncProof";
/// The proof format this build writes and verifies (`v`).
pub const SYNC_PROOF_VERSION: u64 = 1;
/// At most this many ancestors (newest first); older ones drop off. Each
/// save adds one, so this is how many writes one side may make before the
/// other side's base drops out of the proof (then: a conflict, never a
/// silent pull). 512 digests are ~34 KB of header and ~37 KB of MAC input
/// per write – small next to `VAULT_FILE_MAX_BYTES`; the MAC encodes the
/// count as `be64`, so the bound is the format's, not the encoding's.
pub const SYNC_PROOF_MAX_ANCESTORS: usize = 512;
/// Domain separation of the MAC input.
const PROOF_DOMAIN: &[u8] = b"keystead-sync-proof:1";
/// HKDF `info` of the proof key.
const LINEAGE_INFO: &[u8] = b"keystead-sync-lineage:1";

/// `syncProof` in a vault file's header: the digests ([`head_of`]) of the
/// writes this write descends from, newest first, and an HMAC-SHA256 over
/// them, the vault id, the revision and the digest of this very write,
/// under a key derived from the vault key (see [`seal_proof`]). Only a
/// holder of the vault key can make one, so the folder's untrusted header
/// cannot claim an ancestry it does not have: an edited list, a raised
/// revision or a proof copied onto another write does not verify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncProof {
    pub v: u64,
    pub ancestors: Vec<String>,
    /// Lowercase hex HMAC-SHA256.
    pub mac: String,
}

fn is_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The proof `file` carries, unverified – `None` without one, or with one
/// of another version or out of bounds (more than
/// [`SYNC_PROOF_MAX_ANCESTORS`] ancestors, anything but lowercase hex
/// digests). The bounds are checked on the parsed header value before any
/// ancestor is copied out of it: an oversized list is refused without
/// allocating its strings again.
pub fn proof_of(file: &VaultFile) -> Option<SyncProof> {
    let value = file.extra.0.get(SYNC_PROOF_KEY)?;
    let ancestors = value.get("ancestors")?.as_array()?;
    if ancestors.len() > SYNC_PROOF_MAX_ANCESTORS
        || !ancestors.iter().all(|a| a.as_str().is_some_and(is_digest))
    {
        return None;
    }
    let proof = SyncProof::deserialize(value).ok()?;
    let ok = proof.v == SYNC_PROOF_VERSION
        && proof.ancestors.len() <= SYNC_PROOF_MAX_ANCESTORS
        && proof.ancestors.iter().all(|a| is_digest(a))
        && is_digest(&proof.mac);
    ok.then_some(proof)
}

/// The ancestors `file` claims, unverified (none without a proof). Only for
/// this side's own, private copy – whose writes this app made – and only
/// where a wrong claim cannot replace anything (a "remote older" report, a
/// push of this side's copy over one of its own earlier writes).
pub fn claimed_ancestors(file: &VaultFile) -> Vec<String> {
    proof_of(file).map(|p| p.ancestors).unwrap_or_default()
}

/// The key of the proofs: HKDF-SHA256 of the vault key (no salt, info
/// `keystead-sync-lineage:1`).
fn proof_key(vault_key: &SecretKey) -> SecretKey {
    crypto::hkdf_sha256(vault_key.as_slice(), b"", LINEAGE_INFO)
}

/// Length-prefixed (`u64` big endian) bytes.
fn push_prefixed(msg: &mut Vec<u8>, bytes: &[u8]) {
    msg.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    msg.extend_from_slice(bytes);
}

/// HMAC-SHA256 under the proof key over `lp("keystead-sync-proof:1") ‖
/// lp(id) ‖ be64(revision) ‖ lp(digest) ‖ be64(n) ‖ lp(ancestor_1) ‖ … ‖
/// lp(ancestor_n)`, `lp(x) = be64(len(x)) ‖ x`, strings as UTF-8.
fn proof_mac(vault_key: &SecretKey, head: &SyncHead, ancestors: &[String]) -> [u8; 32] {
    let mut msg = Vec::with_capacity(160 + 72 * ancestors.len());
    push_prefixed(&mut msg, PROOF_DOMAIN);
    push_prefixed(&mut msg, head.id.as_bytes());
    msg.extend_from_slice(&head.revision.to_be_bytes());
    push_prefixed(&mut msg, head.digest.as_bytes());
    msg.extend_from_slice(&(ancestors.len() as u64).to_be_bytes());
    for ancestor in ancestors {
        push_prefixed(&mut msg, ancestor.as_bytes());
    }
    crypto::hmac_sha256(proof_key(vault_key).as_slice(), &[&msg])
}

/// Writes the proof of `file` – which must be complete (payload encrypted
/// and key wrapped with `vault_key`): `ancestors` (at most the first
/// [`SYNC_PROOF_MAX_ANCESTORS`]) and the MAC over them and this write's
/// recomputed head. Replaces a proof that was there.
pub fn seal_proof(file: &mut VaultFile, vault_key: &SecretKey, ancestors: &[String]) {
    let ancestors: Vec<String> = ancestors
        .iter()
        .take(SYNC_PROOF_MAX_ANCESTORS)
        .cloned()
        .collect();
    let mac = proof_mac(vault_key, &head_of(file), &ancestors);
    let proof = SyncProof {
        v: SYNC_PROOF_VERSION,
        ancestors,
        mac: data_encoding::HEXLOWER.encode(&mac),
    };
    let value = serde_json::to_value(proof).expect("a proof serialises");
    file.extra.0.insert(SYNC_PROOF_KEY.to_owned(), value);
}

/// The ancestors of `file` if its proof verifies with `vault_key` (the
/// key that opens this very file), `None` otherwise – no proof, another
/// key, or anything changed since it was made (the ancestors, the
/// revision, the id, the encrypted parts).
pub fn verify_proof(file: &VaultFile, vault_key: &SecretKey) -> Option<Vec<String>> {
    let proof = proof_of(file)?;
    let expected = proof_mac(vault_key, &head_of(file), &proof.ancestors);
    let given = data_encoding::HEXLOWER.decode(proof.mac.as_bytes()).ok()?;
    crypto::ct_eq(&expected, &given).then_some(proof.ancestors)
}

/// The ancestors of a new write: `heads` (the writes it directly follows,
/// newest first), then each of `lineages` – duplicates dropped, at most
/// [`SYNC_PROOF_MAX_ANCESTORS`].
pub fn next_lineage(heads: &[&str], lineages: &[&[String]]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let all = heads
        .iter()
        .copied()
        .chain(lineages.iter().flat_map(|l| l.iter().map(String::as_str)));
    for digest in all {
        if out.len() == SYNC_PROOF_MAX_ANCESTORS {
            break;
        }
        if !out.iter().any(|d| d == digest) {
            out.push(digest.to_owned());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

/// What this side knows about the folder's copy's ancestry for [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteProof<'a> {
    /// Its sync proof verified with this side's open vault (the session's
    /// master key opens the copy's vault key, and that key the proof):
    /// these are its ancestors.
    Verified(&'a [String]),
    /// This side cannot check it now: the vault is locked, or the copy has
    /// another master password. A copy that would be pulled is only taken
    /// on trial ([`SyncDecision::PullUnverified`]).
    Unchecked,
    /// Checked, but it has no proof, or one that does not verify: its
    /// ancestry is unknown.
    Unproven,
}

/// What the two-way sync does with one vault ([`decide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    /// The two copies are different vaults (other ids): neither is touched.
    OtherVault,
    /// The folder's copy descends from this side's (its verified ancestry
    /// holds the base): take it.
    PullRemote,
    /// The folder's copy would be pulled, but its ancestry cannot be
    /// checked now (vault locked, or another master password): it may only
    /// be taken **on trial** – confirmed once it opens here, and only if its
    /// proof then verifies and holds the head it replaced; otherwise that
    /// head comes back and the two are a conflict.
    PullUnverified,
    /// Only this side's copy changed since the base: write it to the folder.
    PushLocal,
    /// Both copies are the same write (or there are none): nothing to do
    /// but to record the base.
    UpToDate,
    /// Both copies changed since the base, or nothing proves that one
    /// descends from the other: a conflict for the user to resolve; nothing
    /// is merged, nothing replaced.
    Diverged,
    /// Only the folder has the vault: the phone imports it, the PC offers it.
    NewRemote,
    /// Only this side has the vault and it was never synced: write it to
    /// the folder.
    NewLocal,
    /// Only this side has the vault, but it was synced before: the folder's
    /// copy was removed. It is not written again.
    RemoteGone,
    /// The folder's copy is one of this side's earlier writes while this
    /// side is unchanged since the base (a restored backup, a cloud app
    /// that has not fetched the newest file yet): reported, never taken.
    RemoteOlder,
}

/// The decision for this side's copy `local` and the folder's copy
/// `remote` of one vault (`None`: that side has none), given the `base` of
/// that vault (`None`: never synced with this folder), the ancestors of
/// this side's copy (`local_ancestors`: the open session's verified lineage,
/// or – locked – what this side's own file claims) and what is known about
/// the folder's copy's ancestry (`remote_proof`). Pure: no file access.
///
/// `same(X, Y)`: equal id, revision and digest. `descends(R, X)`:
/// `R.revision > X.revision` and `X.digest` is among `R`'s **verified**
/// ancestors – a higher revision alone never authorizes a replacement.
///
/// * no copies → up to date; only the folder's → new remote; only this
///   side's → new local without a base, remote gone with one;
/// * other ids → other vault; `same(L, R)` → up to date (the same digest
///   with another revision is not);
/// * no base (the first sync with this folder): the folder's copy descends
///   from this side's → pull (unchecked: pull on trial); this side's copy is
///   newer and the folder's is one of its own ancestors → push; else →
///   diverged;
/// * `same(L, B)`: the folder's copy is newer and descends from the base →
///   pull (unchecked: pull on trial); it is older and one of this side's
///   ancestors → remote older; else → diverged;
/// * `same(R, B)` and this side's revision is above the base's → push
///   (this side's own lineage is trusted);
/// * anything else → diverged.
pub fn decide(
    local: Option<&SyncHead>,
    remote: Option<&SyncHead>,
    base: Option<&SyncBase>,
    local_ancestors: &[String],
    remote_proof: RemoteProof<'_>,
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
    if local == remote {
        return SyncDecision::UpToDate;
    }
    let is_local_ancestor = local_ancestors.contains(&remote.digest);
    match base {
        None => {
            if remote.revision > local.revision {
                pull_from(remote_proof, &local.digest)
            } else if local.revision > remote.revision && is_local_ancestor {
                SyncDecision::PushLocal
            } else {
                SyncDecision::Diverged
            }
        }
        Some(base) if base.is(local) => {
            if remote.revision > base.revision {
                pull_from(remote_proof, &base.digest)
            } else if remote.revision < base.revision && is_local_ancestor {
                SyncDecision::RemoteOlder
            } else {
                SyncDecision::Diverged
            }
        }
        Some(base) if base.is(remote) && local.revision > base.revision => SyncDecision::PushLocal,
        Some(_) => SyncDecision::Diverged,
    }
}

/// A pull of a newer folder copy that must descend from the write with
/// digest `from`.
fn pull_from(remote_proof: RemoteProof<'_>, from: &str) -> SyncDecision {
    match remote_proof {
        RemoteProof::Verified(ancestors) if ancestors.iter().any(|a| a == from) => {
            SyncDecision::PullRemote
        }
        RemoteProof::Unchecked => SyncDecision::PullUnverified,
        RemoteProof::Verified(_) | RemoteProof::Unproven => SyncDecision::Diverged,
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
        assert_eq!(
            vault_id_of_phone_conflict_copy(&name),
            None,
            "not the phone's"
        );

        // The phone's conflict copy in the cloud folder.
        let phone = phone_conflict_file_name(id, 1_792_764_309_999);
        assert_eq!(
            phone,
            format!("{id}.keystead.conflict-20261023-140509-phone")
        );
        assert_eq!(vault_id_of_file_name(&phone), None, "never a vault file");
        assert_eq!(vault_id_of_phone_conflict_copy(&phone), Some(id));
        for other in [
            format!("{id}.keystead"),
            format!("{id}.keystead.conflict-2026102-140509-phone"),
            format!("{id}.keystead.conflict-20261023-14050x-phone"),
            format!("{id}.keystead.conflict-20261023-140509-phone.tmp"),
            format!("{id}.keystead.conflict-20261023-140509-phone (1)"),
            format!("{id} (1).keystead.conflict-20261023-140509-phone"),
            "../a.keystead.conflict-20261023-140509-phone".to_owned(),
            ".keystead.conflict-20261023-140509-phone".to_owned(),
        ] {
            assert_eq!(vault_id_of_phone_conflict_copy(&other), None, "{other}");
        }
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

    fn list(digests: &[&str]) -> Vec<String> {
        digests.iter().map(|d| (*d).to_owned()).collect()
    }

    #[test]
    fn sync_decisions() {
        use RemoteProof::{Unchecked, Unproven, Verified};
        use SyncDecision::*;
        let b = base(5, "b5");
        let at_base = head("a", 5, "b5");
        let none: Vec<String> = Vec::new();
        let from_base = list(&["b5", "b4"]);
        let d = |l: Option<&SyncHead>, r: Option<&SyncHead>, b: Option<&SyncBase>| {
            decide(l, r, b, &[], Verified(&from_base))
        };

        // Missing copies.
        assert_eq!(d(None, None, None), UpToDate);
        assert_eq!(d(None, None, Some(&b)), UpToDate);
        assert_eq!(d(None, Some(&at_base), None), NewRemote);
        assert_eq!(d(None, Some(&at_base), Some(&b)), NewRemote);
        assert_eq!(d(Some(&at_base), None, None), NewLocal);
        assert_eq!(d(Some(&at_base), None, Some(&b)), RemoteGone);
        assert_eq!(
            d(Some(&head("a", 9, "l9")), None, Some(&b)),
            RemoteGone,
            "edited here, removed there: still not written again"
        );

        // Another vault, whatever the base, digests or proofs say.
        let other = head("b", 6, "b6");
        for base in [None, Some(&b)] {
            assert_eq!(d(Some(&at_base), Some(&other), base), OtherVault);
            assert_eq!(d(Some(&other), Some(&at_base), base), OtherVault);
        }

        // The same write on both sides.
        assert_eq!(d(Some(&at_base), Some(&at_base), Some(&b)), UpToDate);
        let both = head("a", 9, "x9");
        assert_eq!(d(Some(&both), Some(&both), None), UpToDate);
        assert_eq!(
            d(Some(&both), Some(&both), Some(&b)),
            UpToDate,
            "equal copies beyond the base: the base is refreshed"
        );
        // The same digest with another revision is not the same write.
        assert_eq!(
            decide(
                Some(&at_base),
                Some(&head("a", 9, "b5")),
                Some(&b),
                &[],
                Unproven
            ),
            Diverged
        );
        assert_eq!(
            decide(
                Some(&at_base),
                Some(&head("a", 2, "b5")),
                Some(&b),
                &[],
                Unproven
            ),
            Diverged
        );

        // Only the folder changed and its verified ancestry holds the base:
        // pull – however far ahead (provider lag: several writes since).
        let remote_new = head("a", 6, "r6");
        assert_eq!(d(Some(&at_base), Some(&remote_new), Some(&b)), PullRemote);
        let far = list(&["r8", "r7", "r6", "b5"]);
        assert_eq!(
            decide(
                Some(&at_base),
                Some(&head("a", 9, "r9")),
                Some(&b),
                &[],
                Verified(&far)
            ),
            PullRemote
        );
        // A higher revision alone never is: no proof, a proof without the
        // base, or a branch of its own.
        for proof in [
            Unproven,
            Verified(&none),
            Verified(&far[..2]),
            Verified(&["b4".to_owned()]),
        ] {
            assert_eq!(
                decide(
                    Some(&at_base),
                    Some(&head("a", u64::MAX, "rx")),
                    Some(&b),
                    &[],
                    proof
                ),
                Diverged,
                "{proof:?}"
            );
        }
        // The vault locked (or another master password): only on trial.
        assert_eq!(
            decide(Some(&at_base), Some(&remote_new), Some(&b), &[], Unchecked),
            PullUnverified
        );

        // Only this side changed: push (its own lineage is trusted).
        let local_new = head("a", 6, "l6");
        assert_eq!(d(Some(&local_new), Some(&at_base), Some(&b)), PushLocal);
        assert_eq!(
            decide(Some(&local_new), Some(&at_base), Some(&b), &[], Unchecked),
            PushLocal
        );

        // The folder went back to one of this side's earlier writes while
        // this side is unchanged: reported. Not one of them: a conflict.
        let remote_old = head("a", 3, "r3");
        let mine = list(&["b4", "r3"]);
        assert_eq!(
            decide(Some(&at_base), Some(&remote_old), Some(&b), &mine, Unproven),
            RemoteOlder
        );
        assert_eq!(
            decide(Some(&at_base), Some(&remote_old), Some(&b), &[], Unproven),
            Diverged
        );

        // Both changed: the lost-update case.
        assert_eq!(d(Some(&local_new), Some(&remote_new), Some(&b)), Diverged);
        assert_eq!(
            d(Some(&head("a", 7, "l7")), Some(&remote_new), Some(&b)),
            Diverged
        );
        // This side changed and the folder went back: not a push over it.
        assert_eq!(d(Some(&local_new), Some(&remote_old), Some(&b)), Diverged);
        // The same revision with another digest: one side rewrote it.
        assert_eq!(
            d(Some(&at_base), Some(&head("a", 5, "r5")), Some(&b)),
            Diverged
        );
        assert_eq!(
            d(Some(&head("a", 5, "l5")), Some(&at_base), Some(&b)),
            Diverged
        );
        // This side went back below the base (a restored backup here).
        assert_eq!(
            d(Some(&head("a", 4, "l4")), Some(&at_base), Some(&b)),
            Diverged
        );
        // A base that is neither copy.
        let stale = base(2, "b2");
        assert_eq!(d(Some(&at_base), Some(&remote_new), Some(&stale)), Diverged);
        // The base's digest counts, not only its revision.
        let same_revision = base(5, "other");
        assert_eq!(
            d(Some(&at_base), Some(&remote_new), Some(&same_revision)),
            Diverged
        );
        assert_eq!(
            d(Some(&local_new), Some(&at_base), Some(&same_revision)),
            Diverged
        );
    }

    /// The first sync with a folder (no base): only proven ancestry decides.
    #[test]
    fn sync_decisions_without_a_base() {
        use RemoteProof::{Unchecked, Unproven, Verified};
        use SyncDecision::*;
        let local = head("a", 5, "l5");
        // The folder's copy descends from this side's: pull (on trial when
        // it cannot be checked); otherwise a conflict, however far ahead.
        let ancestry = list(&["r6", "l5"]);
        assert_eq!(
            decide(
                Some(&local),
                Some(&head("a", 7, "r7")),
                None,
                &[],
                Verified(&ancestry)
            ),
            PullRemote
        );
        assert_eq!(
            decide(
                Some(&local),
                Some(&head("a", 7, "r7")),
                None,
                &[],
                Unchecked
            ),
            PullUnverified
        );
        for proof in [Unproven, Verified(&ancestry[..1])] {
            assert_eq!(
                decide(Some(&local), Some(&head("a", 99, "r99")), None, &[], proof),
                Diverged
            );
        }
        // This side's copy is newer and the folder's is one of its own
        // earlier writes: push; any other older copy is a conflict.
        let mine = list(&["l4", "l3"]);
        assert_eq!(
            decide(
                Some(&local),
                Some(&head("a", 3, "l3")),
                None,
                &mine,
                Unproven
            ),
            PushLocal
        );
        assert_eq!(
            decide(
                Some(&local),
                Some(&head("a", 3, "x3")),
                None,
                &mine,
                Unproven
            ),
            Diverged
        );
        // The same revision with other contents.
        assert_eq!(
            decide(
                Some(&local),
                Some(&head("a", 5, "r5")),
                None,
                &mine,
                Unchecked
            ),
            Diverged
        );
    }

    #[test]
    fn lineages_are_bounded_and_without_duplicates() {
        let own = list(&["a", "b", "c"]);
        let other = list(&["c", "d"]);
        assert_eq!(
            next_lineage(&["h", "x"], &[&own, &other]),
            list(&["h", "x", "a", "b", "c", "d"])
        );
        assert_eq!(next_lineage(&["a"], &[&own]), list(&["a", "b", "c"]));
        let long: Vec<String> = (0..SYNC_PROOF_MAX_ANCESTORS + 100)
            .map(|n| format!("{n:064x}"))
            .collect();
        let next = next_lineage(&["h"], &[&long]);
        assert_eq!(next.len(), SYNC_PROOF_MAX_ANCESTORS);
        assert_eq!(next[0], "h");
        assert_eq!(next[1], long[0], "the newest stay");
    }

    fn real_vault() -> (tempfile::TempDir, VaultStore, crate::UnlockedVault) {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultStore::new(dir.path());
        let v = store
            .create_vault_with_params("Privat", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        (dir, store, v)
    }

    #[test]
    fn every_write_carries_a_verifiable_proof() {
        let (_dir, store, mut v) = real_vault();
        let first = VaultFile::read(v.path()).unwrap();
        let h1 = head_of(&first);
        // A new vault: a proof without ancestors.
        assert_eq!(proof_of(&first).unwrap().ancestors, none_list());
        assert_eq!(v.verified_lineage_of(&first), Some(Vec::new()));
        assert!(v.lineage().is_empty());

        // Each save names the previous write first.
        v.save_item(VaultItem::new(ItemType::Note, "n")).unwrap();
        let second = VaultFile::read(v.path()).unwrap();
        let h2 = head_of(&second);
        assert_eq!(
            v.verified_lineage_of(&second),
            Some(vec![h1.digest.clone()])
        );
        v.save().unwrap();
        let third = VaultFile::read(v.path()).unwrap();
        assert_eq!(
            v.verified_lineage_of(&third),
            Some(vec![h2.digest.clone(), h1.digest.clone()])
        );
        assert!(v.descends_from(&h1) && v.descends_from(&h2));
        assert!(!v.descends_from(&head_of(&third)), "not from itself");

        // Another session (another device with the same password) verifies
        // it too, and continues the lineage.
        let mut other = store.unlock(v.id(), "pw").unwrap();
        assert_eq!(other.lineage(), v.lineage());
        other.save().unwrap();
        assert!(other.descends_from(&h1));

        // The JSON: `syncProof` with `v`, `ancestors` and `mac`.
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(v.path()).unwrap()).unwrap();
        let proof = json["syncProof"].as_object().unwrap();
        assert_eq!(proof.len(), 3);
        assert_eq!(proof["v"], 1);
        assert_eq!(proof["mac"].as_str().unwrap().len(), 64);
    }

    fn none_list() -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn edited_or_copied_proofs_do_not_verify() {
        let (_dir, store, mut v) = real_vault();
        let base = head_of(&VaultFile::read(v.path()).unwrap());
        v.save().unwrap();
        let genuine = VaultFile::read(v.path()).unwrap();
        assert!(v.verified_lineage_of(&genuine).is_some());
        let proof = proof_of(&genuine).unwrap();

        let with_proof = |f: &VaultFile, p: &SyncProof| {
            let mut f = f.clone();
            f.extra
                .0
                .insert(SYNC_PROOF_KEY.into(), serde_json::to_value(p).unwrap());
            f
        };
        // An ancestor added (the folder claims to descend from another
        // write), one removed, the order changed.
        let mut added = proof.clone();
        added.ancestors.insert(0, "f".repeat(64));
        assert_eq!(v.verified_lineage_of(&with_proof(&genuine, &added)), None);
        let mut removed = proof.clone();
        removed.ancestors.clear();
        assert_eq!(v.verified_lineage_of(&with_proof(&genuine, &removed)), None);
        // The revision raised (the payload would not open either).
        let mut raised = genuine.clone();
        raised.revision += 5;
        assert_eq!(v.verified_lineage_of(&raised), None);
        // The MAC changed, or not hex.
        let mut mac = proof.clone();
        mac.mac
            .replace_range(0..1, if mac.mac.starts_with('0') { "1" } else { "0" });
        assert_eq!(v.verified_lineage_of(&with_proof(&genuine, &mac)), None);
        let mut upper = proof.clone();
        upper.mac = upper.mac.to_uppercase();
        assert_eq!(proof_of(&with_proof(&genuine, &upper)), None);
        // Another version, too many ancestors.
        let mut v2 = proof.clone();
        v2.v = 2;
        assert_eq!(proof_of(&with_proof(&genuine, &v2)), None);
        let mut many = proof.clone();
        many.ancestors = (0..=SYNC_PROOF_MAX_ANCESTORS)
            .map(|n| format!("{n:064x}"))
            .collect();
        assert_eq!(proof_of(&with_proof(&genuine, &many)), None);
        // Exactly at the bound is still a proof (it does not verify: edited).
        many.ancestors.pop();
        assert!(proof_of(&with_proof(&genuine, &many)).is_some());
        // Ancestors that are not digests, or not strings at all.
        for bad in [
            serde_json::json!([1, 2]),
            serde_json::json!(["F".repeat(64)]),
            serde_json::json!("not a list"),
        ] {
            let mut value = serde_json::to_value(&proof).unwrap();
            value["ancestors"] = bad.clone();
            let mut f = genuine.clone();
            f.extra.0.insert(SYNC_PROOF_KEY.into(), value);
            assert_eq!(proof_of(&f), None, "{bad}");
        }
        // Not an object at all.
        let mut garbage = genuine.clone();
        garbage
            .extra
            .0
            .insert(SYNC_PROOF_KEY.into(), serde_json::json!("x"));
        assert_eq!(proof_of(&garbage), None);
        assert!(claimed_ancestors(&garbage).is_empty());

        // A proof copied from another write of the same vault onto this one.
        v.save().unwrap();
        let later = VaultFile::read(v.path()).unwrap();
        let copied = with_proof(&later, &proof);
        assert_eq!(v.verified_lineage_of(&copied), None);
        // … or from another vault (other key, other id).
        let w = store
            .create_vault_with_params("Andere", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let foreign = VaultFile::read(w.path()).unwrap();
        let mut transplanted = genuine.clone();
        transplanted.extra = foreign.extra.clone();
        assert_eq!(v.verified_lineage_of(&transplanted), None);
        assert_eq!(v.verified_lineage_of(&foreign), None, "another vault");
        // Missing.
        let mut bare = genuine.clone();
        bare.extra.0.remove(SYNC_PROOF_KEY);
        assert_eq!(v.verified_lineage_of(&bare), None);
        // The genuine one still does – and says it descends from the base.
        assert!(v
            .verified_lineage_of(&genuine)
            .unwrap()
            .contains(&base.digest));
    }

    /// Builds that do not know the proof (2.0.x) keep it as an unknown
    /// header key; after their next save it no longer verifies.
    #[test]
    fn writers_without_the_proof_keep_it_but_it_stops_verifying() {
        let (_dir, store, mut v) = real_vault();
        v.save().unwrap();
        let bytes = std::fs::read(v.path()).unwrap();
        // The 2.0.x reader is this `VaultFile::parse`: the proof lands in
        // the unknown header keys and is written back unchanged.
        let parsed = VaultFile::parse(&bytes).unwrap();
        assert!(parsed.extra.0.contains_key(SYNC_PROOF_KEY));
        let reparsed = VaultFile::parse(&parsed.to_json().unwrap()).unwrap();
        assert_eq!(reparsed.extra, parsed.extra);
        assert_eq!(v.verified_lineage_of(&reparsed), Some(v.lineage().to_vec()));
        // A writer that does not know it re-encrypts the payload (a new
        // write) and keeps the old proof: it binds the previous digest.
        let key = parsed.unwrap_key("pw").unwrap();
        let data = parsed.decrypt_payload(&key).unwrap();
        let mut old_writer = parsed.clone();
        old_writer.revision += 1;
        old_writer.encrypt_payload(&key, &data).unwrap();
        assert!(old_writer.extra.0.contains_key(SYNC_PROOF_KEY));
        assert_eq!(v.verified_lineage_of(&old_writer), None);
        // The file still opens everywhere.
        old_writer.write_atomic(v.path(), false).unwrap();
        let reopened = store.unlock(v.id(), "pw").unwrap();
        assert!(reopened.lineage().is_empty(), "no verified ancestry");
    }

    /// A new master password (or recovery key) rotates the vault key: the
    /// proof is made with the new key over the ancestry the session
    /// carried, so the next device still sees its base as an ancestor.
    #[test]
    fn rotations_keep_the_lineage() {
        let (_dir, store, mut v) = real_vault();
        let base = head_of(&VaultFile::read(v.path()).unwrap());
        v.save().unwrap();
        v.change_master_password("pw", "neu").unwrap();
        let rotated = VaultFile::read(v.path()).unwrap();
        // The old key's session elsewhere cannot verify it …
        let stale = VaultStore::new(store.root());
        assert!(stale.unlock(v.id(), "pw").is_err());
        // … the new password's can, and the base is in it.
        let fresh = store.unlock(v.id(), "neu").unwrap();
        assert!(fresh.descends_from(&base));
        assert_eq!(
            fresh.verified_lineage_of(&rotated),
            Some(fresh.lineage().to_vec())
        );
        // A recovery key rotates again.
        let mut fresh = fresh;
        fresh.create_recovery_key().unwrap();
        assert!(fresh.descends_from(&base));
        let again = store.unlock(v.id(), "neu").unwrap();
        assert!(again.descends_from(&base));
    }

    /// Keeping this side's state over the folder's copy names the folder's
    /// head (and its verified ancestry) in the new write's proof.
    #[test]
    fn superseding_writes_descend_from_both_sides() {
        let (_dir, store, mut v) = real_vault();
        let base = head_of(&VaultFile::read(v.path()).unwrap());
        // The other device's branch: a copy of the store, saved twice.
        let other_dir = tempfile::tempdir().unwrap();
        let other_store = VaultStore::new(other_dir.path());
        std::fs::create_dir_all(other_store.vaults_dir()).unwrap();
        std::fs::copy(v.path(), other_store.vault_path(v.id())).unwrap();
        let mut other = other_store.unlock(v.id(), "pw").unwrap();
        other.save().unwrap();
        let other_mid = head_of(&VaultFile::read(other.path()).unwrap());
        other.save().unwrap();
        let theirs = VaultFile::read(other.path()).unwrap();
        // This side's branch.
        v.save().unwrap();
        let mine = head_of(&VaultFile::read(v.path()).unwrap());
        assert!(v.verified_lineage_of(&theirs).is_some());
        v.save_superseding(&theirs).unwrap();
        assert_eq!(v.revision(), theirs.revision + 1);
        let resolved = VaultFile::read(v.path()).unwrap();
        let lineage = other.verified_lineage_of(&resolved).unwrap();
        assert_eq!(lineage[0], mine.digest);
        assert_eq!(lineage[1], head_of(&theirs).digest);
        for h in [&base, &other_mid] {
            assert!(lineage.contains(&h.digest));
        }
        // The other device (at `theirs`, its base) pulls it cleanly.
        let theirs_head = head_of(&theirs);
        let b = SyncBase::new(&theirs_head, "F", 1);
        assert_eq!(
            decide(
                Some(&theirs_head),
                Some(&head_of(&resolved)),
                Some(&b),
                &[],
                RemoteProof::Verified(&lineage)
            ),
            SyncDecision::PullRemote
        );
        let _ = store;
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
