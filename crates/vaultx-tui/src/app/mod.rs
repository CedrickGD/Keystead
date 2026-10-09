//! Interactive application state and key handling.
//!
//! [`App`] is a pure state machine: it never touches the terminal. The
//! event loop in `term.rs` feeds it events, calls [`App::tick`] regularly,
//! runs slow work ([`App::run_pending`], e.g. the key derivation) after a
//! frame was drawn and renders it with `ui::draw`. OS side effects go
//! through [`Platform`].

pub mod form;
pub mod generator;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use vaultx_core::model::{ItemSummary, ItemType, VaultInfo, VaultItem};
use vaultx_core::settings::Settings;
use vaultx_core::{totp, Error, KdfParams, UnlockedVault, VaultStore};
use zeroize::Zeroizing;

use crate::i18n::{Lang, M};
use crate::input::{text_char, InputResult, TextInput};
use crate::platform::{browser_url, Platform};
use form::{CreateVaultForm, FormFocus, ItemForm};
use generator::GeneratorState;

/// Minimum length of a new master password (same as the desktop app).
pub const MIN_MASTER_LEN: usize = 8;
/// How often the vault file is checked for changes by other processes.
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const STATUS_TTL: Duration = Duration::from_secs(8);
const ERROR_TTL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Error,
}

/// Message shown in the status line.
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub kind: StatusKind,
    at: Instant,
    /// `None` = stays until replaced.
    ttl: Option<Duration>,
}

#[derive(Debug)]
pub struct UnlockState {
    pub vault: VaultInfo,
    pub password: TextInput,
    pub error: Option<String>,
}

impl UnlockState {
    fn new(vault: VaultInfo) -> Self {
        UnlockState {
            vault,
            password: TextInput::new(),
            error: None,
        }
    }
}

#[derive(Debug)]
pub enum Screen {
    /// Choose one of several vaults (`selected` indexes `App::vaults`).
    VaultPicker {
        selected: usize,
    },
    CreateVault(CreateVaultForm),
    Unlock(UnlockState),
    /// Item list + details.
    Main,
    Form(Box<ItemForm>),
    Generator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    ConfirmTrash { id: String, name: String },
    ConfirmDiscard,
}

/// Keyboard focus on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Filter,
    Detail,
}

/// Slow work deferred until the "please wait" frame has been drawn.
enum Pending {
    Unlock {
        vault_id: String,
        password: Zeroizing<String>,
    },
    CreateVault {
        name: String,
        password: Zeroizing<String>,
    },
}

/// What the selected item offers (drives the key hints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemActions {
    pub username: bool,
    pub password: bool,
    pub totp: bool,
    pub url: bool,
}

/// Username-like value copied with `u`: login username, identity username
/// (or e-mail), cardholder.
pub(crate) fn username_of(item: &VaultItem) -> String {
    match item.item_type {
        ItemType::Login => item.username().to_owned(),
        ItemType::Identity => item.identity.as_ref().map_or_else(String::new, |i| {
            if i.username.trim().is_empty() {
                i.email.clone()
            } else {
                i.username.clone()
            }
        }),
        ItemType::Card => item
            .card
            .as_ref()
            .map_or_else(String::new, |c| c.cardholder_name.clone()),
        ItemType::Note => String::new(),
    }
}

/// Secret copied with `p`: login password or card number.
pub(crate) fn password_of(item: &VaultItem) -> String {
    match item.item_type {
        ItemType::Card => item
            .card
            .as_ref()
            .map_or_else(String::new, |c| c.number.clone()),
        _ => item.password().to_owned(),
    }
}

/// First non-empty website of a login.
pub(crate) fn first_url(item: &VaultItem) -> Option<String> {
    item.login.as_ref().and_then(|l| {
        l.uris
            .iter()
            .map(|u| u.uri.trim())
            .find(|u| !u.is_empty())
            .map(str::to_owned)
    })
}

#[derive(Debug, Clone, Copy)]
enum CopyWhat {
    Username,
    Password,
    Totp,
}

