//! Exporters: encrypted Keystead file, Bitwarden-style CSV (spreadsheet
//! formulas neutralised, see [`export_csv`]) and unencrypted
//! Bitwarden-compatible JSON. Trashed items and the generator history are
//! never exported.

use std::borrow::Cow;
use std::path::Path;

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams};
use crate::error::{Error, Result};
use crate::format::{self, VaultFile};
use crate::model::{FieldKind, ItemType, UriMatch, VaultData, VaultItem};
use crate::util;

/// Column layout of Bitwarden's CSV export.
pub const BITWARDEN_CSV_HEADER: [&str; 11] = [
    "folder",
    "favorite",
    "type",
    "name",
    "notes",
    "fields",
    "reprompt",
    "login_uri",
    "login_username",
    "login_password",
    "login_totp",
];

fn active_items(data: &VaultData) -> impl Iterator<Item = &VaultItem> {
    data.items.iter().filter(|i| !i.is_trashed())
}

/// Writes a standalone encrypted Keystead file (new id, own random key and
/// salt, default KDF parameters) readable by
/// [`crate::import::import_keystead_export`].
pub fn export_encrypted(data: &VaultData, path: &Path, password: &str) -> Result<()> {
    export_encrypted_with_params(data, path, password, KdfParams::default())
}

/// [`export_encrypted`] with explicit KDF parameters.
pub fn export_encrypted_with_params(
    data: &VaultData,
    path: &Path,
    password: &str,
    kdf: KdfParams,
) -> Result<()> {
    if password.is_empty() {
        return Err(Error::invalid("password_empty"));
    }
    let export = VaultData {
        items: active_items(data).cloned().collect(),
        folders: data.folders.clone(),
        generator_history: Vec::new(),
    };
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Keystead Export".to_owned());
    let key = crypto::random_key()?;
    let file = VaultFile::create(&util::new_id(), &name, password, kdf, &key, &export)?;
    file.write_atomic(path, false)
}

fn field_value(kind: FieldKind, value: &str) -> String {
    match kind {
        FieldKind::Boolean => {
            if value.trim().eq_ignore_ascii_case("true") {
                "true".to_owned()
            } else {
                "false".to_owned()
            }
        }
        _ => value.to_owned(),
    }
}

/// Characters that make a spreadsheet (Excel, LibreOffice, Google Sheets)
/// treat a cell as a formula (`\t`/`\r` only at the very start).
fn is_formula_start(c: char) -> bool {
    matches!(c, '=' | '+' | '-' | '@')
}

/// True if a spreadsheet could evaluate `s` as a formula: it starts with
/// `= + - @ \t \r`, or with `= + - @` after leading whitespace.
pub(crate) fn csv_formula_like(s: &str) -> bool {
    s.starts_with(['\t', '\r']) || s.trim_start().starts_with(is_formula_start)
}

/// Username variant of [`csv_formula_like`]: only `=` and `@` (`+`/`-`
/// start real phone numbers and handles, which then stay unchanged).
pub(crate) fn csv_username_formula_like(s: &str) -> bool {
    s.trim_start().starts_with(['=', '@'])
}

/// Neutralises spreadsheet formulas ("CSV injection"): prefixes `'` if
/// `formula_like(s)` holds – or if `s` already starts with `'`s followed by
/// such a value, so that [`csv_unescape`] restores every value exactly.
fn csv_escape(s: &str, formula_like: fn(&str) -> bool) -> Cow<'_, str> {
    let rest = s.trim_start_matches('\'');
    if formula_like(rest) {
        Cow::Owned(format!("'{s}"))
    } else {
        Cow::Borrowed(s)
    }
}

/// Reverses [`csv_escape`] (used by the CSV import for Bitwarden-style files).
pub(crate) fn csv_unescape(s: &str, formula_like: fn(&str) -> bool) -> &str {
    match s.strip_prefix('\'') {
        Some(rest) if formula_like(rest.trim_start_matches('\'')) => rest,
        _ => s,
    }
}

