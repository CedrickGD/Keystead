//! An unlocked vault: decrypted data plus the vault key, persisted through
//! the v1 file format.
//!
//! Every mutating method works on a copy of the data and only commits it to
//! memory after the file was written successfully, so a failed save (for
//! example `Error::Conflict`) never leaves memory and disk out of sync.
//!
//! Replacing a secret that opens the vault (master password, recovery key)
//! also replaces the vault key ("rotation"), so the old secret together
//! with an older copy of the file (the `.bak`, a backup, a synced version)
//! cannot decrypt the current or any later revision.

use std::path::{Path, PathBuf};

use zeroize::Zeroize;

use crate::crypto::{self, SecretKey};
use crate::error::{Error, Result};
use crate::format::{self, VaultFile};
use crate::icons;
use crate::import::{self, ConflictMode, ImportPlan, ImportReport};
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
    /// Master-password key-encryption key for `file.kdf` (salt and
    /// parameters); always opens `file.wrapped_key` to `key`. Lets the
    /// recovery-key operations wrap a fresh vault key without asking for the
    /// password again, and lets [`UnlockedVault::reload_if_changed`] follow
    /// a vault key rotated by another process.
    master_kek: SecretKey,
    data: VaultData,
}

/// The new secrets of a save that replaces what opens the vault.
struct Rekey {
    key: SecretKey,
    master_kek: SecretKey,
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
pub(crate) fn wipe_data(data: &mut VaultData) {
    wipe_items(&mut data.items);
    for g in &mut data.generator_history {
        g.password.zeroize();
    }
    icons::wipe_icons(&mut data.icons);
}

/// Overwrites the secret strings of items before the memory is freed.
pub(crate) fn wipe_items(items: &mut [VaultItem]) {
    for item in items {
        wipe_item(item);
    }
}

/// Overwrites the secret strings of one item.
pub(crate) fn wipe_item(item: &mut VaultItem) {
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
    pub(crate) fn from_parts(
        path: PathBuf,
        file: VaultFile,
        key: SecretKey,
        master_kek: SecretKey,
        data: VaultData,
    ) -> Self {
        UnlockedVault {
            path,
            file,
            key,
            master_kek,
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
    pub fn save_item(&mut self, mut item: VaultItem) -> Result<VaultItem> {
        item.name = validate_name(&item.name, "name")?;
        item.normalize();
        let now = now_ms();
        self.mutate(move |data| Ok(store_item(data, item, now)))
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

    /// Hosts of non-trashed logins whose website icon should be fetched at
    /// `now` (Unix ms): none stored yet, the last attempt 30 days old, or
    /// the last failure 7 days old. Hosts that must not be contacted (IP
    /// literals, `localhost`, `.local`, intranet names) are left out. See
    /// [`icons::hosts_needing_fetch`].
    pub fn icon_hosts_needing_fetch(&self, now: i64) -> Vec<String> {
        icons::hosts_needing_fetch(&self.data, now)
    }

    /// The stored icon of `host` (base64 of a 64×64 PNG), if any.
    pub fn icon_for_host(&self, host: &str) -> Option<&str> {
        self.data.icons.get(host)?.png.as_deref()
    }

    /// The stored icon of an item ([`icons::icon_host`]), if any.
    pub fn icon_for_item(&self, item: &VaultItem) -> Option<&str> {
        self.icon_for_host(&icons::icon_host(item)?)
    }

    /// Stores the results of an icon fetch run in **one** save: PNG bytes,
    /// or `None` for a failed fetch (keeps an older icon, retried after 7
    /// days). Hosts that no longer belong to a non-trashed login are
    /// ignored, and icons of such hosts are removed. Returns the number of
    /// entries written; nothing is saved if nothing changed. On
    /// `Error::Conflict` nothing changed: `reload_if_changed()` and call
    /// again with the same results.
    pub fn set_icons(&mut self, results: &[(String, Option<Vec<u8>>)], now: i64) -> Result<usize> {
        if results.is_empty() && self.data.icons.is_empty() {
            return Ok(0);
        }
        self.mutate_if_changed(|data| {
            let written = icons::apply_icons(data, results, now);
            let pruned = icons::prune_icons(data);
            Ok((written, written > 0 || pruned > 0))
        })
    }

    /// Removes every stored website icon (one save). Returns how many.
    pub fn clear_icons(&mut self) -> Result<usize> {
        let count = self.data.icons.len();
        if count == 0 {
            return Ok(0);
        }
        self.mutate(|data| {
            icons::wipe_icons(&mut data.icons);
            Ok(count)
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
        self.mutate(move |data| Ok(add_imported(data, items, folders, now)))
    }

    /// Applies an import plan from [`import::plan_import`] in one save (one
    /// revision; nothing is written if nothing changes).
    ///
    /// The incoming items are classified again against the *current* data –
    /// the vault may have changed since the plan was made:
    ///
    /// * an item that became a duplicate is skipped (`duplicates`);
    /// * a conflict whose login is gone (deleted, trashed, or no longer the
    ///   same site and username) is added as a new item if no other login
    ///   matches it;
    /// * `mode` applies only to the conflicts of the preview, and only while
    ///   they still conflict with the login the preview named. Whatever
    ///   else conflicts now – an item planned as new, or a conflict that now
    ///   meets another login – is not imported (`conflictsSkipped`) in
    ///   every mode, so the commit never touches a login the user did not
    ///   decide about.
    ///
    /// Then:
    ///
    /// * New items get fresh ids; folders of the file are merged by name
    ///   into existing folders or created – only those an added item uses.
    /// * Conflicts: [`ConflictMode::Skip`] leaves them out
    ///   (`conflictsSkipped`); [`ConflictMode::KeepBoth`] adds the incoming
    ///   login as a new item; [`ConflictMode::Update`] takes the incoming
    ///   password over into the existing login through the normal save path
    ///   (the old password goes into the password history,
    ///   `passwordRevisedAt` is set) and its TOTP seed if the existing login
    ///   has none. An existing TOTP seed is never replaced and an empty
    ///   incoming password never clears one; a conflict with nothing to take
    ///   over is reported in `conflictsSkipped`. A login is updated at most
    ///   once per commit.
    /// * Conflicts the caller removed from `plan.conflicts` are not imported
    ///   either and are reported in `conflictsSkipped` as planned.
    ///
    /// The plan is dropped (and its secrets overwritten) either way; use
    /// [`Self::commit_import_ref`] to keep it for a retry after an error
    /// such as `Error::Conflict`.
    pub fn commit_import(&mut self, plan: ImportPlan, mode: ConflictMode) -> Result<ImportReport> {
        self.commit_import_ref(&plan, mode)
    }

    /// [`Self::commit_import`] without consuming the plan: on an error
    /// (`conflict` → `reload_if_changed()`, then retry) nothing has changed
    /// and the same plan can be committed again. Committing a plan a second
    /// time after success adds nothing – its items are duplicates by then.
    pub fn commit_import_ref(
        &mut self,
        plan: &ImportPlan,
        mode: ConflictMode,
    ) -> Result<ImportReport> {
        let now = now_ms();
        // Lists for the UI are capped (`IMPORT_LIST_LIMIT`), the counts are
        // complete – a file with millions of duplicates or warnings must not
        // turn into a report of that size.
        let mut report = ImportReport {
            skipped: plan.invalid,
            duplicates: import::first(&plan.duplicates),
            duplicate_count: plan.duplicates.len(),
            warnings: import::first(&plan.warnings),
            warning_count: plan.warning_count(),
            ..Default::default()
        };
        self.mutate_if_changed(move |data| {
            let resolved = import::resolve_import(&data.items, plan, mode);
            report.duplicate_count += resolved.duplicates.len();
            let room = import::IMPORT_LIST_LIMIT.saturating_sub(report.duplicates.len());
            report
                .duplicates
                .extend(resolved.duplicates.into_iter().take(room));
            report.conflicts_skipped = resolved.conflicts_skipped;
            for update in resolved.updates {
                let import::PendingUpdate {
                    position,
                    mut incoming,
                    matched,
                } = update;
                match import::take_over(&data.items[position], &incoming) {
                    Some(item) => {
                        let mut saved = store_item(data, item, now);
                        wipe_item(&mut saved);
                        report.updated += 1;
                    }
                    None => report.conflicts_skipped.push(matched),
                }
                wipe_item(&mut incoming);
            }
            let to_add = resolved.to_add;
            let mut folders = plan.folders.clone();
            folders.retain(|f| {
                to_add
                    .iter()
                    .any(|i| i.folder_id.as_deref() == Some(f.id.as_str()))
            });
            report.imported = add_imported(data, to_add, folders, now);
            let changed = report.imported > 0 || report.updated > 0;
            Ok((report, changed))
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

    /// Changes the master password and rotates the vault key: a fresh key
    /// encrypts the data, so the old password does not open this or any
    /// later revision, even together with an older copy of the file. No
    /// `.bak` of the previous revision is left behind.
    ///
    /// The recovery code of an existing recovery key is not known here, and
    /// the old recovery box only opens the old key. So a vault with a
    /// recovery key gets a **new** one (wrapping the new key), formatted as
    /// `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX` and returned as `Some`; the old code
    /// stops working like the old password. `None` without recovery key.
    pub fn change_master_password(&mut self, current: &str, new: &str) -> Result<Option<String>> {
        if !self.verify_master_password(current) {
            return Err(Error::WrongPassword);
        }
        if new.is_empty() {
            return Err(Error::invalid("password_empty"));
        }
        let key = crypto::random_key()?;
        let mut header = self.file.clone();
        let master_kek = header.set_master_password_kek(&key, new, self.file.kdf_params())?;
        let mut code = None;
        if header.has_recovery() {
            let new_code = format::generate_recovery_code()?;
            header.set_recovery_code(&key, &new_code)?;
            code = Some(new_code);
        }
        let data = self.data.clone();
        if let Err(e) = self.commit(header, data, Some(Rekey { key, master_kek })) {
            // Never written: the code opens nothing.
            code.zeroize();
            return Err(e);
        }
        Ok(code)
    }

    /// Creates a new recovery key (replacing an existing one) and returns
    /// it formatted as `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX`. Rotates the vault
    /// key: a replaced recovery code does not open this or any later
    /// revision, even together with an older copy of the file.
    pub fn create_recovery_key(&mut self) -> Result<String> {
        let code = format::generate_recovery_code()?;
        let (mut header, rekey) = self.rotated_header()?;
        header.set_recovery_code(&rekey.key, &code)?;
        let data = self.data.clone();
        self.commit(header, data, Some(rekey))?;
        Ok(code)
    }

    /// Removes the recovery key. Rotates the vault key (see
    /// [`Self::create_recovery_key`]).
    pub fn remove_recovery_key(&mut self) -> Result<()> {
        let (mut header, rekey) = self.rotated_header()?;
        header.recovery = None;
        let data = self.data.clone();
        self.commit(header, data, Some(rekey))
    }

    pub fn has_recovery_key(&self) -> bool {
        self.file.has_recovery()
    }

    /// A copy of the header with a fresh vault key wrapped by the (unchanged)
    /// master password. The recovery box still wraps the old key: callers
    /// replace or remove it.
    fn rotated_header(&self) -> Result<(VaultFile, Rekey)> {
        // Invariant check: the cached KEK opens the current header.
        let current = self
            .file
            .unwrap_key_with_kek(&self.master_kek)
            .map_err(|_| Error::KeyChanged)?;
        if !crypto::ct_eq(current.as_slice(), self.key.as_slice()) {
            return Err(Error::KeyChanged);
        }
        let key = crypto::random_key()?;
        let mut header = self.file.clone();
        header.wrap_key_with_kek(&self.master_kek, &key)?;
        Ok((
            header,
            Rekey {
                key,
                master_kek: self.master_kek.clone(),
            },
        ))
    }

    /// Persists the data under a header whose key wrappings belong to
    /// `key`/`master_kek` (used by the store after a recovery-key unlock).
    pub(crate) fn rekey_and_save(
        &mut self,
        header: VaultFile,
        key: SecretKey,
        master_kek: SecretKey,
    ) -> Result<()> {
        let data = self.data.clone();
        self.commit(header, data, Some(Rekey { key, master_kek }))
    }

    /// Persists the current state (atomic write, revision check, `.bak`).
    pub fn save(&mut self) -> Result<()> {
        let header = self.file.clone();
        let data = self.data.clone();
        self.write(header, data)
    }

    /// Re-reads the file if another process saved a newer revision.
    /// Returns true if the data was reloaded.
    ///
    /// * A different vault id → `Error::Corrupt`.
    /// * An older revision than the one in memory → `Error::Rollback`
    ///   (restored backup/copy or tampering); the newer in-memory state is
    ///   kept, and saving keeps failing with `Error::Conflict`.
    /// * A newer revision is opened with this session's master KEK (the
    ///   vault key may have been rotated by a recovery-key change). If that
    ///   does not open it – the master password was changed elsewhere –
    ///   `Error::KeyChanged`: the vault has to be unlocked again.
    pub fn reload_if_changed(&mut self) -> Result<bool> {
        let on_disk = VaultFile::read(&self.path)?;
        if on_disk.id != self.file.id {
            return Err(Error::corrupt(
                "vault file was replaced by a different vault",
            ));
        }
        if on_disk.revision == self.file.revision {
            return Ok(false);
        }
        if on_disk.revision < self.file.revision {
            return Err(Error::Rollback);
        }
        let key = on_disk
            .unwrap_key_with_kek(&self.master_kek)
            .map_err(|e| match e {
                Error::WrongPassword => Error::KeyChanged,
                other => other,
            })?;
        let data = on_disk.decrypt_payload(&key)?;
        let mut old = std::mem::replace(&mut self.data, data);
        wipe_data(&mut old);
        self.file = on_disk;
        self.key = key;
        Ok(true)
    }

    /// Path of the vault file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Applies `f` to a copy of the data, persists it and commits it to
    /// memory only on success.
    fn mutate<R>(&mut self, f: impl FnOnce(&mut VaultData) -> Result<R>) -> Result<R> {
        self.mutate_if_changed(|data| f(data).map(|r| (r, true)))
    }

    /// Like [`Self::mutate`], but `f` also reports whether it changed
    /// anything; if not, nothing is written (no new revision).
    fn mutate_if_changed<R>(
        &mut self,
        f: impl FnOnce(&mut VaultData) -> Result<(R, bool)>,
    ) -> Result<R> {
        let mut data = self.data.clone();
        let (result, changed) = match f(&mut data) {
            Ok(r) => r,
            Err(e) => {
                wipe_data(&mut data);
                return Err(e);
            }
        };
        if !changed {
            wipe_data(&mut data);
            return Ok(result);
        }
        // Icons of hosts no login uses any more (deleted, trashed, address
        // changed) leave the vault with the change.
        icons::prune_icons(&mut data);
        let header = self.file.clone();
        self.write(header, data)?;
        Ok(result)
    }

    /// Writes `header` + `data` as the next revision with the current keys.
    fn write(&mut self, header: VaultFile, data: VaultData) -> Result<()> {
        self.commit(header, data, None)
    }

    /// Writes `header` + `data` as the next revision. Holds the inter-process
    /// lock while checking that the file on disk is still at the revision we
    /// loaded (`Error::Conflict` otherwise).
    ///
    /// With `rekey` the payload is encrypted with `rekey.key` (the header
    /// already carries the matching wrappings), the previous revision is not
    /// kept as `.bak` (it would still open with the replaced secret) and the
    /// in-memory keys are replaced once the file was written.
    fn commit(
        &mut self,
        mut header: VaultFile,
        mut data: VaultData,
        rekey: Option<Rekey>,
    ) -> Result<()> {
        let result = (|| {
            if !self.path.exists() {
                // Deleted by another process: never recreate it silently
                // (and do not leave a stray lock file behind).
                return Err(Error::NotFound(format!(
                    "vault file {}",
                    self.path.display()
                )));
            }
            let _lock = format::lock_vault_file(&self.path)?;
            let on_disk = VaultFile::read(&self.path)?;
            if on_disk.revision != self.file.revision || on_disk.id != self.file.id {
                return Err(Error::Conflict);
            }
            header.revision = self.file.revision + 1;
            header.updated_at = now_ms().max(self.file.updated_at);
            match &rekey {
                None => {
                    header.encrypt_payload(&self.key, &data)?;
                    header.write_atomic(&self.path, true)
                }
                Some(rekey) => {
                    header.encrypt_payload(&rekey.key, &data)?;
                    header.write_atomic(&self.path, false)?;
                    format::supersede_backup(&self.path);
                    Ok(())
                }
            }
        })();
        match result {
            Ok(()) => {
                self.file = header;
                let mut old = std::mem::replace(&mut self.data, data);
                wipe_data(&mut old);
                if let Some(rekey) = rekey {
                    self.key = rekey.key;
                    self.master_kek = rekey.master_kek;
                }
                Ok(())
            }
            Err(e) => {
                wipe_data(&mut data);
                Err(e)
            }
        }
    }
}

/// The data part of [`UnlockedVault::save_item`] (name validated and item
/// normalised by the caller): creates or replaces the item, maintains the
/// password history. Returns the stored item.
fn store_item(data: &mut VaultData, mut item: VaultItem, now: i64) -> VaultItem {
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
            let old_revised = old.login.as_ref().and_then(|l| l.password_revised_at);
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
                } else {
                    // Maintained by the core only.
                    login.password_revised_at = old_revised;
                }
            }
            item.password_history.truncate(PASSWORD_HISTORY_MAX);
            item.updated_at = now;
            let mut old = std::mem::replace(&mut data.items[pos], item.clone());
            wipe_item(&mut old);
        }
        None => {
            item.id = util::new_id();
            item.created_at = now;
            item.updated_at = now;
            item.deleted_at = None;
            if let Some(login) = item.login.as_mut() {
                login.password_revised_at = None;
            }
            item.password_history.truncate(PASSWORD_HISTORY_MAX);
            data.items.push(item.clone());
        }
    }
    item
}

/// The data part of [`UnlockedVault::import_items`]: adds the items with
/// fresh ids, merges or creates the folders and maps the folder references.
/// Returns the number of added items.
fn add_imported(
    data: &mut VaultData,
    items: Vec<VaultItem>,
    folders: Vec<Folder>,
    now: i64,
) -> usize {
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
    count
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
