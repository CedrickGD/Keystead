//! Vault file format version 3 (see "Vault file format" in
//! docs/ARCHITECTURE.md) and atomic file writing.
//!
//! Layout: a random 32-byte vault key encrypts the JSON-serialised
//! [`VaultData`] with XChaCha20-Poly1305 (AAD binds vault id and revision).
//! The vault key itself is wrapped with a key derived from the master
//! password (Argon2id) and optionally with a key derived from a recovery code.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams, SecretKey, SALT_LEN};
use crate::error::{Error, Result};
use crate::model::{now_ms, VaultData, VaultInfo};
use crate::util;

/// Value of the `format` field.
pub const FORMAT_NAME: &str = "vaultx";
/// Current (and only supported) format version.
pub const FORMAT_VERSION: u32 = 3;
/// File extension of vault files (without dot).
pub const VAULT_EXTENSION: &str = "vaultx";
/// The only supported key derivation algorithm.
pub const KDF_ALG: &str = "argon2id";

/// Alphabet of recovery codes (Crockford base32).
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Number of symbols in a recovery code (25 × 5 bit = 125 bit).
pub const RECOVERY_CODE_LEN: usize = 25;

/// KDF description stored in the file header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdfSpec {
    pub alg: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    /// Base64 salt.
    pub salt: String,
}

impl KdfSpec {
    pub fn params(&self) -> KdfParams {
        KdfParams {
            memory_kib: self.memory_kib,
            iterations: self.iterations,
            parallelism: self.parallelism,
        }
    }
}

/// A nonce + ciphertext pair (both base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedBox {
    pub nonce: String,
    pub ciphertext: String,
}

impl SealedBox {
    fn from_sealed(s: &crypto::Sealed) -> Self {
        SealedBox {
            nonce: crypto::b64_encode(&s.nonce),
            ciphertext: crypto::b64_encode(&s.ciphertext),
        }
    }

    fn decode(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        Ok((
            crypto::b64_decode(&self.nonce)?,
            crypto::b64_decode(&self.ciphertext)?,
        ))
    }
}

/// The vault key wrapped with a recovery-code-derived key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryBox {
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// Serde representation of a `*.vaultx` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFile {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: u64,
    pub kdf: KdfSpec,
    pub wrapped_key: SealedBox,
    pub recovery: Option<RecoveryBox>,
    pub payload: SealedBox,
}

/// Minimal view used to give a precise error for foreign/future files.
#[derive(Deserialize)]
struct Probe {
    format: Option<serde_json::Value>,
    version: Option<serde_json::Value>,
}

impl VaultFile {
    /// Creates a new, fully encrypted vault file structure (revision 1)
    /// for an existing vault key.
    pub fn create(
        id: &str,
        name: &str,
        master_password: &str,
        params: KdfParams,
        key: &SecretKey,
        data: &VaultData,
    ) -> Result<Self> {
        params.validate()?;
        let now = now_ms();
        let mut file = VaultFile {
            format: FORMAT_NAME.to_owned(),
            version: FORMAT_VERSION,
            id: id.to_owned(),
            name: name.to_owned(),
            created_at: now,
            updated_at: now,
            revision: 1,
            kdf: KdfSpec {
                alg: KDF_ALG.to_owned(),
                memory_kib: params.memory_kib,
                iterations: params.iterations,
                parallelism: params.parallelism,
                salt: String::new(),
            },
            wrapped_key: SealedBox {
                nonce: String::new(),
                ciphertext: String::new(),
            },
            recovery: None,
            payload: SealedBox {
                nonce: String::new(),
                ciphertext: String::new(),
            },
        };
        file.set_master_password(key, master_password, params)?;
        file.encrypt_payload(key, data)?;
        Ok(file)
    }

