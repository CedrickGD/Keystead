//! Importers: VaultX 1.x (PowerShell) vaults, CSV (Chrome/Edge, Firefox,
//! Bitwarden, legacy VaultX / generic), unencrypted Bitwarden JSON and
//! Keystead encrypted exports.
//!
//! Importers return items with their *source* ids; folder references point
//! to the returned folders' ids. [`crate::vault::UnlockedVault::import_items`]
//! assigns fresh ids and maps the folder references.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use hmac::{KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::crypto;
use crate::error::{Error, Result};
use crate::format::VaultFile;
use crate::model::{
    CardData, CustomField, FieldKind, Folder, IdentityData, ItemType, LoginData, LoginUri,
    PasswordHistoryEntry, UriMatch, VaultItem,
};
use crate::paths;
use crate::util;
use crate::vault::UnlockedVault;

/// Result of an import into a vault (returned to the UI).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
}

/// A VaultX 1.x vault found on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyVaultInfo {
    pub name: String,
    pub path: String,
}

/// Parsed import data before it is added to a vault.
#[derive(Debug, Default)]
struct Parsed {
    items: Vec<VaultItem>,
    folders: Vec<Folder>,
    warnings: Vec<String>,
    skipped: usize,
}

impl Parsed {
    fn skip(&mut self, warning: String) {
        self.skipped += 1;
        self.warnings.push(warning);
    }
}

// ---------------------------------------------------------------------------
// Generic JSON helpers (lenient: numbers/bools are accepted as strings).
// ---------------------------------------------------------------------------

fn value_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn get_str(obj: &Map<String, Value>, key: &str) -> String {
    value_str(obj.get(key))
}

/// Case-insensitive key lookup (PowerShell JSON is case-insensitive).
fn get_ci<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    obj.get(key).or_else(|| {
        obj.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

fn get_ci_str(obj: &Map<String, Value>, key: &str) -> String {
    value_str(get_ci(obj, key)).trim().to_owned()
}

fn get_i64(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

fn timestamp(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::String(s)) => util::parse_timestamp_ms(s),
        Some(Value::Number(n)) => n.as_i64(),
        // PowerShell 7 may serialise a DateTime as { "value": ..., "DateTime": ... }.
        Some(Value::Object(o)) => timestamp(get_ci(o, "value")),
        _ => None,
    }
}

fn host_of(url: &str) -> Option<String> {
    let parsed = if url.contains("://") {
        url::Url::parse(url).ok()
    } else {
        url::Url::parse(&format!("https://{url}")).ok()
    };
    parsed
        .and_then(|u| u.host_str().map(str::to_owned))
        .filter(|h| !h.is_empty())
}

fn domain_uri(uri: &str) -> LoginUri {
    LoginUri {
        uri: uri.trim().to_owned(),
        match_type: UriMatch::Domain,
    }
}

fn text_field(name: &str, value: &str) -> CustomField {
    CustomField {
        name: name.to_owned(),
        value: value.to_owned(),
        kind: FieldKind::Text,
    }
}

fn hidden_field(name: &str, value: &str) -> CustomField {
    CustomField {
        name: name.to_owned(),
        value: value.to_owned(),
        kind: FieldKind::Hidden,
    }
}

/// Name for an item without a title: host of its first URI, else username.
fn fallback_name(item: &VaultItem) -> String {
    if let Some(login) = &item.login {
        if let Some(host) = login.uris.first().and_then(|u| host_of(&u.uri)) {
            return host;
        }
        if !login.username.trim().is_empty() {
            return login.username.trim().to_owned();
        }
    }
    "Untitled".to_owned()
}

fn ensure_name(item: &mut VaultItem) {
    item.name = item.name.trim().to_owned();
    if item.name.is_empty() {
        item.name = fallback_name(item);
    }
}

// ---------------------------------------------------------------------------
// Legacy VaultX 1.x
// ---------------------------------------------------------------------------

/// PBKDF2 iteration cap for legacy files (DoS guard; VaultX 1.x used 100 000).
const LEGACY_MAX_ITERATIONS: u32 = 10_000_000;

#[derive(Debug)]
struct LegacyRecovery {
    salt: Vec<u8>,
    iterations: u32,
    iv: Vec<u8>,
    data: Vec<u8>,
}

#[derive(Debug)]
struct LegacyMeta {
    version: i64,
    salt: Vec<u8>,
    iterations: u32,
    iv: Vec<u8>,
    data: Vec<u8>,
    mac: Option<Vec<u8>>,
    account_name: String,
    recovery: Option<LegacyRecovery>,
}

impl LegacyMeta {
    /// v2+ or a present `Mac` means the HMAC must be verified.
    fn mac_required(&self) -> bool {
        self.version >= 2 || self.mac.is_some()
    }
}

fn b64_field(obj: &Map<String, Value>, key: &str) -> Result<Vec<u8>> {
    let s = get_ci_str(obj, key);
    if s.is_empty() {
        return Ok(Vec::new());
    }
    crypto::b64_decode(&s).map_err(|_| Error::corrupt(format!("legacy vault: invalid {key}")))
}

fn iterations_field(obj: &Map<String, Value>, key: &str) -> Result<u32> {
    let n = get_i64(get_ci(obj, key))
        .ok_or_else(|| Error::corrupt(format!("legacy vault: missing {key}")))?;
    if n <= 0 {
        return Err(Error::corrupt(format!("legacy vault: invalid {key}")));
    }
    u32::try_from(n)
        .ok()
        .filter(|n| *n <= LEGACY_MAX_ITERATIONS)
        .ok_or_else(|| Error::Unsupported(format!("legacy vault: {key} too large")))
}

fn parse_legacy_meta(bytes: &[u8]) -> Result<LegacyMeta> {
    let text = util::decode_text(bytes);
    let value: Value = serde_json::from_str(util::strip_bom(&text))
        .map_err(|_| Error::Unsupported("not a VaultX 1.x vault file".into()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Unsupported("not a VaultX 1.x vault file".into()))?;
    if get_ci(obj, "Salt").is_none() || get_ci(obj, "Iterations").is_none() {
        return Err(Error::Unsupported("not a VaultX 1.x vault file".into()));
    }
    let salt = b64_field(obj, "Salt")?;
    if salt.is_empty() {
        return Err(Error::corrupt("legacy vault: missing Salt"));
    }
    let iterations = iterations_field(obj, "Iterations")?;
    let version = get_i64(get_ci(obj, "Version")).unwrap_or(1).max(1);
    let mac = b64_field(obj, "Mac")?;
    let recovery = {
        let rsalt = b64_field(obj, "RecoverySalt")?;
        let riv = b64_field(obj, "RecoveryKeyIV")?;
        let rdata = b64_field(obj, "RecoveryKeyData")?;
        if rsalt.is_empty() || riv.is_empty() || rdata.is_empty() {
            None
        } else {
            Some(LegacyRecovery {
                salt: rsalt,
                iterations: iterations_field(obj, "RecoveryIterations")?,
                iv: riv,
                data: rdata,
            })
        }
    };
    Ok(LegacyMeta {
        version,
        salt,
        iterations,
        iv: b64_field(obj, "IV")?,
        data: b64_field(obj, "Data")?,
        mac: (!mac.is_empty()).then_some(mac),
        account_name: get_ci_str(obj, "AccountName"),
        recovery,
    })
}

/// .NET `Rfc2898DeriveBytes(password, salt, iterations)` = PBKDF2-HMAC-SHA1.
fn pbkdf2_sha1(password: &str, salt: &[u8], iterations: u32, len: usize) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(vec![0u8; len]);
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), salt, iterations, &mut out);
    out
}

