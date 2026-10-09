//! Drag & drop import with realistic export files: browser and Bitwarden
//! CSV dialects, files that must not be taken for an import, tricky URLs,
//! re-importing Keystead's own exports and concurrent changes.

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::export::{export_bitwarden_json, export_csv, export_encrypted_with_params};
use keystead_core::import::{
    detect_import, import_into, plan_import, read_import, ConflictMode, ImportFormat, ParsedImport,
    IMPORT_MAX_BYTES,
};
use keystead_core::model::{
    CardData, CustomField, FieldKind, Folder, IdentityData, ItemType, LoginData, LoginUri,
    UriMatch, VaultItem,
};
use keystead_core::{KdfParams, UnlockedVault, VaultStore};

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

fn parsed(items: Vec<VaultItem>) -> ParsedImport {
    let mut p = ParsedImport::default();
    p.items.extend(items);
    p
}

/// Detected format, or the error code.
fn detect(path: &Path) -> Result<ImportFormat, String> {
    detect_import(path).map(|d| d.format).map_err(|e| e.code())
}

fn csv_items(path: &Path) -> ParsedImport {
    assert_eq!(detect(path), Ok(ImportFormat::Csv), "{}", path.display());
    read_import(path, ImportFormat::Csv, None).unwrap()
}

#[test]
fn browser_and_bitwarden_csv_exports() {
    let dir = tempfile::tempdir().unwrap();
    // Chrome: android entry, quoted password with comma and quote.
    let chrome = "name,url,username,password,note\r\n\
        accounts.google.com,https://accounts.google.com/signin/v2/challenge/pwd,max@gmail.com,g00gle!,\r\n\
        com.spotify.music,android://Xk1Q==@com.spotify.music/,max@gmail.com,sp0t,\r\n\
        github.com,https://github.com/session,octocat,\"pa,ss\"\"word\",line1\r\n";
    let r = csv_items(&write(dir.path(), "Chrome Passwords.csv", chrome));
    assert_eq!(r.items.len(), 3, "{:?}", r.warnings);
    assert_eq!(r.items[2].password(), "pa,ss\"word");

    // Edge (no note column).
    let edge = "name,url,username,password\nexample.com,https://example.com/,u,p\n";
    let r = csv_items(&write(dir.path(), "Microsoft Edge Passwords.csv", edge));
    assert_eq!(r.items.len(), 1);

    // Firefox, including its internal chrome:// entry.
    let firefox = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\r\n\
        \"https://www.mozilla.org\",\"me@x.de\",\"pw1\",,\"https://www.mozilla.org\",\"{a1b2}\",\"1600000000000\",\"1600000000000\",\"1600000000000\"\r\n\
        \"chrome://FirefoxAccounts\",\"me@x.de\",\"{\"\"version\"\":1}\",\"Firefox Accounts credentials\",,\"{c3}\",\"1600000000000\",\"1600000000000\",\"1600000000000\"\r\n";
    let r = csv_items(&write(dir.path(), "logins.csv", firefox));
    assert_eq!(r.items.len(), 2);

    // Bitwarden CSV with a folder and a multi-line note.
    let bitwarden = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
        Social,1,login,Twitter,,,0,https://twitter.com,me,tw,\n\
        ,,note,Secret note,\"multi\nline\",,0,,,,\n";
    let r = csv_items(&write(
        dir.path(),
        "bitwarden_export_20261009.csv",
        bitwarden,
    ));
    assert_eq!((r.items.len(), r.folders.len()), (2, 1));

    // Excel: UTF-8 BOM, then a `sep=;` line.
    let mut excel = vec![0xEF, 0xBB, 0xBF];
    excel.extend_from_slice(
        b"sep=;\r\nname;url;username;password\r\nM\xC3\xBCller;https://x.de;m;p\r\n",
    );
    let r = csv_items(&write(dir.path(), "excel.csv", &excel));
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].name, "Müller");

    // Windows-1252 without BOM, semicolons.
    let r = csv_items(&write(
        dir.path(),
        "cp1252.csv",
        b"name;url;username;password\r\nM\xFCller;https://x.de;m;p\xE4\r\n",
    ));
    assert_eq!(r.items[0].password(), "pä");

    // UTF-16 big endian with BOM.
    let mut utf16 = vec![0xFE, 0xFF];
    for unit in "url,username,password\nhttps://a.com,u,p\n".encode_utf16() {
        utf16.extend_from_slice(&unit.to_be_bytes());
    }
    assert_eq!(
        csv_items(&write(dir.path(), "u16.csv", &utf16)).items.len(),
        1
    );

    // Header only: a CSV without rows.
    let r = csv_items(&write(
        dir.path(),
        "empty.csv",
        "name,url,username,password\r\n",
    ));
    assert!(r.items.is_empty());
}