    /// Parses and validates a vault file from raw bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bytes = util::strip_bom_bytes(bytes);
        let probe: Probe = serde_json::from_slice(bytes)
            .map_err(|_| Error::Unsupported("not a VaultX vault file".into()))?;
        if probe.format.as_ref().and_then(|v| v.as_str()) != Some(FORMAT_NAME) {
            return Err(Error::Unsupported("not a VaultX vault file".into()));
        }
        match probe.version.as_ref().and_then(serde_json::Value::as_u64) {
            Some(v) if v == u64::from(FORMAT_VERSION) => {}
            Some(v) => {
                return Err(Error::Unsupported(format!("vault format version {v}")));
            }
            None => return Err(Error::corrupt("missing format version")),
        }
        let file: VaultFile = serde_json::from_slice(bytes)
            .map_err(|e| Error::corrupt(format!("invalid vault header: {e}")))?;
        file.validate()?;
        Ok(file)
    }

    /// Reads a vault file. `Error::NotFound` if it does not exist.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(format!("vault file {}", path.display()))
            } else {
                Error::io_at(path, e)
            }
        })?;
        Self::parse(&bytes)
    }

    /// Pretty-printed JSON bytes.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut v = serde_json::to_vec_pretty(self)?;
        v.push(b'\n');
        Ok(v)
    }

    /// Writes the file atomically; `keep_backup` copies the previous version
    /// to `<file>.bak` first.
    pub fn write_atomic(&self, path: &Path, keep_backup: bool) -> Result<()> {
        write_atomic(path, &self.to_json()?, keep_backup)
    }

    /// Structural validation (does not need any key).
    pub fn validate(&self) -> Result<()> {
        if self.kdf.alg != KDF_ALG {
            return Err(Error::Unsupported(format!(
                "key derivation algorithm {}",
                self.kdf.alg
            )));
        }
        if self.kdf.params().validate().is_err() {
            return Err(Error::corrupt("key derivation parameters out of range"));
        }
        if self.id.trim().is_empty() {
            return Err(Error::corrupt("missing vault id"));
        }
        if crypto::b64_decode(&self.kdf.salt)?.len() < argon2::MIN_SALT_LEN {
            return Err(Error::corrupt("salt too short"));
        }
        let check_nonce = |b64: &str| -> Result<()> {
            if crypto::b64_decode(b64)?.len() == crypto::NONCE_LEN {
                Ok(())
            } else {
                Err(Error::corrupt("invalid nonce length"))
            }
        };
        check_nonce(&self.wrapped_key.nonce)?;
        check_nonce(&self.payload.nonce)?;
        crypto::b64_decode(&self.wrapped_key.ciphertext)?;
        crypto::b64_decode(&self.payload.ciphertext)?;
        if let Some(r) = &self.recovery {
            check_nonce(&r.nonce)?;
            crypto::b64_decode(&r.ciphertext)?;
            if crypto::b64_decode(&r.salt)?.len() < argon2::MIN_SALT_LEN {
                return Err(Error::corrupt("recovery salt too short"));
            }
        }
        Ok(())
    }

    /// KDF parameters of the master password (and recovery code).
    pub fn kdf_params(&self) -> KdfParams {
        self.kdf.params()
    }

    pub fn has_recovery(&self) -> bool {
        self.recovery.is_some()
    }

    /// Public description of the vault.
    pub fn info(&self, path: &Path) -> VaultInfo {
        VaultInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            path: path.display().to_string(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            has_recovery_key: self.has_recovery(),
        }
    }

    fn key_aad(&self) -> Vec<u8> {
        format!("vaultx:v3:key:{}", self.id).into_bytes()
    }

    fn recovery_aad(&self) -> Vec<u8> {
        format!("vaultx:v3:recovery:{}", self.id).into_bytes()
    }

    fn payload_aad(&self) -> Vec<u8> {
        format!("vaultx:v3:payload:{}:{}", self.id, self.revision).into_bytes()
    }

    /// Unwraps the vault key with the master password.
    /// `Error::WrongPassword` if the password (or the header) does not match.
    pub fn unwrap_key(&self, master_password: &str) -> Result<SecretKey> {
        let salt = crypto::b64_decode(&self.kdf.salt)?;
        let kek = crypto::derive_key(master_password.as_bytes(), &salt, &self.kdf_params())?;
        let (nonce, ct) = self.wrapped_key.decode()?;
        crypto::open_key(&kek, &nonce, &ct, &self.key_aad()).map_err(|_| Error::WrongPassword)
    }

    /// Unwraps the vault key with a recovery code (any accepted spelling).
    /// `Error::NotFound` if the vault has no recovery key,
    /// `Error::WrongPassword` if the code is wrong.
    pub fn unwrap_key_with_recovery(&self, recovery_code: &str) -> Result<SecretKey> {
        let recovery = self
            .recovery
            .as_ref()
            .ok_or_else(|| Error::NotFound("recovery key".into()))?;
        let code = normalize_recovery_code(recovery_code)?;
        let salt = crypto::b64_decode(&recovery.salt)?;
        let kek = crypto::derive_key(code.as_bytes(), &salt, &self.kdf_params())?;
        let nonce = crypto::b64_decode(&recovery.nonce)?;
        let ct = crypto::b64_decode(&recovery.ciphertext)?;
        crypto::open_key(&kek, &nonce, &ct, &self.recovery_aad())
            .map_err(|_| Error::WrongPassword)
    }

    /// Wraps `key` with a new master password (fresh salt).
    pub fn set_master_password(
        &mut self,
        key: &SecretKey,
        master_password: &str,
        params: KdfParams,
    ) -> Result<()> {
        if master_password.is_empty() {
            return Err(Error::invalid("password_empty"));
        }
        let salt = crypto::random_array::<SALT_LEN>()?;
        let kek = crypto::derive_key(master_password.as_bytes(), &salt, &params)?;
        let sealed = crypto::seal(&kek, key.as_slice(), &self.key_aad())?;
        self.kdf = KdfSpec {
            alg: KDF_ALG.to_owned(),
            memory_kib: params.memory_kib,
            iterations: params.iterations,
            parallelism: params.parallelism,
            salt: crypto::b64_encode(&salt),
        };
        self.wrapped_key = SealedBox::from_sealed(&sealed);
        Ok(())
    }

    /// Wraps `key` with a recovery code (normalised form, see
    /// [`normalize_recovery_code`]) using the vault's KDF parameters.
    pub fn set_recovery_code(&mut self, key: &SecretKey, recovery_code: &str) -> Result<()> {
        let code = normalize_recovery_code(recovery_code)?;
        let salt = crypto::random_array::<SALT_LEN>()?;
        let kek = crypto::derive_key(code.as_bytes(), &salt, &self.kdf_params())?;
        let sealed = crypto::seal(&kek, key.as_slice(), &self.recovery_aad())?;
        self.recovery = Some(RecoveryBox {
            salt: crypto::b64_encode(&salt),
            nonce: crypto::b64_encode(&sealed.nonce),
            ciphertext: crypto::b64_encode(&sealed.ciphertext),
        });
        Ok(())
    }

    /// Decrypts the payload. `Error::Corrupt` if authentication fails
    /// (tampered payload, revision or id) or the JSON is invalid.
    pub fn decrypt_payload(&self, key: &SecretKey) -> Result<VaultData> {
        let (nonce, ct) = self.payload.decode()?;
        let plain = crypto::open(key, &nonce, &ct, &self.payload_aad())
            .map_err(|_| Error::corrupt("vault payload failed authentication"))?;
        serde_json::from_slice(&plain)
            .map_err(|e| Error::corrupt(format!("vault payload is not valid vault data: {e}")))
    }

    /// Encrypts `data` as payload for the current `revision`.
    pub fn encrypt_payload(&mut self, key: &SecretKey, data: &VaultData) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(data)?);
        let sealed = crypto::seal(key, &plain, &self.payload_aad())?;
        self.payload = SealedBox::from_sealed(&sealed);
        Ok(())
    }
}

