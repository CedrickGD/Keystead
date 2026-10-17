//! File attachments of items – the commands on top of
//! `keystead_core::attachments` and `UnlockedVault::{add,remove,read}_attachment`
//! (storage, limits and blob format are the core's; see its module docs).
//! The phone (read-only, no file dialogs) has only the image preview,
//! `reveal_attachment`; everything that takes a dialog path or changes
//! the vault is `#[cfg(desktop)]`.
//!
//! * The file bytes never reach the webview, except through
//!   `reveal_attachment`: an in-app image preview the user asked for,
//!   handed over as a data URL and wiped after serialization like a
//!   revealed secret. The page only names paths: `add_attachment` reads the
//!   file the native open dialog chose, `save_attachment` writes to the path
//!   the native save dialog chose. No plaintext temporary files, no "open
//!   with the default app".
//! * Every command takes `pageVaultId` (another open vault is `locked`,
//!   see `Core::mutate_for_page`) and counts as user activity.
//! * `add_attachment` checks the size from the file's metadata before it
//!   reads anything, reads outside the state lock into a wiped buffer and
//!   never more than the limit plus one byte, then lets the core seal and
//!   record the file. `save_attachment` reads the lock epoch together with
//!   the decrypted bytes and writes nothing if the vault was locked or
//!   replaced in between (the pattern of `Core::copy_to_clipboard_since`).
//! * The backend cannot tell a dialog's path from one the page made up, so
//!   it refuses what no dialog produces (relative paths, `.`/`..`
//!   components) and the app's own files on both sides: no source inside
//!   the data directory (`add_attachment`, [`check_source`]), no target
//!   inside the vaults directory or with the vault extension
//!   (`save_attachment`, [`check_target`]).

#[cfg(desktop)]
use std::fs;
#[cfg(desktop)]
use std::io::Read as _;
#[cfg(desktop)]
use std::path::{Component, Path};

#[cfg(desktop)]
use keystead_core::attachments::ATTACHMENT_MAX_BYTES;
use keystead_core::{crypto, Error as CoreError};
#[cfg(desktop)]
use keystead_core::{format, paths};
use zeroize::Zeroizing;

use crate::commands::{run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
#[cfg(desktop)]
use crate::import_flow::remember_export;
#[cfg(desktop)]
use crate::secrets::{self, ItemListEntry};
use crate::state::Core;
use crate::wipe::Wiped;

/// Largest attachment `reveal_attachment` previews (bytes): 8 MiB.
pub const PREVIEW_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The vault file extension a saved attachment must not carry.
#[cfg(desktop)]
const VAULT_EXTENSION: &str = "keystead";

fn not_found(what: &str) -> AppError {
    CoreError::NotFound(what.to_owned()).into()
}

#[cfg(desktop)]
fn io_err(path: &Path, e: std::io::Error) -> AppError {
    AppError::io(format!("{}: {e}", path.display()))
}

/// A path argument of the page: `invalid_input:path_required` if blank.
#[cfg(desktop)]
fn path_arg(path: &str) -> AppResult<&Path> {
    if path.trim().is_empty() {
        return Err(AppError::invalid("path_required"));
    }
    Ok(Path::new(path))
}

/// Reads the file to attach, at most [`ATTACHMENT_MAX_BYTES`]:
/// * missing → `not_found`; a directory or another non-regular file →
///   `io:…`, without opening it;
/// * larger than the limit → `unsupported:attachment_too_large` from the
///   metadata, before reading (also if it grows while being read).
#[cfg(desktop)]
fn read_file(path: &Path) -> AppResult<Zeroizing<Vec<u8>>> {
    let open_err = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::NotFound {
            not_found("file")
        } else {
            io_err(path, e)
        }
    };
    let meta = fs::metadata(path).map_err(open_err)?;
    if !meta.is_file() {
        return Err(io_err(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        ));
    }
    if meta.len() > ATTACHMENT_MAX_BYTES {
        return Err(AppError::unsupported("attachment_too_large"));
    }
    let file = fs::File::open(path).map_err(open_err)?;
    // Room for one byte more than the limit, so that the buffer never
    // reallocates (and leaves copies of the content behind).
    let capacity = usize::try_from(meta.len().saturating_add(1)).unwrap_or(0);
    let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
    file.take(ATTACHMENT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, e))?;
    if bytes.len() as u64 > ATTACHMENT_MAX_BYTES {
        return Err(AppError::unsupported("attachment_too_large"));
    }
    Ok(bytes)
}

