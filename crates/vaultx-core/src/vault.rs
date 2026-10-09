//! An unlocked vault: decrypted data plus the vault key, persisted through
//! the v3 file format.
//!
//! Every mutating method works on a copy of the data and only commits it to
//! memory after the file was written successfully, so a failed save (for
//! example `Error::Conflict`) never leaves memory and disk out of sync.

use std::path::{Path, PathBuf};

use zeroize::Zeroize;

use crate::crypto::{self, SecretKey};
use crate::error::{Error, Result};
use crate::format::{self, VaultFile};
use crate::matching;
use crate::model::{
    now_ms, Folder, GeneratedPassword, ItemSummary, ItemType, PasswordHistoryEntry, VaultData,
    VaultInfo, VaultItem,
};
use crate::util;

/// Maximum number of previous passwords kept per login.
pub const PASSWORD_HISTORY_MAX: usize = 10;
/// Maximum number of generated passwords kept in the generator history.
pub const GENERATOR_HISTORY_MAX: usize = 50;
/// Maximum length of vault, item and folder names (characters).
pub const NAME_MAX_CHARS: usize = 200;

/// A vault that has been unlocked with its master password or recovery key.
pub struct UnlockedVault {
    path: PathBuf,
    /// Header as last read from / written to disk.
    file: VaultFile,
    key: SecretKey,
    data: VaultData,
}

impl std::fmt::Debug for UnlockedVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key or the data.
        f.debug_struct("UnlockedVault")
            .field("path", &self.path)
            .field("id", &self.file.id)
            .field("revision", &self.file.revision)
            .finish_non_exhaustive()
    }
}

impl Drop for UnlockedVault {
    fn drop(&mut self) {
        wipe_data(&mut self.data);
    }
}

/// Overwrites the secret strings of the vault data before the memory is freed.
fn wipe_data(data: &mut VaultData) {
    for item in &mut data.items {
        item.name.zeroize();
        item.notes.zeroize();
        if let Some(l) = item.login.as_mut() {
            l.username.zeroize();
            l.password.zeroize();
            l.totp.zeroize();
            for u in &mut l.uris {
                u.uri.zeroize();
            }
        }
        if let Some(c) = item.card.as_mut() {
            c.cardholder_name.zeroize();
            c.number.zeroize();
            c.code.zeroize();
            c.exp_month.zeroize();
            c.exp_year.zeroize();
        }
        if let Some(i) = item.identity.as_mut() {
            for s in [
                &mut i.first_name,
                &mut i.last_name,
                &mut i.email,
                &mut i.phone,
                &mut i.address1,
                &mut i.address2,
                &mut i.postal_code,
                &mut i.city,
                &mut i.username,
            ] {
                s.zeroize();
            }
        }
        for f in &mut item.fields {
            f.name.zeroize();
            f.value.zeroize();
        }
        for h in &mut item.password_history {
            h.password.zeroize();
        }
    }
    for g in &mut data.generator_history {
        g.password.zeroize();
    }
}

/// Trims a user supplied name and checks it is non-empty and not too long.
pub(crate) fn validate_name(name: &str, what: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid(format!("{what}_required")));
    }
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(Error::invalid(format!("{what}_too_long")));
    }
    Ok(trimmed.to_owned())
}

impl UnlockedVault {
    pub(crate) fn from_parts(path: PathBuf, file: VaultFile, key: SecretKey, data: VaultData) -> Self {
        UnlockedVault {
            path,
            file,
            key,
            data,
        }
    }

    /// Public description (id, name, timestamps, recovery flag).
    pub fn info(&self) -> VaultInfo {
        self.file.info(&self.path)
    }

    /// Vault id.
    pub fn id(&self) -> &str {
        &self.file.id
    }

    /// Vault name.
    pub fn name(&self) -> &str {
        &self.file.name
    }

    /// Revision of the file this state corresponds to.
    pub fn revision(&self) -> u64 {
        self.file.revision
    }

    pub fn data(&self) -> &VaultData {
        &self.data
    }

    /// All items including trashed ones.
    pub fn items(&self) -> &[VaultItem] {
        &self.data.items
    }

    pub fn folders(&self) -> &[Folder] {
        &self.data.folders
    }

    pub fn generator_history(&self) -> &[GeneratedPassword] {
        &self.data.generator_history
    }

    pub fn item(&self, id: &str) -> Option<&VaultItem> {
        self.data.items.iter().find(|i| i.id == id)
    }