/// Bitwarden-style CSV of all logins and secure notes (cards and identities
/// cannot be represented in this format and are left out).
///
/// Values that a spreadsheet would evaluate as a formula get a leading `'`
/// (the usual "CSV injection" defence): folder, name, notes, custom field
/// names, URIs, and usernames starting with `=` or `@`. Passwords and TOTP
/// secrets are written unchanged so the file still imports correctly into
/// other password managers – the file is not meant to be opened in a
/// spreadsheet. Keystead's own CSV import removes the prefix again.
pub fn export_csv(data: &VaultData) -> String {
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_writer(Vec::new());
    // Writing into a Vec cannot fail; errors are ignored defensively.
    let _ = w.write_record(BITWARDEN_CSV_HEADER);
    for item in active_items(data) {
        let kind = match item.item_type {
            ItemType::Login => "login",
            ItemType::Note => "note",
            ItemType::Card | ItemType::Identity => continue,
        };
        let folder = item
            .folder_id
            .as_deref()
            .and_then(|id| data.folders.iter().find(|f| f.id == id))
            .map(|f| f.name.as_str())
            .unwrap_or("");
        // Each line starts with a field name: escaping the names covers the
        // start of the cell (and of every line).
        let fields = Zeroizing::new(
            item.fields
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}",
                        csv_escape(&f.name, csv_formula_like),
                        field_value(f.kind, &f.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let (uris, username, password, totp) = match &item.login {
            Some(l) => (
                l.uris
                    .iter()
                    .map(|u| u.uri.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                l.username.as_str(),
                l.password.as_str(),
                l.totp.as_str(),
            ),
            None => (String::new(), "", "", ""),
        };
        let name = csv_escape(&item.name, csv_formula_like);
        let notes = Zeroizing::new(csv_escape(&item.notes, csv_formula_like).into_owned());
        let username = Zeroizing::new(csv_escape(username, csv_username_formula_like).into_owned());
        let _ = w.write_record([
            csv_escape(folder, csv_formula_like).as_ref(),
            if item.favorite { "1" } else { "" },
            kind,
            name.as_ref(),
            notes.as_str(),
            fields.as_str(),
            "0",
            csv_escape(&uris, csv_formula_like).as_ref(),
            username.as_str(),
            password,
            totp,
        ]);
    }
    let bytes = w.into_inner().unwrap_or_default();
    String::from_utf8(bytes).unwrap_or_default()
}

fn opt(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s.to_owned())
    }
}

fn iso(ms: i64) -> Value {
    Value::String(util::format_iso8601_ms(ms))
}

fn bw_match(m: UriMatch) -> Value {
    match m {
        UriMatch::Domain => Value::Null,
        UriMatch::Host => json!(1),
        UriMatch::StartsWith => json!(2),
        UriMatch::Exact => json!(3),
        UriMatch::Never => json!(5),
    }
}

fn bw_field_type(k: FieldKind) -> u8 {
    match k {
        FieldKind::Text => 0,
        FieldKind::Hidden => 1,
        FieldKind::Boolean => 2,
    }
}

