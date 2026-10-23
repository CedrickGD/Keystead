//! The password generator of the desktop app: generating, the generator
//! history and the generator presets (see "Generator" in "Desktop backend
//! behaviour details" in docs/ARCHITECTURE.md).
//!
//! The generator page and the editor's popover generate *previews*
//! (`generate_password` with `remember: false`): nothing is stored until the
//! user uses a password – copies it or takes it into an item – which the page
//! reports with `remember_generated`. The history reaches the webview
//! redacted (`generator_history`: id and time only); one password is revealed
//! (`reveal_generated`) or copied (`copy_generated`) on demand, like the
//! secrets of items (see `crate::secrets`).
//!
//! Presets: the built-in ones live in code (`keystead_core::generator`), the
//! user's in `settings.json` (`Settings::generator_presets` and
//! `generator_default`). Only the preset commands here change those two;
//! `save_settings` keeps them as stored.

use std::sync::Arc;

use keystead_core::generator::{self as core_gen, GeneratorOptions, GeneratorPreset};
use keystead_core::model::GeneratedPasswordInfo;
use keystead_core::settings::{Settings, GENERATOR_PRESETS_MAX};
use keystead_core::{Error as CoreError, UnlockedVault};
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::commands::{change_settings, parse, run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::state::Core;
use crate::wipe::Wiped;

/// Longest password `remember_generated` accepts (characters) – the same
/// limit as the bridge's `remember_generated` (the phone has no bridge; a
/// desktop test keeps the two equal).
pub const MAX_REMEMBERED_CHARS: usize = 1024;

fn not_found(what: &str) -> AppError {
    CoreError::NotFound(what.to_owned()).into()
}

// ---------------------------------------------------------------------------
// Generating and the history
// ---------------------------------------------------------------------------

/// `generate_password`: a password for `options`. With `remember` it also
/// goes into the history of the page's vault (kept for compatibility; the UI
/// generates previews and reports used ones with `remember_generated`) –
/// skipped while another vault or none is open, generating works anyway.
pub fn generate_for_page(
    core: &Core,
    page_vault_id: Option<&str>,
    options: &GeneratorOptions,
    remember: bool,
) -> AppResult<Zeroizing<String>> {
    let password = Zeroizing::new(core_gen::generate(options)?);
    if remember {
        match core.mutate_for_page(page_vault_id, |v| v.add_generated_password(&password)) {
            Ok(()) | Err(AppError::Locked) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(password)
}

/// Adds `password` – one the user used – to the vault's generator history
/// (the page's `remember_generated` and the bridge's). The same password as
/// the newest entry is not added again: a preview copied twice, or copied
/// and then taken into an item, is one use.
pub fn remember_in(vault: &mut UnlockedVault, password: &str) -> keystead_core::Result<()> {
    if vault
        .generator_history()
        .first()
        .is_some_and(|g| g.password == password)
    {
        return Ok(());
    }
    vault.add_generated_password(password)
}

/// `remember_generated`: 1..=[`MAX_REMEMBERED_CHARS`] characters
/// (`invalid_input:password_empty` / `password_too_long`), into the history
/// of the page's vault (`locked` for another one).
pub fn remember_for_page(
    core: &Core,
    page_vault_id: Option<&str>,
    password: &str,
) -> AppResult<()> {
    if password.is_empty() {
        return Err(AppError::invalid("password_empty"));
    }
    if password.chars().count() > MAX_REMEMBERED_CHARS {
        return Err(AppError::invalid("password_too_long"));
    }
    core.mutate_for_page(page_vault_id, |v| remember_in(v, password))
}

/// The password of history entry `id` (`not_found` if there is none).
fn entry_password(vault: &UnlockedVault, id: &str) -> AppResult<Zeroizing<String>> {
    vault
        .generated_password(id)
        .map(|g| Zeroizing::new(g.password.clone()))
        .ok_or_else(|| not_found("generated password"))
}

/// `reveal_generated`: the password of history entry `id` of the page's
/// vault (`locked` / `not_found`).
pub fn reveal_for_page(
    core: &Core,
    page_vault_id: Option<&str>,
    id: &str,
) -> AppResult<Zeroizing<String>> {
    let st = core.state();
    entry_password(st.page_vault(page_vault_id)?, id)
}

/// `copy_generated`: hands the password of history entry `id` of the page's
/// vault to `copy` (the sensitive clipboard path) after the state lock is
/// released, with the lock epoch read under that lock – like
/// `secrets::copy_secret_with`.
pub fn copy_generated_with(
    core: &Core,
    page_vault_id: Option<&str>,
    id: &str,
    copy: impl FnOnce(&str, u64) -> AppResult<()>,
) -> AppResult<()> {
    let (password, epoch) = {
        let st = core.state();
        let password = entry_password(st.page_vault(page_vault_id)?, id)?;
        (password, core.lock_epoch())
    };
    copy(&password, epoch)
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// The answer of `generator_presets` and of the preset commands.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorPresets {
    /// The built-in presets in display order; `name` is an English
    /// fallback, the UI shows its own translation by id.
    pub builtin: Vec<GeneratorPreset>,
    /// The user's presets (`settings.generatorPresets`), in their order.
    pub user: Vec<GeneratorPreset>,
    /// Id of the default preset (`settings.generatorDefault`).
    pub default_id: String,
    /// Its options: what the editor's popover and the browser extension's
    /// suggestions use without options of their own.
    pub default_options: GeneratorOptions,
}

impl GeneratorPresets {
    pub fn of(settings: &Settings) -> Self {
        GeneratorPresets {
            builtin: core_gen::builtin_presets(),
            user: settings.generator_presets.clone(),
            default_id: settings.generator_default.clone(),
            default_options: settings.default_generator_options(),
        }
    }
}

/// The answer of `save_generator_preset`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedGeneratorPreset {
    /// Id of the saved preset (a new one gets a fresh UUID v4).
    pub id: String,
    pub presets: GeneratorPresets,
}

/// `save_generator_preset`: without `id` a new user preset (at most
/// [`GENERATOR_PRESETS_MAX`], else `invalid_input:presets_too_many`), with
/// one the name and options of that user preset (`not_found`; a built-in id
/// → `invalid_input:preset_id`). The name and options are validated like
/// `GeneratorPreset::normalized` (`preset_name_required`,
/// `preset_name_too_long`, the generator's codes); a name another user
/// preset has (case-insensitive) → `invalid_input:preset_name_taken`.
pub fn save_preset(
    core: &Arc<Core>,
    id: Option<&str>,
    name: &str,
    options: GeneratorOptions,
) -> AppResult<SavedGeneratorPreset> {
    let (id, settings) = change_settings(core, |s| {
        let id = match id {
            Some(id) if core_gen::is_builtin_preset(id) => {
                return Err(AppError::invalid("preset_id"))
            }
            Some(id) if !s.generator_presets.iter().any(|p| p.id == id) => {
                return Err(not_found("generator preset"))
            }
            Some(id) => id.to_owned(),
            None if s.generator_presets.len() >= GENERATOR_PRESETS_MAX => {
                return Err(AppError::invalid("presets_too_many"))
            }
            None => uuid::Uuid::new_v4().to_string(),
        };
        let preset = GeneratorPreset {
            id: id.clone(),
            name: name.to_owned(),
            options,
        }
        .normalized()?;
        let lower = preset.name.to_lowercase();
        if s.generator_presets
            .iter()
            .any(|p| p.id != id && p.name.to_lowercase() == lower)
        {
            return Err(AppError::invalid("preset_name_taken"));
        }
        match s.generator_presets.iter_mut().find(|p| p.id == id) {
            Some(slot) => *slot = preset,
            None => s.generator_presets.push(preset),
        }
        Ok(id)
    })?;
    Ok(SavedGeneratorPreset {
        id,
        presets: GeneratorPresets::of(&settings),
    })
}

/// `delete_generator_preset`: removes a user preset (`not_found`; a built-in
/// id → `invalid_input:preset_id`). Was it the default, the default becomes
/// `"standard"` (`Settings::normalized`).
pub fn delete_preset(core: &Arc<Core>, id: &str) -> AppResult<GeneratorPresets> {
    let ((), settings) = change_settings(core, |s| {
        if core_gen::is_builtin_preset(id) {
            return Err(AppError::invalid("preset_id"));
        }
        let before = s.generator_presets.len();
        s.generator_presets.retain(|p| p.id != id);
        if s.generator_presets.len() == before {
            return Err(not_found("generator preset"));
        }
        Ok(())
    })?;
    Ok(GeneratorPresets::of(&settings))
}

/// `set_default_generator_preset`: makes a built-in or user preset the
/// default (`not_found` for an unknown id).
pub fn set_default_preset(core: &Arc<Core>, id: &str) -> AppResult<GeneratorPresets> {
    let ((), settings) = change_settings(core, |s| {
        if core_gen::preset_options(id, &s.generator_presets).is_none() {
            return Err(not_found("generator preset"));
        }
        s.generator_default = id.to_owned();
        Ok(())
    })?;
    Ok(GeneratorPresets::of(&settings))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `generate_password(options, remember?, pageVaultId)` † → the password.
/// `remember` absent or `false`: a preview, nothing is stored.
#[tauri::command]
pub async fn generate_password(
    core: Shared<'_>,
    options: Value,
    remember: Option<bool>,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<String>> {
    run(&core, true, move |c| {
        let options: GeneratorOptions = parse(options, "options")?;
        let mut password = generate_for_page(
            c,
            page_vault_id.as_deref(),
            &options,
            remember.unwrap_or(false),
        )?;
        Ok(Wiped(std::mem::take(&mut *password)))
    })
    .await
}

/// `remember_generated(password, pageVaultId)` † → `null`: the user used a
/// previewed password (copied it, took it into an item).
#[tauri::command]
pub async fn remember_generated(
    core: Shared<'_>,
    password: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        let password = Zeroizing::new(password);
        remember_for_page(c, page_vault_id.as_deref(), &password)
    })
    .await
}

/// `generator_history` → `GeneratedPasswordInfo[]` (id and time, newest
/// first) – never the passwords.
#[tauri::command]
pub async fn generator_history(core: Shared<'_>) -> CmdResult<Vec<GeneratedPasswordInfo>> {
    run(&core, false, |c| {
        c.read(UnlockedVault::generator_history_info)
    })
    .await
}

/// `reveal_generated(id, pageVaultId)` → the password of one history entry
/// (eye toggle).
#[tauri::command]
pub async fn reveal_generated(
    core: Shared<'_>,
    id: String,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<String>> {
    run(&core, true, move |c| {
        let mut password = reveal_for_page(c, page_vault_id.as_deref(), &id)?;
        Ok(Wiped(std::mem::take(&mut *password)))
    })
    .await
}

/// `copy_generated(id, pageVaultId)` → `null`: copies one history entry like
/// `copy_secret_field`; the password never reaches the webview.
#[tauri::command]
pub async fn copy_generated(
    core: Shared<'_>,
    id: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        copy_generated_with(c, page_vault_id.as_deref(), &id, |text, epoch| {
            c.copy_to_clipboard_since(text, true, Some(epoch))
        })
    })
    .await
}

#[tauri::command]
pub async fn clear_generator_history(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.clear_generator_history())
    })
    .await
}

