//! Forms: the item editor (new login / edit any item) and the "create
//! vault" form.

use vaultx_core::model::{ItemType, LoginUri, VaultItem};
use vaultx_core::totp;
use zeroize::Zeroize;

use crate::i18n::{Lang, M};
use crate::input::TextInput;

/// A field of the item editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKey {
    Name,
    Website,
    Username,
    Password,
    Totp,
    Cardholder,
    Brand,
    Number,
    ExpMonth,
    ExpYear,
    Code,
    Title,
    FirstName,
    LastName,
    Email,
    Phone,
    Company,
    Address1,
    Address2,
    PostalCode,
    City,
    State,
    Country,
    IdUsername,
    Notes,
}

impl FieldKey {
    pub fn label(self) -> M {
        match self {
            FieldKey::Name => M::FieldName,
            FieldKey::Website => M::FieldWebsite,
            FieldKey::Username | FieldKey::IdUsername => M::FieldUsername,
            FieldKey::Password => M::FieldPassword,
            FieldKey::Totp => M::FieldTotpKey,
            FieldKey::Cardholder => M::FieldCardholder,
            FieldKey::Brand => M::FieldBrand,
            FieldKey::Number => M::FieldCardNumber,
            FieldKey::ExpMonth => M::FieldExpMonth,
            FieldKey::ExpYear => M::FieldExpYear,
            FieldKey::Code => M::FieldSecurityCode,
            FieldKey::Title => M::FieldTitle,
            FieldKey::FirstName => M::FieldFirstName,
            FieldKey::LastName => M::FieldLastName,
            FieldKey::Email => M::FieldEmail,
            FieldKey::Phone => M::FieldPhone,
            FieldKey::Company => M::FieldCompany,
            FieldKey::Address1 => M::FieldAddress1,
            FieldKey::Address2 => M::FieldAddress2,
            FieldKey::PostalCode => M::FieldPostalCode,
            FieldKey::City => M::FieldCity,
            FieldKey::State => M::FieldState,
            FieldKey::Country => M::FieldCountry,
            FieldKey::Notes => M::FieldNotes,
        }
    }

    /// Masked unless revealed with Ctrl+R.
    pub fn is_secret(self) -> bool {
        matches!(self, FieldKey::Password | FieldKey::Code)
    }
}

#[derive(Debug)]
pub struct FormField {
    pub key: FieldKey,
    pub input: TextInput,
}

/// Focus target of the item editor besides the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFocus {
    Field(usize),
    Save,
    Cancel,
}

/// The item editor.
#[derive(Debug)]
pub struct ItemForm {
    /// The item being edited (a fresh template for a new one). Fields the
    /// form does not show (folder, further websites, custom fields, …) are
    /// kept unchanged.
    original: VaultItem,
    pub is_new: bool,
    pub fields: Vec<FormField>,
    /// Index into fields, then Save (= len) and Cancel (= len + 1).
    pub focus: usize,
    pub reveal: bool,
    pub dirty: bool,
    pub error: Option<String>,
    /// First visible row (kept up to date by the renderer).
    pub scroll: u16,
}

impl Drop for ItemForm {
    fn drop(&mut self) {
        wipe_item(&mut self.original);
    }
}

/// Overwrites the secret parts of an item copy before it is freed.
fn wipe_item(item: &mut VaultItem) {
    item.notes.zeroize();
    if let Some(l) = item.login.as_mut() {
        l.password.zeroize();
        l.totp.zeroize();
        l.username.zeroize();
    }
    if let Some(c) = item.card.as_mut() {
        c.number.zeroize();
        c.code.zeroize();
    }
    for h in &mut item.password_history {
        h.password.zeroize();
    }
    for f in &mut item.fields {
        f.value.zeroize();
    }
}

fn keys_for(item_type: ItemType) -> &'static [FieldKey] {
    use FieldKey::*;
    match item_type {
        ItemType::Login => &[Name, Website, Username, Password, Totp, Notes],
        ItemType::Card => &[
            Name, Cardholder, Brand, Number, ExpMonth, ExpYear, Code, Notes,
        ],
        ItemType::Identity => &[
            Name, Title, FirstName, LastName, Email, Phone, Company, Address1, Address2,
            PostalCode, City, State, Country, IdUsername, Notes,
        ],
        ItemType::Note => &[Name, Notes],
    }
}

