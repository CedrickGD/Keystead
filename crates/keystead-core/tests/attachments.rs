//! Encrypted file attachments: blobs next to the vault, limits, pruning and
//! sweeping, conflicts, key rotation, exports and the file format version.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keystead_core::attachments::{
    self, ATTACHMENTS_PER_ITEM_MAX, ATTACHMENT_MAX_BYTES, BLOB_OVERHEAD, SWEEP_MIN_AGE,
};
use keystead_core::crypto::SecretKey;
use keystead_core::export::{export_bitwarden_json, export_csv, export_encrypted_with_params};
use keystead_core::format::VaultFile;
use keystead_core::import::{
    import_keystead_export, plan_import, read_import, ConflictMode, ImportFormat,
};
use keystead_core::model::{ItemType, VaultItem};
use keystead_core::{Error, KdfParams, UnlockedVault, VaultStore};

const PW: &str = "pw";
const MARKER: &[u8] = b"PLAINTEXT-MARKER-0123456789";

fn store() -> (tempfile::TempDir, VaultStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = VaultStore::new(dir.path());
    (dir, store)
}

fn create(store: &VaultStore) -> UnlockedVault {
    store
        .create_vault_with_params("Anhänge", PW, KdfParams::insecure_for_tests())
        .expect("create vault")
}

fn note(name: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Note, name);
    item.notes = "text".into();
    item
}

/// File bytes that are easy to spot in a dump.
fn file_bytes(len: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    v.extend_from_slice(MARKER);
    v
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Sorted file names in `dir` (empty if it does not exist).
fn files(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn blob(v: &UnlockedVault, id: &str) -> PathBuf {
    attachments::blob_path(&v.attachments_dir(), id)
}

/// Sets the modification time of `path` back to well before the sweep age.
fn make_old(path: &Path) {
    let f = fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - SWEEP_MIN_AGE - Duration::from_secs(120))
        .unwrap();
}

fn plant_old(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, b"stale").unwrap();
    make_old(&p);
    p
}

fn names_of(v: &UnlockedVault, item_id: &str) -> Vec<String> {
    v.attachments_of(item_id)
        .into_iter()
        .map(|a| a.name)
        .collect()
}

#[test]
fn attach_read_and_remove_round_trip() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Ausweis")).unwrap();
    assert!(v.attachments_of(&item.id).is_empty());
    assert!(!v.attachments_dir().exists());
    let bytes = file_bytes(10_000);

    let revision = v.revision();
    let info = v.add_attachment(&item.id, "  scan.png ", &bytes).unwrap();
    assert_eq!(v.revision(), revision + 1, "one save");
    assert_eq!(info.name, "scan.png");
    assert_eq!(info.item_id, item.id);
    assert_eq!(info.size, bytes.len() as u64);
    assert!(info.created_at > 0);
    assert_eq!(info.id.len(), 36);
    assert_eq!(v.attachments_of(&item.id), vec![info.clone()]);
    assert_eq!(v.attachment_info(&info.id), Some(info.clone()));
    assert_eq!(v.attachment_info("nope"), None);
    assert_eq!(v.read_attachment(&info.id).unwrap().as_slice(), &bytes[..]);

    // The blob: next to the vault, magic + nonce + ciphertext, no plaintext.
    let dir = v.attachments_dir();
    assert_eq!(
        dir,
        store
            .vaults_dir()
            .join(format!("{}.keystead.attachments", v.id()))
    );
    assert_eq!(files(&dir), [format!("{}.bin", info.id)]);
    let sealed = fs::read(blob(&v, &info.id)).unwrap();
    assert!(sealed.starts_with(b"KSAT1"));
    assert_eq!(sealed.len(), bytes.len() + BLOB_OVERHEAD);
    assert!(!contains(&sealed, MARKER));
    assert!(!contains(&sealed, &bytes[..64]));
    // The vault file holds the entry (encrypted) and says so in its version.
    let raw = fs::read(v.path()).unwrap();
    assert!(!contains(&raw, b"scan.png"));
    let file = VaultFile::read(v.path()).unwrap();
    assert_eq!(file.version, 2);
    let key = file.unwrap_key(PW).unwrap();
    let entry = &file.decrypt_payload(&key).unwrap().attachments[&info.id];
    assert_eq!(entry.name, "scan.png");
    assert_eq!(
        keystead_core::crypto::b64_decode(&entry.key).unwrap().len(),
        32
    );
    assert!(!format!("{:?}", v.data()).contains(&entry.key));

    // Oldest first.
    std::thread::sleep(Duration::from_millis(5));
    let second = v.add_attachment(&item.id, "notes.txt", b"hello").unwrap();
    assert_eq!(names_of(&v, &item.id), ["scan.png", "notes.txt"]);
    assert_eq!(v.read_attachment(&second.id).unwrap().as_slice(), b"hello");

    // Survives locking and unlocking.
    let id = v.id().to_owned();
    drop(v);
    let mut v = store.unlock(&id, PW).unwrap();
    assert_eq!(names_of(&v, &item.id), ["scan.png", "notes.txt"]);
    assert_eq!(v.read_attachment(&info.id).unwrap().as_slice(), &bytes[..]);

    // Removing drops the entry and the blob; the last one brings version 1
    // back.
    v.remove_attachment(&info.id).unwrap();
    assert_eq!(v.attachment_info(&info.id), None);
    assert!(matches!(
        v.read_attachment(&info.id),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        v.remove_attachment(&info.id),
        Err(Error::NotFound(_))
    ));
    assert_eq!(files(&dir), [format!("{}.bin", second.id)]);
    assert_eq!(VaultFile::read(v.path()).unwrap().version, 2);
    v.remove_attachment(&second.id).unwrap();
    assert!(files(&dir).is_empty());
    assert_eq!(VaultFile::read(v.path()).unwrap().version, 1);
    assert!(v.data().attachments.is_empty());
    let again = store.unlock(&id, PW).unwrap();
    assert!(again.attachments_of(&item.id).is_empty());
    assert_eq!(again.data(), v.data());
}

