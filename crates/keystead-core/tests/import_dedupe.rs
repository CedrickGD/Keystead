//! Drag & drop import: format detection, duplicate/conflict classification
//! and committing an import plan (see `keystead_core::import`).

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::export::{export_bitwarden_json, export_csv, export_encrypted_with_params};
use keystead_core::import::{
    detect_import, import_into, plan_import, read_import, ConflictMode, ConflictReason,
    DetectedImport, ImportFormat, ImportPlan, ImportReport, ParsedImport, IMPORT_MAX_BYTES,
};
use keystead_core::model::{
    CardData, Folder, IdentityData, ItemType, LoginData, LoginUri, UriMatch, VaultData, VaultItem,
};
use keystead_core::{Error, KdfParams, UnlockedVault, VaultStore};
use serde_json::{json, Value};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn write(dir: &Path, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn new_vault(dir: &Path) -> (VaultStore, UnlockedVault) {
    let store = VaultStore::new(dir.join("data"));
    let vault = store
        .create_vault_with_params("Ziel", "pw", KdfParams::insecure_for_tests())
        .unwrap();
    (store, vault)
}

fn login(name: &str, uri: &str, username: &str, password: &str) -> VaultItem {
    VaultItem {
        item_type: ItemType::Login,
        name: name.into(),
        login: Some(LoginData {
            username: username.into(),
            password: password.into(),
            uris: if uri.is_empty() {
                Vec::new()
            } else {
                vec![LoginUri {
                    uri: uri.into(),
                    match_type: UriMatch::Domain,
                }]
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn with_totp(mut item: VaultItem, totp: &str) -> VaultItem {
    if let Some(l) = item.login.as_mut() {
        l.totp = totp.into();
    }
    item
}

fn card(name: &str, number: &str, exp_year: &str) -> VaultItem {
    VaultItem {
        item_type: ItemType::Card,
        name: name.into(),
        card: Some(CardData {
            number: number.into(),
            exp_year: exp_year.into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn identity(name: &str, first: &str, last: &str, email: &str) -> VaultItem {
    VaultItem {
        item_type: ItemType::Identity,
        name: name.into(),
        identity: Some(IdentityData {
            first_name: first.into(),
            last_name: last.into(),
            email: email.into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn note(name: &str, text: &str) -> VaultItem {
    VaultItem {
        item_type: ItemType::Note,
        name: name.into(),
        notes: text.into(),
        ..Default::default()
    }
}

fn parsed(items: Vec<VaultItem>) -> ParsedImport {
    let mut p = ParsedImport::default();
    p.items.extend(items);
    p
}

fn find<'a>(vault: &'a UnlockedVault, name: &str) -> Vec<&'a VaultItem> {
    vault
        .items()
        .iter()
        .filter(|i| i.name == name && !i.is_trashed())
        .collect()
}

fn unsupported(result: Result<impl std::fmt::Debug, Error>) -> String {
    match result {
        Err(Error::Unsupported(detail)) => detail,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn detected(path: &Path) -> ImportFormat {
    match detect_import(path) {
        Ok(d) => d.format,
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut out = if little_endian {
        vec![0xFF, 0xFE]
    } else {
        vec![0xFE, 0xFF]
    };
    for unit in text.encode_utf16() {
        out.extend_from_slice(&if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    out
}

/// Ids of the items of [`seed`].
struct Seeded {
    github: String,
    mail: String,
    router: String,
    visa: String,
    me: String,
    emergency: String,
    wifi: String,
}

/// A vault with one item of every kind (and a trashed login "Old").
fn seed(vault: &mut UnlockedVault) -> Seeded {
    let mut save = |item: VaultItem| vault.save_item(item).unwrap().id;
    let ids = Seeded {
        github: save(login(
            "GitHub",
            "https://github.com/login",
            "octocat",
            "gh-pass",
        )),
        mail: save(with_totp(
            login(
                "Mail",
                "https://www.mail.example.com",
                "Max@Example.com",
                "mail-pass",
            ),
            "JBSWY3DPEHPK3PXP",
        )),
        router: save(login("Router", "", "admin", "router-pw")),
        visa: save(card("Visa", "4111 1111 1111 1111", "2027")),
        me: save(identity("Ich", "Max", "Muster", "max@example.com")),
        emergency: save(identity("Notfall", "", "", "")),
        wifi: save(note("WLAN", "Passwort: 123\nGast: 456")),
    };
    let old = save(login("Old", "old.example", "u", "p"));
    vault.trash_item(&old).unwrap();
    ids
}

/// The incoming items of the classification tests and their expected class.
fn incoming() -> Vec<VaultItem> {
    vec![
        // 0 duplicate: scheme-less, upper case host, username case/space.
        login("github", "GITHUB.com/x", " OctoCat ", "gh-pass"),
        // 1 duplicate: www./http differ; no TOTP on the incoming side.
        login(
            "Mail (copy)",
            "http://mail.example.com/inbox",
            "max@example.com",
            "mail-pass",
        ),
        // 2 duplicate: same TOTP secret written as otpauth URI.
        with_totp(
            login("Mail", "mail.example.com", "max@example.com", "mail-pass"),
            "otpauth://totp/Mail:max?secret=JBSWY3DPEHPK3PXP&issuer=Mail",
        ),
        // 3 conflict: different password (incoming has a TOTP seed).
        with_totp(
            login("GitHub", "https://github.com", "octocat", "NEW-gh-pass"),
            "JBSWY3DPEHPK3PXP",
        ),
        // 4 conflict: same password, different TOTP seed.
        with_totp(
            login(
                "Mail",
                "https://mail.example.com",
                "max@example.com",
                "mail-pass",
            ),
            "KRSXG5CTMVRXEZLU",
        ),
        // 5 conflict: empty password vs. a set one (site from the name).
        login("router", "", "ADMIN", ""),
        // 6 new: other username.
        login("GitHub work", "https://github.com", "octo-work", "w"),
        // 7 duplicate card (formatting and expiry differ).
        card("My Visa", "4111-1111-1111-1111", "2030"),
        // 8 new card.
        card("Master", "5500 0000 0000 0004", ""),
        // 9 duplicate identity (case).
        identity("Me", "MAX", "muster", "MAX@EXAMPLE.COM"),
        // 10 duplicate identity by name (no personal data).
        identity("notfall", "", "", ""),
        // 11 new identity.
        identity("Other", "Erika", "Muster", "erika@example.com"),
        // 12 duplicate note (line endings, trailing newline).
        note("wlan", "Passwort: 123\r\nGast: 456\n"),
        // 13 new note: same name, other text.
        note("WLAN", "Passwort: 999"),
        // 14 new: the existing "Old" is in the trash.
        login("Old", "old.example", "u", "p"),
        // 15 duplicate of 14 within the file.
        login("Old copy", "https://www.old.example/", "U", "p"),
        // 16 duplicate of the conflict 3 within the file.
        with_totp(
            login("GitHub", "https://github.com", "octocat", "NEW-gh-pass"),
            "JBSWY3DPEHPK3PXP",
        ),
    ]
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

#[test]
fn detects_encrypted_formats() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // Keystead export, whatever its extension.
    let data = VaultData {
        items: vec![login("A", "a.example", "u", "p")],
        ..Default::default()
    };
    let export = d.join("backup.dat");
    export_encrypted_with_params(&data, &export, "export-pw", KdfParams::insecure_for_tests())
        .unwrap();
    assert_eq!(
        detect_import(&export).unwrap(),
        DetectedImport {
            format: ImportFormat::Keystead,
            needs_password: true,
            file_name: "backup.dat".into(),
        }
    );
    // A vault file itself.
    let (_store, vault) = new_vault(d);
    assert_eq!(detected(vault.path()), ImportFormat::Keystead);

    // VaultX 1.x: with Mac + BOM, v1, empty Data, single entry.
    for name in [
        "legacy_v2.json",
        "legacy_v1.json",
        "legacy_v1_empty.json",
        "legacy_single_entry.json",
        "legacy_recovery.json",
    ] {
        let found = detect_import(&fixture(name)).unwrap();
        assert_eq!(found.format, ImportFormat::Legacy, "{name}");
        assert!(found.needs_password);
        assert_eq!(found.file_name, name);
    }
    // Content wins over a misleading extension.
    let renamed = d.join("legacy.csv");
    fs::copy(fixture("legacy_v2.json"), &renamed).unwrap();
    assert_eq!(detected(&renamed), ImportFormat::Legacy);

    // A Keystead file of an unknown format version is refused right away.
    let mut header: Value = serde_json::from_slice(&fs::read(&export).unwrap()).unwrap();
    header["version"] = json!(2);
    let v2 = write(d, "v2.keystead", serde_json::to_vec(&header).unwrap());
    assert_eq!(
        detect_import(&v2).unwrap_err().code(),
        "unsupported:vault format version 2"
    );
}

#[test]
fn detects_bitwarden_json() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let data = VaultData {
        items: vec![login("A", "a.example", "u", "p"), note("N", "x")],
        folders: vec![Folder {
            id: "f".into(),
            name: "F".into(),
        }],
        ..Default::default()
    };
    let own = write(d, "export.json", export_bitwarden_json(&data));
    assert_eq!(
        detect_import(&own).unwrap(),
        DetectedImport {
            format: ImportFormat::BitwardenJson,
            needs_password: false,
            file_name: "export.json".into(),
        }
    );
    // Organisation export (collections), BOM, no extension.
    let org = json!({ "encrypted": false, "collections": [], "items": [] });
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend(serde_json::to_vec(&org).unwrap());
    assert_eq!(
        detected(&write(d, "org", &bytes)),
        ImportFormat::BitwardenJson
    );
    // Bitwarden JSON saved as UTF-16 with BOM, and with a .csv name.
    let text = export_bitwarden_json(&data);
    assert_eq!(
        detected(&write(d, "utf16.json", utf16(&text, true))),
        ImportFormat::BitwardenJson
    );
    assert_eq!(
        detected(&write(d, "export.csv", &text)),
        ImportFormat::BitwardenJson
    );
    let parsed = read_import(
        &write(d, "x.json", &text),
        ImportFormat::BitwardenJson,
        None,
    )
    .unwrap();
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.folders.len(), 1);
}

#[test]
fn encrypted_bitwarden_export_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let password_protected = json!({
        "encrypted": true, "passwordProtected": true, "salt": "c2FsdA==", "kdfType": 0,
        "kdfIterations": 600000, "encKeyValidation_DO_NOT_EDIT": "2.a|b|c", "data": "2.d|e|f"
    });
    let account = json!({
        "encrypted": true, "encKeyValidation_DO_NOT_EDIT": "2.a|b|c",
        "folders": [ { "id": "f", "name": "2.x|y|z" } ],
        "items": [ { "type": 1, "name": "2.x|y|z", "login": { "password": "2.p|q|r" } } ]
    });
    for (name, doc) in [("pp.json", password_protected), ("acc.json", account)] {
        let path = write(d, name, serde_json::to_vec(&doc).unwrap());
        let err = detect_import(&path).unwrap_err();
        assert_eq!(err.code(), "unsupported:bitwarden_encrypted", "{name}");
    }
}

#[test]
fn detects_csv_dialects() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let bitwarden_csv = export_csv(&VaultData {
        items: vec![login("A", "a.example", "u", "p")],
        ..Default::default()
    });
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "chrome.csv",
            b"name,url,username,password,note\r\ngithub.com,https://github.com/,octocat,pw,\r\n"
                .to_vec(),
            "github.com",
        ),
        (
            "firefox.csv",
            b"\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
              \"https://accounts.example\",\"me\",\"pw\",,\"https://accounts.example\",\"{a}\",\"1\",\"1\",\"1\"\n"
                .to_vec(),
            "accounts.example",
        ),
        ("bitwarden.csv", bitwarden_csv.into_bytes(), "A"),
        (
            "semicolon.csv",
            "\u{feff}Title;URL;User;Password;Notes\r\nMail;mail.example;max;pw;n\r\n"
                .as_bytes()
                .to_vec(),
            "Mail",
        ),
        (
            "tabs.tsv",
            b"name\turl\tusername\tpassword\nTab\ttab.example\tu\tp\n".to_vec(),
            "Tab",
        ),
        (
            "excel.csv",
            b"sep=;\nWebsite;Login;Pass\nexcel.example;u;p\n".to_vec(),
            "excel.example",
        ),
        (
            "utf16le.csv",
            utf16("name;url;username;password\r\nUmlaut ü;u.example;u;p\r\n", true),
            "Umlaut ü",
        ),
        (
            "utf16be.csv",
            utf16("name,url,username,password\nBig,b.example,u,p\n", false),
            "Big",
        ),
        (
            "cp1252.csv",
            b"name;url;username;password\r\nM\xfcnchen \x80;m.example;u;p\r\n".to_vec(),
            "München €",
        ),
        // Content wins over the extension.
        (
            "really-a.json",
            b"name,url,username,password\nJ,j.example,u,p\n".to_vec(),
            "J",
        ),
        (
            "noext",
            b"name,url,username,password\nN,n.example,u,p\n".to_vec(),
            "N",
        ),
    ];
    for (name, bytes, first_item) in cases {
        let path = write(d, name, &bytes);
        let found = detect_import(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(found.format, ImportFormat::Csv, "{name}");
        assert!(!found.needs_password);
        let p = read_import(&path, found.format, None).unwrap();
        assert_eq!(p.items.len(), 1, "{name}");
        assert_eq!(p.items[0].name, first_item, "{name}");
    }
}

#[test]
fn unknown_files_are_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("text.txt", b"hello world\nthis is a letter\n".to_vec()),
        // A single recognisable column is not enough for detection.
        ("notes.txt", b"Notes\nbuy milk\n".to_vec()),
        ("empty.csv", Vec::new()),
        ("blank.csv", b"\r\n  \n".to_vec()),
        ("other.json", br#"{"hello": 1, "items": 3}"#.to_vec()),
        ("items-only.json", br#"{"items": []}"#.to_vec()),
        ("array.json", b"[1, 2, 3]".to_vec()),
        (
            "truncated.json",
            br#"{"encrypted": false, "items": [ {"#.to_vec(),
        ),
        (
            "image.png",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01".to_vec(),
        ),
    ];
    for (name, bytes) in cases {
        let path = write(d, name, &bytes);
        assert_eq!(
            unsupported(detect_import(&path)),
            "unknown_format",
            "{name}"
        );
    }
    // accounts.json of VaultX 1.x is not a vault.
    assert_eq!(
        unsupported(detect_import(&fixture("legacy_accounts.json"))),
        "unknown_format"
    );
    assert_eq!(
        detect_import(&fixture("legacy_accounts.json"))
            .unwrap_err()
            .code(),
        "unsupported:unknown_format"
    );
}

#[test]
fn large_missing_and_non_regular_files() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // A sparse file just over the limit: refused from its size.
    let big = d.join("big.csv");
    let file = fs::File::create(&big).unwrap();
    file.set_len(IMPORT_MAX_BYTES + 1).unwrap();
    drop(file);
    assert_eq!(unsupported(detect_import(&big)), "file_too_large");
    assert_eq!(
        unsupported(read_import(&big, ImportFormat::Csv, None)),
        "file_too_large"
    );
    let (_store, mut vault) = new_vault(d);
    assert_eq!(
        import_into(&mut vault, "csv", &big, None)
            .unwrap_err()
            .code(),
        "unsupported:file_too_large"
    );

    assert!(matches!(
        detect_import(&d.join("missing.csv")),
        Err(Error::NotFound(_))
    ));
    let sub = d.join("folder.csv");
    fs::create_dir(&sub).unwrap();
    let err = detect_import(&sub).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err:?}");
    assert!(err.code().starts_with("io:"));
    assert!(matches!(
        read_import(&sub, ImportFormat::Csv, None),
        Err(Error::Io(_))
    ));

    // A FIFO is never opened (opening it would block).
    #[cfg(target_os = "linux")]
    {
        let fifo = d.join("pipe.csv");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if made {
            assert!(matches!(detect_import(&fifo), Err(Error::Io(_))));
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn read_import_passwords_and_trash() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let missing = d.join("missing.keystead");
    // Checked before the file is read.
    for format in [ImportFormat::Legacy, ImportFormat::Keystead] {
        for pw in [None, Some("")] {
            assert_eq!(
                read_import(&missing, format, pw).unwrap_err().code(),
                "invalid_input:password_required"
            );
        }
    }
    assert!(matches!(
        read_import(
            &fixture("legacy_v2.json"),
            ImportFormat::Legacy,
            Some("wrong")
        ),
        Err(Error::WrongPassword)
    ));
    let p = read_import(
        &fixture("legacy_v2.json"),
        ImportFormat::Legacy,
        Some("Korrekt-Pferd-42!"),
    )
    .unwrap();
    assert_eq!(p.items.len(), 3);
    assert_eq!(p.warnings.len(), 1);

    // A vault file (unlike an export) contains its trash and generator
    // history: neither is imported.
    let (_store, mut vault) = new_vault(d);
    vault
        .save_folder(Folder {
            id: String::new(),
            name: "F".into(),
        })
        .unwrap();
    vault.save_item(login("A", "a.example", "u", "p")).unwrap();
    let trashed = vault.save_item(note("Trash", "x")).unwrap();
    vault.trash_item(&trashed.id).unwrap();
    vault.add_generated_password("generated").unwrap();
    assert!(matches!(
        read_import(vault.path(), ImportFormat::Keystead, Some("nope")),
        Err(Error::WrongPassword)
    ));
    let p = read_import(vault.path(), ImportFormat::Keystead, Some("pw")).unwrap();
    assert_eq!(p.items.len(), 1, "trash is never imported");
    assert_eq!(p.items[0].name, "A");
    assert_eq!(p.folders.len(), 1);
    assert_eq!(p.invalid, 0);

    // Invalid rows are counted.
    let csv = write(
        d,
        "bw.csv",
        "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
         ,,login,Ok,,,0,ok.example,u,p,\n,,login,,,,0,,,,\n",
    );
    let p = read_import(&csv, ImportFormat::Csv, None).unwrap();
    assert_eq!((p.items.len(), p.invalid, p.warnings.len()), (1, 1, 1));
    let dbg = format!("{p:?}");
    assert!(!dbg.contains("ok.example"), "{dbg}");
}

#[test]
fn format_names_and_serde_shapes() {
    for (format, name) in [
        (ImportFormat::Legacy, "legacy"),
        (ImportFormat::Csv, "csv"),
        (ImportFormat::BitwardenJson, "bitwarden_json"),
        (ImportFormat::Keystead, "keystead"),
    ] {
        assert_eq!(serde_json::to_value(format).unwrap(), json!(name));
        assert_eq!(name.parse::<ImportFormat>().unwrap(), format);
        assert_eq!(format.to_string(), name);
        assert_eq!(
            format.needs_password(),
            matches!(format, ImportFormat::Legacy | ImportFormat::Keystead)
        );
    }
    assert_eq!(
        "pdf".parse::<ImportFormat>().unwrap_err().code(),
        "invalid_input:format pdf"
    );

    assert_eq!(ConflictMode::default(), ConflictMode::Skip);
    for (mode, name) in [
        (ConflictMode::Skip, "skip"),
        (ConflictMode::Update, "update"),
        (ConflictMode::KeepBoth, "keepBoth"),
    ] {
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<ConflictMode>(json!(name)).unwrap(),
            mode
        );
    }

    let detected = DetectedImport {
        format: ImportFormat::BitwardenJson,
        needs_password: false,
        file_name: "x.json".into(),
    };
    assert_eq!(
        serde_json::to_value(&detected).unwrap(),
        json!({ "format": "bitwarden_json", "needsPassword": false, "fileName": "x.json" })
    );

    let report = serde_json::to_value(ImportReport::default()).unwrap();
    let mut keys: Vec<_> = report.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "conflictsSkipped",
            "duplicates",
            "imported",
            "skipped",
            "updated",
            "warnings"
        ]
    );
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

#[test]
fn plan_classifies_every_item_type() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    let ids = seed(&mut vault);
    let plan = plan_import(vault.data(), parsed(incoming()));

    let new: Vec<_> = plan.new_items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        new,
        ["GitHub work", "Master", "Other", "WLAN", "Old"],
        "{plan:?}"
    );

    let dups: Vec<_> = plan
        .duplicates
        .iter()
        .map(|m| (m.incoming_name.as_str(), m.existing_id.as_str()))
        .collect();
    assert_eq!(
        dups,
        [
            ("github", ids.github.as_str()),
            ("Mail (copy)", ids.mail.as_str()),
            ("Mail", ids.mail.as_str()),
            ("My Visa", ids.visa.as_str()),
            ("Me", ids.me.as_str()),
            ("notfall", ids.emergency.as_str()),
            ("wlan", ids.wifi.as_str()),
            ("Old copy", ""),
            ("GitHub", ""),
        ]
    );
    let gh = &plan.duplicates[0];
    assert_eq!(gh.username, "OctoCat");
    assert_eq!(gh.site, "github.com");
    assert_eq!(gh.item_type, ItemType::Login);
    assert_eq!(gh.existing_name, "GitHub");
    let visa = &plan.duplicates[3];
    assert_eq!(visa.item_type, ItemType::Card);
    assert_eq!(visa.username, "•••• 1111");
    assert_eq!(visa.site, "");
    // Duplicates within the file name the earlier item.
    assert_eq!(plan.duplicates[7].existing_name, "Old");
    assert_eq!(plan.duplicates[7].site, "old.example");

    let conflicts: Vec<_> = plan
        .conflicts
        .iter()
        .map(|c| {
            (
                c.conflict_id.as_str(),
                c.reason,
                c.entry.incoming_name.as_str(),
                c.entry.existing_id.as_str(),
            )
        })
        .collect();
    assert_eq!(
        conflicts,
        [
            (
                "conflict-1",
                ConflictReason::Password,
                "GitHub",
                ids.github.as_str()
            ),
            (
                "conflict-2",
                ConflictReason::Totp,
                "Mail",
                ids.mail.as_str()
            ),
            (
                "conflict-3",
                ConflictReason::Password,
                "router",
                ids.router.as_str()
            ),
        ]
    );
    assert_eq!(plan.conflicts[2].entry.site, "", "no URI");
    assert_eq!(plan.conflicts[2].entry.username, "ADMIN");

    // The preview carries no secrets.
    let preview = plan.preview();
    assert_eq!(preview.new_count, 5);
    let text = serde_json::to_string(&preview).unwrap();
    for secret in [
        "gh-pass",
        "mail-pass",
        "KRSXG5",
        "JBSWY3",
        "4111",
        "5500",
        "Passwort",
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let c = serde_json::to_value(&preview.conflicts[0]).unwrap();
    assert_eq!(c["conflictId"], "conflict-1");
    assert_eq!(c["reason"], "password");
    assert_eq!(c["incomingName"], "GitHub");
    assert_eq!(c["itemType"], "login");
    assert_eq!(c["existingId"], json!(ids.github));
    assert_eq!(c["site"], "github.com");
    assert_eq!(c["username"], "octocat");
    let dbg = format!("{plan:?}");
    assert!(!dbg.contains("pass") && !dbg.contains("GitHub"), "{dbg}");
}

#[test]
fn same_site_different_username_or_trashed_is_new() {
    let mut existing = VaultData::default();
    let mut gh = login("GitHub", "https://github.com", "octocat", "pw");
    gh.id = "gh".into();
    let mut trashed = login("Bank", "bank.example", "me", "pw");
    trashed.id = "bank".into();
    trashed.deleted_at = Some(1);
    existing.items = vec![gh, trashed];
    let plan = plan_import(
        &existing,
        parsed(vec![
            login("GitHub", "https://github.com", "someone", "pw"),
            login("Bank", "bank.example", "me", "other"),
            login("GitLab", "https://gitlab.com", "octocat", "pw"),
            // No URI: the name is the site; "github.com" as a name matches.
            login("github.com", "", "octocat", "pw"),
        ]),
    );
    assert_eq!(plan.new_items.len(), 3);
    assert!(plan.conflicts.is_empty());
    assert_eq!(plan.duplicates.len(), 1);
    assert_eq!(plan.duplicates[0].existing_id, "gh");
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

fn seeded_plan(dir: &TempDir) -> (VaultStore, UnlockedVault, Seeded, ImportPlan) {
    let (store, mut vault) = new_vault(dir.path());
    let ids = seed(&mut vault);
    let plan = plan_import(vault.data(), parsed(incoming()));
    (store, vault, ids, plan)
}

#[test]
fn commit_skip() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault, ids, plan) = seeded_plan(&dir);
    let before = vault.items().len();
    let revision = vault.revision();
    let report = vault.commit_import(plan, ConflictMode::Skip).unwrap();
    assert_eq!(vault.revision(), revision + 1, "one save");
    assert_eq!(report.imported, 5);
    assert_eq!(report.updated, 0);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.duplicates.len(), 9);
    let skipped: Vec<_> = report
        .conflicts_skipped
        .iter()
        .map(|m| m.existing_id.as_str())
        .collect();
    assert_eq!(
        skipped,
        [ids.github.as_str(), ids.mail.as_str(), ids.router.as_str()]
    );
    assert_eq!(vault.items().len(), before + 5);
    assert_eq!(vault.item(&ids.github).unwrap().password(), "gh-pass");
    assert_eq!(find(&vault, "GitHub").len(), 1);
    // The trashed "Old" stays in the trash; the incoming one is new.
    assert_eq!(find(&vault, "Old").len(), 1);
    let new_old = find(&vault, "Old")[0];
    assert_eq!(new_old.id.len(), 36);
    assert!(new_old.created_at > 0);
}

#[test]
fn commit_update_uses_the_save_path() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault, ids, plan) = seeded_plan(&dir);
    let revision = vault.revision();
    let before = vault.items().len();
    let report = vault.commit_import(plan, ConflictMode::Update).unwrap();
    assert_eq!(vault.revision(), revision + 1, "one save");
    assert_eq!(report.imported, 5);
    assert_eq!(report.updated, 1);
    // Same password with another TOTP seed (an existing seed is never
    // replaced) and an empty incoming password: nothing to take over.
    let skipped: Vec<_> = report
        .conflicts_skipped
        .iter()
        .map(|m| m.existing_id.as_str())
        .collect();
    assert_eq!(skipped, [ids.mail.as_str(), ids.router.as_str()]);
    assert_eq!(vault.items().len(), before + 5);

    let gh = vault.item(&ids.github).unwrap();
    let l = gh.login.as_ref().unwrap();
    assert_eq!(l.password, "NEW-gh-pass");
    assert_eq!(l.totp, "JBSWY3DPEHPK3PXP", "added: the login had none");
    assert_eq!(gh.password_history.len(), 1);
    assert_eq!(gh.password_history[0].password, "gh-pass");
    assert!(gh.password_history[0].replaced_at > 0);
    assert_eq!(
        l.password_revised_at,
        Some(gh.password_history[0].replaced_at)
    );
    assert_eq!(gh.updated_at, gh.password_history[0].replaced_at);
    assert_eq!(gh.name, "GitHub");
    assert_eq!(
        l.uris[0].uri, "https://github.com/login",
        "only secrets change"
    );

    let mail = vault.item(&ids.mail).unwrap().login.as_ref().unwrap();
    assert_eq!(mail.totp, "JBSWY3DPEHPK3PXP");
    let router = vault.item(&ids.router).unwrap();
    assert_eq!(router.password(), "router-pw");
    assert!(router.password_history.is_empty());
}

#[test]
fn commit_update_twice_keeps_both_in_history() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    let gh = vault
        .save_item(login("GitHub", "github.com", "octocat", "one"))
        .unwrap();
    let plan = plan_import(
        vault.data(),
        parsed(vec![
            login("GitHub", "github.com", "octocat", "two"),
            login("GitHub", "github.com", "octocat", "three"),
        ]),
    );
    assert_eq!(plan.conflicts.len(), 2);
    let report = vault.commit_import(plan, ConflictMode::Update).unwrap();
    assert_eq!(report.updated, 2);
    let gh = vault.item(&gh.id).unwrap();
    assert_eq!(gh.password(), "three");
    let history: Vec<_> = gh
        .password_history
        .iter()
        .map(|h| h.password.as_str())
        .collect();
    assert_eq!(history, ["two", "one"]);
}

#[test]
fn commit_keep_both() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault, ids, plan) = seeded_plan(&dir);
    let revision = vault.revision();
    let report = vault.commit_import(plan, ConflictMode::KeepBoth).unwrap();
    assert_eq!(vault.revision(), revision + 1);
    assert_eq!(report.imported, 8);
    assert_eq!(report.updated, 0);
    assert!(report.conflicts_skipped.is_empty());
    assert_eq!(report.duplicates.len(), 9);
    assert_eq!(vault.item(&ids.github).unwrap().password(), "gh-pass");
    let mut passwords: Vec<_> = find(&vault, "GitHub")
        .iter()
        .map(|i| i.password().to_owned())
        .collect();
    passwords.sort();
    assert_eq!(passwords, ["NEW-gh-pass", "gh-pass"]);
    assert_eq!(find(&vault, "Mail").len(), 2);
    assert_eq!(find(&vault, "router").len(), 1);
}

#[test]
fn commit_rechecks_the_current_vault() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    let gh = vault
        .save_item(login("GitHub", "github.com", "octocat", "gh"))
        .unwrap();
    let router = vault
        .save_item(login("Router", "192.168.0.1", "admin", "r1"))
        .unwrap();
    let mail = vault
        .save_item(login("Mail", "mail.example", "me", "m1"))
        .unwrap();
    let plan = plan_import(
        vault.data(),
        parsed(vec![
            login("GitHub", "github.com", "octocat", "gh-new"), // conflict
            login("Router", "192.168.0.1", "admin", "r2"),      // conflict
            login("Mail", "mail.example", "me", "m2"),          // conflict
            login("Shop", "shop.example", "me", "s1"),          // new
            login("Forum", "forum.example", "me", "f1"),        // new
        ]),
    );
    assert_eq!((plan.conflicts.len(), plan.new_items.len()), (3, 2));

    // Meanwhile: the conflicting GitHub login is deleted and the router
    // trashed (→ new items), the mail password changed to the incoming one
    // (→ duplicate), "Shop" saved (→ duplicate) and "Forum" saved with
    // another password (→ conflict, skipped).
    vault.delete_item(&gh.id).unwrap();
    vault.trash_item(&router.id).unwrap();
    let mut changed = mail.clone();
    changed.login.as_mut().unwrap().password = "m2".into();
    vault.save_item(changed).unwrap();
    let shop = vault
        .save_item(login("My shop", "https://www.shop.example", "ME", "s1"))
        .unwrap();
    let forum = vault
        .save_item(login("Forum", "forum.example", "me", "other"))
        .unwrap();

    let revision = vault.revision();
    let report = vault.commit_import(plan, ConflictMode::Skip).unwrap();
    assert_eq!(vault.revision(), revision + 1);
    assert_eq!(report.imported, 2, "{report:?}");
    let gh_now = find(&vault, "GitHub");
    assert_eq!(gh_now.len(), 1);
    assert_eq!(gh_now[0].password(), "gh-new");
    assert_eq!(find(&vault, "Router")[0].password(), "r2");
    let dups: Vec<_> = report
        .duplicates
        .iter()
        .map(|m| (m.incoming_name.as_str(), m.existing_id.as_str()))
        .collect();
    assert_eq!(
        dups,
        [("Shop", shop.id.as_str()), ("Mail", mail.id.as_str())]
    );
    assert_eq!(report.conflicts_skipped.len(), 1);
    assert_eq!(report.conflicts_skipped[0].existing_id, forum.id);
    assert_eq!(vault.item(&forum.id).unwrap().password(), "other");
}

#[test]
fn commit_ref_survives_a_save_conflict_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut vault) = new_vault(dir.path());
    vault
        .save_item(login("GitHub", "github.com", "octocat", "gh"))
        .unwrap();
    let plan = plan_import(
        vault.data(),
        parsed(vec![
            login("GitHub", "github.com", "octocat", "gh-new"),
            login("Shop", "shop.example", "me", "s1"),
        ]),
    );

    // Another process saves first: the commit fails and changes nothing.
    let mut other = store.unlock(vault.id(), "pw").unwrap();
    other.save_item(note("Elsewhere", "x")).unwrap();
    let items = vault.items().len();
    assert!(matches!(
        vault.commit_import_ref(&plan, ConflictMode::KeepBoth),
        Err(Error::Conflict)
    ));
    assert_eq!(vault.items().len(), items);
    assert!(vault.reload_if_changed().unwrap());

    let report = vault
        .commit_import_ref(&plan, ConflictMode::KeepBoth)
        .unwrap();
    assert_eq!(report.imported, 2);
    let revision = vault.revision();
    let again = vault
        .commit_import_ref(&plan, ConflictMode::KeepBoth)
        .unwrap();
    assert_eq!(again.imported, 0);
    assert_eq!(again.duplicates.len(), 2);
    assert_eq!(vault.revision(), revision, "nothing to write");
    assert_eq!(find(&vault, "Shop").len(), 1);
}