/// True if `path` lies below `dir`: textually, or once resolved – the path
/// itself if it exists, otherwise its deepest existing ancestor (the dialog
/// may name a folder that does not exist yet; `write_atomic` creates the
/// rest below that ancestor). `canonicalize` sees through symlinks and, on
/// Windows, case and 8.3 variants of the directory names.
#[cfg(desktop)]
fn is_inside(path: &Path, dir: &Path) -> bool {
    if path.starts_with(dir) {
        return true;
    }
    let Ok(dir) = fs::canonicalize(dir) else {
        return false;
    };
    let mut ancestor = Some(path);
    while let Some(p) = ancestor.filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(real) = fs::canonicalize(p) {
            return real.starts_with(&dir);
        }
        ancestor = p.parent();
    }
    false
}

/// True for a path as the native dialogs produce it: absolute, without `.`
/// or `..` components. A `..` behind a folder that does not exist yet is
/// opaque to `canonicalize`, but `write_atomic`'s `create_dir_all` would
/// make it resolve – into the vaults directory, if the page chose so.
#[cfg(desktop)]
fn is_dialog_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}

/// `invalid_input:attachment_target` for a save path inside the vaults
/// directory (the app's own files, incl. the blob directories), with the
/// vault file extension, or not of the dialog's shape: a saved attachment
/// must never overwrite or pose as a vault file. `vaults_dir` is the open
/// store's; the process's default location is checked as well.
#[cfg(desktop)]
fn check_target(path: &Path, vaults_dir: &Path) -> AppResult<()> {
    let is_vault_name = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(VAULT_EXTENSION));
    if !is_dialog_path(path)
        || is_vault_name
        || is_inside(path, vaults_dir)
        || is_inside(path, &paths::vaults_dir())
    {
        return Err(AppError::invalid("attachment_target"));
    }
    Ok(())
}

/// `invalid_input:attachment_source` for a file to attach inside the data
/// directory (the open store's and the process's default one) or not of
/// the dialog's shape: the vault files, their blobs and `settings.json`
/// are the app's own, and attaching one would let a page copy it out
/// again through `save_attachment` or read it through `reveal_attachment`
/// without the master password `export_data` demands.
#[cfg(desktop)]
fn check_source(path: &Path, data_dir: &Path) -> AppResult<()> {
    if !is_dialog_path(path) || is_inside(path, data_dir) || is_inside(path, &paths::data_dir()) {
        return Err(AppError::invalid("attachment_source"));
    }
    Ok(())
}

/// The image type of `bytes` by their magic bytes (the only formats the
/// in-app preview shows), as a MIME type.
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Logic (unit-tested without Tauri)
// ---------------------------------------------------------------------------

/// `add_attachment`: attaches the file at `path` (its file name becomes the
/// attachment's name) to `item_id` and answers the item's new list entry.
/// Errors: `locked`, `invalid_input:path_required`,
/// `invalid_input:attachment_source` ([`check_source`]), `not_found` (file
/// or item), `unsupported:attachment_too_large`,
/// `invalid_input:item_trashed`, `invalid_input:attachments_too_many`,
/// `invalid_input:attachment_name_required` / `attachment_name_too_long`,
/// `io:…`.
#[cfg(desktop)]
pub(crate) fn add(
    c: &Core,
    page_vault_id: Option<&str>,
    item_id: &str,
    path: &str,
) -> AppResult<ItemListEntry> {
    let path = path_arg(path)?;
    {
        // Fail early (before reading up to 20 MiB) if the page's vault is
        // not open – checked again by `mutate_for_page` under the lock of
        // the change – and refuse the app's own files as a source.
        let st = c.state();
        st.page_vault(page_vault_id)?;
        check_source(path, st.store.root())?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| AppError::invalid("attachment_name_required"))?;
    // Read without the state lock: a slow disk must not stall the app.
    let bytes = read_file(path)?;
    // On `conflict` the core removes its blob again and `mutate_for_page`
    // retries once on the reloaded vault.
    c.mutate_for_page(page_vault_id, |v| {
        v.add_attachment(item_id, &name, &bytes)?;
        secrets::item_entry(v, item_id)
    })
}