fn initial_value(item: &VaultItem, key: FieldKey) -> String {
    let login = item.login.as_ref();
    let card = item.card.as_ref();
    let id = item.identity.as_ref();
    let s = match key {
        FieldKey::Name => Some(&item.name),
        FieldKey::Notes => Some(&item.notes),
        FieldKey::Website => login.and_then(|l| l.uris.first()).map(|u| &u.uri),
        FieldKey::Username => login.map(|l| &l.username),
        FieldKey::Password => login.map(|l| &l.password),
        FieldKey::Totp => login.map(|l| &l.totp),
        FieldKey::Cardholder => card.map(|c| &c.cardholder_name),
        FieldKey::Brand => card.map(|c| &c.brand),
        FieldKey::Number => card.map(|c| &c.number),
        FieldKey::ExpMonth => card.map(|c| &c.exp_month),
        FieldKey::ExpYear => card.map(|c| &c.exp_year),
        FieldKey::Code => card.map(|c| &c.code),
        FieldKey::Title => id.map(|i| &i.title),
        FieldKey::FirstName => id.map(|i| &i.first_name),
        FieldKey::LastName => id.map(|i| &i.last_name),
        FieldKey::Email => id.map(|i| &i.email),
        FieldKey::Phone => id.map(|i| &i.phone),
        FieldKey::Company => id.map(|i| &i.company),
        FieldKey::Address1 => id.map(|i| &i.address1),
        FieldKey::Address2 => id.map(|i| &i.address2),
        FieldKey::PostalCode => id.map(|i| &i.postal_code),
        FieldKey::City => id.map(|i| &i.city),
        FieldKey::State => id.map(|i| &i.state),
        FieldKey::Country => id.map(|i| &i.country),
        FieldKey::IdUsername => id.map(|i| &i.username),
    };
    s.cloned().unwrap_or_default()
}

impl ItemForm {
    /// Editor for a new login.
    pub fn new_login() -> Self {
        Self::build(VaultItem::new(ItemType::Login, ""), true)
    }

    /// Editor for an existing item.
    pub fn edit(item: &VaultItem) -> Self {
        Self::build(item.clone(), false)
    }

    fn build(original: VaultItem, is_new: bool) -> Self {
        let fields = keys_for(original.item_type)
            .iter()
            .map(|&key| {
                let value = initial_value(&original, key);
                let input = if key == FieldKey::Notes {
                    TextInput::multiline(&value)
                } else {
                    TextInput::with_value(&value)
                };
                FormField { key, input }
            })
            .collect();
        ItemForm {
            original,
            is_new,
            fields,
            focus: 0,
            reveal: false,
            dirty: false,
            error: None,
            scroll: 0,
        }
    }

    /// Name of the item as it was when the editor was opened.
    pub fn original_name(&self) -> &str {
        &self.original.name
    }

    pub fn focus_target(&self) -> FormFocus {
        match self.focus {
            i if i < self.fields.len() => FormFocus::Field(i),
            i if i == self.fields.len() => FormFocus::Save,
            _ => FormFocus::Cancel,
        }
    }

    pub fn focused_field(&mut self) -> Option<&mut FormField> {
        self.fields.get_mut(self.focus)
    }

    pub fn next(&mut self) {
        self.focus = (self.focus + 1) % (self.fields.len() + 2);
    }

    pub fn prev(&mut self) {
        let n = self.fields.len() + 2;
        self.focus = (self.focus + n - 1) % n;
    }

    fn index_of(&self, key: FieldKey) -> Option<usize> {
        self.fields.iter().position(|f| f.key == key)
    }

    fn get(&self, key: FieldKey) -> &str {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map_or("", |f| f.input.value())
    }

    /// True if the item has a password field (Ctrl+G).
    pub fn has_password(&self) -> bool {
        self.index_of(FieldKey::Password).is_some()
    }

    /// Puts a generated password into the password field.
    pub fn set_password(&mut self, password: &str) {
        if let Some(i) = self.index_of(FieldKey::Password) {
            self.fields[i].input.set_value(password);
            self.dirty = true;
            self.error = None;
        }
    }

    /// Validates the fields and builds the item to save. On error, focuses
    /// the offending field and returns the message.
    pub fn build_item(&mut self, lang: Lang) -> Result<VaultItem, String> {
        match self.try_build(lang) {
            Ok(item) => Ok(item),
            Err((key, msg)) => {
                if let Some(i) = self.index_of(key) {
                    self.focus = i;
                }
                self.error = Some(msg.clone());
                Err(msg)
            }
        }
    }