#[test]
fn commit_leaves_out_removed_conflicts_and_unused_folders() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    let social = vault
        .save_folder(Folder {
            id: String::new(),
            name: "Social".into(),
        })
        .unwrap();
    vault
        .save_item(login("GitHub", "github.com", "octocat", "gh"))
        .unwrap();
    vault
        .save_item(login("Bank", "bank.example", "me", "b"))
        .unwrap();
    let mut p = parsed(vec![
        // Duplicate in a folder that does not exist (no folder created).
        VaultItem {
            folder_id: Some("f-banking".into()),
            ..login("Bank", "bank.example", "me", "b")
        },
        // New, in a folder merged into the existing "Social".
        VaultItem {
            folder_id: Some("f-social".into()),
            ..login("Mastodon", "mastodon.social", "me", "m")
        },
        // New, in a new folder.
        VaultItem {
            folder_id: Some("f-work".into()),
            ..login("Jira", "jira.example", "me", "j")
        },
        login("GitHub", "github.com", "octocat", "gh-new"),
    ]);
    p.folders = ["Banking", "social", "Work", "Empty"]
        .iter()
        .map(|name| Folder {
            id: format!("f-{}", name.to_lowercase()),
            name: (*name).into(),
        })
        .collect();
    p.invalid = 2;
    p.warnings = vec!["Row 3 skipped".into(), "Row 4 skipped".into()];
    let mut plan = plan_import(vault.data(), p);
    assert_eq!(plan.conflicts.len(), 1);
    plan.conflicts.clear();

    let report = vault.commit_import(plan, ConflictMode::KeepBoth).unwrap();
    assert_eq!(report.imported, 2);
    assert_eq!(report.skipped, 2);
    assert_eq!(report.warnings.len(), 2);
    assert!(report.conflicts_skipped.is_empty());
    assert_eq!(find(&vault, "GitHub").len(), 1, "removed conflict left out");
    let names: Vec<_> = vault.folders().iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Social", "Work"]);
    let work = &vault.folders()[1];
    assert_eq!(
        find(&vault, "Mastodon")[0].folder_id.as_deref(),
        Some(social.id.as_str())
    );
    assert_eq!(
        find(&vault, "Jira")[0].folder_id.as_deref(),
        Some(work.id.as_str())
    );
}

