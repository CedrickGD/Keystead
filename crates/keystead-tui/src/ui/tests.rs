//! Rendering tests with ratatui's `TestBackend`.

use keystead_core::settings::Language;
use keystead_core::totp;
use ratatui::crossterm::event::KeyCode;

use crate::app::{App, Overlay, Screen};
use crate::test_support::*;

fn unlocked(lang: Language) -> (Fixture, App) {
    let fx = fixture(lang);
    let mut app = fx.app();
    unlock(&mut app);
    (fx, app)
}

fn select(app: &mut App, name: &str) {
    app.handle_key(ch('/'));
    app.handle_key(ctrl('u'));
    type_text(app, name);
    app.handle_key(key(KeyCode::Enter));
}

#[test]
fn unlock_screen_masks_the_password() {
    let fx = fixture(Language::De);
    let mut app = fx.app();
    let screen = render(&mut app, 80, 24);
    assert!(screen.contains("Tresor entsperren"), "{screen}");
    assert!(screen.contains("Tresor: Privat"));
    assert!(screen.contains("Master-Passwort"));
    assert!(screen.contains("Entsperren"));
    assert!(screen.contains("Esc"));

    type_text(&mut app, "geheim");
    let screen = render(&mut app, 80, 24);
    assert!(screen.contains("••••••"));
    assert!(!screen.contains("geheim"));

    app.handle_key(key(KeyCode::Enter));
    let screen = render(&mut app, 80, 24);
    assert!(screen.contains("Entsperre …"), "busy frame: {screen}");
    app.run_pending();
    let screen = render(&mut app, 80, 24);
    assert!(screen.contains("Falsches Master-Passwort"));
}

#[test]
fn unlock_screen_in_english() {
    let fx = fixture(Language::En);
    let mut app = fx.app();
    let screen = render(&mut app, 80, 24);
    assert!(screen.contains("Unlock vault"), "{screen}");
    assert!(screen.contains("Master password"));
    assert!(screen.contains("Quit"));
}

#[test]
fn main_screen_lists_items_and_masks_secrets() {
    let (_fx, mut app) = unlocked(Language::De);
    select(&mut app, "GitHub");
    app.handle_key(key(KeyCode::Esc)); // clear the search, keep the selection
    let screen = render(&mut app, 110, 30);

    // Header, list and details.
    assert!(screen.contains(" Keystead "), "{screen}");
    assert!(screen.contains("5 Elemente"));
    for name in ["Amazon", "GitHub *", "Ich", "Visa", "WLAN"] {
        assert!(screen.contains(name), "{name} missing:\n{screen}");
    }
    assert!(screen.contains("> L GitHub"), "selection marker:\n{screen}");
    assert!(screen.contains("me@example.com"));
    assert!(screen.contains("Benutzername"));
    assert!(screen.contains("octocat"));
    assert!(screen.contains("••••••••"));
    assert!(!screen.contains("gh-secret-pw-123"));
    assert!(screen.contains("https://github.com"));
    assert!(screen.contains("Recovery codes are in the safe"));
    assert!(screen.contains("Login · * Favorit"));

    // Live TOTP code with the remaining seconds.
    let code = totp::totp_at(TOTP_SEED, NOW).unwrap();
    let grouped = format!("{} {}", &code.code[..3], &code.code[3..]);
    assert!(screen.contains(&grouped), "TOTP missing:\n{screen}");
    assert!(screen.contains(&format!("noch {} s", code.remaining)));

    // Key hints.
    assert!(screen.contains("Hilfe"));
    assert!(screen.contains("Suchen"));

    // "s" reveals the secrets.
    app.handle_key(ch('s'));
    let screen = render(&mut app, 110, 30);
    assert!(screen.contains("gh-secret-pw-123"), "{screen}");
    assert!(screen.contains("Geheime Felder werden angezeigt"));
}

