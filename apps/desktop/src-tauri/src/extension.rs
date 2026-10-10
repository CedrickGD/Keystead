//! The browser extension the app delivers (see "Browser extension
//! deployment" in docs/ARCHITECTURE.md).
//!
//! `extension/chrome` is embedded at build time (`build.rs`) and written to
//! `<data_dir>/browser-extension` on every start when that folder is missing
//! or differs from the embedded copy (another version, a file changed or
//! deleted). The user loads the extension unpacked from there once; after an
//! app update the extension notices the newer `extensionVersion` in the
//! bridge `status` and reloads itself from the same folder.
//!
//! Writing is atomic per folder: the new copy goes to a temporary sibling
//! (`.browser-extension.<id>.tmp`), the old folder is renamed away
//! (`.browser-extension.<id>.old`) and the new one renamed into place. On
//! Windows a browser may hold a file open for a moment, so renames are
//! retried; if the old folder cannot be moved, its files are overwritten in
//! place. Leftovers of an interrupted run (only those two name patterns) are
//! removed on the next run. Nothing outside these folders is touched.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::state::log;

/// The embedded extension: `(relative path with '/', content)`, sorted.
pub static FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/extension_files.rs"));

/// Name of the folder below the data directory.
pub const DIR_NAME: &str = "browser-extension";
/// Prefix of the temporary and retired folders next to it.
const WORK_PREFIX: &str = ".browser-extension.";
const TEMP_SUFFIX: &str = ".tmp";
const RETIRED_SUFFIX: &str = ".old";
/// Rename attempts (Windows: a browser or virus scanner may hold a file).
const RENAME_ATTEMPTS: u32 = 6;
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// `<data_dir>/browser-extension`.
pub fn extension_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR_NAME)
}

/// Whether a top-level entry of the data directory belongs to the
/// extension deployment (not moved by the portable-mode switch; the app
/// writes it again at the new location).
pub fn is_managed_entry(name: &str) -> bool {
    name == DIR_NAME || name.starts_with(WORK_PREFIX)
}

/// The `version` of an extension `manifest.json`.
pub fn manifest_version(manifest: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    value.get("version")?.as_str().map(str::to_owned)
}

/// The version of the embedded extension (`extensionVersion`).
pub fn bundled_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        FILES
            .iter()
            .find(|(path, _)| *path == "manifest.json")
            .and_then(|(_, bytes)| manifest_version(bytes))
            .unwrap_or_default()
    })
}

/// What [`deploy`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deployed {
    /// The folder did not exist and was written.
    Created,
    /// The folder already holds exactly the embedded files.
    Unchanged,
    /// Another version (or changed files) was replaced.
    Replaced,
    /// The old folder could not be moved; its files were overwritten in place.
    Overwritten,
}

/// The extension folder the last [`deploy_logged`] left holding exactly the
/// embedded files (`None` before the first success, after a failure).
static DEPLOYED_TO: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Writes the embedded extension to `<data_dir>/browser-extension` unless it
/// is already there. Serialised (startup thread vs. `open_extension_dir`);
/// errors are logged. Returns the folder.
pub fn deploy_logged(data_dir: &Path) -> PathBuf {
    static RUNNING: Mutex<()> = Mutex::new(());
    let _guard = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
    let target = extension_dir(data_dir);
    let result = deploy(data_dir, FILES);
    match &result {
        Ok(Deployed::Unchanged) => {}
        Ok(done) => log(format_args!(
            "browser extension {} written to {} ({done:?})",
            bundled_version(),
            target.display()
        )),
        Err(e) => log(format_args!(
            "could not write the browser extension to {}: {e}",
            target.display()
        )),
    }
    record_deploy(&target, result.is_ok());
    target
}

fn record_deploy(target: &Path, ok: bool) {
    let mut deployed = DEPLOYED_TO.lock().unwrap_or_else(|p| p.into_inner());
    if ok {
        *deployed = Some(target.to_path_buf());
    } else if deployed.as_deref() == Some(target) {
        *deployed = None;
    }
}

/// The embedded version once `<data_dir>/browser-extension` holds it – i.e.
/// after [`deploy_logged`] succeeded for this folder in this process. Until
/// then (the deploy thread is still writing, it failed, or the data
/// directory moved) `None`: the bridge `status` must not announce a version
/// the folder does not hold yet, or the extension would reload itself from
/// an old, half-written or missing folder.
pub fn deployed_version(data_dir: &Path) -> Option<&'static str> {
    let deployed = DEPLOYED_TO.lock().unwrap_or_else(|p| p.into_inner());
    (deployed.as_deref() == Some(extension_dir(data_dir).as_path())).then(bundled_version)
}