/// AES-256-CBC with PKCS#7 padding. `None` if key/iv sizes are wrong or the
/// padding is invalid (typically a wrong key).
fn aes_cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let dec = cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv).ok()?;
    let mut buf = Zeroizing::new(data.to_vec());
    let len = dec.decrypt_padded::<Pkcs7>(&mut buf).ok()?.len();
    buf.truncate(len);
    Some(buf)
}

enum Attempt {
    Plain(Value),
    WrongKey,
}

/// Decrypts the payload with one key pair.
fn legacy_decrypt(meta: &LegacyMeta, enc_key: &[u8], mac_key: Option<&[u8]>) -> Result<Attempt> {
    if meta.data.is_empty() {
        if meta.mac_required() {
            return Err(Error::corrupt("legacy vault: integrity check failed"));
        }
        // VaultX 1.x stored a vault without entries as empty Data.
        return Ok(Attempt::Plain(Value::Object(Map::new())));
    }
    if meta.iv.len() != 16 {
        return Err(Error::corrupt("legacy vault: invalid IV"));
    }
    let mac_ok = if meta.mac_required() {
        let Some(mac_key) = mac_key else {
            return Err(Error::Unsupported(
                "legacy recovery password without integrity key; use the master password".into(),
            ));
        };
        let Some(expected) = meta.mac.as_deref() else {
            return Err(Error::corrupt("legacy vault: missing Mac"));
        };
        let mut hmac = <hmac::Hmac<sha2::Sha256> as KeyInit>::new_from_slice(mac_key)
            .map_err(|_| Error::corrupt("legacy vault: invalid MAC key"))?;
        hmac.update(&meta.iv);
        hmac.update(&meta.data);
        let actual = hmac.finalize().into_bytes();
        if !crypto::ct_eq(&actual, expected) {
            return Ok(Attempt::WrongKey);
        }
        true
    } else {
        false
    };
    let Some(plain) = aes_cbc_decrypt(enc_key, &meta.iv, &meta.data) else {
        return if mac_ok {
            Err(Error::corrupt("legacy vault: decryption failed"))
        } else {
            Ok(Attempt::WrongKey)
        };
    };
    match serde_json::from_slice::<Value>(util::strip_bom_bytes(&plain)) {
        Ok(v) if v.is_object() => Ok(Attempt::Plain(v)),
        _ if mac_ok => Err(Error::corrupt("legacy vault: payload is not valid JSON")),
        // Without a MAC a wrong key occasionally yields valid padding.
        _ => Ok(Attempt::WrongKey),
    }
}

