//! Small internal helpers shared by several modules.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// Locks a mutex, recovering the data if a previous holder panicked. All
/// state guarded in this crate stays consistent across a panic (every
/// mutation is a single assignment or map operation), so continuing is safe
/// and avoids cascading panics in the app.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Writes `bytes` to `path` atomically: write a temporary file in the same
/// directory, fsync it, then rename it over the target. Creates missing
/// parent directories. With `private`, the file is created with mode 0600 on
/// Unix (Windows: the per-user data directory already restricts access).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = tmp_path(path);
    let result = (|| {
        let mut file = create_file(&tmp, private)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn create_file(path: &Path, private: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    options.open(path)
}

/// Writes one diagnostic line to stderr. Callers must never pass secrets.
/// (In native-host mode stderr is shown in the browser's log; stdout is
/// reserved for protocol frames.) Unlike `eprintln!`, a closed or broken
/// stderr is ignored instead of panicking.
pub(crate) fn log(message: impl std::fmt::Display) {
    let _ = writeln!(io::stderr().lock(), "[vaultx-bridge] {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("file.json");
        write_atomic(&path, b"one", true).unwrap();
        write_atomic(&path, b"two", true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!tmp_path(&path).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