/// Generates a new recovery code formatted as `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX`.
pub fn generate_recovery_code() -> Result<String> {
    let mut raw = Zeroizing::new([0u8; RECOVERY_CODE_LEN]);
    crypto::fill_random(raw.as_mut_slice())?;
    let mut out = String::with_capacity(RECOVERY_CODE_LEN + 4);
    for (i, b) in raw.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        // 32 symbols: masking 5 bits of a uniform byte is unbiased.
        out.push(char::from(CROCKFORD[usize::from(b & 0x1F)]));
    }
    Ok(out)
}

/// Normalises a recovery code as typed by the user: uppercase, dashes and
/// whitespace removed, Crockford look-alikes mapped (O→0, I/L→1).
/// `Error::InvalidInput("recovery_key_format")` if it cannot be a valid code.
pub fn normalize_recovery_code(input: &str) -> Result<String> {
    let mut out = String::with_capacity(RECOVERY_CODE_LEN);
    for c in input.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !c.is_ascii() || !CROCKFORD.contains(&(c as u8)) {
            return Err(Error::invalid("recovery_key_format"));
        }
        out.push(c);
    }
    if out.len() != RECOVERY_CODE_LEN {
        return Err(Error::invalid("recovery_key_format"));
    }
    Ok(out)
}

/// `<path><suffix>`, e.g. `x.vaultx` → `x.vaultx.bak`.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Atomically replaces `path` with `bytes`: write `<path>.tmp`, fsync,
/// optionally copy the current file to `<path>.bak`, rename the temporary
/// file over `path`, fsync the directory (Unix).
pub fn write_atomic(path: &Path, bytes: &[u8], keep_backup: bool) -> Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
    let tmp = sibling(path, ".tmp");
    let result = (|| {
        let mut f = open_private(&tmp)?;
        f.write_all(bytes).map_err(|e| Error::io_at(&tmp, e))?;
        f.sync_all().map_err(|e| Error::io_at(&tmp, e))?;
        drop(f);
        if keep_backup && path.exists() {
            let bak = sibling(path, ".bak");
            fs::copy(path, &bak).map_err(|e| Error::io_at(&bak, e))?;
        }
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    sync_dir(&dir);
    Ok(())
}

/// Creates/truncates a file readable only by the current user (Unix).
fn open_private(path: &Path) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path).map_err(|e| Error::io_at(path, e))
}