fn legacy_unlock(meta: &LegacyMeta, password: &str) -> Result<Value> {
    let material = pbkdf2_sha1(password, &meta.salt, meta.iterations, 64);
    if let Attempt::Plain(v) = legacy_decrypt(meta, &material[..32], Some(&material[32..]))? {
        return Ok(v);
    }
    if let Some(rec) = &meta.recovery {
        let rec_key = pbkdf2_sha1(password, &rec.salt, rec.iterations, 32);
        if let Some(keys) = aes_cbc_decrypt(&rec_key, &rec.iv, &rec.data) {
            let attempt = match keys.len() {
                64 => legacy_decrypt(meta, &keys[..32], Some(&keys[32..]))?,
                32 => legacy_decrypt(meta, &keys[..32], None)?,
                _ => Attempt::WrongKey,
            };
            if let Attempt::Plain(v) = attempt {
                return Ok(v);
            }
        }
    }
    Err(Error::WrongPassword)
}

fn legacy_entry_to_item(entry: &Map<String, Value>) -> Option<VaultItem> {
    let title = get_ci_str(entry, "Title");
    let url = get_ci_str(entry, "Url");
    let mut username = get_ci_str(entry, "Username");
    let password = value_str(get_ci(entry, "Password"));
    let email = get_ci_str(entry, "Email");
    let phone = get_ci_str(entry, "Phone");
    let notes = value_str(get_ci(entry, "Notes"));
    let other = value_str(get_ci(entry, "Other"));
    if [&title, &url, &username, &password, &email, &phone]
        .iter()
        .all(|s| s.is_empty())
        && notes.trim().is_empty()
        && other.trim().is_empty()
    {
        return None;
    }
    let email_is_username = username.is_empty() && !email.is_empty();
    if email_is_username {
        username = email.clone();
    }
    let mut fields = Vec::new();
    if !email.is_empty() && !email_is_username && email != username {
        fields.push(text_field("E-Mail", &email));
    }
    if !phone.is_empty() {
        fields.push(text_field("Telefon", &phone));
    }
    if !other.trim().is_empty() {
        fields.push(text_field("Sonstiges", &other));
    }
    let ts = timestamp(get_ci(entry, "UpdatedAt")).unwrap_or(0);
    let mut item = VaultItem {
        id: get_ci_str(entry, "Id"),
        item_type: ItemType::Login,
        name: title,
        notes,
        login: Some(LoginData {
            username,
            password,
            uris: if url.is_empty() {
                Vec::new()
            } else {
                vec![domain_uri(&url)]
            },
            ..Default::default()
        }),
        fields,
        created_at: ts,
        updated_at: ts,
        ..Default::default()
    };
    ensure_name(&mut item);
    Some(item)
}

fn parse_legacy_file(path: &Path, password: &str) -> Result<Parsed> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(format!("file {}", path.display()))
        } else {
            Error::io_at(path, e)
        }
    })?;
    let meta = parse_legacy_meta(&bytes)?;
    let plain = legacy_unlock(&meta, password)?;
    let mut out = Parsed::default();
    let Some(obj) = plain.as_object() else {
        return Err(Error::corrupt("legacy vault: payload is not an object"));
    };
    if !get_ci_str(obj, "TotpSecret").is_empty() {
        out.warnings.push(
            "The two-factor unlock (TotpSecret) of the old vault is not imported; Keystead does not use it."
                .to_owned(),
        );
    }
    // PowerShell writes a single entry as an object instead of an array.
    let entries: Vec<&Value> = match get_ci(obj, "Entries") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    };
    for (i, entry) in entries.iter().enumerate() {
        match entry.as_object().and_then(legacy_entry_to_item) {
            Some(item) => out.items.push(item),
            None => out.skip(format!("Entry {} skipped: no usable data.", i + 1)),
        }
    }
    Ok(out)
}

/// Decrypts a VaultX 1.x vault file with its master password or its
/// recovery password. Returns the items and warnings.
pub fn import_legacy_file(path: &Path, password: &str) -> Result<(Vec<VaultItem>, Vec<String>)> {
    let parsed = parse_legacy_file(path, password)?;
    Ok((parsed.items, parsed.warnings))
}

/// Display name of a legacy vault file `vault_<name>_<8 hex>.json`.
fn legacy_name_from_file(file_name: &str) -> String {
    let base = match file_name.len().checked_sub(5) {
        Some(i)
            if file_name
                .get(i..)
                .is_some_and(|e| e.eq_ignore_ascii_case(".json")) =>
        {
            &file_name[..i]
        }
        _ => file_name,
    };
    if let Some(rest) = base.strip_prefix("vault_") {
        if let Some((name, hash)) = rest.rsplit_once('_') {
            if hash.len() == 8 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty() {
                return name.replace('_', " ");
            }
        }
    }
    base.to_owned()
}

fn is_legacy_vault(path: &Path) -> Option<LegacyMeta> {
    let bytes = std::fs::read(path).ok()?;
    parse_legacy_meta(&bytes).ok()
}