fn bw_item(item: &VaultItem) -> Value {
    let type_num = match item.item_type {
        ItemType::Login => 1,
        ItemType::Note => 2,
        ItemType::Card => 3,
        ItemType::Identity => 4,
    };
    let fields: Vec<Value> = item
        .fields
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "value": field_value(f.kind, &f.value),
                "type": bw_field_type(f.kind),
                "linkedId": null,
            })
        })
        .collect();
    let history: Vec<Value> = item
        .password_history
        .iter()
        .map(|h| json!({ "lastUsedDate": iso(h.replaced_at), "password": h.password }))
        .collect();
    let mut v = json!({
        "passwordHistory": if history.is_empty() { Value::Null } else { Value::Array(history) },
        "revisionDate": iso(item.updated_at),
        "creationDate": iso(item.created_at),
        "deletedDate": null,
        "id": item.id,
        "organizationId": null,
        "folderId": item.folder_id,
        "type": type_num,
        "reprompt": 0,
        "name": item.name,
        "notes": opt(&item.notes),
        "favorite": item.favorite,
        "fields": fields,
        "collectionIds": null,
    });
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    match item.item_type {
        ItemType::Login => {
            let l = item.login.clone().unwrap_or_default();
            let uris: Vec<Value> = l
                .uris
                .iter()
                .map(|u| json!({ "match": bw_match(u.match_type), "uri": u.uri }))
                .collect();
            obj.insert(
                "login".into(),
                json!({
                    "fido2Credentials": [],
                    "uris": uris,
                    "username": opt(&l.username),
                    "password": opt(&l.password),
                    "totp": opt(&l.totp),
                    "passwordRevisionDate": l.password_revised_at.map(iso),
                }),
            );
        }
        ItemType::Note => {
            obj.insert("secureNote".into(), json!({ "type": 0 }));
        }
        ItemType::Card => {
            let c = item.card.clone().unwrap_or_default();
            obj.insert(
                "card".into(),
                json!({
                    "cardholderName": opt(&c.cardholder_name),
                    "brand": opt(&c.brand),
                    "number": opt(&c.number),
                    "expMonth": opt(&c.exp_month),
                    "expYear": opt(&c.exp_year),
                    "code": opt(&c.code),
                }),
            );
        }
        ItemType::Identity => {
            let i = item.identity.clone().unwrap_or_default();
            obj.insert(
                "identity".into(),
                json!({
                    "title": opt(&i.title),
                    "firstName": opt(&i.first_name),
                    "middleName": null,
                    "lastName": opt(&i.last_name),
                    "address1": opt(&i.address1),
                    "address2": opt(&i.address2),
                    "address3": null,
                    "city": opt(&i.city),
                    "state": opt(&i.state),
                    "postalCode": opt(&i.postal_code),
                    "country": opt(&i.country),
                    "company": opt(&i.company),
                    "email": opt(&i.email),
                    "phone": opt(&i.phone),
                    "ssn": null,
                    "username": opt(&i.username),
                    "passportNumber": null,
                    "licenseNumber": null,
                }),
            );
        }
    }
    v
}

/// Unencrypted Bitwarden-compatible JSON
/// (`{"encrypted":false,"folders":[…],"items":[…]}`).
pub fn export_bitwarden_json(data: &VaultData) -> String {
    let folders: Vec<Value> = data
        .folders
        .iter()
        .map(|f| json!({ "id": f.id, "name": f.name }))
        .collect();
    let items: Vec<Value> = active_items(data).map(bw_item).collect();
    let doc = json!({ "encrypted": false, "folders": folders, "items": items });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// Writes an export file. `format` is one of `"keystead"`, `"csv"`,
/// `"bitwarden_json"` (the `export_data` command values); `password` is
/// required for `keystead`. Every export is written atomically through a
/// new, randomly named temporary file (see [`format::write_atomic`]), so a
/// file planted next to the target is never written to. On Unix the result
/// is readable only by the current user; on Windows it gets the default
/// permissions of the target folder (private inside the user profile, but
/// readable by other users in shared folders such as `C:\Users\Public`).
pub fn export_to_file(
    data: &VaultData,
    format: &str,
    path: &Path,
    password: Option<&str>,
) -> Result<()> {
    match format {
        "keystead" => {
            let pw = password
                .filter(|p| !p.is_empty())
                .ok_or_else(|| Error::invalid("password_required"))?;
            export_encrypted(data, path, pw)
        }
        "csv" => {
            let text = Zeroizing::new(export_csv(data));
            format::write_atomic(path, text.as_bytes(), false)
        }
        "bitwarden_json" => {
            let text = Zeroizing::new(export_bitwarden_json(data));
            format::write_atomic(path, text.as_bytes(), false)
        }
        other => Err(Error::invalid(format!("format {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_escaping_round_trips() {
        for s in [
            "", "'", "''", "plain", "'plain", "a=b", "=1", "'=1", "''=1", " =1", "\t1", "\r1",
            "+1", "-1", "@1", "'@1", "' -1",
        ] {
            let e = csv_escape(s, csv_formula_like);
            assert!(!csv_formula_like(&e), "{s:?} → {e:?}");
            assert_eq!(csv_unescape(&e, csv_formula_like), s, "{s:?}");
        }
        for s in ["+49", "-x", "=x", "@x", "'=x", " @x", "x", "'"] {
            let e = csv_escape(s, csv_username_formula_like);
            assert!(!csv_username_formula_like(&e), "{s:?} → {e:?}");
            assert_eq!(csv_unescape(&e, csv_username_formula_like), s, "{s:?}");
        }
        assert_eq!(csv_escape("+49", csv_username_formula_like), "+49");
        assert_eq!(csv_escape("+49", csv_formula_like), "'+49");
    }
}
