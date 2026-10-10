//! Integration tests for VaultStore / UnlockedVault: create, unlock, items,
//! folders, passwords, recovery, conflicts, tamper detection.

use std::fs;
use std::path::{Path, PathBuf};

use keystead_core::crypto::{b64_decode, b64_encode, SecretKey};
use keystead_core::format::VaultFile;
use keystead_core::model::{CardData, Folder, ItemType, LoginData, LoginUri, UriMatch, VaultItem};
use keystead_core::{Error, KdfParams, UnlockedVault, VaultStore};
use serde_json::Value;

const PW: &str = "correct horse battery staple";

fn store() -> (tempfile::TempDir, VaultStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = VaultStore::new(dir.path());
    (dir, store)
}

fn create(store: &VaultStore, name: &str) -> UnlockedVault {
    store
        .create_vault_with_params(name, PW, KdfParams::insecure_for_tests())
        .expect("create vault")
}

fn login(name: &str, user: &str, password: &str, uri: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Login, name);
    item.login = Some(LoginData {
        username: user.into(),
        password: password.into(),
        uris: if uri.is_empty() {
            vec![]
        } else {
            vec![LoginUri {
                uri: uri.into(),
                match_type: UriMatch::Domain,
            }]
        },
        ..Default::default()
    });
    item
}

#[test]
fn types_are_thread_safe() {
    // The desktop backend keeps these in shared state across threads.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<UnlockedVault>();
    assert_send_sync::<VaultStore>();
    assert_send_sync::<Error>();
}

