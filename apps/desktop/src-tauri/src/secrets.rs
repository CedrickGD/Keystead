//! What the webview gets to see of the items (see "Secrets and the webview"
//! in docs/ARCHITECTURE.md).
//!
//! `list_items` and `save_item` answer redacted [`ItemListEntry`]s: no
//! password, TOTP key, card number/code, hidden custom field value or old
//! password – only flags saying whether there is one. A secret leaves the
//! backend only when the user asks for it: `reveal_secret` (one value, for
//! an eye toggle) and `get_item_for_edit` (the full item, for the editor).
//! `copy_secret_field` copies a secret without sending it to the webview,
//! and `totp_for_item` sends only the current code, never the key.

use keystead_core::model::{FieldKind, IdentityData, ItemType, LoginUri, VaultItem};
use keystead_core::totp::{self, TotpCode};
use keystead_core::{Error as CoreError, UnlockedVault};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::commands::{parse, run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::state::Core;
use crate::wipe::{Wipe, Wiped};

// ---------------------------------------------------------------------------
// Redacted list entries
// ---------------------------------------------------------------------------

/// The place of a secret in a list entry: always serializes as `""`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Redacted;

impl Serialize for Redacted {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("")
    }
}

/// An item as `list_items` / `save_item` hand it to the UI: [`VaultItem`]
/// without its secrets (`ItemListEntry` in `src/lib/types.ts`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemListEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub name: String,
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub notes: String,
    pub login: Option<LoginListData>,
    pub card: Option<CardListData>,
    pub identity: Option<IdentityData>,
    pub fields: Vec<FieldListEntry>,
    pub password_history: Vec<HistoryListEntry>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginListData {
    pub username: String,
    pub password: Redacted,
    pub has_password: bool,
    pub uris: Vec<LoginUri>,
    pub totp: Redacted,
    pub has_totp: bool,
    pub password_revised_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardListData {
    pub cardholder_name: String,
    pub brand: String,
    /// [`masked_card_number`]: "•••• 1234", "••••" or "".
    pub number: String,
    pub has_number: bool,
    pub exp_month: String,
    pub exp_year: String,
    pub code: Redacted,
    pub has_code: bool,
}

/// A custom field; the value of a `hidden` one is `""`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldListEntry {
    pub name: String,
    pub value: String,
    pub kind: FieldKind,
    pub has_value: bool,
}

/// An old password: only when it was replaced.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryListEntry {
    pub replaced_at: i64,
}

/// Card numbers in list entries: "•••• " + the last 4 digits if the number
/// has at least 8 digits (never half of a short number), "••••" for any
/// other non-empty number, "" for none.
pub fn masked_card_number(number: &str) -> String {
    let digits: Zeroizing<Vec<char>> =
        Zeroizing::new(number.chars().filter(char::is_ascii_digit).collect());
    if digits.len() >= 8 {
        let last4: String = digits[digits.len() - 4..].iter().collect();
        format!("•••• {last4}")
    } else if number.trim().is_empty() {
        String::new()
    } else {
        "••••".to_owned()
    }
}

impl From<&VaultItem> for ItemListEntry {
    fn from(item: &VaultItem) -> Self {
        ItemListEntry {
            id: item.id.clone(),
            item_type: item.item_type,
            name: item.name.clone(),
            folder_id: item.folder_id.clone(),
            favorite: item.favorite,
            notes: item.notes.clone(),
            login: item.login.as_ref().map(|l| LoginListData {
                username: l.username.clone(),
                password: Redacted,
                has_password: !l.password.is_empty(),
                uris: l.uris.clone(),
                totp: Redacted,
                has_totp: !l.totp.trim().is_empty(),
                password_revised_at: l.password_revised_at,
            }),
            card: item.card.as_ref().map(|c| CardListData {
                cardholder_name: c.cardholder_name.clone(),
                brand: c.brand.clone(),
                number: masked_card_number(&c.number),
                has_number: !c.number.trim().is_empty(),
                exp_month: c.exp_month.clone(),
                exp_year: c.exp_year.clone(),
                code: Redacted,
                has_code: !c.code.is_empty(),
            }),
            identity: item.identity.clone(),
            fields: item
                .fields
                .iter()
                .map(|f| FieldListEntry {
                    name: f.name.clone(),
                    value: if f.kind == FieldKind::Hidden {
                        String::new()
                    } else {
                        f.value.clone()
                    },
                    kind: f.kind,
                    has_value: !f.value.is_empty(),
                })
                .collect(),
            password_history: item
                .password_history
                .iter()
                .map(|h| HistoryListEntry {
                    replaced_at: h.replaced_at,
                })
                .collect(),
            created_at: item.created_at,
            updated_at: item.updated_at,
            deleted_at: item.deleted_at,
        }
    }
}

