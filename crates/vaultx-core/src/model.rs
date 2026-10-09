//! Plain data model stored (encrypted) inside a vault file.
//!
//! This file is the shared contract between the core, the desktop backend,
//! the frontend (`apps/desktop/src/lib/types.ts` mirrors it 1:1) and the
//! terminal UI. All JSON uses camelCase keys. Timestamps are Unix epoch
//! milliseconds (UTC).

use serde::{Deserialize, Serialize};

/// Everything inside the encrypted payload of a vault.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VaultData {
    pub items: Vec<VaultItem>,
    pub folders: Vec<Folder>,
    /// Last generated passwords (newest first, max 50). Only kept locally.
    pub generator_history: Vec<GeneratedPassword>,
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
}

/// Current time in Unix epoch milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