#[test]
fn create_list_and_unlock() {
    let (_dir, store) = store();
    assert!(store.list_vaults().unwrap().is_empty());
    let a = create(&store, "  Privat ");
    let b = create(&store, "arbeit");
    assert_eq!(a.name(), "Privat");
    assert!(a.path().exists());
    assert_eq!(a.path(), store.vault_path(a.id()));

    let list = store.list_vaults().unwrap();
    let names: Vec<_> = list.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["arbeit", "Privat"]);
    assert_eq!(list[1].id, a.id());
    assert!(!list[1].has_recovery_key);
    assert_eq!(list[1].path, a.path().display().to_string());

    let v = store.unlock(b.id(), PW).unwrap();
    assert!(v.items().is_empty());
    assert_eq!(v.info(), b.info());
    assert!(matches!(
        store.unlock(a.id(), "wrong"),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(
        store.unlock(a.id(), ""),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(
        store.unlock("00000000-0000-4000-8000-000000000000", PW),
        Err(Error::NotFound(_))
    ));
    for bad in ["../evil", "a/b", "", "x\\y", "a.b"] {
        assert!(
            matches!(store.unlock(bad, PW), Err(Error::NotFound(_))),
            "{bad}"
        );
    }
    assert!(matches!(
        store.create_vault_with_params("  ", PW, KdfParams::insecure_for_tests()),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        store.create_vault_with_params("x", "", KdfParams::insecure_for_tests()),
        Err(Error::InvalidInput(_))
    ));
    let absurd = KdfParams {
        memory_kib: 4 * 1024 * 1024,
        iterations: 3,
        parallelism: 4,
    };
    assert!(matches!(
        store.create_vault_with_params("x", PW, absurd),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn list_skips_foreign_and_broken_files() {
    let (_dir, store) = store();
    let a = create(&store, "Echt");
    let vaults = store.vaults_dir();
    fs::write(vaults.join("junk.keystead"), b"garbage").unwrap();
    fs::write(vaults.join("notes.txt"), b"hello").unwrap();
    // A copy whose file name does not match its id.
    fs::copy(a.path(), vaults.join("copy.keystead")).unwrap();
    fs::create_dir(vaults.join("dir.keystead")).unwrap();
    let list = store.list_vaults().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Echt");
}

#[test]
fn plaintext_never_on_disk() {
    let (_dir, store) = store();
    let mut v = create(&store, "Geheim");
    v.save_item(login(
        "Bank",
        "max",
        "Sup3r-Geheim-Passwort",
        "bank.example",
    ))
    .unwrap();
    let raw = fs::read_to_string(v.path()).unwrap();
    assert!(!raw.contains("Sup3r-Geheim-Passwort"));
    assert!(!raw.contains("Bank"));
    assert!(raw.contains("\"format\": \"keystead\""));
}

#[test]
fn items_crud_and_persistence() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");

    let saved = v
        .save_item(login("GitHub", "octocat", "pw-1", "https://github.com"))
        .unwrap();
    assert_eq!(saved.id.len(), 36);
    assert!(saved.created_at > 0);
    assert_eq!(saved.created_at, saved.updated_at);
    assert!(saved.password_history.is_empty());
    assert_eq!(v.revision(), 2);

    // Persisted.
    let again = store.unlock(v.id(), PW).unwrap();
    assert_eq!(again.item(&saved.id), Some(&saved));

    // Password change → history + passwordRevisedAt; createdAt is kept even
    // if the client sends something else.
    let mut edit = saved.clone();
    edit.created_at = 1;
    if let Some(l) = edit.login.as_mut() {
        l.password = "pw-2".into();
    }
    let updated = v.save_item(edit).unwrap();
    assert_eq!(updated.created_at, saved.created_at);
    assert!(updated.updated_at >= saved.updated_at);
    assert_eq!(updated.password_history.len(), 1);
    assert_eq!(updated.password_history[0].password, "pw-1");
    assert!(updated
        .login
        .as_ref()
        .unwrap()
        .password_revised_at
        .is_some());

    // Unchanged password → no new history entry; passwordRevisedAt is
    // maintained by the core, whatever the client sends.
    let mut unchanged = updated.clone();
    if let Some(l) = unchanged.login.as_mut() {
        l.password_revised_at = None;
    }
    let same = v.save_item(unchanged).unwrap();
    assert_eq!(same.password_history.len(), 1);
    assert_eq!(
        same.login.as_ref().unwrap().password_revised_at,
        updated.login.as_ref().unwrap().password_revised_at
    );
    let mut fresh = login("Fresh", "f", "f", "");
    if let Some(l) = fresh.login.as_mut() {
        l.password_revised_at = Some(5);
    }
    assert_eq!(
        v.save_item(fresh)
            .unwrap()
            .login
            .unwrap()
            .password_revised_at,
        None
    );

    // History is capped at 10, newest first.
    let mut cur = same;
    for n in 3..=15 {
        if let Some(l) = cur.login.as_mut() {
            l.password = format!("pw-{n}");
        }
        cur = v.save_item(cur).unwrap();
    }
    assert_eq!(cur.password_history.len(), 10);
    assert_eq!(cur.password_history[0].password, "pw-14");
    assert_eq!(cur.password_history[9].password, "pw-5");

    // Unknown id → new item; normalisation; unknown folder cleared.
    let mut odd = login("Odd", "", "", "");
    odd.id = "does-not-exist".into();
    odd.card = Some(CardData::default());
    odd.folder_id = Some("nope".into());
    let odd = v.save_item(odd).unwrap();
    assert_ne!(odd.id, "does-not-exist");
    assert!(odd.card.is_none());
    assert!(odd.folder_id.is_none());

    let mut note = VaultItem::new(ItemType::Note, "Notiz");
    note.login = Some(LoginData::default());
    note.notes = "text".into();
    let note = v.save_item(note).unwrap();
    assert!(note.login.is_none());

    assert!(matches!(
        v.save_item(VaultItem::new(ItemType::Login, "   ")),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        v.save_item(VaultItem::new(ItemType::Login, "x".repeat(201))),
        Err(Error::InvalidInput(_))
    ));

    // Trash, restore, delete, empty trash.
    v.trash_item(&odd.id).unwrap();
    let trashed_at = v.item(&odd.id).unwrap().deleted_at;
    assert!(trashed_at.is_some());
    v.trash_item(&odd.id).unwrap();
    assert_eq!(v.item(&odd.id).unwrap().deleted_at, trashed_at);
    assert!(v.summaries().iter().all(|s| s.id != odd.id));
    assert_eq!(v.items().len(), 4);
    v.restore_item(&odd.id).unwrap();
    assert!(v.item(&odd.id).unwrap().deleted_at.is_none());
    // Saving keeps the server-side trash state.
    v.trash_item(&note.id).unwrap();
    let mut n2 = v.item(&note.id).unwrap().clone();
    n2.deleted_at = None;
    n2.notes = "edited".into();
    v.save_item(n2).unwrap();
    assert!(v.item(&note.id).unwrap().deleted_at.is_some());
    assert_eq!(v.empty_trash().unwrap(), 1);
    assert_eq!(v.empty_trash().unwrap(), 0);
    assert!(v.item(&note.id).is_none());
    v.delete_item(&odd.id).unwrap();
    assert!(v.item(&odd.id).is_none());
    for r in [
        v.trash_item("missing"),
        v.restore_item("missing"),
        v.delete_item("missing"),
    ] {
        assert!(matches!(r, Err(Error::NotFound(_))));
    }

    let reopened = store.unlock(v.id(), PW).unwrap();
    assert_eq!(reopened.data(), v.data());
}

#[test]
fn summaries_search_and_url_lookup() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    let mut gh = login("github", "octocat", "x", "https://github.com");
    gh.notes = "Work account".into();
    v.save_item(gh).unwrap();
    v.save_item(login("Amazon", "max@example.com", "x", "amazon.de"))
        .unwrap();
    let mut fav = login("GitHub Gist", "octo", "x", "https://gist.github.com");
    fav.favorite = true;
    v.save_item(fav).unwrap();
    let mut card = VaultItem::new(ItemType::Card, "Kreditkarte");
    card.card = Some(CardData {
        brand: "Visa".into(),
        number: "4111111111111111".into(),
        ..Default::default()
    });
    v.save_item(card).unwrap();
    let mut trashed = login("Old GitHub", "old", "x", "https://github.com");
    trashed = v.save_item(trashed).unwrap();
    v.trash_item(&trashed.id).unwrap();

    let names: Vec<String> = v.summaries().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["Amazon", "github", "GitHub Gist", "Kreditkarte"]);
    let card_summary = v.summaries().into_iter().find(|s| s.name == "Kreditkarte");
    assert_eq!(card_summary.unwrap().subtitle, "•••• 1111");

    let search = |q: &str| -> Vec<String> { v.search(q).into_iter().map(|s| s.name).collect() };
    assert_eq!(search("git"), ["github", "GitHub Gist"]);
    assert_eq!(search("GIT octocat"), ["github"]);
    assert_eq!(search("work"), ["github"]);
    assert_eq!(search("visa"), ["Kreditkarte"]);
    assert_eq!(search("amazon.de"), ["Amazon"]);
    assert_eq!(search("example"), ["Amazon"]);
    assert!(search("nothing-here").is_empty());
    assert!(search("old").is_empty());
    assert_eq!(search("   ").len(), 4);

    let urls: Vec<String> = v
        .logins_for_url("https://github.com/login")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(urls, ["GitHub Gist", "github"]);
    assert!(v.logins_for_url("https://example.org").is_empty());
    assert_eq!(v.logins_for_url("https://www.amazon.de/").len(), 1);
}

#[test]
fn folders() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    let f = v
        .save_folder(Folder {
            id: String::new(),
            name: " Arbeit ".into(),
        })
        .unwrap();
    assert_eq!(f.name, "Arbeit");
    assert!(!f.id.is_empty());
    let renamed = v
        .save_folder(Folder {
            id: f.id.clone(),
            name: "Job".into(),
        })
        .unwrap();
    assert_eq!(renamed.id, f.id);
    assert_eq!(v.folders().len(), 1);
    assert_eq!(v.folders()[0].name, "Job");
    assert!(matches!(
        v.save_folder(Folder {
            id: String::new(),
            name: "".into()
        }),
        Err(Error::InvalidInput(_))
    ));

    let mut item = login("Jira", "me", "pw", "jira.example.com");
    item.folder_id = Some(f.id.clone());
    let item = v.save_item(item).unwrap();
    assert_eq!(item.folder_id.as_deref(), Some(f.id.as_str()));
    v.delete_folder(&f.id).unwrap();
    assert!(v.folders().is_empty());
    assert!(v.item(&item.id).unwrap().folder_id.is_none());
    assert!(matches!(v.delete_folder(&f.id), Err(Error::NotFound(_))));
}