#[test]
fn tampered_or_swapped_blobs_are_corrupt() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Docs")).unwrap();
    let bytes_a = file_bytes(500);
    let bytes_b = file_bytes(700);
    let a = v.add_attachment(&item.id, "a.pdf", &bytes_a).unwrap();
    let b = v.add_attachment(&item.id, "b.pdf", &bytes_b).unwrap();
    let (pa, pb) = (blob(&v, &a.id), blob(&v, &b.id));
    let (sealed_a, sealed_b) = (fs::read(&pa).unwrap(), fs::read(&pb).unwrap());
    let corrupt = |v: &UnlockedVault, id: &str| {
        let err = v.read_attachment(id).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{err:?}");
        assert!(err.code().starts_with("corrupt:"));
    };

    // Swapped between two attachments (each has its own key and AAD).
    fs::write(&pa, &sealed_b).unwrap();
    fs::write(&pb, &sealed_a).unwrap();
    corrupt(&v, &a.id);
    corrupt(&v, &b.id);
    fs::write(&pb, &sealed_b).unwrap();
    assert_eq!(v.read_attachment(&b.id).unwrap().as_slice(), &bytes_b[..]);

    // Flipped ciphertext byte, flipped nonce byte, wrong magic, truncated,
    // empty, absurdly large.
    for (what, mutate) in [
        (
            "ciphertext",
            Box::new(|s: &mut Vec<u8>| s[BLOB_OVERHEAD + 3] ^= 1) as Box<dyn Fn(&mut Vec<u8>)>,
        ),
        (
            "tag",
            Box::new(|s: &mut Vec<u8>| {
                let last = s.len() - 1;
                s[last] ^= 1;
            }),
        ),
        ("nonce", Box::new(|s: &mut Vec<u8>| s[7] ^= 1)),
        ("magic", Box::new(|s: &mut Vec<u8>| s[0] = b'X')),
        (
            "truncated",
            Box::new(|s: &mut Vec<u8>| s.truncate(BLOB_OVERHEAD - 1)),
        ),
        ("empty", Box::new(|s: &mut Vec<u8>| s.clear())),
        (
            "huge",
            Box::new(|s: &mut Vec<u8>| {
                s.resize(ATTACHMENT_MAX_BYTES as usize + BLOB_OVERHEAD + 1, 0)
            }),
        ),
    ] {
        let mut tampered = sealed_a.clone();
        mutate(&mut tampered);
        fs::write(&pa, &tampered).unwrap();
        let err = v.read_attachment(&a.id).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{what}: {err:?}");
    }

    // Missing file → not found; restored → readable again.
    fs::remove_file(&pa).unwrap();
    assert!(matches!(v.read_attachment(&a.id), Err(Error::NotFound(_))));
    fs::write(&pa, &sealed_a).unwrap();
    assert_eq!(v.read_attachment(&a.id).unwrap().as_slice(), &bytes_a[..]);
}