/// Legacy vaults in an explicit directory (`accounts.json` + `vault_*.json`).
pub fn legacy_scan_dir(dir: &Path) -> Vec<LegacyVaultInfo> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |name: String, path: PathBuf, out: &mut Vec<LegacyVaultInfo>| {
        let key = path
            .file_name()
            .map(|f| f.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if seen.insert(key) {
            out.push(LegacyVaultInfo {
                name,
                path: path.display().to_string(),
            });
        }
    };

    if let Ok(bytes) = std::fs::read(dir.join("accounts.json")) {
        let text = util::decode_text(&bytes);
        let accounts: Vec<Value> = match serde_json::from_str::<Value>(util::strip_bom(&text)) {
            Ok(Value::Array(a)) => a,
            Ok(v @ Value::Object(_)) => vec![v],
            _ => Vec::new(),
        };
        for acc in accounts.iter().filter_map(Value::as_object) {
            let file = get_ci_str(acc, "File");
            // Only plain file names inside the legacy directory are accepted.
            let file_name = file.rsplit(['/', '\\']).next().unwrap_or("").trim();
            if file_name.is_empty() || file_name == "." || file_name == ".." {
                continue;
            }
            let path = dir.join(file_name);
            let Some(meta) = is_legacy_vault(&path) else {
                continue;
            };
            let mut name = get_ci_str(acc, "Name");
            if name.is_empty() {
                name = if meta.account_name.is_empty() {
                    legacy_name_from_file(file_name)
                } else {
                    meta.account_name.clone()
                };
            }
            add(name, path, &mut out);
        }
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name().and_then(|f| f.to_str()).is_some_and(|f| {
                        let lower = f.to_lowercase();
                        lower.starts_with("vault_") && lower.ends_with(".json")
                    })
            })
            .collect();
        files.sort();
        for path in files {
            let Some(meta) = is_legacy_vault(&path) else {
                continue;
            };
            let file_name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = if meta.account_name.is_empty() {
                legacy_name_from_file(&file_name)
            } else {
                meta.account_name
            };
            add(name, path, &mut out);
        }
    }
    out
}

/// VaultX 1.x vaults of this user: `%LOCALAPPDATA%\VaultX\accounts.json`
/// plus `vault_*.json` files on Windows; elsewhere only if
/// `$KEYSTEAD_LEGACY_DIR` is set.
pub fn legacy_scan() -> Vec<LegacyVaultInfo> {
    paths::legacy_dir()
        .map(|d| legacy_scan_dir(&d))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Column aliases of the generic / legacy VaultX importer
/// (from `Convert-CsvRowToEntry` in legacy/VaultX.ps1, plus TOTP, folder and
/// favorite columns used by other managers).
const ALIAS_TITLE: &[&str] = &["name", "title", "site", "site_name", "label"];
const ALIAS_URL: &[&str] = &[
    "url",
    "origin_url",
    "origin",
    "hostname",
    "host",
    "website",
    "site",
    "form_action_origin",
    "formactionorigin",
    "login_uri",
    "uri",
    "domain",
];
const ALIAS_USERNAME: &[&str] = &[
    "username",
    "user",
    "login",
    "login_username",
    "login_name",
    "user_name",
];
const ALIAS_PASSWORD: &[&str] = &["password", "pass", "password_value", "passwords", "secret"];
const ALIAS_EMAIL: &[&str] = &["email", "email_address", "e-mail"];
const ALIAS_PHONE: &[&str] = &["phone", "phone_number", "tel"];
const ALIAS_NOTES: &[&str] = &[
    "notes",
    "note",
    "comment",
    "comments",
    "memo",
    "description",
];
const ALIAS_OTHER: &[&str] = &["other", "extra", "misc"];
const ALIAS_TOTP: &[&str] = &["totp", "otp", "login_totp", "otpauth", "2fa"];
const ALIAS_FOLDER: &[&str] = &["folder", "group", "grouping"];
const ALIAS_FAVORITE: &[&str] = &["favorite", "fav"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvFormat {
    Bitwarden,
    Firefox,
    /// Chrome/Edge (`name,url,username,password,note`), legacy VaultX and
    /// anything else with recognisable column names.
    Generic,
}

fn detect_csv_format(headers: &[String]) -> CsvFormat {
    let has = |h: &str| headers.iter().any(|x| x == h);
    if has("login_password") || has("login_uri") || has("login_username") {
        CsvFormat::Bitwarden
    } else if has("url")
        && has("password")
        && (has("httprealm") || has("formactionorigin") || has("guid"))
    {
        CsvFormat::Firefox
    } else {
        CsvFormat::Generic
    }
}

/// Picks the delimiter (',' ';' or tab) that occurs most often outside
/// quotes in the header line.
fn detect_delimiter(header_line: &str) -> u8 {
    let mut counts = [0usize; 3];
    let mut in_quotes = false;
    for c in header_line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => counts[0] += 1,
            ';' if !in_quotes => counts[1] += 1,
            '\t' if !in_quotes => counts[2] += 1,
            _ => {}
        }
    }
    if counts[1] > counts[0] && counts[1] >= counts[2] {
        b';'
    } else if counts[2] > counts[0] && counts[2] > counts[1] {
        b'\t'
    } else {
        b','
    }
}

