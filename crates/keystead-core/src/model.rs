//! Plain data model stored (encrypted) inside a vault file.
//!
//! This file is the shared contract between the core, the desktop backend,
//! the frontend (`apps/desktop/src/lib/types.ts` mirrors it 1:1) and the
//! terminal UI. All JSON uses camelCase keys. Timestamps are Unix epoch
//! milliseconds (UTC).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Everything inside the encrypted payload of a vault.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VaultData {
    pub items: Vec<VaultItem>,
    pub folders: Vec<Folder>,
    /// Last generated passwords (newest first, max 50). Only kept locally.
    pub generator_history: Vec<GeneratedPassword>,
    /// Website icons by host (see [`crate::icons::icon_host`]), fetched by
    /// the desktop app. Kept inside the encrypted payload – the set of hosts
    /// would reveal the user's accounts. Never exported or imported.
    pub icons: BTreeMap<String, IconEntry>,
}

/// A website icon stored in the vault ([`VaultData::icons`]).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IconEntry {
    /// Base64 (standard, padded) of a 64×64 PNG; `None` if no fetch has
    /// succeeded yet. A failed refresh keeps the previous icon.
    pub png: Option<String>,
    /// Unix ms of the last fetch attempt (successful or not).
    pub fetched_at: i64,
    /// Unix ms of the last failed attempt; `None` after a success.
    pub failed_at: Option<i64>,
}

