//! Test fixtures: a temporary data directory with a vault (cheap KDF) and
//! a recording [`Platform`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use keystead_core::model::{CardData, IdentityData, ItemType, LoginUri, VaultItem};
use keystead_core::settings::{Language, Settings};
use keystead_core::{KdfParams, UnlockedVault, VaultStore};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use tempfile::TempDir;

use crate::app::App;
use crate::platform::Platform;
use crate::ui;

pub const PASSWORD: &str = "correct horse battery staple";
pub const TOTP_SEED: &str = "JBSWY3DPEHPK3PXP";
/// Fixed clock of the mock platform.
pub const NOW: u64 = 1_700_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    CopySecret(String, Option<Duration>),
    CopyText(String),
    ClearPending,
    OpenUrl(String),
}

pub type CallLog = Rc<RefCell<Vec<Call>>>;

pub struct MockPlatform {
    pub log: CallLog,
}

impl Platform for MockPlatform {
    fn copy_secret(
        &mut self,
        text: &str,
        clear_after: Option<Duration>,
    ) -> keystead_core::Result<()> {
        self.log
            .borrow_mut()
            .push(Call::CopySecret(text.to_owned(), clear_after));
        Ok(())
    }

    fn copy_text(&mut self, text: &str) -> keystead_core::Result<()> {
        self.log.borrow_mut().push(Call::CopyText(text.to_owned()));
        Ok(())
    }

    fn clear_pending_secret(&mut self) {
        self.log.borrow_mut().push(Call::ClearPending);
    }

    fn open_url(&mut self, url: &str) -> std::io::Result<()> {
        self.log.borrow_mut().push(Call::OpenUrl(url.to_owned()));
        Ok(())
    }

    fn unix_time(&self) -> u64 {
        NOW
    }
}

pub struct Fixture {
    pub dir: TempDir,
    pub vault_id: String,
    pub log: CallLog,
}

impl Fixture {
    pub fn store(&self) -> VaultStore {
        VaultStore::new(self.dir.path())
    }

    /// A second, independent handle on the vault (simulates another
    /// process such as the desktop app).
    pub fn other_process(&self) -> UnlockedVault {
        self.store()
            .unlock(&self.vault_id, PASSWORD)
            .expect("unlock fixture vault")
    }

    pub fn settings(&self) -> Settings {
        Settings::load_from(&self.dir.path().join("settings.json"))
    }

    pub fn calls(&self) -> Vec<Call> {
        self.log.borrow().clone()
    }

    pub fn app(&self) -> App {
        let platform = MockPlatform {
            log: self.log.clone(),
        };
        let mut app = App::new(self.store(), self.settings(), Box::new(platform), None);
        app.create_kdf = KdfParams::insecure_for_tests();
        app
    }
}

pub fn login(name: &str, user: &str, password: &str, uri: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Login, name);
    if let Some(l) = item.login.as_mut() {
        l.username = user.into();
        l.password = password.into();
        if !uri.is_empty() {
            l.uris.push(LoginUri {
                uri: uri.into(),
                ..Default::default()
            });
        }
    }
    item
}

fn sample_items() -> Vec<VaultItem> {
    let mut github = login(
        "GitHub",
        "octocat",
        "gh-secret-pw-123",
        "https://github.com",
    );
    if let Some(l) = github.login.as_mut() {
        l.totp = TOTP_SEED.into();
    }
    github.notes = "Recovery codes are in the safe".into();
    github.favorite = true;
    let amazon = login("Amazon", "me@example.com", "amz-pass", "amazon.de");
    let mut card = VaultItem::new(ItemType::Card, "Visa");
    card.card = Some(CardData {
        cardholder_name: "Max Muster".into(),
        brand: "Visa".into(),
        number: "4111 1111 1111 1234".into(),
        exp_month: "03".into(),
        exp_year: "2028".into(),
        code: "987".into(),
    });
    let mut note = VaultItem::new(ItemType::Note, "WLAN");
    note.notes = "SSID: home\nKey: wifi-secret".into();
    let mut ident = VaultItem::new(ItemType::Identity, "Ich");
    ident.identity = Some(IdentityData {
        first_name: "Max".into(),
        last_name: "Muster".into(),
        email: "max@example.com".into(),
        ..Default::default()
    });
    vec![github, amazon, card, note, ident]
}

/// Data dir with one vault "Privat" (sorted items: Amazon, GitHub, Ich,
/// Visa, WLAN) and settings in `lang`.
pub fn fixture(lang: Language) -> Fixture {
    fixture_with(lang, true)
}

pub fn fixture_with(lang: Language, with_items: bool) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = VaultStore::new(dir.path());
    let mut vault = store
        .create_vault_with_params("Privat", PASSWORD, KdfParams::insecure_for_tests())
        .expect("create vault");
    if with_items {
        for item in sample_items() {
            vault.save_item(item).expect("save item");
        }
    }
    let settings = Settings {
        language: lang,
        ..Settings::default()
    };
    settings
        .save_to(&dir.path().join("settings.json"))
        .expect("save settings");
    Fixture {
        vault_id: vault.id().to_owned(),
        dir,
        log: Rc::new(RefCell::new(Vec::new())),
    }
}

pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub fn ch(c: char) -> KeyEvent {
    let mods = if c.is_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    KeyEvent::new(KeyCode::Char(c), mods)
}

pub fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

pub fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(ch(c));
    }
}

/// Types the master password and runs the deferred unlock.
pub fn unlock(app: &mut App) {
    type_text(app, PASSWORD);
    app.handle_key(key(KeyCode::Enter));
    assert!(app.is_busy(), "unlock should be pending");
    app.run_pending();
}

/// Renders the app into a `width` × `height` test terminal and returns the
/// screen as text (one line per row).
pub fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