#[test]
fn commit_without_changes_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    vault
        .save_item(login("GitHub", "github.com", "octocat", "gh"))
        .unwrap();
    let revision = vault.revision();
    let plan = plan_import(
        vault.data(),
        parsed(vec![
            login("GitHub", "https://github.com", "octocat", "gh"),
            login("GitHub", "github.com", "octocat", "other"),
        ]),
    );
    let report = vault.commit_import(plan, ConflictMode::Skip).unwrap();
    assert_eq!((report.imported, report.updated), (0, 0));
    assert_eq!(report.duplicates.len(), 1);
    assert_eq!(report.conflicts_skipped.len(), 1);
    assert_eq!(vault.revision(), revision);

    let empty = plan_import(vault.data(), ParsedImport::default());
    let report = vault.commit_import(empty, ConflictMode::Update).unwrap();
    assert_eq!(report, ImportReport::default());
    assert_eq!(vault.revision(), revision);
}

// ---------------------------------------------------------------------------
// import_into (the import_data command) and the full drag & drop flow
// ---------------------------------------------------------------------------

#[test]
fn import_into_skips_duplicates_and_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (_store, mut vault) = new_vault(d);
    let first = write(
        d,
        "chrome.csv",
        "name,url,username,password,note\r\n\
         GitHub,https://github.com/login,octocat,gh,\r\n\
         Mail,https://mail.example,me,m,\r\n\
         Mail,https://www.mail.example/,ME,m,\r\n\
         Shop,https://shop.example,me,s,\r\n",
    );
    let report = import_into(&mut vault, "csv", &first, None).unwrap();
    assert_eq!(report.imported, 3);
    assert_eq!(report.duplicates.len(), 1, "within the file");
    assert_eq!(report.duplicates[0].existing_id, "");

    let revision = vault.revision();
    let report = import_into(&mut vault, "csv", &first, None).unwrap();
    assert_eq!(report.imported, 0);
    assert_eq!(report.duplicates.len(), 4);
    assert_eq!(vault.revision(), revision);

    let second = write(
        d,
        "firefox.csv",
        "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
         \"https://github.com\",\"octocat\",\"gh-changed\",,,\"{a}\",\"1\",\"1\",\"1\"\n\
         \"https://news.example\",\"me\",\"n\",,,\"{b}\",\"1\",\"1\",\"1\"\n",
    );
    let report = import_into(&mut vault, "csv", &second, None).unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(report.updated, 0);
    assert_eq!(report.conflicts_skipped.len(), 1);
    assert_eq!(report.conflicts_skipped[0].site, "github.com");
    assert_eq!(find(&vault, "GitHub")[0].password(), "gh");
    assert_eq!(vault.items().len(), 4);
}

