//! State-machine tests: key handling, vault operations and side effects.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use keystead_core::settings::{Language, Settings};
use keystead_core::{totp, KdfParams, VaultStore};

use super::*;
use crate::test_support::*;

fn names(app: &App) -> Vec<String> {
    app.items.iter().map(|s| s.name.clone()).collect()
}

fn selected_name(app: &App) -> Option<String> {
    app.selected_summary().map(|s| s.name.clone())
}

fn unlocked(lang: Language) -> (Fixture, App) {
    let fx = fixture(lang);
    let mut app = fx.app();
    unlock(&mut app);
    assert!(matches!(app.screen, Screen::Main));
    (fx, app)
}

/// Selects the item with `name` via the search.
fn select(app: &mut App, name: &str) {
    app.handle_key(ch('/'));
    app.handle_key(ctrl('u'));
    type_text(app, name);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(selected_name(app).as_deref(), Some(name));
}

#[test]
fn unlock_flow_and_last_vault() {
    let fx = fixture(Language::De);
    let mut app = fx.app();
    assert!(matches!(app.screen, Screen::Unlock(_)));

    // Empty password: hint, nothing pending.
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.is_busy());
    match &app.screen {
        Screen::Unlock(u) => assert_eq!(u.error.as_deref(), Some("Bitte gib das Passwort ein.")),
        _ => panic!("expected unlock screen"),
    }

    // Wrong password: error, input cleared.
    type_text(&mut app, "wrong");
    app.handle_key(key(KeyCode::Enter));
    app.run_pending();
    match &app.screen {
        Screen::Unlock(u) => {
            assert!(u.error.as_deref().is_some_and(|e| e.contains("Falsches")));
            assert!(u.password.is_empty());
        }
        _ => panic!("expected unlock screen"),
    }

    // Typing clears the error; the right password unlocks.
    unlock(&mut app);
    assert!(matches!(app.screen, Screen::Main));
    assert_eq!(names(&app), ["Amazon", "GitHub", "Ich", "Visa", "WLAN"]);
    assert_eq!(selected_name(&app).as_deref(), Some("Amazon"));
    assert_eq!(
        fx.settings().last_vault_id.as_deref(),
        Some(fx.vault_id.as_str())
    );
}

#[test]
fn picker_for_several_vaults_preselects_last_used() {
    let fx = fixture(Language::En);
    let second = fx
        .store()
        .create_vault_with_params("Arbeit", PASSWORD, KdfParams::insecure_for_tests())
        .unwrap();
    let mut settings = fx.settings();
    settings.last_vault_id = Some(fx.vault_id.clone());
    settings
        .save_to(&fx.dir.path().join("settings.json"))
        .unwrap();

    let mut app = fx.app();
    // Sorted by name: Arbeit, Privat -> the last used (Privat) is preselected.
    assert!(matches!(app.screen, Screen::VaultPicker { selected: 1 }));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Enter));
    match &app.screen {
        Screen::Unlock(u) => assert_eq!(u.vault.id, second.id()),
        _ => panic!("expected unlock screen"),
    }
    // Esc goes back to the picker with that vault selected.
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.screen, Screen::VaultPicker { selected: 0 }));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    unlock(&mut app);
    assert_eq!(app.vault_name(), Some("Privat"));
    // q on the picker quits.
    let mut app = fx.app();
    app.handle_key(ch('q'));
    assert!(app.should_quit());
}

#[test]
fn vault_hint_selects_vault() {
    let fx = fixture(Language::En);
    let app = App::new(
        fx.store(),
        fx.settings(),
        Box::new(MockPlatform {
            log: fx.log.clone(),
        }),
        Some("PRIVAT"),
    );
    assert!(matches!(&app.screen, Screen::Unlock(u) if u.vault.name == "Privat"));
    let app = App::new(
        fx.store(),
        fx.settings(),
        Box::new(MockPlatform {
            log: fx.log.clone(),
        }),
        Some("missing"),
    );
    assert!(app
        .status
        .as_ref()
        .is_some_and(|s| s.kind == StatusKind::Error));
}