/// The interactive application.
pub struct App {
    pub lang: Lang,
    store: VaultStore,
    pub settings: Settings,
    platform: Box<dyn Platform>,
    pub vaults: Vec<VaultInfo>,
    pub screen: Screen,
    pub overlay: Option<Overlay>,
    vault: Option<UnlockedVault>,
    /// Filtered, sorted item list of the main screen.
    pub items: Vec<ItemSummary>,
    pub list_state: TableState,
    pub filter: TextInput,
    pub focus: Focus,
    pub show_secrets: bool,
    pub detail_scroll: u16,
    /// Set by the renderer: largest useful `detail_scroll`.
    pub detail_max_scroll: u16,
    /// Set by the renderer: visible list rows (PgUp/PgDn).
    pub page_size: usize,
    pub generator: GeneratorState,
    pub status: Option<Status>,
    pending: Option<Pending>,
    last_activity: Instant,
    last_refresh: Instant,
    last_refresh_error: Option<String>,
    /// KDF parameters for vaults created in the TUI (cheap ones in tests).
    pub(crate) create_kdf: KdfParams,
    should_quit: bool,
}

impl App {
    /// Creates the app. `vault_hint` (from `--vault`) preselects a vault by
    /// id or name.
    pub fn new(
        store: VaultStore,
        settings: Settings,
        platform: Box<dyn Platform>,
        vault_hint: Option<&str>,
    ) -> Self {
        let lang = Lang::from(settings.language);
        let now = Instant::now();
        let mut app = App {
            lang,
            store,
            settings,
            platform,
            vaults: Vec::new(),
            screen: Screen::Main,
            overlay: None,
            vault: None,
            items: Vec::new(),
            list_state: TableState::default(),
            filter: TextInput::new(),
            focus: Focus::List,
            show_secrets: false,
            detail_scroll: 0,
            detail_max_scroll: u16::MAX,
            page_size: 10,
            generator: GeneratorState::default(),
            status: None,
            pending: None,
            last_activity: now,
            last_refresh: now,
            last_refresh_error: None,
            create_kdf: KdfParams::default(),
            should_quit: false,
        };
        app.reload_vault_list();
        app.screen = app.start_screen();
        if let Some(hint) = vault_hint {
            match find_vault(&app.vaults, hint) {
                Some(v) => app.screen = Screen::Unlock(UnlockState::new(v.clone())),
                None => app.set_status(
                    lang.tf(M::VaultHintNotFound, &[("name", hint)]),
                    StatusKind::Error,
                ),
            }
        }
        app
    }

    fn settings_path(&self) -> PathBuf {
        self.store.root().join("settings.json")
    }

    fn reload_vault_list(&mut self) {
        match self.store.list_vaults() {
            Ok(v) => self.vaults = v,
            Err(e) => {
                let msg = self.lang.error(&e);
                self.set_status(msg, StatusKind::Error);
            }
        }
    }

    /// Picker for several vaults, unlock for one, creation for none.
    fn start_screen(&self) -> Screen {
        match self.vaults.len() {
            0 => Screen::CreateVault(CreateVaultForm::new(self.lang, true)),
            1 => Screen::Unlock(UnlockState::new(self.vaults[0].clone())),
            _ => {
                let selected = self
                    .settings
                    .last_vault_id
                    .as_deref()
                    .and_then(|id| self.vaults.iter().position(|v| v.id == id))
                    .unwrap_or(0);
                Screen::VaultPicker { selected }
            }
        }
    }

    // ---- accessors used by the renderer ---------------------------------

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// True while slow work (unlocking, creating a vault) is waiting to run.
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn vault_name(&self) -> Option<&str> {
        self.vault.as_ref().map(UnlockedVault::name)
    }

    /// Number of items outside the trash.
    pub fn total_items(&self) -> usize {
        self.vault
            .as_ref()
            .map_or(0, |v| v.items().iter().filter(|i| !i.is_trashed()).count())
    }