/// Names, usernames, websites, notes and identities are no secrets in the
/// sense of the redaction, but personal: wiped like the full items were.
impl Wipe for ItemListEntry {
    fn wipe(&mut self) {
        self.name.zeroize();
        self.notes.zeroize();
        if let Some(l) = self.login.as_mut() {
            l.username.zeroize();
            for u in &mut l.uris {
                u.uri.zeroize();
            }
        }
        if let Some(c) = self.card.as_mut() {
            for s in [
                &mut c.cardholder_name,
                &mut c.brand,
                &mut c.number,
                &mut c.exp_month,
                &mut c.exp_year,
            ] {
                s.zeroize();
            }
        }
        if let Some(i) = self.identity.as_mut() {
            for s in [
                &mut i.title,
                &mut i.first_name,
                &mut i.last_name,
                &mut i.email,
                &mut i.phone,
                &mut i.company,
                &mut i.address1,
                &mut i.address2,
                &mut i.postal_code,
                &mut i.city,
                &mut i.state,
                &mut i.country,
                &mut i.username,
            ] {
                s.zeroize();
            }
        }
        for f in &mut self.fields {
            f.name.zeroize();
            f.value.zeroize();
        }
    }
}

/// All items of the vault (incl. trash), redacted (`list_items`).
pub fn list_entries(vault: &UnlockedVault) -> Vec<ItemListEntry> {
    vault.items().iter().map(ItemListEntry::from).collect()
}

/// The redacted answer to `save_item`; the saved full item is wiped.
pub fn saved_entry(saved: VaultItem) -> ItemListEntry {
    let saved = Wiped(saved);
    ItemListEntry::from(&saved.0)
}

// ---------------------------------------------------------------------------
// One secret at a time
// ---------------------------------------------------------------------------

/// Which secret of an item `reveal_secret` / `copy_secret_field` mean. JSON:
/// `"password"`, `"totp"` (the stored key), `"totpCode"` (the current code),
/// `"cardNumber"`, `"cardCode"`, `{ "custom": i }` (`fields[i]`),
/// `{ "history": i }` (`passwordHistory[i]`, newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretField {
    Password,
    Totp,
    TotpCode,
    CardNumber,
    CardCode,
    Custom(usize),
    History(usize),
}

fn not_found(what: &str) -> AppError {
    CoreError::NotFound(what.to_owned()).into()
}

/// The item `item_id` of the page's vault (`locked` / `not_found`).
fn page_item<'a>(
    vault: Result<&'a UnlockedVault, AppError>,
    item_id: &str,
) -> AppResult<&'a VaultItem> {
    vault?.item(item_id).ok_or_else(|| not_found("item"))
}

/// The value of `field`; `not_found` if the item has no such field (another
/// item type, an index out of range, no TOTP key for `totpCode`).
fn secret_of(item: &VaultItem, field: SecretField) -> AppResult<Zeroizing<String>> {
    let value = match field {
        SecretField::Password => item.login.as_ref().map(|l| l.password.as_str()),
        SecretField::Totp => item.login.as_ref().map(|l| l.totp.as_str()),
        SecretField::TotpCode => {
            let seed = item
                .login
                .as_ref()
                .map(|l| l.totp.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| not_found("totp"))?;
            let code = totp::totp_now(seed)?;
            return Ok(Zeroizing::new(code.code));
        }
        SecretField::CardNumber => item.card.as_ref().map(|c| c.number.as_str()),
        SecretField::CardCode => item.card.as_ref().map(|c| c.code.as_str()),
        SecretField::Custom(i) => item.fields.get(i).map(|f| f.value.as_str()),
        SecretField::History(i) => item.password_history.get(i).map(|h| h.password.as_str()),
    };
    value
        .map(|v| Zeroizing::new(v.to_owned()))
        .ok_or_else(|| not_found("field"))
}

