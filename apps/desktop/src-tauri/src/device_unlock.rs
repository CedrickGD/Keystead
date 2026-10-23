//! Fingerprint unlock on the phone ("Mit Fingerabdruck entsperren", Block 5
//! stage 1b, decision E5) – the platform-neutral part, unit-tested on the
//! desktop host: the stored blob per vault, what exactly the keystore
//! seals, and the status / enable / disable / unlock flows over a
//! [`Keystore`] (on Android the Android Keystore through
//! `tauri-plugin-biometric`, see `mobile::biometric`; a fake in the tests).
//! See "Fingerprint unlock (phone)" under "Mobile (Android)" in
//! docs/ARCHITECTURE.md.
//!
//! * **Stored**: `<data>/device-unlock/<vault-id>.json` ([`Blob`]) – the
//!   vault's master KEK (`UnlockedVault::master_kek`: what the master
//!   password derives, it opens the header's wrapped vault key) **sealed by
//!   the keystore** with a key that only a strong biometric releases, and
//!   the KDF salt that KEK belongs to. The KEK itself never reaches the
//!   disk or the webview.
//! * **Sealed plaintext**: `keystead-device-unlock:1:<vault-id>:<base64
//!   KEK>` ([`seal_input`] / [`kek_of`]) – the keystore's AES-GCM has no
//!   associated data, so the vault id inside binds the KEK to its vault (a
//!   blob copied to another vault's name opens nothing).
//! * **Unlock** ([`unlock`]): blob → the vault file still has the blob's
//!   salt → keystore (the system's fingerprint prompt) → KEK →
//!   `VaultStore::unlock_with_master_kek` → `Core::install_vault_since`,
//!   the same end as the master password (no Argon2 step).
//! * **Invalid** – the blob is removed and the vault is remembered as reset
//!   ([`DeviceUnlockStatus::invalidated`]; `unsupported:device_unlock_invalid`,
//!   the UI asks for the master password and offers to set it up again): a
//!   damaged or foreign blob, a new master password (another salt in the
//!   vault file: changed on the PC and synced – also `Error::KeyChanged` of
//!   the open vault), a keystore key that is gone or was invalidated (a new
//!   fingerprint enrolled, the screen lock removed), a KEK that does not
//!   open the vault. A cancelled prompt, a lockout or an unrecognised
//!   finger keep the blob.
//! * **Off** (`Settings.deviceUnlock` false, the default) means no blob
//!   anywhere: [`status`] removes leftovers, [`unlock`] refuses. Only the
//!   commands here change the setting (`save_settings` keeps it).

// Only the Android build reaches these flows through a keystore (the
// biometric plugin's cipher); the desktop compiles this module for its unit
// tests and iOS has no keystore cipher.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use keystead_core::crypto::{self, SecretKey, KEY_LEN};
use keystead_core::format::{self, VaultFile};
use keystead_core::model::VaultInfo;
use keystead_core::{sync, Error as CoreError};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::state::{log, Core};

/// `<data>/device-unlock`.
pub const DIR: &str = "device-unlock";
/// [`Blob::version`] this build writes and reads.
const BLOB_VERSION: u32 = 1;
/// Start of the sealed plaintext (see the module docs).
const PLAINTEXT_PREFIX: &str = "keystead-device-unlock:1:";

/// Error detail: the stored setup no longer opens the vault and was
/// removed (see the module docs).
pub const INVALID: &str = "device_unlock_invalid";
/// Error detail: no fingerprint unlock is set up for this vault (or the
/// feature is off).
pub const NOT_SET: &str = "device_unlock_not_set";
/// Error detail: the vault's copy came from the cloud folder and the
/// stored setup does not fit it; the setup is kept until the copy opens
/// with the master password (`sync_trial::on_trial`).
pub const NEEDS_PASSWORD: &str = "device_unlock_needs_password";

/// What `<data>/device-unlock/<vault-id>.json` holds. No secret in the
/// clear: `sealed` is the keystore's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    pub version: u32,
    pub vault_id: String,
    /// The vault header's KDF salt (base64) the sealed KEK was derived
    /// with: another salt in the vault file = a new master password.
    pub kdf_salt: String,
    /// The keystore's sealed form of [`seal_input`] (the biometric
    /// plugin: `base64(ciphertext);base64(iv)`) – opaque here.
    pub sealed: String,
    /// When it was set up (ms).
    pub created_at: i64,
}

/// The texts of the system's fingerprint prompt (from the UI, translated).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub title: String,
    /// The line under the title (what the prompt is for).
    pub reason: String,
    /// The button that closes the prompt (back to the master password).
    pub cancel: String,
}

/// Why the keystore cannot be used at all right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unavailable {
    /// No fingerprint sensor (or no strong biometric) on this device.
    NoHardware,
    /// The device has one, but no fingerprint is enrolled (Android needs a
    /// screen lock for that).
    NotEnrolled,
    /// Temporarily unavailable (the sensor is busy), or anything else.
    Other,
}

/// A failed keystore operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeystoreError {
    /// The user closed the prompt, or the system did (the app went to the
    /// background, a timeout).
    Cancelled,
    /// Too many unrecognised attempts; the sensor is locked for a while.
    Lockout,
    /// The finger was not recognised.
    NotRecognized,
    /// See [`Unavailable`].
    Unavailable(Unavailable),
    /// The keystore cannot open what was stored: its key is gone or was
    /// invalidated (a new fingerprint, the screen lock removed), or the
    /// sealed data was damaged.
    Invalidated,
    /// Another prompt is open.
    Busy,
    /// Anything else (the message is logged, never a secret).
    Other(String),
}

