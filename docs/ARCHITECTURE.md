# VaultX 2 – Architecture & Contracts

VaultX 2 is a local-only password manager in the spirit of Bitwarden:
a desktop app (Tauri 2: Rust backend + React UI), a terminal UI, and a
Chromium browser extension that talks to the desktop app via Native
Messaging. **Nothing ever leaves the machine** – there is no server and no
cloud sync.

This document is the binding contract between the components. If you change
an interface here, change every side.

```
┌────────────────────┐  native messaging   ┌──────────────────┐  local socket   ┌──────────────────────────┐
│ Chrome/Edge/Brave  │ ─── stdin/stdout ──▶ │ VaultX.exe       │ ── named pipe ─▶│ VaultX.exe (desktop app, │
│ extension (MV3)    │ ◀── length-prefixed ─│ (native host mode)│ ◀─ len-prefix ─ │ tray, holds unlocked     │
└────────────────────┘     JSON             └──────────────────┘    JSON         │ vault in memory)         │
                                                                                 └──────────┬───────────────┘
                                                                                            │ vaultx-core
                                                 ┌──────────────────────┐                   ▼
                                                 │ vaultx-cli / --cli   │──── vaultx-core ──▶ encrypted *.vaultx files
                                                 │ (ratatui TUI)        │
                                                 └──────────────────────┘
```

## Repository layout

| Path | What |
|------|------|
| `Cargo.toml` | Cargo workspace |
| `crates/vaultx-core` | Crypto, vault file format, storage, model, generator, TOTP, import/export, URL matching, health report. No UI, no IPC. |
| `crates/vaultx-bridge` | Browser bridge: protocol types, framing, local socket server (used by the app) & client, native-messaging host runner, host-manifest registration for Chrome/Edge/Brave/Chromium. |
| `crates/vaultx-tui` | Terminal UI (ratatui + crossterm). Library `vaultx_tui::run()` + binary `vaultx-cli`. |
| `apps/desktop` | Tauri 2 app. `src/` = React + TypeScript + Vite frontend, `src-tauri/` = Rust backend (binary name `VaultX`). |
| `extension/chrome` | Manifest V3 extension, plain JavaScript (no build step, load unpacked). |
| `legacy/` | The old PowerShell VaultX 1.x, kept for reference. |
| `docs/` | This file + user docs. |

## Executable modes (`apps/desktop/src-tauri/src/main.rs`)

One portable `VaultX.exe` (Windows GUI subsystem in release) does everything:

1. **Native host mode** – if any CLI argument starts with `chrome-extension://`
   (Chrome passes the caller origin as first argument; on Windows it may also
   pass `--parent-window=N`), run `vaultx_bridge::host::run()` and exit.
   Never initialise Tauri in this mode, never print anything to stdout except
   protocol frames.
2. **Terminal mode** – `--cli` (or `cli` as first arg): on Windows call
   `FreeConsole()` + `AllocConsole()` so the TUI gets its own new console
   window, then `vaultx_tui::run()`, exit. Additionally the console-subsystem
   binary `vaultx-cli(.exe)` from `crates/vaultx-tui` runs the same TUI inside
   an existing terminal.
3. **GUI mode** – everything else. `--background` starts hidden in the tray
   (used when the native host has to launch the app). Uses
   `tauri-plugin-single-instance`: a second GUI launch focuses the first
   window (and a second `--background` launch does nothing).

### Terminal UI & command line (`vaultx-tui`)
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
  re-checked every 5 s (`reload_if_changed`), `Conflict` → reload + retry once.
  `--vault NAME|ID` preselects a vault. Creating a vault is possible when none
  exists (`n` in the picker).
* Subcommands (all accept `--vault NAME|ID`; default: the only vault, else
  `lastVaultId`): `vaults`, `list`, `get <id|name|search terms>
  [--field password|username|totp|notes|uri] [--copy]`, `generate [--length N |
  --passphrase] [--words N] [--no-symbols] [--copy]`. `--copy` keeps the
  process alive until the clipboard is cleared (any key clears at once).
* Master password: prompted on the TTY (`rpassword`), or – insecure, for
  scripts – `$VAULTX_MASTER_PASSWORD`.
* Language from `Settings.language` (German default, English for `en`).