#[test]
fn other_files_are_not_imports() {
    let dir = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[u8])] = &[
        (
            "readme.md",
            b"# Title\n\nSome text, with commas; and semicolons.\n",
        ),
        ("notes.txt", b"Notes\nbuy milk\n"),
        ("truncated.json", b"{\"format\":\"keystead\",\"version\":1"),
        ("items-object.json", b"{\"encrypted\":false,\"items\":{}}"),
        ("array.json", b"[{\"name\":\"x\"}]"),
        ("object.json", b"{\"name\":\"x\",\"password\":\"y\"}"),
        ("image.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
        ("blank.csv", b"\n\n\n"),
        ("null.json", b"null"),
        ("salt-only.json", b"{\"Salt\":\"abc\"}"),
        (
            "package.json",
            b"{\"name\":\"x\",\"version\":\"1.0.0\",\"items\":[1]}",
        ),
    ];
    for (name, bytes) in cases {
        let path = write(dir.path(), name, bytes);
        assert_eq!(
            detect(&path),
            Err("unsupported:unknown_format".to_owned()),
            "{name}"
        );
    }
    // Account- and password-protected Bitwarden exports.
    for (name, text) in [
        (
            "bw_account.json",
            r#"{"encrypted":true,"encKeyValidation_DO_NOT_EDIT":"2.x|y|z","folders":[],"items":[]}"#,
        ),
        (
            "bw_password.json",
            r#"{"encrypted":true,"passwordProtected":true,"salt":"abc","kdfType":0,"kdfIterations":600000,"encKeyValidation_DO_NOT_EDIT":"2.x","data":"2.y"}"#,
        ),
    ] {
        assert_eq!(
            detect(&write(dir.path(), name, text)),
            Err("unsupported:bitwarden_encrypted".to_owned())
        );
    }
    // A Keystead marker with a broken header is refused before a password
    // is asked for.
    let broken = write(
        dir.path(),
        "broken.keystead",
        r#"{"format":"keystead","version":1}"#,
    );
    assert!(detect(&broken).unwrap_err().starts_with("corrupt:"));
}

#[test]
fn size_limit_and_password_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.csv");
    fs::File::create(&path)
        .unwrap()
        .set_len(IMPORT_MAX_BYTES + 1)
        .unwrap();
    assert_eq!(detect(&path), Err("unsupported:file_too_large".to_owned()));
    let code = |format, password| {
        read_import(&path, format, password)
            .map(|_| ())
            .unwrap_err()
            .code()
    };
    assert_eq!(code(ImportFormat::Csv, None), "unsupported:file_too_large");
    // The password is checked before the file is touched.
    assert_eq!(
        code(ImportFormat::Keystead, None),
        "invalid_input:password_required"
    );
}

#[test]
fn legacy_vault_saved_as_utf16() {
    // PowerShell 5 `Out-File` writes UTF-16LE with BOM.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_v1.json");
    let text = fs::read_to_string(fixture).unwrap();
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "vault_x_0123abcd.json", &bytes);
    assert_eq!(detect(&path), Ok(ImportFormat::Legacy));
}

#[test]
fn tricky_urls_and_usernames() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    let mut save = |item| vault.save_item(item).unwrap();
    save(login("Shop", "https://shop.example.com:443/", "Max", "a"));
    save(login("Bücher", "https://bücher.de/login", "max", "b"));
    save(login(
        "Intranet",
        "http://intranet.local:8080/x",
        "max",
        "c",
    ));
    save(login("App", "android://hash@com.app/", "max", "d"));
    save(login("Sub", "https://login.sub.example.com", "max", "e"));
    let plan = plan_import(
        vault.data(),
        parsed(vec![
            // duplicates
            login("x", "shop.example.com", " max ", "a"),
            login("x", "HTTP://WWW.SHOP.EXAMPLE.COM/path/", "MAX", "a"),
            login("x", "https://xn--bcher-kva.de", "max", "b"),
            login("x", "intranet.local:9999", "max", "c"),
            login("App", "android://other@com.app/", "MAX", "d"),
            login("x", "https://user:pw@shop.example.com/", "max", "a"),
            // new: other hosts
            login("x", "https://sub.example.com", "max", "e"),
            login("x", "https://example.com/", "max", "e"),
            // conflict
            login("x", "https://shop.example.com/", "max", "zzz"),
            // new (empty username), then the same again in the file
            login("x", "https://shop.example.com/", "", "a"),
            login("x", "https://shop.example.com/", "", "a"),
        ]),
    );
    let existing: Vec<&str> = plan
        .duplicates
        .iter()
        .map(|d| d.existing_name.as_str())
        .collect();
    assert_eq!(
        existing,
        ["Shop", "Shop", "Bücher", "Intranet", "App", "Shop", "x"]
    );
    assert_eq!(plan.duplicates[6].existing_id, "");
    assert_eq!(plan.duplicates[2].site, "xn--bcher-kva.de");
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.new_items.len(), 3);
    // The preview sent to the UI contains no password.
    let json = serde_json::to_string(&plan.preview()).unwrap();
    for password in ["\"a\"", "\"b\"", "zzz"] {
        assert!(!json.contains(password), "{json}");
    }
}

