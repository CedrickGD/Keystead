//! Vault file format (see "Vault file format" in docs/ARCHITECTURE.md) and
//! atomic file writing. This build reads versions 1 and 2 (see
//! [`SUPPORTED_VERSIONS`]) and writes version 2 while the vault has
//! attachments, version 1 otherwise ([`version_for`]); header and payload
//! keys it does not know are written back unchanged ([`UnknownFields`]).
//!
//! Layout: a random 32-byte vault key encrypts the JSON-serialised
//! [`VaultData`] with XChaCha20-Poly1305 (AAD binds vault id and revision).
//! The vault key itself is wrapped with a key derived from the master
//! password (Argon2id) and optionally with a key derived from a recovery code.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams, SecretKey, SALT_LEN};
use crate::error::{Error, Result};
use crate::model::{now_ms, UnknownFields, VaultData, VaultInfo};
use crate::util;

/// Value of the `format` field.
pub const FORMAT_NAME: &str = "keystead";
/// Format version of files without attachments (new vaults, exports).
pub const FORMAT_VERSION: u32 = 1;
/// Format version of files whose payload has attachments
/// ([`VaultData::attachments`] not empty): same layout as version 1, but
/// the payload carries the attachment entries with their blob keys. Builds
/// that only read version 1 refuse such a file (`unsupported:vault format
/// version 2`) instead of dropping the keys on their next save.
pub const FORMAT_VERSION_ATTACHMENTS: u32 = 2;
/// Format versions [`VaultFile::parse`] accepts. Every write derives the
/// version from the payload ([`version_for`]): 2 while there are
/// attachments, back to 1 once the last one is removed, so older builds
/// open the vault again. Header keys ([`VaultFile::extra`]) and top-level
/// payload keys ([`VaultData::extra`]) this build does not know survive
/// every save. Any other version → `unsupported:vault format version <n>`.
pub const SUPPORTED_VERSIONS: RangeInclusive<u32> = 1..=2;

/// The format version a file with payload `data` is written as:
/// [`FORMAT_VERSION_ATTACHMENTS`] iff `data.attachments` is not empty,
/// [`FORMAT_VERSION`] otherwise.
pub fn version_for(data: &VaultData) -> u32 {
    if data.attachments.is_empty() {
        FORMAT_VERSION
    } else {
        FORMAT_VERSION_ATTACHMENTS
    }
}
/// File extension of vault files (without dot).
pub const VAULT_EXTENSION: &str = "keystead";
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