impl std::fmt::Debug for IconEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never dump the image data.
        f.debug_struct("IconEntry")
            .field("png_len", &self.png.as_ref().map(String::len))
            .field("fetched_at", &self.fetched_at)
            .field("failed_at", &self.failed_at)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    #[default]
    Login,
    Card,
    Identity,
    Note,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VaultItem {
    /// UUID v4. Empty string on a *new* item sent from the UI; the core assigns it.
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub name: String,
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub notes: String,
    /// Present (Some) iff `item_type == Login`.
    pub login: Option<LoginData>,
    /// Present (Some) iff `item_type == Card`.
    pub card: Option<CardData>,
    /// Present (Some) iff `item_type == Identity`.
    pub identity: Option<IdentityData>,
    pub fields: Vec<CustomField>,
    /// Previous passwords of a login, newest first, max 10. Maintained by the core.
    pub password_history: Vec<PasswordHistoryEntry>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Set when the item is in the trash ("Papierkorb").
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginData {
    pub username: String,
    pub password: String,
    pub uris: Vec<LoginUri>,
    /// TOTP seed: either a bare base32 secret or a full `otpauth://totp/...` URI.
    /// Empty string = no TOTP.
    pub totp: String,
    /// Unix ms of the last password change (None if never changed after creation).
    pub password_revised_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginUri {
    pub uri: String,
    #[serde(rename = "match")]
    pub match_type: UriMatch,
}

/// How a stored URI is compared with the page URL for autofill suggestions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UriMatch {
    /// Same registrable domain (example.com matches login.example.com). Default.
    #[default]
    Domain,
    /// Same host (and port, if given).
    Host,
    /// Page URL starts with the stored URI.
    StartsWith,
    /// Page URL equals the stored URI.
    Exact,
    /// Never suggest for autofill.
    Never,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardData {
    pub cardholder_name: String,
    /// Free text, e.g. "Visa", "Mastercard". The UI may auto-detect it from the number.
    pub brand: String,
    pub number: String,
    /// "01".."12" or empty.
    pub exp_month: String,
    /// Four digits or empty.
    pub exp_year: String,
    pub code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdentityData {
    pub title: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: String,
    pub company: String,
    pub address1: String,
    pub address2: String,
    pub postal_code: String,
    pub city: String,
    pub state: String,
    pub country: String,
    pub username: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomField {
    pub name: String,
    pub value: String,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    #[default]
    Text,
    /// Masked like a password in the UI.
    Hidden,
    /// value is "true" / "false".
    Boolean,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PasswordHistoryEntry {
    pub password: String,
    /// When this password stopped being the current one.
    pub replaced_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    /// UUID v4. Empty on a new folder; the core assigns it.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GeneratedPassword {
    pub password: String,
    pub created_at: i64,
}

/// Public, non-secret description of a vault file on disk (readable while locked).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub has_recovery_key: bool,
}

/// Lightweight item view without secrets (used by search lists, the browser
/// extension and the TUI list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub name: String,
    /// Login username, card "•••• 1234", identity full name, empty for notes.
    pub subtitle: String,
    /// First URI of a login (empty otherwise).
    pub uri: String,
    pub favorite: bool,
    pub has_totp: bool,
    pub folder_id: Option<String>,
    /// Website icon as a `data:image/png;base64,…` URL. Only filled in by
    /// the desktop app's browser bridge for a few rows (omitted otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Current time in Unix epoch milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Helper methods (no change to the serialised shape above).
// ---------------------------------------------------------------------------

impl VaultItem {
    /// An empty item of the given type with the matching type-specific part.
    pub fn new(item_type: ItemType, name: impl Into<String>) -> Self {
        let mut item = VaultItem {
            item_type,
            name: name.into(),
            ..Default::default()
        };
        item.normalize();
        item
    }

    /// True if the item is in the trash.
    pub fn is_trashed(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Makes `login`/`card`/`identity` match `item_type` (Some for the
    /// matching one, None for the others) and drops the password history of
    /// non-login items.
    pub fn normalize(&mut self) {
        match self.item_type {
            ItemType::Login => {
                self.login.get_or_insert_with(LoginData::default);
                self.card = None;
                self.identity = None;
            }
            ItemType::Card => {
                self.card.get_or_insert_with(CardData::default);
                self.login = None;
                self.identity = None;
            }
            ItemType::Identity => {
                self.identity.get_or_insert_with(IdentityData::default);
                self.login = None;
                self.card = None;
            }
            ItemType::Note => {
                self.login = None;
                self.card = None;
                self.identity = None;
            }
        }
        if self.item_type != ItemType::Login {
            self.password_history.clear();
        }
    }

    /// Login username (empty for other types).
    pub fn username(&self) -> &str {
        self.login.as_ref().map_or("", |l| l.username.as_str())
    }

    /// Login password (empty for other types).
    pub fn password(&self) -> &str {
        self.login.as_ref().map_or("", |l| l.password.as_str())
    }

    /// Secret-free summary for lists.
    pub fn summary(&self) -> ItemSummary {
        let (subtitle, uri, has_totp) = match self.item_type {
            ItemType::Login => {
                let login = self.login.as_ref();
                (
                    login.map(|l| l.username.clone()).unwrap_or_default(),
                    login
                        .and_then(|l| l.uris.first())
                        .map(|u| u.uri.clone())
                        .unwrap_or_default(),
                    login.is_some_and(|l| !l.totp.trim().is_empty()),
                )
            }
            ItemType::Card => (
                self.card.as_ref().map(card_subtitle).unwrap_or_default(),
                String::new(),
                false,
            ),
            ItemType::Identity => (
                self.identity
                    .as_ref()
                    .map(identity_subtitle)
                    .unwrap_or_default(),
                String::new(),
                false,
            ),
            ItemType::Note => (String::new(), String::new(), false),
        };
        ItemSummary {
            id: self.id.clone(),
            item_type: self.item_type,
            name: self.name.clone(),
            subtitle,
            uri,
            favorite: self.favorite,
            has_totp,
            folder_id: self.folder_id.clone(),
            icon: None,
        }
    }
}

fn card_subtitle(card: &CardData) -> String {
    let digits: Vec<char> = card.number.chars().filter(char::is_ascii_digit).collect();
    if digits.len() >= 4 {
        let last4: String = digits[digits.len() - 4..].iter().collect();
        format!("•••• {last4}")
    } else {
        card.brand.trim().to_owned()
    }
}

fn identity_subtitle(id: &IdentityData) -> String {
    let full = format!("{} {}", id.first_name.trim(), id.last_name.trim());
    let full = full.trim();
    if full.is_empty() {
        id.email.trim().to_owned()
    } else {
        full.to_owned()
    }
}

impl From<&VaultItem> for ItemSummary {
    fn from(item: &VaultItem) -> Self {
        item.summary()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_json_shape() {
        let mut item = VaultItem::new(ItemType::Login, "GitHub");
        item.id = "x".into();
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "login");
        assert_eq!(v["folderId"], serde_json::Value::Null);
        assert_eq!(v["login"]["passwordRevisedAt"], serde_json::Value::Null);
        assert_eq!(v["card"], serde_json::Value::Null);
        assert_eq!(v["deletedAt"], serde_json::Value::Null);
        assert!(v["passwordHistory"].is_array());
        let uri = LoginUri {
            uri: "https://a".into(),
            match_type: UriMatch::StartsWith,
        };
        assert_eq!(serde_json::to_value(&uri).unwrap()["match"], "startsWith");
        // Partial JSON from the UI fills in defaults.
        let parsed: VaultItem = serde_json::from_str(r#"{"type":"note","name":"n"}"#).unwrap();
        assert_eq!(parsed.item_type, ItemType::Note);
        assert!(parsed.login.is_none());
    }

    #[test]
    fn normalize_matches_type() {
        let mut item = VaultItem::new(ItemType::Login, "x");
        item.password_history.push(PasswordHistoryEntry::default());
        item.item_type = ItemType::Card;
        item.normalize();
        assert!(item.login.is_none() && item.card.is_some() && item.identity.is_none());
        assert!(item.password_history.is_empty());
        item.item_type = ItemType::Note;
        item.normalize();
        assert!(item.login.is_none() && item.card.is_none() && item.identity.is_none());
        item.item_type = ItemType::Identity;
        item.normalize();
        assert!(item.identity.is_some());
    }

    #[test]
    fn summaries() {
        let mut login = VaultItem::new(ItemType::Login, "Mail");
        if let Some(l) = login.login.as_mut() {
            l.username = "me@example.com".into();
            l.uris.push(LoginUri {
                uri: "https://mail.example.com".into(),
                match_type: UriMatch::Domain,
            });
            l.totp = "JBSWY3DPEHPK3PXP".into();
        }
        let s = login.summary();
        assert_eq!(s.subtitle, "me@example.com");
        assert_eq!(s.uri, "https://mail.example.com");
        assert!(s.has_totp);

        let mut card = VaultItem::new(ItemType::Card, "Visa");
        if let Some(c) = card.card.as_mut() {
            c.number = "4111 1111 1111 1234".into();
        }
        assert_eq!(card.summary().subtitle, "•••• 1234");
        if let Some(c) = card.card.as_mut() {
            c.number = "12".into();
            c.brand = "Visa".into();
        }
        assert_eq!(card.summary().subtitle, "Visa");

        let mut ident = VaultItem::new(ItemType::Identity, "Me");
        if let Some(i) = ident.identity.as_mut() {
            i.first_name = "Max".into();
            i.last_name = "Muster".into();
        }
        assert_eq!(ident.summary().subtitle, "Max Muster");
        assert_eq!(VaultItem::new(ItemType::Note, "n").summary().subtitle, "");
    }
}