/// `reveal_secret`: the value of one field of an item of the page's vault.
pub fn reveal(
    core: &Core,
    page_vault_id: Option<&str>,
    item_id: &str,
    field: SecretField,
) -> AppResult<Zeroizing<String>> {
    let st = core.state();
    secret_of(page_item(st.page_vault(page_vault_id), item_id)?, field)
}

/// `copy_secret_field`: hands the value to `copy` (the sensitive clipboard
/// path) after the state lock is released, with the lock epoch read under
/// that lock (`Core::copy_to_clipboard_since` copies nothing if the vault
/// was locked or replaced in between). A card number is copied without its
/// spaces; an empty value is `not_found` (nothing to copy).
pub fn copy_secret_with(
    core: &Core,
    page_vault_id: Option<&str>,
    item_id: &str,
    field: SecretField,
    copy: impl FnOnce(&str, u64) -> AppResult<()>,
) -> AppResult<()> {
    let (mut secret, epoch) = {
        let st = core.state();
        let secret = secret_of(page_item(st.page_vault(page_vault_id), item_id)?, field)?;
        (secret, core.lock_epoch())
    };
    if field == SecretField::CardNumber {
        secret.retain(|c| !c.is_whitespace());
    }
    if secret.is_empty() {
        return Err(not_found("field"));
    }
    copy(&secret, epoch)
}

/// `totp_for_item`: the current code of a login's TOTP key (never the key).
pub fn item_totp(core: &Core, page_vault_id: Option<&str>, item_id: &str) -> AppResult<TotpCode> {
    let st = core.state();
    let item = page_item(st.page_vault(page_vault_id), item_id)?;
    let seed = item
        .login
        .as_ref()
        .map(|l| l.totp.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| not_found("totp"))?;
    Ok(totp::totp_now(seed)?)
}

/// `get_item_for_edit`: the full item, for the editor.
pub fn item_for_edit(
    core: &Core,
    page_vault_id: Option<&str>,
    item_id: &str,
) -> AppResult<VaultItem> {
    let st = core.state();
    Ok(page_item(st.page_vault(page_vault_id), item_id)?.clone())
}

/// `set_favorite`: (un)marks an item as favourite without the UI sending
/// the full item back.
pub fn set_favorite_for_page(
    core: &Core,
    page_vault_id: Option<&str>,
    item_id: &str,
    favorite: bool,
) -> AppResult<ItemListEntry> {
    let saved = core.mutate_for_page(page_vault_id, |v| {
        // `save_item` would add an unknown id as a new item.
        let mut item = Wiped(
            v.item(item_id)
                .cloned()
                .ok_or_else(|| CoreError::NotFound("item".to_owned()))?,
        );
        item.0.favorite = favorite;
        v.save_item(item.0.clone())
    })?;
    Ok(saved_entry(saved))
}

impl Wipe for TotpCode {
    fn wipe(&mut self) {
        self.code.zeroize();
    }
}

// ---------------------------------------------------------------------------
// Commands (all take `pageVaultId`: another open vault is `locked`)
// ---------------------------------------------------------------------------

/// `reveal_secret(itemId, field, pageVaultId)` → the value (eye toggle).
#[tauri::command]
pub async fn reveal_secret(
    core: Shared<'_>,
    item_id: String,
    field: Value,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<String>> {
    run(&core, true, move |c| {
        let field: SecretField = parse(field, "field")?;
        let secret = reveal(c, page_vault_id.as_deref(), &item_id, field)?;
        Ok(Wiped(secret.to_string()))
    })
    .await
}

/// `copy_secret_field(itemId, field, pageVaultId)` → `null`: copies like
/// `copy_text` with `sensitive`; the value never reaches the webview.
#[tauri::command]
pub async fn copy_secret_field(
    core: Shared<'_>,
    item_id: String,
    field: Value,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        let field: SecretField = parse(field, "field")?;
        copy_secret_with(
            c,
            page_vault_id.as_deref(),
            &item_id,
            field,
            |text, epoch| c.copy_to_clipboard_since(text, true, Some(epoch)),
        )
    })
    .await
}