/// Serde representation of a `*.keystead` file.
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
    /// Header keys this build does not know (a later format version's
    /// fields), written back unchanged – see [`VaultData::extra`].
    #[serde(flatten)]
    pub extra: UnknownFields,
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
        Self::create_with_kek(id, name, master_password, params, key, data).map(|(file, _)| file)
    }

    /// [`VaultFile::create`] that also returns the master key-encryption
    /// key it derived (kept by an [`crate::UnlockedVault`]).
    pub(crate) fn create_with_kek(
        id: &str,
        name: &str,
        master_password: &str,
        params: KdfParams,
        key: &SecretKey,
        data: &VaultData,
    ) -> Result<(Self, SecretKey)> {
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
            extra: UnknownFields::default(),
        };
        let kek = file.set_master_password_kek(key, master_password, params)?;
        file.encrypt_payload(key, data)?;
        Ok((file, kek))
    }

    /// Parses and validates a vault file from raw bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bytes = util::strip_bom_bytes(bytes);
        let probe: Probe = serde_json::from_slice(bytes).map_err(|_| {
            // Damaged JSON (e.g. truncated) vs. some other kind of file.
            if bytes.trim_ascii_start().starts_with(b"{") {
                Error::corrupt("vault file is not valid JSON")
            } else {
                Error::Unsupported("not a Keystead vault file".into())
            }
        })?;
        if probe.format.as_ref().and_then(|v| v.as_str()) != Some(FORMAT_NAME) {
            return Err(Error::Unsupported("not a Keystead vault file".into()));
        }
        match probe.version.as_ref().and_then(serde_json::Value::as_u64) {
            Some(v) if u32::try_from(v).is_ok_and(|v| SUPPORTED_VERSIONS.contains(&v)) => {}
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
        format!("keystead:v1:key:{}", self.id).into_bytes()
    }

    fn recovery_aad(&self) -> Vec<u8> {
        format!("keystead:v1:recovery:{}", self.id).into_bytes()
    }

    fn payload_aad(&self) -> Vec<u8> {
        format!("keystead:v1:payload:{}:{}", self.id, self.revision).into_bytes()
    }

    /// Unwraps the vault key with the master password.
    /// `Error::WrongPassword` if the password (or the header) does not match.
    pub fn unwrap_key(&self, master_password: &str) -> Result<SecretKey> {
        let kek = self.derive_master_kek(master_password)?;
        self.unwrap_key_with_kek(&kek)
    }

    /// Derives the key-encryption key of `master_password` for this header's
    /// salt and KDF parameters (the slow Argon2id step of [`Self::unwrap_key`]).
    pub(crate) fn derive_master_kek(&self, master_password: &str) -> Result<SecretKey> {
        let salt = crypto::b64_decode(&self.kdf.salt)?;
        crypto::derive_key(master_password.as_bytes(), &salt, &self.kdf_params())
    }

    /// Unwraps the vault key with an already derived master KEK.
    /// `Error::WrongPassword` if it does not open the wrapping.
    pub(crate) fn unwrap_key_with_kek(&self, kek: &SecretKey) -> Result<SecretKey> {
        let (nonce, ct) = self.wrapped_key.decode()?;
        crypto::open_key(kek, &nonce, &ct, &self.key_aad()).map_err(|_| Error::WrongPassword)
    }

    /// Wraps `key` with an already derived master KEK. The KEK must belong
    /// to this header's salt and KDF parameters, which are kept.
    pub(crate) fn wrap_key_with_kek(&mut self, kek: &SecretKey, key: &SecretKey) -> Result<()> {
        let sealed = crypto::seal(kek, key.as_slice(), &self.key_aad())?;
        self.wrapped_key = SealedBox::from_sealed(&sealed);
        Ok(())
    }

    /// Unwraps the vault key with a recovery code (any accepted spelling).
    /// `Error::NotFound` if the vault has no recovery key,
    /// `Error::WrongPassword` if the code is wrong.
    pub fn unwrap_key_with_recovery(&self, recovery_code: &str) -> Result<SecretKey> {
        let kek = self.derive_recovery_kek(recovery_code)?;
        self.unwrap_recovery_with_kek(&kek)
    }

    fn recovery_box(&self) -> Result<&RecoveryBox> {
        self.recovery
            .as_ref()
            .ok_or_else(|| Error::NotFound("recovery key".into()))
    }

    /// Derives the key-encryption key of a recovery code for this header's
    /// recovery salt. `Error::NotFound` without recovery key.
    pub(crate) fn derive_recovery_kek(&self, recovery_code: &str) -> Result<SecretKey> {
        let recovery = self.recovery_box()?;
        let code = normalize_recovery_code(recovery_code)?;
        let salt = crypto::b64_decode(&recovery.salt)?;
        crypto::derive_key(code.as_bytes(), &salt, &self.kdf_params())
    }

    /// Unwraps the vault key with an already derived recovery KEK.
    pub(crate) fn unwrap_recovery_with_kek(&self, kek: &SecretKey) -> Result<SecretKey> {
        let recovery = self.recovery_box()?;
        let nonce = crypto::b64_decode(&recovery.nonce)?;
        let ct = crypto::b64_decode(&recovery.ciphertext)?;
        crypto::open_key(kek, &nonce, &ct, &self.recovery_aad()).map_err(|_| Error::WrongPassword)
    }

    /// Wraps `key` with an already derived recovery KEK, keeping the
    /// recovery salt the KEK belongs to. `Error::NotFound` without recovery key.
    pub(crate) fn wrap_recovery_with_kek(
        &mut self,
        kek: &SecretKey,
        key: &SecretKey,
    ) -> Result<()> {
        let salt = self.recovery_box()?.salt.clone();
        let sealed = crypto::seal(kek, key.as_slice(), &self.recovery_aad())?;
        self.recovery = Some(RecoveryBox {
            salt,
            nonce: crypto::b64_encode(&sealed.nonce),
            ciphertext: crypto::b64_encode(&sealed.ciphertext),
        });
        Ok(())
    }

    /// Wraps `key` with a new master password (fresh salt).
    pub fn set_master_password(
        &mut self,
        key: &SecretKey,
        master_password: &str,
        params: KdfParams,
    ) -> Result<()> {
        self.set_master_password_kek(key, master_password, params)
            .map(drop)
    }

    /// [`Self::set_master_password`] that returns the derived master KEK.
    pub(crate) fn set_master_password_kek(
        &mut self,
        key: &SecretKey,
        master_password: &str,
        params: KdfParams,
    ) -> Result<SecretKey> {
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
        Ok(kek)
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

    /// Encrypts `data` as payload for the current `revision` and sets the
    /// header `version` the payload calls for ([`version_for`]).
    pub fn encrypt_payload(&mut self, key: &SecretKey, data: &VaultData) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(data)?);
        let sealed = crypto::seal(key, &plain, &self.payload_aad())?;
        self.payload = SealedBox::from_sealed(&sealed);
        self.version = version_for(data);
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