/// `rename` with a few retries: on Windows, virus scanners and indexers
/// briefly hold files open, which makes `MoveFileEx` fail with
/// "access denied".
fn rename_with_retry(from: &Path, to: &Path) -> Result<()> {
    let mut attempt = 0u64;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 * attempt));
            }
            Err(e) => return Err(Error::io_at(to, e)),
        }
    }
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    // Best effort: makes the rename durable on power loss.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

/// Exclusive inter-process lock on `<vault>.lock`, held while a save
/// checks the on-disk revision and replaces the file. Released on drop
/// (and by the OS if the process dies).
pub(crate) struct FileLock {
    _file: File,
}

pub(crate) fn lock_vault_file(vault_path: &Path) -> Result<FileLock> {
    let lock_path = sibling(vault_path, ".lock");
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::io_at(&lock_path, e))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(FileLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "vault file is locked by another process",
                )));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::io_at(&lock_path, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ItemType, VaultItem};

    fn sample_file(pw: &str) -> (VaultFile, SecretKey, VaultData) {
        let key = crypto::random_key().unwrap();
        let data = VaultData {
            items: vec![VaultItem {
                id: "a".into(),
                item_type: ItemType::Note,
                name: "Hello".into(),
                notes: "secret note".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let file = VaultFile::create(
            "11111111-2222-4333-8444-555555555555",
            "Privat",
            pw,
            KdfParams::insecure_for_tests(),
            &key,
            &data,
        )
        .unwrap();
        (file, key, data)
    }

    #[test]
    fn json_shape_matches_contract() {
        let (file, _, _) = sample_file("pw");
        let v: serde_json::Value = serde_json::from_slice(&file.to_json().unwrap()).unwrap();
        assert_eq!(v["format"], "vaultx");
        assert_eq!(v["version"], 3);
        assert_eq!(v["revision"], 1);
        assert_eq!(v["kdf"]["alg"], "argon2id");
        assert_eq!(v["kdf"]["memoryKib"], 64);
        assert!(v["kdf"]["salt"].is_string());
        assert!(v["wrappedKey"]["nonce"].is_string());
        assert!(v["recovery"].is_null());
        assert!(v["payload"]["ciphertext"].is_string());
        assert!(v["createdAt"].is_i64());
        let raw = String::from_utf8(file.to_json().unwrap()).unwrap();
        assert!(!raw.contains("secret note"));
    }

    #[test]
    fn unwrap_and_decrypt() {
        let (file, key, data) = sample_file("correct horse");
        let parsed = VaultFile::parse(&file.to_json().unwrap()).unwrap();
        let k = parsed.unwrap_key("correct horse").unwrap();
        assert_eq!(*k, *key);
        assert_eq!(parsed.decrypt_payload(&k).unwrap(), data);
        assert!(matches!(
            parsed.unwrap_key("wrong"),
            Err(Error::WrongPassword)
        ));
    }

    #[test]
    fn payload_bound_to_revision_and_id() {
        let (mut file, key, _) = sample_file("pw");
        file.revision = 2;
        assert!(matches!(file.decrypt_payload(&key), Err(Error::Corrupt(_))));
        file.revision = 1;
        file.id = "other".into();
        assert!(matches!(file.decrypt_payload(&key), Err(Error::Corrupt(_))));
        assert!(matches!(file.unwrap_key("pw"), Err(Error::WrongPassword)));
    }

    #[test]
    fn rejects_foreign_and_future_files() {
        assert!(matches!(
            VaultFile::parse(b"{\"Version\":2,\"Salt\":\"x\"}"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            VaultFile::parse(b"not json"),
            Err(Error::Unsupported(_))
        ));
        let (file, _, _) = sample_file("pw");
        let mut v: serde_json::Value = serde_json::from_slice(&file.to_json().unwrap()).unwrap();
        v["version"] = 4.into();
        assert!(matches!(
            VaultFile::parse(&serde_json::to_vec(&v).unwrap()),
            Err(Error::Unsupported(_))
        ));
        v["version"] = 3.into();
        v["kdf"]["memoryKib"] = (4u64 * 1024 * 1024).into();
        assert!(matches!(
            VaultFile::parse(&serde_json::to_vec(&v).unwrap()),
            Err(Error::Corrupt(_))
        ));
        v["kdf"]["memoryKib"] = 64.into();
        v["kdf"]["alg"] = "scrypt".into();
        assert!(matches!(
            VaultFile::parse(&serde_json::to_vec(&v).unwrap()),
            Err(Error::Unsupported(_))
        ));
        v["kdf"]["alg"] = "argon2id".into();
        v["payload"]["nonce"] = "AAAA".into();
        assert!(matches!(
            VaultFile::parse(&serde_json::to_vec(&v).unwrap()),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn parse_accepts_bom() {
        let (file, _, _) = sample_file("pw");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(file.to_json().unwrap());
        assert_eq!(VaultFile::parse(&bytes).unwrap(), file);
    }

    #[test]
    fn recovery_codes() {
        let code = generate_recovery_code().unwrap();
        assert_eq!(code.len(), 29);
        assert_eq!(code.matches('-').count(), 4);
        let norm = normalize_recovery_code(&code).unwrap();
        assert_eq!(norm.len(), 25);
        assert_eq!(
            normalize_recovery_code(&code.to_lowercase().replace('-', " ")).unwrap(),
            norm
        );
        assert_eq!(
            normalize_recovery_code("o1il0-00000-00000-00000-00000").unwrap(),
            "0111000000000000000000000"
        );
        assert!(normalize_recovery_code("ABC").is_err());
        assert!(normalize_recovery_code("UUUUU-UUUUU-UUUUU-UUUUU-UUUUU").is_err());

        let (mut file, key, _) = sample_file("pw");
        assert!(matches!(
            file.unwrap_key_with_recovery(&code),
            Err(Error::NotFound(_))
        ));
        file.set_recovery_code(&key, &code).unwrap();
        let k = file
            .unwrap_key_with_recovery(&code.to_lowercase())
            .unwrap();
        assert_eq!(*k, *key);
        let other = generate_recovery_code().unwrap();
        assert!(matches!(
            file.unwrap_key_with_recovery(&other),
            Err(Error::WrongPassword)
        ));
    }

    #[test]
    fn atomic_write_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("f.json");
        write_atomic(&p, b"one", true).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        assert!(!sibling(&p, ".bak").exists());
        write_atomic(&p, b"two", true).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::read(sibling(&p, ".bak")).unwrap(), b"one");
        assert!(!sibling(&p, ".tmp").exists());
        write_atomic(&p, b"three", false).unwrap();
        assert_eq!(fs::read(sibling(&p, ".bak")).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.vaultx");
        let guard = lock_vault_file(&p).unwrap();
        let lock_path = sibling(&p, ".lock");
        let f = OpenOptions::new().write(true).open(&lock_path).unwrap();
        assert!(matches!(
            f.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(guard);
        assert!(f.try_lock().is_ok());
    }
}