fn normalize_header(h: &str) -> String {
    util::strip_bom(h)
        .trim()
        .trim_matches('"')
        .trim()
        .to_lowercase()
}

/// A CSV row addressed by (normalised) column name.
struct Row<'a> {
    headers: &'a [String],
    record: &'a csv::StringRecord,
}

impl Row<'_> {
    fn get(&self, name: &str) -> String {
        self.headers
            .iter()
            .position(|h| h == name)
            .and_then(|i| self.record.get(i))
            .unwrap_or("")
            .to_owned()
    }

    /// First non-empty value among the aliases (trimmed).
    fn first(&self, aliases: &[&str]) -> String {
        aliases
            .iter()
            .map(|a| self.get(a))
            .map(|v| v.trim().to_owned())
            .find(|v| !v.is_empty())
            .unwrap_or_default()
    }

    /// Like [`Row::first`] but untrimmed (passwords may contain spaces).
    fn first_raw(&self, aliases: &[&str]) -> String {
        aliases
            .iter()
            .map(|a| self.get(a))
            .find(|v| !v.trim().is_empty())
            .unwrap_or_default()
    }

    fn is_blank(&self) -> bool {
        self.record.iter().all(|f| f.trim().is_empty())
    }
}

fn truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "ja" | "x"
    )
}

/// Collects folders by (case-insensitive) name.
#[derive(Default)]
struct FolderSet {
    by_key: HashMap<String, String>,
    folders: Vec<Folder>,
}

impl FolderSet {
    /// Returns the (temporary) folder id for `name`.
    fn id_for(&mut self, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let key = name.to_lowercase();
        if let Some(id) = self.by_key.get(&key) {
            return Some(id.clone());
        }
        let id = format!("import-folder-{}", self.folders.len() + 1);
        self.folders.push(Folder {
            id: id.clone(),
            name: name.to_owned(),
        });
        self.by_key.insert(key, id.clone());
        Some(id)
    }
}

/// Bitwarden CSV `fields`: one `name: value` per line.
fn parse_bitwarden_csv_fields(s: &str) -> Vec<CustomField> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| match line.split_once(": ") {
            Some((n, v)) => text_field(n.trim(), v),
            None => match line.split_once(':') {
                Some((n, v)) => text_field(n.trim(), v.trim_start()),
                None => text_field(line.trim(), ""),
            },
        })
        .collect()
}

fn split_uris(s: &str) -> Vec<LoginUri> {
    s.split(',')
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(domain_uri)
        .collect()
}

fn csv_bitwarden_row(row: &Row, folders: &mut FolderSet) -> Option<VaultItem> {
    let kind = row.get("type").trim().to_lowercase();
    let username = row.get("login_username").trim().to_owned();
    let password = row.get("login_password");
    let uris = split_uris(&row.get("login_uri"));
    let totp = row.get("login_totp").trim().to_owned();
    let name = row.get("name").trim().to_owned();
    let notes = row.get("notes");
    let fields = parse_bitwarden_csv_fields(&row.get("fields"));
    let has_login =
        !username.is_empty() || !password.is_empty() || !uris.is_empty() || !totp.is_empty();
    if !has_login && name.is_empty() && notes.trim().is_empty() && fields.is_empty() {
        return None;
    }
    let item_type = match kind.as_str() {
        "note" | "securenote" => ItemType::Note,
        "login" => ItemType::Login,
        _ if has_login => ItemType::Login,
        _ => ItemType::Note,
    };
    let mut item = VaultItem {
        item_type,
        name,
        notes,
        favorite: truthy(&row.get("favorite")),
        folder_id: folders.id_for(&row.get("folder")),
        fields,
        login: (item_type == ItemType::Login).then_some(LoginData {
            username,
            password,
            uris,
            totp,
            password_revised_at: None,
        }),
        ..Default::default()
    };
    item.normalize();
    ensure_name(&mut item);
    Some(item)
}

fn csv_firefox_row(row: &Row) -> Option<VaultItem> {
    let url = row.get("url").trim().to_owned();
    let username = row.get("username").trim().to_owned();
    let password = row.get("password");
    if url.is_empty() && username.is_empty() && password.is_empty() {
        return None;
    }
    let created = row.get("timecreated").trim().parse::<i64>().unwrap_or(0);
    let changed = row
        .get("timepasswordchanged")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|t| *t > 0 && *t != created);
    let mut item = VaultItem {
        item_type: ItemType::Login,
        name: host_of(&url).unwrap_or_else(|| url.clone()),
        login: Some(LoginData {
            username,
            password,
            uris: if url.is_empty() {
                Vec::new()
            } else {
                vec![domain_uri(&url)]
            },
            totp: String::new(),
            password_revised_at: changed,
        }),
        created_at: created,
        updated_at: changed.unwrap_or(created),
        ..Default::default()
    };
    ensure_name(&mut item);
    Some(item)
}