#[test]
fn main_screen_other_item_types() {
    let (_fx, mut app) = unlocked(Language::En);
    select(&mut app, "Visa");
    let screen = render(&mut app, 110, 30);
    assert!(screen.contains("Cardholder"), "{screen}");
    assert!(screen.contains("Max Muster"));
    assert!(screen.contains("•••• 1234"));
    assert!(!screen.contains("4111"));
    assert!(screen.contains("03/2028"));
    assert!(!screen.contains("987"));

    select(&mut app, "WLAN");
    let screen = render(&mut app, 110, 30);
    assert!(screen.contains("Secure note"));
    assert!(screen.contains("SSID: home"));
    assert!(screen.contains("Key: wifi-secret"));

    select(&mut app, "Ich");
    let screen = render(&mut app, 110, 30);
    assert!(screen.contains("max@example.com"));
    assert!(screen.contains("Identity"));
}

#[test]
fn search_line_and_empty_result() {
    let (_fx, mut app) = unlocked(Language::En);
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("Search: Type to search"), "{screen}");
    type_text(&mut app, "zzz");
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("Search: zzz"));
    assert!(screen.contains("No matches."));
    assert!(screen.contains("0 of 5"));
}

#[test]
fn narrow_terminal_shows_one_pane() {
    let (_fx, mut app) = unlocked(Language::En);
    let screen = render(&mut app, 60, 20);
    assert!(screen.contains("Items"));
    assert!(!screen.contains("Details"));
    app.handle_key(key(KeyCode::Enter));
    let screen = render(&mut app, 60, 20);
    assert!(screen.contains("Details"), "{screen}");
    assert!(screen.contains("me@example.com"));
}

#[test]
fn overlays() {
    let (_fx, mut app) = unlocked(Language::De);
    app.handle_key(ch('?'));
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("Tastenkürzel"), "{screen}");
    assert!(screen.contains("Passwort kopieren"));
    assert!(screen.contains("nach 30 s"));
    assert!(screen.contains("15 min"));
    app.handle_key(key(KeyCode::Esc));

    app.handle_key(ch('d'));
    assert!(matches!(app.overlay, Some(Overlay::ConfirmTrash { .. })));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("„Amazon“ in den Papierkorb verschieben?"),
        "{screen}"
    );
    assert!(screen.contains(" j  Ja"));
}

#[test]
fn form_and_generator_screens() {
    let (_fx, mut app) = unlocked(Language::De);
    app.handle_key(ch('n'));
    type_text(&mut app, "Bank");
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("Neues Login"), "{screen}");
    for label in [
        "Name",
        "Website",
        "Benutzername",
        "Passwort",
        "2FA-Schlüssel",
        "Notizen",
    ] {
        assert!(screen.contains(label), "{label} missing:\n{screen}");
    }
    assert!(screen.contains("> Name"));
    assert!(screen.contains("Bank"));
    assert!(screen.contains("[ Speichern ]"));
    assert!(screen.contains("Strg+S"));
    assert!(screen.contains("Strg+G"));

    // Generated passwords are masked until Ctrl+R.
    app.handle_key(ctrl('g'));
    let password = match &app.screen {
        Screen::Form(f) => f.fields[3].input.value().to_owned(),
        _ => panic!("expected form"),
    };
    let screen = render(&mut app, 100, 30);
    assert!(!screen.contains(&password));
    app.handle_key(ctrl('r'));
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains(&password), "{screen}");

    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('j'));
    app.handle_key(ch('g'));
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("Passwort-Generator"), "{screen}");
    assert!(screen.contains(app.generator.value.as_str()));
    assert!(screen.contains("Stärke"));
    assert!(screen.contains("< 20 >"));
    assert!(screen.contains("[x]"));
    assert!(screen.contains("Kopieren"));
}