#[test]
fn first_run_creates_vault() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings {
        language: Language::En,
        ..Settings::default()
    };
    let log = CallLog::default();
    let mut app = App::new(
        VaultStore::new(dir.path()),
        settings,
        Box::new(MockPlatform { log }),
        None,
    );
    app.create_kdf = KdfParams::insecure_for_tests();
    let Screen::CreateVault(f) = &app.screen else {
        panic!("expected create vault screen");
    };
    assert!(f.first_run);
    assert_eq!(f.focus, 1, "password field focused, default name prefilled");

    type_text(&mut app, "short");
    app.handle_key(key(KeyCode::Enter)); // -> confirmation field
    app.handle_key(key(KeyCode::Enter)); // submit
    assert!(!app.is_busy());
    assert!(
        matches!(&app.screen, Screen::CreateVault(f) if f.error.as_deref().is_some_and(|e| e.contains('8')))
    );

    // Focus is back on the password: replace it.
    app.handle_key(ctrl('u'));
    type_text(&mut app, PASSWORD);
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, PASSWORD);
    app.handle_key(key(KeyCode::Enter));
    assert!(app.is_busy());
    app.run_pending();
    assert!(matches!(app.screen, Screen::Main));
    assert_eq!(app.vault_name(), Some("Personal"));
    assert_eq!(app.vaults.len(), 1);
    assert!(app.items.is_empty());
    // Esc on the empty first-run form quits.
    let mut app = App::new(
        VaultStore::new(tempfile::tempdir().unwrap().path()),
        Settings::default(),
        Box::new(MockPlatform {
            log: CallLog::default(),
        }),
        None,
    );
    app.handle_key(key(KeyCode::Esc));
    assert!(app.should_quit());
}

#[test]
fn navigation_keys() {
    let (_fx, mut app) = unlocked(Language::De);
    app.page_size = 2;
    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected_name(&app).as_deref(), Some("GitHub"));
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(selected_name(&app).as_deref(), Some("Visa"));
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(selected_name(&app).as_deref(), Some("WLAN"));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected_name(&app).as_deref(), Some("WLAN"));
    app.handle_key(key(KeyCode::Home));
    assert_eq!(selected_name(&app).as_deref(), Some("Amazon"));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(selected_name(&app).as_deref(), Some("Amazon"));
    app.handle_key(key(KeyCode::End));
    assert_eq!(selected_name(&app).as_deref(), Some("WLAN"));
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(selected_name(&app).as_deref(), Some("Ich"));

    // Enter focuses the details, where arrows scroll instead of selecting.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Detail);
    app.detail_max_scroll = 3;
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.detail_scroll, 2);
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.detail_scroll, 3);
    assert_eq!(selected_name(&app).as_deref(), Some("Ich"));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::List);
    assert!(!app.should_quit());
}

#[test]
fn search_by_slash_and_by_typing() {
    let (_fx, mut app) = unlocked(Language::De);
    // "a" is not a shortcut: typing starts the search.
    type_text(&mut app, "ama");
    assert_eq!(app.focus, Focus::Filter);
    assert_eq!(app.filter.value(), "ama");
    assert_eq!(names(&app), ["Amazon"]);
    // In the search field shortcut letters are text.
    type_text(&mut app, "zon q");
    assert_eq!(app.filter.value(), "amazon q");
    assert!(app.items.is_empty());
    assert!(!app.should_quit());
    // Esc clears the search.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::List);
    assert!(app.filter.is_empty());
    assert_eq!(app.items.len(), 5);

    // "/" + text, Enter keeps the filter and returns to shortcuts.
    app.handle_key(ch('/'));
    type_text(&mut app, "octocat");
    assert_eq!(names(&app), ["GitHub"]);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::List);
    app.handle_key(ch('u'));
    assert_eq!(
        app.status.as_ref().map(|s| s.kind),
        Some(StatusKind::Success)
    );
    // Esc in the list first clears the filter, then quits.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.filter.is_empty() && !app.should_quit());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.should_quit());
}

#[test]
fn copy_actions_use_clipboard_settings() {
    let (fx, mut app) = unlocked(Language::De);
    select(&mut app, "GitHub");
    app.handle_key(ch('p'));
    app.handle_key(ch('u'));
    app.handle_key(ch('t'));
    let code = totp::totp_at(TOTP_SEED, NOW).unwrap().code;
    let clear = Some(Duration::from_secs(30));
    assert_eq!(
        fx.calls(),
        vec![
            Call::CopySecret("gh-secret-pw-123".into(), clear),
            Call::CopyText("octocat".into()),
            Call::CopySecret(code, clear),
        ]
    );
    let status = app.status.clone().unwrap();
    assert_eq!(
        status.text,
        "Code kopiert – wird in 30 s aus der Zwischenablage gelöscht"
    );

    // A note has no password, username or TOTP: nothing is copied.
    fx.log.borrow_mut().clear();
    select(&mut app, "WLAN");
    for c in ['p', 'u', 't', 'o'] {
        app.handle_key(ch(c));
    }
    assert!(fx.calls().is_empty());
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("Für dieses Element ist keine Website hinterlegt.")
    );

    // Cards: p copies the number, u the cardholder.
    select(&mut app, "Visa");
    app.handle_key(ch('p'));
    app.handle_key(ch('u'));
    assert_eq!(
        fx.calls(),
        vec![
            Call::CopySecret("4111 1111 1111 1234".into(), clear),
            Call::CopyText("Max Muster".into()),
        ]
    );

    // clipboardClearSeconds = 0: no automatic clearing.
    fx.log.borrow_mut().clear();
    app.settings.clipboard_clear_seconds = 0;
    select(&mut app, "Amazon");
    app.handle_key(ch('p'));
    assert_eq!(fx.calls(), vec![Call::CopySecret("amz-pass".into(), None)]);
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("Passwort kopiert")
    );
}