#[test]
fn limits_are_checked_before_anything_is_written() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Big")).unwrap();
    let dir = v.attachments_dir();
    let revision = v.revision();

    let huge = vec![7u8; ATTACHMENT_MAX_BYTES as usize + 1];
    let err = v.add_attachment(&item.id, "huge.bin", &huge).unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    assert_eq!(err.code(), "unsupported:attachment_too_large");
    assert!(!dir.exists(), "nothing was written");
    assert_eq!(v.revision(), revision);
    drop(huge);
    // Exactly the limit is fine.
    let max = vec![9u8; ATTACHMENT_MAX_BYTES as usize];
    let big = v.add_attachment(&item.id, "max.bin", &max).unwrap();
    assert_eq!(big.size, ATTACHMENT_MAX_BYTES);
    assert_eq!(v.read_attachment(&big.id).unwrap().len(), max.len());
    drop(max);
    v.remove_attachment(&big.id).unwrap();
    assert!(files(&dir).is_empty());

    // Names.
    let revision = v.revision();
    for bad in ["", "   ", "\t\n"] {
        let err = v.add_attachment(&item.id, bad, b"x").unwrap_err();
        assert_eq!(err.code(), "invalid_input:attachment_name_required");
    }
    let err = v
        .add_attachment(&item.id, &"x".repeat(201), b"x")
        .unwrap_err();
    assert_eq!(err.code(), "invalid_input:attachment_name_too_long");
    assert!(files(&dir).is_empty());
    assert_eq!(v.revision(), revision);
    let long = "ä".repeat(200);
    let ok = v
        .add_attachment(&item.id, &format!("  {long} "), b"x")
        .unwrap();
    assert_eq!(ok.name, long);

    // At most 10 per item; another item has its own ten.
    for n in 2..=ATTACHMENTS_PER_ITEM_MAX {
        v.add_attachment(&item.id, &format!("f{n}.txt"), b"x")
            .unwrap();
    }
    assert_eq!(v.attachments_of(&item.id).len(), ATTACHMENTS_PER_ITEM_MAX);
    let revision = v.revision();
    let err = v.add_attachment(&item.id, "f11.txt", b"x").unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
    assert_eq!(err.code(), "invalid_input:attachments_too_many");
    assert_eq!(files(&dir).len(), ATTACHMENTS_PER_ITEM_MAX);
    assert_eq!(v.revision(), revision);
    let other = v.save_item(note("Other")).unwrap();
    v.add_attachment(&other.id, "o.txt", b"x").unwrap();
    assert_eq!(files(&dir).len(), ATTACHMENTS_PER_ITEM_MAX + 1);
}

