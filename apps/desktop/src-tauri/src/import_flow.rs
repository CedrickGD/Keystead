//! Import with a preview – the import dialog and drag & drop (see "Import
//! with preview" in docs/ARCHITECTURE.md):
//!
//! 1. `analyze_import` recognises the file by its content
//!    (`keystead_core::import::detect_import`), reads it – encrypted formats
//!    only once a password is given – and compares it with the open vault
//!    (`plan_import`). The UI gets a secret-free [`ImportPreview`].
//! 2. The plan waits in the backend ([`ImportSlot`]) until the user decides:
//!    `commit_import` with a conflict mode, or `cancel_import`.
//!
//! The plan holds the decrypted items of the file, so it is kept as briefly
//! as possible: one slot (a new analysis replaces the old plan), dropped –
//! and its secrets overwritten (`ImportPlan`'s `Drop`) – after every commit
//! attempt, on cancel, after [`IMPORT_TTL`] (checked by the monitor thread)
//! and whenever its vault is closed (lock of any kind, vault switch through
//! the browser extension, deletion, portable-mode move, update:
//! `Core::finish_lock`).
//!
//! Also here: `show_export`, the export dialog's "Im Ordner anzeigen".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keystead_core::import::{
    self, ConflictMode, ImportFormat, ImportPlan, ImportPreview, ImportReport,
};
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::commands::{parse, run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::state::Core;

/// How long an analysed import waits for `commit_import`.
pub const IMPORT_TTL: Duration = Duration::from_secs(15 * 60);

/// Answer of `analyze_import`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportAnalysis {
    /// Identifies the waiting plan for `commit_import` / `cancel_import`;
    /// `None` while the file still needs its password (no plan yet).
    pub import_id: Option<String>,
    pub file_name: String,
    /// The detected format.
    pub format: ImportFormat,
    /// The format is encrypted (VaultX 1.x vault, Keystead export).
    pub needs_password: bool,
    /// `None` = ask for the password and analyse again.
    pub preview: Option<ImportPreview>,
}

/// An analysed import waiting for the user's decision.
struct PendingImport {
    id: String,
    /// The vault the plan was made for.
    vault_id: String,
    expires: Instant,
    plan: ImportPlan,
}

/// The one waiting import plan (`Core::import_slot`). Dropping a plan
/// overwrites its secrets.
#[derive(Default)]
pub struct ImportSlot {
    pending: Option<PendingImport>,
}

impl ImportSlot {
    /// Stores a plan, dropping the previous one.
    fn put(&mut self, pending: PendingImport) {
        self.pending = Some(pending);
    }

    /// Takes the plan `id` out if it has not expired at `now`.
    fn take(&mut self, id: &str, now: Instant) -> Option<PendingImport> {
        self.expire(now);
        if self.pending.as_ref().is_some_and(|p| p.id == id) {
            self.pending.take()
        } else {
            None
        }
    }

    /// Drops the waiting plan, if any. Returns whether there was one.
    pub fn clear(&mut self) -> bool {
        self.pending.take().is_some()
    }