fn csv_generic_row(row: &Row, folders: &mut FolderSet) -> Option<VaultItem> {
    let title = row.first(ALIAS_TITLE);
    let url = row.first(ALIAS_URL);
    let mut username = row.first(ALIAS_USERNAME);
    let password = row.first_raw(ALIAS_PASSWORD);
    let email = row.first(ALIAS_EMAIL);
    let phone = row.first(ALIAS_PHONE);
    let notes = row.first_raw(ALIAS_NOTES);
    let other = row.first(ALIAS_OTHER);
    let totp = row.first(ALIAS_TOTP);
    if [
        &title, &url, &username, &password, &email, &phone, &notes, &other, &totp,
    ]
    .iter()
    .all(|s| s.trim().is_empty())
    {
        return None;
    }
    let email_is_username = username.is_empty() && !email.is_empty();
    if email_is_username {
        username = email.clone();
    }
    let mut fields = Vec::new();
    if !email.is_empty() && !email_is_username && email != username {
        fields.push(text_field("E-Mail", &email));
    }
    if !phone.is_empty() {
        fields.push(text_field("Telefon", &phone));
    }
    if !other.is_empty() {
        fields.push(text_field("Sonstiges", &other));
    }
    let is_note = url.is_empty() && username.is_empty() && password.is_empty() && totp.is_empty();
    let item_type = if is_note {
        ItemType::Note
    } else {
        ItemType::Login
    };
    let mut item = VaultItem {
        item_type,
        name: if title.is_empty() { url.clone() } else { title },
        notes,
        favorite: truthy(&row.first(ALIAS_FAVORITE)),
        folder_id: folders.id_for(&row.first(ALIAS_FOLDER)),
        fields,
        login: (!is_note).then(|| LoginData {
            username,
            password,
            uris: split_uris_single(&url),
            totp,
            password_revised_at: None,
        }),
        ..Default::default()
    };
    item.normalize();
    ensure_name(&mut item);
    Some(item)
}

fn split_uris_single(url: &str) -> Vec<LoginUri> {
    if url.trim().is_empty() {
        Vec::new()
    } else {
        vec![domain_uri(url)]
    }
}

fn parse_csv(text: &str) -> Result<Parsed> {
    let mut text = util::strip_bom(text);
    let mut first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    // Excel's "sep=;" hint line.
    let mut delimiter = None;
    if let Some(sep) = first_line.trim().strip_prefix("sep=") {
        if let Some(&b) = sep.as_bytes().first() {
            delimiter = Some(b);
            let skip = text.find(first_line).map_or(0, |p| p + first_line.len());
            text = text[skip..].trim_start_matches(['\r', '\n']);
            first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        }
    }
    if first_line.trim().is_empty() {
        return Err(Error::invalid("csv_empty"));
    }
    let delimiter = delimiter.unwrap_or_else(|| detect_delimiter(first_line));
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(true)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| Error::invalid(format!("csv: {e}")))?
        .iter()
        .map(normalize_header)
        .collect();
    let format = detect_csv_format(&headers);
    if format == CsvFormat::Generic {
        let known = [
            ALIAS_TITLE,
            ALIAS_URL,
            ALIAS_USERNAME,
            ALIAS_PASSWORD,
            ALIAS_EMAIL,
            ALIAS_NOTES,
        ];
        if !headers
            .iter()
            .any(|h| known.iter().any(|list| list.contains(&h.as_str())))
        {
            return Err(Error::invalid("csv_unknown_columns"));
        }
    }

    let mut out = Parsed::default();
    let mut folders = FolderSet::default();
    for (index, result) in reader.records().enumerate() {
        let line = index + 2;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                let line = e.position().map_or(line as u64, |p| p.line());
                out.skip(format!("Row {line} skipped: {e}"));
                continue;
            }
        };
        let line = record.position().map_or(line as u64, |p| p.line());
        let row = Row {
            headers: &headers,
            record: &record,
        };
        if row.is_blank() {
            continue;
        }
        let item = match format {
            CsvFormat::Bitwarden => csv_bitwarden_row(&row, &mut folders),
            CsvFormat::Firefox => csv_firefox_row(&row),
            CsvFormat::Generic => csv_generic_row(&row, &mut folders),
        };
        match item {
            Some(item) => out.items.push(item),
            None => out.skip(format!("Row {line} skipped: no usable data.")),
        }
    }
    out.folders = folders.folders;
    Ok(out)
}

/// Imports a CSV export. Detects Bitwarden, Firefox, Chrome/Edge and
/// legacy-VaultX/generic column layouts; ',' ';' or tab separated; a UTF-8
/// BOM is ignored. Returns items, folders and warnings.
pub fn import_csv(text: &str) -> Result<(Vec<VaultItem>, Vec<Folder>, Vec<String>)> {
    let p = parse_csv(text)?;
    Ok((p.items, p.folders, p.warnings))
}

// ---------------------------------------------------------------------------
// Bitwarden JSON
// ---------------------------------------------------------------------------

