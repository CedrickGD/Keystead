//! The sync server's password (stage 2c), kept apart from everything else
//! – never in `settings.json` or `link.json`, never logged, never handed
//! back to the webview (see "Server credentials" in docs/ARCHITECTURE.md).
//!
//! * One password per server folder, under the folder's key
//!   (`webdav:<normalized folder URL>`, `remote::Remote::key`): the file
//!   `<data>/sync/credentials/<key hash>.bin` (the first 32 hex digits of the
//!   key's SHA-256 – the file name tells nothing about the server).
//! * What is in the file is sealed by the platform ([`Sealer`]):
//!   - **Windows**: DPAPI (`CryptProtectData`, the user's login secret, no
//!     prompt) with the key as additional entropy – another Windows user,
//!     or the data folder copied to another PC (portable mode), cannot open
//!     it; the app then asks for the password again.
//!   - **Android**: the Kotlin plugin `KeysteadSecretPlugin` (AES-256-GCM, a
//!     key in the Android Keystore that never leaves it, usable only while
//!     the phone is unlocked; the key hash as associated data) –
//!     `mobile::secret`.
//!   - **Linux, macOS**: nothing yet – the password is kept in memory only
//!     ([`Credentials::persistent`] is false) and asked for again after a
//!     restart (`unsupported:sync_credentials_missing`).
//! * Passwords are [`Zeroizing`] in memory. A loaded password is kept in
//!   memory while the app runs (the sync may need it while the phone is
//!   locked – the Keystore key is not usable then; the server client holds
//!   it anyway).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use keystead_core::format;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::state::log;

/// `unsupported:sync_credentials_missing` – no password is stored for the
/// server (Linux/macOS after a restart, a data folder from another PC or
/// user, a lost Keystore key): the user enters it again.
pub const MISSING: &str = "sync_credentials_missing";

/// The file format's first byte (the sealed bytes follow).
const VERSION: u8 = 1;

/// Seals and opens a credential with a key the platform keeps.
pub trait Sealer: Send + Sync {
    /// `context`: the key hash – bound to the sealed bytes (DPAPI entropy,
    /// GCM associated data), so a file cannot be moved to another server.
    fn seal(&self, context: &str, plain: &[u8]) -> AppResult<Vec<u8>>;
    fn open(&self, context: &str, sealed: &[u8]) -> AppResult<Zeroizing<Vec<u8>>>;
}

/// The credentials of the sync servers (see the module docs).
pub struct Credentials {
    /// `None`: kept in memory only.
    sealer: Mutex<Option<Arc<dyn Sealer>>>,
    /// Stored or loaded passwords by key hash.
    memory: Mutex<BTreeMap<String, Zeroizing<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The first 32 hex digits of the key's SHA-256.
pub fn key_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// `<data>/sync/credentials`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sync").join("credentials")
}

fn path(data_dir: &Path, hash: &str) -> PathBuf {
    dir(data_dir).join(format!("{hash}.bin"))
}

impl Credentials {
    pub fn new(sealer: Option<Arc<dyn Sealer>>) -> Self {
        Credentials {
            sealer: Mutex::new(sealer),
            memory: Mutex::new(BTreeMap::new()),
        }
    }

    /// This platform's default: DPAPI on Windows, memory only elsewhere
    /// (Android installs the Keystore plugin once it is loaded,
    /// [`Credentials::set_sealer`]).
    pub fn platform() -> Self {
        #[cfg(windows)]
        let sealer: Option<Arc<dyn Sealer>> = Some(Arc::new(dpapi::Dpapi));
        #[cfg(not(windows))]
        let sealer: Option<Arc<dyn Sealer>> = None;
        Self::new(sealer)
    }