    pub fn folder_name(&self, id: &str) -> Option<&str> {
        let vault = self.vault.as_ref()?;
        vault
            .folders()
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.as_str())
    }

    pub fn unix_time(&self) -> u64 {
        self.platform.unix_time()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.list_state.selected().filter(|i| *i < self.items.len())
    }

    pub fn selected_summary(&self) -> Option<&ItemSummary> {
        self.items.get(self.selected_index()?)
    }

    pub fn selected_item(&self) -> Option<&VaultItem> {
        let id = &self.selected_summary()?.id;
        self.vault.as_ref()?.item(id)
    }

    pub fn selected_actions(&self) -> Option<ItemActions> {
        let item = self.selected_item()?;
        Some(ItemActions {
            username: !username_of(item).trim().is_empty(),
            password: !password_of(item).trim().is_empty(),
            totp: item
                .login
                .as_ref()
                .is_some_and(|l| !l.totp.trim().is_empty()),
            url: first_url(item).is_some(),
        })
    }

    pub fn set_status(&mut self, text: impl Into<String>, kind: StatusKind) {
        let ttl = match kind {
            StatusKind::Error => ERROR_TTL,
            _ => STATUS_TTL,
        };
        self.status = Some(Status {
            text: text.into(),
            kind,
            at: Instant::now(),
            ttl: Some(ttl),
        });
    }

    fn set_sticky_status(&mut self, text: String) {
        self.status = Some(Status {
            text,
            kind: StatusKind::Info,
            at: Instant::now(),
            ttl: None,
        });
    }

    // ---- list handling ----------------------------------------------------

    fn select(&mut self, index: Option<usize>) {
        let index = match (index, self.items.len()) {
            (_, 0) | (None, _) => None,
            (Some(i), n) => Some(i.min(n - 1)),
        };
        if index != self.list_state.selected() {
            self.detail_scroll = 0;
        }
        self.list_state.select(index);
    }

    fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let current = self.selected_index().unwrap_or(0);
        self.select(Some(current.saturating_add_signed(delta)));
    }

    /// Re-runs the filter, keeping the selected item selected if possible.
    fn refresh_items(&mut self) {
        let keep = self.selected_summary().map(|s| s.id.clone());
        let old_index = self.list_state.selected();
        self.items = match &self.vault {
            Some(v) => v.search(self.filter.value()),
            None => Vec::new(),
        };
        let index = keep
            .and_then(|id| self.items.iter().position(|s| s.id == id))
            .or(old_index)
            .or(Some(0));
        self.select(index);
    }

    fn select_id(&mut self, id: &str) {
        if !self.items.iter().any(|s| s.id == id) {
            self.filter.clear();
            self.refresh_items();
        }
        if let Some(i) = self.items.iter().position(|s| s.id == id) {
            self.select(Some(i));
        }
    }

    // ---- events -------------------------------------------------------------

    /// Handles a terminal event (key releases, which Windows reports, are
    /// ignored).
    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Paste(text) => self.handle_paste(&text),
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.last_activity = Instant::now();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.pending.is_some() {
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(key);
            return;
        }
        if self.vault.is_some() {
            if ctrl && key.code == KeyCode::Char('l') {
                let msg = self.lang.t(M::Locked).to_owned();
                self.lock(msg);
                return;
            }
            if key.code == KeyCode::F(1) {
                self.overlay = Some(Overlay::Help);
                return;
            }
        }
        match self.screen {
            Screen::VaultPicker { .. } => self.picker_key(key),
            Screen::CreateVault(_) => self.create_vault_key(key),
            Screen::Unlock(_) => self.unlock_key(key),
            Screen::Main => self.main_key(key),
            Screen::Form(_) => self.form_key(key),
            Screen::Generator => self.generator_key(key),
        }
    }

    /// Pasted text goes into the focused input.
    pub fn handle_paste(&mut self, text: &str) {
        self.last_activity = Instant::now();
        if self.pending.is_some() || self.overlay.is_some() {
            return;
        }
        match &mut self.screen {
            Screen::Unlock(u) => {
                u.password.insert_str(text);
                u.error = None;
            }
            Screen::CreateVault(f) => {
                f.focused().insert_str(text);
                f.error = None;
                f.update_score();
            }
            Screen::Form(f) => {
                if let Some(field) = f.focused_field() {
                    field.input.insert_str(text);
                    f.dirty = true;
                    f.error = None;
                }
            }
            Screen::Main => {
                self.focus = Focus::Filter;
                self.filter.insert_str(text);
                self.refresh_items();
                self.select(Some(0));
            }
            Screen::VaultPicker { .. } | Screen::Generator => {}
        }
    }

    /// Periodic housekeeping: expires status messages, auto-locks after
    /// inactivity and picks up changes other processes saved.
    pub fn tick(&mut self, now: Instant) {
        if let Some(s) = &self.status {
            if s.ttl
                .is_some_and(|ttl| now.saturating_duration_since(s.at) >= ttl)
            {
                self.status = None;
            }
        }
        if self.vault.is_none() || self.pending.is_some() {
            return;
        }
        let minutes = self.settings.auto_lock_minutes;
        if minutes > 0
            && now.saturating_duration_since(self.last_activity)
                >= Duration::from_secs(u64::from(minutes) * 60)
        {
            let msg = self.lang.tf(M::AutoLocked, &[("m", &minutes.to_string())]);
            self.lock(msg);
            return;
        }
        if now.saturating_duration_since(self.last_refresh) >= REFRESH_INTERVAL {
            self.last_refresh = now;
            self.reload_from_disk();
        }
    }

    fn reload_from_disk(&mut self) {
        let lang = self.lang;
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        match vault.reload_if_changed() {
            Ok(changed) => {
                self.last_refresh_error = None;
                if changed {
                    self.refresh_items();
                    self.set_status(lang.t(M::ExternalChange), StatusKind::Info);
                }
            }
            Err(e) => {
                let msg = lang.tf(M::VaultFileError, &[("err", &lang.error(&e))]);
                if self.last_refresh_error.as_ref() != Some(&msg) {
                    self.set_status(msg.clone(), StatusKind::Error);
                    self.last_refresh_error = Some(msg);
                }
            }
        }
    }

    /// Runs deferred slow work (call after drawing the "please wait" frame).
    pub fn run_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let lang = self.lang;
        match pending {
            Pending::Unlock { vault_id, password } => match self.store.unlock(&vault_id, &password)
            {
                Ok(vault) => self.enter_vault(vault),
                Err(e) => {
                    if let Screen::Unlock(u) = &mut self.screen {
                        u.error = Some(lang.error(&e));
                        u.password.clear();
                    }
                }
            },
            Pending::CreateVault { name, password } => {
                match self
                    .store
                    .create_vault_with_params(&name, &password, self.create_kdf)
                {
                    Ok(vault) => {
                        self.reload_vault_list();
                        self.enter_vault(vault);
                    }
                    Err(e) => {
                        if let Screen::CreateVault(f) = &mut self.screen {
                            f.error = Some(lang.error(&e));
                        }
                    }
                }
            }
        }
    }

    fn enter_vault(&mut self, vault: UnlockedVault) {
        // Re-read the settings: the desktop app may have changed them.
        let path = self.settings_path();
        let mut settings = Settings::load_from(&path);
        let mut save_error = None;
        if settings.last_vault_id.as_deref() != Some(vault.id()) {
            settings.last_vault_id = Some(vault.id().to_owned());
            if let Err(e) = settings.save_to(&path) {
                save_error = Some(e);
            }
        }
        self.lang = Lang::from(settings.language);
        self.settings = settings;
        self.vault = Some(vault);
        self.screen = Screen::Main;
        self.focus = Focus::List;
        self.filter.clear();
        self.show_secrets = false;
        self.list_state = TableState::default();
        self.status = None;
        self.refresh_items();
        let now = Instant::now();
        self.last_activity = now;
        self.last_refresh = now;
        if let Some(e) = save_error {
            let msg = self
                .lang
                .tf(M::SettingsSaveFailed, &[("err", &self.lang.error(&e))]);
            self.set_status(msg, StatusKind::Error);
        }
    }

    /// Locks: forgets the decrypted vault, clears a copied secret from the
    /// clipboard and returns to the password prompt.
    pub fn lock(&mut self, message: String) {
        let info = self.vault.take().map(|v| v.info());
        self.platform.clear_pending_secret();
        self.items.clear();
        self.list_state = TableState::default();
        self.filter.clear();
        self.focus = Focus::List;
        self.show_secrets = false;
        self.detail_scroll = 0;
        self.overlay = None;
        self.generator.clear_value();
        self.reload_vault_list();
        self.screen = match info {
            Some(info) => Screen::Unlock(UnlockState::new(info)),
            None => self.start_screen(),
        };
        self.set_sticky_status(message);
    }

    /// Final cleanup before the process exits.
    pub fn shutdown(&mut self) {
        self.platform.clear_pending_secret();
        self.vault = None;
        self.generator.clear_value();
    }

    /// Runs a vault mutation; on `Conflict` (another process saved in the
    /// meantime) reloads the vault and retries once. Returns whether the
    /// retry happened.
    fn mutate_vault<R>(
        &mut self,
        op: impl Fn(&mut UnlockedVault) -> vaultx_core::Result<R>,
    ) -> Result<(R, bool), Error> {
        let Some(vault) = self.vault.as_mut() else {
            return Err(Error::NotFound("vault".into()));
        };
        let (result, reloaded) = match op(vault) {
            Err(Error::Conflict) => match vault.reload_if_changed() {
                Ok(_) => (op(vault).map(|r| (r, true)), true),
                Err(e) => (Err(e), false),
            },
            other => (other.map(|r| (r, false)), false),
        };
        if reloaded {
            self.refresh_items();
        }
        result
    }

    // ---- overlays ---------------------------------------------------------

    fn overlay_key(&mut self, key: KeyEvent) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        let answer = match key.code {
            KeyCode::Enter | KeyCode::Char('j' | 'J' | 'y' | 'Y') => Some(true),
            KeyCode::Esc | KeyCode::Char('n' | 'N' | 'q') => Some(false),
            _ => None,
        };
        match (overlay, answer) {
            (Overlay::Help, _) => {}
            (Overlay::ConfirmTrash { id, name }, Some(true)) => self.trash(&id, &name),
            (Overlay::ConfirmDiscard, Some(true)) => self.close_form(),
            (_, Some(false)) => {}
            (overlay, None) => self.overlay = Some(overlay),
        }
    }

    // ---- vault picker / creation / unlock ---------------------------------

    fn picker_key(&mut self, key: KeyEvent) {
        let Screen::VaultPicker { selected } = &mut self.screen else {
            return;
        };
        let last = self.vaults.len().saturating_sub(1);
        match key.code {
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => *selected = (*selected + 1).min(last),
            KeyCode::Home => *selected = 0,
            KeyCode::End => *selected = last,
            KeyCode::Enter => {
                if let Some(v) = self.vaults.get(*selected) {
                    self.screen = Screen::Unlock(UnlockState::new(v.clone()));
                }
            }
            KeyCode::Char('n') => {
                self.screen = Screen::CreateVault(CreateVaultForm::new(self.lang, false));
            }
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    fn create_vault_key(&mut self, key: KeyEvent) {
        let lang = self.lang;
        let Screen::CreateVault(f) = &mut self.screen else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                if self.vaults.is_empty() {
                    self.should_quit = true;
                } else {
                    self.screen = self.start_screen();
                }
            }
            KeyCode::Tab | KeyCode::Down => f.focus = (f.focus + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => f.focus = (f.focus + 2) % 3,
            KeyCode::Enter if f.focus < 2 => f.focus += 1,
            KeyCode::Enter => {
                if f.validate(lang, MIN_MASTER_LEN).is_ok() {
                    self.pending = Some(Pending::CreateVault {
                        name: f.name.value().trim().to_owned(),
                        password: Zeroizing::new(f.password.value().to_owned()),
                    });
                }
            }
            _ => {
                if f.focused().handle_key(&key) == InputResult::Edited {
                    f.error = None;
                    if f.focus < 2 {
                        f.update_score();
                    }
                }
            }
        }
    }

    fn unlock_key(&mut self, key: KeyEvent) {
        let Screen::Unlock(u) = &mut self.screen else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                if u.password.is_empty() {
                    u.error = Some(self.lang.t(M::PasswordRequired).to_owned());
                } else {
                    u.error = None;
                    self.pending = Some(Pending::Unlock {
                        vault_id: u.vault.id.clone(),
                        password: Zeroizing::new(u.password.value().to_owned()),
                    });
                }
            }
            KeyCode::Esc => {
                if self.vaults.len() > 1 {
                    let selected = self
                        .vaults
                        .iter()
                        .position(|v| v.id == u.vault.id)
                        .unwrap_or(0);
                    self.screen = Screen::VaultPicker { selected };
                } else {
                    self.should_quit = true;
                }
            }
            _ => {
                if u.password.handle_key(&key) == InputResult::Edited {
                    u.error = None;
                }
            }
        }
    }

    // ---- main screen --------------------------------------------------------

    fn page(&self) -> isize {
        isize::try_from(self.page_size.max(1)).unwrap_or(10)
    }

    fn navigate(&mut self, key: KeyCode) {
        let delta = match key {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -self.page(),
            KeyCode::PageDown => self.page(),
            KeyCode::Home => isize::MIN / 2,
            KeyCode::End => isize::MAX / 2,
            _ => return,
        };
        if self.focus == Focus::Detail {
            let scrolled = i64::from(self.detail_scroll) + delta as i64;
            let max = i64::from(self.detail_max_scroll);
            self.detail_scroll = u16::try_from(scrolled.clamp(0, max)).unwrap_or(0);
        } else {
            self.move_selection(delta);
        }
    }

    fn main_key(&mut self, key: KeyEvent) {
        if self.focus == Focus::Filter {
            self.filter_key(key);
            return;
        }
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => self.navigate(key.code),
            KeyCode::Enter | KeyCode::Tab => {
                self.focus = if self.focus == Focus::Detail || self.selected_summary().is_none() {
                    Focus::List
                } else {
                    Focus::Detail
                };
            }
            KeyCode::Right if self.selected_summary().is_some() => self.focus = Focus::Detail,
            KeyCode::Left | KeyCode::BackTab => self.focus = Focus::List,
            KeyCode::Esc => {
                if self.focus == Focus::Detail {
                    self.focus = Focus::List;
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                    self.refresh_items();
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char(c) if plain => match c {
                'q' => self.should_quit = true,
                '/' => self.focus = Focus::Filter,
                '?' => self.overlay = Some(Overlay::Help),
                'u' => self.copy_selected(CopyWhat::Username),
                'p' => self.copy_selected(CopyWhat::Password),
                't' => self.copy_selected(CopyWhat::Totp),
                'o' => self.open_selected(),
                's' => {
                    self.show_secrets = !self.show_secrets;
                    let m = if self.show_secrets {
                        M::SecretsShown
                    } else {
                        M::SecretsHidden
                    };
                    self.set_status(self.lang.t(m), StatusKind::Info);
                }
                'n' => self.screen = Screen::Form(Box::new(ItemForm::new_login())),
                'e' => {
                    if let Some(item) = self.selected_item() {
                        let form = ItemForm::edit(item);
                        self.screen = Screen::Form(Box::new(form));
                    }
                }
                'd' => {
                    if let Some(s) = self.selected_summary() {
                        self.overlay = Some(Overlay::ConfirmTrash {
                            id: s.id.clone(),
                            name: s.name.clone(),
                        });
                    }
                }
                'g' => {
                    self.screen = Screen::Generator;
                    if self.generator.value.is_empty() {
                        self.generator.regenerate();
                    }
                }
                _ => self.start_filter(&key),
            },
            _ => self.start_filter(&key),
        }
    }

    /// Typing a character that is not a shortcut starts the search.
    fn start_filter(&mut self, key: &KeyEvent) {
        if let Some(c) = text_char(key) {
            self.focus = Focus::Filter;
            self.filter.insert_char(c);
            self.refresh_items();
            self.select(Some(0));
        }
    }

    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.focus = Focus::List;
                self.refresh_items();
            }
            KeyCode::Enter | KeyCode::Tab => self.focus = Focus::List,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                self.navigate(key.code)
            }
            KeyCode::Backspace if self.filter.is_empty() => self.focus = Focus::List,
            _ => {
                if self.filter.handle_key(&key) == InputResult::Edited {
                    self.refresh_items();
                    self.select(Some(0));
                }
            }
        }
    }

    fn copy_selected(&mut self, what: CopyWhat) {
        let lang = self.lang;
        let now = self.platform.unix_time();
        let Some(item) = self.selected_item() else {
            return;
        };
        let (value, label, secret, missing) = match what {
            CopyWhat::Username => (
                Zeroizing::new(username_of(item)),
                M::WhatUsername,
                false,
                M::NoUsername,
            ),
            CopyWhat::Password => {
                let label = if item.item_type == ItemType::Card {
                    M::WhatCardNumber
                } else {
                    M::WhatPassword
                };
                (
                    Zeroizing::new(password_of(item)),
                    label,
                    true,
                    M::NoPassword,
                )
            }
            CopyWhat::Totp => {
                let seed = item.login.as_ref().map_or("", |l| l.totp.trim());
                if seed.is_empty() {
                    (Zeroizing::new(String::new()), M::WhatCode, true, M::NoTotp)
                } else {
                    match totp::totp_at(seed, now) {
                        Ok(code) => (Zeroizing::new(code.code), M::WhatCode, true, M::NoTotp),
                        Err(e) => {
                            let msg = lang.error(&e);
                            self.set_status(msg, StatusKind::Error);
                            return;
                        }
                    }
                }
            }
        };
        if value.trim().is_empty() {
            self.set_status(lang.t(missing), StatusKind::Info);
            return;
        }
        self.copy_value(&value, label, secret);
    }

    fn copy_value(&mut self, value: &str, label: M, secret: bool) {
        let lang = self.lang;
        let secs = self.settings.clipboard_clear_seconds;
        let result = if secret {
            let clear = (secs > 0).then(|| Duration::from_secs(u64::from(secs)));
            self.platform.copy_secret(value, clear)
        } else {
            self.platform.copy_text(value)
        };
        let what = lang.t(label);
        match result {
            Ok(()) if secret && secs > 0 => self.set_status(
                lang.tf(M::CopiedClears, &[("what", what), ("s", &secs.to_string())]),
                StatusKind::Success,
            ),
            Ok(()) => self.set_status(lang.tf(M::Copied, &[("what", what)]), StatusKind::Success),
            Err(e) => self.set_status(
                lang.tf(M::ClipboardError, &[("err", &clipboard_detail(&e))]),
                StatusKind::Error,
            ),
        }
    }

    fn open_selected(&mut self) {
        let lang = self.lang;
        let Some(item) = self.selected_item() else {
            return;
        };
        let Some(uri) = first_url(item) else {
            self.set_status(lang.t(M::NoUrl), StatusKind::Info);
            return;
        };
        let Some(url) = browser_url(&uri) else {
            self.set_status(lang.t(M::UrlNotAllowed), StatusKind::Error);
            return;
        };
        match self.platform.open_url(&url) {
            Ok(()) => self.set_status(lang.tf(M::OpenedUrl, &[("url", &url)]), StatusKind::Success),
            Err(e) => self.set_status(
                lang.tf(M::OpenFailed, &[("err", &e.to_string())]),
                StatusKind::Error,
            ),
        }
    }

    fn trash(&mut self, id: &str, name: &str) {
        let lang = self.lang;
        match self.mutate_vault(|v| v.trash_item(id)) {
            Ok(((), retried)) => {
                self.refresh_items();
                self.focus = Focus::List;
                let msg = if retried {
                    lang.t(M::ConflictRetried).to_owned()
                } else {
                    lang.tf(M::ItemTrashed, &[("name", name)])
                };
                self.set_status(msg, StatusKind::Success);
            }
            Err(e) => self.set_status(lang.error(&e), StatusKind::Error),
        }
    }

    // ---- item editor ----------------------------------------------------------

    fn form_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if ctrl {
            match key.code {
                KeyCode::Char('s') => return self.save_form(),
                KeyCode::Char('g') => return self.generate_into_form(),
                KeyCode::Char('r') => {
                    if let Screen::Form(f) = &mut self.screen {
                        f.reveal = !f.reveal;
                    }
                    return;
                }
                _ => {}
            }
        }
        let Screen::Form(form) = &mut self.screen else {
            return;
        };
        enum After {
            Nothing,
            Save,
            Cancel,
        }
        let after = match key.code {
            KeyCode::Esc => After::Cancel,
            KeyCode::Tab => {
                form.next();
                After::Nothing
            }
            KeyCode::BackTab => {
                form.prev();
                After::Nothing
            }
            _ => match form.focus_target() {
                target @ (FormFocus::Save | FormFocus::Cancel) => match key.code {
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if target == FormFocus::Save {
                            After::Save
                        } else {
                            After::Cancel
                        }
                    }
                    KeyCode::Up | KeyCode::Left if target == FormFocus::Cancel => {
                        form.prev();
                        After::Nothing
                    }
                    KeyCode::Right | KeyCode::Down if target == FormFocus::Save => {
                        form.next();
                        After::Nothing
                    }
                    KeyCode::Up => {
                        form.prev();
                        After::Nothing
                    }
                    _ => After::Nothing,
                },
                FormFocus::Field(i) => {
                    let field = &mut form.fields[i];
                    if key.code == KeyCode::Enter && !field.input.is_multiline() {
                        form.next();
                    } else {
                        match field.input.handle_key(&key) {
                            InputResult::Edited => {
                                form.dirty = true;
                                form.error = None;
                            }
                            InputResult::Moved => {}
                            InputResult::Ignored => match key.code {
                                KeyCode::Up => form.prev(),
                                KeyCode::Down => form.next(),
                                _ => {}
                            },
                        }
                    }
                    After::Nothing
                }
            },
        };
        match after {
            After::Nothing => {}
            After::Save => self.save_form(),
            After::Cancel => {
                if form_is_dirty(&self.screen) {
                    self.overlay = Some(Overlay::ConfirmDiscard);
                } else {
                    self.close_form();
                }
            }
        }
    }

    fn close_form(&mut self) {
        if matches!(self.screen, Screen::Form(_)) {
            self.screen = Screen::Main;
        }
    }

    fn generate_into_form(&mut self) {
        let lang = self.lang;
        let Screen::Form(form) = &mut self.screen else {
            return;
        };
        if !form.has_password() {
            return;
        }
        match vaultx_core::generator::generate(&self.generator.options) {
            Ok(pw) => {
                let pw = Zeroizing::new(pw);
                form.set_password(&pw);
                self.set_status(lang.t(M::PasswordGenerated), StatusKind::Success);
            }
            Err(e) => self.set_status(lang.error(&e), StatusKind::Error),
        }
    }

    fn save_form(&mut self) {
        let lang = self.lang;
        let Screen::Form(form) = &mut self.screen else {
            return;
        };
        let Ok(item) = form.build_item(lang) else {
            return;
        };
        let is_new = form.is_new;
        match self.mutate_vault(|v| v.save_item(item.clone())) {
            Ok((saved, retried)) => {
                self.screen = Screen::Main;
                self.focus = Focus::List;
                self.refresh_items();
                self.select_id(&saved.id);
                let msg = if retried {
                    M::ConflictRetried
                } else if is_new {
                    M::ItemCreated
                } else {
                    M::ItemSaved
                };
                self.set_status(lang.t(msg), StatusKind::Success);
            }
            Err(e) => {
                if let Screen::Form(form) = &mut self.screen {
                    form.error = Some(lang.error(&e));
                }
            }
        }
    }

    // ---- generator ------------------------------------------------------------

    fn generator_key(&mut self, key: KeyEvent) {
        let g = &mut self.generator;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                g.clear_value();
                self.screen = Screen::Main;
            }
            KeyCode::Up => g.move_row(-1),
            KeyCode::Down => g.move_row(1),
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                let accepted = match key.code {
                    KeyCode::Left => g.adjust_row(-1),
                    KeyCode::Right => g.adjust_row(1),
                    _ => g.toggle_row(),
                };
                if !accepted {
                    let msg = self.lang.t(M::ErrNoCharset);
                    self.set_status(msg, StatusKind::Info);
                }
            }
            KeyCode::Char('-') => g.adjust_size(-1),
            KeyCode::Char('+' | '=') => g.adjust_size(1),
            KeyCode::Enter | KeyCode::Char('r') => g.regenerate(),
            KeyCode::Char('c') => self.copy_generated(),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            _ => {}
        }
    }

    fn copy_generated(&mut self) {
        if self.generator.value.is_empty() {
            return;
        }
        let value = self.generator.value.clone();
        self.copy_value(&value, M::WhatPassword, true);
        // Keep it in the vault's generator history (like the desktop app).
        if let Err(e) = self.mutate_vault(|v| v.add_generated_password(&value)) {
            let msg = self.lang.error(&e);
            self.set_status(msg, StatusKind::Error);
        }
    }
}

fn form_is_dirty(screen: &Screen) -> bool {
    matches!(screen, Screen::Form(f) if f.dirty)
}

/// Finds a vault by id or (case-insensitive) name.
pub fn find_vault<'a>(vaults: &'a [VaultInfo], query: &str) -> Option<&'a VaultInfo> {
    let query = query.trim();
    vaults.iter().find(|v| v.id == query).or_else(|| {
        let wanted = query.to_lowercase();
        let mut by_name = vaults
            .iter()
            .filter(|v| v.name.trim().to_lowercase() == wanted);
        let first = by_name.next();
        // Ambiguous names select nothing.
        if by_name.next().is_some() {
            None
        } else {
            first
        }
    })
}

/// The useful part of a clipboard error.
pub fn clipboard_detail(e: &Error) -> String {
    match e {
        Error::Io(io) => io.to_string(),
        Error::Unsupported(d) => d.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