/// `generator_presets` → [`GeneratorPresets`].
#[tauri::command]
pub async fn generator_presets(core: Shared<'_>) -> CmdResult<GeneratorPresets> {
    run(&core, false, |c| {
        Ok(GeneratorPresets::of(&c.state().settings))
    })
    .await
}

/// `save_generator_preset(id, name, options)` → [`SavedGeneratorPreset`].
#[tauri::command]
pub async fn save_generator_preset(
    core: Shared<'_>,
    id: Option<String>,
    name: String,
    options: Value,
) -> CmdResult<SavedGeneratorPreset> {
    run(&core, true, move |c| {
        let options: GeneratorOptions = parse(options, "options")?;
        save_preset(c, id.as_deref(), &name, options)
    })
    .await
}

/// `delete_generator_preset(id)` → [`GeneratorPresets`].
#[tauri::command]
pub async fn delete_generator_preset(core: Shared<'_>, id: String) -> CmdResult<GeneratorPresets> {
    run(&core, true, move |c| delete_preset(c, &id)).await
}

/// `set_default_generator_preset(id)` → [`GeneratorPresets`].
#[tauri::command]
pub async fn set_default_generator_preset(
    core: Shared<'_>,
    id: String,
) -> CmdResult<GeneratorPresets> {
    run(&core, true, move |c| set_default_preset(c, &id)).await
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use keystead_core::generator::{GeneratorKind, PRESET_PIN, PRESET_STANDARD};

    use super::*;
    use crate::state::test_support::core_with_open_vault;

    fn vault_id(c: &Core) -> String {
        c.read(|v| v.id().to_owned()).unwrap()
    }

    fn passwords(c: &Core) -> Vec<String> {
        c.read(|v| {
            v.generator_history()
                .iter()
                .map(|g| g.password.clone())
                .collect()
        })
        .unwrap()
    }

    fn stored_settings(c: &Core) -> Settings {
        Settings::load_from(&c.data_dir().join("settings.json"))
    }

    #[test]
    fn the_remembered_limit_is_the_bridges() {
        assert_eq!(
            MAX_REMEMBERED_CHARS,
            keystead_bridge::dispatcher::MAX_REMEMBERED_CHARS
        );
    }

    #[test]
    fn previews_are_stored_only_once_used() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let page = Some(id.as_str());
        let options = GeneratorOptions::default();

        // A preview is not stored.
        let preview = generate_for_page(&c, page, &options, false).unwrap();
        assert_eq!(preview.chars().count(), 20);
        assert!(passwords(&c).is_empty());

        // Using it stores it once, also when it is used twice in a row.
        remember_for_page(&c, page, &preview).unwrap();
        remember_for_page(&c, page, &preview).unwrap();
        assert_eq!(passwords(&c), [preview.as_str()]);
        let second = generate_for_page(&c, page, &options, false).unwrap();
        remember_for_page(&c, page, &second).unwrap();
        assert_eq!(passwords(&c), [second.as_str(), preview.as_str()]);
        // Only the newest entry counts as "the same use".
        remember_for_page(&c, page, &preview).unwrap();
        assert_eq!(passwords(&c).len(), 3);

        // Lengths in characters, like the bridge.
        let code = |pw: &str| remember_for_page(&c, page, pw).unwrap_err().code();
        assert_eq!(code(""), "invalid_input:password_empty");
        assert_eq!(code(&"x".repeat(1025)), "invalid_input:password_too_long");
        remember_for_page(&c, page, &"ä".repeat(1024)).unwrap();
        assert_eq!(passwords(&c).len(), 4);

        // Another page's vault or none: nothing is stored.
        for other in [None, Some("other-vault")] {
            assert_eq!(
                remember_for_page(&c, other, "pw").unwrap_err().code(),
                "locked"
            );
        }
        assert_eq!(passwords(&c).len(), 4);

        // `remember: true` (compatibility) stores right away; for another
        // vault it only generates.
        let stored = generate_for_page(&c, page, &options, true).unwrap();
        assert_eq!(passwords(&c)[0], *stored);
        generate_for_page(&c, Some("other-vault"), &options, true).unwrap();
        assert_eq!(passwords(&c).len(), 5);
        // Invalid options are the generator's error.
        let short = GeneratorOptions {
            length: 2,
            ..options
        };
        assert_eq!(
            generate_for_page(&c, page, &short, false)
                .unwrap_err()
                .code(),
            "invalid_input:length"
        );
    }

    #[test]
    fn the_history_reaches_the_page_without_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let id = vault_id(&c);
        let page = Some(id.as_str());
        for pw in ["Gen-Secret-1!", "Gen-Secret-2?", "korrekt-pferd-batterie"] {
            remember_for_page(&c, page, pw).unwrap();
        }

        let info = c.read(UnlockedVault::generator_history_info).unwrap();
        let json = serde_json::to_value(&info).unwrap();
        let text = json.to_string();
        for secret in ["Gen-Secret", "korrekt", "pferd"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        let entries = json.as_array().unwrap();
        assert_eq!(entries.len(), 3);
        for entry in entries {
            let mut keys: Vec<&str> = entry
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, ["createdAt", "id"]);
            assert!(entry["id"].as_str().is_some_and(|id| id.len() == 36));
            assert!(entry["createdAt"].is_i64());
        }
        // Newest first, each id addresses its entry.
        assert_eq!(
            *reveal_for_page(&c, page, &info[0].id).unwrap(),
            "korrekt-pferd-batterie"
        );
        assert_eq!(
            *reveal_for_page(&c, page, &info[2].id).unwrap(),
            "Gen-Secret-1!"
        );
    }

    #[test]
    fn history_entries_are_revealed_and_copied_by_id_only_for_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let vid = vault_id(&c);
        let page = Some(vid.as_str());
        remember_for_page(&c, page, "first-generated").unwrap();
        remember_for_page(&c, page, "second-generated").unwrap();
        let info = c.read(UnlockedVault::generator_history_info).unwrap();
        let (second, first) = (info[0].id.clone(), info[1].id.clone());

        assert_eq!(
            *reveal_for_page(&c, page, &first).unwrap(),
            "first-generated"
        );
        let copied = RefCell::new(Vec::<(String, u64)>::new());
        let copy = |page: Option<&str>, id: &str| {
            copy_generated_with(&c, page, id, |text, epoch| {
                copied.borrow_mut().push((text.to_owned(), epoch));
                Ok(())
            })
        };
        copy(page, &second).unwrap();
        assert_eq!(
            *copied.borrow(),
            [("second-generated".to_owned(), c.lock_epoch())]
        );

        // Unknown ids: not_found, the clipboard is not touched.
        for unknown in ["", "no-such-entry"] {
            assert_eq!(
                reveal_for_page(&c, page, unknown).unwrap_err().code(),
                "not_found"
            );
            assert_eq!(copy(page, unknown).unwrap_err().code(), "not_found");
        }
        // Another page's vault or none: locked.
        for other in [None, Some("other-vault")] {
            assert_eq!(
                reveal_for_page(&c, other, &first).unwrap_err().code(),
                "locked"
            );
            assert_eq!(copy(other, &first).unwrap_err().code(), "locked");
        }
        // A failing clipboard is reported.
        let err = copy_generated_with(&c, page, &first, |_, _| Err(AppError::io("clipboard")));
        assert_eq!(err.unwrap_err().code(), "io:clipboard");
        assert_eq!(copied.borrow().len(), 1);

        // Clearing the history: the ids are gone.
        c.mutate_for_page(page, |v| v.clear_generator_history())
            .unwrap();
        assert_eq!(
            reveal_for_page(&c, page, &first).unwrap_err().code(),
            "not_found"
        );

        // Locked: the same answer, also for the page's own vault id.
        remember_for_page(&c, page, "third-generated").unwrap();
        let third = c.read(UnlockedVault::generator_history_info).unwrap()[0]
            .id
            .clone();
        c.lock(None);
        assert_eq!(
            reveal_for_page(&c, page, &third).unwrap_err().code(),
            "locked"
        );
        assert_eq!(copy(page, &third).unwrap_err().code(), "locked");
        assert_eq!(copied.borrow().len(), 1);
    }

    /// `copy_generated` reads the password and the lock epoch under the
    /// state lock and copies after releasing it. A lock (auto-lock, Win+L)
    /// that arrives while the read holds the state lock waits for it – and
    /// must not have started its epoch yet: otherwise the read gets the new
    /// epoch with the old vault's password, the lock's clipboard clear runs
    /// before the copy, and the copy passes `copy_to_clipboard_since`.
    #[test]
    fn a_copy_racing_a_lock_is_refused_or_cleared() {
        use std::sync::Barrier;
        use std::time::Duration;

        use crate::state::LockReason;

        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let vid = vault_id(&c);
        let page = Some(vid.as_str());
        remember_for_page(&c, page, "raced-generated").unwrap();
        let entry = c.read(UnlockedVault::generator_history_info).unwrap()[0]
            .id
            .clone();

        // Deterministic: the read holds the state lock while the lock waits.
        let before = c.lock_epoch();
        let (password, epoch) = {
            let st = c.state();
            let locker = {
                let c = Arc::clone(&c);
                std::thread::spawn(move || c.lock(Some(LockReason::Timeout)))
            };
            // Time for the lock to reach the state lock and wait there.
            std::thread::sleep(Duration::from_millis(200));
            assert!(!locker.is_finished());
            let read = (
                entry_password(st.page_vault(page).unwrap(), &entry).unwrap(),
                c.lock_epoch(),
            );
            drop(st);
            assert!(locker.join().unwrap(), "the lock closed the vault");
            read
        };
        assert_eq!(*password, "raced-generated");
        assert_eq!(epoch, before, "the lock started its epoch while waiting");
        assert_ne!(epoch, c.lock_epoch());
        // Refused before the clipboard is touched.
        assert_eq!(
            c.copy_to_clipboard_since(&password, true, Some(epoch))
                .unwrap_err()
                .code(),
            "locked"
        );

        // Racing for real: every password copy_generated read while the
        // vault was open carries an epoch older than the lock's – the copy
        // is refused if it comes after the lock, and cleared by the lock if
        // it came before.
        for _ in 0..40 {
            c.unlock(&vid, "master").unwrap();
            let start = Arc::new(Barrier::new(2));
            let copier = {
                let (c, start, vid, entry) = (
                    Arc::clone(&c),
                    Arc::clone(&start),
                    vid.clone(),
                    entry.clone(),
                );
                std::thread::spawn(move || {
                    let mut epochs = Vec::new();
                    start.wait();
                    loop {
                        let copied = copy_generated_with(&c, Some(&vid), &entry, |text, epoch| {
                            assert_eq!(text, "raced-generated");
                            epochs.push(epoch);
                            Ok(())
                        });
                        match copied {
                            Ok(()) => {}
                            Err(e) => {
                                assert_eq!(e.code(), "locked");
                                return epochs;
                            }
                        }
                    }
                })
            };
            start.wait();
            assert!(c.lock(Some(LockReason::Timeout)));
            let epochs = copier.join().unwrap();
            let after_lock = c.lock_epoch();
            assert!(
                epochs.iter().all(|&e| e < after_lock),
                "a copy read with the lock's epoch {after_lock}: {epochs:?}"
            );
        }
    }

    fn bank_options() -> GeneratorOptions {
        GeneratorOptions {
            length: 24,
            symbol_set: "-_-".into(),
            exclude: "xyz".into(),
            ..GeneratorOptions::default()
        }
    }

    #[test]
    fn user_presets_are_saved_renamed_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());

        let presets = GeneratorPresets::of(&c.state().settings);
        let builtin: Vec<&str> = presets.builtin.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(builtin, ["standard", "alnum", "pin", "long", "passphrase"]);
        assert!(presets.user.is_empty());
        assert_eq!(presets.default_id, PRESET_STANDARD);
        assert_eq!(presets.default_options, GeneratorOptions::default());

        // New: a fresh UUID, the name trimmed, the options normalised.
        let saved = save_preset(&c, None, "  Bank  ", bank_options()).unwrap();
        assert_eq!(saved.id.len(), 36);
        assert!(!core_gen::is_builtin_preset(&saved.id));
        let user = &saved.presets.user;
        assert_eq!(user.len(), 1);
        assert_eq!(
            (user[0].id.as_str(), user[0].name.as_str()),
            (saved.id.as_str(), "Bank")
        );
        assert_eq!(user[0].options.symbol_set, "-_");
        assert_eq!(user[0].options.length, 24);
        // Persisted.
        assert_eq!(stored_settings(&c).generator_presets, *user);
        assert_eq!(c.state().settings.generator_presets, *user);

        // Rename / update in place (a name differing only in case is the
        // preset's own).
        let pin_like = GeneratorOptions {
            length: 8,
            uppercase: false,
            lowercase: false,
            symbols: false,
            ..GeneratorOptions::default()
        };
        let renamed = save_preset(&c, Some(&saved.id), "BANK", pin_like.clone()).unwrap();
        assert_eq!(renamed.id, saved.id);
        assert_eq!(renamed.presets.user.len(), 1);
        assert_eq!(renamed.presets.user[0].name, "BANK");
        assert_eq!(renamed.presets.user[0].options, pin_like);
        let other = save_preset(&c, None, "Arbeit", bank_options()).unwrap();
        assert_eq!(other.presets.user.len(), 2);

        // Refused, nothing changes.
        let code = |r: AppResult<SavedGeneratorPreset>| r.unwrap_err().code();
        let short = GeneratorOptions {
            length: 2,
            ..GeneratorOptions::default()
        };
        assert_eq!(
            code(save_preset(&c, None, "bank", bank_options())),
            "invalid_input:preset_name_taken"
        );
        assert_eq!(
            code(save_preset(&c, Some(&other.id), " Bank ", bank_options())),
            "invalid_input:preset_name_taken"
        );
        assert_eq!(
            code(save_preset(&c, None, "   ", bank_options())),
            "invalid_input:preset_name_required"
        );
        assert_eq!(
            code(save_preset(&c, None, &"x".repeat(41), bank_options())),
            "invalid_input:preset_name_too_long"
        );
        assert_eq!(
            code(save_preset(&c, None, "Kurz", short)),
            "invalid_input:length"
        );
        let bad_symbols = GeneratorOptions {
            symbol_set: "a".into(),
            ..GeneratorOptions::default()
        };
        assert_eq!(
            code(save_preset(&c, None, "Kaputt", bad_symbols)),
            "invalid_input:symbol_set"
        );
        assert_eq!(
            code(save_preset(&c, Some(PRESET_PIN), "PIN", bank_options())),
            "invalid_input:preset_id"
        );
        assert_eq!(
            code(save_preset(&c, Some("gone"), "Weg", bank_options())),
            "not_found"
        );
        assert_eq!(stored_settings(&c).generator_presets.len(), 2);

        // Delete.
        let code = |r: AppResult<GeneratorPresets>| r.unwrap_err().code();
        assert_eq!(
            code(delete_preset(&c, PRESET_STANDARD)),
            "invalid_input:preset_id"
        );
        assert_eq!(code(delete_preset(&c, "gone")), "not_found");
        let left = delete_preset(&c, &other.id).unwrap();
        assert_eq!(left.user.len(), 1);
        assert_eq!(left.user[0].id, saved.id);
        assert_eq!(stored_settings(&c).generator_presets, left.user);
    }

    #[test]
    fn at_most_fifty_user_presets() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        for i in 0..GENERATOR_PRESETS_MAX {
            save_preset(&c, None, &format!("Preset {i}"), bank_options()).unwrap();
        }
        let err = save_preset(&c, None, "One more", bank_options()).unwrap_err();
        assert_eq!(err.code(), "invalid_input:presets_too_many");
        // Updating one still works.
        let first = c.state().settings.generator_presets[0].id.clone();
        save_preset(&c, Some(&first), "Renamed", bank_options()).unwrap();
        assert_eq!(
            c.state().settings.generator_presets.len(),
            GENERATOR_PRESETS_MAX
        );
    }

    #[test]
    fn the_default_preset_resolves_and_falls_back_to_standard() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());

        // A built-in default.
        let presets = set_default_preset(&c, PRESET_PIN).unwrap();
        assert_eq!(presets.default_id, PRESET_PIN);
        assert_eq!(
            presets.default_options,
            core_gen::builtin_preset(PRESET_PIN).unwrap().options
        );
        assert_eq!(stored_settings(&c).generator_default, PRESET_PIN);

        // A user preset as the default.
        let saved = save_preset(&c, None, "Bank", bank_options()).unwrap();
        let presets = set_default_preset(&c, &saved.id).unwrap();
        assert_eq!(presets.default_id, saved.id);
        assert_eq!(presets.default_options.length, 24);
        assert_eq!(presets.default_options.symbol_set, "-_");
        assert_eq!(
            c.state().settings.default_generator_options(),
            presets.default_options
        );
        // Editing it changes what the default generates.
        let passphrase = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            words: 6,
            ..GeneratorOptions::default()
        };
        let edited = save_preset(&c, Some(&saved.id), "Bank", passphrase.clone()).unwrap();
        assert_eq!(edited.presets.default_options, passphrase);

        // Unknown ids are refused, the default stays.
        assert_eq!(
            set_default_preset(&c, "gone").unwrap_err().code(),
            "not_found"
        );
        assert_eq!(c.state().settings.generator_default, saved.id);

        // Deleting the default falls back to "standard".
        let presets = delete_preset(&c, &saved.id).unwrap();
        assert_eq!(presets.default_id, PRESET_STANDARD);
        assert_eq!(presets.default_options, GeneratorOptions::default());
        let stored = stored_settings(&c);
        assert_eq!(stored.generator_default, PRESET_STANDARD);
        assert!(stored.generator_presets.is_empty());
    }
}