    /// Drops the plan `id` (cancel). Returns whether it was waiting.
    fn cancel(&mut self, id: &str) -> bool {
        if self.pending.as_ref().is_some_and(|p| p.id == id) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// Drops a plan made for `vault_id` (that vault was closed).
    pub fn drop_for_vault(&mut self, vault_id: &str) -> bool {
        if self.pending.as_ref().is_some_and(|p| p.vault_id == vault_id) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// Drops the plan if it expired at `now`. Returns whether it did.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.pending.as_ref().is_some_and(|p| now >= p.expires) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn pending_id(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.id.as_str())
    }
}

/// A fresh id per analysis (only this process hands them out and checks
/// them, so a counter is enough).
fn next_import_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("import-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Logic (unit-tested without Tauri)
// ---------------------------------------------------------------------------

/// `analyze_import`: see the module documentation. Errors: `locked` (no
/// open vault, or not `page_vault_id`), `invalid_input:path_required`, the
/// detection and reading errors of `keystead_core::import`
/// (`unsupported:unknown_format`, `unsupported:bitwarden_encrypted`,
/// `unsupported:file_too_large`, `not_found`, `io:…`, `wrong_password`, …).
pub(crate) fn analyze(
    c: &Core,
    path: &str,
    password: Option<&str>,
    page_vault_id: Option<&str>,
) -> AppResult<ImportAnalysis> {
    if path.trim().is_empty() {
        return Err(AppError::invalid("path_required"));
    }
    // A new analysis replaces the waiting plan, also when it fails.
    c.import_slot().clear();
    // Fail early (before the slow key derivation) if the page's vault is
    // not open; checked again below, under the same lock as the plan.
    c.state().page_vault(page_vault_id)?;

    let path = Path::new(path);
    let detected = import::detect_import(path)?;
    let password = password.filter(|p| !p.is_empty());
    if detected.needs_password && password.is_none() {
        return Ok(ImportAnalysis {
            import_id: None,
            file_name: detected.file_name,
            format: detected.format,
            needs_password: true,
            preview: None,
        });
    }
    // Key derivation and parsing without the state lock.
    let parsed = import::read_import(path, detected.format, password)?;

    let st = c.state();
    let vault = st.page_vault(page_vault_id)?;
    let plan = import::plan_import(vault.data(), parsed);
    let preview = plan.preview();
    let id = next_import_id();
    // Stored while the vault is known to be open: closing it (which takes it
    // out under this lock first) drops the plan afterwards.
    c.import_slot().put(PendingImport {
        id: id.clone(),
        vault_id: vault.id().to_owned(),
        expires: Instant::now() + IMPORT_TTL,
        plan,
    });
    drop(st);
    Ok(ImportAnalysis {
        import_id: Some(id),
        file_name: detected.file_name,
        format: detected.format,
        needs_password: detected.needs_password,
        preview: Some(preview),
    })
}

/// `commit_import`: applies the waiting plan `import_id` to the page's
/// vault (`Core::mutate_for_page`: reload and retry once on `conflict`) and
/// drops it – also when the commit fails. `not_found` for an unknown,
/// cancelled, already committed or expired plan; `locked` if the page's
/// vault is not open or the plan was made for another vault.
pub(crate) fn commit(
    c: &Core,
    import_id: &str,
    mode: ConflictMode,
    page_vault_id: Option<&str>,
) -> AppResult<ImportReport> {
    let pending = c
        .import_slot()
        .take(import_id, Instant::now())
        .ok_or_else(|| keystead_core::Error::NotFound(format!("import {import_id}")))?;
    if page_vault_id != Some(pending.vault_id.as_str()) {
        return Err(AppError::Locked);
    }
    let report =
        c.mutate_for_page(page_vault_id, |v| v.commit_import_ref(&pending.plan, mode))?;
    drop(pending);
    c.emit_changed();
    Ok(report)
}

/// `cancel_import`: drops the waiting plan `import_id` (unknown ids are
/// ignored).
pub(crate) fn cancel(c: &Core, import_id: &str) {
    c.import_slot().cancel(import_id);
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Recognises and reads an import file and compares it with the open vault.
#[tauri::command]
pub async fn analyze_import(
    core: Shared<'_>,
    path: String,
    password: Option<String>,
    page_vault_id: Option<String>,
) -> CmdResult<ImportAnalysis> {
    run(&core, true, move |c| {
        let password = password.map(Zeroizing::new);
        analyze(
            c,
            &path,
            password.as_deref().map(String::as_str),
            page_vault_id.as_deref(),
        )
    })
    .await
}

/// Imports the analysed file; `conflictMode`: `"skip"` | `"update"` |
/// `"keepBoth"`.
#[tauri::command]
pub async fn commit_import(
    core: Shared<'_>,
    import_id: String,
    conflict_mode: Value,
    page_vault_id: Option<String>,
) -> CmdResult<ImportReport> {
    run(&core, true, move |c| {
        let mode: ConflictMode = parse(conflict_mode, "conflictMode")?;
        commit(c, &import_id, mode, page_vault_id.as_deref())
    })
    .await
}

/// Drops an analysed import without importing anything.
#[tauri::command]
pub async fn cancel_import(core: Shared<'_>, import_id: String) -> CmdResult<()> {
    run(&core, false, move |c: &Arc<Core>| {
        cancel(c, &import_id);
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Export: "Im Ordner anzeigen"
// ---------------------------------------------------------------------------

/// The file the last successful `export_data` wrote: `show_export` opens
/// only its folder, never a path the page names.
static LAST_EXPORT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Called by `export_data` after writing `path`.
pub fn remember_export(path: &Path) {
    let mut last = LAST_EXPORT.lock().unwrap_or_else(|p| p.into_inner());
    *last = Some(path.to_path_buf());
}

/// The last exported file if it still exists.
fn last_export() -> Option<PathBuf> {
    let last = LAST_EXPORT.lock().unwrap_or_else(|p| p.into_inner());
    last.as_ref().filter(|p| p.is_file()).cloned()
}

/// Shows `file` in the file manager: Windows Explorer with the file
/// selected, elsewhere its folder.
fn reveal(file: &Path) -> AppResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // `/select,"<path>"` as one raw argument (Explorer's own syntax; a
        // Windows path cannot contain `"`).
        let mut child = std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", file.display()))
            .spawn()
            .map_err(|e| AppError::io(format!("explorer: {e}")))?;
        // Reap it in the background (Explorer answers with exit code 1).
        let _ = std::thread::Builder::new()
            .name("keystead-explorer".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let dir = file
            .parent()
            .filter(|d| d.is_dir())
            .ok_or_else(|| AppError::io(format!("{}: no folder", file.display())))?;
        open::that_detached(dir).map_err(|e| AppError::io(format!("{}: {e}", dir.display())))
    }
}

/// "Im Ordner anzeigen" after an export: `not_found` if nothing was
/// exported since the app started or the file is gone.
#[tauri::command]
pub async fn show_export(core: Shared<'_>) -> CmdResult<()> {
    run(&core, true, |_| {
        let file =
            last_export().ok_or_else(|| keystead_core::Error::NotFound("export".into()))?;
        reveal(&file)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use keystead_core::export;
    use keystead_core::model::VaultData;
    use keystead_core::settings::Settings;
    use keystead_core::KdfParams;

    use super::*;
    use crate::state::test_support::core_with_open_vault;
    use crate::state::{LockReason, EVENT_CHANGED};

    const CSV: &str = "name,url,username,password\n\
        GitHub,https://github.com,octocat,hunter2\n\
        Mail,https://mail.example.com,max@example.com,Sommer2024!\n";

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    fn vault_id(c: &Core) -> String {
        c.state().vault.as_ref().unwrap().id().to_owned()
    }

    fn item_names(c: &Core) -> Vec<String> {
        c.read(|v| v.items().iter().map(|i| i.name.clone()).collect())
            .unwrap()
    }

    fn analyze_csv(c: &Core, dir: &Path) -> ImportAnalysis {
        let path = write(dir, "chrome.csv", CSV);
        analyze(c, path.to_str().unwrap(), None, Some(&vault_id(c))).unwrap()
    }

    #[test]
    fn analyze_previews_and_commit_imports_once() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let analysis = analyze_csv(&c, dir.path());
        assert_eq!(analysis.format, ImportFormat::Csv);
        assert!(!analysis.needs_password);
        assert_eq!(analysis.file_name, "chrome.csv");
        let preview = analysis.preview.unwrap();
        assert_eq!(preview.new_count, 2);
        assert!(preview.duplicates.is_empty() && preview.conflicts.is_empty());
        // Nothing is written before the commit.
        assert!(item_names(&c).is_empty());

        let import_id = analysis.import_id.unwrap();
        let report = commit(&c, &import_id, ConflictMode::Skip, Some(&id)).unwrap();
        assert_eq!(report.imported, 2);
        assert_eq!(item_names(&c).len(), 2);
        assert!(c.emitted().iter().any(|(name, _)| name == EVENT_CHANGED));

        // The plan is gone after the commit.
        let again = commit(&c, &import_id, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(again.code(), "not_found");

        // A second analysis of the same file finds only duplicates.
        let preview = analyze_csv(&c, dir.path()).preview.unwrap();
        assert_eq!(preview.new_count, 0);
        assert_eq!(preview.duplicates.len(), 2);
    }

    #[test]
    fn conflicts_follow_the_chosen_mode() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let first = analyze_csv(&c, dir.path()).import_id.unwrap();
        commit(&c, &first, ConflictMode::Skip, Some(&id)).unwrap();

        let changed = write(
            dir.path(),
            "changed.csv",
            "name,url,username,password\nGitHub,https://github.com,octocat,n3w-Passw0rt\n",
        );
        let analysis = analyze(&c, changed.to_str().unwrap(), None, Some(&id)).unwrap();
        let preview = analysis.preview.unwrap();
        assert_eq!(preview.conflicts.len(), 1);
        let report = commit(
            &c,
            &analysis.import_id.unwrap(),
            ConflictMode::Update,
            Some(&id),
        )
        .unwrap();
        assert_eq!((report.imported, report.updated), (0, 1));
        let password = c
            .read(|v| {
                v.items()
                    .iter()
                    .find(|i| i.name == "GitHub")
                    .and_then(|i| i.login.as_ref())
                    .map(|l| l.password.clone())
            })
            .unwrap();
        assert_eq!(password.as_deref(), Some("n3w-Passw0rt"));
    }

    #[test]
    fn encrypted_files_ask_for_the_password_first() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let source = core_with_open_vault(&dir.path().join("source"), Settings::default());
        let path = write(&dir.path().join("source"), "chrome.csv", CSV);
        let report = source
            .mutate(|v| keystead_core::import::import_into(v, "csv", &path, None))
            .unwrap();
        assert_eq!(report.imported, 2);
        let export_path = dir.path().join("Backup.keystead");
        let data: VaultData = source.read(|v| v.data().clone()).unwrap();
        export::export_encrypted_with_params(
            &data,
            &export_path,
            "export-pw",
            KdfParams::insecure_for_tests(),
        )
        .unwrap();
        let export = export_path.to_str().unwrap();

        // No password (or an empty one): the dialog asks for it; no plan yet.
        for password in [None, Some("")] {
            let analysis = analyze(&c, export, password, Some(&id)).unwrap();
            assert_eq!(analysis.format, ImportFormat::Keystead);
            assert!(analysis.needs_password);
            assert!(analysis.preview.is_none());
            assert!(analysis.import_id.is_none());
            assert_eq!(c.import_slot().pending_id(), None);
        }
        let wrong = analyze(&c, export, Some("nope"), Some(&id)).unwrap_err();
        assert_eq!(wrong.code(), "wrong_password");

        let analysis = analyze(&c, export, Some("export-pw"), Some(&id)).unwrap();
        assert!(analysis.needs_password);
        assert_eq!(analysis.preview.unwrap().new_count, 2);
        assert!(analysis.import_id.is_some());
    }

    #[test]
    fn unknown_files_are_refused_with_stable_codes() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let text = write(dir.path(), "notes.txt", "just some text\nnothing to import\n");
        let err = analyze(&c, text.to_str().unwrap(), None, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "unsupported:unknown_format");
        let encrypted = write(
            dir.path(),
            "bitwarden.json",
            r#"{"encrypted":true,"passwordProtected":true,"data":"x"}"#,
        );
        let err = analyze(&c, encrypted.to_str().unwrap(), None, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "unsupported:bitwarden_encrypted");
        let missing = dir.path().join("missing.csv");
        let err = analyze(&c, missing.to_str().unwrap(), None, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");
        let err = analyze(&c, "  ", None, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "invalid_input:path_required");
    }

    #[test]
    fn a_new_analysis_replaces_the_waiting_plan() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let first = analyze_csv(&c, dir.path()).import_id.unwrap();
        let second = analyze_csv(&c, dir.path()).import_id.unwrap();
        assert_ne!(first, second);
        assert_eq!(c.import_slot().pending_id(), Some(second.as_str()));
        let err = commit(&c, &first, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");

        // A failed analysis drops the waiting plan as well.
        let bad = write(dir.path(), "bad.txt", "nothing");
        analyze(&c, bad.to_str().unwrap(), None, Some(&id)).unwrap_err();
        assert_eq!(c.import_slot().pending_id(), None);
        let err = commit(&c, &second, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");
        assert!(item_names(&c).is_empty());
    }

    #[test]
    fn cancel_drops_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let import_id = analyze_csv(&c, dir.path()).import_id.unwrap();
        cancel(&c, "import-unknown");
        assert_eq!(c.import_slot().pending_id(), Some(import_id.as_str()));
        cancel(&c, &import_id);
        assert_eq!(c.import_slot().pending_id(), None);
        let err = commit(&c, &import_id, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");
        assert!(item_names(&c).is_empty());
    }

    #[test]
    fn plans_expire_after_the_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let import_id = analyze_csv(&c, dir.path()).import_id.unwrap();
        let now = Instant::now();
        assert!(!c.import_slot().expire(now));
        assert!(!c
            .import_slot()
            .expire(now + IMPORT_TTL - Duration::from_secs(5)));
        assert_eq!(c.import_slot().pending_id(), Some(import_id.as_str()));
        assert!(c
            .import_slot()
            .expire(now + IMPORT_TTL + Duration::from_secs(1)));
        let err = commit(&c, &import_id, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");

        // `take` checks the expiry itself (a commit after the TTL, before
        // the monitor's next check).
        let import_id = analyze_csv(&c, dir.path()).import_id.unwrap();
        let late = Instant::now() + IMPORT_TTL + Duration::from_secs(1);
        assert!(c.import_slot().take(&import_id, late).is_none());
        assert_eq!(c.import_slot().pending_id(), None);
    }

    #[test]
    fn locking_drops_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let import_id = analyze_csv(&c, dir.path()).import_id.unwrap();
        assert!(c.lock(Some(LockReason::Timeout)));
        assert_eq!(c.import_slot().pending_id(), None);

        // Unlocked again: the old plan stays gone.
        c.unlock(&id, "master").unwrap();
        let err = commit(&c, &import_id, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");
        assert!(item_names(&c).is_empty());

        // Locked: nothing to analyse.
        c.lock(None);
        let path = write(dir.path(), "x.csv", CSV);
        let err = analyze(&c, path.to_str().unwrap(), None, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "locked");
    }

    #[test]
    fn a_vault_switch_drops_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let old_id = vault_id(&c);
        let import_id = analyze_csv(&c, dir.path()).import_id.unwrap();
        // The browser extension opens another vault (`install_vault`).
        let other = c
            .store()
            .create_vault_with_params("Other", "pw", KdfParams::insecure_for_tests())
            .unwrap();
        let new_id = other.id().to_owned();
        c.install_vault(other).unwrap();
        assert_eq!(c.import_slot().pending_id(), None);
        for page in [Some(old_id.as_str()), Some(new_id.as_str())] {
            let err = commit(&c, &import_id, ConflictMode::Skip, page).unwrap_err();
            assert_eq!(err.code(), "not_found");
        }
        assert!(item_names(&c).is_empty(), "nothing lands in the new vault");
    }

    #[test]
    fn page_commands_for_another_vault_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let path = write(dir.path(), "chrome.csv", CSV);
        let path = path.to_str().unwrap();
        for page in [None, Some("another-vault")] {
            let err = analyze(&c, path, None, page).unwrap_err();
            assert_eq!(err.code(), "locked");
        }

        // A plan of this vault, committed by a page of another vault: refused
        // and dropped, nothing imported.
        let import_id = analyze(&c, path, None, Some(&id))
            .unwrap()
            .import_id
            .unwrap();
        let err = commit(&c, &import_id, ConflictMode::Skip, Some("another-vault")).unwrap_err();
        assert_eq!(err.code(), "locked");
        assert!(item_names(&c).is_empty());
        let err = commit(&c, &import_id, ConflictMode::Skip, Some(&id)).unwrap_err();
        assert_eq!(err.code(), "not_found");
    }

    #[test]
    fn conflict_modes_parse_from_the_ui_values() {
        for (raw, mode) in [
            ("skip", ConflictMode::Skip),
            ("update", ConflictMode::Update),
            ("keepBoth", ConflictMode::KeepBoth),
        ] {
            assert_eq!(parse::<ConflictMode>(Value::from(raw), "conflictMode").unwrap(), mode);
        }
        let err = parse::<ConflictMode>(Value::from("merge"), "conflictMode").unwrap_err();
        assert_eq!(err.code(), "invalid_input:conflictMode");
    }

    #[test]
    fn show_export_only_knows_the_last_exported_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "Privat-2026-10-09.csv", "name\n");
        remember_export(&file);
        assert_eq!(last_export(), Some(file.clone()));
        fs::remove_file(&file).unwrap();
        assert_eq!(last_export(), None, "a deleted export is not shown");
    }

    #[test]
    fn analysis_serializes_for_the_ui() {
        let analysis = ImportAnalysis {
            import_id: None,
            file_name: "vault_max.json".into(),
            format: ImportFormat::Legacy,
            needs_password: true,
            preview: None,
        };
        assert_eq!(
            serde_json::to_value(&analysis).unwrap(),
            serde_json::json!({
                "importId": null,
                "fileName": "vault_max.json",
                "format": "legacy",
                "needsPassword": true,
                "preview": null,
            })
        );
    }
}