    fn try_build(&self, lang: Lang) -> Result<VaultItem, (FieldKey, String)> {
        let mut item = self.original.clone();
        item.name = self.get(FieldKey::Name).trim().to_owned();
        if item.name.is_empty() {
            return Err((FieldKey::Name, lang.t(M::ErrNameRequired).to_owned()));
        }
        item.notes = self.get(FieldKey::Notes).trim_end().to_owned();
        let trimmed = |key| self.get(key).trim().to_owned();
        match item.item_type {
            ItemType::Login => {
                let login = item.login.get_or_insert_with(Default::default);
                login.username = trimmed(FieldKey::Username);
                login.password = self.get(FieldKey::Password).to_owned();
                login.totp = trimmed(FieldKey::Totp);
                if !login.totp.is_empty() {
                    if let Err(e) = totp::parse(&login.totp) {
                        return Err((FieldKey::Totp, lang.error(&e)));
                    }
                }
                let url = trimmed(FieldKey::Website);
                match (login.uris.is_empty(), url.is_empty()) {
                    (true, true) => {}
                    (true, false) => login.uris.push(LoginUri {
                        uri: url,
                        ..Default::default()
                    }),
                    (false, true) => {
                        login.uris.remove(0);
                    }
                    (false, false) => login.uris[0].uri = url,
                }
            }
            ItemType::Card => {
                let card = item.card.get_or_insert_with(Default::default);
                card.cardholder_name = trimmed(FieldKey::Cardholder);
                card.brand = trimmed(FieldKey::Brand);
                card.number = trimmed(FieldKey::Number);
                card.code = trimmed(FieldKey::Code);
                card.exp_month = normalize_month(&trimmed(FieldKey::ExpMonth))
                    .ok_or_else(|| (FieldKey::ExpMonth, lang.t(M::InvalidMonth).to_owned()))?;
                card.exp_year = normalize_year(&trimmed(FieldKey::ExpYear))
                    .ok_or_else(|| (FieldKey::ExpYear, lang.t(M::InvalidYear).to_owned()))?;
            }
            ItemType::Identity => {
                let id = item.identity.get_or_insert_with(Default::default);
                id.title = trimmed(FieldKey::Title);
                id.first_name = trimmed(FieldKey::FirstName);
                id.last_name = trimmed(FieldKey::LastName);
                id.email = trimmed(FieldKey::Email);
                id.phone = trimmed(FieldKey::Phone);
                id.company = trimmed(FieldKey::Company);
                id.address1 = trimmed(FieldKey::Address1);
                id.address2 = trimmed(FieldKey::Address2);
                id.postal_code = trimmed(FieldKey::PostalCode);
                id.city = trimmed(FieldKey::City);
                id.state = trimmed(FieldKey::State);
                id.country = trimmed(FieldKey::Country);
                id.username = trimmed(FieldKey::IdUsername);
            }
            ItemType::Note => {}
        }
        Ok(item)
    }
}

/// "" stays empty; 1..=12 becomes "01".."12"; anything else is invalid.
fn normalize_month(s: &str) -> Option<String> {
    if s.is_empty() {
        return Some(String::new());
    }
    let m: u8 = s.parse().ok()?;
    (1..=12).contains(&m).then(|| format!("{m:02}"))
}

/// "" stays empty; four digits stay; two digits become 20xx.
fn normalize_year(s: &str) -> Option<String> {
    if s.is_empty() {
        return Some(String::new());
    }
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match s.len() {
        4 => Some(s.to_owned()),
        2 => Some(format!("20{s}")),
        _ => None,
    }
}

/// The "create vault" form: name, master password, confirmation.
#[derive(Debug)]
pub struct CreateVaultForm {
    pub name: TextInput,
    pub password: TextInput,
    pub confirm: TextInput,
    /// 0 = name, 1 = password, 2 = confirmation.
    pub focus: usize,
    pub error: Option<String>,
    /// zxcvbn score of the password (None while empty).
    pub score: Option<u8>,
    /// Shown above the form when no vault exists yet.
    pub first_run: bool,
}

impl CreateVaultForm {
    pub fn new(lang: Lang, first_run: bool) -> Self {
        CreateVaultForm {
            name: TextInput::with_value(lang.t(M::DefaultVaultName)),
            password: TextInput::new(),
            confirm: TextInput::new(),
            focus: if first_run { 1 } else { 0 },
            error: None,
            score: None,
            first_run,
        }
    }

    pub fn focused(&mut self) -> &mut TextInput {
        match self.focus {
            0 => &mut self.name,
            1 => &mut self.password,
            _ => &mut self.confirm,
        }
    }

    pub fn update_score(&mut self) {
        self.score = (!self.password.is_empty()).then(|| {
            vaultx_core::health::strength(self.password.value(), &[self.name.value()]).score
        });
    }