#[test]
fn generator_history_is_capped() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    for i in 0..55 {
        v.add_generated_password(&format!("gen-{i}")).unwrap();
    }
    assert_eq!(v.generator_history().len(), 50);
    assert_eq!(v.generator_history()[0].password, "gen-54");
    assert_eq!(v.generator_history()[49].password, "gen-5");
    assert!(matches!(
        v.add_generated_password(""),
        Err(Error::InvalidInput(_))
    ));
    v.clear_generator_history().unwrap();
    assert!(store
        .unlock(v.id(), PW)
        .unwrap()
        .generator_history()
        .is_empty());
}

#[test]
fn change_master_password() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    v.save_item(login("A", "a", "a", "")).unwrap();
    assert!(v.verify_master_password(PW));
    assert!(!v.verify_master_password("nope"));
    assert!(matches!(
        v.change_master_password("nope", "new"),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(
        v.change_master_password(PW, ""),
        Err(Error::InvalidInput(_))
    ));
    // Without a recovery key there is no new one.
    assert_eq!(
        v.change_master_password(PW, "neues Passwort ✓").unwrap(),
        None
    );
    assert!(!v.has_recovery_key());
    assert!(v.verify_master_password("neues Passwort ✓"));
    assert!(!v.verify_master_password(PW));
    assert!(matches!(
        store.unlock(v.id(), PW),
        Err(Error::WrongPassword)
    ));
    let again = store.unlock(v.id(), "neues Passwort ✓").unwrap();
    assert_eq!(again.items().len(), 1);
}