fn bw_match(v: Option<&Value>, warnings: &mut Vec<String>, item_name: &str) -> UriMatch {
    match get_i64(v) {
        None | Some(0) => UriMatch::Domain,
        Some(1) => UriMatch::Host,
        Some(2) => UriMatch::StartsWith,
        Some(3) => UriMatch::Exact,
        Some(5) => UriMatch::Never,
        Some(4) => {
            warnings.push(format!(
                "\"{item_name}\": regular-expression URL matching is not supported; domain matching is used."
            ));
            UriMatch::Domain
        }
        Some(_) => UriMatch::Domain,
    }
}

fn bw_month(s: &str) -> String {
    match s.trim().parse::<u32>() {
        Ok(m) if (1..=12).contains(&m) => format!("{m:02}"),
        _ => s.trim().to_owned(),
    }
}

fn bw_year(s: &str) -> String {
    let s = s.trim();
    if s.len() == 2 && s.chars().all(|c| c.is_ascii_digit()) {
        format!("20{s}")
    } else {
        s.to_owned()
    }
}

fn bw_item(obj: &Map<String, Value>, out: &mut Parsed) {
    let name = get_str(obj, "name");
    let label = if name.trim().is_empty() {
        "(unnamed)".to_owned()
    } else {
        name.trim().to_owned()
    };
    if obj.get("deletedDate").is_some_and(|v| !v.is_null()) {
        out.skip(format!(
            "\"{label}\" skipped: it is in the Bitwarden trash."
        ));
        return;
    }
    let item_type = match get_i64(obj.get("type")) {
        Some(1) => ItemType::Login,
        Some(2) | Some(5) => ItemType::Note,
        Some(3) => ItemType::Card,
        Some(4) => ItemType::Identity,
        other => {
            out.skip(format!(
                "\"{label}\" skipped: unsupported item type {}.",
                other.map_or_else(|| "?".to_owned(), |t| t.to_string())
            ));
            return;
        }
    };
    let mut fields: Vec<CustomField> = obj
        .get("fields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_object)
                .filter_map(|f| {
                    let kind = match get_i64(f.get("type")) {
                        Some(1) => FieldKind::Hidden,
                        Some(2) => FieldKind::Boolean,
                        Some(3) => return None, // linked field: no value of its own
                        _ => FieldKind::Text,
                    };
                    Some(CustomField {
                        name: get_str(f, "name"),
                        value: get_str(f, "value"),
                        kind,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mut item = VaultItem {
        id: get_str(obj, "id"),
        item_type,
        name,
        // Organisation items have collections instead of a folder.
        folder_id: obj
            .get("folderId")
            .and_then(Value::as_str)
            .or_else(|| {
                obj.get("collectionIds")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
            })
            .map(str::to_owned),
        favorite: obj
            .get("favorite")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        notes: get_str(obj, "notes"),
        created_at: timestamp(obj.get("creationDate")).unwrap_or(0),
        updated_at: timestamp(obj.get("revisionDate")).unwrap_or(0),
        ..Default::default()
    };

    match item_type {
        ItemType::Login => {
            let login = obj.get("login").and_then(Value::as_object);
            let empty = Map::new();
            let login = login.unwrap_or(&empty);
            let uris = login
                .get("uris")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_object)
                        .filter_map(|u| {
                            let uri = get_str(u, "uri").trim().to_owned();
                            (!uri.is_empty()).then(|| LoginUri {
                                match_type: bw_match(u.get("match"), &mut out.warnings, &label),
                                uri,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if login
                .get("fido2Credentials")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                out.warnings
                    .push(format!("\"{label}\": passkeys are not imported."));
            }
            item.login = Some(LoginData {
                username: get_str(login, "username"),
                password: get_str(login, "password"),
                uris,
                totp: get_str(login, "totp"),
                password_revised_at: timestamp(login.get("passwordRevisionDate")),
            });
            item.password_history = obj
                .get("passwordHistory")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_object)
                        .map(|h| PasswordHistoryEntry {
                            password: get_str(h, "password"),
                            replaced_at: timestamp(h.get("lastUsedDate")).unwrap_or(0),
                        })
                        .filter(|h| !h.password.is_empty())
                        .collect()
                })
                .unwrap_or_default();
        }
        ItemType::Card => {
            let empty = Map::new();
            let c = obj.get("card").and_then(Value::as_object).unwrap_or(&empty);
            item.card = Some(CardData {
                cardholder_name: get_str(c, "cardholderName"),
                brand: get_str(c, "brand"),
                number: get_str(c, "number"),
                exp_month: bw_month(&get_str(c, "expMonth")),
                exp_year: bw_year(&get_str(c, "expYear")),
                code: get_str(c, "code"),
            });
        }
        ItemType::Identity => {
            let empty = Map::new();
            let i = obj
                .get("identity")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            item.identity = Some(IdentityData {
                title: get_str(i, "title"),
                first_name: get_str(i, "firstName"),
                last_name: get_str(i, "lastName"),
                email: get_str(i, "email"),
                phone: get_str(i, "phone"),
                company: get_str(i, "company"),
                address1: get_str(i, "address1"),
                address2: get_str(i, "address2"),
                postal_code: get_str(i, "postalCode"),
                city: get_str(i, "city"),
                state: get_str(i, "state"),
                country: get_str(i, "country"),
                username: get_str(i, "username"),
            });
            for (key, label, hidden) in [
                ("middleName", "Zweiter Vorname", false),
                ("address3", "Adresse 3", false),
                ("ssn", "Sozialversicherungsnummer", true),
                ("passportNumber", "Reisepassnummer", true),
                ("licenseNumber", "Führerscheinnummer", true),
            ] {
                let v = get_str(i, key);
                if !v.is_empty() {
                    fields.push(if hidden {
                        hidden_field(label, &v)
                    } else {
                        text_field(label, &v)
                    });
                }
            }
        }
        ItemType::Note => {
            if let Some(ssh) = obj.get("sshKey").and_then(Value::as_object) {
                out.warnings
                    .push(format!("\"{label}\": SSH key imported as a secure note."));
                for (key, label, hidden) in [
                    ("privateKey", "Private Key", true),
                    ("publicKey", "Public Key", false),
                    ("keyFingerprint", "Fingerprint", false),
                ] {
                    let v = get_str(ssh, key);
                    if !v.is_empty() {
                        fields.push(if hidden {
                            hidden_field(label, &v)
                        } else {
                            text_field(label, &v)
                        });
                    }
                }
            }
        }
    }
    item.fields = fields;
    item.normalize();
    ensure_name(&mut item);
    out.items.push(item);
}

fn parse_bitwarden_json(text: &str) -> Result<Parsed> {
    let value: Value = serde_json::from_str(util::strip_bom(text))
        .map_err(|e| Error::invalid(format!("json: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::invalid("bitwarden_json"))?;
    if obj.get("encrypted").and_then(Value::as_bool) == Some(true) {
        return Err(Error::Unsupported(
            "encrypted Bitwarden export; export as unencrypted JSON".into(),
        ));
    }
    let items = obj
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid("bitwarden_json"))?;
    let mut out = Parsed::default();
    // Organisation exports use "collections" instead of "folders".
    for key in ["folders", "collections"] {
        if let Some(folders) = obj.get(key).and_then(Value::as_array) {
            for f in folders.iter().filter_map(Value::as_object) {
                let id = get_str(f, "id");
                let name = get_str(f, "name");
                if !id.is_empty() && !name.trim().is_empty() {
                    out.folders.push(Folder { id, name });
                }
            }
        }
    }
    for item in items {
        match item.as_object() {
            Some(o) => bw_item(o, &mut out),
            None => out.skip("An item with invalid structure was skipped.".to_owned()),
        }
    }
    Ok(out)
}

/// Imports an unencrypted Bitwarden JSON export.
pub fn import_bitwarden_json(text: &str) -> Result<(Vec<VaultItem>, Vec<Folder>, Vec<String>)> {
    let p = parse_bitwarden_json(text)?;
    Ok((p.items, p.folders, p.warnings))
}

// ---------------------------------------------------------------------------
// Keystead export
// ---------------------------------------------------------------------------

/// Reads an encrypted Keystead export (any v1 vault file) with its password.
/// Trashed items are left out.
pub fn import_keystead_export(path: &Path, password: &str) -> Result<(Vec<VaultItem>, Vec<Folder>)> {
    let file = VaultFile::read(path)?;
    let key = file.unwrap_key(password)?;
    let data = file.decrypt_payload(&key)?;
    let items = data
        .items
        .iter()
        .filter(|i| !i.is_trashed())
        .cloned()
        .collect();
    Ok((items, data.folders.clone()))
}

// ---------------------------------------------------------------------------
// Convenience for the frontends
// ---------------------------------------------------------------------------

fn read_text_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(format!("file {}", path.display()))
        } else {
            Error::io_at(path, e)
        }
    })?;
    Ok(util::decode_text(&bytes))
}

/// Imports a file into an unlocked vault. `format` is one of `"legacy"`,
/// `"csv"`, `"bitwarden_json"`, `"keystead"` (the `import_data` command
/// values); `password` is required for `legacy` and `keystead`.
/// Text files may be UTF-8, UTF-16 (with BOM) or Windows-1252.
pub fn import_into(
    vault: &mut UnlockedVault,
    format: &str,
    path: &Path,
    password: Option<&str>,
) -> Result<ImportReport> {
    let need_password = || -> Result<&str> {
        password
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::invalid("password_required"))
    };
    let parsed = match format {
        "legacy" => parse_legacy_file(path, need_password()?)?,
        "csv" => parse_csv(&read_text_file(path)?)?,
        "bitwarden_json" => parse_bitwarden_json(&read_text_file(path)?)?,
        "keystead" => {
            let (items, folders) = import_keystead_export(path, need_password()?)?;
            Parsed {
                items,
                folders,
                ..Default::default()
            }
        }
        other => return Err(Error::invalid(format!("format {other}"))),
    };
    let imported = vault.import_items(parsed.items, parsed.folders)?;
    Ok(ImportReport {
        imported,
        skipped: parsed.skipped,
        warnings: parsed.warnings,
    })
}