#[test]
fn open_url_adds_scheme() {
    let (fx, mut app) = unlocked(Language::En);
    select(&mut app, "Amazon");
    app.handle_key(ch('o'));
    select(&mut app, "GitHub");
    app.handle_key(ch('o'));
    assert_eq!(
        fx.calls(),
        vec![
            Call::OpenUrl("https://amazon.de".into()),
            Call::OpenUrl("https://github.com".into()),
        ]
    );
}

#[test]
fn toggle_secrets_and_help_overlay() {
    let (_fx, mut app) = unlocked(Language::En);
    assert!(!app.show_secrets);
    app.handle_key(ch('s'));
    assert!(app.show_secrets);
    app.handle_key(ch('?'));
    assert_eq!(app.overlay, Some(Overlay::Help));
    // Any key closes the help without triggering its shortcut.
    app.handle_key(ch('s'));
    assert_eq!(app.overlay, None);
    assert!(app.show_secrets);
    app.handle_key(key(KeyCode::F(1)));
    assert_eq!(app.overlay, Some(Overlay::Help));
}

#[test]
fn new_login_with_generated_password() {
    let (fx, mut app) = unlocked(Language::De);
    app.handle_key(ch('n'));
    assert!(matches!(app.screen, Screen::Form(_)));
    type_text(&mut app, "Neue Seite");
    app.handle_key(key(KeyCode::Enter)); // -> website
    type_text(&mut app, "neu.example");
    app.handle_key(key(KeyCode::Tab)); // -> username
    type_text(&mut app, "ich");
    app.handle_key(key(KeyCode::Down)); // -> password
    app.handle_key(ctrl('g'));
    let generated = match &app.screen {
        Screen::Form(f) => f.fields[3].input.value().to_owned(),
        _ => panic!("expected form"),
    };
    assert_eq!(generated.chars().count(), 20);
    app.handle_key(key(KeyCode::Down)); // -> totp
    app.handle_key(key(KeyCode::Down)); // -> notes (multi-line)
    type_text(&mut app, "Zeile 1");
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "Zeile 2");
    app.handle_key(ctrl('s'));

    assert!(matches!(app.screen, Screen::Main));
    assert_eq!(selected_name(&app).as_deref(), Some("Neue Seite"));
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("Element erstellt")
    );
    let other = fx.other_process();
    let saved = other
        .items()
        .iter()
        .find(|i| i.name == "Neue Seite")
        .expect("saved to disk");
    let login = saved.login.as_ref().unwrap();
    assert_eq!(login.username, "ich");
    assert_eq!(login.password, generated);
    assert_eq!(login.uris[0].uri, "neu.example");
    assert_eq!(saved.notes, "Zeile 1\nZeile 2");
}

#[test]
fn new_login_requires_name_and_save_button_works() {
    let (_fx, mut app) = unlocked(Language::En);
    app.handle_key(ch('n'));
    app.handle_key(ctrl('s'));
    match &app.screen {
        Screen::Form(f) => {
            assert_eq!(f.error.as_deref(), Some("Please enter a name."));
            assert_eq!(f.focus, 0);
        }
        _ => panic!("form must stay open"),
    }
    type_text(&mut app, "Via button");
    // Shift+Tab from the name wraps to "Cancel", once more to "Save".
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.screen, Screen::Main));
    assert_eq!(selected_name(&app).as_deref(), Some("Via button"));
}