## Data locations (`vaultx_core::paths`)

* `data_dir()`:
  1. `$VAULTX_DATA_DIR` if set.
  2. **Portable mode**: a folder named `VaultX-Data` next to the running
     executable, if it exists (the user creates it, or Settings → "Portabler
     Modus" creates it and moves the vaults).
  3. Otherwise the OS local data dir: Windows `%LOCALAPPDATA%\VaultX\v2`,
     Linux `~/.local/share/vaultx`, macOS `~/Library/Application Support/VaultX`.
* Vault files: `<data_dir>/vaults/<vault-id>.vaultx` (+ `<vault-id>.vaultx.bak`
  = previous good version, written before each save; `<vault-id>.vaultx.lock`
  = empty inter-process lock file held only while a save runs).
* Settings: `<data_dir>/settings.json` (non-secret, see `Settings` below).
* Paired browser clients: `<data_dir>/bridge-clients.json` (stores only
  SHA-256 hashes of client tokens).
* Legacy VaultX 1.x vaults: `%LOCALAPPDATA%\VaultX\accounts.json` +
  `vault_*.json` (Linux/macOS: none – only manual file import).
  `$VAULTX_LEGACY_DIR` overrides this directory on every OS (tests).

## Vault file format (version 3) – `vaultx_core::format`

UTF-8 JSON, written atomically (write `*.tmp` in same dir → fsync → rename).

```json
{
  "format": "vaultx",
  "version": 3,
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
  `"vaultx:v3:payload:" + id + ":" + revision`.
* The master password → Argon2id (params above, salt 16 B) → 32-byte KEK →
  wraps the vault key (XChaCha20-Poly1305, AAD `"vaultx:v3:key:" + id`).
  Changing the master password re-wraps only the key.
* Optional **recovery key**: random 25 chars of Crockford base32 shown as
  `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX` (125 bit). KEK_r = Argon2id(recovery code
  normalised to uppercase without dashes, own salt, same params) wraps the
  vault key (AAD `"vaultx:v3:recovery:" + id`).
* `revision` increments on every save; saving checks the on-disk revision
  equals the loaded one, otherwise `Error::Conflict` (another process – TUI or
  app – changed it). Callers reload and retry.
* Base64 = standard alphabet with padding. Secrets in memory use
  `zeroize`/`secrecy` where practical.
* Argon2id default params: m = 64 MiB, t = 3, p = 4. Tests may use a cheaper
  `KdfParams::insecure_for_tests()`.

## Core API – `vaultx-core` (Rust)

```rust
pub mod error;    // pub enum Error { Io, Json, WrongPassword, Corrupt(String), Conflict, NotFound(String), InvalidInput(String), Unsupported(String) }  pub type Result<T>
pub mod model;    // see crates/vaultx-core/src/model.rs (the data contract)
pub mod crypto;   // KdfParams, derive_key, seal/open (XChaCha20-Poly1305), random bytes
pub mod format;   // VaultFile (serde of the JSON above), read/write atomic
pub mod paths;    // data_dir(), vaults_dir(), settings_path(), legacy_dir(), is_portable()
pub mod store;    // VaultStore
pub mod vault;    // UnlockedVault
pub mod generator;// GeneratorOptions, generate()
pub mod totp;     // parse + code generation
pub mod matching; // URL matching for autofill
pub mod health;   // HealthReport, strength()
pub mod import;   // legacy VaultX 1.x, CSV (Chrome/Edge/Firefox/Bitwarden/generic), Bitwarden JSON, VaultX export
pub mod export;   // encrypted .vaultx export, CSV, Bitwarden-compatible JSON
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
    pub fn unlock_with_recovery_key(&self, vault_id: &str, recovery_key: &str, new_master_password: &str) -> Result<UnlockedVault>; // also re-wraps with the new master pw
    pub fn delete_vault(&self, vault_id: &str, master_password: &str) -> Result<()>;      // verifies pw first, removes file + .bak
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
    pub fn import_items(&mut self, items: Vec<VaultItem>, folders: Vec<Folder>) -> Result<usize>; // assigns fresh ids, maps folder ids, persists once
    pub fn rename(&mut self, name: &str) -> Result<()>;
    pub fn verify_master_password(&self, password: &str) -> bool;
    pub fn change_master_password(&mut self, current: &str, new: &str) -> Result<()>;
    pub fn create_recovery_key(&mut self) -> Result<String>;   // returns formatted code, replaces an existing one
    pub fn remove_recovery_key(&mut self) -> Result<()>;
    pub fn has_recovery_key(&self) -> bool;
    pub fn save(&mut self) -> Result<()>;                      // atomic write, revision check, .bak
    pub fn reload_if_changed(&mut self) -> Result<bool>;       // re-reads file if revision on disk differs (uses in-memory key)
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
// import
pub struct ImportReport { pub imported: usize, pub skipped: usize, pub warnings: Vec<String> }  // camelCase
pub struct LegacyVaultInfo { pub name: String, pub path: String } // camelCase
pub fn legacy_scan() -> Vec<LegacyVaultInfo>;                       // reads %LOCALAPPDATA%\VaultX\accounts.json + vault_*.json
pub fn import_legacy_file(path: &Path, password: &str) -> Result<(Vec<VaultItem>, Vec<String> /*warnings*/)>; // VaultX 1.x format, master OR recovery password
pub fn import_csv(text: &str) -> Result<(Vec<VaultItem>, Vec<Folder>, Vec<String>)>; // auto-detects Chrome/Edge/Firefox/Bitwarden/legacy-VaultX/generic headers, ',' or ';'
pub fn import_bitwarden_json(text: &str) -> Result<(Vec<VaultItem>, Vec<Folder>, Vec<String>)>; // unencrypted Bitwarden export
pub fn import_vaultx_export(path: &Path, password: &str) -> Result<(Vec<VaultItem>, Vec<Folder>)>;
// export
pub fn export_encrypted(data: &VaultData, path: &Path, password: &str) -> Result<()>; // standalone v3 file (own random key/salt), trash excluded
pub fn export_csv(data: &VaultData) -> String;            // Bitwarden-style CSV (logins + notes), trash excluded
pub fn export_bitwarden_json(data: &VaultData) -> String; // unencrypted Bitwarden JSON, trash excluded
```

### Core additions & behaviour details (implemented in `vaultx-core`)
Additive helpers beyond the signatures above (all optional to use):
```rust
// error
impl Error { pub fn code(&self) -> String }  // stable UI code; Json → "corrupt:<d>", NotFound(_) → "not_found"
// paths (all infallible)
pub fn data_dir() -> PathBuf; pub fn vaults_dir() -> PathBuf; pub fn settings_path() -> PathBuf;
pub fn legacy_dir() -> Option<PathBuf>; pub fn is_portable() -> bool;
pub fn portable_dir() -> Option<PathBuf>;   // "<exe dir>/VaultX-Data", whether it exists or not
pub fn default_data_dir() -> PathBuf;       // OS location, ignoring portable mode and $VAULTX_DATA_DIR
// crypto
impl KdfParams { pub fn validate(&self) -> Result<()> }  // file DoS guard: memory ≤ 1 GiB, t ≤ 20, p ≤ 16
// store / vault
impl VaultStore { pub fn vaults_dir(&self) -> PathBuf }
impl UnlockedVault { pub fn id(&self) -> &str; pub fn name(&self) -> &str; pub fn revision(&self) -> u64;
                     pub fn folders(&self) -> &[Folder]; pub fn generator_history(&self) -> &[GeneratedPassword] }
// import / export: one call per Tauri `import_data` / `export_data` format string
pub fn import::import_into(vault: &mut UnlockedVault, format: &str /*legacy|csv|bitwarden_json|vaultx*/,
                           path: &Path, password: Option<&str>) -> Result<ImportReport>; // reads UTF-8/UTF-16/Windows-1252 text
pub fn import::legacy_scan_dir(dir: &Path) -> Vec<LegacyVaultInfo>;
pub fn export::export_to_file(data: &VaultData, format: &str /*vaultx|csv|bitwarden_json*/,
                              path: &Path, password: Option<&str>) -> Result<()>;
pub fn export::export_encrypted_with_params(data, path, password, kdf: KdfParams) -> Result<()>;
// settings
impl Settings { pub fn load() -> Settings; pub fn load_from(&Path) -> Settings; pub fn save(&self) -> Result<()>;
                pub fn save_to(&self, &Path) -> Result<()> }   // + free fns settings::load(), settings::save(&Settings)
// clipboard
pub fn copy_secret(text: &str, clear_after: Option<Duration>) -> Result<()>; // excluded from clipboard history
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
* `invalid_input` details used by the core: `name_required`, `name_too_long`,
  `password_empty`, `password_required`, `kdf_params`, `recovery_key_format`,
  `length`, `words`, `separator`, `no_character_set`,
  `minimums_exceed_length`, `totp_secret`, `totp_secret_empty`, `totp_uri`,
  `totp_digits`, `totp_period`, `csv_empty`, `csv_unknown_columns`,
  `bitwarden_json`, `json: …`, `csv: …`, `format <x>`.
* Strength texts (`crackTime`, `warning`, `suggestions`) and import warnings
  are English.

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

## Settings (`vaultx_core::settings::Settings`, `<data_dir>/settings.json`)

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
`corrupt:<detail>`, `unsupported:<detail>`. Argument names are camelCase on
the JS side (Tauri converts to snake_case).

| Command | Args | Returns |
|---|---|---|
| `app_info` | – | `AppInfo { version, dataDir, portable, platform: "windows"\|"linux"\|"macos", extensionId }` |
| `list_vaults` | – | `VaultInfo[]` |
| `session_state` | – | `SessionState { unlocked: boolean, vault: VaultInfo \| null }` |
| `create_vault` | `name, masterPassword` | `VaultInfo` (and unlocks it) |
| `unlock_vault` | `vaultId, masterPassword` | `VaultInfo` |
| `unlock_with_recovery` | `vaultId, recoveryKey, newMasterPassword` | `VaultInfo` |
| `lock_vault` | – | `null` |
| `touch_activity` | – | `null` (resets auto-lock timer; UI calls it throttled to ≤1/30 s on input) |
| `list_items` | – | `VaultItem[]` (all, incl. trash; UI filters) |
| `list_folders` | – | `Folder[]` |
| `save_item` | `item: VaultItem` | `VaultItem` |
| `trash_item` / `restore_item` / `delete_item` | `id` | `null` |
| `empty_trash` | – | `number` |
| `save_folder` | `folder: Folder` | `Folder` |
| `delete_folder` | `id` | `null` |
| `generate_password` | `options: GeneratorOptions, remember: boolean` | `string` (adds to history if remember) |
| `generator_history` | – | `GeneratedPassword[]` |
| `clear_generator_history` | – | `null` |
| `password_strength` | `password` | `Strength` |
| `totp_code` | `seed` | `TotpCode` |
| `copy_text` | `text, sensitive: boolean` | `null` (sensitive → cleared after `clipboardClearSeconds`, only if clipboard still holds it) |
| `health_report` | – | `HealthReport` |
| `change_master_password` | `current, newPassword` | `null` |
| `create_recovery_key` | – | `string` |
| `remove_recovery_key` | – | `null` |
| `rename_vault` | `name` | `VaultInfo` |
| `delete_vault` | `vaultId, masterPassword` | `null` (locks if it was the open vault) |
| `legacy_scan` | – | `LegacyVaultInfo[]` |
| `import_data` | `format: "legacy"\|"csv"\|"bitwarden_json"\|"vaultx", path, password: string \| null` | `ImportReport` (into the unlocked vault) |
| `export_data` | `format: "vaultx"\|"csv"\|"bitwarden_json", path, password: string \| null, masterPassword` | `null` (re-verifies master pw) |
| `get_settings` | – | `Settings` |
| `save_settings` | `settings: Settings` | `Settings` |
| `browser_status` | – | `BrowserStatus { serverRunning, extensionId, browsers: BrowserInfo[], clients: PairedClient[] }` |
| `register_browsers` | `browsers: string[]` (ids) | `BrowserStatus` |
| `unregister_browsers` | `browsers: string[]` | `BrowserStatus` |
| `revoke_client` | `clientId` | `BrowserStatus` |
| `respond_pairing` | `requestId, approve: boolean` | `null` |
| `open_terminal` | – | `null` (spawns `VaultX --cli` in a new console) |
| `open_data_dir` | – | `null` |
| `set_portable_mode` | `enabled: boolean` | `AppInfo` (moves vault files; requires locked or re-unlock) |

`BrowserInfo { id: "chrome"|"edge"|"brave"|"chromium"|"vivaldi", name, detected: boolean, registered: boolean }`,
`PairedClient { id, name, createdAt, lastSeenAt }`.

**Events** (backend → frontend, `app.emit`):
* `vault://locked` – payload `{ reason: "manual"|"timeout"|"system" }`
* `vault://changed` – payload `{}` – items changed outside the UI (extension saved a login, file changed by TUI → reloaded). UI re-fetches.
* `bridge://pairing-request` – payload `{ requestId, clientName, code }` – UI shows a modal with the 6-digit code; user approves/denies → `respond_pairing`.
* `bridge://unlock-request` – payload `{}` – extension asked to open the app; UI focuses the unlock screen.
* `vault://unlocked` – payload `VaultInfo` – the vault was unlocked outside
  the UI (extension `unlock`); the UI switches to the unlocked view. Not
  sent for the UI's own `unlock_vault`/`create_vault`/`unlock_with_recovery`.

The frontend's `src/lib/api.ts` wraps every command with typed functions and
falls back to an in-memory **mock backend** (`src/lib/mock.ts`, realistic
sample data, password `demo`) when not running inside Tauri
(`!("__TAURI_INTERNALS__" in window)`) – used for `npm run dev` in a browser
and for automated screenshots. Mock-only extras: `?mock=empty` starts without
any vault (first-run screen), `?lang=en` starts in English, and
`window.__vaultxMock` offers `triggerPairing(name?)`, `simulateTimeoutLock()`,
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
* Locking (any reason) also clears a secret still pending in the clipboard;
  so does quitting the app. `lock_vault` (the UI's own request) emits no event;
  tray "Sperren" and the extension's `lock` emit `vault://locked {manual}`.
* Every unlock/create stores the vault as `lastVaultId`. The extension's
  `unlock` opens `lastVaultId` if it exists, else the only vault (else
  `not_found`).
* `import_data` emits `vault://changed` after a successful import.
* `delete_vault` / `respond_pairing` / `revoke_client`: unknown ids →
  `not_found`.
* `open_terminal`: Windows `CREATE_NEW_CONSOLE`; Linux tries `$TERMINAL`,
  `x-terminal-emulator`, `gnome-terminal`, `konsole`, `xfce4-terminal`,
  `kitty`, `alacritty`, `foot`, `xterm` (none → `unsupported:no_terminal`);
  macOS Terminal via `osascript`.
* `set_portable_mode` locks an open vault itself (no event; the UI checks
  `session_state` afterwards), stops the bridge, moves the whole data
  directory (copy, commit by creating/renaming `VaultX-Data`, then delete
  the old copy; `*.lock`/`*.tmp` are skipped), re-registers the native host
  and restarts the bridge. Errors: `unsupported:data_dir_override` (with
  `$VAULTX_DATA_DIR`), `invalid_input:target_exists` (a vault file with the
  same id already exists at the destination), `io:…`.
* Pairing requests that arrived before the page subscribed (app launched
  hidden by the native host) are re-sent ~1 s after the first
  `session_state` call of each page load; the UI de-duplicates by
  `requestId`.
* Debug builds only: `VAULTX_TEST_AUTO_APPROVE_PAIRING=1` approves pairing
  requests automatically (no dialog) for automated end-to-end tests.
* Window: links/`window.open` and any navigation away from the app open in
  the system browser (http/https only). Without a tray icon (Linux without
  AppIndicator) the close button quits even with `minimizeToTray`, and
  `startInTray` is ignored.

## Browser bridge protocol – `vaultx-bridge`

### Framing
Both legs (browser↔host via stdin/stdout, host↔app via local socket) use the
native-messaging framing: `u32` **little-endian** byte length + UTF-8 JSON.
Max message 1 MiB (host → browser) / 4 MiB otherwise.

### Local socket
* Windows: named pipe `\\.\pipe\vaultx-bridge-<USERNAME>` (via the
  `interprocess` crate, `GenericNamespaced`), current-user only.
* Unix: `$XDG_RUNTIME_DIR/vaultx-bridge.sock`, fallback
  `/tmp/vaultx-bridge-<uid>.sock`, mode 0600.
* The host forwards each browser message to the app and the app's reply back.
  If it cannot connect, it launches the app (`<own exe> --background`, the
  host *is* VaultX.exe) and retries for up to 8 s; if that fails it answers
  `{ "id", "ok": false, "error": "app_unavailable" }`.

### Messages (extension → app)
Every request: `{ "id": "<uuid>", "type": "<type>", "clientId"?: string, "token"?: string, ...payload }`.
Every response: `{ "id": "<same>", "ok": true, "data": ... }` or `{ "id", "ok": false, "error": "<code>" }`.
Error codes: `not_paired`, `pairing_denied`, `locked`, `wrong_password`, `not_found`, `invalid_request`, `app_unavailable`, `internal`.

| type | needs pairing | payload | data |
|---|---|---|---|
| `status` | no | – | `{ appVersion, paired: bool, unlocked: bool, vaultName: string\|null }` |
| `pair` | no | `clientName` (e.g. "Chrome – DESKTOP-1"), `code` (6 digits shown in the popup) | `{ clientId, token }` once the user approved in the app (the request blocks up to 120 s) |
| `unlock` | yes | `password` | `{ vaultName }` |
| `lock` | yes | – | `null` |
| `focus_app` | no | – | `null` (shows/raises the window) |
| `logins_for_url` | yes, unlocked | `url` | `ItemSummary[]` |
| `search` | yes, unlocked | `query` | `ItemSummary[]` (max 50) |
| `get_login` | yes, unlocked | `itemId` | `{ id, name, username, password, totp: TotpCode\|null, uris: string[] }` |
| `get_totp` | yes, unlocked | `itemId` | `TotpCode` |
| `generate_password` | yes | `options?` (GeneratorOptions partial) | `string` (uses defaults + stored in generator history if unlocked) |
| `save_login` | yes, unlocked | `name, url, username, password` | `{ id }` (creates new login) |
| `update_password` | yes, unlocked | `itemId, password` | `{ id }` |

Pairing flow: the extension generates a random 6-digit `code`, displays it in
its popup, and sends `pair { clientName, code }`. The app emits
`bridge://pairing-request` with that code; the user compares the codes and
approves. The app stores `{ id, name, tokenSha256, createdAt, lastSeenAt }`
and returns `{ clientId, token }` (token = 32 random bytes, base64url). The
extension stores both in `chrome.storage.local`.

### Native host registration
Host name: **`com.vaultx.bridge`**. Manifest file `com.vaultx.bridge.json`
(written to `<data_dir>/native-host/`) with `"path"` = absolute path of the
current exe, `"type": "stdio"`, `"allowed_origins": ["chrome-extension://imfndemblnaalppnmdplagajjielnaok/"]`.
* Windows: registry `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.vaultx.bridge`,
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

### Bridge behaviour details & Rust API (implemented in `vaultx-bridge`)
Behaviour (additive to the table above):
* Requests on one connection are answered in order and the host keeps a
  single connection, so a pending `pair` (≤ 120 s) delays later messages on
  the same native-messaging port.
* Credentials are checked before the lock state (`not_paired` wins over
  `locked`). Unknown fields are ignored; missing/mistyped fields, an unknown
  `type` or a non-object → `invalid_request` (with the request's `id` if it
  is a string, else `""`).
* `status`: `vaultName` is only revealed to paired clients (else `null`);
  invalid credentials just give `paired: false`.
* `pair`: `clientName` trimmed, 1–100 chars; `code` exactly 6 ASCII digits,
  else `invalid_request` (the app is not asked). A new `pair` with the same
  `clientName` supersedes a pending one (the older gets `pairing_denied`); at
  most 4 pending requests; deny/timeout → `pairing_denied`.
* `unlock`: after 5 wrong passwords in a row it answers `wrong_password` for
  30 s without trying; every further failure restarts the 30 s, a success
  resets the counter. Attempts are serialised.
* `VaultBackend::on_activity` runs after successful *user actions* only
  (`unlock`, `search`, `get_login`, `get_totp`, `generate_password`,
  `save_login`, `update_password`) – `status` polling and the automatic
  `logins_for_url` must not keep the vault from auto-locking.
* `bridge-clients.json` = `{ "version": 1, "clients": [ { id, name,
  tokenSha256, createdAt, lastSeenAt } ] }`; `tokenSha256` = lowercase hex
  SHA-256 of the UTF-8 token string; atomic writes (0600 on Unix);
  `lastSeenAt` is persisted at most once a minute. A corrupt file is moved to
  `bridge-clients.json.corrupt` (browsers must pair again).
* Server: never displaces a live server (second instance → `AlreadyRunning`);
  removes stale Unix socket files; both ends check the peer uid on Unix.
  Windows: pipe DACL = current user's SID only (best effort, falls back to
  default pipe security), remote clients rejected. After `stop()` open
  connections are closed unanswered on their next request, so the host
  reconnects to a restarted server (or launches the app).
* Host: oversized browser frame → `invalid_request` and exit; app reply
  > 1 MiB → `internal`; after a failed launch, requests within 30 s fail fast
  with `app_unavailable`. `$VAULTX_BRIDGE_SOCKET` overrides the endpoint
  (Unix: socket path, Windows: pipe name), `$VAULTX_APP_EXE` the executable
  the host launches.

```rust
pub trait VaultBackend: Send + Sync + 'static {   // implemented by the desktop app
    fn app_version(&self) -> String;
    fn unlocked_vault_name(&self) -> Option<String>;
    fn unlock(&self, password: &str) -> Result<String, BridgeError>;            // → vault name
    fn lock(&self);
    fn focus_app(&self);
    fn request_pairing(&self, request_id: &str, client_name: &str, code: &str); // emit bridge://pairing-request, don't block
    fn logins_for_url(&self, url: &str) -> Result<Vec<ItemSummary>, BridgeError>;
    fn search(&self, query: &str) -> Result<Vec<ItemSummary>, BridgeError>;     // capped to 50 by the dispatcher
    fn get_login(&self, item_id: &str) -> Result<LoginSecret, BridgeError>;
    fn get_totp(&self, item_id: &str) -> Result<TotpCode, BridgeError>;
    fn generate_password(&self, options: GeneratorOptions) -> Result<String, BridgeError>;
    fn save_login(&self, name: &str, url: &str, username: &str, password: &str) -> Result<String, BridgeError>;
    fn update_password(&self, item_id: &str, password: &str) -> Result<String, BridgeError>;
    fn on_activity(&self) {}
}
impl From<vaultx_core::Error> for BridgeError;  // WrongPassword/NotFound keep meaning, InvalidInput → invalid_request, rest → internal
let dispatcher = Arc::new(Dispatcher::new(backend, ClientStore::open_default()?));
let server: ServerHandle = start_server(dispatcher.clone())?;  // Err(Error::AlreadyRunning(_)) if another instance serves
server.is_running(); server.stop();                            // Drop stops too
dispatcher.respond_pairing(&request_id, approve) -> bool;      // false: unknown/expired
dispatcher.pending_pairings() -> Vec<PairingRequest>;          // { requestId, clientName, code }
dispatcher.clients() -> Vec<PairedClient>; dispatcher.revoke(&client_id) -> Result<bool>;
host::is_host_invocation(std::env::args_os().skip(1)) -> bool; host::run() -> i32;  // native host mode
register::browsers() -> Vec<BrowserInfo>; register::register(&[BrowserId], exe: &Path) -> Result<()>;
register::unregister(&[BrowserId]) -> Result<()>; register::needs_reregister(exe: &Path) -> bool;
register::registered_browsers() -> Vec<BrowserId>;            // re-register these when needs_reregister()
// vaultx_bridge::Error { Io, FrameTooLarge, Json, AlreadyRunning, Core }, Error::code() → "io:…"/"corrupt:…"
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
* Everything important is one click away: copy buttons next to every field,
  "eye" toggle for secrets, a generate button in every password field,
  keyboard shortcuts (Ctrl+F search, Ctrl+N new, Ctrl+L lock,
  Ctrl+Shift+C on a selected item copies its password, Ctrl+B the username,
  Ctrl+S saves while editing, Esc cancels editing / closes dialogs).
* German first, English second (`src/i18n/de.ts`, `src/i18n/en.ts`).
* No hidden options: settings is one page with clear sections.
