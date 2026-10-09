# Keystead – Architecture & Contracts

Keystead (*Key* + *Homestead* – your keys stay at home) is a local-only
password manager in the spirit of Bitwarden:
a desktop app (Tauri 2: Rust backend + React UI), a terminal UI, and a
Chromium browser extension that talks to the desktop app via Native
Messaging. **Nothing ever leaves the machine** – there is no server and no
cloud sync.

This document is the binding contract between the components. If you change
an interface here, change every side.

```
┌────────────────────┐  native messaging    ┌────────────────────┐  local socket   ┌────────────────────────────┐
│ Chrome/Edge/Brave  │ ─── stdin/stdout ──▶ │ Keystead.exe       │ ── named pipe ─▶│ Keystead.exe (desktop app, │
│ extension (MV3)    │ ◀── length-prefixed ─│ (native host mode) │ ◀─ len-prefix ─ │ tray, holds unlocked       │
└────────────────────┘     JSON             └────────────────────┘    JSON         │ vault in memory)           │
                                                                                   └─────────────┬──────────────┘
                                                                                                 │ keystead-core
                                               ┌────────────────────────┐                        ▼
                                               │ keystead-cli / --cli   │── keystead-core ──▶ encrypted *.keystead files
                                               │ (ratatui TUI)          │
                                               └────────────────────────┘
```

## Repository layout

| Path | What |
|------|------|
| `Cargo.toml` | Cargo workspace |
| `crates/keystead-core` | Crypto, vault file format, storage, model, generator, TOTP, import/export, URL matching, health report. No UI, no IPC. |
| `crates/keystead-bridge` | Browser bridge: protocol types, framing, local socket server (used by the app) & client, native-messaging host runner, host-manifest registration for Chrome/Edge/Brave/Chromium. |
| `crates/keystead-tui` | Terminal UI (ratatui + crossterm). Library `keystead_tui::run()` + binary `keystead-cli`. |
| `apps/desktop` | Tauri 2 app. `src/` = React + TypeScript + Vite frontend, `src-tauri/` = Rust backend (binary name `Keystead`). |
| `extension/chrome` | Manifest V3 extension, plain JavaScript (no build step, load unpacked). |
| `assets/` | Brand assets: `keystead.svg` (app icon), `keystead-glyph.svg` (single-colour shield, `currentColor`), `keystead-1024.png` (source for `npx tauri icon` and the extension icons), `Keystead.ico`. |
| `legacy/` | The old PowerShell VaultX 1.x, kept for reference. |
| `docs/` | This file + user docs. |

## Executable modes (`apps/desktop/src-tauri/src/main.rs`)

One portable `Keystead.exe` (Windows GUI subsystem in release) does everything:

1. **Native host mode** – if any CLI argument starts with `chrome-extension://`
   (Chrome passes the caller origin as first argument; on Windows it may also
   pass `--parent-window=N`), run `keystead_bridge::host::run()` and exit.
   Never initialise Tauri in this mode, never print anything to stdout except
   protocol frames.
2. **Terminal mode** – `--cli` (or `cli` as first arg): on Windows call
   `FreeConsole()` + `AllocConsole()` so the TUI gets its own new console
   window, then `keystead_tui::run()`, exit. Additionally the console-subsystem
   binary `keystead-cli(.exe)` from `crates/keystead-tui` runs the same TUI inside
   an existing terminal.
3. **GUI mode** – everything else. `--background` starts hidden in the tray
   (used when the native host has to launch the app); such an instance exits
   right away if its bridge server is not running (browser integration off,
   or the endpoint is served elsewhere), so it never lingers invisibly. Uses
   `tauri-plugin-single-instance`: a second GUI launch focuses the first
   window (and a second `--background` launch does nothing).