#[test]
fn edit_and_discard_confirmation() {
    let (fx, mut app) = unlocked(Language::De);
    select(&mut app, "GitHub");
    // Unchanged form: Esc closes immediately.
    app.handle_key(ch('e'));
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.screen, Screen::Main));

    // Changed form: Esc asks first.
    app.handle_key(ch('e'));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab)); // username
    app.handle_key(ctrl('u'));
    type_text(&mut app, "hubot");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Some(Overlay::ConfirmDiscard));
    app.handle_key(ch('n'));
    assert!(matches!(app.screen, Screen::Form(_)));
    assert_eq!(app.overlay, None);
    app.handle_key(ctrl('s'));
    assert!(matches!(app.screen, Screen::Main));
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("Änderungen gespeichert")
    );
    let other = fx.other_process();
    let github = other.items().iter().find(|i| i.name == "GitHub").unwrap();
    assert_eq!(github.username(), "hubot");
    assert_eq!(github.password(), "gh-secret-pw-123");
    assert_eq!(github.login.as_ref().unwrap().totp, TOTP_SEED);

    // Discarding keeps the stored item.
    app.handle_key(ch('e'));
    type_text(&mut app, " (alt)");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('j'));
    assert!(matches!(app.screen, Screen::Main));
    assert_eq!(selected_name(&app).as_deref(), Some("GitHub"));
}

#[test]
fn trash_with_confirmation() {
    let (fx, mut app) = unlocked(Language::De);
    select(&mut app, "Amazon");
    app.handle_key(key(KeyCode::Esc)); // clear search
    app.handle_key(ch('d'));
    assert!(matches!(&app.overlay, Some(Overlay::ConfirmTrash { name, .. }) if name == "Amazon"));
    // Unrelated keys keep the question open, "n" cancels.
    app.handle_key(ch('x'));
    assert!(app.overlay.is_some());
    app.handle_key(ch('n'));
    assert_eq!(app.items.len(), 5);

    app.handle_key(ch('d'));
    app.handle_key(ch('j'));
    assert_eq!(names(&app), ["GitHub", "Ich", "Visa", "WLAN"]);
    assert_eq!(selected_name(&app).as_deref(), Some("GitHub"));
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("„Amazon“ in den Papierkorb verschoben")
    );
    let other = fx.other_process();
    assert!(other
        .items()
        .iter()
        .any(|i| i.name == "Amazon" && i.is_trashed()));
}

#[test]
fn conflict_is_resolved_by_reload_and_retry() {
    let (fx, mut app) = unlocked(Language::En);
    // Another process saves in the meantime.
    let mut other = fx.other_process();
    other.save_item(login("From app", "x", "y", "")).unwrap();

    select(&mut app, "WLAN");
    app.handle_key(ch('d'));
    app.handle_key(ch('y'));
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("The vault had been changed elsewhere – reloaded and saved again.")
    );
    app.handle_key(key(KeyCode::Esc));
    assert!(names(&app).contains(&"From app".to_owned()));
    assert!(!names(&app).contains(&"WLAN".to_owned()));
    let check = fx.other_process();
    assert!(check.items().iter().any(|i| i.name == "From app"));
    assert!(check
        .items()
        .iter()
        .any(|i| i.name == "WLAN" && i.is_trashed()));

    // The same for saving an edited item.
    other.reload_if_changed().unwrap();
    other.save_item(login("Second", "", "", "")).unwrap();
    select(&mut app, "GitHub");
    app.handle_key(ch('e'));
    type_text(&mut app, "!");
    app.handle_key(ctrl('s'));
    assert!(matches!(app.screen, Screen::Main));
    let check = fx.other_process();
    assert!(check.items().iter().any(|i| i.name == "GitHub!"));
    assert!(check.items().iter().any(|i| i.name == "Second"));
}

#[test]
fn external_changes_are_picked_up() {
    let (fx, mut app) = unlocked(Language::En);
    let mut other = fx.other_process();
    other
        .save_item(login("Added elsewhere", "", "", ""))
        .unwrap();
    app.tick(Instant::now());
    assert_eq!(app.items.len(), 5, "not before the refresh interval");
    app.tick(Instant::now() + Duration::from_secs(6));
    assert_eq!(app.items.len(), 6);
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("The vault was changed elsewhere and has been reloaded.")
    );
}

#[test]
fn manual_lock_clears_clipboard_and_secrets() {
    let (fx, mut app) = unlocked(Language::De);
    app.handle_key(ch('s'));
    app.handle_key(ch('g'));
    assert!(!app.generator.value.is_empty());
    app.handle_key(ctrl('l'));
    assert!(matches!(&app.screen, Screen::Unlock(u) if u.vault.name == "Privat"));
    assert!(app.vault_name().is_none());
    assert!(app.items.is_empty());
    assert!(!app.show_secrets);
    assert!(app.generator.value.is_empty());
    assert_eq!(fx.calls(), vec![Call::ClearPending]);
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("Tresor gesperrt.")
    );
    // The lock message stays until something else happens.
    app.tick(Instant::now() + Duration::from_secs(3600));
    assert!(app.status.is_some());
    unlock(&mut app);
    assert!(matches!(app.screen, Screen::Main));
}