#[test]
fn reimporting_own_exports_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut vault) = new_vault(dir.path());
    let folder = vault
        .save_folder(Folder {
            id: String::new(),
            name: "Arbeit".into(),
        })
        .unwrap();
    let mut github = login(
        "  GitHub ",
        "https://github.com",
        "=octo",
        " pw with space ",
    );
    github.folder_id = Some(folder.id);
    github.notes = "=SUM(A1)".into();
    github.fields = vec![CustomField {
        name: "+pin".into(),
        value: "1234".into(),
        kind: FieldKind::Hidden,
    }];
    github.login.as_mut().unwrap().totp = "JBSWY3DPEHPK3PXP".into();
    for item in [
        github,
        login("Router", "", "admin", "r"),
        login("App only", "androidapp://com.x", "", ""),
        VaultItem {
            item_type: ItemType::Note,
            name: "Note".into(),
            notes: "line1\nline2\n".into(),
            ..Default::default()
        },
        VaultItem {
            item_type: ItemType::Card,
            name: "Visa".into(),
            card: Some(CardData {
                number: "4111111111111111".into(),
                exp_month: "1".into(),
                exp_year: "27".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        VaultItem {
            item_type: ItemType::Identity,
            name: "Me".into(),
            identity: Some(IdentityData {
                first_name: "Max".into(),
                email: "m@x.de".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    ] {
        vault.save_item(item).unwrap();
    }
    let total = vault.items().len();
    let csv = write(dir.path(), "x.csv", export_csv(vault.data()));
    let json = write(dir.path(), "x.json", export_bitwarden_json(vault.data()));
    let encrypted = dir.path().join("x.keystead");
    export_encrypted_with_params(
        vault.data(),
        &encrypted,
        "exp",
        KdfParams::insecure_for_tests(),
    )
    .unwrap();
    assert_eq!(detect(&csv), Ok(ImportFormat::Csv));
    assert_eq!(detect(&json), Ok(ImportFormat::BitwardenJson));
    assert_eq!(detect(&encrypted), Ok(ImportFormat::Keystead));

    let revision = vault.revision();
    for (path, format, password) in [
        (&csv, "csv", None),
        (&json, "bitwarden_json", None),
        (&encrypted, "keystead", Some("exp")),
    ] {
        let report = import_into(&mut vault, format, path, password).unwrap();
        assert_eq!(report.imported, 0, "{format}: {report:?}");
        assert!(report.conflicts_skipped.is_empty(), "{format}: {report:?}");
    }
    assert_eq!(vault.items().len(), total);
    assert_eq!(vault.folders().len(), 1);
    assert_eq!(vault.revision(), revision, "nothing written");
}

#[test]
fn update_after_another_process_changed_the_login() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut vault) = new_vault(dir.path());
    let github = vault
        .save_item(login("GitHub", "github.com", "octo", "old"))
        .unwrap();
    let plan = plan_import(
        vault.data(),
        parsed(vec![login("GitHub", "github.com", "octo", "new")]),
    );
    assert_eq!(plan.conflicts.len(), 1);

    let mut other = store.unlock(vault.id(), "pw").unwrap();
    let mut changed = github.clone();
    changed.login.as_mut().unwrap().password = "third".into();
    other.save_item(changed).unwrap();
    assert!(vault.reload_if_changed().unwrap());

    let revision = vault.revision();
    let report = vault.commit_import(plan, ConflictMode::Update).unwrap();
    assert_eq!(report.updated, 1);
    assert_eq!(vault.revision(), revision + 1);
    let item = vault.item(&github.id).unwrap();
    assert_eq!(item.password(), "new");
    let history: Vec<&str> = item
        .password_history
        .iter()
        .map(|h| h.password.as_str())
        .collect();
    assert_eq!(history, ["third", "old"]);

    // A file with nothing but duplicates writes nothing.
    let path = write(
        dir.path(),
        "dups.csv",
        "name,url,username,password\nGitHub,https://github.com,OCTO,new\nGitHub,https://www.github.com/,octo ,new\n",
    );
    let revision = vault.revision();
    let report = import_into(&mut vault, "csv", &path, None).unwrap();
    assert_eq!((report.imported, report.duplicates.len()), (0, 2));
    assert!(report.duplicates.iter().all(|d| d.existing_id == github.id));
    assert_eq!(vault.revision(), revision);
}
