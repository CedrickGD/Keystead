//! Exporters: encrypted VaultX file, Bitwarden-style CSV and unencrypted
//! Bitwarden-compatible JSON. Trashed items and the generator history are
//! never exported.

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

/// Writes a standalone encrypted VaultX file (new id, own random key and
/// salt, default KDF parameters) readable by
/// [`crate::import::import_vaultx_export`].
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
        .unwrap_or_else(|| "VaultX Export".to_owned());
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

/// Bitwarden-style CSV of all logins and secure notes (cards and identities
/// cannot be represented in this format and are left out).
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
        let fields = Zeroizing::new(
            item.fields
                .iter()
                .map(|f| format!("{}: {}", f.name, field_value(f.kind, &f.value)))
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
        let _ = w.write_record([
            folder,
            if item.favorite { "1" } else { "" },
            kind,
            item.name.as_str(),
            item.notes.as_str(),
            fields.as_str(),
            "0",
            uris.as_str(),
            username,
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

/// Writes an export file. `format` is one of `"vaultx"`, `"csv"`,
/// `"bitwarden_json"` (the `export_data` command values); `password` is
/// required for `vaultx`. Plain-text exports are written atomically and,
/// on Unix, readable only by the current user.
pub fn export_to_file(
    data: &VaultData,
    format: &str,
    path: &Path,
    password: Option<&str>,
) -> Result<()> {
    match format {
        "vaultx" => {
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