    /// Non-trashed items, sorted by name (case-insensitive).
    pub fn summaries(&self) -> Vec<ItemSummary> {
        let mut v: Vec<&VaultItem> = self.data.items.iter().filter(|i| !i.is_trashed()).collect();
        sort_by_name(&mut v);
        v.into_iter().map(VaultItem::summary).collect()
    }

    /// Non-trashed items matching every whitespace-separated term of
    /// `query` (case-insensitive) in name, username, URIs, notes or card
    /// brand. An empty query returns all summaries.
    pub fn search(&self, query: &str) -> Vec<ItemSummary> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut v: Vec<&VaultItem> = self
            .data
            .items
            .iter()
            .filter(|i| !i.is_trashed() && matches_terms(i, &terms))
            .collect();
        sort_by_name(&mut v);
        v.into_iter().map(VaultItem::summary).collect()
    }

    /// Non-trashed logins with a URI matching `url` (see
    /// [`matching::uri_matches`]); favorites first, then by name.
    pub fn logins_for_url(&self, url: &str) -> Vec<ItemSummary> {
        let mut v: Vec<&VaultItem> = self
            .data
            .items
            .iter()
            .filter(|i| !i.is_trashed() && i.item_type == ItemType::Login)
            .filter(|i| {
                i.login
                    .as_ref()
                    .is_some_and(|l| l.uris.iter().any(|u| matching::uri_matches(u, url)))
            })
            .collect();
        sort_by_name(&mut v);
        v.sort_by_key(|i| !i.favorite);
        v.into_iter().map(VaultItem::summary).collect()
    }

    /// Creates or updates an item and persists the vault.
    ///
    /// * New if `id` is empty or unknown: assigns a UUID and `createdAt`.
    /// * Always sets `updatedAt`; keeps `createdAt`/`deletedAt` of an
    ///   existing item.
    /// * Logins: a changed password moves the old one to the password
    ///   history (max 10) and sets `passwordRevisedAt`.
    /// * login/card/identity are normalised to the item type; an unknown
    ///   folder id is cleared.
    pub fn save_item(&mut self, item: VaultItem) -> Result<VaultItem> {
        let mut item = item;
        item.name = validate_name(&item.name, "name")?;
        item.normalize();
        let now = now_ms();
        self.mutate(move |data| {
            if let Some(fid) = item.folder_id.as_deref() {
                if !data.folders.iter().any(|f| f.id == fid) {
                    item.folder_id = None;
                }
            }
            let existing = if item.id.is_empty() {
                None
            } else {
                data.items.iter().position(|i| i.id == item.id)
            };
            match existing {
                Some(pos) => {
                    let old = &data.items[pos];
                    item.created_at = old.created_at;
                    item.deleted_at = old.deleted_at;
                    let old_password = zeroize::Zeroizing::new(old.password().to_owned());
                    if let Some(login) = item.login.as_mut() {
                        if !old_password.is_empty() && *old_password != login.password {
                            item.password_history.insert(
                                0,
                                PasswordHistoryEntry {
                                    password: old_password.to_string(),
                                    replaced_at: now,
                                },
                            );
                            login.password_revised_at = Some(now);
                        }
                    }
                    item.password_history.truncate(PASSWORD_HISTORY_MAX);
                    item.updated_at = now;
                    data.items[pos] = item.clone();
                }
                None => {
                    item.id = util::new_id();
                    item.created_at = now;
                    item.updated_at = now;
                    item.deleted_at = None;
                    item.password_history.truncate(PASSWORD_HISTORY_MAX);
                    data.items.push(item.clone());
                }
            }
            Ok(item)
        })
    }

    /// Moves an item to the trash.
    pub fn trash_item(&mut self, id: &str) -> Result<()> {
        let now = now_ms();
        self.mutate(|data| {
            let item = find_item_mut(data, id)?;
            if item.deleted_at.is_none() {
                item.deleted_at = Some(now);
            }
            Ok(())
        })
    }

    /// Restores an item from the trash.
    pub fn restore_item(&mut self, id: &str) -> Result<()> {
        self.mutate(|data| {
            find_item_mut(data, id)?.deleted_at = None;
            Ok(())
        })
    }

    /// Deletes an item permanently.
    pub fn delete_item(&mut self, id: &str) -> Result<()> {
        self.mutate(|data| {
            let pos = data
                .items
                .iter()
                .position(|i| i.id == id)
                .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
            data.items.remove(pos);
            Ok(())
        })
    }

    /// Permanently deletes all trashed items; returns how many.
    pub fn empty_trash(&mut self) -> Result<usize> {
        let count = self.data.items.iter().filter(|i| i.is_trashed()).count();
        if count == 0 {
            return Ok(0);
        }
        self.mutate(|data| {
            data.items.retain(|i| !i.is_trashed());
            Ok(count)
        })
    }

    /// Creates (empty or unknown id) or renames a folder.
    pub fn save_folder(&mut self, folder: Folder) -> Result<Folder> {
        let name = validate_name(&folder.name, "name")?;
        self.mutate(|data| {
            if !folder.id.is_empty() {
                if let Some(f) = data.folders.iter_mut().find(|f| f.id == folder.id) {
                    f.name = name;
                    return Ok(f.clone());
                }
            }
            let f = Folder {
                id: util::new_id(),
                name,
            };
            data.folders.push(f.clone());
            Ok(f)
        })
    }

    /// Deletes a folder; its items are moved to "no folder".
    pub fn delete_folder(&mut self, id: &str) -> Result<()> {
        self.mutate(|data| {
            let pos = data
                .folders
                .iter()
                .position(|f| f.id == id)
                .ok_or_else(|| Error::NotFound(format!("folder {id}")))?;
            data.folders.remove(pos);
            for item in data.items.iter_mut() {
                if item.folder_id.as_deref() == Some(id) {
                    item.folder_id = None;
                }
            }
            Ok(())
        })
    }

    /// Prepends a generated password to the generator history (max 50).
    pub fn add_generated_password(&mut self, password: &str) -> Result<()> {
        if password.is_empty() {
            return Err(Error::invalid("password_empty"));
        }
        let now = now_ms();
        self.mutate(|data| {
            data.generator_history.insert(
                0,
                GeneratedPassword {
                    password: password.to_owned(),
                    created_at: now,
                },
            );
            data.generator_history.truncate(GENERATOR_HISTORY_MAX);
            Ok(())
        })
    }

    pub fn clear_generator_history(&mut self) -> Result<()> {
        self.mutate(|data| {
            data.generator_history.clear();
            Ok(())
        })
    }

    /// Adds imported items and folders with fresh ids and persists once.
    ///
    /// Folder ids referenced by the items are mapped to the new ids; an
    /// imported folder whose name already exists (case-insensitive) is merged
    /// into the existing folder. Returns the number of imported items.
    pub fn import_items(&mut self, items: Vec<VaultItem>, folders: Vec<Folder>) -> Result<usize> {
        if items.is_empty() && folders.is_empty() {
            return Ok(0);
        }
        let now = now_ms();
        self.mutate(move |data| {
            let mut id_map = std::collections::HashMap::new();
            for folder in folders {
                let name = folder.name.trim();
                if name.is_empty() {
                    continue;
                }
                let target = match data
                    .folders
                    .iter()
                    .find(|f| f.name.trim().to_lowercase() == name.to_lowercase())
                {
                    Some(existing) => existing.id.clone(),
                    None => {
                        let f = Folder {
                            id: util::new_id(),
                            name: truncate_chars(name, NAME_MAX_CHARS),
                        };
                        let id = f.id.clone();
                        data.folders.push(f);
                        id
                    }
                };
                id_map.insert(folder.id, target);
            }
            let count = items.len();
            for mut item in items {
                item.id = util::new_id();
                item.normalize();
                item.name = truncate_chars(item.name.trim(), NAME_MAX_CHARS);
                if item.name.is_empty() {
                    item.name = "?".to_owned();
                }
                item.folder_id = item.folder_id.and_then(|f| id_map.get(&f).cloned());
                item.deleted_at = None;
                if item.created_at <= 0 {
                    item.created_at = now;
                }
                if item.updated_at <= 0 {
                    item.updated_at = now;
                }
                item.password_history.truncate(PASSWORD_HISTORY_MAX);
                data.items.push(item);
            }
            Ok(count)
        })
    }

    /// Renames the vault.
    pub fn rename(&mut self, name: &str) -> Result<()> {
        let name = validate_name(name, "name")?;
        let mut header = self.file.clone();
        header.name = name;
        let data = self.data.clone();
        self.write(header, data)
    }

    /// True if `password` is the current master password.
    pub fn verify_master_password(&self, password: &str) -> bool {
        self.file
            .unwrap_key(password)
            .is_ok_and(|k| crypto::ct_eq(k.as_slice(), self.key.as_slice()))
    }

    /// Changes the master password (re-wraps the vault key only).
    pub fn change_master_password(&mut self, current: &str, new: &str) -> Result<()> {
        if !self.verify_master_password(current) {
            return Err(Error::WrongPassword);
        }
        if new.is_empty() {
            return Err(Error::invalid("password_empty"));
        }
        let mut header = self.file.clone();
        header.set_master_password(&self.key, new, self.file.kdf_params())?;
        let data = self.data.clone();
        self.write(header, data)
    }

    /// Creates a new recovery key (replacing an existing one) and returns
    /// it formatted as `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX`.
    pub fn create_recovery_key(&mut self) -> Result<String> {
        let code = format::generate_recovery_code()?;
        let mut header = self.file.clone();
        header.set_recovery_code(&self.key, &code)?;
        let data = self.data.clone();
        self.write(header, data)?;
        Ok(code)
    }

    pub fn remove_recovery_key(&mut self) -> Result<()> {
        let mut header = self.file.clone();
        header.recovery = None;
        let data = self.data.clone();
        self.write(header, data)
    }

    pub fn has_recovery_key(&self) -> bool {
        self.file.has_recovery()
    }

    /// Persists the data with a modified header (used by the store after a
    /// recovery-key unlock).
    pub(crate) fn replace_header_and_save(&mut self, header: VaultFile) -> Result<()> {
        let data = self.data.clone();
        self.write(header, data)
    }

    /// Persists the current state (atomic write, revision check, `.bak`).
    pub fn save(&mut self) -> Result<()> {
        let header = self.file.clone();
        let data = self.data.clone();
        self.write(header, data)
    }

    /// Re-reads the file if another process saved a newer revision; the
    /// in-memory vault key is used (the master password may have changed).
    /// Returns true if the data was reloaded.
    pub fn reload_if_changed(&mut self) -> Result<bool> {
        let on_disk = VaultFile::read(&self.path)?;
        if on_disk.revision == self.file.revision && on_disk.id == self.file.id {
            return Ok(false);
        }
        if on_disk.id != self.file.id {
            return Err(Error::corrupt("vault file was replaced by a different vault"));
        }
        let data = on_disk.decrypt_payload(&self.key)?;
        let mut old = std::mem::replace(&mut self.data, data);
        wipe_data(&mut old);
        self.file = on_disk;
        Ok(true)
    }

    /// Path of the vault file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Applies `f` to a copy of the data, persists it and commits it to
    /// memory only on success.
    fn mutate<R>(&mut self, f: impl FnOnce(&mut VaultData) -> Result<R>) -> Result<R> {
        let mut data = self.data.clone();
        let result = match f(&mut data) {
            Ok(r) => r,
            Err(e) => {
                wipe_data(&mut data);
                return Err(e);
            }
        };
        let header = self.file.clone();
        self.write(header, data)?;
        Ok(result)
    }

    /// Writes `header` + `data` as the next revision. Holds the inter-process
    /// lock while checking that the file on disk is still at the revision we
    /// loaded (`Error::Conflict` otherwise).
    fn write(&mut self, mut header: VaultFile, mut data: VaultData) -> Result<()> {
        let result = (|| {
            if !self.path.exists() {
                // Deleted by another process: never recreate it silently
                // (and do not leave a stray lock file behind).
                return Err(Error::NotFound(format!("vault file {}", self.path.display())));
            }
            let _lock = format::lock_vault_file(&self.path)?;
            let on_disk = VaultFile::read(&self.path)?;
            if on_disk.revision != self.file.revision || on_disk.id != self.file.id {
                return Err(Error::Conflict);
            }
            header.revision = self.file.revision + 1;
            header.updated_at = now_ms().max(self.file.updated_at);
            header.encrypt_payload(&self.key, &data)?;
            header.write_atomic(&self.path, true)
        })();
        match result {
            Ok(()) => {
                self.file = header;
                let mut old = std::mem::replace(&mut self.data, data);
                wipe_data(&mut old);
                Ok(())
            }
            Err(e) => {
                wipe_data(&mut data);
                Err(e)
            }
        }
    }
}

fn find_item_mut<'a>(data: &'a mut VaultData, id: &str) -> Result<&'a mut VaultItem> {
    data.items
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| Error::NotFound(format!("item {id}")))
}

fn sort_by_name(v: &mut [&VaultItem]) {
    v.sort_by_cached_key(|i| (i.name.to_lowercase(), i.id.clone()));
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn matches_terms(item: &VaultItem, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let mut haystack = Vec::with_capacity(4);
    haystack.push(item.name.to_lowercase());
    haystack.push(item.notes.to_lowercase());
    if let Some(l) = &item.login {
        haystack.push(l.username.to_lowercase());
        haystack.extend(l.uris.iter().map(|u| u.uri.to_lowercase()));
    }
    if let Some(c) = &item.card {
        haystack.push(c.brand.to_lowercase());
    }
    terms
        .iter()
        .all(|t| haystack.iter().any(|h| h.contains(t.as_str())))
}
