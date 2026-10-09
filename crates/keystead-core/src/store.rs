//! The set of vault files in `<data_dir>/vaults`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::crypto::{self, KdfParams};
use crate::error::{Error, Result};
use crate::format::{self, VaultFile, VAULT_EXTENSION};
use crate::model::{VaultData, VaultInfo};
use crate::paths;
use crate::util;
use crate::vault::{validate_name, UnlockedVault};

/// Access to the vault files below a data directory.
#[derive(Debug, Clone)]
pub struct VaultStore {
    root: PathBuf,
}

/// Vault ids become file names: only allow a conservative character set.
fn check_vault_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::NotFound(format!("vault {id}")))
    }
}

impl VaultStore {
    /// Store in [`paths::data_dir`].
    pub fn open_default() -> Result<Self> {
        let store = VaultStore::new(paths::data_dir());
        let dir = store.vaults_dir();
        fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
        Ok(store)
    }

    /// Store rooted at an arbitrary data directory (tests, portable mode).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        VaultStore { root: root.into() }
    }

    /// The data directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/vaults`.
    pub fn vaults_dir(&self) -> PathBuf {
        self.root.join("vaults")
    }

    /// `<root>/vaults/<vault-id>.keystead`.
    pub fn vault_path(&self, vault_id: &str) -> PathBuf {
        self.vaults_dir()
            .join(format!("{vault_id}.{VAULT_EXTENSION}"))
    }

    /// All readable vaults, sorted by name (case-insensitive). Files that
    /// cannot be parsed, or whose file name does not match their id, are
    /// skipped.
    pub fn list_vaults(&self) -> Result<Vec<VaultInfo>> {
        let dir = self.vaults_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io_at(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(VAULT_EXTENSION) || !path.is_file()
            {
                continue;
            }
            let Ok(file) = VaultFile::read(&path) else {
                continue;
            };
            if path.file_stem().and_then(|s| s.to_str()) != Some(file.id.as_str()) {
                continue;
            }
            out.push(file.info(&path));
        }
        out.sort_by_cached_key(|v| (v.name.to_lowercase(), v.id.clone()));
        Ok(out)
    }

    /// Creates a new vault with the default (strong) KDF parameters.
    pub fn create_vault(&self, name: &str, master_password: &str) -> Result<UnlockedVault> {
        self.create_vault_with_params(name, master_password, KdfParams::default())
    }

    /// Creates a new vault with explicit KDF parameters.
    pub fn create_vault_with_params(
        &self,
        name: &str,
        master_password: &str,
        kdf: KdfParams,
    ) -> Result<UnlockedVault> {
        let name = validate_name(name, "name")?;
        if master_password.is_empty() {
            return Err(Error::invalid("password_empty"));
        }
        kdf.validate()?;
        let id = util::new_id();
        let key = crypto::random_key()?;
        let data = VaultData::default();
        let file = VaultFile::create(&id, &name, master_password, kdf, &key, &data)?;
        let path = self.vault_path(&id);
        if path.exists() {
            // A UUID v4 collision is practically impossible; never overwrite.
            return Err(Error::Conflict);
        }
        file.write_atomic(&path, false)?;
        Ok(UnlockedVault::from_parts(path, file, key, data))
    }

    fn read_vault(&self, vault_id: &str) -> Result<(PathBuf, VaultFile)> {
        check_vault_id(vault_id)?;
        let path = self.vault_path(vault_id);
        let file = VaultFile::read(&path)?;
        if file.id != vault_id {
            return Err(Error::corrupt("vault id does not match file name"));
        }
        Ok((path, file))
    }

    /// Unlocks a vault. `Error::WrongPassword` on a bad password.
    pub fn unlock(&self, vault_id: &str, master_password: &str) -> Result<UnlockedVault> {
        let (path, file) = self.read_vault(vault_id)?;
        let key = file.unwrap_key(master_password)?;
        let data = file.decrypt_payload(&key)?;
        Ok(UnlockedVault::from_parts(path, file, key, data))
    }

    /// Unlocks a vault with its recovery key and sets a new master password.
    /// The recovery key stays valid.
    pub fn unlock_with_recovery_key(
        &self,
        vault_id: &str,
        recovery_key: &str,
        new_master_password: &str,
    ) -> Result<UnlockedVault> {
        if new_master_password.is_empty() {
            return Err(Error::invalid("password_empty"));
        }
        let (path, file) = self.read_vault(vault_id)?;
        let key = file.unwrap_key_with_recovery(recovery_key)?;
        let data = file.decrypt_payload(&key)?;
        let params = file.kdf_params();
        let mut header = file.clone();
        header.set_master_password(&key, new_master_password, params)?;
        let mut vault = UnlockedVault::from_parts(path, file, key, data);
        vault.replace_header_and_save(header)?;
        Ok(vault)
    }

    /// Deletes a vault after verifying its master password (removes the
    /// file, its `.bak` and helper files).
    pub fn delete_vault(&self, vault_id: &str, master_password: &str) -> Result<()> {
        let (path, file) = self.read_vault(vault_id)?;
        file.unwrap_key(master_password)?;
        {
            let _lock = format::lock_vault_file(&path)?;
            fs::remove_file(&path).map_err(|e| Error::io_at(&path, e))?;
            for suffix in [".bak", ".tmp"] {
                let p = format::sibling(&path, suffix);
                match fs::remove_file(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(Error::io_at(&p, e)),
                }
            }
        }
        // The lock file can only be removed once the lock is released.
        let _ = fs::remove_file(format::sibling(&path, ".lock"));
        Ok(())
    }
}