/// Writes `files` to `<data_dir>/browser-extension` (see the module docs).
pub fn deploy(data_dir: &Path, files: &[(&str, &[u8])]) -> io::Result<Deployed> {
    for (rel, _) in files {
        check_relative(rel)?;
    }
    fs::create_dir_all(data_dir)?;
    remove_leftovers(data_dir);

    let target = extension_dir(data_dir);
    let exists = match fs::symlink_metadata(&target) {
        Ok(_) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    if exists && is_current(&target, files) {
        return Ok(Deployed::Unchanged);
    }

    let temp = create_work_dir(data_dir, TEMP_SUFFIX)?;
    if let Err(e) = write_files(&temp, files) {
        remove_path(&temp);
        return Err(e);
    }

    if !exists {
        return match rename_retrying(&temp, &target) {
            Ok(()) => Ok(Deployed::Created),
            Err(e) => {
                remove_path(&temp);
                Err(e)
            }
        };
    }

    let retired = work_path(data_dir, RETIRED_SUFFIX);
    match rename_retrying(&target, &retired) {
        Ok(()) => {
            if let Err(e) = rename_retrying(&temp, &target) {
                // Put the old folder back rather than leave none.
                let _ = rename_retrying(&retired, &target);
                remove_path(&temp);
                return Err(e);
            }
            // May fail while a file is still open (Windows): the next start
            // removes it.
            remove_path(&retired);
            Ok(Deployed::Replaced)
        }
        Err(e) => {
            // The folder itself is in use (Windows: a process has it or a
            // file in it open without delete sharing): overwrite the files.
            remove_path(&temp);
            // Never write through a link: a symlinked/junctioned folder could
            // point anywhere, so only overwrite a real directory in place.
            if is_link(&target)? {
                return Err(e);
            }
            log(format_args!(
                "could not move {} ({e}); overwriting its files",
                target.display()
            ));
            write_files(&target, files)?;
            Ok(Deployed::Overwritten)
        }
    }
}

/// True for a symlink or (on Windows) any reparse point such as a junction.
fn is_link(path: &Path) -> io::Result<bool> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Only plain relative paths (`a/b.js`) – never `..`, absolute or empty.
fn check_relative(rel: &str) -> io::Result<()> {
    let path = Path::new(rel);
    let plain = !rel.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid extension file path {rel:?}"),
        ))
    }
}

/// True if `dir` holds every file of `files` with identical content
/// (additional files there are ignored).
fn is_current(dir: &Path, files: &[(&str, &[u8])]) -> bool {
    dir.is_dir()
        && files
            .iter()
            .all(|(rel, bytes)| fs::read(dir.join(rel)).is_ok_and(|on_disk| on_disk == *bytes))
}

/// Writes `manifest.json` last: a browser that reloads the extension while
/// the in-place fallback is still overwriting files still sees the old
/// version (and keeps waiting for the new one).
fn write_files(dir: &Path, files: &[(&str, &[u8])]) -> io::Result<()> {
    let manifest_last = files
        .iter()
        .filter(|(rel, _)| *rel != "manifest.json")
        .chain(files.iter().filter(|(rel, _)| *rel == "manifest.json"));
    for (rel, bytes) in manifest_last {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        retrying(|| fs::write(&path, bytes))?;
    }
    Ok(())
}

/// A fresh name `.browser-extension.<unique>.<suffix>` next to the folder.
fn work_path(data_dir: &Path, suffix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    data_dir.join(format!(
        "{WORK_PREFIX}{:x}-{nanos:x}{suffix}",
        std::process::id()
    ))
}