#[test]
fn unknown_and_trashed_items_and_pruning() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let dir = v.attachments_dir();
    let err = v.add_attachment("missing", "x.txt", b"x").unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert!(!dir.exists());

    // The trash keeps its attachments but takes no new ones.
    let item = v.save_item(note("Trash me")).unwrap();
    let a = v.add_attachment(&item.id, "a.txt", b"aaa").unwrap();
    v.trash_item(&item.id).unwrap();
    assert_eq!(names_of(&v, &item.id), ["a.txt"]);
    assert_eq!(v.read_attachment(&a.id).unwrap().as_slice(), b"aaa");
    let err = v.add_attachment(&item.id, "b.txt", b"bbb").unwrap_err();
    assert_eq!(err.code(), "invalid_input:item_trashed");
    assert_eq!(files(&dir).len(), 1);
    v.restore_item(&item.id).unwrap();
    let b = v.add_attachment(&item.id, "b.txt", b"bbb").unwrap();
    assert_eq!(names_of(&v, &item.id), ["a.txt", "b.txt"]);
    assert_eq!(VaultFile::read(v.path()).unwrap().version, 2);

    // Deleting the item prunes its entries; the blobs go with them.
    v.delete_item(&item.id).unwrap();
    assert!(v.data().attachments.is_empty());
    assert!(v.attachment_info(&a.id).is_none() && v.attachment_info(&b.id).is_none());
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
    assert_eq!(VaultFile::read(v.path()).unwrap().version, 1);

    // Emptying the trash too; another item's attachment stays.
    let gone = v.save_item(note("Gone")).unwrap();
    let kept = v.save_item(note("Kept")).unwrap();
    let c = v.add_attachment(&gone.id, "c.txt", b"ccc").unwrap();
    let k = v.add_attachment(&kept.id, "k.txt", b"kkk").unwrap();
    v.trash_item(&gone.id).unwrap();
    assert_eq!(v.empty_trash().unwrap(), 1);
    assert!(v.attachment_info(&c.id).is_none());
    assert_eq!(files(&dir), [format!("{}.bin", k.id)]);
    assert_eq!(v.read_attachment(&k.id).unwrap().as_slice(), b"kkk");
    assert_eq!(store.unlock(v.id(), PW).unwrap().data(), v.data());
}

#[test]
fn sweep_removes_old_unreferenced_files_only() {
    let (_dir, store) = store();
    let mut v = create(&store);
    assert_eq!(v.sweep_attachments(), 0, "no directory yet");
    let item = v.save_item(note("Sweep")).unwrap();
    let a = v.add_attachment(&item.id, "a.txt", b"aaa").unwrap();
    let dir = v.attachments_dir();
    // A referenced blob is kept however old it is.
    make_old(&blob(&v, &a.id));
    let orphan = plant_old(&dir, "0f0f0f0f-0000-4000-8000-000000000001.bin");
    let stale_tmp = plant_old(
        &dir,
        ".0f0f0f0f-0000-4000-8000-000000000002.bin.0123456789abcdef.tmp",
    );
    let fresh = dir.join("0f0f0f0f-0000-4000-8000-000000000003.bin");
    fs::write(&fresh, b"being written by the TUI").unwrap();

    assert_eq!(v.sweep_attachments(), 2);
    assert!(!orphan.exists() && !stale_tmp.exists());
    assert!(fresh.exists(), "younger than an hour: spared");
    assert_eq!(v.read_attachment(&a.id).unwrap().as_slice(), b"aaa");
    assert_eq!(v.sweep_attachments(), 0);
    assert_eq!(attachments::sweep(&dir, v.data()), 0);

    // Unlocking sweeps as well.
    let orphan = plant_old(&dir, "0f0f0f0f-0000-4000-8000-000000000004.bin");
    let id = v.id().to_owned();
    drop(v);
    let v = store.unlock(&id, PW).unwrap();
    assert!(!orphan.exists());
    assert!(fresh.exists());
    assert_eq!(v.read_attachment(&a.id).unwrap().as_slice(), b"aaa");
    fs::remove_file(&fresh).unwrap();
}

#[test]
fn an_add_after_a_conflict_can_be_retried() {
    let (_dir, store) = store();
    let mut app = create(&store);
    let id = app.id().to_owned();
    let item = app.save_item(note("Shared")).unwrap();
    let mut tui = store.unlock(&id, PW).unwrap();
    tui.save_item(note("From tui")).unwrap();

    let bytes = file_bytes(300);
    let err = app
        .add_attachment(&item.id, "scan.png", &bytes)
        .unwrap_err();
    assert!(matches!(err, Error::Conflict), "{err:?}");
    assert!(
        app.data().attachments.is_empty(),
        "unchanged after the conflict"
    );
    assert!(app.attachments_of(&item.id).is_empty());
    assert!(
        files(&app.attachments_dir()).is_empty(),
        "no blob left behind"
    );

    assert!(app.reload_if_changed().unwrap());
    let info = app.add_attachment(&item.id, "scan.png", &bytes).unwrap();
    assert_eq!(files(&app.attachments_dir()), [format!("{}.bin", info.id)]);
    assert!(app.items().iter().any(|i| i.name == "From tui"));
    // The other session sees it after its reload.
    assert!(matches!(
        tui.remove_attachment(&info.id),
        Err(Error::NotFound(_))
    ));
    assert!(tui.reload_if_changed().unwrap());
    assert_eq!(
        tui.read_attachment(&info.id).unwrap().as_slice(),
        &bytes[..]
    );
    tui.remove_attachment(&info.id).unwrap();
    assert!(files(&app.attachments_dir()).is_empty());
    assert!(app.reload_if_changed().unwrap());
    assert!(app.attachments_of(&item.id).is_empty());
}