    /// Android: the Keystore plugin (`gui::setup`, after the plugins
    /// loaded).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn set_sealer(&self, sealer: Arc<dyn Sealer>) {
        *lock(&self.sealer) = Some(sealer);
    }

    /// Whether a stored password survives a restart (the PC's status).
    #[cfg_attr(mobile, allow(dead_code))]
    pub fn persistent(&self) -> bool {
        lock(&self.sealer).is_some()
    }

    fn sealer(&self) -> Option<Arc<dyn Sealer>> {
        lock(&self.sealer).clone()
    }

    /// Stores `password` for the server folder `key` (replacing one stored
    /// before).
    pub fn store(&self, data_dir: &Path, key: &str, password: &Zeroizing<String>) -> AppResult<()> {
        let hash = key_hash(key);
        if let Some(sealer) = self.sealer() {
            let mut bytes = vec![VERSION];
            bytes.extend(sealer.seal(&hash, password.as_bytes())?);
            fs::create_dir_all(dir(data_dir))?;
            format::write_atomic(&path(data_dir, &hash), &bytes, false)?;
        }
        lock(&self.memory).insert(hash, password.clone());
        Ok(())
    }

    /// The password stored for `key`, `None` without one. A file that cannot
    /// be opened (another user or PC, a lost Keystore key) counts as none
    /// (logged, by code only).
    pub fn load(&self, data_dir: &Path, key: &str) -> AppResult<Option<Zeroizing<String>>> {
        let hash = key_hash(key);
        if let Some(password) = lock(&self.memory).get(&hash) {
            return Ok(Some(password.clone()));
        }
        let Some(sealer) = self.sealer() else {
            return Ok(None);
        };
        let bytes = match fs::read(path(data_dir, &hash)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let Some((&VERSION, sealed)) = bytes.split_first() else {
            log("a stored server password has an unknown format");
            return Ok(None);
        };
        let plain = match sealer.open(&hash, sealed) {
            Ok(plain) => plain,
            Err(e) => {
                log(format_args!(
                    "a stored server password cannot be opened: {}",
                    e.code()
                ));
                return Ok(None);
            }
        };
        let Ok(text) = std::str::from_utf8(&plain) else {
            return Ok(None);
        };
        let password = Zeroizing::new(text.to_owned());
        lock(&self.memory).insert(hash, password.clone());
        Ok(Some(password))
    }

    /// [`Credentials::load`], `unsupported:sync_credentials_missing` without
    /// one.
    pub fn require(&self, data_dir: &Path, key: &str) -> AppResult<Zeroizing<String>> {
        self.load(data_dir, key)?
            .ok_or_else(|| AppError::unsupported(MISSING))
    }

    /// Forgets the password of `key` (file and memory). None stored is
    /// fine.
    pub fn remove(&self, data_dir: &Path, key: &str) -> AppResult<()> {
        let hash = key_hash(key);
        lock(&self.memory).remove(&hash);
        match fs::remove_file(path(data_dir, &hash)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Windows: DPAPI with the user's login secret.
#[cfg(windows)]
mod dpapi {
    use std::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use zeroize::{Zeroize as _, Zeroizing};

    use super::Sealer;
    use crate::error::{AppError, AppResult};

    pub struct Dpapi;

    /// What the sealed bytes are bound to besides the user.
    fn entropy(context: &str) -> Vec<u8> {
        format!("keystead-sync-credential\0{context}").into_bytes()
    }

    fn blob(bytes: &[u8]) -> AppResult<CRYPT_INTEGER_BLOB> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len()).map_err(|_| AppError::invalid("credential"))?,
            pbData: bytes.as_ptr().cast_mut(),
        })
    }

    /// Copies the output DPAPI allocated and frees it (wiped first).
    ///
    /// # Safety
    /// `out` must come from a successful `CryptProtectData` /
    /// `CryptUnprotectData` call.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData` (caller's
        // contract); they are wiped and released with `LocalFree` as its
        // documentation says.
        unsafe {
            let data = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
            let bytes = data.to_vec();
            data.zeroize();
            LocalFree(out.pbData.cast());
            bytes
        }
    }

    impl Sealer for Dpapi {
        fn seal(&self, context: &str, plain: &[u8]) -> AppResult<Vec<u8>> {
            let entropy = entropy(context);
            let input = blob(plain)?;
            let extra = blob(&entropy)?;
            let mut out = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: ptr::null_mut(),
            };
            // SAFETY: the input blobs point to live slices of their length;
            // `out` receives a buffer DPAPI allocates (`take` frees it).
            let ok = unsafe {
                CryptProtectData(
                    &input,
                    ptr::null(),
                    &extra,
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            };
            if ok == 0 {
                return Err(AppError::io(format!(
                    "CryptProtectData: {}",
                    std::io::Error::last_os_error()
                )));
            }
            // SAFETY: the call succeeded.
            Ok(unsafe { take(out) })
        }

        fn open(&self, context: &str, sealed: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
            let entropy = entropy(context);
            let input = blob(sealed)?;
            let extra = blob(&entropy)?;
            let mut out = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: ptr::null_mut(),
            };
            // SAFETY: as in `seal`.
            let ok = unsafe {
                CryptUnprotectData(
                    &input,
                    ptr::null_mut(),
                    &extra,
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            };
            if ok == 0 {
                return Err(AppError::io(format!(
                    "CryptUnprotectData: {}",
                    std::io::Error::last_os_error()
                )));
            }
            // SAFETY: the call succeeded.
            Ok(Zeroizing::new(unsafe { take(out) }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "webdav:https://cloud.example/remote.php/dav/files/max/Keystead/";
    const OTHER: &str = "webdav:https://dav.example/Keystead/";

    /// A sealer that only reverses the bytes (and checks the context).
    struct Mirror;

    impl Sealer for Mirror {
        fn seal(&self, context: &str, plain: &[u8]) -> AppResult<Vec<u8>> {
            let mut bytes = context.as_bytes().to_vec();
            bytes.push(0);
            bytes.extend(plain.iter().rev());
            Ok(bytes)
        }
        fn open(&self, context: &str, sealed: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
            let prefix = format!("{context}\0");
            let rest = sealed
                .strip_prefix(prefix.as_bytes())
                .ok_or_else(|| AppError::invalid("context"))?;
            Ok(Zeroizing::new(rest.iter().rev().copied().collect()))
        }
    }

    fn pw(text: &str) -> Zeroizing<String> {
        Zeroizing::new(text.to_owned())
    }

    fn round_trip(sealer: Option<Arc<dyn Sealer>>, persistent: bool) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let credentials = Credentials::new(sealer.clone());
        assert_eq!(credentials.persistent(), persistent);
        assert_eq!(credentials.load(data, KEY).unwrap(), None);
        assert_eq!(
            credentials.require(data, KEY).unwrap_err().code(),
            "unsupported:sync_credentials_missing"
        );
        credentials.store(data, KEY, &pw("app-passwort-1")).unwrap();
        credentials.store(data, OTHER, &pw("anderes")).unwrap();
        assert_eq!(
            credentials.load(data, KEY).unwrap().unwrap().as_str(),
            "app-passwort-1"
        );
        // A restart: only a sealed file survives.
        let restarted = Credentials::new(sealer);
        let file = path(data, &key_hash(KEY));
        if persistent {
            assert_eq!(
                restarted.require(data, KEY).unwrap().as_str(),
                "app-passwort-1"
            );
            let bytes = fs::read(&file).unwrap();
            assert!(!bytes
                .windows("app-passwort-1".len())
                .any(|w| w == b"app-passwort-1"));
            // The file name tells nothing about the server.
            let name = file.file_name().unwrap().to_str().unwrap().to_owned();
            assert_eq!(name.len(), 32 + ".bin".len());
            assert!(!name.contains("cloud"));
        } else {
            assert_eq!(restarted.load(data, KEY).unwrap(), None);
            assert!(!dir_has_files(data));
        }
        // Replaced, then removed (twice is fine); the other stays.
        credentials.store(data, KEY, &pw("neu")).unwrap();
        assert_eq!(credentials.require(data, KEY).unwrap().as_str(), "neu");
        credentials.remove(data, KEY).unwrap();
        credentials.remove(data, KEY).unwrap();
        assert_eq!(credentials.load(data, KEY).unwrap(), None);
        assert!(!file.exists());
        assert_eq!(
            credentials.require(data, OTHER).unwrap().as_str(),
            "anderes"
        );
    }

    fn dir_has_files(data: &Path) -> bool {
        fs::read_dir(dir(data)).is_ok_and(|mut entries| entries.next().is_some())
    }

    #[test]
    fn memory_only_credentials_last_until_the_restart() {
        round_trip(None, false);
    }

    #[test]
    fn sealed_credentials_survive_a_restart() {
        round_trip(Some(Arc::new(Mirror)), true);
    }

    #[test]
    fn a_file_moved_to_another_key_or_damaged_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let sealer: Arc<dyn Sealer> = Arc::new(Mirror);
        Credentials::new(Some(Arc::clone(&sealer)))
            .store(data, KEY, &pw("geheim"))
            .unwrap();
        fs::copy(path(data, &key_hash(KEY)), path(data, &key_hash(OTHER))).unwrap();
        let fresh = Credentials::new(Some(Arc::clone(&sealer)));
        assert_eq!(fresh.load(data, OTHER).unwrap(), None);
        fs::write(path(data, &key_hash(KEY)), [9, 1, 2]).unwrap();
        assert_eq!(fresh.load(data, KEY).unwrap(), None);
        fs::write(path(data, &key_hash(KEY)), []).unwrap();
        assert_eq!(fresh.load(data, KEY).unwrap(), None);
    }

    #[test]
    fn key_hashes_are_stable_and_short() {
        assert_eq!(key_hash(KEY), key_hash(KEY));
        assert_ne!(key_hash(KEY), key_hash(OTHER));
        assert_eq!(key_hash("").len(), 32);
        assert!(key_hash(KEY).bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// The real DPAPI of this machine's user.
    #[cfg(windows)]
    #[test]
    fn dpapi_seals_for_this_user_and_key_only() {
        let dpapi = dpapi::Dpapi;
        let hash = key_hash(KEY);
        let sealed = dpapi.seal(&hash, b"app-passwort").unwrap();
        assert!(!sealed.windows(12).any(|w| w == b"app-passwort"));
        assert_eq!(
            dpapi.open(&hash, &sealed).unwrap().as_slice(),
            b"app-passwort"
        );
        // Another key's entropy does not open it; nor do damaged bytes.
        assert!(dpapi.open(&key_hash(OTHER), &sealed).is_err());
        let mut damaged = sealed.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert!(dpapi.open(&hash, &damaged).is_err());
        assert!(Credentials::platform().persistent());
        round_trip(Some(Arc::new(dpapi::Dpapi)), true);
    }
}
