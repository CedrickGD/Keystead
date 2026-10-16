//! Data locations (see "Data locations" in docs/ARCHITECTURE.md).
//!
//! Resolution order of [`data_dir`]:
//! 1. the process-wide override set with [`set_data_dir_override`] (the
//!    mobile app's private folder, set once at start-up),
//! 2. `$KEYSTEAD_DATA_DIR` if set (and non-empty),
//! 3. portable mode: a folder `Keystead-Data` next to the running executable,
//!    if it exists,
//! 4. the OS local data directory (`%LOCALAPPDATA%\Keystead`,
//!    `~/.local/share/keystead`, `~/Library/Application Support/Keystead`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Environment variable that overrides the data directory.
pub const DATA_DIR_ENV: &str = "KEYSTEAD_DATA_DIR";
/// Environment variable that overrides the legacy (VaultX 1.x) directory.
pub const LEGACY_DIR_ENV: &str = "KEYSTEAD_LEGACY_DIR";
/// Name of the portable data folder next to the executable.
pub const PORTABLE_DIR_NAME: &str = "Keystead-Data";

/// The data directory set by [`set_data_dir_override`].
static DATA_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Sets the data directory of this process, taking precedence over
/// `$KEYSTEAD_DATA_DIR`, the portable folder and the OS default. Meant for
/// the mobile app, which has no environment and no usable OS default and
/// calls this with its private app folder before anything reads a path.
/// Only the first call counts: a second call (and an empty path) returns
/// false and changes nothing.
pub fn set_data_dir_override(dir: PathBuf) -> bool {
    if dir.as_os_str().is_empty() {
        return false;
    }
    DATA_DIR_OVERRIDE.set(dir).is_ok()
}

/// The override set by [`set_data_dir_override`], if any.
pub fn data_dir_override() -> Option<&'static Path> {
    DATA_DIR_OVERRIDE.get().map(PathBuf::as_path)
}

/// The data directory currently in use.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = data_dir_override() {
        return dir.to_path_buf();
    }
    if let Some(dir) = env_dir(DATA_DIR_ENV) {
        return dir;
    }
    if let Some(dir) = portable_dir().filter(|d| d.is_dir()) {
        return dir;
    }
    default_data_dir()
}

/// `<data_dir>/vaults`.
pub fn vaults_dir() -> PathBuf {
    data_dir().join("vaults")
}

/// `<data_dir>/settings.json`.
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// True if the portable folder next to the executable is in use (and the
/// data directory is overridden neither by [`set_data_dir_override`] nor
/// by `$KEYSTEAD_DATA_DIR`).
pub fn is_portable() -> bool {
    data_dir_override().is_none()
        && env_dir(DATA_DIR_ENV).is_none()
        && portable_dir().is_some_and(|d| d.is_dir())
}

/// Location of the portable data folder (`Keystead-Data` next to the current
/// executable), whether or not it exists. `None` if the executable path
/// cannot be determined.
pub fn portable_dir() -> Option<PathBuf> {
    // `current_exe` already resolves symlinks on Linux; it is deliberately
    // not canonicalised (that would yield `\\?\` paths on Windows).
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(PORTABLE_DIR_NAME))
}

/// The non-portable OS data directory (ignores portable mode and
/// `$KEYSTEAD_DATA_DIR`). Falls back to the home directory or the temp dir if
/// the OS reports no local data directory.
pub fn default_data_dir() -> PathBuf {
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(any(windows, target_os = "macos")) {
        base.join("Keystead")
    } else {
        base.join("keystead")
    }
}

/// Directory of the legacy VaultX 1.x (PowerShell) vaults:
/// `$KEYSTEAD_LEGACY_DIR` if set, otherwise `%LOCALAPPDATA%\VaultX` on
/// Windows and `None` elsewhere.
pub fn legacy_dir() -> Option<PathBuf> {
    if let Some(dir) = env_dir(LEGACY_DIR_ENV) {
        return Some(dir);
    }
    if cfg!(windows) {
        env_dir("LOCALAPPDATA")
            .or_else(dirs::data_local_dir)
            .or_else(|| env_dir("TEMP"))
            .map(|d| d.join("VaultX"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dir_layout() {
        let d = default_data_dir();
        if cfg!(any(windows, target_os = "macos")) {
            assert!(d.ends_with("Keystead"));
        } else {
            assert!(d.ends_with("keystead"));
        }
    }

    #[test]
    fn portable_dir_is_next_to_exe() {
        let p = portable_dir().unwrap();
        assert_eq!(p.file_name().unwrap(), PORTABLE_DIR_NAME);
        let exe = std::env::current_exe().unwrap();
        assert_eq!(p.parent(), exe.parent());
    }

    // Environment-variable dependent behaviour and the process-wide
    // override are tested in tests/paths_env.rs (separate process, so no
    // races with other tests).
}