#[test]
fn auto_lock_after_inactivity() {
    let (fx, mut app) = unlocked(Language::De);
    app.tick(Instant::now() + Duration::from_secs(14 * 60));
    assert!(matches!(app.screen, Screen::Main));
    app.tick(Instant::now() + Duration::from_secs(15 * 60 + 1));
    assert!(matches!(app.screen, Screen::Unlock(_)));
    assert_eq!(
        app.status.as_ref().map(|s| s.text.as_str()),
        Some("Tresor nach 15 min Inaktivität gesperrt.")
    );
    assert_eq!(fx.calls(), vec![Call::ClearPending]);

    // 0 = never.
    unlock(&mut app);
    app.settings.auto_lock_minutes = 0;
    app.tick(Instant::now() + Duration::from_secs(24 * 3600));
    assert!(matches!(app.screen, Screen::Main));
}

#[test]
fn generator_screen() {
    let (fx, mut app) = unlocked(Language::En);
    app.handle_key(ch('g'));
    assert!(matches!(app.screen, Screen::Generator));
    assert_eq!(app.generator.value.chars().count(), 20);
    app.handle_key(ch('+'));
    app.handle_key(ch('+'));
    app.handle_key(ch('-'));
    assert_eq!(app.generator.value.chars().count(), 21);
    // Mode row: Space switches to passphrases.
    app.handle_key(ch(' '));
    assert_eq!(
        app.generator.options.kind,
        keystead_core::generator::GeneratorKind::Passphrase
    );
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.generator.options.words, 6);
    // Separator row: cycle to a space and count the words.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(ch(' '));
    assert_eq!(app.generator.value.split(' ').count(), 6);
    let value = app.generator.value.to_string();
    app.handle_key(ch('c'));
    assert_eq!(
        fx.calls(),
        vec![Call::CopySecret(
            value.clone(),
            Some(Duration::from_secs(30))
        )]
    );
    let other = fx.other_process();
    assert_eq!(other.generator_history()[0].password, value);
    app.handle_key(key(KeyCode::Enter));
    assert_ne!(app.generator.value.as_str(), value);
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.screen, Screen::Main));
    assert!(app.generator.value.is_empty());
}

#[test]
fn quit_paths_and_ignored_events() {
    let (_fx, mut app) = unlocked(Language::De);
    // Key releases (reported on Windows) are ignored.
    let mut release = ch('q');
    release.kind = KeyEventKind::Release;
    app.handle_event(Event::Key(release));
    assert!(!app.should_quit());
    app.handle_event(Event::Key(ch('q')));
    assert!(app.should_quit());

    // Ctrl+C quits from a form too, where "q" is text.
    let (_fx, mut app) = unlocked(Language::De);
    app.handle_key(ch('n'));
    app.handle_key(ch('q'));
    assert!(!app.should_quit());
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit());
}

#[test]
fn paste_goes_into_focused_input() {
    let fx = fixture(Language::En);
    let mut app = fx.app();
    app.handle_event(Event::Paste(format!("{PASSWORD}\n")));
    app.handle_key(key(KeyCode::Enter));
    app.run_pending();
    assert!(matches!(app.screen, Screen::Main));
    app.handle_paste("visa");
    assert_eq!(app.focus, Focus::Filter);
    assert_eq!(names(&app), ["Visa"]);
}

#[test]
fn altgr_characters_are_typed() {
    let fx = fixture(Language::De);
    let dir = fx.dir.path().to_owned();
    // A vault whose password needs AltGr on a German keyboard.
    let store = VaultStore::new(&dir);
    store
        .create_vault_with_params("Zweit", "a@b€c", KdfParams::insecure_for_tests())
        .unwrap();
    let mut app = fx.app();
    app.handle_key(key(KeyCode::Home)); // picker: "Privat" vs "Zweit"
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Enter));
    let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
    app.handle_key(ch('a'));
    app.handle_key(KeyEvent::new(KeyCode::Char('@'), altgr));
    app.handle_key(ch('b'));
    app.handle_key(KeyEvent::new(KeyCode::Char('€'), altgr));
    app.handle_key(ch('c'));
    app.handle_key(key(KeyCode::Enter));
    app.run_pending();
    assert_eq!(app.vault_name(), Some("Zweit"));
}