#[test]
fn recovery_key_flow() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    v.save_item(login("A", "a", "secret", "")).unwrap();
    assert!(!v.has_recovery_key());
    assert!(matches!(
        store.unlock_with_recovery_key(v.id(), "AAAAA-AAAAA-AAAAA-AAAAA-AAAAA", "x"),
        Err(Error::NotFound(_))
    ));

    let code = v.create_recovery_key().unwrap();
    assert_eq!(code.len(), 29);
    assert!(v.has_recovery_key());
    assert!(store.list_vaults().unwrap()[0].has_recovery_key);

    // Wrong code / malformed code / empty new password.
    let rest = &code[1..];
    let wrong = if code.starts_with('0') {
        format!("1{rest}")
    } else {
        format!("0{rest}")
    };
    assert!(matches!(
        store.unlock_with_recovery_key(v.id(), &wrong, "new"),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(
        store.unlock_with_recovery_key(v.id(), "short", "new"),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        store.unlock_with_recovery_key(v.id(), &code, ""),
        Err(Error::InvalidInput(_))
    ));

    // Lowercase, without dashes: accepted; sets the new master password.
    let typed = code.replace('-', "").to_lowercase();
    let r = store
        .unlock_with_recovery_key(v.id(), &typed, "after-recovery")
        .unwrap();
    assert_eq!(r.items().len(), 1);
    assert!(matches!(
        store.unlock(v.id(), PW),
        Err(Error::WrongPassword)
    ));
    let mut u = store.unlock(v.id(), "after-recovery").unwrap();
    assert!(u.has_recovery_key());
    // The recovery key remains valid.
    store
        .unlock_with_recovery_key(v.id(), &code, "third")
        .unwrap();
    // That set another master password: `u` has to unlock again.
    assert!(matches!(u.reload_if_changed(), Err(Error::KeyChanged)));
    let mut u = store.unlock(v.id(), "third").unwrap();

    // Replacing the key invalidates the old one.
    let code2 = u.create_recovery_key().unwrap();
    assert_ne!(code, code2);
    assert!(matches!(
        store.unlock_with_recovery_key(v.id(), &code, "x"),
        Err(Error::WrongPassword)
    ));
    u.remove_recovery_key().unwrap();
    assert!(!u.has_recovery_key());
    assert!(matches!(
        store.unlock_with_recovery_key(v.id(), &code2, "x"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn conflict_detection_and_reload() {
    let (_dir, store) = store();
    let created = create(&store, "Shared");
    let id = created.id().to_owned();
    let mut app = store.unlock(&id, PW).unwrap();
    let mut tui = store.unlock(&id, PW).unwrap();

    let a = app.save_item(login("From app", "a", "1", "")).unwrap();
    let err = tui.save_item(login("From tui", "b", "2", "")).unwrap_err();
    assert!(matches!(err, Error::Conflict));
    assert_eq!(err.code(), "conflict");
    // The failed save left memory untouched.
    assert!(tui.items().is_empty());
    assert!(matches!(tui.rename("x"), Err(Error::Conflict)));
    assert_eq!(tui.name(), "Shared");

    assert!(tui.reload_if_changed().unwrap());
    assert!(tui.item(&a.id).is_some());
    assert!(!tui.reload_if_changed().unwrap());
    tui.save_item(login("From tui", "b", "2", "")).unwrap();

    assert!(app.reload_if_changed().unwrap());
    assert_eq!(app.items().len(), 2);
    assert!(!app.reload_if_changed().unwrap());

    // A master password change elsewhere: the other session has to unlock
    // again (see `other_sessions_follow_rotation_or_must_unlock_again`).
    app.change_master_password(PW, "other").unwrap();
    assert!(matches!(tui.reload_if_changed(), Err(Error::KeyChanged)));
    let mut tui = store.unlock(&id, "other").unwrap();
    tui.save_item(login("After", "c", "3", "")).unwrap();
    assert!(app.reload_if_changed().unwrap());
    assert_eq!(app.items().len(), 3);
    assert_eq!(app.revision(), tui.revision());
}

fn bak_path(v: &UnlockedVault) -> PathBuf {
    let mut p = v.path().as_os_str().to_owned();
    p.push(".bak");
    PathBuf::from(p)
}

/// What an offline attacker who knows an old secret gets from an older
/// copy of the vault file (the `.bak`, a backup, a synced version).
fn key_from_copy(
    copy: &[u8],
    unwrap: impl Fn(&VaultFile) -> keystead_core::Result<SecretKey>,
) -> SecretKey {
    unwrap(&VaultFile::parse(copy).expect("parse copy")).expect("old secret opens the old copy")
}

fn opens_current(v: &UnlockedVault, key: &SecretKey) -> bool {
    VaultFile::read(v.path())
        .unwrap()
        .decrypt_payload(key)
        .is_ok()
}

#[test]
fn replacing_or_removing_the_recovery_key_rotates_the_vault_key() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    v.save_item(login("A", "a", "secret-a", "")).unwrap();
    let code1 = v.create_recovery_key().unwrap();
    v.save_item(login("B", "b", "secret-b", "")).unwrap();
    let bak = bak_path(&v);
    assert!(VaultFile::read(&bak)
        .unwrap()
        .unwrap_key_with_recovery(&code1)
        .is_ok());
    let copy_with_code1 = fs::read(v.path()).unwrap();

    // Replace: the old code opens neither the current file nor the .bak,
    // and the key it yields from an older copy decrypts no later revision.
    let code2 = v.create_recovery_key().unwrap();
    let old_key = key_from_copy(&copy_with_code1, |f| f.unwrap_key_with_recovery(&code1));
    assert!(!opens_current(&v, &old_key));
    let current = VaultFile::read(v.path()).unwrap();
    assert!(matches!(
        current.unwrap_key_with_recovery(&code1),
        Err(Error::WrongPassword)
    ));
    if bak.exists() {
        assert!(VaultFile::read(&bak)
            .unwrap()
            .unwrap_key_with_recovery(&code1)
            .is_err());
    }
    let k2 = current.unwrap_key_with_recovery(&code2).unwrap();
    assert_eq!(current.decrypt_payload(&k2).unwrap().items.len(), 2);
    assert_eq!(store.unlock(v.id(), PW).unwrap().items().len(), 2);
    v.save_item(login("C", "c", "secret-c", "")).unwrap();
    assert!(!opens_current(&v, &old_key));

    // Remove: the same for the removed code.
    let copy_with_code2 = fs::read(v.path()).unwrap();
    v.remove_recovery_key().unwrap();
    assert!(!v.has_recovery_key());
    if bak.exists() {
        assert!(VaultFile::read(&bak)
            .unwrap()
            .unwrap_key_with_recovery(&code2)
            .is_err());
    }
    let key2 = key_from_copy(&copy_with_code2, |f| f.unwrap_key_with_recovery(&code2));
    assert!(!opens_current(&v, &key2));
    for n in 0..3 {
        v.save_item(login(&format!("Later {n}"), "x", "later-secret", ""))
            .unwrap();
    }
    assert!(!opens_current(&v, &key2));
    assert!(!opens_current(&v, &old_key));
    let reopened = store.unlock(v.id(), PW).unwrap();
    assert_eq!(reopened.items().len(), 6);
    assert_eq!(reopened.data(), v.data());
}

#[test]
fn changing_the_master_password_rotates_the_vault_key() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    v.save_item(login("A", "a", "secret-a", "")).unwrap();
    v.save_item(login("B", "b", "secret-b", "")).unwrap();
    let bak = bak_path(&v);
    let old_copy = fs::read(v.path()).unwrap();

    v.change_master_password(PW, "new master").unwrap();
    let old_key = key_from_copy(&old_copy, |f| f.unwrap_key(PW));
    assert!(!opens_current(&v, &old_key));
    // No .bak that still opens with the old password.
    if bak.exists() {
        assert!(matches!(
            VaultFile::read(&bak).unwrap().unwrap_key(PW),
            Err(Error::WrongPassword)
        ));
    }
    v.save_item(login("C", "c", "secret-c", "")).unwrap();
    assert!(!opens_current(&v, &old_key));
    let reopened = store.unlock(v.id(), "new master").unwrap();
    assert_eq!(reopened.data(), v.data());
}

#[test]
fn master_password_change_with_recovery_key_issues_a_new_recovery_key() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    v.save_item(login("A", "a", "secret-a", "")).unwrap();
    let old_code = v.create_recovery_key().unwrap();
    v.save_item(login("B", "b", "secret-b", "")).unwrap();
    let id = v.id().to_owned();
    let bak = bak_path(&v);
    // The .bak holds the previous revision with the old wrappings.
    assert!(VaultFile::read(&bak)
        .unwrap()
        .unwrap_key_with_recovery(&old_code)
        .is_ok());
    let old_copy = fs::read(v.path()).unwrap();

    let new_code = v
        .change_master_password(PW, "new master")
        .unwrap()
        .expect("the recovery key is replaced");
    assert_eq!(new_code.len(), 29);
    assert_ne!(new_code, old_code);
    assert!(v.has_recovery_key());
    assert!(store.list_vaults().unwrap()[0].has_recovery_key);

    // The vault key was rotated: neither old secret opens the current file,
    // and the key they yield from an older copy decrypts no later revision.
    let current = VaultFile::read(v.path()).unwrap();
    assert!(matches!(current.unwrap_key(PW), Err(Error::WrongPassword)));
    assert!(matches!(
        current.unwrap_key_with_recovery(&old_code),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(store.unlock(&id, PW), Err(Error::WrongPassword)));
    assert!(matches!(
        store.unlock_with_recovery_key(&id, &old_code, "x"),
        Err(Error::WrongPassword)
    ));
    let old_key = key_from_copy(&old_copy, |f| f.unwrap_key(PW));
    let old_recovery_key = key_from_copy(&old_copy, |f| f.unwrap_key_with_recovery(&old_code));
    assert_eq!(*old_key, *old_recovery_key);
    assert!(!opens_current(&v, &old_key));

    // The .bak does not keep the old wrappings (replaced by the new
    // revision, or removed).
    if bak.exists() {
        let b = VaultFile::read(&bak).unwrap();
        assert_eq!(b.revision, current.revision);
        assert!(matches!(b.unwrap_key(PW), Err(Error::WrongPassword)));
        assert!(matches!(
            b.unwrap_key_with_recovery(&old_code),
            Err(Error::WrongPassword)
        ));
    }

    // The new recovery key wraps the new vault key.
    let k = current.unwrap_key_with_recovery(&new_code).unwrap();
    assert_eq!(*k, *current.unwrap_key("new master").unwrap());
    assert_eq!(current.decrypt_payload(&k).unwrap().items.len(), 2);

    // Later revisions: still closed to the old key, open with the new secrets.
    v.save_item(login("C", "c", "secret-c", "")).unwrap();
    assert!(!opens_current(&v, &old_key));
    assert_eq!(store.unlock(&id, "new master").unwrap().data(), v.data());
    let r = store
        .unlock_with_recovery_key(&id, &new_code, "after recovery")
        .unwrap();
    assert_eq!(r.data(), v.data());
    // A recovery-key unlock keeps that key valid (it knows the code) ...
    drop(r);
    let mut r = store
        .unlock_with_recovery_key(&id, &new_code, "after recovery 2")
        .unwrap();
    // ... and a later master password change replaces it again.
    let third_code = r
        .change_master_password("after recovery 2", "final")
        .unwrap()
        .expect("the recovery key is replaced");
    assert!(matches!(
        store.unlock_with_recovery_key(&id, &new_code, "x"),
        Err(Error::WrongPassword)
    ));
    assert_eq!(
        store
            .unlock_with_recovery_key(&id, &third_code, "fourth")
            .unwrap()
            .items()
            .len(),
        3
    );
}

#[test]
fn failed_master_password_change_keeps_the_recovery_key() {
    let (_dir, store) = store();
    let id = create(&store, "Shared").id().to_owned();
    let mut app = store.unlock(&id, PW).unwrap();
    let code = app.create_recovery_key().unwrap();
    let mut tui = store.unlock(&id, PW).unwrap();
    // Another process saved in between: nothing is written, the code
    // generated for the failed attempt is never returned.
    tui.save_item(login("From tui", "b", "2", "")).unwrap();
    assert!(matches!(
        app.change_master_password(PW, "new master"),
        Err(Error::Conflict)
    ));
    assert!(app.verify_master_password(PW));
    let current = VaultFile::read(app.path()).unwrap();
    assert!(current.unwrap_key_with_recovery(&code).is_ok());
    assert!(current.unwrap_key(PW).is_ok());
    // After a reload the retry succeeds and replaces the key.
    assert!(app.reload_if_changed().unwrap());
    let new_code = app
        .change_master_password(PW, "new master")
        .unwrap()
        .unwrap();
    assert!(matches!(
        store.unlock_with_recovery_key(&id, &code, "x"),
        Err(Error::WrongPassword)
    ));
    assert_eq!(
        store
            .unlock_with_recovery_key(&id, &new_code, "y")
            .unwrap()
            .items()
            .len(),
        1
    );
}

#[test]
fn recovery_unlock_rotates_the_vault_key() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    v.save_item(login("A", "a", "secret-a", "")).unwrap();
    let code = v.create_recovery_key().unwrap();
    v.save_item(login("B", "b", "secret-b", "")).unwrap();
    let bak = bak_path(&v);
    let old_copy = fs::read(v.path()).unwrap();
    let id = v.id().to_owned();
    drop(v);

    let r = store
        .unlock_with_recovery_key(&id, &code, "after recovery")
        .unwrap();
    let old_key = key_from_copy(&old_copy, |f| f.unwrap_key(PW));
    assert!(!opens_current(&r, &old_key));
    if bak.exists() {
        assert!(matches!(
            VaultFile::read(&bak).unwrap().unwrap_key(PW),
            Err(Error::WrongPassword)
        ));
    }
    // The recovery key stays valid (it wraps the new key).
    let current = VaultFile::read(r.path()).unwrap();
    let k = current.unwrap_key_with_recovery(&code).unwrap();
    assert_eq!(current.decrypt_payload(&k).unwrap().items.len(), 2);
    assert_eq!(
        store.unlock(&id, "after recovery").unwrap().data(),
        r.data()
    );
}

#[test]
fn other_sessions_follow_rotation_or_must_unlock_again() {
    let (_dir, store) = store();
    let id = create(&store, "Shared").id().to_owned();
    let mut app = store.unlock(&id, PW).unwrap();
    let mut tui = store.unlock(&id, PW).unwrap();
    app.save_item(login("From app", "a", "1", "")).unwrap();

    // Recovery-key changes rotate the vault key but keep the master
    // password: other sessions follow transparently and keep saving.
    app.create_recovery_key().unwrap();
    assert!(tui.reload_if_changed().unwrap());
    assert_eq!(tui.items().len(), 1);
    assert!(tui.has_recovery_key());
    tui.save_item(login("From tui", "b", "2", "")).unwrap();
    assert!(app.reload_if_changed().unwrap());
    app.remove_recovery_key().unwrap();
    assert!(tui.reload_if_changed().unwrap());
    tui.save_item(login("From tui 2", "c", "3", "")).unwrap();
    assert!(app.reload_if_changed().unwrap());
    assert_eq!(app.data(), tui.data());
    assert_eq!(store.unlock(&id, PW).unwrap().items().len(), 3);

    // A master password change: a distinct error (not `corrupt`), the
    // in-memory state is kept and saving fails loudly.
    app.change_master_password(PW, "new master").unwrap();
    let (revision, data) = (tui.revision(), tui.data().clone());
    let err = tui.reload_if_changed().unwrap_err();
    assert!(matches!(err, Error::KeyChanged), "{err:?}");
    assert_eq!(err.code(), "locked");
    assert_eq!(tui.revision(), revision);
    assert_eq!(tui.data(), &data);
    assert!(matches!(
        tui.save_item(login("Lost", "d", "4", "")),
        Err(Error::Conflict)
    ));
    assert!(matches!(tui.reload_if_changed(), Err(Error::KeyChanged)));
    let again = store.unlock(&id, "new master").unwrap();
    assert_eq!(again.data(), app.data());

    // The same with a recovery key, which the change replaces.
    let mut tui = again;
    app.create_recovery_key().unwrap();
    assert!(tui.reload_if_changed().unwrap());
    let code = app
        .change_master_password("new master", "newest")
        .unwrap()
        .expect("the recovery key is replaced");
    assert!(matches!(tui.reload_if_changed(), Err(Error::KeyChanged)));
    assert!(matches!(tui.create_recovery_key(), Err(Error::Conflict)));
    let again = store.unlock(&id, "newest").unwrap();
    assert!(again.has_recovery_key());
    assert_eq!(again.data(), app.data());
    assert_eq!(
        store
            .unlock_with_recovery_key(&id, &code, "after recovery")
            .unwrap()
            .data(),
        app.data()
    );
}

#[test]
fn rollback_to_an_older_revision_is_refused() {
    let (_dir, store) = store();
    let mut v = create(&store, "Current");
    v.save_item(login("A", "a", "pw-old", "")).unwrap();
    v.save_item(login("B", "b", "pw-b", "")).unwrap();
    let path = v.path().to_path_buf();
    let (revision, data) = (v.revision(), v.data().clone());
    assert_eq!(revision, 3);

    // Put the valid older revision (.bak) back, with an attacker-chosen
    // name in the unauthenticated header.
    fs::copy(bak_path(&v), &path).unwrap();
    tamper(&path, |j| j["name"] = Value::from("Evil"));
    let err = v.reload_if_changed().unwrap_err();
    assert!(matches!(err, Error::Rollback), "{err:?}");
    assert_eq!(err.code(), "corrupt:rollback");
    assert_eq!(v.revision(), revision);
    assert_eq!(v.data(), &data);
    assert_eq!(v.name(), "Current");
    // Saving does not build on the rolled-back file.
    assert!(matches!(
        v.save_item(login("C", "c", "pw-c", "")),
        Err(Error::Conflict)
    ));
    assert!(matches!(v.rename("x"), Err(Error::Conflict)));
    assert_eq!(VaultFile::read(&path).unwrap().revision, 2);
    assert_eq!(v.data(), &data);
    // A newer revision is still picked up (conflict_detection_and_reload).
    // Unlocking anew opens what is on disk now.
    assert_eq!(store.unlock(v.id(), PW).unwrap().revision(), 2);
}

#[test]
fn backup_is_previous_revision() {
    let (_dir, store) = store();
    let mut v = create(&store, "Privat");
    let bak = {
        let mut p = v.path().as_os_str().to_owned();
        p.push(".bak");
        std::path::PathBuf::from(p)
    };
    assert!(!bak.exists());
    v.save_item(login("A", "a", "a", "")).unwrap();
    assert_eq!(VaultFile::read(&bak).unwrap().revision, 1);
    v.save_item(login("B", "b", "b", "")).unwrap();
    assert_eq!(VaultFile::read(&bak).unwrap().revision, 2);
    assert_eq!(VaultFile::read(v.path()).unwrap().revision, 3);
    let tmp = {
        let mut p = v.path().as_os_str().to_owned();
        p.push(".tmp");
        std::path::PathBuf::from(p)
    };
    assert!(!tmp.exists());
    // No temporary files are left behind.
    let mut names: Vec<String> = fs::read_dir(store.vaults_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let id = v.id();
    assert_eq!(
        names,
        [
            format!("{id}.keystead"),
            format!("{id}.keystead.bak"),
            format!("{id}.keystead.lock"),
        ]
    );
}

#[test]
fn rename_and_delete_vault() {
    let (_dir, store) = store();
    let mut v = create(&store, "Alt");
    v.rename("  Neu ").unwrap();
    assert_eq!(v.info().name, "Neu");
    assert_eq!(store.list_vaults().unwrap()[0].name, "Neu");
    assert!(matches!(v.rename(""), Err(Error::InvalidInput(_))));
    v.save_item(login("A", "a", "a", "")).unwrap(); // creates .bak
                                                    // Temporary files left behind by a crash (current and old naming).
    let vaults = store.vaults_dir();
    fs::write(
        vaults.join(format!(".{}.keystead.0123456789abcdef.tmp", v.id())),
        b"x",
    )
    .unwrap();
    fs::write(
        vaults.join(format!(".{}.keystead.bak.0123456789abcdef.tmp", v.id())),
        b"x",
    )
    .unwrap();
    fs::write(vaults.join(format!("{}.keystead.tmp", v.id())), b"x").unwrap();

    assert!(matches!(
        store.delete_vault(v.id(), "wrong"),
        Err(Error::WrongPassword)
    ));
    assert!(v.path().exists());
    store.delete_vault(v.id(), PW).unwrap();
    assert!(!v.path().exists());
    let leftovers: Vec<_> = fs::read_dir(store.vaults_dir()).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert!(store.list_vaults().unwrap().is_empty());
    assert!(matches!(store.unlock(v.id(), PW), Err(Error::NotFound(_))));
    // The still-open vault cannot silently recreate the file.
    assert!(matches!(
        v.save_item(login("B", "b", "b", "")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        store.delete_vault(v.id(), PW),
        Err(Error::NotFound(_))
    ));
}

/// Rewrites one field of the vault JSON.
fn tamper(path: &Path, f: impl FnOnce(&mut Value)) {
    let mut v: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    f(&mut v);
    fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

fn flip_b64(v: &mut Value, index: usize) {
    let s = v.as_str().unwrap();
    let mut bytes = b64_decode(s).unwrap();
    let i = index.min(bytes.len() - 1);
    bytes[i] ^= 0x01;
    *v = Value::String(b64_encode(&bytes));
}

#[test]
fn tamper_detection() {
    let (_dir, store) = store();
    let mut v = create(&store, "Tamper");
    v.save_item(login("A", "a", "secret", "")).unwrap();
    let id = v.id().to_owned();
    let path = v.path().to_path_buf();
    let original = fs::read(&path).unwrap();

    type Mutation = Box<dyn Fn(&mut Value)>;
    type Expect = fn(&Error) -> bool;
    let cases: Vec<(&str, Mutation, Expect)> = vec![
        (
            "payload ciphertext first byte",
            Box::new(|j| flip_b64(&mut j["payload"]["ciphertext"], 0)),
            |e| matches!(e, Error::Corrupt(_)),
        ),
        (
            "payload ciphertext tag",
            Box::new(|j| flip_b64(&mut j["payload"]["ciphertext"], usize::MAX)),
            |e| matches!(e, Error::Corrupt(_)),
        ),
        (
            "payload nonce",
            Box::new(|j| flip_b64(&mut j["payload"]["nonce"], 3)),
            |e| matches!(e, Error::Corrupt(_)),
        ),
        (
            "revision",
            Box::new(|j| j["revision"] = Value::from(j["revision"].as_u64().unwrap() + 1)),
            |e| matches!(e, Error::Corrupt(_)),
        ),
        (
            "wrapped key ciphertext",
            Box::new(|j| flip_b64(&mut j["wrappedKey"]["ciphertext"], 5)),
            |e| matches!(e, Error::WrongPassword),
        ),
        (
            "wrapped key nonce",
            Box::new(|j| flip_b64(&mut j["wrappedKey"]["nonce"], 0)),
            |e| matches!(e, Error::WrongPassword),
        ),
        (
            "kdf salt",
            Box::new(|j| flip_b64(&mut j["kdf"]["salt"], 2)),
            |e| matches!(e, Error::WrongPassword),
        ),
        (
            "kdf iterations",
            Box::new(|j| j["kdf"]["iterations"] = Value::from(2)),
            |e| matches!(e, Error::WrongPassword),
        ),
        (
            "kdf memory",
            Box::new(|j| j["kdf"]["memoryKib"] = Value::from(128)),
            |e| matches!(e, Error::WrongPassword),
        ),
        (
            "id in header",
            Box::new(|j| j["id"] = Value::from("00000000-0000-4000-8000-000000000001")),
            |e| matches!(e, Error::Corrupt(_)),
        ),
        (
            "absurd kdf memory",
            Box::new(|j| j["kdf"]["memoryKib"] = Value::from(u64::from(u32::MAX))),
            |e| matches!(e, Error::Corrupt(_)),
        ),
        (
            "unknown version",
            Box::new(|j| j["version"] = Value::from(99)),
            |e| matches!(e, Error::Unsupported(_)),
        ),
        (
            "invalid base64",
            Box::new(|j| j["payload"]["ciphertext"] = Value::from("@@@")),
            |e| matches!(e, Error::Corrupt(_)),
        ),
    ];
    for (what, mutate, expected) in cases {
        fs::write(&path, &original).unwrap();
        tamper(&path, mutate);
        let err = store.unlock(&id, PW).unwrap_err();
        assert!(expected(&err), "{what}: got {err:?}");
    }

    // Truncated file.
    fs::write(&path, &original[..original.len() / 2]).unwrap();
    assert!(matches!(store.unlock(&id, PW), Err(Error::Corrupt(_))));

    // Changing the id consistently (file name too) breaks the key wrapping.
    fs::write(&path, &original).unwrap();
    let new_id = "00000000-0000-4000-8000-000000000002";
    tamper(&path, |j| j["id"] = Value::from(new_id));
    fs::rename(&path, store.vault_path(new_id)).unwrap();
    assert!(matches!(
        store.unlock(new_id, PW),
        Err(Error::WrongPassword)
    ));
    fs::rename(store.vault_path(new_id), &path).unwrap();

    // Restored original still works.
    fs::write(&path, &original).unwrap();
    assert_eq!(store.unlock(&id, PW).unwrap().items().len(), 1);
}

#[test]
fn reload_detects_replaced_or_corrupted_file() {
    let (_dir, store) = store();
    let mut v = create(&store, "R");
    let path = v.path().to_path_buf();
    let mut other = create(&store, "Other");
    other.save_item(login("x", "x", "x", "")).unwrap();
    // Replace the file with a different vault (other id).
    fs::copy(other.path(), &path).unwrap();
    assert!(v.reload_if_changed().is_err());
    fs::remove_file(&path).unwrap();
    assert!(matches!(v.reload_if_changed(), Err(Error::NotFound(_))));
}

#[test]
fn debug_output_has_no_secrets() {
    let (_dir, store) = store();
    let mut v = create(&store, "Dbg");
    v.save_item(login("A", "a", "very-secret-pw", "")).unwrap();
    let dbg = format!("{v:?}");
    assert!(!dbg.contains("very-secret-pw"));
    assert!(dbg.contains("UnlockedVault"));
}

/// Real-world parameters (64 MiB, t=3, p=4); ~0.4 s thanks to the
/// opt-level overrides for the crypto crates in the workspace Cargo.toml.
#[test]
fn default_kdf_params_round_trip() {
    let (_dir, store) = store();
    let started = std::time::Instant::now();
    let mut v = store.create_vault("Default", PW).unwrap();
    v.save_item(login("A", "a", "a", "")).unwrap();
    let u = store.unlock(v.id(), PW).unwrap();
    assert_eq!(u.items().len(), 1);
    let file = VaultFile::read(v.path()).unwrap();
    assert_eq!(file.kdf_params(), KdfParams::default());
    assert!(matches!(
        store.unlock(v.id(), "x"),
        Err(Error::WrongPassword)
    ));
    eprintln!("default params: {:?}", started.elapsed());
}