/// `remove_attachment`: drops the attachment (entry, then blob) and answers
/// its item's new list entry. `not_found` for an unknown id.
#[cfg(desktop)]
pub(crate) fn remove(
    c: &Core,
    page_vault_id: Option<&str>,
    attachment_id: &str,
) -> AppResult<ItemListEntry> {
    c.mutate_for_page(page_vault_id, |v| {
        let item_id = v
            .attachment_info(attachment_id)
            .map(|info| info.item_id)
            .ok_or_else(|| CoreError::NotFound("attachment".to_owned()))?;
        v.remove_attachment(attachment_id)?;
        secrets::item_entry(v, &item_id)
    })
}

/// `save_attachment`: decrypts the attachment and writes the plaintext to
/// `path` (the native save dialog's choice), atomically. The target is
/// checked first ([`check_target`]), the bytes and the lock epoch are read
/// under the state lock, and nothing is written if a lock or vault switch
/// happened since (`locked`). The written file is remembered for
/// `show_export`. Errors: `locked`, `invalid_input:path_required`,
/// `invalid_input:attachment_target`, `not_found`, `corrupt:…`, `io:…`.
#[cfg(desktop)]
pub(crate) fn save(
    c: &Core,
    page_vault_id: Option<&str>,
    attachment_id: &str,
    path: &str,
) -> AppResult<()> {
    save_with(c, page_vault_id, attachment_id, path, || {})
}

/// [`save`] with a hook between reading the bytes and writing them (unit
/// tests put a lock there).
#[cfg(desktop)]
fn save_with(
    c: &Core,
    page_vault_id: Option<&str>,
    attachment_id: &str,
    path: &str,
    before_write: impl FnOnce(),
) -> AppResult<()> {
    let path = path_arg(path)?;
    let (bytes, epoch) = {
        let st = c.state();
        let vault = st.page_vault(page_vault_id)?;
        check_target(path, &st.store.vaults_dir())?;
        let bytes = vault.read_attachment(attachment_id)?;
        (bytes, c.lock_epoch())
    };
    before_write();
    // Locked or switched since the bytes were read: the user's session
    // ended, no file of it appears afterwards.
    if epoch != c.lock_epoch() {
        return Err(AppError::Locked);
    }
    format::write_atomic(path, &bytes, false)?;
    // "Im Ordner anzeigen" (`show_export`) opens only this file's folder.
    remember_export(path);
    Ok(())
}

/// `reveal_attachment`: the attachment as a `data:image/…;base64,…` URL for
/// the in-app preview – only PNG, JPEG, GIF and WebP (recognised by their
/// magic bytes) of at most [`PREVIEW_MAX_BYTES`]; anything else is
/// `unsupported:attachment_preview` (a too large file without reading it).
/// `not_found` for an unknown id. The caller wraps the result in [`Wiped`].
pub(crate) fn reveal(
    c: &Core,
    page_vault_id: Option<&str>,
    attachment_id: &str,
) -> AppResult<String> {
    let st = c.state();
    let vault = st.page_vault(page_vault_id)?;
    let info = vault
        .attachment_info(attachment_id)
        .ok_or_else(|| not_found("attachment"))?;
    if info.size > PREVIEW_MAX_BYTES {
        return Err(AppError::unsupported("attachment_preview"));
    }
    let bytes = vault.read_attachment(attachment_id)?;
    let mime = image_mime(&bytes).ok_or_else(|| AppError::unsupported("attachment_preview"))?;
    let encoded = Zeroizing::new(crypto::b64_encode(&bytes));
    let prefix = format!("data:{mime};base64,");
    let mut url = String::with_capacity(prefix.len() + encoded.len());
    url.push_str(&prefix);
    url.push_str(&encoded);
    Ok(url)
}

// ---------------------------------------------------------------------------
// Commands (all take `pageVaultId`: another open vault is `locked`)
// ---------------------------------------------------------------------------