impl From<KeystoreError> for AppError {
    fn from(e: KeystoreError) -> Self {
        match e {
            KeystoreError::Cancelled => AppError::unsupported("device_unlock_cancelled"),
            KeystoreError::Lockout => AppError::unsupported("device_unlock_lockout"),
            KeystoreError::NotRecognized => AppError::unsupported("device_unlock_failed"),
            KeystoreError::Unavailable(Unavailable::NotEnrolled) => {
                AppError::unsupported("device_unlock_not_enrolled")
            }
            KeystoreError::Unavailable(_) => AppError::unsupported("device_unlock_unavailable"),
            KeystoreError::Invalidated => AppError::unsupported(INVALID),
            KeystoreError::Busy => AppError::unsupported("device_unlock_busy"),
            KeystoreError::Other(message) => AppError::io(format_args!("device_unlock: {message}")),
        }
    }
}

/// A rejection of the biometric plugin's `biometricCipher` (Android:
/// `invoke.reject(message, code)` in its `BiometricPlugin.kt` 2.5) as a
/// [`KeystoreError`]; `opening`: a decryption. Pure, so the mapping is
/// tested on the host.
///
/// The codes come from the plugin's table of `BiometricPrompt` errors. Its
/// own failures carry an empty code and one of two messages: "Failed to
/// initialize the cipher: …" (the key was invalidated – a new fingerprint,
/// the screen lock removed – or the stored data is malformed) or "Failed to
/// decrypt the data: …" (another key, altered data). For a decryption both
/// mean the stored setup is useless; encrypting replaces an invalidated key
/// itself. Anything else (an error code missing from the plugin's table, a
/// broken activity result) is [`KeystoreError::Other`] and keeps the setup.
pub fn plugin_cipher_error(code: Option<&str>, message: &str, opening: bool) -> KeystoreError {
    match code {
        // The user's cancel button or back, or the system's (the app left
        // the foreground, a timeout).
        Some("userCancel" | "systemCancel") => KeystoreError::Cancelled,
        Some("biometryLockout") => KeystoreError::Lockout,
        Some("authenticationFailed") => KeystoreError::NotRecognized,
        Some("biometryNotEnrolled") => KeystoreError::Unavailable(Unavailable::NotEnrolled),
        Some("biometryNotAvailable" | "passcodeNotSet") => {
            KeystoreError::Unavailable(Unavailable::Other)
        }
        None | Some("")
            if opening
                && (message.starts_with("Failed to initialize the cipher")
                    || message.starts_with("Failed to decrypt the data")) =>
        {
            KeystoreError::Invalidated
        }
        _ => KeystoreError::Other(message.to_owned()),
    }
}

/// The biometric plugin's `status` (`isAvailable`, `errorCode`, `error`) as
/// [`Keystore::availability`]. Pure, see [`plugin_cipher_error`]. (The
/// plugin asks `BiometricManager` about weak biometrics; the cipher's prompt
/// asks for strong ones – a device with only weak ones fails there.)
pub fn plugin_availability(
    is_available: bool,
    error_code: Option<&str>,
    error: Option<&str>,
) -> Result<(), Unavailable> {
    if is_available {
        return Ok(());
    }
    Err(match (error_code, error) {
        (Some("biometryNotEnrolled"), _) => Unavailable::NotEnrolled,
        (_, Some("No biometric on this device.")) => Unavailable::NoHardware,
        _ => Unavailable::Other,
    })
}

/// The platform keystore: seals and opens text with a key that only a
/// strong biometric releases (each call shows the system prompt).
pub trait Keystore {
    /// Whether a strong biometric can be used now.
    fn availability(&self) -> Result<(), Unavailable>;
    /// Seals `plaintext` (after the user's fingerprint).
    fn seal(&self, plaintext: &str, prompt: &Prompt) -> Result<String, KeystoreError>;
    /// Opens what [`Keystore::seal`] returned (after the user's
    /// fingerprint).
    fn open(&self, sealed: &str, prompt: &Prompt) -> Result<Zeroizing<String>, KeystoreError>;
}

/// `device_unlock_status`: what the UI may offer. No secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceUnlockStatus {
    /// A strong biometric can be used on this device right now.
    pub available: bool,
    /// Why not (`null` when available).
    pub unavailable_reason: Option<Unavailable>,
    /// `Settings.deviceUnlock`.
    pub enabled: bool,
    /// The vaults that open with the fingerprint (ids, sorted).
    pub vaults: Vec<String>,
    /// Vaults whose setup was removed since the app started because it no
    /// longer opened them (see the module docs) – the UI explains it and
    /// offers to set it up again.
    pub invalidated: Vec<String>,
}

// ---------------------------------------------------------------------------
// The blob files
// ---------------------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// `<data>/device-unlock`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// A vault id as the app names its vault files (letters, digits, `-`,
/// `_`; at most 64) – nothing else becomes a file name here.
fn valid_vault_id(vault_id: &str) -> bool {
    sync::vault_id_of_file_name(&format!("{vault_id}.{}", format::VAULT_EXTENSION)).is_some()
}

/// `<data>/device-unlock/<vault-id>.json`; another id → `not_found`.
pub fn blob_path(data_dir: &Path, vault_id: &str) -> AppResult<PathBuf> {
    if !valid_vault_id(vault_id) {
        return Err(AppError::Code("not_found".into()));
    }
    Ok(dir(data_dir).join(format!("{vault_id}.json")))
}

fn invalid() -> AppError {
    AppError::unsupported(INVALID)
}

fn not_set() -> AppError {
    AppError::unsupported(NOT_SET)
}

fn needs_password() -> AppError {
    AppError::unsupported(NEEDS_PASSWORD)
}