fn create_work_dir(data_dir: &Path, suffix: &str) -> io::Result<PathBuf> {
    loop {
        let dir = work_path(data_dir, suffix);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Removes temporary/retired folders of earlier runs (interrupted, or a
/// file was still open when they should have been deleted).
fn remove_leftovers(data_dir: &Path) {
    let Ok(entries) = fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(WORK_PREFIX)
            && (name.ends_with(TEMP_SUFFIX) || name.ends_with(RETIRED_SUFFIX))
        {
            remove_path(&entry.path());
        }
    }
}

/// Deletes a folder (or file / symlink – never what a symlink points to),
/// logging failures.
fn remove_path(path: &Path) {
    let result = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    if let Err(e) = result {
        log(format_args!("could not delete {}: {e}", path.display()));
    }
}

fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    retrying(|| fs::rename(from, to))
}

/// Runs `op` up to [`RENAME_ATTEMPTS`] times with growing pauses: on
/// Windows a file opened by another process (browser, virus scanner) makes
/// renames and writes fail for a moment. `NotFound` is not retried.
fn retrying(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound || attempt >= RENAME_ATTEMPTS => {
                return Err(e)
            }
            Err(_) => {
                std::thread::sleep(RETRY_DELAY * attempt);
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> Vec<u8> {
        format!("{{\"manifest_version\":3,\"name\":\"Keystead\",\"version\":\"{version}\"}}")
            .into_bytes()
    }

    fn bundle<'a>(manifest: &'a [u8], script: &'a [u8]) -> Vec<(&'static str, &'a [u8])> {
        vec![
            ("background.js", script),
            ("lib/bridge.js", b"// bridge"),
            ("manifest.json", manifest),
        ]
    }

    fn read(dir: &Path, rel: &str) -> Vec<u8> {
        fs::read(extension_dir(dir).join(rel)).unwrap()
    }

    fn work_dirs(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(WORK_PREFIX))
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn links_are_detected_so_the_fallback_never_writes_through_them() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!is_link(&real).unwrap());
        assert!(is_link(&link).unwrap());
    }

    #[test]
    fn embedded_extension_is_complete() {
        let paths: Vec<&str> = FILES.iter().map(|(p, _)| *p).collect();
        for needed in [
            "manifest.json",
            "background.js",
            "popup.html",
            "lib/bridge.js",
        ] {
            assert!(paths.contains(&needed), "{needed} missing: {paths:?}");
        }
        assert!(paths.iter().all(|p| check_relative(p).is_ok()));
        // Chrome requires 1–4 dot-separated integers.
        let version = bundled_version();
        let parts: Vec<&str> = version.split('.').collect();
        assert!((1..=4).contains(&parts.len()), "{version}");
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "{version}");
    }

    #[test]
    fn fresh_write() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let m = manifest("2.0.0.5");
        assert_eq!(
            deploy(&data, &bundle(&m, b"v5")).unwrap(),
            Deployed::Created
        );
        assert_eq!(read(&data, "manifest.json"), m);
        assert_eq!(read(&data, "background.js"), b"v5");
        assert_eq!(read(&data, "lib/bridge.js"), b"// bridge");
        assert!(work_dirs(&data).is_empty());
    }

    #[test]
    fn same_version_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest("2.0.0.5");
        deploy(dir.path(), &bundle(&m, b"v5")).unwrap();
        let ext = extension_dir(dir.path());
        // A file the user (or the browser) put there stays.
        fs::write(ext.join("notes.txt"), "mine").unwrap();
        let before = fs::metadata(ext.join("background.js"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            deploy(dir.path(), &bundle(&m, b"v5")).unwrap(),
            Deployed::Unchanged
        );
        assert_eq!(fs::read_to_string(ext.join("notes.txt")).unwrap(), "mine");
        let after = fs::metadata(ext.join("background.js"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "not rewritten");
    }

    #[test]
    fn upgrade_replaces_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let old = manifest("2.0.0.5");
        let mut old_bundle = bundle(&old, b"v5");
        old_bundle.push(("obsolete.js", b"gone in v6"));
        deploy(dir.path(), &old_bundle).unwrap();

        let new = manifest("2.0.0.6");
        assert_eq!(
            deploy(dir.path(), &bundle(&new, b"v6")).unwrap(),
            Deployed::Replaced
        );
        assert_eq!(read(dir.path(), "manifest.json"), new);
        assert_eq!(read(dir.path(), "background.js"), b"v6");
        assert!(!extension_dir(dir.path()).join("obsolete.js").exists());
        assert!(work_dirs(dir.path()).is_empty());
    }

    #[test]
    fn missing_or_changed_files_are_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest("2.0.0.5");
        deploy(dir.path(), &bundle(&m, b"v5")).unwrap();
        fs::remove_file(extension_dir(dir.path()).join("lib/bridge.js")).unwrap();
        assert_eq!(
            deploy(dir.path(), &bundle(&m, b"v5")).unwrap(),
            Deployed::Replaced
        );
        assert_eq!(read(dir.path(), "lib/bridge.js"), b"// bridge");
        fs::write(extension_dir(dir.path()).join("background.js"), "edited").unwrap();
        assert_eq!(
            deploy(dir.path(), &bundle(&m, b"v5")).unwrap(),
            Deployed::Replaced
        );
        assert_eq!(read(dir.path(), "background.js"), b"v5");
    }

    #[test]
    fn interrupted_runs_are_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let m = manifest("2.0.0.5");
        deploy(data, &bundle(&m, b"v5")).unwrap();
        // Leftovers of a run that was killed half-way.
        let temp = data.join(".browser-extension.1a2b-3c.tmp");
        fs::create_dir_all(temp.join("lib")).unwrap();
        fs::write(temp.join("lib/half.js"), "x").unwrap();
        let retired = data.join(".browser-extension.1a2b-3d.old");
        fs::create_dir_all(&retired).unwrap();
        fs::write(retired.join("manifest.json"), "{}").unwrap();
        // Not ours: kept.
        fs::create_dir_all(data.join("vaults")).unwrap();
        fs::write(data.join("vaults").join("a.keystead"), "vault").unwrap();
        fs::write(data.join(".browser-extension-notes.txt"), "user file").unwrap();

        assert_eq!(
            deploy(data, &bundle(&m, b"v5")).unwrap(),
            Deployed::Unchanged
        );
        assert!(!temp.exists());
        assert!(!retired.exists());
        assert_eq!(
            fs::read_to_string(data.join("vaults/a.keystead")).unwrap(),
            "vault"
        );
        assert!(data.join(".browser-extension-notes.txt").exists());
        assert!(work_dirs(data).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep.txt"), "user data").unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        std::os::unix::fs::symlink(&elsewhere, extension_dir(&data)).unwrap();

        let m = manifest("2.0.0.7");
        assert_eq!(
            deploy(&data, &bundle(&m, b"v7")).unwrap(),
            Deployed::Replaced
        );
        assert!(!fs::symlink_metadata(extension_dir(&data))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(elsewhere.join("keep.txt")).unwrap(),
            "user data"
        );
    }

    #[test]
    fn bad_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../evil.js", "/abs.js", "", "a/../../b"] {
            let files: Vec<(&str, &[u8])> = vec![(bad, b"x")];
            assert!(deploy(dir.path(), &files).is_err(), "{bad}");
        }
        assert!(!extension_dir(dir.path()).exists());
    }

    #[test]
    fn version_is_announced_only_after_a_successful_deploy() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        assert_eq!(deployed_version(&data), None, "nothing written yet");
        let target = deploy_logged(&data);
        assert_eq!(deployed_version(&data), Some(bundled_version()));
        assert_eq!(
            read(&data, "manifest.json"),
            FILES.iter().find(|(p, _)| *p == "manifest.json").unwrap().1
        );
        // Another data directory (portable-mode switch): not before its own
        // deploy.
        assert_eq!(deployed_version(&dir.path().join("other")), None);
        // A failed deploy of the same folder withdraws it.
        record_deploy(&target, false);
        assert_eq!(deployed_version(&data), None);
        // A data directory that cannot be created: never announced.
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "a file, not a folder").unwrap();
        deploy_logged(&blocked);
        assert_eq!(deployed_version(&blocked), None);
    }

    #[test]
    fn the_manifest_is_written_last() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest("2.0.0.8");
        // "x" is a file, so "x/y.js" fails: everything before it is written,
        // the manifest (sorted first) is not.
        let files: Vec<(&str, &[u8])> = vec![("manifest.json", &m), ("x", b"1"), ("x/y.js", b"2")];
        assert!(write_files(dir.path(), &files).is_err());
        assert!(dir.path().join("x").exists());
        assert!(!dir.path().join("manifest.json").exists());
    }

    #[test]
    fn manifest_versions() {
        assert_eq!(
            manifest_version(&manifest("2.0.0.42")).as_deref(),
            Some("2.0.0.42")
        );
        assert_eq!(manifest_version(b"{}"), None);
        assert_eq!(manifest_version(b"not json"), None);
        assert!(is_managed_entry("browser-extension"));
        assert!(is_managed_entry(".browser-extension.1-2.tmp"));
        assert!(!is_managed_entry("vaults"));
        assert!(!is_managed_entry("browser-extension-old"));
    }
}