#[test]
fn create_vault_and_picker_screens() {
    let dir = tempfile::tempdir().unwrap();
    let settings = keystead_core::settings::Settings {
        language: Language::De,
        ..Default::default()
    };
    let mut app = App::new(
        keystead_core::VaultStore::new(dir.path()),
        settings,
        Box::new(MockPlatform {
            log: CallLog::default(),
        }),
        None,
    );
    let screen = render(&mut app, 90, 30);
    assert!(screen.contains("Neuen Tresor erstellen"), "{screen}");
    assert!(screen.contains("Noch kein Tresor vorhanden"));
    assert!(screen.contains("Privat"));
    type_text(&mut app, "abc");
    let screen = render(&mut app, 90, 30);
    assert!(screen.contains("Stärke: Sehr schwach"), "{screen}");

    let fx = fixture(Language::En);
    fx.store()
        .create_vault_with_params(
            "Work",
            PASSWORD,
            keystead_core::KdfParams::insecure_for_tests(),
        )
        .unwrap();
    let mut app = fx.app();
    let screen = render(&mut app, 90, 30);
    assert!(screen.contains("Choose a vault"), "{screen}");
    assert!(screen.contains("> Privat"));
    assert!(screen.contains("Work"));
}

/// Brings the app into the state to render.
type Setup = Box<dyn Fn(&mut App)>;

#[test]
fn rendering_never_panics_at_any_size() {
    let (_fx, mut app) = unlocked(Language::De);
    let mut states: Vec<Setup> = vec![
        Box::new(|_| {}),
        Box::new(|a| a.handle_key(key(KeyCode::Enter))),
        Box::new(|a| a.handle_key(ch('?'))),
        Box::new(|a| {
            a.handle_key(key(KeyCode::Esc));
            a.handle_key(ch('d'));
        }),
        Box::new(|a| {
            a.handle_key(key(KeyCode::Esc));
            a.handle_key(ch('n'));
        }),
        Box::new(|a| {
            a.handle_key(key(KeyCode::Esc));
            a.handle_key(ch('j'));
            a.handle_key(ch('g'));
        }),
        Box::new(|a| a.handle_key(ctrl('l'))),
    ];
    for setup in states.drain(..) {
        setup(&mut app);
        for w in [1u16, 2, 10, 29, 30, 31, 45, 71, 72, 73, 120] {
            for h in [1u16, 3, 7, 8, 9, 12, 24, 50] {
                render(&mut app, w, h);
            }
        }
    }
    let screen = render(&mut app, 20, 5);
    assert!(screen.contains("Fenster zu klein"));
}

#[test]
fn key_hints_follow_the_selected_item_and_wrap() {
    let (_fx, mut app) = unlocked(Language::En);
    select(&mut app, "GitHub");
    app.handle_key(key(KeyCode::Esc));
    // Everything fits into two lines even at 80 columns (German is longest).
    let screen = render(&mut app, 80, 24);
    let footer: String = screen.lines().rev().take(2).collect::<Vec<_>>().join("\n");
    for hint in [
        " ? ",
        "Help",
        "Password",
        "User",
        " t ",
        "Code",
        "Open",
        "New",
        "Edit",
        "Delete",
        "Generator",
        "Ctrl+L",
        "Lock",
        " q ",
        "Quit",
    ] {
        assert!(footer.contains(hint), "{hint} missing:\n{footer}");
    }

    // A secure note offers neither password, username, code nor website.
    select(&mut app, "WLAN");
    let screen = render(&mut app, 120, 30);
    let footer: String = screen.lines().rev().take(2).collect::<Vec<_>>().join("\n");
    for absent in ["Password", "User", "Code", "Open"] {
        assert!(
            !footer.contains(absent),
            "{absent} shown for a note:\n{footer}"
        );
    }
    assert!(footer.contains("Edit") && footer.contains("Quit"));

    let (_fx, mut app) = unlocked(Language::De);
    select(&mut app, "GitHub");
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 80, 24);
    let footer: String = screen.lines().rev().take(2).collect::<Vec<_>>().join("\n");
    for hint in [
        "Hilfe",
        "Passwort",
        "Code",
        "Bearbeiten",
        "Strg+L",
        "Sperren",
        "Beenden",
    ] {
        assert!(footer.contains(hint), "{hint} missing:\n{footer}");
    }
}