/// `totp_for_item(itemId, pageVaultId)` → `TotpCode`. Polled by the UI every
/// period: not an activity.
#[tauri::command]
pub async fn totp_for_item(
    core: Shared<'_>,
    item_id: String,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<TotpCode>> {
    run(&core, false, move |c| {
        item_totp(c, page_vault_id.as_deref(), &item_id).map(Wiped)
    })
    .await
}

/// `get_item_for_edit(itemId, pageVaultId)` → the full `VaultItem`.
#[tauri::command]
pub async fn get_item_for_edit(
    core: Shared<'_>,
    item_id: String,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<VaultItem>> {
    run(&core, true, move |c| {
        item_for_edit(c, page_vault_id.as_deref(), &item_id).map(Wiped)
    })
    .await
}

/// `set_favorite(itemId, favorite, pageVaultId)` † → `ItemListEntry`.
#[tauri::command]
pub async fn set_favorite(
    core: Shared<'_>,
    item_id: String,
    favorite: bool,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<ItemListEntry>> {
    run(&core, true, move |c| {
        set_favorite_for_page(c, page_vault_id.as_deref(), &item_id, favorite).map(Wiped)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use keystead_core::model::{CardData, CustomField, IdentityData, LoginData, UriMatch};
    use keystead_core::settings::Settings;

    use super::*;
    use crate::state::test_support::core_with_open_vault;

    const SECRETS: &[&str] = &[
        "hunter2-current",
        "old-pass-1",
        "JBSWY3DPEHPK3PXP",
        "4111222233334444",
        "4111 2222 3333 4444",
        "9876",
        "hidden-pin-5521",
        "trash-pass-77",
        "note-card-code",
        "12345",
    ];

    struct Ids {
        login: String,
        card: String,
        short_card: String,
        identity: String,
        note: String,
        trashed: String,
    }

    fn vault_id(c: &Core) -> String {
        c.state().vault.as_ref().unwrap().id().to_owned()
    }

    fn save(c: &Core, item: VaultItem) -> VaultItem {
        c.mutate(|v| v.save_item(item.clone())).unwrap()
    }

    /// A vault with every kind of secret: a login with password, TOTP key,
    /// hidden field and history; cards; an identity; a note; a trashed login.
    fn fill(c: &Core) -> Ids {
        let mut login = VaultItem::new(ItemType::Login, "Bank");
        login.notes = "plain notes".into();
        login.login = Some(LoginData {
            username: "alice".into(),
            password: "old-pass-1".into(),
            uris: vec![LoginUri {
                uri: "https://bank.example".into(),
                match_type: UriMatch::Domain,
            }],
            totp: "JBSWY3DPEHPK3PXP".into(),
            password_revised_at: None,
        });
        login.fields = vec![
            CustomField {
                name: "Customer no".into(),
                value: "C-100".into(),
                kind: FieldKind::Text,
            },
            CustomField {
                name: "PIN".into(),
                value: "hidden-pin-5521".into(),
                kind: FieldKind::Hidden,
            },
            CustomField {
                name: "Active".into(),
                value: "true".into(),
                kind: FieldKind::Boolean,
            },
            CustomField {
                name: "Empty secret".into(),
                value: String::new(),
                kind: FieldKind::Hidden,
            },
        ];
        let mut login = save(c, login);
        // Changing the password moves the old one into the history.
        login.login.as_mut().unwrap().password = "hunter2-current".into();
        let login = save(c, login);
        assert_eq!(login.password_history.len(), 1);

        let mut card = VaultItem::new(ItemType::Card, "Visa");
        card.card = Some(CardData {
            cardholder_name: "Alice Doe".into(),
            brand: "Visa".into(),
            number: "4111 2222 3333 4444".into(),
            exp_month: "08".into(),
            exp_year: "2028".into(),
            code: "9876".into(),
        });
        let card = save(c, card);

        let mut short_card = VaultItem::new(ItemType::Card, "Club card");
        short_card.card = Some(CardData {
            number: "12345".into(),
            code: "note-card-code".into(),
            ..CardData::default()
        });
        let short_card = save(c, short_card);

        let mut identity = VaultItem::new(ItemType::Identity, "Me");
        identity.identity = Some(IdentityData {
            first_name: "Alice".into(),
            email: "alice@example.com".into(),
            ..IdentityData::default()
        });
        let identity = save(c, identity);

        let mut note = VaultItem::new(ItemType::Note, "Note");
        note.notes = "note text".into();
        let note = save(c, note);

        let mut trashed = VaultItem::new(ItemType::Login, "Old");
        trashed.login.as_mut().unwrap().password = "trash-pass-77".into();
        let trashed = save(c, trashed);
        c.mutate(|v| v.trash_item(&trashed.id)).unwrap();

        Ids {
            login: login.id,
            card: card.id,
            short_card: short_card.id,
            identity: identity.id,
            note: note.id,
            trashed: trashed.id,
        }
    }

    fn entry<'a>(entries: &'a [ItemListEntry], id: &str) -> &'a ItemListEntry {
        entries.iter().find(|e| e.id == id).unwrap()
    }

    #[test]
    fn list_entries_contain_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let ids = fill(&c);
        let entries = c.read(list_entries).unwrap();
        assert_eq!(entries.len(), 6, "trash included");
        let json = serde_json::to_string(&Wiped(entries.clone())).unwrap();
        for secret in SECRETS {
            assert!(!json.contains(secret), "{secret} in {json}");
        }
        // What the list needs is still there.
        for kept in [
            "Bank",
            "alice",
            "bank.example",
            "plain notes",
            "C-100",
            "Alice Doe",
            "alice@example.com",
            "note text",
        ] {
            assert!(json.contains(kept), "{kept} missing in {json}");
        }

        let value = serde_json::to_value(entry(&entries, &ids.login)).unwrap();
        let login = &value["login"];
        assert_eq!(login["password"], "");
        assert_eq!(login["hasPassword"], true);
        assert_eq!(login["totp"], "");
        assert_eq!(login["hasTotp"], true);
        assert_eq!(login["username"], "alice");
        assert_eq!(login["uris"][0]["match"], "domain");
        assert!(login["passwordRevisedAt"].is_i64());
        assert_eq!(value["type"], "login");
        assert_eq!(
            value["fields"],
            serde_json::json!([
                { "name": "Customer no", "value": "C-100", "kind": "text", "hasValue": true },
                { "name": "PIN", "value": "", "kind": "hidden", "hasValue": true },
                { "name": "Active", "value": "true", "kind": "boolean", "hasValue": true },
                { "name": "Empty secret", "value": "", "kind": "hidden", "hasValue": false },
            ])
        );
        let history = value["passwordHistory"].as_array().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].as_object().unwrap().len(), 1, "{history:?}");
        assert!(history[0]["replacedAt"].is_i64());

        let card = serde_json::to_value(entry(&entries, &ids.card)).unwrap();
        assert_eq!(card["card"]["number"], "•••• 4444");
        assert_eq!(card["card"]["hasNumber"], true);
        assert_eq!(card["card"]["code"], "");
        assert_eq!(card["card"]["hasCode"], true);
        assert_eq!(card["card"]["brand"], "Visa");
        assert_eq!(card["login"], Value::Null);
        // A short number is not half revealed.
        let short = entry(&entries, &ids.short_card).card.clone().unwrap();
        assert_eq!(short.number, "••••");
        assert!(short.has_number && short.has_code);

        let trashed = entry(&entries, &ids.trashed);
        assert!(trashed.deleted_at.is_some());
        assert!(trashed.login.as_ref().unwrap().has_password);
        let note = entry(&entries, &ids.note);
        assert!(note.login.is_none() && note.card.is_none() && note.identity.is_none());
        assert!(entry(&entries, &ids.identity).identity.is_some());
        let empty = VaultItem::new(ItemType::Login, "Empty");
        let empty = ItemListEntry::from(&empty).login.unwrap();
        assert!(!empty.has_password && !empty.has_totp);
    }

    #[test]
    fn masked_card_numbers() {
        assert_eq!(masked_card_number("4111 1111 1111 1234"), "•••• 1234");
        assert_eq!(masked_card_number("378282246310005"), "•••• 0005");
        assert_eq!(masked_card_number("1234-5678"), "•••• 5678");
        assert_eq!(masked_card_number("1234567"), "••••");
        assert_eq!(masked_card_number("abc"), "••••");
        assert_eq!(masked_card_number("  "), "");
        assert_eq!(masked_card_number(""), "");
    }

    #[test]
    fn saved_entries_are_redacted() {
        let mut item = VaultItem::new(ItemType::Login, "Bank");
        item.login.as_mut().unwrap().password = "hunter2-current".into();
        let json = serde_json::to_string(&saved_entry(item)).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
        assert!(json.contains("\"hasPassword\":true"), "{json}");
    }

    #[test]
    fn secret_fields_parse_from_the_ui_values() {
        let p = |v: Value| parse::<SecretField>(v, "field");
        assert_eq!(p("password".into()).unwrap(), SecretField::Password);
        assert_eq!(p("totp".into()).unwrap(), SecretField::Totp);
        assert_eq!(p("totpCode".into()).unwrap(), SecretField::TotpCode);
        assert_eq!(p("cardNumber".into()).unwrap(), SecretField::CardNumber);
        assert_eq!(p("cardCode".into()).unwrap(), SecretField::CardCode);
        assert_eq!(
            p(serde_json::json!({ "custom": 2 })).unwrap(),
            SecretField::Custom(2)
        );
        assert_eq!(
            p(serde_json::json!({ "history": 0 })).unwrap(),
            SecretField::History(0)
        );
        for bad in [
            serde_json::json!("pin"),
            serde_json::json!({ "custom": -1 }),
            serde_json::json!({ "custom": "1" }),
            serde_json::json!({ "history": 1, "custom": 1 }),
            serde_json::json!(null),
        ] {
            assert_eq!(p(bad).unwrap_err().code(), "invalid_input:field");
        }
    }

    #[test]
    fn reveal_returns_the_requested_secret() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let ids = fill(&c);
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let r = |id: &str, field| reveal(&c, page, id, field).map(|s| s.to_string());

        assert_eq!(
            r(&ids.login, SecretField::Password).unwrap(),
            "hunter2-current"
        );
        assert_eq!(
            r(&ids.login, SecretField::Totp).unwrap(),
            "JBSWY3DPEHPK3PXP"
        );
        let code = r(&ids.login, SecretField::TotpCode).unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|ch| ch.is_ascii_digit()));
        assert_eq!(
            r(&ids.login, SecretField::Custom(1)).unwrap(),
            "hidden-pin-5521"
        );
        assert_eq!(r(&ids.login, SecretField::Custom(0)).unwrap(), "C-100");
        assert_eq!(r(&ids.login, SecretField::Custom(3)).unwrap(), "");
        assert_eq!(
            r(&ids.login, SecretField::History(0)).unwrap(),
            "old-pass-1"
        );
        assert_eq!(
            r(&ids.card, SecretField::CardNumber).unwrap(),
            "4111 2222 3333 4444"
        );
        assert_eq!(r(&ids.card, SecretField::CardCode).unwrap(), "9876");
        // Trashed items can be looked at, too.
        assert_eq!(
            r(&ids.trashed, SecretField::Password).unwrap(),
            "trash-pass-77"
        );

        // Fields the item does not have, unknown items.
        for (id, field) in [
            (ids.login.as_str(), SecretField::CardNumber),
            (ids.login.as_str(), SecretField::Custom(4)),
            (ids.login.as_str(), SecretField::History(1)),
            (ids.card.as_str(), SecretField::Password),
            (ids.card.as_str(), SecretField::TotpCode),
            (ids.trashed.as_str(), SecretField::TotpCode),
            (ids.identity.as_str(), SecretField::Totp),
            (ids.note.as_str(), SecretField::History(0)),
            ("no-such-item", SecretField::Password),
        ] {
            assert_eq!(r(id, field).unwrap_err().code(), "not_found", "{field:?}");
        }
    }

    #[test]
    fn copy_hands_the_secret_to_the_clipboard_path_only() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let ids = fill(&c);
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let copied = RefCell::new(Vec::<String>::new());
        let copy = |id: &str, field| {
            copy_secret_with(&c, page, id, field, |text, _| {
                copied.borrow_mut().push(text.to_owned());
                Ok(())
            })
        };

        copy(&ids.login, SecretField::Password).unwrap();
        copy(&ids.login, SecretField::Custom(1)).unwrap();
        copy(&ids.login, SecretField::History(0)).unwrap();
        copy(&ids.card, SecretField::CardNumber).unwrap();
        copy(&ids.card, SecretField::CardCode).unwrap();
        copy(&ids.login, SecretField::TotpCode).unwrap();
        let copied_now = copied.borrow().clone();
        assert_eq!(
            copied_now[..5],
            [
                "hunter2-current",
                "hidden-pin-5521",
                "old-pass-1",
                "4111222233334444",
                "9876"
            ]
        );
        assert_eq!(copied_now[5].len(), 6);

        // Nothing to copy, or nothing there: the clipboard is not touched.
        assert_eq!(
            copy(&ids.login, SecretField::Custom(3)).unwrap_err().code(),
            "not_found"
        );
        assert_eq!(
            copy("no-such-item", SecretField::Password)
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert_eq!(copied.borrow().len(), 6);

        // A failing clipboard is reported.
        let err = copy_secret_with(&c, page, &ids.login, SecretField::Password, |_, _| {
            Err(AppError::io("clipboard"))
        })
        .unwrap_err();
        assert_eq!(err.code(), "io:clipboard");
    }

    #[test]
    fn totp_and_edit_give_what_the_ui_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let ids = fill(&c);
        let page = vault_id(&c);
        let page = Some(page.as_str());

        let code = item_totp(&c, page, &ids.login).unwrap();
        assert_eq!(code.code.len(), 6);
        assert_eq!(code.period, 30);
        let json = serde_json::to_string(&Wiped(code)).unwrap();
        assert!(!json.contains("JBSWY3DPEHPK3PXP"), "{json}");
        for id in [&ids.card, &ids.trashed, &ids.note] {
            assert_eq!(item_totp(&c, page, id).unwrap_err().code(), "not_found");
        }

        let full = item_for_edit(&c, page, &ids.login).unwrap();
        let stored = c.read(|v| v.item(&ids.login).cloned()).unwrap().unwrap();
        assert_eq!(full, stored);
        assert_eq!(full.password(), "hunter2-current");
        assert_eq!(full.password_history[0].password, "old-pass-1");
        assert_eq!(
            item_for_edit(&c, page, "no-such-item").unwrap_err().code(),
            "not_found"
        );
    }

    #[test]
    fn favorite_is_set_without_touching_the_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let ids = fill(&c);
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let before = item_for_edit(&c, page, &ids.login).unwrap();

        let entry = set_favorite_for_page(&c, page, &ids.login, true).unwrap();
        assert!(entry.favorite);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
        let after = item_for_edit(&c, page, &ids.login).unwrap();
        assert!(after.favorite);
        assert_eq!(after.login, before.login);
        assert_eq!(after.password_history, before.password_history);
        assert_eq!(after.fields, before.fields);
        assert!(
            !set_favorite_for_page(&c, page, &ids.login, false)
                .unwrap()
                .favorite
        );

        // An unknown id is not added as a new item.
        let count = c.read(|v| v.items().len()).unwrap();
        assert_eq!(
            set_favorite_for_page(&c, page, "no-such-item", true)
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert_eq!(c.read(|v| v.items().len()).unwrap(), count);
    }

    #[test]
    fn page_commands_for_another_vault_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let ids = fill(&c);
        let copied = RefCell::new(0);
        for page in [None, Some("other-vault")] {
            assert_eq!(
                reveal(&c, page, &ids.login, SecretField::Password)
                    .unwrap_err()
                    .code(),
                "locked"
            );
            let err = copy_secret_with(&c, page, &ids.login, SecretField::Password, |_, _| {
                *copied.borrow_mut() += 1;
                Ok(())
            })
            .unwrap_err();
            assert_eq!(err.code(), "locked");
            assert_eq!(
                item_totp(&c, page, &ids.login).unwrap_err().code(),
                "locked"
            );
            assert_eq!(
                item_for_edit(&c, page, &ids.login).unwrap_err().code(),
                "locked"
            );
            assert_eq!(
                set_favorite_for_page(&c, page, &ids.login, true)
                    .unwrap_err()
                    .code(),
                "locked"
            );
        }
        assert_eq!(*copied.borrow(), 0);
        assert!(!c.read(|v| v.item(&ids.login).unwrap().favorite).unwrap());

        // Locked: the same answer, also for the page's own vault id.
        let page = vault_id(&c);
        c.lock(None);
        assert_eq!(
            reveal(&c, Some(&page), &ids.login, SecretField::Password)
                .unwrap_err()
                .code(),
            "locked"
        );
        assert_eq!(
            item_for_edit(&c, Some(&page), &ids.login)
                .unwrap_err()
                .code(),
            "locked"
        );
    }
}
