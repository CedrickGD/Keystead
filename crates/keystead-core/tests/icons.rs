//! Website icons inside the vault: storing fetch results, pruning,
//! persistence, conflicts and exports.

use keystead_core::export::{export_bitwarden_json, export_csv, export_encrypted_with_params};
use keystead_core::icons::{self, ICON_REFRESH_MS, ICON_RETRY_MS};
use keystead_core::import::{import_keystead_export, plan_import, read_import, ImportFormat};
use keystead_core::model::{ItemType, LoginData, LoginUri, UriMatch, VaultData, VaultItem};
use keystead_core::{Error, KdfParams, UnlockedVault, VaultStore};

const PW: &str = "pw";
const DAY: i64 = 24 * 60 * 60 * 1000;

fn store() -> (tempfile::TempDir, VaultStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = VaultStore::new(dir.path());
    (dir, store)
}

fn create(store: &VaultStore) -> UnlockedVault {
    store
        .create_vault_with_params("Icons", PW, KdfParams::insecure_for_tests())
        .expect("create vault")
}

fn login(name: &str, uri: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Login, name);
    item.login = Some(LoginData {
        username: "user".into(),
        password: "secret".into(),
        uris: vec![LoginUri {
            uri: uri.into(),
            match_type: UriMatch::Domain,
        }],
        ..Default::default()
    });
    item
}

/// A tiny but real PNG (1×1, transparent).
fn png() -> Vec<u8> {
    keystead_core::crypto::b64_decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
    )
    .unwrap()
}

#[test]
fn fetch_results_are_stored_in_one_save_and_survive_unlock() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let now = 1_700_000_000_000;
    v.save_item(login("GitHub", "https://www.github.com/login"))
        .unwrap();
    v.save_item(login("GitHub 2", "github.com")).unwrap();
    v.save_item(login("Bank", "https://bank.example.com"))
        .unwrap();
    v.save_item(login("Router", "http://192.168.178.1"))
        .unwrap();
    assert_eq!(
        v.icon_hosts_needing_fetch(now),
        vec!["bank.example.com", "github.com"]
    );

    let rev = v.revision();
    let written = v
        .set_icons(
            &[
                ("github.com".into(), Some(png())),
                ("bank.example.com".into(), None),
                // Not a host of any login: ignored.
                ("evil.example.org".into(), Some(png())),
            ],
            now,
        )
        .unwrap();
    assert_eq!(written, 2);
    assert_eq!(v.revision(), rev + 1, "one save for the whole run");
    assert!(v.icon_hosts_needing_fetch(now).is_empty());
    let expected = keystead_core::crypto::b64_encode(&png());
    assert_eq!(v.icon_for_host("github.com"), Some(expected.as_str()));
    assert_eq!(v.icon_for_host("bank.example.com"), None);
    assert_eq!(v.icon_for_host("evil.example.org"), None);
    let gh = v.items().iter().find(|i| i.name == "GitHub 2").unwrap();
    assert_eq!(v.icon_for_item(gh), Some(expected.as_str()));

    // Retries and refreshes.
    assert_eq!(
        v.icon_hosts_needing_fetch(now + ICON_RETRY_MS),
        vec!["bank.example.com"]
    );
    assert_eq!(
        v.icon_hosts_needing_fetch(now + ICON_REFRESH_MS),
        vec!["bank.example.com", "github.com"]
    );

    // Nothing to store → no save.
    assert_eq!(v.set_icons(&[], now).unwrap(), 0);
    assert_eq!(v.revision(), rev + 1);

    // Persisted inside the encrypted payload.
    let raw = std::fs::read_to_string(v.path()).unwrap();
    assert!(!raw.contains("github.com") && !raw.contains("icons"));
    let id = v.id().to_owned();
    drop(v);
    let v = store.unlock(&id, PW).unwrap();
    assert_eq!(v.icon_for_host("github.com"), Some(expected.as_str()));
    let entry = &v.data().icons["bank.example.com"];
    assert_eq!((entry.fetched_at, entry.failed_at), (now, Some(now)));
}

#[test]
fn icons_leave_with_their_last_login() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let a = v.save_item(login("A", "https://a.example.com")).unwrap();
    let a2 = v.save_item(login("A2", "a.example.com/x")).unwrap();
    let b = v.save_item(login("B", "https://b.example.com")).unwrap();
    v.set_icons(
        &[
            ("a.example.com".into(), Some(png())),
            ("b.example.com".into(), Some(png())),
        ],
        1,
    )
    .unwrap();

    // Another login still uses a.example.com.
    v.trash_item(&a.id).unwrap();
    assert!(v.icon_for_host("a.example.com").is_some());
    v.delete_item(&a2.id).unwrap();
    assert!(!v.data().icons.contains_key("a.example.com"));

    // Changing the address drops the old host's icon.
    let mut moved = v.item(&b.id).unwrap().clone();
    moved.login.as_mut().unwrap().uris[0].uri = "https://c.example.com".into();
    v.save_item(moved).unwrap();
    assert!(v.data().icons.is_empty());
    assert_eq!(v.icon_hosts_needing_fetch(2), vec!["c.example.com"]);

    // Restoring the trashed login makes its host due again.
    v.restore_item(&a.id).unwrap();
    assert_eq!(
        v.icon_hosts_needing_fetch(2),
        vec!["a.example.com", "c.example.com"]
    );
}