/// `add_attachment(itemId, path, pageVaultId)` † → `ItemListEntry`.
#[cfg(desktop)]
#[tauri::command]
pub async fn add_attachment(
    core: Shared<'_>,
    item_id: String,
    path: String,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<ItemListEntry>> {
    run(&core, true, move |c| {
        add(c, page_vault_id.as_deref(), &item_id, &path).map(Wiped)
    })
    .await
}

/// `save_attachment(attachmentId, path, pageVaultId)` → `null`: writes the
/// decrypted file to `path`.
#[cfg(desktop)]
#[tauri::command]
pub async fn save_attachment(
    core: Shared<'_>,
    attachment_id: String,
    path: String,
    page_vault_id: Option<String>,
) -> CmdResult<()> {
    run(&core, true, move |c| {
        save(c, page_vault_id.as_deref(), &attachment_id, &path)
    })
    .await
}

/// `remove_attachment(attachmentId, pageVaultId)` † → `ItemListEntry`.
#[cfg(desktop)]
#[tauri::command]
pub async fn remove_attachment(
    core: Shared<'_>,
    attachment_id: String,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<ItemListEntry>> {
    run(&core, true, move |c| {
        remove(c, page_vault_id.as_deref(), &attachment_id).map(Wiped)
    })
    .await
}

/// `reveal_attachment(attachmentId, pageVaultId)` → `string`: a data URL
/// of an image attachment for the in-app preview.
#[tauri::command]
pub async fn reveal_attachment(
    core: Shared<'_>,
    attachment_id: String,
    page_vault_id: Option<String>,
) -> CmdResult<Wiped<String>> {
    run(&core, true, move |c| {
        reveal(c, page_vault_id.as_deref(), &attachment_id).map(Wiped)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use keystead_core::attachments::ATTACHMENTS_PER_ITEM_MAX;
    use keystead_core::model::{ItemType, VaultItem};
    use keystead_core::settings::Settings;
    use keystead_core::KdfParams;

    use super::*;
    use crate::import_flow::{export_serial, last_export};
    use crate::state::test_support::core_with_open_vault;
    use crate::wipe::Wipe as _;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR not really a picture";
    const PDF: &[u8] = b"%PDF-1.4 Kontoauszug 2026";

    fn vault_id(c: &Core) -> String {
        c.state().vault.as_ref().unwrap().id().to_owned()
    }

    fn note(c: &Core, name: &str) -> String {
        c.mutate(|v| v.save_item(VaultItem::new(ItemType::Note, name)))
            .unwrap()
            .id
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn attachments_dir(c: &Core) -> PathBuf {
        c.read(|v| v.attachments_dir()).unwrap()
    }

    #[test]
    fn add_reads_the_file_and_answers_the_items_entry() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let item = note(&c, "Bank");
        let pdf = write(dir.path(), "Kontoauszug.pdf", PDF);

        let entry = add(&c, page, &item, pdf.to_str().unwrap()).unwrap();
        assert_eq!(entry.id, item);
        assert_eq!(entry.attachments.len(), 1);
        let a = &entry.attachments[0];
        assert_eq!(a.name, "Kontoauszug.pdf");
        assert_eq!(a.size, PDF.len() as u64);
        assert!(a.created_at > 0 && !a.id.is_empty());

        // Stored sealed: the blob is not the file, the vault reads it back.
        let blob = fs::read(attachments_dir(&c).join(format!("{}.bin", a.id))).unwrap();
        assert!(blob.starts_with(b"KSAT1"));
        assert!(!blob.windows(PDF.len()).any(|w| w == PDF));
        let plain = c.read(|v| v.read_attachment(&a.id)).unwrap().unwrap();
        assert_eq!(plain.as_slice(), PDF);
        // The source file is untouched.
        assert_eq!(fs::read(&pdf).unwrap(), PDF);

        // A second one: oldest first, and the list shows both.
        let png = write(dir.path(), "scan.png", PNG);
        let entry = add(&c, page, &item, png.to_str().unwrap()).unwrap();
        let names: Vec<&str> = entry.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Kontoauszug.pdf", "scan.png"]);
        let listed = c.read(secrets::list_entries).unwrap();
        let listed = listed.iter().find(|e| e.id == item).unwrap();
        assert_eq!(listed.attachments, entry.attachments);

        // The answer carries no key and is wiped like a list entry.
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("\"key\""), "{json}");
        let keys: Vec<String> = c
            .read(|v| {
                v.data()
                    .attachments
                    .values()
                    .map(|e| e.key.clone())
                    .collect()
            })
            .unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| !json.contains(k)), "{json}");
        let mut wiped = entry;
        wiped.wipe();
        assert!(wiped.attachments.iter().all(|a| a.name.is_empty()));

        // An empty path, a missing file, a folder.
        assert_eq!(
            add(&c, page, &item, "  ").unwrap_err().code(),
            "invalid_input:path_required"
        );
        let missing = dir.path().join("missing.pdf");
        assert_eq!(
            add(&c, page, &item, missing.to_str().unwrap())
                .unwrap_err()
                .code(),
            "not_found"
        );
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        let err = add(&c, page, &item, folder.to_str().unwrap()).unwrap_err();
        assert!(err.code().starts_with("io:"), "{}", err.code());
        assert_eq!(c.read(|v| v.attachments_of(&item).len()).unwrap(), 2);
    }

    #[test]
    fn limits_are_checked_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let item = note(&c, "Bank");

        // One byte over the limit (sparse: no 20 MiB are written): refused
        // from the metadata, nothing read, no blob directory created.
        let huge = dir.path().join("huge.bin");
        fs::File::create(&huge)
            .unwrap()
            .set_len(ATTACHMENT_MAX_BYTES + 1)
            .unwrap();
        assert_eq!(
            add(&c, page, &item, huge.to_str().unwrap())
                .unwrap_err()
                .code(),
            "unsupported:attachment_too_large"
        );
        assert!(c.read(|v| v.attachments_of(&item).is_empty()).unwrap());
        assert!(!attachments_dir(&c).exists());

        // Exactly the limit is fine.
        let max = dir.path().join("max.bin");
        fs::File::create(&max)
            .unwrap()
            .set_len(ATTACHMENT_MAX_BYTES)
            .unwrap();
        let entry = add(&c, page, &item, max.to_str().unwrap()).unwrap();
        assert_eq!(entry.attachments[0].size, ATTACHMENT_MAX_BYTES);
        let max_id = entry.attachments[0].id.clone();
        c.mutate(|v| v.remove_attachment(&max_id)).unwrap();

        // The core's checks pass through: trashed item, unknown item, too
        // many attachments.
        let small = write(dir.path(), "small.txt", b"x");
        let small = small.to_str().unwrap();
        let trashed = note(&c, "Old");
        c.mutate(|v| v.trash_item(&trashed)).unwrap();
        assert_eq!(
            add(&c, page, &trashed, small).unwrap_err().code(),
            "invalid_input:item_trashed"
        );
        assert_eq!(
            add(&c, page, "no-such-item", small).unwrap_err().code(),
            "not_found"
        );
        for i in 0..ATTACHMENTS_PER_ITEM_MAX {
            c.mutate(|v| v.add_attachment(&item, &format!("f{i}.txt"), b"x"))
                .unwrap();
        }
        assert_eq!(
            add(&c, page, &item, small).unwrap_err().code(),
            "invalid_input:attachments_too_many"
        );
        assert_eq!(
            c.read(|v| v.attachments_of(&item).len()).unwrap(),
            ATTACHMENTS_PER_ITEM_MAX
        );
    }

    #[test]
    fn save_writes_the_plaintext_and_remembers_the_file() {
        // Every successful `save` sets the process-wide "last export".
        let _serial = export_serial();
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let item = note(&c, "Bank");
        let id = c
            .mutate(|v| v.add_attachment(&item, "Kontoauszug.pdf", PDF))
            .unwrap()
            .id;

        // Into a folder that does not exist yet (the dialog may name one).
        let target = dir.path().join("out").join("Kontoauszug.pdf");
        save(&c, page, &id, target.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), PDF);
        assert_eq!(last_export(), Some(target.clone()));
        // No temporary file is left next to it.
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);

        assert_eq!(
            save(&c, page, "no-such-attachment", target.to_str().unwrap())
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert_eq!(
            save(&c, page, &id, "").unwrap_err().code(),
            "invalid_input:path_required"
        );
        // A missing blob is not_found, nothing is written.
        fs::remove_file(attachments_dir(&c).join(format!("{id}.bin"))).unwrap();
        let other = dir.path().join("other.pdf");
        assert_eq!(
            save(&c, page, &id, other.to_str().unwrap())
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert!(!other.exists());
    }

    #[test]
    fn save_refuses_vault_paths() {
        let _serial = export_serial();
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let own = vault_id(&c);
        let page = Some(own.as_str());
        let item = note(&c, "Bank");
        let id = c
            .mutate(|v| v.add_attachment(&item, "a.txt", b"a"))
            .unwrap()
            .id;
        let vaults = c.state().store.vaults_dir();

        for target in [
            vaults.join("a.txt"),
            vaults.join("deeper").join("a.txt"),
            attachments_dir(&c).join("a.txt"),
            dir.path().join("Backup.keystead"),
            dir.path().join("BACKUP.KEYSTEAD"),
            dir.path().join("sub").join("x.keystead"),
        ] {
            assert_eq!(
                save(&c, page, &id, target.to_str().unwrap())
                    .unwrap_err()
                    .code(),
                "invalid_input:attachment_target",
                "{}",
                target.display()
            );
            assert!(!target.exists(), "{}", target.display());
        }
        // The vault's own file is untouched and still opens.
        let vault_path = c.read(|v| v.path().to_path_buf()).unwrap();
        assert!(c.store().unlock(&own, "master").is_ok());
        assert!(vault_path.is_file());

        // What no dialog produces: relative paths, and `..` behind a folder
        // that does not exist yet – opaque to `canonicalize`, but
        // `write_atomic`'s `create_dir_all` would have made it resolve,
        // straight onto the blob.
        let blob = attachments_dir(&c).join(format!("{id}.bin"));
        let sealed = fs::read(&blob).unwrap();
        let sneaky = dir
            .path()
            .join("nonexist")
            .join("..")
            .join("data")
            .join("vaults")
            .join(format!("{own}.keystead.attachments"))
            .join(format!("{id}.bin"));
        for target in [
            sneaky,
            PathBuf::from("relative").join("a.txt"),
            PathBuf::from("a.txt"),
        ] {
            assert_eq!(
                save(&c, page, &id, target.to_str().unwrap())
                    .unwrap_err()
                    .code(),
                "invalid_input:attachment_target",
                "{}",
                target.display()
            );
        }
        assert!(!dir.path().join("nonexist").exists());
        assert_eq!(fs::read(&blob).unwrap(), sealed);
        assert!(!Path::new("relative").exists() && !Path::new("a.txt").exists());

        // Windows: the vaults directory spelled in another case is the same
        // directory – `canonicalize` of the deepest existing folder says so,
        // also below a folder that does not exist yet.
        #[cfg(windows)]
        for spelled in [
            vaults.to_str().unwrap().to_lowercase(),
            vaults.to_str().unwrap().to_uppercase(),
        ] {
            let spelled = PathBuf::from(spelled);
            for target in [spelled.join("a.txt"), spelled.join("new").join("a.txt")] {
                assert_eq!(
                    save(&c, page, &id, target.to_str().unwrap())
                        .unwrap_err()
                        .code(),
                    "invalid_input:attachment_target",
                    "{}",
                    target.display()
                );
                assert!(!target.exists(), "{}", target.display());
            }
        }

        // `.keystead.bak` or a name merely containing the word is fine.
        let ok = dir.path().join("keystead-notes.txt");
        save(&c, page, &id, ok.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(&ok).unwrap(), b"a");
    }

    #[test]
    fn add_refuses_the_apps_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let item = note(&c, "Bank");
        let existing = c
            .mutate(|v| v.add_attachment(&item, "a.txt", b"a"))
            .unwrap()
            .id;
        let data = c.data_dir();
        let vault_file = c.read(|v| v.path().to_path_buf()).unwrap();
        let blob = attachments_dir(&c).join(format!("{existing}.bin"));
        let settings = write(&data, "settings.json", b"{}");
        let stray = write(&data.join("vaults"), "notes.txt", b"n");

        // Anything in the data directory, and a relative path (which would
        // resolve against the process's working directory).
        for source in [
            vault_file,
            blob,
            settings,
            stray,
            PathBuf::from("Cargo.toml"),
        ] {
            assert!(source.exists(), "{}", source.display());
            assert_eq!(
                add(&c, page, &item, source.to_str().unwrap())
                    .unwrap_err()
                    .code(),
                "invalid_input:attachment_source",
                "{}",
                source.display()
            );
        }
        assert_eq!(c.read(|v| v.attachments_of(&item).len()).unwrap(), 1);
        assert_eq!(fs::read_dir(attachments_dir(&c)).unwrap().count(), 1);

        // Next to the data directory is fine.
        let ok = write(dir.path(), "ok.txt", b"ok");
        let entry = add(&c, page, &item, ok.to_str().unwrap()).unwrap();
        assert_eq!(entry.attachments.len(), 2);
    }

    #[test]
    fn save_after_a_lock_or_vault_switch_writes_nothing() {
        let _serial = export_serial();
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let item = note(&c, "Bank");
        let id = c
            .mutate(|v| v.add_attachment(&item, "a.txt", b"a"))
            .unwrap()
            .id;

        // Locked between reading the bytes and writing the file.
        let target = dir.path().join("after-lock.txt");
        let err = save_with(&c, Some(&page), &id, target.to_str().unwrap(), || {
            assert!(c.lock(None));
        })
        .unwrap_err();
        assert_eq!(err.code(), "locked");
        assert!(!target.exists());

        // Another vault replaced the page's one meanwhile.
        c.unlock(&page, "master").unwrap();
        let err = save_with(&c, Some(&page), &id, target.to_str().unwrap(), || {
            let other = c
                .store()
                .create_vault_with_params("Other", "pw", KdfParams::insecure_for_tests())
                .unwrap();
            c.install_vault(other).unwrap();
        })
        .unwrap_err();
        assert_eq!(err.code(), "locked");
        assert!(!target.exists());

        // Nothing in between: written.
        c.unlock(&page, "master").unwrap();
        save_with(&c, Some(&page), &id, target.to_str().unwrap(), || {}).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"a");
    }

    #[test]
    fn remove_drops_the_entry_and_the_blob() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let item = note(&c, "Bank");
        let first = c
            .mutate(|v| v.add_attachment(&item, "first.txt", b"1"))
            .unwrap()
            .id;
        let second = c
            .mutate(|v| v.add_attachment(&item, "second.txt", b"2"))
            .unwrap()
            .id;
        let blob = |id: &str| attachments_dir(&c).join(format!("{id}.bin"));
        assert!(blob(&first).is_file() && blob(&second).is_file());

        let entry = remove(&c, page, &first).unwrap();
        assert_eq!(entry.id, item);
        assert_eq!(entry.attachments.len(), 1);
        assert_eq!(entry.attachments[0].id, second);
        assert!(!blob(&first).exists() && blob(&second).is_file());
        assert_eq!(
            c.read(|v| v.read_attachment(&first))
                .unwrap()
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert_eq!(remove(&c, page, &first).unwrap_err().code(), "not_found");
        assert_eq!(
            remove(&c, page, "no-such-attachment").unwrap_err().code(),
            "not_found"
        );

        // Attachments of trashed items can be removed, too.
        c.mutate(|v| v.trash_item(&item)).unwrap();
        let entry = remove(&c, page, &second).unwrap();
        assert!(entry.deleted_at.is_some());
        assert!(entry.attachments.is_empty());
        assert!(!blob(&second).exists());
    }

    #[test]
    fn reveal_previews_images_only() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let page = vault_id(&c);
        let page = Some(page.as_str());
        let item = note(&c, "Bank");
        let attach = |name: &str, bytes: &[u8]| {
            c.mutate(|v| v.add_attachment(&item, name, bytes))
                .unwrap()
                .id
        };

        let png = attach("scan.png", PNG);
        let url = reveal(&c, page, &png).unwrap();
        let expected = format!("data:image/png;base64,{}", crypto::b64_encode(PNG));
        assert_eq!(url, expected);
        let (_, data) = url.split_once(',').unwrap();
        assert_eq!(crypto::b64_decode(data).unwrap(), PNG);
        // Wiped after serialization like a revealed secret.
        let json = serde_json::to_string(&Wiped(url.clone())).unwrap();
        assert_eq!(json, format!("\"{url}\""));

        let jpeg = attach("photo.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F']);
        assert!(reveal(&c, page, &jpeg)
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        let gif = attach("anim.gif", b"GIF89a\x01\0\x01\0");
        assert!(reveal(&c, page, &gif)
            .unwrap()
            .starts_with("data:image/gif;base64,"));
        let webp = attach("pic.webp", b"RIFF\x24\0\0\0WEBPVP8 ");
        assert!(reveal(&c, page, &webp)
            .unwrap()
            .starts_with("data:image/webp;base64,"));

        // Not an image (whatever the name says), an empty file, too large.
        let pdf = attach("statement.png", PDF);
        assert_eq!(
            reveal(&c, page, &pdf).unwrap_err().code(),
            "unsupported:attachment_preview"
        );
        let empty = attach("empty.png", b"");
        assert_eq!(
            reveal(&c, page, &empty).unwrap_err().code(),
            "unsupported:attachment_preview"
        );
        let mut big = PNG.to_vec();
        big.resize(usize::try_from(PREVIEW_MAX_BYTES).unwrap() + 1, 0);
        let big = attach("big.png", &big);
        assert_eq!(
            reveal(&c, page, &big).unwrap_err().code(),
            "unsupported:attachment_preview"
        );
        assert_eq!(
            reveal(&c, page, "no-such-attachment").unwrap_err().code(),
            "not_found"
        );
        // A tampered blob is reported as corrupt, not previewed.
        let blob = attachments_dir(&c).join(format!("{png}.bin"));
        let mut bytes = fs::read(&blob).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&blob, bytes).unwrap();
        assert!(reveal(&c, page, &png)
            .unwrap_err()
            .code()
            .starts_with("corrupt:"));
    }

    #[test]
    fn image_types_by_magic_bytes() {
        assert_eq!(image_mime(PNG), Some("image/png"));
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF]), Some("image/jpeg"));
        assert_eq!(image_mime(b"GIF87a"), Some("image/gif"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEBP"), Some("image/webp"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEB"), None);
        assert_eq!(image_mime(b"\x89PNG"), None);
        assert_eq!(
            image_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(image_mime(b""), None);
    }

    #[test]
    fn page_commands_for_another_vault_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let c = core_with_open_vault(dir.path(), Settings::default());
        let own = vault_id(&c);
        let item = note(&c, "Bank");
        let id = c
            .mutate(|v| v.add_attachment(&item, "a.txt", b"a"))
            .unwrap()
            .id;
        let source = write(dir.path(), "b.txt", b"b");
        let source = source.to_str().unwrap();
        let target = dir.path().join("saved.txt");
        let target = target.to_str().unwrap();

        for page in [None, Some("other-vault")] {
            assert_eq!(add(&c, page, &item, source).unwrap_err().code(), "locked");
            assert_eq!(save(&c, page, &id, target).unwrap_err().code(), "locked");
            assert_eq!(remove(&c, page, &id).unwrap_err().code(), "locked");
            assert_eq!(reveal(&c, page, &id).unwrap_err().code(), "locked");
        }
        assert!(!Path::new(target).exists());
        assert_eq!(c.read(|v| v.attachments_of(&item).len()).unwrap(), 1);

        // Locked: the same answer, also for the page's own vault id.
        c.lock(None);
        let page = Some(own.as_str());
        assert_eq!(add(&c, page, &item, source).unwrap_err().code(), "locked");
        assert_eq!(save(&c, page, &id, target).unwrap_err().code(), "locked");
        assert_eq!(remove(&c, page, &id).unwrap_err().code(), "locked");
        assert_eq!(reveal(&c, page, &id).unwrap_err().code(), "locked");
        assert!(!Path::new(target).exists());
    }
}