/// `<path><suffix>`, e.g. `x.keystead` → `x.keystead.bak`.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Atomically replaces `path` with `bytes`: write a new, uniquely named
/// temporary file `.<file name>.<16 hex digits>.tmp` in the same directory, fsync,
/// optionally replace `<path>.bak` with a copy of the current file (the same
/// way), rename the temporary file over `path`, fsync the directory (Unix).
///
/// The temporary file is always created by this call (never an existing
/// file or a symlink planted by someone else), on Unix with mode 0600, and
/// only that file is removed again if something fails.
pub fn write_atomic(path: &Path, bytes: &[u8], keep_backup: bool) -> Result<()> {
    let dir = parent_dir(path);
    fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
    let tmp = write_temp(&dir, path, bytes)?;
    if keep_backup && path.exists() {
        backup_current(path)?;
    }
    tmp.persist(path)?;
    sync_dir(&dir);
    Ok(())
}

/// Replaces `<path>.bak` with a copy of the current `path` (atomically, via
/// a new temporary file).
pub(crate) fn backup_current(path: &Path) -> Result<()> {
    let bak = sibling(path, ".bak");
    let current = fs::read(path).map_err(|e| Error::io_at(path, e))?;
    write_temp(&parent_dir(&bak), &bak, &current)?.persist(&bak)
}

/// Makes sure `<path>.bak` holds no older revision than `path` itself: copies
/// the current file over it, or removes it if that fails. Best effort, used
/// after the secrets that open a vault were replaced (an older revision
/// would still open with the replaced secret).
pub(crate) fn supersede_backup(path: &Path) {
    let bak = sibling(path, ".bak");
    if !bak.exists() || backup_current(path).is_ok() {
        return;
    }
    let mut attempt = 0u64;
    loop {
        match fs::remove_file(&bak) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 * attempt));
            }
            _ => return,
        }
    }
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Number of random names tried before giving up on creating a temp file.
const TEMP_ATTEMPTS: usize = 8;
/// Characters of the target file name used in temporary file names.
const TEMP_NAME_CHARS: usize = 80;

/// Temporary files for `target` are named `<prefix><16 hex digits>.tmp`
/// with prefix `.<target file name>.` (the file name shortened to 80
/// characters) in the target's directory.
pub(crate) fn temp_file_prefix(target: &Path) -> String {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().chars().take(TEMP_NAME_CHARS).collect())
        .unwrap_or_else(|| "file".to_owned());
    format!(".{name}.")
}

/// A temporary file this process created; removed on drop unless
/// [`TempFile::persist`] renamed it into place.
struct TempFile {
    path: PathBuf,
    armed: bool,
}