#[test]
fn clear_icons_removes_everything() {
    let (_dir, store) = store();
    let mut v = create(&store);
    v.save_item(login("A", "https://a.example.com")).unwrap();
    assert_eq!(v.clear_icons().unwrap(), 0);
    v.set_icons(&[("a.example.com".into(), Some(png()))], 1)
        .unwrap();
    let rev = v.revision();
    assert_eq!(v.clear_icons().unwrap(), 1);
    assert_eq!(v.revision(), rev + 1);
    assert!(v.data().icons.is_empty());
    assert_eq!(v.icon_hosts_needing_fetch(2), vec!["a.example.com"]);
}

#[test]
fn set_icons_after_a_conflict_can_be_retried() {
    let (_dir, store) = store();
    let mut app = create(&store);
    let id = app.id().to_owned();
    app.save_item(login("A", "https://a.example.com")).unwrap();
    let mut tui = store.unlock(&id, PW).unwrap();
    // Another process adds a login and trashes nothing.
    tui.save_item(login("B", "https://b.example.com")).unwrap();

    let results = vec![
        ("a.example.com".to_owned(), Some(png())),
        ("b.example.com".to_owned(), Some(png())),
    ];
    assert!(matches!(app.set_icons(&results, 5), Err(Error::Conflict)));
    assert!(app.data().icons.is_empty(), "unchanged after the conflict");
    assert!(app.reload_if_changed().unwrap());
    assert_eq!(app.set_icons(&results, 5).unwrap(), 2);
    assert_eq!(app.data().icons.len(), 2);
    // The other process's login survived.
    assert!(app.items().iter().any(|i| i.name == "B"));
}

#[test]
fn vaults_without_icons_still_open() {
    // Payload JSON written before icons existed.
    let old = r#"{"items":[],"folders":[],"generatorHistory":[]}"#;
    let data: VaultData = serde_json::from_str(old).unwrap();
    assert!(data.icons.is_empty());
    let v = serde_json::to_value(&data).unwrap();
    assert_eq!(v["icons"], serde_json::json!({}));
    let with = r#"{"icons":{"example.com":{"png":null,"fetchedAt":5,"failedAt":5}}}"#;
    let data: VaultData = serde_json::from_str(with).unwrap();
    assert_eq!(data.icons["example.com"].failed_at, Some(5));
}

#[test]
fn exports_never_carry_icons_and_imports_never_bring_them() {
    let (dir, store) = store();
    let mut v = create(&store);
    v.save_item(login("A", "https://a.example.com")).unwrap();
    v.set_icons(&[("a.example.com".into(), Some(png()))], 1)
        .unwrap();
    let b64 = keystead_core::crypto::b64_encode(&png());

    assert!(!export_csv(v.data()).contains(&b64));
    let json = export_bitwarden_json(v.data());
    assert!(!json.contains(&b64) && !json.contains("icons"));

    let path = dir.path().join("export.keystead");
    export_encrypted_with_params(v.data(), &path, "export", KdfParams::insecure_for_tests())
        .unwrap();
    let file = keystead_core::format::VaultFile::read(&path).unwrap();
    let key = file.unwrap_key("export").unwrap();
    let exported = file.decrypt_payload(&key).unwrap();
    assert_eq!(exported.items.len(), 1);
    assert!(exported.icons.is_empty());
    let (items, _) = import_keystead_export(&path, "export").unwrap();
    assert_eq!(items.len(), 1);

    // An import plan never touches icons: committing adds items only.
    let mut other = create(&store);
    let parsed = read_import(&path, ImportFormat::Keystead, Some("export")).unwrap();
    let plan = plan_import(other.data(), parsed);
    other
        .commit_import(plan, keystead_core::import::ConflictMode::Skip)
        .unwrap();
    assert_eq!(other.items().len(), 1);
    assert!(other.data().icons.is_empty());
    assert_eq!(other.icon_hosts_needing_fetch(DAY), vec!["a.example.com"]);
}

#[test]
fn local_hosts_are_never_due() {
    let (_dir, store) = store();
    let mut v = create(&store);
    for uri in [
        "http://localhost:8080",
        "http://nas:5000",
        "https://printer.local",
        "http://[::1]/",
        "https://10.0.0.1",
        "https://fritz.box.lan",
    ] {
        v.save_item(login(uri, uri)).unwrap();
    }
    assert!(v.icon_hosts_needing_fetch(DAY).is_empty());
    assert!(!icons::is_fetchable_host("nas"));
}
