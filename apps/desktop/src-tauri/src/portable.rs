//! Portable mode: moving the data directory between the OS location and the
//! `VaultX-Data` folder next to the executable (see "Data locations" in
//! docs/ARCHITECTURE.md). `paths::data_dir()` picks the portable folder
//! whenever it exists, so its creation/removal is the switch.
//!
//! The move is copy-then-commit, so an error never leaves the active data
//! directory incomplete:
//! * enable: copy everything into `VaultX-Data.partial`, then rename it to
//!   `VaultX-Data` (commit), then delete the old copies;
//! * disable: copy everything into the OS directory (staged under
//!   temporary names, then renamed), then rename `VaultX-Data` away
//!   (commit) and delete it.
//!
//! Lock files (`*.lock`) and temporary files (`*.tmp`) are not moved. The
//! caller must make sure no vault is open and the bridge is stopped.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use vaultx_core::paths;

use crate::error::{AppError, AppResult};
use crate::state::log;

const STAGING_SUFFIX: &str = ".partial";
const RETIRED_SUFFIX: &str = ".old";
const MOVE_SUFFIX: &str = ".vxmove";

fn io_err(path: &Path, e: io::Error) -> AppError {
    AppError::io(format!("{}: {e}", path.display()))
}

/// Why portable mode cannot be changed, if it cannot.
pub fn check_available() -> AppResult<PathBuf> {
    if std::env::var_os(paths::DATA_DIR_ENV).is_some_and(|v| !v.is_empty()) {
        return Err(AppError::unsupported("data_dir_override"));
    }
    paths::portable_dir().ok_or_else(|| AppError::unsupported("portable_unavailable"))
}

/// Switches portable mode on or off and moves the data. Returns the new
/// data directory. No-op if the mode is already as requested.
pub fn set_portable(enabled: bool) -> AppResult<PathBuf> {
    let portable = check_available()?;
    if paths::is_portable() == enabled {
        return Ok(paths::data_dir());
    }
    let default = paths::default_data_dir();
    if enabled {
        enable(&default, &portable)?;
    } else {
        disable(&portable, &default)?;
    }
    Ok(paths::data_dir())
}

fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    dir.with_file_name(name)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// All files below `root` (relative paths), without lock/temp files.
fn collect_files(root: &Path) -> AppResult<Vec<PathBuf>> {
    fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> AppResult<()> {
        let dir = root.join(rel);
        let entries = fs::read_dir(&dir).map_err(|e| io_err(&dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_err(&dir, e))?;
            let file_type = entry.file_type().map_err(|e| io_err(&entry.path(), e))?;
            let rel_path = rel.join(entry.file_name());
            if file_type.is_dir() {
                walk(root, &rel_path, out)?;
            } else if file_type.is_file() {
                let skip = rel_path
                    .extension()
                    .is_some_and(|ext| ext == "lock" || ext == "tmp");
                if !skip {
                    out.push(rel_path);
                }
            } else {
                log(format_args!(
                    "not moving {} (not a regular file)",
                    root.join(&rel_path).display()
                ));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, Path::new(""), &mut out)?;
    }
    Ok(out)
}

/// Copies a file (creating parent directories) and flushes it to disk.
fn copy_file(from: &Path, to: &Path) -> AppResult<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    fs::copy(from, to).map_err(|e| io_err(from, e))?;
    OpenOptions::new()
        .write(true)
        .open(to)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_err(to, e))
}

fn is_vault_file(rel: &Path) -> bool {
    rel.extension().is_some_and(|ext| ext == "vaultx")
}