impl TempFile {
    fn persist(mut self, target: &Path) -> Result<()> {
        rename_with_retry(&self.path, target)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Creates a new temporary file for `target` in `dir` and writes + fsyncs
/// `bytes` to it.
fn write_temp(dir: &Path, target: &Path, bytes: &[u8]) -> Result<TempFile> {
    let prefix = temp_file_prefix(target);
    for _ in 0..TEMP_ATTEMPTS {
        let suffix = data_encoding::HEXLOWER.encode(&crypto::random_array::<8>()?);
        let path = dir.join(format!("{prefix}{suffix}.tmp"));
        let f = match open_private_new(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io_at(&path, e)),
        };
        // From here on the file is ours: remove it again on any error.
        let tmp = TempFile { path, armed: true };
        let written = (|| {
            let mut f = f;
            f.write_all(bytes)?;
            f.sync_all()
        })();
        return match written {
            Ok(()) => Ok(tmp),
            Err(e) => Err(Error::io_at(&tmp.path, e)),
        };
    }
    Err(Error::io_at(
        dir,
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique temporary file",
        ),
    ))
}

/// Creates a new file readable only by the current user (Unix mode 0600).
/// Fails with `AlreadyExists` if anything – including a dangling symlink –
/// exists at `path` (`O_CREAT|O_EXCL` / `CREATE_NEW`); std opens with
/// `O_CLOEXEC`.
fn open_private_new(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Leftover temporary files of `target` (from a crash during a write):
/// `<prefix>*.tmp` as created by [`write_atomic`] and the `<target>.tmp`
/// of older versions.
pub(crate) fn stale_temp_files(target: &Path) -> Vec<PathBuf> {
    let prefix = temp_file_prefix(target);
    let mut out = vec![sibling(target, ".tmp")];
    if let Ok(entries) = fs::read_dir(parent_dir(target)) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.ends_with(".tmp") {
                out.push(entry.path());
            }
        }
    }
    out
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