#[test]
fn drag_and_drop_flow_between_vaults() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let store = VaultStore::new(d.join("data"));
    let mut source = store
        .create_vault_with_params("Quelle", "src-pw", KdfParams::insecure_for_tests())
        .unwrap();
    source
        .save_item(login("GitHub", "github.com", "octocat", "gh-2"))
        .unwrap();
    source
        .save_item(login("Shop", "shop.example", "me", "s"))
        .unwrap();
    source
        .save_item(card("Visa", "4111111111111111", ""))
        .unwrap();
    let mut target = store
        .create_vault_with_params("Ziel", "dst-pw", KdfParams::insecure_for_tests())
        .unwrap();
    let gh = target
        .save_item(login("GitHub", "https://www.github.com", "Octocat", "gh-1"))
        .unwrap();
    target
        .save_item(card("Kreditkarte", "4111 1111 1111 1111", ""))
        .unwrap();

    // The other vault's file is dropped onto the window.
    let found = detect_import(source.path()).unwrap();
    assert_eq!(found.format, ImportFormat::Keystead);
    assert!(found.needs_password);
    assert_eq!(
        read_import(source.path(), found.format, None)
            .unwrap_err()
            .code(),
        "invalid_input:password_required"
    );
    let parsed = read_import(source.path(), found.format, Some("src-pw")).unwrap();
    let plan = plan_import(target.data(), parsed);
    let preview = plan.preview();
    assert_eq!(preview.new_count, 1);
    assert_eq!(preview.duplicates.len(), 1);
    assert_eq!(preview.conflicts.len(), 1);
    assert_eq!(preview.conflicts[0].entry.existing_id, gh.id);

    let report = target.commit_import(plan, ConflictMode::Update).unwrap();
    assert_eq!((report.imported, report.updated), (1, 1));
    let gh = target.item(&gh.id).unwrap();
    assert_eq!(gh.password(), "gh-2");
    assert_eq!(gh.password_history[0].password, "gh-1");
}