fn enable(default: &Path, portable: &Path) -> AppResult<()> {
    let files = collect_files(default)?;
    let staging = sibling(portable, STAGING_SUFFIX);
    if staging.exists() {
        // Leftover of an interrupted earlier attempt.
        fs::remove_dir_all(&staging).map_err(|e| io_err(&staging, e))?;
    }
    let result = (|| {
        fs::create_dir_all(&staging).map_err(|e| io_err(&staging, e))?;
        for rel in &files {
            copy_file(&default.join(rel), &staging.join(rel))?;
        }
        // Commit: from now on `paths::data_dir()` is the portable folder.
        fs::rename(&staging, portable).map_err(|e| io_err(portable, e))
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    // The data now lives next to the exe; do not leave copies behind.
    remove_moved(default, &files);
    Ok(())
}

fn disable(portable: &Path, default: &Path) -> AppResult<()> {
    let files = collect_files(portable)?;
    if let Some(rel) = files
        .iter()
        .find(|rel| is_vault_file(rel) && default.join(rel).exists())
    {
        log(format_args!(
            "{} already exists; not overwriting it",
            default.join(rel).display()
        ));
        return Err(AppError::invalid("target_exists"));
    }

    // 1. Stage all copies under temporary names.
    let mut staged: Vec<PathBuf> = Vec::new();
    for rel in &files {
        let tmp = with_suffix(&default.join(rel), MOVE_SUFFIX);
        if let Err(e) = copy_file(&portable.join(rel), &tmp) {
            let _ = fs::remove_file(&tmp);
            remove_all(&staged);
            return Err(e);
        }
        staged.push(tmp);
    }

    // 2. Give them their final names (new files are remembered for rollback).
    let mut created: Vec<PathBuf> = Vec::new();
    for (i, rel) in files.iter().enumerate() {
        let target = default.join(rel);
        let existed = target.exists();
        if let Err(e) = fs::rename(&staged[i], &target) {
            remove_all(&staged[i..]);
            remove_all(&created);
            return Err(io_err(&target, e));
        }
        if !existed {
            created.push(target);
        }
    }

    // 3. Commit: without the portable folder `paths::data_dir()` is the OS
    //    location again.
    let retired = sibling(portable, RETIRED_SUFFIX);
    if retired.exists() {
        let _ = fs::remove_dir_all(&retired);
    }
    if let Err(e) = fs::rename(portable, &retired) {
        remove_all(&created);
        return Err(io_err(portable, e));
    }
    if let Err(e) = fs::remove_dir_all(&retired) {
        log(format_args!(
            "could not delete {} after moving the data: {e}",
            retired.display()
        ));
    }
    Ok(())
}

fn remove_all(paths: &[PathBuf]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}

/// Deletes the moved files below `root`, leftover lock files and the
/// directories that became empty (best effort, logged).
fn remove_moved(root: &Path, files: &[PathBuf]) {
    for rel in files {
        let path = root.join(rel);
        if let Err(e) = fs::remove_file(&path) {
            log(format_args!("could not delete {}: {e}", path.display()));
        }
    }
    // Leftover lock files and now-empty directories, deepest first.
    let mut dirs: Vec<PathBuf> = files
        .iter()
        .filter_map(|rel| rel.parent().map(|p| root.join(p)))
        .collect();
    dirs.push(root.join("vaults"));
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for dir in &dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "lock" || e == "tmp") {
                    let _ = fs::remove_file(path);
                }
            }
        }
        let _ = fs::remove_dir(dir);
    }
    let _ = fs::remove_dir(root);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn enable_and_disable_move_everything() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path();
        let default = tmp.join("os").join("vaultx");
        let portable = tmp.join("usb").join("VaultX-Data");
        fs::create_dir_all(portable.parent().unwrap()).unwrap();
        write(&default.join("vaults/a.vaultx"), "A");
        write(&default.join("vaults/a.vaultx.bak"), "A0");
        write(&default.join("vaults/a.vaultx.lock"), "");
        write(&default.join("settings.json"), "{}");
        write(&default.join("native-host/com.vaultx.bridge.json"), "{}");

        enable(&default, &portable).unwrap();
        assert_eq!(
            fs::read_to_string(portable.join("vaults/a.vaultx")).unwrap(),
            "A"
        );
        assert_eq!(
            fs::read_to_string(portable.join("vaults/a.vaultx.bak")).unwrap(),
            "A0"
        );
        assert!(portable.join("settings.json").is_file());
        assert!(portable
            .join("native-host/com.vaultx.bridge.json")
            .is_file());
        assert!(!portable.join("vaults/a.vaultx.lock").exists());
        assert!(!default.exists(), "old data must be removed");
        assert!(!sibling(&portable, STAGING_SUFFIX).exists());

        // An unrelated vault already in the OS location is kept.
        write(&default.join("vaults/b.vaultx"), "B");
        write(&default.join("settings.json"), "old");
        disable(&portable, &default).unwrap();
        assert!(!portable.exists());
        assert_eq!(
            fs::read_to_string(default.join("vaults/a.vaultx")).unwrap(),
            "A"
        );
        assert_eq!(
            fs::read_to_string(default.join("vaults/b.vaultx")).unwrap(),
            "B"
        );
        assert_eq!(
            fs::read_to_string(default.join("settings.json")).unwrap(),
            "{}"
        );
        assert!(collect_files(&default)
            .unwrap()
            .iter()
            .all(|p| !p.to_string_lossy().ends_with(MOVE_SUFFIX)));

        // A vault with the same id on both sides is never overwritten.
        enable(&default, &portable).unwrap();
        write(&default.join("vaults/a.vaultx"), "other");
        let err = disable(&portable, &default).unwrap_err();
        assert_eq!(err.code(), "invalid_input:target_exists");
        assert!(portable.join("vaults/a.vaultx").is_file());
        assert_eq!(
            fs::read_to_string(default.join("vaults/a.vaultx")).unwrap(),
            "other"
        );
        assert!(!default.join("settings.json.vxmove").exists());
    }
}