/// Runs a removal (`remove_file`, `remove_dir_all`) best effort with the
/// same few retries as [`rename_with_retry`]: on Windows, virus scanners
/// and indexers briefly hold freshly written files open (sharing
/// violation / access denied). A missing path counts as removed; whatever
/// still fails after the last attempt stays. Used for the leftovers of a
/// deleted vault, which counts as deleted once its file is gone.
pub(crate) fn remove_with_retry(mut remove: impl FnMut() -> std::io::Result<()>) {
    let mut attempt = 0u64;
    loop {
        match remove() {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 * attempt));
            }
            Err(_) => return,
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
        assert_eq!(v["format"], "keystead");
        assert_eq!(v["version"], 1);
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
        assert!(matches!(
            VaultFile::parse(b"\n{\"format\": \"keystead\", \"vers"),
            Err(Error::Corrupt(_))
        ));
        let (file, _, _) = sample_file("pw");
        let mut v: serde_json::Value = serde_json::from_slice(&file.to_json().unwrap()).unwrap();
        v["version"] = 3.into();
        let err = VaultFile::parse(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
        assert_eq!(err.code(), "unsupported:vault format version 3");
        v["version"] = 1.into();
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

    /// The header version follows the payload: 2 while there are
    /// attachments, 1 otherwise; both are read, version 3 is refused, and
    /// header and payload keys this build does not know survive a save.
    #[test]
    fn reads_version_1_and_2_only() {
        assert_eq!(SUPPORTED_VERSIONS, 1..=2);
        assert!(SUPPORTED_VERSIONS.contains(&FORMAT_VERSION));
        assert!(SUPPORTED_VERSIONS.contains(&FORMAT_VERSION_ATTACHMENTS));
        let (mut file, key, mut data) = sample_file("pw");
        assert_eq!(file.version, 1);
        assert_eq!(version_for(&data), 1);
        // A payload with an attachment and a field only a later build
        // understands.
        data.attachments.insert(
            "a1".into(),
            crate::model::AttachmentEntry {
                item_id: "a".into(),
                name: "scan.png".into(),
                size: 3,
                created_at: 1,
                key: "c2VjcmV0".into(),
            },
        );
        let sync = serde_json::json!({ "s1": { "token": "dG9rZW4=" } });
        data.extra.0.insert("sync".into(), sync.clone());
        file.encrypt_payload(&key, &data).unwrap();
        assert_eq!(file.version, 2, "set by encrypt_payload");
        let mut v: serde_json::Value = serde_json::from_slice(&file.to_json().unwrap()).unwrap();
        assert_eq!(v["version"], 2);
        v["attachmentsDir"] = "x".into();
        let parsed = VaultFile::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.extra.0["attachmentsDir"], "x");
        let mut expected = file.clone();
        expected.extra.0.insert("attachmentsDir".into(), "x".into());
        assert_eq!(parsed, expected);
        let decrypted = parsed.decrypt_payload(&key).unwrap();
        assert_eq!(decrypted, data);
        assert_eq!(decrypted.attachments["a1"].key, "c2VjcmV0");
        assert_eq!(decrypted.extra.0["sync"], sync);
        // A save by this build (new revision, changed items) keeps all of it.
        let mut saved = parsed.clone();
        saved.revision += 1;
        let mut changed = decrypted.clone();
        changed.items[0].favorite = true;
        saved.encrypt_payload(&key, &changed).unwrap();
        let again = VaultFile::parse(&saved.to_json().unwrap()).unwrap();
        assert_eq!(again.version, 2);
        assert_eq!(again.extra.0["attachmentsDir"], "x");
        let payload = again.decrypt_payload(&key).unwrap();
        assert!(payload.items[0].favorite);
        assert_eq!(payload.attachments, data.attachments);
        assert_eq!(payload.extra.0["sync"], sync);
        let raw: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&payload).unwrap()).unwrap();
        assert_eq!(raw["sync"], sync, "flattened, not nested");
        assert_eq!(raw["attachments"]["a1"]["itemId"], "a");
        // The last attachment gone → version 1 again, still with the
        // unknown keys; a version-1 file with a (hand-written) attachment
        // payload is read as well.
        let mut back = again.clone();
        back.revision += 1;
        let mut without = payload.clone();
        without.attachments.clear();
        back.encrypt_payload(&key, &without).unwrap();
        assert_eq!(back.version, 1);
        let reparsed = VaultFile::parse(&back.to_json().unwrap()).unwrap();
        assert_eq!(reparsed.version, 1);
        assert_eq!(reparsed.extra.0["attachmentsDir"], "x");
        assert_eq!(reparsed.decrypt_payload(&key).unwrap(), without);
        v["version"] = 1.into();
        let lenient = VaultFile::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(lenient.decrypt_payload(&key).unwrap(), data);
        for version in [0u64, 3, u64::from(u32::MAX) + 1] {
            v["version"] = version.into();
            let err = VaultFile::parse(&serde_json::to_vec(&v).unwrap()).unwrap_err();
            assert_eq!(
                err.code(),
                format!("unsupported:vault format version {version}")
            );
        }
        v["version"] = "1".into();
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
        let k = file.unwrap_key_with_recovery(&code.to_lowercase()).unwrap();
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

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Another local user pre-creates the temporary file names the old code
    /// used (world-readable file, symlink to a victim file).
    #[cfg(unix)]
    #[test]
    fn atomic_write_ignores_planted_temp_files() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("export.csv");
        let planted = sibling(&target, ".tmp");
        fs::write(&planted, b"").unwrap();
        fs::set_permissions(&planted, fs::Permissions::from_mode(0o666)).unwrap();
        let planted_ino = fs::metadata(&planted).unwrap().ino();
        let victim = dir.path().join("victim.txt");
        fs::write(&victim, b"victim").unwrap();
        let target2 = dir.path().join("export.json");
        symlink(&victim, sibling(&target2, ".tmp")).unwrap();

        write_atomic(&target, b"secret", false).unwrap();
        write_atomic(&target2, b"secret 2", false).unwrap();

        let meta = fs::metadata(&target).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_ne!(meta.ino(), planted_ino);
        assert_eq!(fs::read(&target).unwrap(), b"secret");
        assert_eq!(fs::read(&planted).unwrap(), b"", "planted file untouched");
        let planted_mode = fs::metadata(&planted).unwrap().permissions().mode();
        assert_eq!(planted_mode & 0o777, 0o666);
        assert_eq!(fs::read(&victim).unwrap(), b"victim");
        assert_eq!(fs::read(&target2).unwrap(), b"secret 2");
        assert_eq!(
            fs::metadata(&target2).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Writing again works while the planted files still exist, and no
        // temporary file of ours is left behind.
        write_atomic(&target, b"again", true).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"again");
        assert_eq!(
            dir_names(dir.path()),
            [
                "export.csv",
                "export.csv.bak",
                "export.csv.tmp",
                "export.json",
                "export.json.tmp",
                "victim.txt"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn backup_replaces_a_planted_symlink_instead_of_following_it() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.keystead");
        let victim = dir.path().join("victim");
        fs::write(&victim, b"victim").unwrap();
        write_atomic(&p, b"one", true).unwrap();
        let bak = sibling(&p, ".bak");
        symlink(&victim, &bak).unwrap();
        write_atomic(&p, b"two", true).unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"victim");
        let meta = fs::symlink_metadata(&bak).unwrap();
        assert!(meta.file_type().is_file());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read(&bak).unwrap(), b"one");
    }

    #[cfg(unix)]
    #[test]
    fn failed_write_removes_only_its_own_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        // Renaming a file over a directory fails.
        let target = dir.path().join("t");
        fs::create_dir(&target).unwrap();
        let planted = sibling(&target, ".tmp");
        fs::write(&planted, b"planted").unwrap();
        assert!(write_atomic(&target, b"secret", false).is_err());
        assert_eq!(fs::read(&planted).unwrap(), b"planted");
        assert_eq!(dir_names(dir.path()), ["t", "t.tmp"]);
    }

    #[test]
    fn supersede_backup_drops_the_previous_revision() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.keystead");
        let bak = sibling(&p, ".bak");
        write_atomic(&p, b"one", true).unwrap();
        supersede_backup(&p);
        assert!(!bak.exists(), "no backup is created");
        write_atomic(&p, b"two", true).unwrap();
        assert_eq!(fs::read(&bak).unwrap(), b"one");
        write_atomic(&p, b"three", false).unwrap();
        supersede_backup(&p);
        assert_eq!(fs::read(&bak).unwrap(), b"three");
        assert_eq!(dir_names(dir.path()), ["v.keystead", "v.keystead.bak"]);
    }

    #[test]
    fn remove_with_retry_is_best_effort() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, b"x").unwrap();
        remove_with_retry(|| fs::remove_file(&file));
        assert!(!file.exists());
        // Nothing to remove: done at once, no retries.
        let started = Instant::now();
        remove_with_retry(|| fs::remove_file(&file));
        remove_with_retry(|| fs::remove_dir_all(dir.path().join("none")));
        assert!(started.elapsed() < Duration::from_millis(500));
        // A removal that keeps failing is retried ten times, then given up.
        let mut calls = 0;
        remove_with_retry(|| {
            calls += 1;
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "held open",
            ))
        });
        assert_eq!(calls, 11);
    }

    #[test]
    fn stale_temp_files_match_only_this_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.keystead");
        for name in [
            ".a.keystead.0123456789abcdef.tmp",
            ".a.keystead.bak.0123456789abcdef.tmp",
            ".ab.keystead.0123456789abcdef.tmp",
            ".a.keystead.0123456789abcdef.txt",
            "a.keystead.tmp",
        ] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        let mut found: Vec<String> = stale_temp_files(&target)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                ".a.keystead.0123456789abcdef.tmp",
                ".a.keystead.bak.0123456789abcdef.tmp",
                "a.keystead.tmp"
            ]
        );
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.keystead");
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