    /// Checks the input; on error focuses the offending field and returns
    /// the message.
    pub fn validate(&mut self, lang: Lang, min_len: usize) -> Result<(), String> {
        let (focus, msg) = if self.name.value().trim().is_empty() {
            (0, lang.t(M::ErrNameRequired).to_owned())
        } else if self.password.is_empty() {
            (1, lang.t(M::PasswordRequired).to_owned())
        } else if self.password.value().chars().count() < min_len {
            (1, lang.tf(M::TooShort, &[("min", &min_len.to_string())]))
        } else if self.score.is_some_and(|s| s < 2) {
            (1, lang.t(M::TooWeak).to_owned())
        } else if self.password.value() != self.confirm.value() {
            (2, lang.t(M::Mismatch).to_owned())
        } else {
            self.error = None;
            return Ok(());
        };
        self.focus = focus;
        self.error = Some(msg.clone());
        Err(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaultx_core::model::CardData;

    #[test]
    fn login_form_round_trip() {
        let mut item = VaultItem::new(ItemType::Login, "GitHub");
        item.id = "id-1".into();
        item.folder_id = Some("folder".into());
        if let Some(l) = item.login.as_mut() {
            l.username = "me".into();
            l.password = "pw".into();
            l.uris = vec![
                LoginUri {
                    uri: "https://github.com".into(),
                    ..Default::default()
                },
                LoginUri {
                    uri: "https://gist.github.com".into(),
                    ..Default::default()
                },
            ];
        }
        let mut form = ItemForm::edit(&item);
        assert_eq!(form.get(FieldKey::Website), "https://github.com");
        let i = form.index_of(FieldKey::Website).unwrap();
        form.fields[i].input.set_value("");
        form.set_password("new-pw");
        let built = form.build_item(Lang::De).unwrap();
        let login = built.login.unwrap();
        assert_eq!(login.password, "new-pw");
        // The first website was removed, the second one is kept.
        assert_eq!(login.uris.len(), 1);
        assert_eq!(login.uris[0].uri, "https://gist.github.com");
        assert_eq!(built.folder_id.as_deref(), Some("folder"));
        assert_eq!(built.id, "id-1");
    }

    #[test]
    fn validation_focuses_field() {
        let mut form = ItemForm::new_login();
        form.focus = 3;
        let err = form.build_item(Lang::En).unwrap_err();
        assert_eq!(err, "Please enter a name.");
        assert_eq!(form.focus, 0);
        form.fields[0].input.set_value("X");
        let t = form.index_of(FieldKey::Totp).unwrap();
        form.fields[t].input.set_value("not base32 !!");
        assert!(form.build_item(Lang::En).is_err());
        assert_eq!(form.focus, t);
        form.fields[t].input.set_value("JBSWY3DPEHPK3PXP");
        let item = form.build_item(Lang::En).unwrap();
        assert_eq!(item.login.unwrap().totp, "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn card_expiry_normalisation() {
        let mut item = VaultItem::new(ItemType::Card, "Visa");
        item.card = Some(CardData::default());
        let mut form = ItemForm::edit(&item);
        let m = form.index_of(FieldKey::ExpMonth).unwrap();
        let y = form.index_of(FieldKey::ExpYear).unwrap();
        form.fields[m].input.set_value("3");
        form.fields[y].input.set_value("28");
        let card = form.build_item(Lang::De).unwrap().card.unwrap();
        assert_eq!(
            (card.exp_month.as_str(), card.exp_year.as_str()),
            ("03", "2028")
        );
        form.fields[m].input.set_value("13");
        assert!(form.build_item(Lang::De).is_err());
        assert_eq!(form.focus, m);
        assert!(normalize_year("028").is_none());
        assert!(normalize_year("20a8").is_none());
    }

    #[test]
    fn focus_cycles_through_buttons() {
        let mut form = ItemForm::edit(&VaultItem::new(ItemType::Note, "n"));
        assert_eq!(form.fields.len(), 2);
        form.next();
        form.next();
        assert_eq!(form.focus_target(), FormFocus::Save);
        form.next();
        assert_eq!(form.focus_target(), FormFocus::Cancel);
        form.next();
        assert_eq!(form.focus_target(), FormFocus::Field(0));
        form.prev();
        assert_eq!(form.focus_target(), FormFocus::Cancel);
    }

    #[test]
    fn create_vault_validation() {
        let mut f = CreateVaultForm::new(Lang::En, true);
        assert_eq!(f.name.value(), "Personal");
        assert!(f.validate(Lang::En, 8).is_err());
        f.password.set_value("short");
        f.update_score();
        assert!(f.validate(Lang::En, 8).unwrap_err().contains('8'));
        f.password.set_value("correct horse battery staple");
        f.update_score();
        f.confirm.set_value("something else");
        assert_eq!(
            f.validate(Lang::En, 8).unwrap_err(),
            "The passwords do not match."
        );
        assert_eq!(f.focus, 2);
        f.confirm.set_value("correct horse battery staple");
        assert!(f.validate(Lang::En, 8).is_ok());
    }
}