/// What an offline attacker who knows an old secret gets from an older
/// copy of the vault file.
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
fn rekeying_the_vault_keeps_attachments_readable_without_rewriting_blobs() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Rotate")).unwrap();
    let bytes = file_bytes(2_000);
    let a = v.add_attachment(&item.id, "a.pdf", &bytes).unwrap();
    let sealed = fs::read(blob(&v, &a.id)).unwrap();
    let old_copy = fs::read(v.path()).unwrap();
    let id = v.id().to_owned();

    v.change_master_password(PW, "new master").unwrap();
    assert_eq!(v.read_attachment(&a.id).unwrap().as_slice(), &bytes[..]);
    assert_eq!(fs::read(blob(&v, &a.id)).unwrap(), sealed, "blob untouched");
    let old_key = key_from_copy(&old_copy, |f| f.unwrap_key(PW));
    assert!(!opens_current(&v, &old_key));
    assert!(matches!(store.unlock(&id, PW), Err(Error::WrongPassword)));

    let code = v.create_recovery_key().unwrap();
    assert_eq!(v.read_attachment(&a.id).unwrap().as_slice(), &bytes[..]);
    assert_eq!(fs::read(blob(&v, &a.id)).unwrap(), sealed);
    assert!(!opens_current(&v, &old_key));
    let b = v.add_attachment(&item.id, "b.pdf", b"after").unwrap();

    let reopened = store.unlock(&id, "new master").unwrap();
    assert_eq!(
        reopened.read_attachment(&a.id).unwrap().as_slice(),
        &bytes[..]
    );
    assert_eq!(
        reopened.read_attachment(&b.id).unwrap().as_slice(),
        b"after"
    );
    drop(reopened);
    let recovered = store.unlock_with_recovery_key(&id, &code, "third").unwrap();
    assert_eq!(
        recovered.read_attachment(&a.id).unwrap().as_slice(),
        &bytes[..]
    );
    assert_eq!(fs::read(blob(&recovered, &a.id)).unwrap(), sealed);
    assert_eq!(VaultFile::read(recovered.path()).unwrap().version, 2);
}

#[test]
fn delete_vault_removes_the_attachments_directory() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Delete")).unwrap();
    v.add_attachment(&item.id, "a.txt", b"aaa").unwrap();
    let dir = v.attachments_dir();
    assert!(dir.is_dir());
    assert!(matches!(
        store.delete_vault(v.id(), "wrong"),
        Err(Error::WrongPassword)
    ));
    assert!(dir.is_dir());
    store.delete_vault(v.id(), PW).unwrap();
    assert!(!dir.exists());
    assert!(!v.path().exists());
    assert!(files(&store.vaults_dir()).is_empty());
    // A vault without attachments has no directory to remove.
    let plain = create(&store);
    store.delete_vault(plain.id(), PW).unwrap();
    assert!(files(&store.vaults_dir()).is_empty());
}

/// Keeps a blob undeletable for the guard's lifetime: on Windows a handle
/// without delete sharing (what a scanner holds), elsewhere a read-only
/// directory.
#[cfg(windows)]
struct Resist {
    _file: fs::File,
}

#[cfg(windows)]
fn resist(_dir: &Path, blob: &Path) -> Resist {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    let _file = fs::File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(blob)
        .unwrap();
    Resist { _file }
}

#[cfg(unix)]
struct Resist {
    dir: PathBuf,
}

#[cfg(unix)]
fn resist(dir: &Path, _blob: &Path) -> Resist {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    Resist {
        dir: dir.to_path_buf(),
    }
}

#[cfg(unix)]
impl Drop for Resist {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o755));
    }
}

/// False if [`resist`] cannot bind this process (root on Unix).
#[cfg(unix)]
fn resist_binds(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(dir).unwrap().uid() != 0
}

#[cfg(windows)]
fn resist_binds(_dir: &Path) -> bool {
    true
}