/// Writes `blob` atomically (replaces an older one of that vault).
pub fn store_blob(data_dir: &Path, blob: &Blob) -> AppResult<()> {
    let path = blob_path(data_dir, &blob.vault_id)?;
    fs::create_dir_all(dir(data_dir))?;
    let bytes =
        serde_json::to_vec_pretty(blob).map_err(|e| AppError::io(format_args!("blob: {e}")))?;
    format::write_atomic(&path, &bytes, false)?;
    Ok(())
}

/// The blob of `vault_id`: `None` without one; a file that is not a blob of
/// this vault (damaged, another version, another vault's, empty fields) →
/// `unsupported:device_unlock_invalid` – the caller removes it.
pub fn load_blob(data_dir: &Path, vault_id: &str) -> AppResult<Option<Blob>> {
    let path = blob_path(data_dir, vault_id)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let blob: Blob = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let usable = blob.version == BLOB_VERSION
        && blob.vault_id == vault_id
        && !blob.kdf_salt.is_empty()
        && !blob.sealed.is_empty();
    if usable {
        Ok(Some(blob))
    } else {
        Err(invalid())
    }
}

/// Removes the blob of `vault_id`; whether there was one.
pub fn remove_blob(data_dir: &Path, vault_id: &str) -> AppResult<bool> {
    let path = blob_path(data_dir, vault_id)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Removes every blob (and the folder).
pub fn remove_all_blobs(data_dir: &Path) -> AppResult<()> {
    match fs::remove_dir_all(dir(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The vault ids with a blob file (sorted; whether the file is usable is
/// [`load_blob`]'s question). Other files in the folder are ignored.
pub fn enrolled_vaults(data_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir(data_dir)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?;
            valid_vault_id(id).then(|| id.to_owned())
        })
        .collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// The sealed plaintext
// ---------------------------------------------------------------------------

/// What the keystore seals for `vault_id` (see the module docs). Wiped on
/// drop.
pub fn seal_input(vault_id: &str, kek: &SecretKey) -> Zeroizing<String> {
    let encoded = Zeroizing::new(crypto::b64_encode(kek.as_slice()));
    let mut plaintext = Zeroizing::new(String::with_capacity(
        PLAINTEXT_PREFIX.len() + vault_id.len() + 1 + encoded.len(),
    ));
    plaintext.push_str(PLAINTEXT_PREFIX);
    plaintext.push_str(vault_id);
    plaintext.push(':');
    plaintext.push_str(&encoded);
    plaintext
}

/// The KEK in what the keystore opened for `vault_id`; anything but
/// [`seal_input`] of this vault → `unsupported:device_unlock_invalid`.
pub fn kek_of(vault_id: &str, plaintext: &str) -> AppResult<SecretKey> {
    let rest = plaintext
        .strip_prefix(PLAINTEXT_PREFIX)
        .ok_or_else(invalid)?;
    let (id, encoded) = rest.split_once(':').ok_or_else(invalid)?;
    if id != vault_id {
        return Err(invalid());
    }
    let bytes = Zeroizing::new(crypto::b64_decode(encoded).map_err(|_| invalid())?);
    if bytes.len() != KEY_LEN {
        return Err(invalid());
    }
    let mut kek: SecretKey = Zeroizing::new([0u8; KEY_LEN]);
    kek.copy_from_slice(&bytes);
    Ok(kek)
}

// ---------------------------------------------------------------------------
// Invalidated setups (in memory, since the start)
// ---------------------------------------------------------------------------

static INVALIDATED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn invalidated() -> MutexGuard<'static, BTreeSet<String>> {
    INVALIDATED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Removes the blob of `vault_id` because it no longer opens the vault
/// and remembers the vault as reset (see the module docs). Best effort.
pub fn invalidate(data_dir: &Path, vault_id: &str, why: &str) {
    match remove_blob(data_dir, vault_id) {
        Ok(true) => {
            log(format_args!("fingerprint unlock of a vault reset: {why}"));
            invalidated().insert(vault_id.to_owned());
        }
        Ok(false) => {}
        Err(e) => log(format_args!(
            "could not remove a fingerprint unlock setup: {}",
            e.code()
        )),
    }
}

/// After a new copy of vault `vault_id` was stored (cloud-folder sync): a
/// blob for another salt – a new master password – is invalid now; so is a
/// damaged one.
pub fn forget_if_stale(data_dir: &Path, vault_id: &str, current_salt: &str) {
    match load_blob(data_dir, vault_id) {
        Ok(Some(blob)) if blob.kdf_salt == current_salt => {}
        Ok(Some(_)) => invalidate(data_dir, vault_id, "the master password was changed"),
        Ok(None) => {}
        Err(_) => invalidate(data_dir, vault_id, "damaged"),
    }
}

// ---------------------------------------------------------------------------
// The flows (commands in `mobile::biometric`)
// ---------------------------------------------------------------------------

/// Sets `Settings.deviceUnlock` (persisted) if it differs.
fn set_enabled(core: &Core, enabled: bool) {
    let mut st = core.state();
    if st.settings.device_unlock != enabled {
        st.settings.device_unlock = enabled;
        st.save_settings();
    }
}

/// The status, after tidying up: off → no blob is kept; a blob whose vault
/// is gone is removed, one that is damaged or whose salt is no longer the
/// vault file's is invalidated.
pub fn status(core: &Core, keystore: &dyn Keystore) -> DeviceUnlockStatus {
    let data_dir = core.data_dir();
    let enabled = core.state().settings.device_unlock;
    if enabled {
        let store = core.store();
        for vault_id in enrolled_vaults(&data_dir) {
            let file = match VaultFile::read(&store.vault_path(&vault_id)) {
                Ok(file) => file,
                Err(CoreError::NotFound(_)) => {
                    // The vault is gone: nothing to explain.
                    let _ = remove_blob(&data_dir, &vault_id);
                    continue;
                }
                // Unreadable right now: decide at the next unlock.
                Err(_) => continue,
            };
            // A copy on trial proves no new salt (see `unlock`).
            if !crate::sync_trial::on_trial(&data_dir, &vault_id) {
                forget_if_stale(&data_dir, &vault_id, &file.kdf.salt);
            }
        }
    } else if let Err(e) = remove_all_blobs(&data_dir) {
        log(format_args!(
            "could not remove the fingerprint unlock setups: {}",
            e.code()
        ));
    }
    let availability = keystore.availability();
    DeviceUnlockStatus {
        available: availability.is_ok(),
        unavailable_reason: availability.err(),
        enabled,
        vaults: if enabled {
            enrolled_vaults(&data_dir)
        } else {
            Vec::new()
        },
        invalidated: invalidated().iter().cloned().collect(),
    }
}

/// Sets up fingerprint unlock for the open vault the page shows
/// (`page_vault_id`; another open vault or none → `locked`): its master
/// KEK is sealed by the keystore (the system prompt), the blob stored and
/// `Settings.deviceUnlock` turned on.
pub fn enable(
    core: &Core,
    keystore: &dyn Keystore,
    page_vault_id: Option<&str>,
    prompt: &Prompt,
) -> AppResult<DeviceUnlockStatus> {
    let (vault_id, plaintext, kdf_salt) = {
        let st = core.state();
        let vault = st.page_vault(page_vault_id)?;
        let kek = vault.master_kek();
        (
            vault.id().to_owned(),
            seal_input(vault.id(), &kek),
            vault.kdf_salt().to_owned(),
        )
    };
    keystore
        .availability()
        .map_err(KeystoreError::Unavailable)?;
    let sealed = keystore.seal(&plaintext, prompt)?;
    drop(plaintext);
    let data_dir = core.data_dir();
    store_blob(
        &data_dir,
        &Blob {
            version: BLOB_VERSION,
            vault_id: vault_id.clone(),
            kdf_salt,
            sealed,
            created_at: now_ms(),
        },
    )?;
    set_enabled(core, true);
    invalidated().remove(&vault_id);
    Ok(status(core, keystore))
}

/// Turns fingerprint unlock off for `vault_id`, or for every vault
/// (`None`); without any vault left the setting goes off. The blobs are
/// removed (the keystore key itself stays – without a blob it opens
/// nothing).
pub fn disable(
    core: &Core,
    keystore: &dyn Keystore,
    vault_id: Option<&str>,
) -> AppResult<DeviceUnlockStatus> {
    let data_dir = core.data_dir();
    match vault_id {
        Some(vault_id) => {
            remove_blob(&data_dir, vault_id)?;
            invalidated().remove(vault_id);
            if enrolled_vaults(&data_dir).is_empty() {
                set_enabled(core, false);
            }
        }
        None => {
            remove_all_blobs(&data_dir)?;
            invalidated().clear();
            set_enabled(core, false);
        }
    }
    Ok(status(core, keystore))
}

/// Unlocks `vault_id` with the fingerprint (see the module docs).
/// `unsupported:device_unlock_not_set` without a setup (or with the
/// feature off), `unsupported:device_unlock_invalid` when the setup no
/// longer opens the vault (removed), `not_found` for a vault that is gone;
/// the keystore's errors ([`KeystoreError`]) keep the setup. A lock while
/// the vault opens wins (`locked`, like a password unlock).
pub fn unlock(
    core: &Core,
    keystore: &dyn Keystore,
    vault_id: &str,
    prompt: &Prompt,
) -> AppResult<VaultInfo> {
    let data_dir = core.data_dir();
    if !core.state().settings.device_unlock {
        return Err(not_set());
    }
    let blob = match load_blob(&data_dir, vault_id) {
        Ok(Some(blob)) => blob,
        Ok(None) => return Err(not_set()),
        Err(_) => {
            invalidate(&data_dir, vault_id, "damaged");
            return Err(invalid());
        }
    };
    let file = match VaultFile::read(&core.store().vault_path(vault_id)) {
        Ok(file) => file,
        Err(e @ CoreError::NotFound(_)) => {
            let _ = remove_blob(&data_dir, vault_id);
            return Err(e.into());
        }
        Err(e) => return Err(e.into()),
    };
    // A new master password: no prompt for a KEK that cannot open it. A copy
    // on trial (from the cloud folder, not opened here yet) proves no new
    // password – anyone who can write the folder can change its salt: the
    // setup stays until the copy opens with the password.
    if file.kdf.salt != blob.kdf_salt {
        if crate::sync_trial::on_trial(&data_dir, vault_id) {
            return Err(needs_password());
        }
        invalidate(&data_dir, vault_id, "the master password was changed");
        return Err(invalid());
    }
    let kek = match keystore.open(&blob.sealed, prompt) {
        Ok(plaintext) => kek_of(vault_id, &plaintext),
        Err(KeystoreError::Invalidated) => Err(invalid()),
        Err(e) => return Err(e.into()),
    };
    let Ok(kek) = kek else {
        invalidate(&data_dir, vault_id, "the keystore no longer opens it");
        return Err(invalid());
    };
    // Like a password unlock that starts now (the prompt may have taken a
    // while; a lock during it found no vault to close) – with the rules for
    // a copy the cloud-folder sync stored (`sync_trial::unlock_copy`).
    match crate::sync_trial::unlock_copy(core, vault_id, |store| {
        store.unlock_with_master_kek(vault_id, &kek)
    }) {
        // The file changed since the salt check (a sync with a new master
        // password): this KEK opens nothing any more – unless that copy is
        // still on trial (see above).
        Err(AppError::Core(CoreError::WrongPassword)) => {
            if crate::sync_trial::on_trial(&data_dir, vault_id) {
                return Err(needs_password());
            }
            invalidate(&data_dir, vault_id, "the master password was changed");
            Err(invalid())
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use keystead_core::settings::Settings;
    use keystead_core::KdfParams;

    use super::*;
    use crate::state::test_support::core_with_open_vault;

    /// A keystore that "seals" by reversing and tagging the text and
    /// records every prompt; `next` makes the next call fail.
    #[derive(Default)]
    struct FakeKeystore {
        unavailable: Option<Unavailable>,
        next: RefCell<Option<KeystoreError>>,
        prompts: RefCell<Vec<String>>,
    }

    impl FakeKeystore {
        fn failing_once(e: KeystoreError) -> Self {
            let keystore = FakeKeystore::default();
            *keystore.next.borrow_mut() = Some(e);
            keystore
        }

        fn take_failure(&self) -> Result<(), KeystoreError> {
            match self.next.borrow_mut().take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    impl Keystore for FakeKeystore {
        fn availability(&self) -> Result<(), Unavailable> {
            self.unavailable.map_or(Ok(()), Err)
        }

        fn seal(&self, plaintext: &str, prompt: &Prompt) -> Result<String, KeystoreError> {
            self.prompts
                .borrow_mut()
                .push(format!("seal:{}", prompt.title));
            self.take_failure()?;
            Ok(format!(
                "sealed:{}",
                plaintext.chars().rev().collect::<String>()
            ))
        }

        fn open(&self, sealed: &str, prompt: &Prompt) -> Result<Zeroizing<String>, KeystoreError> {
            self.prompts
                .borrow_mut()
                .push(format!("open:{}", prompt.title));
            self.take_failure()?;
            let inner = sealed
                .strip_prefix("sealed:")
                .ok_or(KeystoreError::Invalidated)?;
            Ok(Zeroizing::new(inner.chars().rev().collect()))
        }
    }

    fn prompt(title: &str) -> Prompt {
        Prompt {
            title: title.into(),
            reason: "Tresor öffnen".into(),
            cancel: "Master-Passwort".into(),
        }
    }

    /// A core with a vault open (password `master`); returns its id.
    fn setup(dir: &Path) -> (Arc<Core>, String) {
        let core = core_with_open_vault(dir, Settings::default());
        let id = core.read(|v| v.id().to_owned()).unwrap();
        (core, id)
    }

    fn data_dir(core: &Core) -> PathBuf {
        core.data_dir()
    }

    fn enabled(core: &Core) -> bool {
        core.state().settings.device_unlock
    }

    /// The test vault, locked: unlocked again only through the fingerprint.
    fn enable_and_lock(core: &Core, id: &str, keystore: &FakeKeystore) {
        enable(core, keystore, Some(id), &prompt("einrichten")).unwrap();
        core.lock(None);
    }

    #[test]
    fn the_blob_files_round_trip_and_damaged_ones_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert_eq!(load_blob(data, "v1").unwrap(), None);
        assert!(enrolled_vaults(data).is_empty());
        let blob = Blob {
            version: BLOB_VERSION,
            vault_id: "v1".into(),
            kdf_salt: "c2FsdA==".into(),
            sealed: "abc;def".into(),
            created_at: 7,
        };
        store_blob(data, &blob).unwrap();
        assert_eq!(load_blob(data, "v1").unwrap(), Some(blob.clone()));
        assert_eq!(enrolled_vaults(data), ["v1"]);
        // The file holds the sealed form only, camelCase.
        let text = fs::read_to_string(dir.path().join("device-unlock/v1.json")).unwrap();
        assert!(text.contains(r#""kdfSalt": "c2FsdA==""#), "{text}");
        assert!(text.contains(r#""sealed": "abc;def""#), "{text}");

        // Damaged, another version, another vault's, empty fields.
        let path = blob_path(data, "v1").unwrap();
        let tampered = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = serde_json::to_value(&blob).unwrap();
            edit(&mut value);
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            load_blob(data, "v1").map_err(|e| e.code())
        };
        let expected = Err(format!("unsupported:{INVALID}"));
        assert_eq!(tampered(&|v| v["version"] = 2.into()), expected);
        assert_eq!(tampered(&|v| v["vaultId"] = "v2".into()), expected);
        assert_eq!(tampered(&|v| v["sealed"] = "".into()), expected);
        assert_eq!(tampered(&|v| v["kdfSalt"] = "".into()), expected);
        assert_eq!(tampered(&|v| *v = serde_json::json!([1, 2])), expected);
        fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_blob(data, "v1").map_err(|e| e.code()), expected);

        // Only vault ids become file names; other files are ignored.
        assert_eq!(blob_path(data, "../x").unwrap_err().code(), "not_found");
        assert_eq!(blob_path(data, "").unwrap_err().code(), "not_found");
        fs::write(dir.path().join("device-unlock/notes.txt"), b"x").unwrap();
        fs::write(dir.path().join("device-unlock/bad name.json"), b"x").unwrap();
        assert_eq!(enrolled_vaults(data), ["v1"]);

        assert!(remove_blob(data, "v1").unwrap());
        assert!(!remove_blob(data, "v1").unwrap());
        store_blob(data, &blob).unwrap();
        remove_all_blobs(data).unwrap();
        assert!(!dir.path().join("device-unlock").exists());
        remove_all_blobs(data).unwrap();
    }

    #[test]
    fn the_plugins_answers_are_classified() {
        use KeystoreError as K;
        let error = |code: Option<&str>, message: &str, opening: bool| {
            plugin_cipher_error(code, message, opening)
        };
        assert_eq!(error(Some("userCancel"), "Cancel", true), K::Cancelled);
        assert_eq!(error(Some("systemCancel"), "x", false), K::Cancelled);
        assert_eq!(error(Some("biometryLockout"), "x", true), K::Lockout);
        assert_eq!(
            error(Some("authenticationFailed"), "x", true),
            K::NotRecognized
        );
        assert_eq!(
            error(Some("biometryNotEnrolled"), "x", false),
            K::Unavailable(Unavailable::NotEnrolled)
        );
        for code in ["biometryNotAvailable", "passcodeNotSet"] {
            assert_eq!(
                error(Some(code), "x", true),
                K::Unavailable(Unavailable::Other)
            );
        }
        // The plugin's own failures when opening: the setup is useless.
        let init = "Failed to initialize the cipher: Key permanently invalidated";
        let decrypt = "Failed to decrypt the data: AEADBadTagException";
        for code in [None, Some("")] {
            assert_eq!(error(code, init, true), K::Invalidated);
            assert_eq!(error(code, decrypt, true), K::Invalidated);
            // Never when sealing (the plugin replaces an invalidated key),
            // never for an unknown message (e.g. an error code missing from
            // the plugin's table): those keep the setup.
            assert_eq!(error(code, init, false), K::Other(init.into()));
            assert_eq!(
                error(code, "Security update required", true),
                K::Other("Security update required".into())
            );
        }
        assert_eq!(
            error(Some("invalidContext"), "Missing data", true),
            K::Other("Missing data".into())
        );

        assert_eq!(plugin_availability(true, None, None), Ok(()));
        assert_eq!(
            plugin_availability(
                false,
                Some("biometryNotEnrolled"),
                Some("Biometrics not enrolled.")
            ),
            Err(Unavailable::NotEnrolled)
        );
        assert_eq!(
            plugin_availability(
                false,
                Some("biometryNotAvailable"),
                Some("No biometric on this device.")
            ),
            Err(Unavailable::NoHardware)
        );
        assert_eq!(
            plugin_availability(
                false,
                Some("biometryNotAvailable"),
                Some("Biometry unavailable.")
            ),
            Err(Unavailable::Other)
        );
    }

    #[test]
    fn the_sealed_plaintext_binds_the_kek_to_its_vault() {
        let kek: SecretKey = Zeroizing::new([7u8; KEY_LEN]);
        let plaintext = seal_input("vault-a", &kek);
        assert!(plaintext.starts_with("keystead-device-unlock:1:vault-a:"));
        assert_eq!(*kek_of("vault-a", &plaintext).unwrap(), *kek);
        let invalid = Err(format!("unsupported:{INVALID}"));
        let code = |id: &str, text: &str| kek_of(id, text).map(|_| ()).map_err(|e| e.code());
        // Another vault's, another format, a truncated key, garbage.
        assert_eq!(code("vault-b", &plaintext), invalid);
        assert_eq!(code("vault-a", &plaintext.replace(":1:", ":2:")), invalid);
        assert_eq!(code("vault-a", &plaintext[..plaintext.len() - 4]), invalid);
        assert_eq!(
            code("vault-a", "keystead-device-unlock:1:vault-a:!!"),
            invalid
        );
        assert_eq!(code("vault-a", ""), invalid);
    }

    #[test]
    fn enabling_seals_the_open_vaults_kek_and_the_fingerprint_unlocks_it() {
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        let keystore = FakeKeystore::default();
        assert!(!enabled(&core));

        // Only the vault the page shows.
        assert_eq!(
            enable(&core, &keystore, Some("other"), &prompt("x"))
                .unwrap_err()
                .code(),
            "locked"
        );
        assert!(keystore.prompts.borrow().is_empty());

        let status = enable(&core, &keystore, Some(&id), &prompt("einrichten")).unwrap();
        assert!(status.enabled && status.available);
        assert_eq!(status.vaults, std::slice::from_ref(&id));
        assert!(enabled(&core));
        // The blob holds the sealed form, never the KEK.
        let kek = core.read(|v| v.master_kek()).unwrap();
        let raw = fs::read_to_string(blob_path(&data_dir(&core), &id).unwrap()).unwrap();
        assert!(!raw.contains(&crypto::b64_encode(kek.as_slice())), "{raw}");
        let blob = load_blob(&data_dir(&core), &id).unwrap().unwrap();
        assert_eq!(
            blob.kdf_salt,
            core.read(|v| v.kdf_salt().to_owned()).unwrap()
        );
        // The setting is persisted with the store (tests: next to it).
        let saved = fs::read_to_string(data_dir(&core).join("settings.json")).unwrap();
        assert!(saved.contains(r#""deviceUnlock": true"#), "{saved}");

        core.lock(None);
        let info = unlock(&core, &keystore, &id, &prompt("entsperren")).unwrap();
        assert_eq!(info.id, id);
        assert_eq!(core.read(|v| v.master_kek()).unwrap(), kek);
        assert_eq!(
            *keystore.prompts.borrow(),
            ["seal:einrichten", "open:entsperren"]
        );
    }

    #[test]
    fn a_cancelled_or_failed_prompt_keeps_the_setup() {
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        enable_and_lock(&core, &id, &FakeKeystore::default());
        for (error, code) in [
            (
                KeystoreError::Cancelled,
                "unsupported:device_unlock_cancelled",
            ),
            (KeystoreError::Lockout, "unsupported:device_unlock_lockout"),
            (
                KeystoreError::NotRecognized,
                "unsupported:device_unlock_failed",
            ),
            (
                KeystoreError::Unavailable(Unavailable::NotEnrolled),
                "unsupported:device_unlock_not_enrolled",
            ),
            (KeystoreError::Busy, "unsupported:device_unlock_busy"),
            (KeystoreError::Other("x".into()), "io:device_unlock: x"),
        ] {
            let keystore = FakeKeystore::failing_once(error);
            let err = unlock(&core, &keystore, &id, &prompt("p")).unwrap_err();
            assert_eq!(err.code(), code);
            assert!(core.state().vault.is_none());
            assert!(
                load_blob(&data_dir(&core), &id).unwrap().is_some(),
                "{code}"
            );
        }
        // A cancelled setup stores nothing.
        let other_dir = tempfile::tempdir().unwrap();
        let (core2, id2) = setup(other_dir.path());
        let keystore = FakeKeystore::failing_once(KeystoreError::Cancelled);
        let err = enable(&core2, &keystore, Some(&id2), &prompt("p")).unwrap_err();
        assert_eq!(err.code(), "unsupported:device_unlock_cancelled");
        assert!(!enabled(&core2));
        assert!(enrolled_vaults(&data_dir(&core2)).is_empty());
        // No usable fingerprint: no prompt at all.
        let keystore = FakeKeystore {
            unavailable: Some(Unavailable::NoHardware),
            ..FakeKeystore::default()
        };
        let err = enable(&core2, &keystore, Some(&id2), &prompt("p")).unwrap_err();
        assert_eq!(err.code(), "unsupported:device_unlock_unavailable");
        assert!(keystore.prompts.borrow().is_empty());
        let status = super::status(&core2, &keystore);
        assert!(!status.available);
        assert_eq!(status.unavailable_reason, Some(Unavailable::NoHardware));
    }

    #[test]
    fn an_invalidated_key_or_a_tampered_blob_resets_the_setup() {
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        let keystore = FakeKeystore::default();
        let data = data_dir(&core);

        // A new fingerprint enrolled: the keystore key is gone.
        enable_and_lock(&core, &id, &keystore);
        let invalidated = FakeKeystore::failing_once(KeystoreError::Invalidated);
        let err = unlock(&core, &invalidated, &id, &prompt("p")).unwrap_err();
        assert_eq!(err.code(), format!("unsupported:{INVALID}"));
        assert!(load_blob(&data, &id).unwrap().is_none());
        let status = super::status(&core, &keystore);
        assert!(status.invalidated.contains(&id));
        assert!(status.vaults.is_empty());
        // Never set up any more: not an invalid one.
        assert_eq!(
            unlock(&core, &keystore, &id, &prompt("p"))
                .unwrap_err()
                .code(),
            format!("unsupported:{NOT_SET}")
        );

        // Setting it up again clears the mark.
        core.unlock(&id, "master").unwrap();
        let status = enable(&core, &keystore, Some(&id), &prompt("p")).unwrap();
        assert!(!status.invalidated.contains(&id));
        core.lock(None);

        // The sealed data was altered: what the keystore opens is not this
        // vault's KEK (here another vault's id inside).
        let mut blob = load_blob(&data, &id).unwrap().unwrap();
        let foreign = seal_input("another-vault", &Zeroizing::new([1u8; KEY_LEN]));
        blob.sealed = format!("sealed:{}", foreign.chars().rev().collect::<String>());
        store_blob(&data, &blob).unwrap();
        assert_eq!(
            unlock(&core, &keystore, &id, &prompt("p"))
                .unwrap_err()
                .code(),
            format!("unsupported:{INVALID}")
        );
        assert!(load_blob(&data, &id).unwrap().is_none());
        assert!(core.state().vault.is_none());

        // A damaged file.
        core.unlock(&id, "master").unwrap();
        enable(&core, &keystore, Some(&id), &prompt("p")).unwrap();
        core.lock(None);
        fs::write(blob_path(&data, &id).unwrap(), b"{}").unwrap();
        assert_eq!(
            unlock(&core, &keystore, &id, &prompt("p"))
                .unwrap_err()
                .code(),
            format!("unsupported:{INVALID}")
        );
        assert!(!blob_path(&data, &id).unwrap().exists());

        // The right vault id but a key that does not open the vault.
        core.unlock(&id, "master").unwrap();
        enable(&core, &keystore, Some(&id), &prompt("p")).unwrap();
        core.lock(None);
        let mut blob = load_blob(&data, &id).unwrap().unwrap();
        let wrong = seal_input(&id, &Zeroizing::new([9u8; KEY_LEN]));
        blob.sealed = format!("sealed:{}", wrong.chars().rev().collect::<String>());
        store_blob(&data, &blob).unwrap();
        assert_eq!(
            unlock(&core, &keystore, &id, &prompt("p"))
                .unwrap_err()
                .code(),
            format!("unsupported:{INVALID}")
        );
        assert!(load_blob(&data, &id).unwrap().is_none());
        assert!(core.state().vault.is_none());
    }

    #[test]
    fn a_master_password_changed_elsewhere_resets_the_setup_without_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        let keystore = FakeKeystore::default();
        enable_and_lock(&core, &id, &keystore);
        let data = data_dir(&core);

        // The PC changed the master password and the sync brought the file
        // (here: another session on the same store).
        let store = core.store();
        let mut other = store.unlock(&id, "master").unwrap();
        other.change_master_password("master", "neu").unwrap();
        drop(other);

        let prompts_before = keystore.prompts.borrow().len();
        assert_eq!(
            unlock(&core, &keystore, &id, &prompt("p"))
                .unwrap_err()
                .code(),
            format!("unsupported:{INVALID}")
        );
        assert_eq!(keystore.prompts.borrow().len(), prompts_before, "no prompt");
        assert!(load_blob(&data, &id).unwrap().is_none());
        assert!(super::status(&core, &keystore).invalidated.contains(&id));

        // The same found by the status (the unlock screen asks first) and
        // by the sync's check after storing a copy.
        core.unlock(&id, "neu").unwrap();
        enable(&core, &keystore, Some(&id), &prompt("p")).unwrap();
        let salt = core.read(|v| v.kdf_salt().to_owned()).unwrap();
        forget_if_stale(&data, &id, &salt);
        assert!(load_blob(&data, &id).unwrap().is_some(), "same salt: kept");
        core.lock(None);
        let mut other = store.unlock(&id, "neu").unwrap();
        other.change_master_password("neu", "neuer").unwrap();
        drop(other);
        let status = super::status(&core, &keystore);
        assert!(status.vaults.is_empty());
        assert!(load_blob(&data, &id).unwrap().is_none());

        core.unlock(&id, "neuer").unwrap();
        enable(&core, &keystore, Some(&id), &prompt("p")).unwrap();
        forget_if_stale(&data, &id, "another salt");
        assert!(load_blob(&data, &id).unwrap().is_none());
    }

    #[test]
    fn disabling_removes_the_setups_and_off_means_none() {
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        let keystore = FakeKeystore::default();
        enable(&core, &keystore, Some(&id), &prompt("p")).unwrap();
        // A second vault of the same store.
        let second = core
            .store()
            .create_vault_with_params("Zwei", "pw2", KdfParams::insecure_for_tests())
            .unwrap();
        let second_id = second.id().to_owned();
        core.install_vault(second).unwrap();
        let status = enable(&core, &keystore, Some(&second_id), &prompt("p")).unwrap();
        let mut both = vec![id.clone(), second_id.clone()];
        both.sort();
        assert_eq!(status.vaults, both);

        // One vault off: the other stays, the setting too.
        let status = disable(&core, &keystore, Some(&second_id)).unwrap();
        assert_eq!(status.vaults, std::slice::from_ref(&id));
        assert!(status.enabled);
        // The last one off: the setting goes off.
        let status = disable(&core, &keystore, Some(&id)).unwrap();
        assert!(status.vaults.is_empty());
        assert!(!status.enabled && !enabled(&core));

        // Everything off at once.
        enable(&core, &keystore, Some(&second_id), &prompt("p")).unwrap();
        let status = disable(&core, &keystore, None).unwrap();
        assert!(!status.enabled && status.vaults.is_empty());
        assert!(!dir.path().join("data/device-unlock").exists());

        // Off (e.g. a reset settings file): leftovers go, unlock refuses.
        enable(&core, &keystore, Some(&second_id), &prompt("p")).unwrap();
        core.lock(None);
        set_enabled(&core, false);
        assert_eq!(
            unlock(&core, &keystore, &second_id, &prompt("p"))
                .unwrap_err()
                .code(),
            format!("unsupported:{NOT_SET}")
        );
        assert!(load_blob(&data_dir(&core), &second_id).unwrap().is_some());
        let status = super::status(&core, &keystore);
        assert!(status.vaults.is_empty());
        assert!(enrolled_vaults(&data_dir(&core)).is_empty());
    }

    #[test]
    fn a_vault_that_is_gone_loses_its_setup() {
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        let keystore = FakeKeystore::default();
        enable_and_lock(&core, &id, &keystore);
        fs::remove_file(core.store().vault_path(&id)).unwrap();
        assert_eq!(
            unlock(&core, &keystore, &id, &prompt("p"))
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert!(load_blob(&data_dir(&core), &id).unwrap().is_none());
        // Gone, not reset: nothing to explain.
        assert!(!super::status(&core, &keystore).invalidated.contains(&id));

        // The status tidies up the same way.
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        enable_and_lock(&core, &id, &keystore);
        fs::remove_file(core.store().vault_path(&id)).unwrap();
        assert!(super::status(&core, &keystore).vaults.is_empty());
        assert!(load_blob(&data_dir(&core), &id).unwrap().is_none());
    }

    /// Locks the core while its prompt is open (the background lock of a
    /// phone whose activity the prompt paused).
    struct LockingKeystore {
        core: Arc<Core>,
        inner: FakeKeystore,
    }

    impl Keystore for LockingKeystore {
        fn availability(&self) -> Result<(), Unavailable> {
            self.inner.availability()
        }

        fn seal(&self, plaintext: &str, prompt: &Prompt) -> Result<String, KeystoreError> {
            self.inner.seal(plaintext, prompt)
        }

        fn open(&self, sealed: &str, prompt: &Prompt) -> Result<Zeroizing<String>, KeystoreError> {
            self.core.lock(None);
            self.inner.open(sealed, prompt)
        }
    }

    #[test]
    fn a_lock_during_the_prompt_does_not_stop_the_unlock() {
        // The unlock takes its lock-epoch ticket after the prompt, like a
        // password unlock that starts then: a lock while the prompt was
        // open found no vault to close and does not cancel it.
        let dir = tempfile::tempdir().unwrap();
        let (core, id) = setup(dir.path());
        enable_and_lock(&core, &id, &FakeKeystore::default());
        let keystore = LockingKeystore {
            core: Arc::clone(&core),
            inner: FakeKeystore::default(),
        };
        let epoch = core.lock_epoch();
        assert_eq!(unlock(&core, &keystore, &id, &prompt("p")).unwrap().id, id);
        assert!(core.lock_epoch() > epoch);
        assert!(core.state().vault.is_some());
    }
}