### Terminal UI & command line (`keystead-tui`)
`pub fn run() -> i32` / `pub fn run_with_args(Vec<String>) -> i32` (`args[0]`
= program; a leading `--cli`/`cli` is ignored). The caller exits with the
returned code: `0` success, `1` error, `2` invalid usage.
* No subcommand → interactive full-screen UI (needs a terminal on stdout,
  else exit 1): vault picker (> 1 vault; preselects `lastVaultId`, which an
  unlock updates), master-password prompt, item list + details; copy
  (`u`/`p`/`t`, secrets via `clipboard::copy_secret` with
  `clipboardClearSeconds`, pending secret cleared on lock/quit), `o` open
  http(s) website, `n`/`e` edit, `d` trash, `g` generator (copies are added
  to the vault's generator history), auto-lock after `autoLockMinutes`, file
  re-checked every 5 s (`reload_if_changed`), `Conflict` → reload + retry once;
  `KeyChanged` (master password changed elsewhere) locks the TUI, `Rollback`
  is shown as an error (the newer in-memory state stays open).
  `--vault NAME|ID` preselects a vault. Creating a vault is possible when none
  exists (`n` in the picker).
* Subcommands (all accept `--vault NAME|ID`; default: the only vault, else
  `lastVaultId`): `vaults`, `list`, `get <id|name|search terms>
  [--field password|username|totp|notes|uri] [--copy]`, `generate [--length N |
  --passphrase] [--words N] [--no-symbols] [--copy]`. `--copy` keeps the
  process alive until the clipboard is cleared (any key clears at once).
* Master password: prompted on the TTY (`rpassword`), or – insecure, for
  scripts – `$KEYSTEAD_MASTER_PASSWORD`.
* Language from `Settings.language` (German default, English for `en`).

## Data locations (`keystead_core::paths`)

* `data_dir()`:
  1. `$KEYSTEAD_DATA_DIR` if set.
  2. **Portable mode**: a folder named `Keystead-Data` next to the running
     executable, if it exists (the user creates it, or Settings → "Portabler
     Modus" creates it and moves the vaults).
  3. Otherwise the OS local data dir: Windows `%LOCALAPPDATA%\Keystead`,
     Linux `~/.local/share/keystead`, macOS `~/Library/Application Support/Keystead`.
* Vault files: `<data_dir>/vaults/<vault-id>.keystead` (+ `<vault-id>.keystead.bak`
  = previous good version, written before each save – except when the master
  password or recovery key changes: then the `.bak` is replaced by the new
  revision (or removed), so no local copy still opens with the old secret;
  `<vault-id>.keystead.lock` = empty inter-process lock file held only while a
  save runs).
* Settings: `<data_dir>/settings.json` (non-secret, see `Settings` below).
* Paired browser clients: `<data_dir>/bridge-clients.json` (stores only
  SHA-256 hashes of client tokens).
* Legacy VaultX 1.x vaults: `%LOCALAPPDATA%\VaultX\accounts.json` +
  `vault_*.json` (Linux/macOS: none – only manual file import).
  `$KEYSTEAD_LEGACY_DIR` overrides this directory on every OS (tests).

## Vault file format (version 1) – `keystead_core::format`

UTF-8 JSON, written atomically: write a new temporary file
`.<file name>.<16 random hex>.tmp` in the same dir (created exclusively –
`O_CREAT|O_EXCL`/`CREATE_NEW`, never an existing file or symlink; Unix mode
0600) → fsync → rename over the target. The `.bak` is replaced the same way
(never written through an existing file). On error only the temporary file
this write created is removed. All `format::write_atomic` users (vault
files, settings, plain-text exports) share this.

```json
{
  "format": "keystead",
  "version": 1,
  "id": "uuid-v4",
  "name": "Privat",
  "createdAt": 1700000000000,
  "updatedAt": 1700000000000,
  "revision": 12,
  "kdf": { "alg": "argon2id", "memoryKib": 65536, "iterations": 3, "parallelism": 4, "salt": "b64" },
  "wrappedKey": { "nonce": "b64-24B", "ciphertext": "b64" },
  "recovery": null | { "salt": "b64", "nonce": "b64", "ciphertext": "b64" },
  "payload": { "nonce": "b64-24B", "ciphertext": "b64" }
}
```

* A random 32-byte **vault key** encrypts the payload (`VaultData` as JSON)
  with **XChaCha20-Poly1305**. AAD for the payload = UTF-8 bytes of
  `"keystead:v1:payload:" + id + ":" + revision`.
* The master password → Argon2id (params above, salt 16 B) → 32-byte KEK →
  wraps the vault key (XChaCha20-Poly1305, AAD `"keystead:v1:key:" + id`).
* **Key rotation**: replacing a secret replaces the vault key where
  possible, so the old secret plus an older copy of the file (`.bak`, a
  backup, a synced version) yields only the old key, which decrypts no
  later revision:
  * `create_recovery_key` (also "replace") and `remove_recovery_key`: fresh
    vault key, re-wrapped with the unchanged master password (the unlocked
    vault keeps the master KEK in memory for this – no password prompt).
  * `unlock_with_recovery_key`: fresh vault key, wrapped with the new master
    password and again with the (still valid) recovery code.
  * `change_master_password`: fresh vault key if the vault has **no**
    recovery key. With a recovery key only the vault key is re-wrapped (the
    recovery code is unknown to the app, a new key would make it useless):
    older copies still open with the old password until the recovery key is
    replaced, which rotates the key.
  * Copies made before a rotation still open with the old secret – but only
    show the data they contained back then.
* Optional **recovery key**: random 25 chars of Crockford base32 shown as
  `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX` (125 bit). KEK_r = Argon2id(recovery code
  normalised to uppercase without dashes, own salt, same params) wraps the
  vault key (AAD `"keystead:v1:recovery:" + id`).
* `revision` increments on every save; saving checks the on-disk revision
  equals the loaded one, otherwise `Error::Conflict` (another process – TUI or
  app – changed it). Callers reload and retry.
* Only id + revision are authenticated (payload AAD); name, timestamps, kdf
  and the recovery box are not. `reload_if_changed` therefore refuses an
  on-disk revision lower than the one in memory (`Error::Rollback`, code
  `corrupt:rollback`): a restored `.bak`/backup or a rolled-back file is not
  adopted while a vault is open, and saving keeps failing with `Conflict`
  instead of silently building on it. Locking and unlocking again opens the
  file as it is. (A rollback while no session is open is not detected.)
* Base64 = standard alphabet with padding. Secrets in memory use
  `zeroize`/`secrecy` where practical.
* Argon2id default params: m = 64 MiB, t = 3, p = 4. Tests may use a cheaper
  `KdfParams::insecure_for_tests()`.

## Core API – `keystead-core` (Rust)

```rust
pub mod error;    // pub enum Error { Io, Json, WrongPassword, Corrupt(String), Conflict, KeyChanged, Rollback, NotFound(String), InvalidInput(String), Unsupported(String) }  pub type Result<T>
pub mod model;    // see crates/keystead-core/src/model.rs (the data contract)
pub mod crypto;   // KdfParams, derive_key, seal/open (XChaCha20-Poly1305), random bytes
pub mod format;   // VaultFile (serde of the JSON above), read/write atomic
pub mod paths;    // data_dir(), vaults_dir(), settings_path(), legacy_dir(), is_portable()
pub mod store;    // VaultStore
pub mod vault;    // UnlockedVault
pub mod generator;// GeneratorOptions, generate()
pub mod totp;     // parse + code generation
pub mod matching; // URL matching for autofill
pub mod health;   // HealthReport, strength()
pub mod import;   // legacy VaultX 1.x, CSV (Chrome/Edge/Firefox/Bitwarden/generic), Bitwarden JSON, Keystead export; format detection, duplicate/conflict check
pub mod export;   // encrypted .keystead export, CSV, Bitwarden-compatible JSON
pub mod settings; // Settings (non-secret app settings) load/save
pub mod clipboard;// copy_secret(text, clear_after: Option<Duration>) using arboard (shared by app & TUI)
```

### `store::VaultStore`
```rust
impl VaultStore {
    pub fn open_default() -> Result<Self>;                       // paths::data_dir()
    pub fn new(root: impl Into<PathBuf>) -> Self;                // tests
    pub fn root(&self) -> &Path;
    pub fn list_vaults(&self) -> Result<Vec<VaultInfo>>;         // sorted by name; skips unreadable files
    pub fn create_vault(&self, name: &str, master_password: &str) -> Result<UnlockedVault>;
    pub fn create_vault_with_params(&self, name: &str, master_password: &str, kdf: KdfParams) -> Result<UnlockedVault>;
    pub fn unlock(&self, vault_id: &str, master_password: &str) -> Result<UnlockedVault>;   // Error::WrongPassword on bad pw
    pub fn unlock_with_recovery_key(&self, vault_id: &str, recovery_key: &str, new_master_password: &str) -> Result<UnlockedVault>; // sets the new master pw, rotates the vault key, recovery key stays valid
    pub fn delete_vault(&self, vault_id: &str, master_password: &str) -> Result<()>;      // verifies pw first, removes file + .bak + leftover temp files
    pub fn vault_path(&self, vault_id: &str) -> PathBuf;
}
```

### `vault::UnlockedVault`
```rust
impl UnlockedVault {
    pub fn info(&self) -> VaultInfo;
    pub fn data(&self) -> &VaultData;
    pub fn items(&self) -> &[VaultItem];                 // includes trashed items
    pub fn item(&self, id: &str) -> Option<&VaultItem>;
    pub fn summaries(&self) -> Vec<ItemSummary>;         // non-trashed, sorted by name (case-insensitive)
    pub fn search(&self, query: &str) -> Vec<ItemSummary>; // non-trashed; matches name, username, uris, notes, card brand; case-insensitive, all whitespace-separated terms must match
    pub fn logins_for_url(&self, url: &str) -> Vec<ItemSummary>; // non-trashed logins whose URIs match (matching::uri_matches), favorites first
    pub fn save_item(&mut self, item: VaultItem) -> Result<VaultItem>;
        // new if id empty or unknown: assigns uuid, createdAt; always sets updatedAt;
        // for logins: if password changed, pushes old one to password_history (max 10) and sets passwordRevisedAt;
        // normalises: login/card/identity Some/None according to type. Persists (save()).
    pub fn trash_item(&mut self, id: &str) -> Result<()>;      // sets deletedAt, persists
    pub fn restore_item(&mut self, id: &str) -> Result<()>;
    pub fn delete_item(&mut self, id: &str) -> Result<()>;     // permanent
    pub fn empty_trash(&mut self) -> Result<usize>;
    pub fn save_folder(&mut self, folder: Folder) -> Result<Folder>;
    pub fn delete_folder(&mut self, id: &str) -> Result<()>;   // items in it get folder_id = None
    pub fn add_generated_password(&mut self, password: &str) -> Result<()>; // history max 50, persists
    pub fn clear_generator_history(&mut self) -> Result<()>;
    pub fn import_items(&mut self, items: Vec<VaultItem>, folders: Vec<Folder>) -> Result<usize>; // assigns fresh ids, maps folder ids, persists once (no duplicate check)
    pub fn commit_import(&mut self, plan: ImportPlan, mode: ConflictMode) -> Result<ImportReport>;      // applies an import plan in one save, see "Import: detection, duplicates, conflicts"
    pub fn commit_import_ref(&mut self, plan: &ImportPlan, mode: ConflictMode) -> Result<ImportReport>; // same, keeps the plan (retry after `conflict` + reload_if_changed)
    pub fn rename(&mut self, name: &str) -> Result<()>;
    pub fn verify_master_password(&self, password: &str) -> bool;
    pub fn change_master_password(&mut self, current: &str, new: &str) -> Result<()>; // rotates the vault key unless a recovery key exists (see "Key rotation")
    pub fn create_recovery_key(&mut self) -> Result<String>;   // returns formatted code, replaces an existing one; rotates the vault key
    pub fn remove_recovery_key(&mut self) -> Result<()>;       // rotates the vault key
    pub fn has_recovery_key(&self) -> bool;
    pub fn save(&mut self) -> Result<()>;                      // atomic write, revision check, .bak
    pub fn reload_if_changed(&mut self) -> Result<bool>;       // re-reads file if the revision on disk is newer (opens it with the in-memory master KEK, so a recovery-key rotation elsewhere is followed);
                                                               // older revision → Error::Rollback; master password changed elsewhere → Error::KeyChanged (unlock again); state unchanged on error
    pub fn path(&self) -> &Path;
}
```

### Other core modules
```rust
// generator
#[serde(rename_all = "camelCase")] pub struct GeneratorOptions {
    pub kind: GeneratorKind,          // "password" | "passphrase"
    pub length: u32,                  // password: 5..=128, default 20
    pub uppercase: bool, pub lowercase: bool, pub digits: bool, pub symbols: bool, // defaults true,true,true,true
    pub min_digits: u32, pub min_symbols: u32,   // default 1, 1
    pub avoid_ambiguous: bool,        // default false (excludes Il1O0)
    pub words: u32,                   // passphrase: 3..=20, default 5
    pub separator: String,            // default "-"
    pub capitalize: bool,             // default true
    pub include_number: bool,         // default true
}
pub fn generate(opts: &GeneratorOptions) -> Result<String>;   // OsRng, unbiased; passphrase uses the embedded EFF large wordlist
// totp
pub struct TotpCode { pub code: String, pub period: u32, pub remaining: u32 }  // serde camelCase
pub fn totp_now(seed: &str) -> Result<TotpCode>;  // seed = base32 secret or otpauth:// URI (supports SHA1/SHA256/SHA512, digits 6-8, period)
pub fn totp_at(seed: &str, unix_seconds: u64) -> Result<TotpCode>;
// matching
pub fn uri_matches(stored: &LoginUri, page_url: &str) -> bool;
pub fn registrable_domain(host: &str) -> Option<String>; // via `psl` crate; IPs/localhost returned as-is
// health
pub struct Strength { pub score: u8 /*0..=4*/, pub crack_time: String, pub warning: String, pub suggestions: Vec<String> } // camelCase, via zxcvbn
pub fn strength(password: &str, user_inputs: &[&str]) -> Strength;
pub struct HealthReport { pub total_logins: usize, pub weak: Vec<String> /*item ids*/, pub reused: Vec<Vec<String>> /*groups of item ids*/, pub old: Vec<String> /* >365 days */, pub missing_totp_count: usize, pub score: u8 /*0..=100*/ }
pub fn health_report(data: &VaultData) -> HealthReport;
// import (drag & drop flow: detect_import → read_import → plan_import → UnlockedVault::commit_import)
pub struct ImportReport {          // camelCase
    pub imported: usize,           // items added as new items (incl. conflicts kept with keepBoth)
    pub updated: usize,            // existing logins updated from the file (mode update)
    pub skipped: usize,            // invalid rows/entries of the file (each has a warning)
    pub duplicates: Vec<ImportMatch>,        // not imported: already present (existingId "" = twice in the file)
    pub conflicts_skipped: Vec<ImportMatch>, // "conflictsSkipped": conflicts not imported
    pub warnings: Vec<String>,
}
pub enum ImportFormat { Legacy, Csv, BitwardenJson, Keystead } // serde "legacy" | "csv" | "bitwarden_json" | "keystead"; FromStr/Display/as_str(), needs_password()
pub struct DetectedImport { pub format: ImportFormat, pub needs_password: bool, pub file_name: String } // camelCase
pub fn detect_import(path: &Path) -> Result<DetectedImport>;        // by content, see below
pub struct ParsedImport { pub items: Vec<VaultItem>, pub folders: Vec<Folder>, pub warnings: Vec<String>, pub invalid: usize } // Debug without secrets, wiped on drop
pub fn read_import(path: &Path, format: ImportFormat, password: Option<&str>) -> Result<ParsedImport>;
pub fn plan_import(existing: &VaultData, parsed: ParsedImport) -> ImportPlan;
pub struct ImportPlan { pub new_items: Vec<VaultItem>, pub folders: Vec<Folder>, pub duplicates: Vec<ImportMatch>,
                        pub conflicts: Vec<ImportConflict>, pub invalid: usize, pub warnings: Vec<String>, /* + incoming conflict items (private) */ }
impl ImportPlan { pub fn preview(&self) -> ImportPreview }          // Debug prints counts only; items wiped on drop; not Clone/Serialize
pub struct ImportPreview { pub new_count: usize, pub duplicates: Vec<ImportMatch>, pub conflicts: Vec<ImportConflict>, pub invalid: usize, pub warnings: Vec<String> } // camelCase, secret-free
pub struct ImportMatch { pub incoming_name: String, pub username: String, pub site: String, pub item_type: ItemType, pub existing_id: String, pub existing_name: String } // camelCase, no passwords
pub struct ImportConflict { pub conflict_id: String /*"conflict-1", …*/, pub reason: ConflictReason, #[serde(flatten)] pub entry: ImportMatch } // camelCase, flat JSON
pub enum ConflictReason { Password, Totp }                         // serde "password" | "totp"
pub enum ConflictMode { #[default] Skip, Update, KeepBoth }        // serde "skip" | "update" | "keepBoth"
pub const IMPORT_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub struct LegacyVaultInfo { pub name: String, pub path: String } // camelCase
pub fn legacy_scan() -> Vec<LegacyVaultInfo>;                       // reads %LOCALAPPDATA%\VaultX\accounts.json + vault_*.json
pub fn import_legacy_file(path: &Path, password: &str) -> Result<(Vec<VaultItem>, Vec<String> /*warnings*/)>; // VaultX 1.x format, master OR recovery password
pub fn import_csv(text: &str) -> Result<(Vec<VaultItem>, Vec<Folder>, Vec<String>)>; // auto-detects Chrome/Edge/Firefox/Bitwarden/legacy-VaultX/generic headers, ',' or ';'
pub fn import_bitwarden_json(text: &str) -> Result<(Vec<VaultItem>, Vec<Folder>, Vec<String>)>; // unencrypted Bitwarden export
pub fn import_keystead_export(path: &Path, password: &str) -> Result<(Vec<VaultItem>, Vec<Folder>)>;
// export
pub fn export_encrypted(data: &VaultData, path: &Path, password: &str) -> Result<()>; // standalone v1 file (own random key/salt), trash excluded
pub fn export_csv(data: &VaultData) -> String;            // Bitwarden-style CSV (logins + notes), trash excluded; formula-like values get a leading ' (see below)
pub fn export_bitwarden_json(data: &VaultData) -> String; // unencrypted Bitwarden JSON, trash excluded
```

### Core additions & behaviour details (implemented in `keystead-core`)
Additive helpers beyond the signatures above (all optional to use):
```rust
// error
impl Error { pub fn code(&self) -> String }  // stable UI code; Json → "corrupt:<d>", NotFound(_) → "not_found", KeyChanged → "locked", Rollback → "corrupt:rollback"
// paths (all infallible)
pub fn data_dir() -> PathBuf; pub fn vaults_dir() -> PathBuf; pub fn settings_path() -> PathBuf;
pub fn legacy_dir() -> Option<PathBuf>; pub fn is_portable() -> bool;
pub fn portable_dir() -> Option<PathBuf>;   // "<exe dir>/Keystead-Data", whether it exists or not
pub fn default_data_dir() -> PathBuf;       // OS location, ignoring portable mode and $KEYSTEAD_DATA_DIR
// crypto
impl KdfParams { pub fn validate(&self) -> Result<()> }  // file DoS guard: memory ≤ 1 GiB, t ≤ 20, p ≤ 16
// store / vault
impl VaultStore { pub fn vaults_dir(&self) -> PathBuf }
impl UnlockedVault { pub fn id(&self) -> &str; pub fn name(&self) -> &str; pub fn revision(&self) -> u64;
                     pub fn folders(&self) -> &[Folder]; pub fn generator_history(&self) -> &[GeneratedPassword] }
// import / export: one call per Tauri `import_data` / `export_data` format string
pub fn import::import_into(vault: &mut UnlockedVault, format: &str /*legacy|csv|bitwarden_json|keystead*/,
                           path: &Path, password: Option<&str>) -> Result<ImportReport>; // = read_import + plan_import + commit_import(Skip): never imports duplicates
pub fn import::legacy_scan_dir(dir: &Path) -> Vec<LegacyVaultInfo>;
pub fn export::export_to_file(data: &VaultData, format: &str /*keystead|csv|bitwarden_json*/,
                              path: &Path, password: Option<&str>) -> Result<()>;
pub fn export::export_encrypted_with_params(data, path, password, kdf: KdfParams) -> Result<()>;
// settings
impl Settings { pub fn load() -> Settings; pub fn load_from(&Path) -> Settings; pub fn save(&self) -> Result<()>;
                pub fn save_to(&self, &Path) -> Result<()> }   // + free fns settings::load(), settings::save(&Settings)
// clipboard
pub fn copy_secret(text: &str, clear_after: Option<Duration>) -> Result<()>; // excluded from clipboard history; Windows: CanIncludeInClipboardHistory=0 and CanUploadToCloudClipboard=0 (no Cloud Clipboard sync)
pub fn copy_text(text: &str) -> Result<()>;            // non-secret, no auto-clear
pub fn clear_pending_secret() -> Result<bool>;         // e.g. on lock: clears now if our secret is still there
// totp
pub fn parse(seed: &str) -> Result<TotpParams>;        // secret, algorithm, digits, period, issuer, account
// health
pub fn health_report_at(data: &VaultData, now_ms: i64) -> HealthReport;
```
Behaviour notes:
* `save_item`: an empty/whitespace name → `invalid_input:name_required`
  (> 200 chars → `name_too_long`); `createdAt`, `deletedAt` and
  `passwordRevisedAt` of existing items are kept by the core (trash state
  changes only via trash/restore); unknown `folderId` → `null`.
* Every mutation is committed to memory only after the file was written,
  so after `conflict` the in-memory state is unchanged: call
  `reload_if_changed()` and retry. A save never recreates a deleted vault
  file (`not_found`).
* Recovery codes are normalised to uppercase without dashes/whitespace;
  Crockford look-alikes are accepted (`O`→`0`, `I`/`L`→`1`). A malformed
  code → `invalid_input:recovery_key_format`; a vault without recovery key →
  `not_found`.
* Legacy import: an HMAC mismatch is reported as `wrong_password` (VaultX
  1.x could not distinguish either). Importers keep the source ids;
  `import_items` replaces them and merges imported folders into existing
  folders of the same name (case-insensitive).
* CSV export ("CSV injection"): folder, name, notes, custom field names and
  URIs that a spreadsheet would evaluate (starting with `= + - @`, also after
  leading whitespace, or with tab/CR) get a leading `'`; usernames only when
  starting with `=` or `@` (phone numbers like `+49…` stay). Passwords and
  TOTP secrets are written unchanged so other managers import them
  correctly – the file must not be opened in a spreadsheet. Values that
  already start with `'` followed by such a value get one more `'`; the
  Bitwarden-style CSV import strips exactly one, so a Keystead → Keystead
  round trip is lossless. The Bitwarden JSON export is unchanged.
* Plain-text exports (`csv`, `bitwarden_json`) are written with
  `write_atomic` (new temporary file, see "Vault file format"): readable only
  by the current user on Unix; on Windows the file gets the target folder's
  default permissions.
* `invalid_input` details used by the core: `name_required`, `name_too_long`,
  `password_empty`, `password_required`, `kdf_params`, `recovery_key_format`,
  `length`, `words`, `separator`, `no_character_set`,
  `minimums_exceed_length`, `totp_secret`, `totp_secret_empty`, `totp_uri`,
  `totp_digits`, `totp_period`, `csv_empty`, `csv_unknown_columns`,
  `bitwarden_json`, `json: …`, `csv: …`, `format <x>`.
* `unsupported` details of the import meant for the UI: `unknown_format`,
  `bitwarden_encrypted`, `file_too_large` (others are English free text, e.g.
  `vault format version 2`, `not a Keystead vault file`).
* Strength texts (`crackTime`, `warning`, `suggestions`) and import warnings
  are English.

### Import: detection, duplicates, conflicts (`import`)
Drag & drop: `detect_import(path)` → ask for a password if `needsPassword` →
`read_import(path, format, password)` → `plan_import(vault.data(), parsed)` →
show `plan.preview()` → `vault.commit_import(plan, mode)`. The plan holds the
incoming secrets in memory until it is committed or dropped (wiped on drop,
`Debug` prints counts only). `import_into` runs the same steps with
`ConflictMode::Skip`, so every import route skips items that already exist.

* **Reading** (`detect_import`, `read_import`, and the older `import_*`
  helpers that take a path): files over `IMPORT_MAX_BYTES` (50 MiB) →
  `unsupported:file_too_large` without reading them; missing → `not_found`;
  directories, other non-regular files (FIFO, device – never opened) and
  unreadable files → `io:…`. Text is UTF-8 (BOM optional), UTF-16 with BOM or
  Windows-1252. `read_import` checks `password_required` (missing/empty
  password for `legacy`/`keystead`) before touching the file; a wrong one →
  `wrong_password`. Keystead files: trash and generator history are never
  imported.
* **Detection by content** (`detect_import`); the extension only decides
  which reading is tried first (`.csv`/`.tsv`/`.txt` → CSV first):
  1. JSON object with `"format": "keystead"` → `keystead`, needs password
     (header validated like a vault file, e.g.
     `unsupported:vault format version 2`);
  2. `"encrypted": true` → `unsupported:bitwarden_encrypted` (password- or
     account-protected Bitwarden export);
  3. `items` array and `"encrypted": false` (or a `folders`/`collections`
     array) → `bitwarden_json`;
  4. `Salt` + `Iterations` + `IV`/`Data` (keys case-insensitive, `Mac`
     optional) → `legacy`, needs password (header validated);
  5. a CSV header row of a supported dialect (Bitwarden, Firefox, or the
     generic/Chrome/legacy aliases with at least two recognised columns;
     `,` `;` or tab, Excel `sep=` line) → `csv`;
  6. anything else (other JSON, plain text, binary, empty) →
     `unsupported:unknown_format`.
  JSON is scanned shallowly (top-level keys only, values skipped), so
  detection builds no tree of the file and keeps none of its secrets.
* **Matching** (`plan_import`; only non-trashed vault items count):
  * login: key (site, username) – site = lower-case host of the first
    http(s) URI (scheme-less URIs count as `https://`), trailing dot and a
    leading `www.` removed, port ignored; without a usable URI the name
    (trimmed, lower case, inner whitespace collapsed). Username trimmed,
    case-insensitive. Same key + same password (+ same TOTP seed if both have
    one; a bare secret equals an `otpauth://` URI with that secret) =
    **duplicate**, even if the incoming item has extra data (TOTP, notes).
    Same key + different password (also empty vs. set) = **conflict**
    (`reason: "password"`); same password but different TOTP seeds =
    conflict (`reason: "totp"`). With several existing logins for the key,
    the conflict names the most recently updated one.
  * card: the digits of the number (cards without digits are always new) →
    duplicate; cards never conflict.
  * identity: (first name, last name, e-mail), case-insensitive; if all are
    empty, the normalised name → duplicate only.
  * secure note: (normalised name, note text with `\r\n` → `\n` and
    surrounding whitespace ignored) → duplicate; same name with another text
    is a new item.
  * Items of different types never match. An item equal to an earlier item
    of the same file is a duplicate of that one (`existingId` empty,
    `existingName` = the earlier item's name).
  * `ImportMatch`: `incomingName`; `username` = login username (trimmed), for
    other types the list subtitle (card `•••• 1234`, identity name/e-mail,
    empty for notes); `site` = the login host (empty without URI / for other
    types); `itemType`; `existingId`, `existingName`. Never a password, card
    number or note text.
* **Commit** (`commit_import`, `commit_import_ref`): one save = one revision;
  nothing is written when nothing changes. The incoming items are
  classified again against the current vault: an item that became a
  duplicate meanwhile is skipped (`duplicates`), a conflict whose login was
  deleted or trashed is added as a new item, an item planned as new that now
  conflicts follows the mode. Conflicts removed from `plan.conflicts` by the
  caller are left out.
  * `skip` (default): conflicts are not imported (`conflictsSkipped`).
  * `update`: the incoming password replaces the existing one through the
    normal save path (old password → `passwordHistory`, `passwordRevisedAt`
    and `updatedAt` set); the incoming TOTP seed is added if the existing
    login has none. An existing TOTP seed is never replaced and an empty
    incoming password never clears one – such a conflict with nothing to take
    over goes to `conflictsSkipped`. Name, URIs, notes, folder stay.
  * `keepBoth`: the incoming login is added as a new item.
  * New items get fresh ids (like `import_items`); the file's folders are
    merged by name into existing folders or created – only folders used by
    an added item (a re-import leaves no empty folders behind).
  * `commit_import_ref` leaves the plan intact: after `conflict`,
    `reload_if_changed()` and commit the same plan again. Committing a plan
    a second time adds nothing (its items are duplicates by then).

### Legacy VaultX 1.x format (for `import_legacy_file`)
JSON object with `Version` (1|2), `Salt` (b64, 16 B), `Iterations` (int,
usually 100000), `IV` (b64, 16 B), `Data` (b64 AES-256-CBC/PKCS7
ciphertext), optional `Mac` (b64 HMAC-SHA256 over IV‖ciphertext), optional
`AccountName`, `VaultId`, recovery fields `RecoverySalt`,
`RecoveryIterations`, `RecoveryKeyIV`, `RecoveryKeyData`.

* Key material = PBKDF2-**HMAC-SHA1** (.NET `Rfc2898DeriveBytes` default)
  (password UTF-8, salt, iterations) → 64 bytes: `encKey = [0..32]`,
  `macKey = [32..64]`. For v1 files without `Mac` only 32 bytes (`encKey`) are
  used.
* If `Version >= 2` or `Mac` present: verify HMAC first (constant time).
* Recovery: `recKey` = PBKDF2-SHA1(recovery pw, RecoverySalt,
  RecoveryIterations, 32); AES-CBC-decrypt `RecoveryKeyData` with
  `RecoveryKeyIV` → 64 bytes (`encKey‖macKey`) or 32 bytes (`encKey` only).
* Plaintext is JSON (UTF-8, possibly with BOM) `{ "Entries": [ { "Id",
  "Title", "Url", "Username", "Password", "Phone", "Email", "Notes",
  "Other", "UpdatedAt" } ], "TotpSecret"?: "..." }`. `Entries` may also be a
  single object instead of an array (PowerShell quirk).
* Mapping: Title→name, Url→uris[0] (if non-empty), Username→username (if
  empty use Email), Password, Notes→notes, Email (if not used as username)
  → custom field "E-Mail", Phone → custom field "Telefon", Other → custom
  field "Sonstiges". `TotpSecret` was the legacy *vault unlock* 2FA – it is
  ignored with a warning.

## Settings (`keystead_core::settings::Settings`, `<data_dir>/settings.json`)

```ts
interface Settings {
  theme: "system" | "light" | "dark";        // default "system"
  language: "de" | "en";                     // default: "de" if OS locale starts with de, else "en"
  autoLockMinutes: number;                   // 0 = never; default 15
  lockOnSystemLock: boolean;                 // default true (best effort)
  clipboardClearSeconds: number;             // 0 = never; default 30
  minimizeToTray: boolean;                   // close button hides to tray; default true
  startInTray: boolean;                      // default false
  browserIntegration: boolean;               // bridge server on/off; default true
  lastVaultId: string | null;
  showIcons: boolean;                        // website favicons – false by default: offline, letter avatars instead
}
```

## Desktop backend ↔ frontend (Tauri commands)

All commands are `async`, return `Result<T, String>` where the error string is
a **stable error code** the UI translates: `wrong_password`, `locked`,
`not_found`, `conflict`, `invalid_input:<detail>`, `io:<detail>`,
`corrupt:<detail>`, `unsupported:<detail>`. `locked` also comes from the core
(`Error::KeyChanged`: the master password was changed by another process,
the session has to unlock again); `corrupt:rollback` = `Error::Rollback`.
Argument names are camelCase on the JS side (Tauri converts to snake_case).

Commands marked † change or export the open vault and also take
`pageVaultId: string | null`: the vault the UI page works on (`api.ts` adds
it – the vault the page shows, or the one it has just created or unlocked
itself, e.g. for the setup wizard's import and recovery key). If that is not
the open vault, they answer `locked` and change nothing (checked under the
same state lock as the change, `Core::mutate_for_page`): the browser
extension can replace the open vault a moment before the page learns of it
(`vault://unlocked`) and reloads, and an edit meant for the old vault must not
land in the new one – `save_item` with an id the new vault does not know
would create the item there. `generate_password` then only skips the
history. The bridge's own writes are not affected.

| Command | Args | Returns |
|---|---|---|
| `app_info` | – | `AppInfo { version, dataDir, portable, platform: "windows"\|"linux"\|"macos", extensionId }` |
| `list_vaults` | – | `VaultInfo[]` |
| `session_state` | – | `SessionState { unlocked: boolean, vault: VaultInfo \| null }` |
| `create_vault` | `name, masterPassword` | `VaultInfo` (and unlocks it) |
| `unlock_vault` | `vaultId, masterPassword` | `VaultInfo` |
| `unlock_with_recovery` | `vaultId, recoveryKey, newMasterPassword` | `VaultInfo` |
| `lock_vault` | – | `null` |
| `touch_activity` | – | `null` (resets auto-lock timer; UI calls it throttled to ≤1/30 s on input – mouse move/click, key, wheel – while its window is focused and visible) |
| `list_items` | – | `VaultItem[]` (all, incl. trash; UI filters) |
| `list_folders` | – | `Folder[]` |
| `save_item` † | `item: VaultItem` | `VaultItem` |
| `trash_item` / `restore_item` / `delete_item` † | `id` | `null` |
| `empty_trash` † | – | `number` |
| `save_folder` † | `folder: Folder` | `Folder` |
| `delete_folder` † | `id` | `null` |
| `generate_password` † | `options: GeneratorOptions, remember: boolean` | `string` (adds to history if remember) |
| `generator_history` | – | `GeneratedPassword[]` |
| `clear_generator_history` † | – | `null` |
| `password_strength` | `password` | `Strength` |
| `totp_code` | `seed` | `TotpCode` |
| `copy_text` | `text, sensitive: boolean` | `null` (sensitive → excluded from clipboard history, cleared after `clipboardClearSeconds` and on lock/quit – also with `clipboardClearSeconds` = 0 –, only if clipboard still holds it. The UI copies passwords, TOTP codes, card number/code, hidden fields, notes of every item type, generated passwords and the recovery key as sensitive) |
| `health_report` | – | `HealthReport` |
| `change_master_password` † | `current, newPassword` | `null` |
| `create_recovery_key` † | – | `string` |
| `remove_recovery_key` † | – | `null` |
| `rename_vault` † | `name` | `VaultInfo` |
| `delete_vault` | `vaultId, masterPassword` | `null` (locks if it was the open vault) |
| `legacy_scan` | – | `LegacyVaultInfo[]` |
| `import_data` † | `format: "legacy"\|"csv"\|"bitwarden_json"\|"keystead", path, password: string \| null` | `ImportReport` (into the unlocked vault) |
| `export_data` † | `format: "keystead"\|"csv"\|"bitwarden_json", path, password: string \| null, masterPassword` | `null` (re-verifies master pw) |
| `get_settings` | – | `Settings` |
| `save_settings` | `settings: Settings` | `Settings` |
| `browser_status` | – | `BrowserStatus { serverRunning, extensionId, browsers: BrowserInfo[], clients: PairedClient[] }` |
| `register_browsers` | `browsers: string[]` (ids) | `BrowserStatus` |
| `unregister_browsers` | `browsers: string[]` | `BrowserStatus` |
| `revoke_client` | `clientId` | `BrowserStatus` |
| `respond_pairing` | `requestId, approve: boolean` | `null` |
| `open_terminal` | – | `null` (spawns `Keystead --cli` in a new console) |
| `open_data_dir` | – | `null` |
| `set_portable_mode` | `enabled: boolean` | `AppInfo` (moves vault files; requires locked or re-unlock) |

`BrowserInfo { id: "chrome"|"edge"|"brave"|"chromium"|"vivaldi", name, detected: boolean, registered: boolean }`,
`PairedClient { id, name, createdAt, lastSeenAt }`.

**Events** (backend → frontend, `app.emit`):
* `vault://locked` – payload `{ reason: "manual"|"timeout"|"system" }`
* `vault://changed` – payload `{}` – items changed outside the UI (extension saved a login, file changed by TUI → reloaded). UI re-fetches.
* `bridge://pairing-request` – payload `{ requestId, clientName, code }` – UI shows a modal with the 6-digit code; user approves/denies → `respond_pairing`.
* `bridge://pairing-closed` – payload `{ requestId }` – that request ended
  without a decision (cancelled in the browser / browser closed, replaced by
  a newer request of the same browser, timed out): the UI closes its modal.
  `respond_pairing` for it answers `not_found`.
* `bridge://unlock-request` – payload `{}` – extension asked to open the app; UI focuses the unlock screen.
* `vault://unlocked` – payload `VaultInfo` – the vault was unlocked outside
  the UI (extension `unlock`); the UI switches to the unlocked view. Also
  sent when the extension switched to another vault while one was open: the
  UI then leaves the old vault like on lock (page reload, see below; an info
  toast `lock.switchedToast` is carried over) and the boot shows the new one.
  Until then the old view's commands marked † answer `locked`.
  Not sent for the UI's own `unlock_vault`/`create_vault`/`unlock_with_recovery`.

The frontend's `src/lib/api.ts` wraps every command with typed functions and
falls back to an in-memory **mock backend** (`src/lib/mock.ts`, realistic
sample data, password `demo`) when not running inside Tauri
(`!("__TAURI_INTERNALS__" in window)`) – used for `npm run dev` in a browser
and for automated screenshots. Mock-only extras: `?mock=empty` starts without
any vault (first-run screen), `?lang=en` starts in English, and
`window.__keysteadMock` offers `triggerPairing(name?)`, `simulateTimeoutLock()`,
`simulateExternalChange()` and `simulateUnlockRequest()`.

Frontend integration requirements for `src-tauri`:
* File pickers use `@tauri-apps/plugin-dialog` (`open`, `save`): register
  `tauri-plugin-dialog` and grant `dialog:allow-open` + `dialog:allow-save`
  (plus `core:default` for events) to the main window's capability.
* Website links call `window.open(url, "_blank")` (http/https only); the app
  must route new-window requests to the system browser (e.g. via
  `tauri-plugin-opener`) instead of opening a webview window.
* The UI uses React inline `style` attributes and `data:` SVGs in CSS, so a
  CSP needs `style-src 'self' 'unsafe-inline'` and `img-src 'self' data:`.
* Build: `devUrl` `http://localhost:1420`, `frontendDist` `../dist`,
  `beforeDevCommand` `npm run dev`, `beforeBuildCommand` `npm run build`.
* Leaving an open vault (any lock, `delete_vault` of the open vault, the
  vault closed by `set_portable_mode`, the extension switching to another
  vault – the splash is shown until the reload) reloads the page (`src/lib/discard.ts`)
  once the commands in flight have answered (≤ 3 s), so the decrypted items,
  generator history etc. do not linger in the renderer's JS heap – hygiene:
  unreachable, not overwritten. Visible toasts marked `carry` (lock reason,
  portable-mode result; never vault data) survive the reload via
  `sessionStorage`. Not with the mock backend (it lives in the same page; a
  vault switch remounts the main screen, which is keyed by the vault id).
* A `locked` answer to the main screen's `list_items`/`list_folders` also
  returns to the unlock screen (not only `vault://locked`).

### Desktop backend behaviour details (implemented in `src-tauri`)
* Commands run on the blocking thread pool; complex arguments (`item`,
  `folder`, `options`, `settings`) that do not parse → `invalid_input:item`
  / `folder` / `options` / `settings`. Mutations that hit `conflict` reload
  the vault and retry once (the UI then also gets `vault://changed`).
* Auto-lock activity = `touch_activity`, deliberate user commands (unlock,
  mutations, `copy_text`, generator, import/export, settings, …) and bridge
  user actions. Polled/passive commands (`totp_code`, `browser_status`,
  `session_state`, `app_info`, `list_*`, `get_settings`) do **not** reset
  the timer. The monitor checks every 5 s (idle time includes sleep).
* `lockOnSystemLock`: Windows uses session notifications
  (`WM_WTSSESSION_CHANGE`/`WTS_SESSION_LOCK`) and `PBT_APMSUSPEND`; on every
  OS a clock jump > 2 min between the monitor's 1-s ticks (suspend /
  hibernate) locks with reason `system`. Linux/macOS do not detect a plain
  screen lock without suspend.
* The vault file is checked every 2 s (`reload_if_changed`) → `vault://changed`.
* Locking (any reason) also clears a secret copied through the app
  (`copy_text` with `sensitive`, the bridge's `copy_field`/`copy_secret`) if
  the clipboard still holds it – also with `clipboardClearSeconds` = 0 (the
  app then remembers the secret itself, as `Zeroizing<String>`); so does
  quitting the app. `lock_vault` (the UI's own request) emits no event;
  tray "Sperren" and the extension's `lock` emit `vault://locked {manual}`.
* `list_items`, `save_item` and `generator_history` overwrite the strings of
  their cloned result once Tauri has serialized it (`src/wipe.rs`); the
  serialized IPC message itself is out of reach.
* Every unlock/create stores the vault as `lastVaultId` (also an
  extension unlock, so the app's unlock screen preselects that vault after
  the next lock). The extension's `unlock` opens its `vaultId`; without one
  `lastVaultId` if it exists, else the only vault (else `not_found`). If
  another vault is open, the new one is unlocked (key derivation) while the
  old one stays open and usable; only on success it replaces it
  (`Core::install_vault`): the old vault is dropped (wiped) and a secret it
  copied is cleared from the clipboard like on lock, without
  `vault://locked`; then `vault://unlocked` with the new `VaultInfo`. A failed
  attempt (wrong password, rate limit, unknown id) leaves the open vault
  open. Unlocking the vault that is already open only checks the password
  (no event).
* `import_data` emits `vault://changed` after a successful import.
* `delete_vault` / `respond_pairing` / `revoke_client`: unknown ids →
  `not_found`.
* `open_terminal`: Windows `CREATE_NEW_CONSOLE`; Linux tries `$TERMINAL`,
  `x-terminal-emulator`, `gnome-terminal`, `konsole`, `xfce4-terminal`,
  `kitty`, `alacritty`, `foot`, `xterm` (none → `unsupported:no_terminal`);
  macOS Terminal via `osascript`.
* `set_portable_mode` first checks the preconditions (`portable::preflight`:
  the errors below except a failing move, plus `io:…` if the exe folder is
  not writable) while the vault stays open and the bridge keeps running.
  Then it stops the bridge, locks an open vault itself and emits
  `vault://locked {manual}` – whether the move then succeeds or fails; the
  UI also checks `session_state` afterwards in both cases –, moves the whole data
  directory (copy, commit by creating/renaming `Keystead-Data`, then delete
  the old copy; `*.lock`/`*.tmp` – including the `.<name>.<hex>.tmp` temporary
  files of `write_atomic` – are skipped), re-registers the native host
  and restarts the bridge. Errors: `unsupported:data_dir_override` (with
  `$KEYSTEAD_DATA_DIR`), `unsupported:portable_installed` (enabling while the
  exe lives in the default data directory itself – the per-user NSIS setup
  installs to `%LOCALAPPDATA%\Keystead`), `invalid_input:target_exists` (a
  vault file with the same id already exists at the destination), `io:…`.
* Pairing requests that arrived before the page subscribed (app launched
  hidden by the native host) are re-sent ~1 s after the first
  `session_state` call of each page load; the UI de-duplicates by
  `requestId`. If the lock state or the open vault changed in that second
  (the extension unlocked, switched or locked the vault while the page
  booted), `vault://unlocked` resp. `vault://locked {manual}` is sent again at
  that point.
* Debug builds only: `KEYSTEAD_TEST_AUTO_APPROVE_PAIRING=1` approves pairing
  requests automatically (no dialog) for automated end-to-end tests.
* Window: links/`window.open` and any navigation away from the app open in
  the system browser (http/https only). Without a tray icon (Linux without
  AppIndicator) the close button quits even with `minimizeToTray`, and
  `startInTray` is ignored.
* The main window is excluded from screen capture (`contentProtected` in
  tauri.conf.json and `content_protected(true)` on the builder: Windows
  `WDA_EXCLUDEFROMCAPTURE` – screenshots, recordings, screen sharing and
  Windows Recall see it black or not at all; before Windows 10 2004 it is
  shown black; macOS `NSWindowSharingNone`; no effect on Linux).

## Browser bridge protocol – `keystead-bridge`

### Framing
Both legs (browser↔host via stdin/stdout, host↔app via local socket) use the
native-messaging framing: `u32` **little-endian** byte length + UTF-8 JSON.
Max message 1 MiB (host → browser) / 4 MiB otherwise.

### Local socket
* Windows: named pipe `\\.\pipe\keystead-bridge-<USERNAME>` (via the
  `interprocess` crate, `GenericNamespaced`), current-user only: the pipe's
  security descriptor has the current user's SID as **owner** and a DACL
  granting only that SID. The pipe namespace is machine-wide and the name is
  predictable, so another user could create it first ("squatting"): **both
  ends verify the pipe owner** – the host (client) checks that the pipe it
  connected to is owned by its own user SID *before sending anything*
  (otherwise `PermissionDenied`, nothing is sent and the browser gets
  `app_unavailable`), and a starting app that finds the name taken does the
  same check: owned by its user → `AlreadyRunning`, otherwise (or not
  verifiable) a `PermissionDenied` I/O error ("possible hijack", logged) and
  the bridge stays off.
* Unix: `$XDG_RUNTIME_DIR/keystead-bridge.sock`, fallback
  `/tmp/keystead-bridge-<uid>.sock`, mode 0600. Both ends check the peer uid
  (`SO_PEERCRED`).
* The host forwards each browser message to the app and the app's reply back.
  If it cannot connect, it launches the app (`<own exe> --background`, the
  host *is* Keystead.exe) and retries for up to 8 s; if that fails it answers
  `{ "id", "ok": false, "error": "app_unavailable" }`. An endpoint served by
  another user is answered with `app_unavailable` right away (no launch).

### Messages (extension → app)
Every request: `{ "id": "<uuid>", "type": "<type>", "clientId"?: string, "token"?: string, ...payload }`.
Every response: `{ "id": "<same>", "ok": true, "data": ... }` or `{ "id", "ok": false, "error": "<code>" }`.
Error codes: `not_paired`, `pairing_denied`, `locked`, `wrong_password`, `not_found`, `invalid_request`, `app_unavailable`, `internal`.

| type | needs pairing | payload | data |
|---|---|---|---|
| `status` | no | – | `{ appVersion, paired: bool, unlocked: bool, vaultName: string\|null, vaultId: string\|null }` |
| `pair` | no | `clientName` (e.g. "Chrome – DESKTOP-1"), `code` (6 digits shown in the popup) | `{ clientId, token }` once the user approved in the app (the request blocks up to 120 s) |
| `list_vaults` | yes (locked or unlocked) | – | `{ vaults: { id, name }[], currentVaultId: string\|null, lastVaultId: string\|null }` – sorted by name (case-insensitive); `currentVaultId` = the unlocked vault |
| `unlock` | yes | `password`, `vaultId?` | `{ vaultName, vaultId }` – opens `vaultId` (switching if another vault is open), without it the app's last used vault |
| `lock` | yes | – | `null` |
| `focus_app` | no | – | `null` (shows/raises the window) |
| `logins_for_url` | yes, unlocked | `url` | `ItemSummary[]` |
| `search` | yes, unlocked | `query` | `ItemSummary[]` (max 50) |
| `get_login` | yes, unlocked | `itemId` | `{ id, name, username, password, totp: TotpCode\|null, uris: string[] }` |
| `get_totp` | yes, unlocked | `itemId` | `TotpCode` |
| `generate_password` | yes | `options?` (GeneratorOptions partial) | `string` (uses defaults + stored in generator history if unlocked) |
| `save_login` | yes, unlocked | `name, url, username, password` | `{ id }` (creates new login) |
| `update_password` | yes, unlocked | `itemId, password` | `{ id }` |
| `check_login_password` | yes, unlocked | `itemId, password` | `bool` – whether `password` equals the stored password (the secret is never returned; used for the save/update prompt) |
| `copy_field` | yes, unlocked | `itemId, field: "password"\|"totp"` | `{ remaining: number\|null }` – the **app** copies the field (seconds a TOTP code stays valid, `null` for passwords); the secret is not sent to the browser |
| `copy_secret` | yes, unlocked | `text` (non-empty, e.g. a generated password) | `null` – copied by the app like `copy_field` |

Pairing flow: the extension generates a random 6-digit `code`, displays it in
its popup, and sends `pair { clientName, code }`. The app emits
`bridge://pairing-request` with that code; the user compares the codes and
approves. The app stores `{ id, name, tokenSha256, createdAt, lastSeenAt }`
and returns `{ clientId, token }` (token = 32 random bytes, base64url). The
extension stores both in `chrome.storage.local`.

### Native host registration
Host name: **`com.keystead.bridge`**. Manifest file `com.keystead.bridge.json`
(written to `<data_dir>/native-host/`) with `"path"` = absolute path of the
current exe, `"type": "stdio"`, `"allowed_origins": ["chrome-extension://imfndemblnaalppnmdplagajjielnaok/"]`.
* Windows: registry `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.keystead.bridge`,
  `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\…`,
  `HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\…`,
  `HKCU\Software\Chromium\NativeMessagingHosts\…`,
  `HKCU\Software\Vivaldi\NativeMessagingHosts\…` (default value = manifest path).
* Linux: copy manifest to `~/.config/google-chrome/NativeMessagingHosts/`,
  `~/.config/chromium/…`, `~/.config/microsoft-edge/…`,
  `~/.config/BraveSoftware/Brave-Browser/…`, `~/.config/vivaldi/…`.
* macOS: `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/` etc.
* The app re-registers on start when `browserIntegration` is on and a
  registered manifest points to a different exe path (portable exe moved).

### Extension ID
The extension's `manifest.json` contains a fixed public `key`, so the
unpacked extension always has the ID **`imfndemblnaalppnmdplagajjielnaok`**.

### Bridge behaviour details & Rust API (implemented in `keystead-bridge`)
Behaviour (additive to the table above):
* Requests on one connection are answered in order and the host keeps a
  single connection, so a pending `pair` (≤ 120 s) delays later messages on
  the same native-messaging port.
* Credentials are checked before the lock state (`not_paired` wins over
  `locked`). Unknown fields are ignored; missing/mistyped fields, an unknown
  `type` or a non-object → `invalid_request` (with the request's `id` if it
  is a string, else `""`).
* `status`: `vaultName` and `vaultId` are only revealed to paired clients
  (else `null`); invalid credentials just give `paired: false`.
* `list_vaults`: `VaultBackend::list_vaults` sorted by name (lowercase, then
  id); `lastVaultId` only if that vault is listed. Not a user action (the
  popup lists the vaults whenever it opens).
* `pair`: `clientName` trimmed, 1–100 chars; `code` exactly 6 ASCII digits,
  else `invalid_request` (the app is not asked). A new `pair` with the same
  `clientName` supersedes a pending one (the older gets `pairing_denied`); at
  most 4 pending requests; deny/timeout → `pairing_denied`. While a `pair`
  waits, the app polls its connection (every 250 ms): if the requester hung
  up (the extension cancelled, the browser ended the host), the request is
  withdrawn and can no longer be approved. Withdrawn, superseded and
  timed-out requests are reported via `VaultBackend::pairing_closed`.
* `unlock`: after 5 wrong passwords in a row it answers `wrong_password` for
  30 s without trying; every further failure restarts the 30 s, a success
  resets the counter. Attempts are serialised. The limit is shared by all
  vaults (a wrong password while switching counts too). `vaultId` absent or
  `null` = the backend picks the vault (previous behaviour); `""` →
  `invalid_request`; an unknown id → `not_found` (does not count as a
  failure). A failed unlock never closes the open vault.
* `VaultBackend::on_activity` runs after successful *user actions* only
  (`unlock`, `search`, `get_login`, `get_totp`, `generate_password`,
  `save_login`, `update_password`, `copy_field`, `copy_secret`) – `status`
  polling, the automatic `logins_for_url` and `check_login_password` (a page
  can trigger it by submitting forms) must not keep the vault from
  auto-locking.
* `copy_field` / `copy_secret` call `VaultBackend::copy_secret`, whose default
  copies with `keystead_core::clipboard::copy_secret` in the app process:
  excluded from clipboard history, cleared after `clipboardClearSeconds` from
  the settings file (0 = never) if the clipboard still holds it, and – being
  the same pending secret – cleared when the app locks. (The desktop app
  overrides it with the path of its own sensitive `copy_text`.) TOTP codes
  are copied without spaces. `copy_field` with another `field` → `invalid_request`; a
  login without TOTP seed → `not_found`.
* `check_login_password` compares without an early exit and answers only
  `true`/`false` (unknown id or non-login → `not_found`).
* Memory hygiene (best effort): request passwords (`unlock`, `save_login`,
  `update_password`, `check_login_password`, `copy_secret`) are held in
  `Zeroizing<String>`; `LoginSecret` wipes username, password and TOTP code
  on drop (so its fields cannot be moved out); a `Response` wipes every
  string in `data` on drop (`protocol::wipe_value`). Transient copies inside
  serde/serde_json (buffered tagged/flattened content, escaped strings,
  buffer growth) are not covered.
* `bridge-clients.json` = `{ "version": 1, "clients": [ { id, name,
  tokenSha256, createdAt, lastSeenAt } ] }`; `tokenSha256` = lowercase hex
  SHA-256 of the UTF-8 token string; atomic writes (0600 on Unix);
  `lastSeenAt` is persisted at most once a minute. A corrupt file is moved to
  `bridge-clients.json.corrupt` (browsers must pair again).
* Server: never displaces a live server (second instance of the same user →
  `AlreadyRunning`; endpoint held by another user or not verifiable →
  `Error::Io(PermissionDenied)`, see "Local socket"); removes stale Unix
  socket files; both ends check the peer uid on Unix. Windows: pipe owner +
  DACL = current user's SID only (best effort, falls back to default pipe
  security), remote clients rejected; clients verify the pipe owner (any
  Win32 failure counts as "not ours"), the server relies on the DACL for its
  clients. After `stop()` open connections are closed unanswered on their
  next request, so the host reconnects to a restarted server (or launches the
  app).
* Host: oversized browser frame → `invalid_request` and exit; app reply
  > 1 MiB → `internal`; after a failed launch, requests within 30 s fail fast
  with `app_unavailable`; an endpoint that fails the owner check →
  `app_unavailable` without launching the app. `$KEYSTEAD_BRIDGE_SOCKET` overrides the endpoint
  (Unix: socket path, Windows: pipe name), `$KEYSTEAD_APP_EXE` the executable
  the host launches.

```rust
pub trait VaultBackend: Send + Sync + 'static {   // implemented by the desktop app
    fn app_version(&self) -> String;
    fn unlocked_vault(&self) -> Option<VaultSummary>;                            // { id, name } of the open vault
    fn list_vaults(&self) -> Result<(Vec<VaultSummary>, Option<String>), BridgeError>; // all vaults (any order) + lastVaultId
    fn unlock(&self, vault_id: Option<&str>, password: &str) -> Result<VaultSummary, BridgeError>; // None: last used / only vault; replaces an open vault only on success
    fn lock(&self);
    fn focus_app(&self);
    fn request_pairing(&self, request_id: &str, client_name: &str, code: &str); // emit bridge://pairing-request, don't block
    fn pairing_closed(&self, request_id: &str) {}                                // emit bridge://pairing-closed
    fn logins_for_url(&self, url: &str) -> Result<Vec<ItemSummary>, BridgeError>;
    fn search(&self, query: &str) -> Result<Vec<ItemSummary>, BridgeError>;     // capped to 50 by the dispatcher
    fn get_login(&self, item_id: &str) -> Result<LoginSecret, BridgeError>;
    fn get_totp(&self, item_id: &str) -> Result<TotpCode, BridgeError>;
    fn generate_password(&self, options: GeneratorOptions) -> Result<String, BridgeError>;
    fn save_login(&self, name: &str, url: &str, username: &str, password: &str) -> Result<String, BridgeError>;
    fn update_password(&self, item_id: &str, password: &str) -> Result<String, BridgeError>;
    fn copy_secret(&self, text: &str) -> Result<(), BridgeError> { /* default: core clipboard::copy_secret, clipboardClearSeconds from settings.json */ }
    fn on_activity(&self) {}
}
impl From<keystead_core::Error> for BridgeError;  // WrongPassword/NotFound keep meaning, InvalidInput → invalid_request, rest → internal
let dispatcher = Arc::new(Dispatcher::new(backend, ClientStore::open_default()?));
let server: ServerHandle = start_server(dispatcher.clone())?;  // Err(Error::AlreadyRunning(_)) if another instance serves
// BridgeHandler { fn handle(&self, Request) -> Response; fn handle_for_peer(&self, Request, peer_gone: &dyn Fn() -> bool) -> Response }
server.is_running(); server.stop();                            // Drop stops too
dispatcher.respond_pairing(&request_id, approve) -> bool;      // false: unknown/expired
dispatcher.pending_pairings() -> Vec<PairingRequest>;          // { requestId, clientName, code }
dispatcher.clients() -> Vec<PairedClient>; dispatcher.revoke(&client_id) -> Result<bool>;
host::is_host_invocation(std::env::args_os().skip(1)) -> bool; host::run() -> i32;  // native host mode
register::browsers() -> Vec<BrowserInfo>; register::register(&[BrowserId], exe: &Path) -> Result<()>;
register::unregister(&[BrowserId]) -> Result<()>; register::needs_reregister(exe: &Path) -> bool;
register::registered_browsers() -> Vec<BrowserId>;            // re-register these when needs_reregister()
// keystead_bridge::Error { Io, FrameTooLarge, Json, AlreadyRunning, Core }, Error::code() → "io:…"/"corrupt:…"
```

## UI/UX principles (desktop + extension)

* Look & feel of a modern password manager (Bitwarden/1Password-like):
  left sidebar (Alle Elemente, Favoriten, Typen, Ordner, Papierkorb,
  Werkzeuge: Generator, Sicherheitsbericht), middle item list with search,
  right detail pane. Edit inline in the detail pane.
* Calm, clean design: system font stack (Segoe UI Variable / Inter
  fallback), 8-px spacing grid, 1 accent colour (blue `#3b82f6`-ish), soft
  borders, no gradients, light + dark theme via CSS variables, follows
  system theme by default.
* Branding: the Keystead mark (white shield with keyhole on a blue→violet
  rounded tile, `assets/keystead.svg`) is the only gradient. Desktop:
  `src/components/Logo.tsx` (`variant="glyph"` = shield in `currentColor`);
  extension: `logo()` in `lib/icons.js` and `content.js`. Inline SVGs use
  per-instance gradient ids (a reference into a hidden SVG does not render).
* Everything important is one click away: copy buttons next to every field,
  "eye" toggle for secrets, a generate button in every password field,
  keyboard shortcuts (Ctrl+F search, Ctrl+N new, Ctrl+L lock,
  Ctrl+Shift+C on a selected item copies its password, Ctrl+B the username,
  Ctrl+S saves while editing, Esc cancels editing / closes dialogs).
* German first, English second (`src/i18n/de.ts`, `src/i18n/en.ts`).
* No hidden options: settings is one page with clear sections.