#[test]
fn delete_vault_succeeds_when_the_directory_resists() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Delete")).unwrap();
    let a = v.add_attachment(&item.id, "a.txt", b"aaa").unwrap();
    let dir = v.attachments_dir();
    let blob = blob(&v, &a.id);

    // Removing the vault file is the deletion; a directory that cannot be
    // emptied right now (a scanner holding a fresh blob) is left behind
    // after a short retry instead of failing the call – the keys are gone,
    // and the caller must be able to close the vault it deleted.
    let held = resist(&dir, &blob);
    store.delete_vault(v.id(), PW).unwrap();
    assert!(!v.path().exists());
    assert!(store.list_vaults().unwrap().is_empty());
    assert!(matches!(store.unlock(v.id(), PW), Err(Error::NotFound(_))));
    assert!(matches!(
        store.delete_vault(v.id(), PW),
        Err(Error::NotFound(_))
    ));
    let leftovers = files(&store.vaults_dir());
    if resist_binds(&dir) {
        assert_eq!(leftovers, [format!("{}.keystead.attachments", v.id())]);
        assert!(blob.is_file(), "the held blob stays behind");
    } else {
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
    drop(held);
    // Deletable by hand once released.
    let _ = fs::remove_dir_all(&dir);
    assert!(!dir.exists());
}

#[test]
fn exports_carry_nothing_and_imports_bring_none() {
    let (dir, store) = store();
    let mut v = create(&store);
    let item = v.save_item(note("Ausweis")).unwrap();
    v.add_attachment(&item.id, "scan-of-passport.png", &file_bytes(100))
        .unwrap();
    let key_b64 = v.data().attachments.values().next().unwrap().key.clone();

    let csv = export_csv(v.data());
    assert!(!csv.contains("scan-of-passport") && !csv.contains(&key_b64));
    let json = export_bitwarden_json(v.data());
    assert!(!json.contains("scan-of-passport") && !json.contains("attachments"));
    assert!(!json.contains(&key_b64));

    let path = dir.path().join("export.keystead");
    export_encrypted_with_params(v.data(), &path, "export", KdfParams::insecure_for_tests())
        .unwrap();
    let file = VaultFile::read(&path).unwrap();
    assert_eq!(
        file.version, 1,
        "an export never needs the attachment reader"
    );
    let key = file.unwrap_key("export").unwrap();
    let exported = file.decrypt_payload(&key).unwrap();
    assert_eq!(exported.items.len(), 1);
    assert!(exported.attachments.is_empty());
    let (items, _) = import_keystead_export(&path, "export").unwrap();
    assert_eq!(items.len(), 1);

    let mut other = create(&store);
    let parsed = read_import(&path, ImportFormat::Keystead, Some("export")).unwrap();
    let plan = plan_import(other.data(), parsed);
    other.commit_import(plan, ConflictMode::Skip).unwrap();
    assert_eq!(other.items().len(), 1);
    assert!(other.data().attachments.is_empty());
    assert!(other.attachments_of(&other.items()[0].id).is_empty());
    assert!(!other.attachments_dir().exists());
    assert_eq!(VaultFile::read(other.path()).unwrap().version, 1);
}

#[test]
fn file_version_follows_the_attachments() {
    let (_dir, store) = store();
    let mut v = create(&store);
    let id = v.id().to_owned();
    let version = || VaultFile::read(&store.vault_path(&id)).unwrap().version;
    assert_eq!(version(), 1);
    let item = v.save_item(note("V")).unwrap();
    assert_eq!(version(), 1);
    let a = v.add_attachment(&item.id, "a.txt", b"a").unwrap();
    assert_eq!(version(), 2);
    // Unrelated saves keep it.
    v.save_item(note("Other")).unwrap();
    v.rename("Renamed").unwrap();
    assert_eq!(version(), 2);
    assert_eq!(
        store
            .unlock(&id, PW)
            .unwrap()
            .attachments_of(&item.id)
            .len(),
        1
    );
    assert_eq!(store.list_vaults().unwrap()[0].name, "Renamed");
    v.remove_attachment(&a.id).unwrap();
    assert_eq!(version(), 1);
    assert!(store.unlock(&id, PW).unwrap().data().attachments.is_empty());
}
