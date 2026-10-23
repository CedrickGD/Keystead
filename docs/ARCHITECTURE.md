# Keystead – Architecture & Contracts

Keystead (*Key* + *Homestead* – your keys stay at home) is a local-only
password manager in the spirit of Bitwarden:
a desktop app (Tauri 2: Rust backend + React UI), a terminal UI, and a
Chromium browser extension that talks to the desktop app via Native
Messaging. **No Keystead server, no account, no cloud service**: the vaults
stay encrypted on the user's machine; only if the user turns on "Kopie im
Cloud-Ordner aktuell halten" does the desktop app put encrypted copies into
a local folder of their choice, which their own cloud client (Nextcloud,
OneDrive) uploads – for the phone (see "Cloud-folder mirror (desktop) and
phone link"). The app itself only goes online for the update check, (unless
turned off) to load website icons directly from the websites (see "Website
icons"), and – only when the user presses the button, never on its own – to
check passwords against Have I Been Pwned, where just the first 5 hex
characters of a password's SHA-1 leave the device (see "Leaked-password
check").
The Android app (read-only in its first stages, editing since stage 2b – see "Mobile (Android)") has
no network permission at all: it reads encrypted vault copies from a
folder of the user's own cloud that the user links through the system
picker – the cloud's own app (Nextcloud, OneDrive) does the transfer, and
the provider sees the encrypted file with its plain-text header (vault
name, revision, times), never its content.

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
| `crates/keystead-core` | Crypto, vault file format, storage, model, generator, TOTP, import/export, URL matching, health report, the offline half of the leaked-password check (hashing, answer parsing), the header-only half of the cloud-folder sync (`sync`: copy-in decision, file-name rule, size limit). No UI, no IPC, no networking. |
| `crates/keystead-bridge` | Browser bridge: protocol types, framing, local socket server (used by the app) & client, native-messaging host runner, host-manifest registration for Chrome/Edge/Brave/Chromium. |
| `crates/keystead-tui` | Terminal UI (ratatui + crossterm). Library `keystead_tui::run()` + binary `keystead-cli`. |
| `apps/desktop` | Tauri 2 app. `src/` = React + TypeScript + Vite frontend, `src-tauri/` = Rust backend: the library `keystead_lib` (`src/lib.rs`, the whole app – also the phone's entry point) and the desktop binary `Keystead` (`src/main.rs`). `src-tauri/tauri.android.conf.json` = the Android overrides of `tauri.conf.json` (merged by Tauri). |
| `apps/desktop/src-tauri/gen/android` | The Android Studio project from `npx tauri android init` (Gradle, manifest, `MainActivity.kt`), **committed** with its hardening and the app's own folder plugin `KeysteadFolderPlugin.kt` – see "Mobile (Android)". Build output, `keystore.properties`, `tauri.properties` and the files the CLI regenerates are gitignored. |
| `extension/chrome` | Manifest V3 extension, plain JavaScript (no build step, load unpacked). Embedded into the app at build time (`src-tauri/build.rs`). |
| `apps/desktop/tests` | `node --test` tests of the UI's dependency-free modules (`src/lib/layout.mjs`: phone-layout decision and back-gesture order). |
| `extension/tests` | `node --test` tests of the extension's pure helpers (`lib/version.js`, `lib/pending.js`, `lib/settings.js`, `lib/generator.js`), of the locales (`_locales/de` and `en` have the same keys and placeholders), of the form detection `lib/forms.js` on a small dependency-free fake DOM (`helpers/mini-dom.mjs`) and of the service worker's generator traffic – the real `background.js` on a fake `chrome` (`helpers/fake-chrome.mjs`). |
| `.github/workflows/build.yml`, `.github/scripts/` | CI: tests, Windows build, Android build (APK signing and gates: `android-apk.mjs`), versioning, updater signing/manifest, releases (see "Releases & in-app updates", "Mobile (Android)"). |
| `assets/` | Brand assets: `keystead.svg` (app icon), `keystead-glyph.svg` (single-colour shield, `currentColor`), `keystead-1024.png` (source for `npx tauri icon` and the extension icons), `Keystead.ico`. |
| `docs/` | This file + user docs. |

## Executable modes (`apps/desktop/src-tauri/src/main.rs`, `lib.rs`)

The app crate is one library plus one binary (`Cargo.toml`: `[lib] name =
"keystead_lib"`, `crate-type = ["staticlib", "cdylib", "rlib"]`, and `[[bin]]
Keystead`). The library holds the whole app; `#[cfg(desktop)]` /
`#[cfg(mobile)]` (the aliases `tauri_build` sets from the target OS) mark
what only one side has. Desktop-only: the browser bridge and extension, the
terminal UI, tray, single instance, in-app updates, portable mode, the
website-icon fetcher (`icons/fetch.rs`), the leaked-password check, the
cloud-folder mirror (`mirror.rs`), the file-dialog based attachment, import
and export commands (the attachment
image preview, `reveal_attachment`, is on both), Windows session lock
detection and the suspend (clock-jump) detection. Phone-only
(`#[cfg(mobile)]`, Android and iOS): `src/mobile.rs` – the data directory
override, the clipboard backend on `tauri-plugin-clipboard-manager` and
links through `tauri-plugin-opener` (both plugins are called from Rust only,
the webview gets no permission for them, and the opener's injected click
hook is off) –, `src/mobile/sync.rs` (the cloud-folder sync: the binding of
the Kotlin plugin `KeysteadFolderPlugin.kt` in `gen/android`, its commands,
the automatic sync and the push after an edit) and `src/mobile/biometric.rs` (fingerprint unlock on
`tauri-plugin-biometric`); their platform-neutral engines `folder_sync.rs`
and `device_unlock.rs` are compiled on the desktop only for their unit
tests. What only the Android build has – the plugin bindings, the biometric
plugin, the fingerprint prompt's guard `Core::begin_own_prompt` – is gated
on `target_os = "android"`. Dependencies follow the same split, in three
tables: `[target.'cfg(not(any(target_os = "android", target_os =
"ios")))'.dependencies]` (desktop), `[target.'cfg(any(target_os =
"android", target_os = "ios"))'.dependencies]` (both phones: clipboard
manager, opener) and `[target.'cfg(target_os = "android")'.dependencies]`
(`tauri-plugin-biometric`, whose keystore cipher exists on Android only); the
`tauri` crate's `tray-icon` feature exists only on the desktop. `build.rs` embeds the browser extension
on the desktop only (the phone gets an empty table). Windows detail: the
library's unit-test executable links the app's resource file (icon, version
info, manifest with the Common Controls v6 dependency the dialog plugin's
imports need) itself – `#[link]` under `cfg(test)` in `lib.rs`, the search
path comes from `build.rs` – because `tauri_build` hands it to the binaries
only and Cargo has no instruction for a library's unit tests.

* **Desktop** – the binary (below) ends in `keystead_lib::run_desktop(background)`.
* **Phone** – the Android/iOS shell loads the library and calls
  `keystead_lib::run()` (`#[tauri::mobile_entry_point]`), which builds the
  same Tauri app without the desktop-only plugins, services and commands;
  the binary has no role there (`fn main() {}`). Since stage 2b the phone
  **edits** vaults (items, folders, tags, the generator, a vault's name,
  new vaults); desktop-only stay the master password, recovery keys,
  deleting a vault, attachment changes and import/export. The phone's own
  commands are the cloud-folder sync (two-way: it stores the folder's vault
  copies and blobs, writes the phone's vault files back, keeps
  `sync/link.json`, `sync/trial/`, `sync/base/`, `sync/conflicts.json` and
  `conflicts/`) and the fingerprint unlock (it stores the device-unlock
  blobs and `Settings.deviceUnlock`) –, see "On the phone" under "Desktop
  backend ↔ frontend". Everything
  Android-specific – the committed Android
  project, its hardening, lifecycle, clipboard, build, CI and signing – is
  in "Mobile (Android)".

One portable `Keystead.exe` (Windows GUI subsystem in release) does everything:

1. **Native host mode** – if any CLI argument starts with `chrome-extension://`
   (Chrome passes the caller origin as first argument; on Windows it may also
   pass `--parent-window=N`), run `keystead_bridge::host::run()` and exit.
   Never initialise Tauri in this mode, never print anything to stdout except
   protocol frames.
2. **Terminal mode** – `--cli` (or `cli` as first arg): on Windows call
   `FreeConsole()` + `AllocConsole()` so the TUI gets its own new console
   window, then `keystead_tui::run()`, exit. Windows also: the process holds
   the named mutex `Local\Keystead-TerminalUI` while it runs (the update
   refuses to install meanwhile, see "In-app updates"), and a console
   control handler (`SetConsoleCtrlHandler`: Ctrl+C/Ctrl+Break as a signal,
   closing the console window, logoff, shutdown) clears a secret the TUI
   copied if the clipboard still holds it – its timed clear runs in-process
   and would die with it – before the default handler ends the process. Additionally the console-subsystem
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
  unlock updates), master-password prompt, item list + details (the details
  list an item's tags under its type line and, below them, the number of
  its attachments – `Anhänge: 2`, only when it has any; both read-only, the
  TUI neither adds nor reads attachments; the search matches tags like the
  desktop); copy
  (`u`/`p`/`t`, secrets via `clipboard::copy_secret` with
  `clipboardClearSeconds`, pending secret cleared on lock/quit), `o` open
  http(s) website, `n`/`e` edit (the editor keeps what it does not show:
  folder, tags, further websites, custom fields), `d` trash, `g` generator
  (not reworked with the desktop generator: it starts from the
  `GeneratorOptions` defaults, not the default preset, and offers only
  kind, length 4–128, the four groups, look-alikes, words, separator
  (`-`, space, `.`, `_`), capitalisation and number – English list; a
  value appears on opening and after every option change, shown only in
  the terminal; only `c` (copy) adds it to the vault's generator history,
  via `add_generated_password` without the desktop's newest-entry check;
  Ctrl+G in the editor fills the password with those options and stores
  nothing), auto-lock after
  `autoLockMinutes`, file re-checked every 5 s (`reload_if_changed`),
  `Conflict` → reload + retry once;
  `KeyChanged` (master password changed elsewhere) locks the TUI, `Rollback`
  is shown as an error (the newer in-memory state stays open).
  `--vault NAME|ID` preselects a vault. Creating a vault is possible when none
  exists (`n` in the picker).
* Subcommands (all accept `--vault NAME|ID`; default: the only vault, else
  `lastVaultId`): `vaults`, `list`, `get <id|name|search terms>
  [--field password|username|totp|notes|uri] [--copy]`, `generate [--length N |
  --passphrase] [--words N] [--no-symbols] [--copy]` (the `GeneratorOptions`
  defaults with these changes, not the default preset; `--length` 5–128,
  `--words` 3–20; nothing is stored). `--copy` keeps the
  process alive until the clipboard is cleared (any key clears at once).
* Master password: prompted on the TTY (`rpassword`), or – insecure, for
  scripts – `$KEYSTEAD_MASTER_PASSWORD`.
* Language from `Settings.language` (German default, English for `en`).

## Mobile (Android)

Block 5 (design and the owner's decisions: `docs/BLOCK5-DESIGN.md`).
**Stages 1a, 1b, 2a and the phone engine and PC side of 2b – the state
described here – are an offline Android app that edits its vaults and syncs
them both ways through the user's cloud folder** (stage 2b's UI and docs
are still in progress, see `docs/BLOCK5-DESIGN.md`): the same Rust crate and the same
React UI as the desktop,
built by CI as one APK (`Keystead-<version>-android.apk`, arm64, Android 10 /
API 29 and later) that every release carries. It opens the vaults in its own
app folder – lists, searches, reveals and copies, shows TOTP codes, image
previews of attachments and the security report – and locks when it stays
in the background. The vaults reach the phone through the user's **own
cloud folder**: the PC keeps encrypted copies there, the phone links that
folder and copies them in, and writes the vault files it changed back
(never blobs; nothing in the folder is ever deleted or renamed).

| Part | Stage | In short | Contract |
|---|---|---|---|
| Read-only app | 1a (E4) | only the read commands and the phone's own were registered; since 2b the phone edits (`AppInfo.readOnly` false, `attachmentsReadOnly` true) | "Commands" below; "On the phone" under "Desktop backend ↔ frontend" |
| Background lock, clipboard | 1a (E3) | lock after `lockInBackgroundSeconds` away (default 60 s); copied secrets cleared without reading the clipboard | "Lifecycle", "Clipboard" under "Android project"; "Background lock (phone)" |
| Android project, APK | 1a (E1, E2, E8–E10) | committed `gen/android`, no permission, no backup, `FLAG_SECURE`, back gesture; signed APK and gates in CI | "Android project", "Build, CI and signing" |
| Phone layout | 1b, 2b | one column, drawer, detail stacked over the list, back gesture, desktop-only gates; since 2b the editor, a new vault, the conflict dialog and the re-link prompt | "Phone layout" |
| Fingerprint unlock | 1b (E5) | opt-in, default off; the master KEK sealed by the Android Keystore, no PIN/pattern fallback | "Fingerprint unlock (phone)" |
| Cloud-folder link and copy-in | 2a | the phone links one folder and copies vault files and attachments in – each copy on trial until it opens | "Cloud-folder sync (phone)" |
| Phone write-back, conflicts | 2b | read/write permission; the phone pushes its edits (verified by reading back), a sync base per vault tells pull, push and conflict apart; conflicts are resolved by the user | "Cloud-folder sync (phone)" |
| Desktop mirror | 2a (E7), 2b | "Kopie im Cloud-Ordner aktuell halten": the PC writes the encrypted copies into that folder; since 2b it takes the phone's edits back (scans, pull, conflicts) and offers vaults made on the phone | "Cloud-folder mirror (desktop) and phone link" |
| No network | 1a–2a (E6) | no `INTERNET` permission, no update check | "No network, and why" below |

* **No network, and why**: the APK requests **no `INTERNET` permission**,
  in every stage so far – also not from stage 2 on, as decision E6 had
  planned for a "new version" hint through a small Kotlin plugin (dropped
  by the implementation run of 2026-10-17; for the owner to confirm). Why:
  the phone needs no network for its job – the user's cloud app moves the
  files and Keystead only reads its folder through the system's document
  provider, with no Keystead server anywhere; without the permission
  Android gives the app no network socket at all, so no bug in the page or
  in a dependency can send anything over the network by itself; the
  promise is checkable – the merged manifest of every CI build is gated on
  it (`android-apk.mjs verify`), and anyone can read it with `aapt2 dump
  badging`; and the app's own network code (the website-icon fetcher, the
  leaked-password check and the update check on reqwest, whose platform
  TLS verifier would panic on Android without JNI set-up) is not built for
  the phone, so there are no TLS or proxy settings that could be wrong.
  The price: no update check – About offers "Releases auf GitHub
  öffnen" (`openExternal`: the opener plugin hands the link to the
  browser, which needs no permission of Keystead's) and the user installs
  the newer APK over the old one ("Build, CI and signing" → Updates); no
  website-icon fetcher (the icons the PC stored in the vault are shown);
  no leaked-password check. Only the debug manifest adds `INTERNET`, for
  `tauri android dev` (below).

* **Entry point and gates**: the Android shell (`MainActivity`, a
  `TauriActivity`) loads `libkeystead_lib.so` and calls `keystead_lib::run()`
  (`#[tauri::mobile_entry_point]`) – see "Executable modes" for the crate
  layout and the `cfg(desktop)`/`cfg(mobile)` split. Rust code is gated on
  `mobile`/`desktop` (so an iOS build compiles the same paths), Android
  APIs – and what only the Android build calls (the plugin bindings, the
  prompt guard; `folder_sync.rs` and `device_unlock.rs` allow dead code
  outside Android) – on `target_os = "android"`; the Kotlin side lives in
  `gen/android` (below). No CI job compiles the iOS branches yet.
* **Data location**: `mobile::prepare`, the first thing in `setup`, sets
  `paths::set_data_dir_override(app_local_data_dir())` – on Android the
  app's private internal storage `/data/user/0/io.github.cedrickgd.keystead/`
  (verified in the emulator). Inside, as on the desktop: `vaults/<id>.keystead`
  and `settings.json`, plus `sync/link.json` (the linked cloud folder,
  below), `sync/trial/` (copies on trial, below), `sync/base/` (sync
  bases, see "Data locations"), `sync/conflicts.json` (recorded conflicts),
  `sync/outgoing/` (a vault file only while it is written to the folder),
  `conflicts/` (conflict copies) and
  `device-unlock/<vault-id>.json` (fingerprint unlock, below);
  nothing of the bridge (`bridge-clients.json`), no
  `browser-extension` folder, no portable mode, no `$KEYSTEAD_DATA_DIR` (see
  "Data locations"). Other apps cannot read the folder, Android's cloud
  backup and device-to-device transfer skip it (manifest below), and
  **uninstalling the app deletes it** – a vault made on the phone that was
  never written to a linked folder (`localOnly`) is lost with it.
* **Commands**: the vault commands without the desktop-only ones (master
  password, recovery keys, `delete_vault`, attachment changes,
  import/export, the network features) and the phone's own
  (`#[cfg(mobile)]`: `folder_status`, `link_folder`, `sync_now`,
  `unlink_folder` for the cloud-folder sync; `device_unlock_status`,
  `enable_device_unlock`, `disable_device_unlock`, `unlock_with_device` for
  the fingerprint) and the conflict commands both platforms share
  (`sync_conflicts`, `resolve_sync_conflict`) are registered ("**On the phone**" under "Desktop backend ↔ frontend", where
  the table has their arguments and answers; a unit test in `gui.rs` keeps
  the lists and that paragraph in step); every other command is unknown to
  the phone's invoke handler. `app_info` answers `platform: "android"`,
  `readOnly: false`, `attachmentsReadOnly: true`, `extensionId: null`,
  `portable: false`. The phone UI hides only what stays PC-only ("Editing
  on the phone (stage 2b)" under "Phone layout").
* **Settings** (see "Settings"): the phone-only `lockInBackgroundSeconds`
  (default 60, `0` = at once, `null` = never; Settings → "Sperren im
  Hintergrund": Sofort / 1 min / 5 min / Nie, a row shown only on
  `android`/`ios`) and `deviceUnlock` (fingerprint unlock, below – changed
  only by the device-unlock commands, `save_settings` keeps the stored
  value). `autoLockMinutes` and `clipboardClearSeconds` work as on the
  desktop; the desktop-only fields (`lockOnSystemLock`, `minimizeToTray`,
  `startInTray`, `browserIntegration`, `websiteIcons`, `updateCheck`,
  `updateChannel`, `mirrorFolder`) are kept in the file but do nothing on
  the phone.
* **Error codes** of the phone's own commands (stable codes the UI
  translates: `error.unsupported.<detail>` / `error.input.<detail>` in
  `de.ts`/`en.ts`):
  * cloud-folder sync – `unsupported:folder_access_lost` (the folder can no
    longer be read: permission revoked, cloud app removed or signed out,
    folder deleted or moved – pick it again; also kept as `lastError`),
    `unsupported:folder_not_linked` (`sync_now` without a folder),
    `unsupported:folder_picker` (the device has no folder picker),
    `unsupported:sync_file_too_large` (a file above its limit – a vault
    file 64 MiB, a blob 20 MiB + 45 bytes; per file reported as `tooLarge`),
    `unsupported:platform` (iOS: no folder plugin yet), `not_found` and
    `io:<detail>` (the plugin's other rejections); a single file's problem
    is no error but its `outcome` in `files`; `unsupported:sync_copy_rejected`
    (an unlock – password or fingerprint – found a synced copy forged or
    damaged and there was no earlier copy to go back to: removed, see
    "Copies on trial"); stage 2b's write-back and conflicts:
    `unsupported:sync_write_unverified` (a vault file written to the folder
    did not read back equal, twice – per file as `failed`),
    `unsupported:sync_name_taken` (the provider would not create
    `<id>.keystead` under that name), `unsupported:sync_not_writable` (no
    write permission – per file reported as `notWritable`),
    `unsupported:sync_local_changed` (per file: the phone saved while the
    folder's copy was about to replace its copy; the push after it looks
    again), `unsupported:sync_conflict_password` (`resolve_sync_conflict`
    with `keep: "local"` for a folder copy with another master password),
    `unsupported:sync_conflict_changed` (either copy changed since the
    conflict was recorded – it was synced again; fetch `sync_conflicts`
    anew), `invalid_input:keep`, `invalid_input:folder_write` (the plugin
    refused a write's name or source);
  * fingerprint – `unsupported:device_unlock_unavailable` /
    `device_unlock_not_enrolled` (no prompt shown), `device_unlock_cancelled`,
    `device_unlock_failed` (finger not recognised), `device_unlock_lockout`,
    `device_unlock_busy` (another prompt open), `io:device_unlock: <detail>`
    (anything else) – these keep the setup; `device_unlock_invalid` (reset,
    the blob is removed), `device_unlock_not_set` (none set up, or the
    feature off), `locked` (`enable_device_unlock` for a vault that is not
    the page's);
  * lock reasons only the phone sends (`vault://locked`): `background`
    (the background lock) and `keyChanged` (a synced copy of the open vault
    has a new master password);
  * the desktop mirror's codes (`invalid_input:mirror_folder`,
    `invalid_input:mirror_folder_data_dir`,
    `unsupported:mirror_folder_missing`, `unsupported:mirror_other_password`,
    `unsupported:mirror_remote_older`, `unsupported:mirror_remote_gone`,
    `invalid_input:vault_exists`) are in "Cloud-folder mirror (desktop) and
    phone link".
* **Lifecycle, background lock, clipboard**: see the Android project
  below and "Background lock (phone)" under "Desktop backend behaviour
  details".
* **Cloud-folder sync (phone)** – stage 2a copy-in, two-way since stage 2b
  (`src-tauri/src/folder_sync.rs`: the platform-neutral engine, unit-tested
  on the desktop host; `src/mobile/sync.rs`: the Kotlin plugin binding, the
  commands, the automatic sync and the push; `KeysteadFolderPlugin.kt` in
  the Android project, below):
  * **The folder**: the user picks one folder in the system's folder picker
    (`ACTION_OPEN_DOCUMENT_TREE`) – the Nextcloud or OneDrive app offers its
    folders there as a `DocumentsProvider` (OneDrive's support is not
    verified: the emulator image has no Play Store). The app keeps a
    **persisted read and write permission** (`takePersistableUriPermission`;
    a provider that refuses write gives a read-only link, `writable: false`;
    `unlink_folder` gives it back, linking another folder gives the old one
    back) – no storage permission, no network. A link of stage 2a holds read
    permission only: the phone reports its edits as `notWritable` until the
    folder is picked again. The PC keeps one copy
    per vault there: `<id>.keystead` and, with attachments,
    `<id>.keystead.attachments/<attachment id>.bin` (the data folder's
    layout; never `.bak`, `.lock` or temporary files – the PC side writes
    the blobs before the vault file, see "Cloud-folder mirror (desktop) and
    phone link"). Only top-level files named
    `<id>.keystead` after the id in their own header count
    (`sync::vault_id_of_file_name`): renamed copies and a cloud app's
    conflict copies (`… (1).keystead`) are reported as skipped, hidden files
    are ignored.
  * **Copy-in** (`folder_sync::sync_folder`): every vault file is untrusted
    input – at most 64 MiB (`unsupported:sync_file_too_large`, reported as
    "too large"), copied by the plugin into `<data>/sync/incoming/` (the
    only target it accepts; emptied before and after every sync), parsed and
    validated (`VaultFile::parse`, KDF limits included) before anything is
    written; then `keystead_core::sync::decide` (pure, unit-tested) on the
    heads of the phone's copy `<data>/vaults/<id>.keystead` and the folder's
    (`sync::head_of`: revision and a digest of the encrypted parts) and the
    vault's **sync base** for this folder (`sync/base/<id>.json`: the last
    head both agreed on – recorded for every copy the phone stores or
    pushes, and for equal copies): no copy → **import** (a new vault on the
    phone); only the folder's copy changed → **update** (replaced
    atomically with `format::write_atomic`, no `.bak` – an older copy would
    still open with an old master password; only the copy on trial below,
    until the new one opened; never over an edit the phone saved since the
    sync read its copy, `sync_local_changed`); only the phone's changed →
    **push** (below); the same write → nothing (the base is recorded);
    both changed, or no base and different copies → **conflict** (below;
    nothing is written anywhere); the folder's copy below the base while
    the phone's is unchanged → the phone keeps its copy and reports "the
    PC's copy is older" (a rollback is never copied in); another vault at
    that place (other id) → never touched. The same write with another
    revision (a header changed by hand) is pulled when raised (it then
    fails the checks below) and "older" when lowered. A copy on trial is
    judged by the trial rules below instead (it has no edits of the
    phone's). A damaged copy on the phone is replaced.
  * **Push** (`folder_sync::sync_vault` for one vault, from
    `Core::request_mirror` – every change of a vault and every unlock –
    through `mobile::sync::push_soon`: 3 s after the last change, on the
    push thread, at once when the app leaves the foreground; and in every
    full sync): the folder's copy is read and compared first; then the
    phone's vault file goes through `<data>/sync/outgoing/<id>.keystead` to
    the plugin's `writeFile`, which truncates and rewrites the folder's file
    or creates it under exactly that name. The sync reads the written file
    back and compares the bytes; a mismatch is written once more, then
    reported (`failed`, `unsupported:sync_write_unverified`); only a
    verified write becomes the base (`pushed`). A vault the phone made and
    never synced is written as a new file once a writable folder is linked
    (`localOnly` without a folder, `notWritable` with a read-only one). A
    vault whose folder copy disappeared after it was synced (it has a base,
    or the last results show the folder had it) is **not** written again
    (`missing`). Only vault files are written – never blobs (attachments
    stay view-only on the phone; an edit keeps the PC's: the payload still
    references them and their blobs stay in the folder) – and no file of
    the folder is ever deleted, renamed or moved.
  * **Conflicts** (`<data>/sync/conflicts.json`: per vault the
    `SyncConflict` fields plus both copies' digests and `verified`;
    camelCase, no secrets, read leniently; removed with the link): recorded
    whenever a sync finds one. With the vault open its keys check the
    folder's copy first (`check_copy`): forged → rejected like any forged
    copy, no conflict; otherwise `verified`. Only verified conflicts of the
    page's open vault are shown (`sync_conflicts`, `sync://conflict` on
    every change of the list) – a conflict found while locked is checked by
    the sync after the unlock. A later sync that finds both sides agreeing
    removes it. `resolve_sync_conflict` keeps one side; the other side's
    vault file is kept as a conflict copy in `<data>/conflicts/` (never in
    the folder). **Keep the PC's** (`remote`): its blobs are copied, its
    file replaces the phone's (the base becomes its head) and the open
    session is replaced by one of the new copy – another master password:
    the copy goes on trial and the vault locks (`keyChanged`). **Keep the
    phone's** (`local`): refused when the folder's copy has another master
    password (`passwordChanged`; the side with the old password never
    wins); otherwise the open vault is saved again as a revision above the
    folder's (`UnlockedVault::save_superseding`), the folder's head becomes
    the base and the vault is pushed. Both refuse when either copy changed
    since the conflict was recorded (`sync_conflict_changed`, synced again)
    or the folder's copy turns out forged (`sync_copy_rejected`). "Later"
    is nothing: the conflict stays. **Attachments**: the vault file is read first, then
    its blob folder listed (every blob that copy references is already
    there; the folder's top level was listed before the vault file was
    read, so for a copy with attachments – format version 2 – a blob folder
    missing from that listing is looked up in a fresh one: the PC may have
    added the vault's first attachment meanwhile); blobs the phone lacks or whose size differs (a blob never
    changes under its id) are copied – only names `<id>.bin` with a blob's
    shape (`attachments::looks_like_blob`: magic bytes, size; nothing is
    decrypted) – and stored **before** the phone's vault file is replaced.
    Blobs the vault no longer references are left to the core's sweep (on
    unlock, and after a reload of the open vault: an hour after they
    arrived). Vaults the folder no longer has stay on the phone ("missing").
    **2a → 2b**: at the first start of 2b the bases are seeded once per link
    (`folder_sync::seed_bases`, `LinkState.basesSeeded`): a vault the last
    2a sync found imported, updated or up to date at the revision the
    phone's copy still has gets that copy's head as its base.
  * **Copies on trial** (`sync_trial::Trial` in `src-tauri/src/sync_trial.rs`
    – shared by both platforms since stage 2b; on the PC for the copies
    its mirror pulls while the vault is locked and for adopted vaults –,
    `<data>/sync/trial/`): the
    folder's plain header – the revision included – is not authenticated
    without the keys, so anyone who can write the folder (a shared folder, a
    compromised cloud account, a misbehaving sync client) could raise it. A
    copy the sync stores is therefore **unconfirmed** until it opens on the
    phone: `<id>.json` (`unconfirmed` – that write's revision and payload
    nonce –, `keptRevision`, `rejected`; camelCase, no secrets, removed once
    there is nothing to remember) and `<id>.keystead`, the copy it replaced
    if that one was confirmed (it opened here, or was there before this
    rule). While a copy is unconfirmed its revision proves nothing: the
    folder's next copy only has to beat the **kept** revision (the same
    write again → up to date), so a forged revision never blocks the PC's
    genuine ones; a copy that never opened on the phone just follows the
    folder. Every unlock – password or fingerprint – goes through
    `sync_trial::unlock_copy`: the copy opens → confirmed, the kept copy
    goes; it does not although its key opened (`corrupt`: the payload's AAD
    binds the revision, so a raised revision fails here, as do damaged
    bytes) → **rejected**: the kept copy is put back and opened instead (the
    same unlock, no second prompt), or – without one – the copy is removed
    (`unsupported:sync_copy_rejected`; the PC's next copy brings the vault
    again), and that write (revision and payload nonce) is never taken
    again (outcome `rejected`; the sync checks that once more under the
    trial lock right before it stores a copy); the vault's sync base
    (`sync/base/`, see "Data locations") goes back to the kept copy's head,
    or – without one – is removed. A wrong password is not a
    rejection (it may be the PC's new one) – except when it proves a forged
    key wrapping: the copy on trial has the kept copy's KDF salt and
    parameters and the same password (or fingerprint KEK) opens the kept
    copy (`kept_copy_opens`, from a scratch store `<data>/sync/probe`,
    removed again; one extra key derivation for a wrong password while a
    copy is on trial) → rejected as above. A copy with another master
    password stays on trial until that password opens it. Known limits:
    `rejected` remembers one write, so a forger who keeps changing the
    nonce gets each variant on trial again (each costs one fallback at the
    next unlock, the kept copy survives); a forged copy with another salt
    cannot be told from a genuine new password – it stays on trial (the
    unlock screen says "changed on the PC") until the PC's next copy beats
    the kept revision. The fingerprint setup is never judged by a copy on
    trial (`sync_trial::on_trial`): the sync, `device_unlock_status` and
    the unlock leave a setup with another salt alone, the unlock answers
    `unsupported:device_unlock_needs_password`; once that copy opens with
    the password (`confirm`), a setup for another salt is reset. With the vault open, its keys check
    the copy before it is stored (`UnlockedVault::check_copy`): genuine →
    stored confirmed; forged → refused, nothing written (`rejected`);
    another master password → stored on trial, then the lock below.
  * **The open vault**: its file is replaced (after the check above) and
    reloaded (`reload_if_changed`) under the same state lock, so the
    monitor's own 2-s change check never sees it first → `vault://changed`. A new master
    password (`Error::KeyChanged`) closes it like a lock with reason
    `keyChanged` (toast: the password was changed on the PC). A copy whose
    key-derivation salt changed (a new master password) is marked
    `passwordChanged` until a newer revision arrives – the unlock screen
    then explains a "wrong password".
  * **When**: at start, when the app comes back to the foreground
    (`Resumed`) – at most every 60 s, on a thread of its own –, after
    linking and on "Jetzt synchronisieren" (`sync_now`, always); one vault
    3 s after each change and after each unlock (the push, above), at once
    on `Suspended`. One sync, push, resolution or link change at a time.
    Never in the background without the app (no service, no job). Every
    change of the state is announced as `sync://status`.
  * **State** `<data>/sync/link.json` (`LinkState`, camelCase, written
    atomically, read leniently – a damaged file counts as "not linked"):
    `treeUri`, `folderName`, `linkedAt`, `writable` (the app holds write
    permission – as of the last sync; `false` for a 2a link),
    `basesSeeded` (above), `lastSyncAt` (the last sync that
    read the folder), `lastError` (the code of the last failed attempt –
    `unsupported:folder_access_lost` when the permission was revoked, the
    cloud app removed or signed out, or the folder deleted or moved: pick it
    again), `files` (per vault file: `fileName`, `vaultId`, `vaultName`,
    `revision`, `outcome` – `imported`, `updated`, `upToDate`, `pcOlder`,
    `otherVault`, `misnamed`, `unreadable`, `tooLarge`, `failed`,
    `missing`, `rejected`, and since 2b `pushed` (the phone's copy was
    written), `conflict` (both changed; nothing written), `notWritable`
    (the phone's copy is newer or new, the link is read-only) and
    `localOnly` (no folder linked: in `FolderStatus.files` the phone's
    vaults get it) –, `error`, `attachmentsCopied`, `attachmentsFailed`,
    `passwordChanged` – for a `conflict`: the folder's copy has another
    master password). No secrets anywhere; the commands answer this
    status (`FolderStatus`, plus `writable`).
  * **UI**: the welcome screen without a vault explains the way (on the PC:
    "Kopie im Cloud-Ordner aktuell halten" with the Nextcloud/OneDrive
    folder; on the phone: that same folder through the cloud app) and offers
    "Cloud-Ordner wählen"; the unlock screen has a sync status line (last
    sync, "Jetzt synchronisieren", a lost folder); Settings →
    "Synchronisierung" (phone only): folder, last sync, per-vault result
    (also the 2b outcomes `pushed`, `conflict`, `notWritable`, `localOnly`),
    "Jetzt synchronisieren", "Ordner ändern", "Verbindung trennen" (the
    copies stay). A folder linked read-only (`linked && !writable`, e.g. a
    link from before 2b) gets a banner at the top of that section ("Damit
    Änderungen zum PC kommen, wähle den Ordner einmal neu", button "Ordner
    erneut wählen" → `link_folder`, the picker opens at the linked folder),
    a note under the unlock screen's sync line, and once per app start a
    toast when a status reports an edit not written (`notWritable`) with
    the same action. The welcome screen also offers "Neuen Tresor
    erstellen" ("Phone layout"). The mock (`?platform=android`) simulates
    it (see "Mock" below).
* **Fingerprint unlock (phone)** – stage 1b, decision E5, opt-in
  (`Settings.deviceUnlock`, default off; `src-tauri/src/device_unlock.rs`:
  blobs and flows, platform-neutral and unit-tested on the desktop host;
  `src/mobile/biometric.rs`: the keystore binding and the commands;
  `src/state/deviceUnlock.tsx` in the UI):
  * **Keystore**: the official `tauri-plugin-biometric` 2.5 (Android only,
    `[target.'cfg(target_os = "android")'.dependencies]`), called from Rust
    through its `biometric_cipher` – verified in its sources: AES-256-GCM
    with the key `tauri-plugin-biometric-key` in the `AndroidKeyStore`,
    `setUserAuthenticationRequired(true)` without a validity period, so
    every use needs a **strong biometric** through a `BiometricPrompt` with
    a `CryptoObject` (`BIOMETRIC_STRONG` on Android 11+; Android 10's prompt
    asks for strong biometrics for a crypto operation as well). The key is
    invalidated by a newly enrolled fingerprint (Android's default
    `setInvalidatedByBiometricEnrollment(true)`) and by removing the screen
    lock. Keystead always passes `allowDeviceCredential = false`: no PIN,
    pattern or password instead of the finger. The plugin's pass/fail
    `authenticate` is never used (it would unlock nothing). Not offered by
    the plugin: StrongBox, `setUnlockedDeviceRequired`, a key of Keystead's
    own (one key for the whole app) – an own Kotlin plugin (E5 option b)
    remains possible as hardening, e.g. together with the Autofill service.
  * **Stored**: `<data>/device-unlock/<vault-id>.json` = `{ version: 1,
    vaultId, kdfSalt, sealed, createdAt }` (written atomically). `sealed` is
    the keystore's output (`base64(ciphertext);base64(iv)`) of the text
    `keystead-device-unlock:1:<vault-id>:<base64 master KEK>` – the KEK is
    `UnlockedVault::master_kek` (what the master password derives; it opens
    the header's wrapped vault key, see "Core API"), and the vault id inside
    binds it to its vault (the keystore's AES-GCM has no associated data, so
    a blob copied to another vault's name opens nothing). `kdfSalt` is the
    header salt the KEK belongs to (`UnlockedVault::kdf_salt`): every new
    master password has a fresh salt. The KEK is never on disk in the clear
    and never reaches the webview; Rust's copies are `Zeroizing`. What
    cannot be wiped: the plugin takes and returns text, so the base64 KEK
    passes as JSON over JNI into Kotlin strings and, as an intent extra,
    into the plugin's `BiometricActivity` (in the app's process, but intents
    and activity results travel through Android's activity manager in
    `system_server`) – memory of the device's own trusted system, never a
    file or a log, and garbage once the call returned. One copy would stay
    **reachable**: the activity keeps the text of its last cipher operation
    in a static field (`BiometricActivity.encryptDecryptData`, never
    cleared) – after a setup the plaintext KEK, for the whole process
    lifetime, also after the vault locked. So every encryption (also a
    cancelled one: the activity takes the text before its prompt) is
    followed by a decryption of the malformed text `x`, which overwrites the
    field and fails in the activity's `getCipher()` ("Failed to initialize
    the cipher") before any prompt is shown (`PluginKeystore::scrub`, under
    the same prompt lock and own-prompt guard; the transparent activity
    pauses `MainActivity` for a moment). A decryption leaves only the sealed
    text there (its result goes back as an intent extra). An own Kotlin
    cipher plugin (E5 b: bytes in and out, buffers cleared, no static state)
    would make this unnecessary.
  * **Set up** (`enable_device_unlock`, Settings → Sicherheit → "Mit
    Fingerabdruck entsperren" for the open vault): only for the open vault
    the page shows (`pageVaultId`, else `locked`); the sensor must be usable
    (else no prompt: `unsupported:device_unlock_unavailable` /
    `device_unlock_not_enrolled`); the system prompt seals the KEK, the blob
    is stored and `deviceUnlock` turned on. **Off** (`disable_device_unlock`)
    removes the blob of one vault (or all: `null`); the setting goes off
    with the last one. Off means no blob anywhere (`device_unlock_status`
    removes leftovers); the keystore key itself stays – without a blob it
    opens nothing.
  * **Unlock** (`unlock_with_device`): blob → the vault file still has the
    blob's salt (else reset **without** a prompt) → the system prompt opens
    the sealed text → the KEK of this vault → `VaultStore::unlock_with_master_kek`
    (no Argon2) → `Core::install_vault_since`, the same end as the master
    password. The lock epoch is read **after** the prompt: the user has just
    proven presence, so a lock while the prompt was open does not cancel it.
  * **Reset** (blob removed, the vault remembered in memory as reset –
    `DeviceUnlockStatus.invalidated` –, error
    `unsupported:device_unlock_invalid`): a damaged or foreign blob; a new
    master password – another salt in the vault file (changed on the PC and
    synced: `folder_sync` checks after storing a copy, the monitor on
    `Error::KeyChanged` of the open vault, the status and the unlock before
    any prompt; a recovery-key unlock on the PC sets a new master password
    too – on the phone it does not exist); the keystore cannot open it (the
    plugin's "Failed to initialize the cipher …" – a new fingerprint, the
    screen lock removed – or "Failed to decrypt the data …";
    `device_unlock::plugin_cipher_error`); a KEK that does not open the
    vault. A vault that is gone just loses its blob. Kept: a cancelled
    prompt (`device_unlock_cancelled`), an unrecognised finger
    (`device_unlock_failed`; the system prompt itself allows retries), a
    lockout (`device_unlock_lockout`), another prompt open
    (`device_unlock_busy`), anything else (`io:device_unlock: …`).
  * **The prompt and the background lock**: the plugin shows the prompt in
    an activity of its own, which pauses `MainActivity` (`Suspended`). While
    a prompt of the app's own is open (`Core::begin_own_prompt` until the
    plugin call returns; at most 120 s) and for 2 s after it closed
    (`monitor::prompt_covers_pause`, pure and tested), a `Background` signal
    does not start the background clock; if the activity is still paused
    once the prompt has closed (the user left the app from the prompt), the
    clock starts then. Verified in the emulator with "Sofort": setting up
    and unlocking leave the vault open. `app://suspended` still hides
    revealed values. One prompt at a time; the plugin call blocks the
    command's worker thread, never Android's UI thread.
  * **Webview**: no capability grants `biometric:*` (guard test), so the
    page can neither prompt nor encrypt or decrypt; the commands take only
    the prompt's texts (`{ title, reason, cancel }`, translated by the UI)
    and answer `DeviceUnlockStatus` / `VaultInfo`.
  * **UI**: Settings → Sicherheit: the switch for the open vault (disabled
    without a usable sensor, with the reason). Unlock screen: "Mit
    Fingerabdruck entsperren" when the selected vault is set up, and the
    prompt opens **on its own** once per unlock screen as soon as the app
    is visible (start, return after a background lock) – not after the user
    locked by hand (a `sessionStorage` note that survives the lock's page
    reload). Closing the prompt is no error (the password field is right
    there). A reset vault shows a note; after the next password unlock a
    dialog offers to set it up again ("Nicht jetzt" forgets the note).
  * **Manifest**: the plugin's library adds its `BiometricActivity` and
    androidx.biometric's `USE_BIOMETRIC` / `USE_FINGERPRINT` (normal
    permissions, granted at install) to the **merged** manifest; the app's
    own `AndroidManifest.xml` still declares none, and nothing adds
    `INTERNET` (the APK gate checks the merged result).
  * **Known limits**: the "reset" note lives in memory (after the process
    ends the vault just has no fingerprint unlock any more – the switch
    shows it off); the plugin's `status` asks Android about *weak*
    biometrics, so a device with only weak ones (some face unlocks) offers
    the switch and then fails at the prompt (`device_unlock_not_enrolled`);
    the system prompt shows the app's launcher icon. iOS has no keystore
    cipher in the plugin (pass/fail only): fingerprint unlock is Android
    only.
* **Phone layout** (the same React app; `src/lib/layout.mjs` decides,
  `src/lib/phone.ts` holds the hooks, `src/styles/phone.css` the styles):
  App.tsx sets `data-layout="phone"` on `<html>` when `AppInfo.platform` is
  `android`/`ios` or the viewport is at most 700 px wide (one breakpoint,
  `matchMedia`, so the browser preview with the mock gets it too; the
  desktop window's minimum is 900 px); every phone rule is keyed on that
  attribute. `AppContext.phoneLayout` tells components. Three different
  questions, kept apart: the layout (`phoneLayout`), the platform
  (`isPhonePlatform(info.platform)`: desktop- and PC-only features) and
  `info.readOnly` / `info.attachmentsReadOnly` (no editing – no platform
  since 2b – / no attachment changes – the phone).
  * **Structure**: one column. The sidebar is a drawer (menu button in front
    of the search field and in the bar the pages – settings, security
    report, generator – get; scrim; closes on every choice, on Escape and
    on back; closed it is `display: none` – Android's accessibility bridge
    missed a drawer shown by `visibility`, so TalkBack would not have found
    it). The item list stays mounted; an opened item is stacked over it
    (`.vault-view.detail-open`, the list keeps its scroll position and is
    hidden from screen readers) with a back arrow in its header; nothing
    selected shows just the list (no start panel). Navigating through the
    drawer always starts a new list without a detail on top.
  * **Back gesture** (`keystead:back` from MainActivity, see below):
    `lib/layers.ts` routes it like Escape – the top layer (dialog, menu,
    drawer) closes first, even a dialog that cannot be dismissed consumes
    it; then the back handlers (`useBackHandler`, newest first). The main
    screen's handler follows `phoneBackStep`: editor (leaves it; unsaved
    changes ask first, like "Abbrechen") → detail → clear the search → from a page or a filter to
    "Alle Elemente" → not handled, so Android moves the app to the
    background. The unlock and welcome screens register nothing.
  * **Touch**: icon buttons, buttons, fields, menu items and drawer rows at
    least 44 px (small switches and tag chips get a 44-px hit area), 15-px
    body and 16-px field text; the list's quick-copy buttons are always
    there beside the text; hover backgrounds that would stick after a tap
    are wrapped in `@media (hover: hover)` in the shared stylesheets
    (unchanged for a mouse); no keyboard hints. Heights use `100dvh`;
    `env(safe-area-inset-*)` pads the frame, dialogs and toasts (0 on
    Android, where the shell pads the webview – for iOS later). Menus are
    moved back into the window when they would stick out (`Menu.tsx`, all
    platforms), dialogs sit at the bottom at full width, toasts span the
    width and sit above Android 13+'s own "copied" bubble. Nothing focuses a text field on its own (unlock screen, dialogs:
    the keyboard would cover the form); when the keyboard opens, the
    focused field is scrolled back into view.
  * **Editing on the phone (stage 2b)**: `readOnly` is `false`, so the
    phone has "Neu", the editor (stacked over the list like the detail,
    with its own back step), favourite toggle, trash / restore / delete,
    empty trash, folder and tag management, the generator (page and the
    editor's popover) and renaming the vault. Attachments are view-only
    (`attachmentsReadOnly`: preview, no add / remove, no "Speichern unter
    …"). PC-only, gated on `isPhonePlatform`: master password, recovery key
    and the recovery reminder, "Passwort vergessen?" (recovery unlock),
    delete vault, import / export (no import launcher, no drop target).
    The unlock screen offers "Neuen Tresor erstellen"; the welcome screen
    (`PhoneWelcome`, also reached from there – then with "Zurück zum
    Entsperren") shows what the app is, the way to a vault from the PC
    ("Tresor aus Cloud-Ordner holen" in its slot
    `data-slot="vault-source"`, "Cloud-folder sync (phone)" → UI) and,
    after "oder", "Neuen Tresor erstellen": the create wizard with only the
    vault step (no recovery-key step, no extension card, no website-icons
    switch; a note that the vault stays on the phone until a folder is
    linked and the PC then asks); the new vault opens at once. `readOnly`
    still hides every editing control and every write action in
    `MainScreen.tsx` returns early on it (no platform sets it any more).
  * **Sync conflicts** (both platforms, `state/syncConflicts.tsx` mounted
    with the open vault, `components/SyncConflictDialog.tsx`): see
    "Conflict dialog" under "UI/UX principles".
  * **Desktop-only on the phone platform**: browser integration (settings
    section, pairing dialog), file drop (no `FileDropProvider`), import /
    export and advanced sections (data folder, portable mode, terminal),
    tray switches, "Beim Sperren des Computers sperren", website icons,
    the Have I Been Pwned card, "Speichern unter …" for attachments, the
    update banner, check, auto check and channel (and the boot's
    `pending_update`); About says "Android · Daten im App-Speicher" and
    offers "Releases auf GitHub öffnen" (`openExternal`; Android opens the
    browser – no permission needed).
  * **Mock**: `?platform=android` (or `ios`) gives `attachmentsReadOnly` and the phone
    platform and refuses every command the phone does not register
    (`PHONE_COMMANDS` in `mock.ts`, like Tauri's "not found"), so a phone
    screen that still reaches a desktop command shows up in the console;
    `__keysteadMock.simulateBack()` sends the back gesture. The sync
    commands are simulated: "Cloud-Ordner wählen" links "Nextcloud ›
    Keystead" and copies in the sample vaults (with `?mock=empty` the
    welcome screen shows first), `simulatePcSave()` lets the next sync bring
    a newer revision of the open vault (`vault://changed`),
    `simulatePcPasswordChange()` one with a new master password (lock reason
    `keyChanged`), `simulateFolderLost()` lets the next sync fail with
    `unsupported:folder_access_lost`; like the real app, a linked folder is
    synced on its own at start and whenever the page becomes visible again
    (the stand-in for `Resumed`), at most every 60 s and never while one
    runs. Fingerprint unlock: the "system
    prompt" answers after ~0.9 s with success unless
    `setFingerprintResult("cancel" | "fail" | "lockout")` makes the next one
    fail; `simulateNewFingerprint()` invalidates every setup (reset at its
    next use), a synced password change resets that vault's setup;
    `?fingerprint=on` starts with "Privat" set up, `?fingerprint=none` /
    `?fingerprint=notEnrolled` without a usable sensor. Stage 2b: every
    edit is "pushed" into a linked folder ~1 s later (outcome `pushed`),
    a vault made on the phone stays local until a folder is linked;
    `?sync=readonly` starts linked read-only (edits → `notWritable`, the
    re-link banner and the one-time toast; "Ordner erneut wählen" makes the
    link writable and pushes); `__keysteadMock.simulateConflict(passwordChanged?)`
    records a conflict for the open vault (also on the PC mock). Narrow the
    browser to phone width to see the layout; a narrow window without
    `?platform` shows the phone layout with editing (the editor is stacked
    like the detail).

**Android project** (`src-tauri/gen/android`, generated once by `npx tauri
android init` with CLI 2.12 – Gradle 9.6.1, AGP 9.3.1, Kotlin 2.2.10,
compileSdk/targetSdk 37 – and committed; never re-run `init` over it).
`tauri.android.conf.json` is merged into `tauri.conf.json` for Android
builds: identifier `io.github.cedrickgd.keystead` (never changes – a new
one would be a different app; the desktop keeps `com.keystead.desktop`),
`bundle.android.minSdkVersion` 29 (Android 10), and the main window as a
plain phone window (`label`, `title`, `create: false` – `setup` builds it
itself, like on the desktop, for the navigation and new-window hooks); the
CSP and everything else are inherited unchanged. CI adds
`bundle.android.versionCode` (below). Hardening on top of the template,
guarded by unit tests in `gui.rs` (`include_str!` of the committed files):
* `AndroidManifest.xml`: **no permission at all** – in particular no
  `INTERNET` (the phone is offline by design; website icons, the leaked-
  password check and update checks do not exist there). The merged
  manifest of the APK gets `USE_BIOMETRIC` and `USE_FINGERPRINT` from the
  biometric plugin's androidx.biometric (fingerprint unlock, above) and
  androidx.core's signature-protected `DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION`
  – never `INTERNET`. Only
  `src/debug/AndroidManifest.xml` adds `INTERNET`, because `tauri android
  dev` loads the UI from the Vite dev server on the PC. `allowBackup=
  "false"` plus `dataExtractionRules` (`res/xml/data_extraction_rules.xml`
  excludes every domain for cloud backup and device-to-device transfer:
  `root`, `file`, `database`, `sharedpref`, `external` and the
  device-protected storage's `device_root`, `device_file`,
  `device_database`, `device_sharedpref` – Android skips a rule with an
  unknown domain name silently),
  `windowSoftInputMode="adjustResize"`, WebView Safe Browsing off
  (`android.webkit.WebView.EnableSafeBrowsing` = false), no `debuggable`.
* `MainActivity.kt`: `FLAG_SECURE` on the window (no screenshots,
  recordings or recents thumbnails – Tauri's `contentProtected` is a no-op
  on Android); in `onWebViewCreate` `allowFileAccess`/`allowContentAccess`/
  geolocation/Safe Browsing off (the UI is served from `tauri.localhost`
  only). **Insets**: the activity is edge-to-edge (`enableEdgeToEdge`), but
  Android's WebView reports at most a display cutout to the page as
  `env(safe-area-inset-*)` (verified: WebView 133 gave the cutout's top
  inset, 0 for the gesture bar) and `adjustResize` does nothing for an
  edge-to-edge window (the keyboard covered a focused field, the viewport
  stayed 800 px). So an insets listener pads the content view – the
  webview's parent, wry calls `setContentView(webView)` – by the system
  bars, the cutout and the keyboard (`WindowInsetsCompat.Type.systemBars()
  or displayCutout() or ime()`) and consumes them: the webview gets the
  free area (360×724 CSS px on the emulator, 449 px high with the keyboard
  open; `env()` is then 0) and the padding shows the window background
  (`windowBackground` in `res/values*/themes.xml`: the UI's `--bg-app`
  colour, light and dark, so the status bar's icons – dark or light with
  the system theme – stay readable; a UI theme set against the system
  theme leaves a light/dark strip behind the bars). **Back gesture**: the activity dispatches the DOM event
  `keystead:back` (cancelable) on `window`; a listener that calls
  `preventDefault()` has consumed it (close a dialog, leave the detail
  view), otherwise the activity calls `moveTaskToBack(true)` – the app goes
  to the background without finishing, so an open vault stays open until
  the background lock applies. The gesture never becomes a WebView history
  step: `TauriActivity.handleBackNavigation = false` turns off wry's
  handler, but Tauri's core `AppPlugin` adds an `OnBackPressedCallback` of
  its own (`webView.goBack()` whenever `canGoBack()`, unless a JS
  `back-button` listener exists). The dispatcher asks the most recently
  added callback first, and the Rust side registers every plugin before it
  builds the webview – so the activity registers its callback in
  `onCreate` and moves it on top in `onWebViewCreate` (`remove()`, then
  `addCallback` again). The UI may therefore use history (`pushState`,
  `#anchors`) without changing what back does.
* `KeysteadFolderPlugin.kt` (`app/src/main/java/io/github/cedrickgd/keystead/`,
  a Tauri Android plugin – a `@TauriPlugin` class with `@Command`s – that
  `mobile::sync::plugin()` registers as `keystead-folder` with
  `register_android_plugin("io.github.cedrickgd.keystead",
  "KeysteadFolderPlugin")`; the tauri-android consumer ProGuard rules keep
  `@TauriPlugin`/`@InvokeArg` classes for R8): `pickFolder` (tree picker,
  `EXTRA_INITIAL_URI` = the linked folder, persisted **read and write**
  permission – read only when the provider refuses write (a
  `SecurityException`) → `{ tree, name, writable }`; closed → code
  `cancelled`), `releaseFolder` (gives back what is held),
  `folderAccess(tree)` (`{ read, write }`: the persisted permissions held
  now; the sync asks before each run),
  `listChildren(tree, parent?)` (`{ entries: [{ name, uri, dir, size }] }`
  – document URIs that must lie inside the granted tree), `copyFile(tree,
  uri, target, maxBytes)` (streams the document into a **new** file, which
  must be in `<app data>/sync/incoming/`; more than `maxBytes` → code
  `too_large`, the file is removed; fsync before the answer),
  `writeFile(tree, name, source)` (needs the write permission, else
  `not_writable`; `name` must be `<id>.keystead` and `source` exactly
  `<app data>/sync/outgoing/<name>`, else `invalid`; the top-level file of
  that name is opened with `openFileDescriptor(…, "wt")` and rewritten, or
  – when the provider lists none – created with
  `DocumentsContract.createDocument` and its display name checked: a
  provider that picked another name (`… (1).keystead`) → `name_taken`,
  nothing written into that new empty file, which stays (reported as
  `misnamed`); `fd.sync()` where the descriptor allows it; a size the
  provider reports that differs → `write_unverified`; answers
  `{ uri, size }`). Rejection codes: `access_lost` (no persisted
  permission, `SecurityException`, a provider that no longer knows the
  URI), `not_found`, `too_large`, `picker_unavailable`, `not_writable`,
  `name_taken`, `write_unverified`, `invalid`, `io`. Provider calls run on
  the plugin's own thread (a cloud app may download or upload first; never
  on the UI thread). Never `deleteDocument`, `renameDocument`,
  `moveDocument` or `openOutputStream`, never a blob (guard test in
  `gui.rs`). No capability grants
  `keystead-folder:*`, and Tauri checks the ACL for every `plugin:`
  command – the webview cannot call it.
* `app/build.gradle.kts`: release signing from `gen/android/keystore.
  properties` (gitignored; keys `storeFile`, `password`, `keyAlias`,
  optional `keyPassword`; CI writes it from the secrets) – without the file
  a release build is signed with the debug key so it still installs
  locally, but such an APK can never update a release-signed app. The
  keystore itself must lie **outside the repository**: a `storeFile` inside
  the checkout (the nearest folder above `gen/android` with a `.git`) fails
  the build, and `*.jks`, `*.keystore`, `*.p12` are gitignored (root and
  `gen/android`) as a second line.
* **Lifecycle**: `WindowEvent::Suspended`/`Resumed` (tauri's mapping of the
  activity's `onPause`/`onResume`; the first `onResume` is swallowed by
  tauri) reach `gui::on_window_event`. `Suspended` emits `app://suspended`
  and starts the monitor's background clock (`MonitorSignal::Background`);
  `Resumed` stops it (`MonitorSignal::Foreground`) and starts the throttled
  cloud-folder sync (above) – see "Background lock (phone)" under
  "Desktop backend behaviour details". The fingerprint prompt is an
  activity of the app's own and does not start the background clock (see
  "Fingerprint unlock (phone)"). `RunEvent::Exit`/`Core::shutdown` practically never
  run on Android (the process is killed, not quit): nothing
  security-relevant depends on them. tao ignores the first `onResume` of
  every activity: an activity the system recreates (rare with the
  manifest's `configChanges`) comes back without `Resumed`, so the
  background clock keeps running and the lock fires – the safe side.
  iOS (not built yet): tauri maps `Suspended` to
  `applicationWillResignActive` but `Resumed` to
  `applicationWillEnterForeground`, which is asymmetric (a Control Center
  pull-down resigns without a matching resume) – to revisit before an iOS
  release.
* **Clipboard** (`mobile::PluginClipboard`, installed before anything
  else in `setup`): sensitive text goes through the plugin's `write_secret`
  (`EXTRA_IS_SENSITIVE`: Android 13+ shows no preview); the backend never
  reads (Android ≥ 10 refuses reads without window focus), so clearing
  "our" secret is unconditional. Every clear first overwrites the clip with
  an empty text (writes are allowed without focus), then calls the plugin's
  `clear`, which calls `clearPrimaryClip()` only after `hasPrimaryClip()` –
  false without window focus, so on its own it would silently do nothing
  while the core has already dropped the pending secret. Window focus is
  not the lifecycle: besides the background (the usual case – the timer
  fires while the user pastes in the other app) the app is unfocused while
  resumed right after `Resumed` (before the window gets focus – when the
  monitor's catch-up clear runs), in split screen or freeform with the
  other app focused, and under the notification shade. The price: Android
  13+ may show its "copied" bubble for the empty write while Keystead is
  in front. (Verified in the emulator: a timed clear from the background
  empties the clipboard.)
  Known limits: Android freezes the process of an app that stays in the
  background (cached-apps freezer; emulator API 36: frozen after about
  60–70 s away, not within the first ~45 s) – the clear timer and the
  monitor then only run again when the app returns, and a process the
  system kills takes its clear timer with it; a secret whose timer is
  that long then stays until the next copy, the return to the app, or
  Android's own clipboard expiry (Android 13+: after an hour). The default
  of 30 s is not affected. A Kotlin `AlarmManager` receiver would be the
  hardening (it thaws the process).
* Capabilities are split by platform (`platforms` in the capability file):
  `capabilities/default.json` (windows, linux, macOS) and
  `capabilities/mobile.json` (android, iOS – `core:default` and
  `core:event:default` only). Neither grants the clipboard-manager,
  opener or biometric plugin to the webview, and the opener is registered with
  `open_js_links_on_click(false)` (its injected script would turn every
  `target="_blank"` click into a call of its forbidden JS command – the
  link would do nothing).
* **Links**: Android's WebView has no new-window handler, so a link or
  `window.open(url, "_blank")` navigates in place; the navigation hook
  (`build_main_window`) refuses it and hands it to `platform::open_url`,
  which opens it with the opener plugin **on a thread of its own**. The
  hook runs on Android's UI thread (`shouldOverrideUrlLoading`), and a
  plugin call blocks until that thread's looper has run it – called there
  it would freeze the app ("app not responding").
* Builds on the PC (the environment needs `JAVA_HOME` = JDK 17,
  `ANDROID_HOME` and `NDK_HOME` = NDK 28.2.13676358): `npx tauri android
  build --apk --debug --target x86_64` for the emulator, `npx tauri android
  build --apk --target aarch64` for phones (release build: R8, signed with
  the release key when `keystore.properties` exists, else with the debug
  key), `npx tauri android dev` with a running emulator (UI from the Vite
  dev server – the reason for the debug manifest's `INTERNET`; not tried
  in this project yet). The APK lands in
  `gen/android/app/build/outputs/apk/universal/<debug|release>/
  app-universal-<debug|release>.apk` (the "universal" flavour holds just
  the ABIs that were built). The debug APK has `INTERNET` from the debug
  manifest, is debuggable, keeps the Rust debug symbols (~200 MB) and is
  signed with the debug key. Local builds keep Tauri's default
  `versionCode` (`major*1e6 + minor*1e3 + patch`, 2000001 for 2.0.1) –
  higher than any CI build's, so uninstall such a build before installing
  a CI APK (it is signed with another key anyway). The CLI rewrites `tauri
  = { version = "2", features = [] }` in `Cargo.toml` on every run (kept
  in that form). Note: `tauri` itself pulls `reqwest` into the Android
  build (its dev-server proxy); the app never uses it there.

**Build, CI and signing** (job `android` in `.github/workflows/build.yml`,
`.github/scripts/android-apk.mjs`; the release side in "Releases & in-app
updates"):
* Every push and pull request builds the release APK for arm64 on Ubuntu
  (`npx tauri android build --apk --target aarch64 --ci`; Temurin JDK 17,
  the runner's Android SDK plus – if the image lacks them – NDK
  28.2.13676358, platform `android-37.0`, build-tools 36.0.0) and lints the
  `cfg(mobile)` code, which the test job never compiles: `cargo clippy -p
  keystead-core -p keystead-desktop --target aarch64-linux-android -- -D
  warnings` (no `--all-targets`: the app crate's tests are desktop-only).
* **Versions**: `versionName` = the build's version (`2.0.1-beta.57`, or
  the tag's `2.0.1`); `versionCode` = the run number
  (`release-version.mjs --write` sets `bundle.android.versionCode` in
  `tauri.android.conf.json`, 1 … 2 100 000 000). Android installs an APK
  over an installed app only with the same signing certificate and a
  versionCode that is not lower; run numbers only grow (a re-run keeps its
  number), so every build updates the ones before it – beta, stable
  release, next beta. Tauri's default would give every build of a version
  the same code. **Never rename `build.yml`**: its run number would start
  at 1 again and installed apps would refuse every later APK
  (`INSTALL_FAILED_VERSION_DOWNGRADE`). `autoIncrementVersionCode` is not
  used (a second counter in a committed file).
* **Signing**: the release key is the PKCS12 keystore
  `keystead-android-release.jks` (RSA 4096, alias `keystead`, valid 30
  years) in the private repo `CedrickGD/keys`, folder `keystead/`; its
  certificate's SHA-256 is
  `B7:3D:75:D6:F7:F4:60:5B:0B:4A:7A:3A:8E:00:6E:E8:E3:D0:81:F1:C3:37:85:BC:79:98:A8:8F:02:1F:3C:AA`
  (also in README.md and the workflow's `ANDROID_RELEASE_CERT_SHA256`).
  Only builds that publish (tags `v*`, pushes with `[release]` – the
  release job's condition) get the secrets `ANDROID_KEYSTORE_BASE64`,
  `ANDROID_KEYSTORE_PASSWORD` (store and key password) and
  `ANDROID_KEY_ALIAS`: `android-apk.mjs keystore` refuses anything that is
  not a PKCS12/JKS keystore, writes it to `$RUNNER_TEMP` (outside the
  checkout) and `gen/android/keystore.properties` next to the project
  (values escaped for `java.util.Properties`; both files mode 0600 and
  removed after the build); a missing secret fails the build. Every other
  build is signed with the runner's debug key (generated on the runner, so
  it may differ from run to run) – it installs, but never as an update of
  a release-signed app (nor the other way round, nor necessarily of another
  run's debug-signed APK: then uninstall first). Losing the
  keystore means no installed app can be updated again: users would have
  to uninstall – which deletes the phone's data – and install an APK with
  a new key.
* **Gates** (`android-apk.mjs verify`, every build): `apksigner verify
  --print-certs` (a valid signature, exactly one signer – on publishing
  builds exactly the release certificate, so a wrong secret never ships an
  APK that cannot update installed apps), `zipalign -c -P 16 -v 4` (native
  libraries aligned for devices with 16 KB memory pages), `aapt2 dump
  badging` (package `io.github.cedrickgd.keystead`, the expected
  versionName and versionCode, minSdk 29, no `android.permission.INTERNET`,
  also not as `<uses-permission-sdk-23>`, not debuggable, `arm64-v8a`
  code). The step summary shows the APK's
  SHA-256 and the signer certificate. The artifact `Keystead-<version>-
  android` holds `Keystead-<version>-android.apk`; the release job
  attaches it to the release.
* **Updates**: the phone has no updater and no update check – it stays
  without the `INTERNET` permission (the "new version" hint through a
  Kotlin plugin that decision E6 planned for stage 2 was dropped for that;
  About links to the releases page). The user downloads the newer APK from the releases page
  and installs it over the old one; the package installer checks
  certificate and versionCode, the app's data stays. `latest.json` and the
  `updater-beta` channel stay desktop-only.

**Desktop-only, and why**:

| Not on the phone | Why |
|---|---|
| Browser bridge, extension, pairing (`browser_status`, `respond_pairing`, …) | Native Messaging exists for desktop browsers only; the phone's counterpart is an Autofill service (stage 3) |
| Terminal UI, `open_terminal`, `open_data_dir`, `open_extension_dir` | no terminal, no file manager to show a path in |
| Tray, single instance, `--background`, portable mode | no such concepts on Android (one task, app-private storage) |
| In-app updates (`check_update`, `pending_update`, `install_update`) | Android installs APKs itself and checks their signature; no network (above) |
| Website-icon fetcher, leaked-password check | need the network; reqwest's platform TLS verifier would need JNI set-up; permanently off on the phone (E6) – stored icons are shown (`get_icons`) |
| Master password, recovery key (`change_master_password`, `create_recovery_key`, `remove_recovery_key`, `unlock_with_recovery`), `delete_vault` | owner's decision for 2b: a new master password and the recovery key are made on the PC (the phone gets the new password with the next revision); deleting a vault stays a PC action. The phone edits items, folders, tags, the generator history and the vault name, and creates vaults (since 2b) |
| Adding, saving and removing attachments (`add_attachment`, `save_attachment`, `remove_attachment`), import, export, `show_export` | they take file-dialog paths, and Android hands out `content://` URIs; the phone keeps attachments view-only (2b), so it never drops the PC's. The image preview `reveal_attachment` is on the phone; vaults and their attachments arrive through the cloud-folder sync |
| Windows session lock and the suspend (clock-jump) detection, `lockOnSystemLock` | replaced by the background lock |
| The cloud-folder mirror (`mirror_status`, `folder_offers`, `adopt_folder_vault`, `dismiss_folder_vault`, `mirrorFolder`) | the PC's half of the two-way sync over a folder path; the phone's half is its linked folder (`folder_status`, …) – `sync_conflicts`/`resolve_sync_conflict` are shared |

**iOS** is not built. The code stays iOS-compatible (gates on
`mobile`/`desktop`, Android-only code on `target_os = "android"`,
`capabilities/mobile.json` names `iOS`, only plugins with iOS support – the
biometric plugin is an Android-only dependency); no CI job compiles the iOS
branches yet, so their lints are unchecked. Planned is an unsigned simulator build on GitHub's free
macOS runner (`npx tauri ios build --debug --target aarch64-sim --no-sign`,
launch and screenshots; decision E13) – no Apple account, no device build.
Open before any iOS release: the `Suspended`/`Resumed` asymmetry (above),
no `FLAG_SECURE` equivalent (the app-switcher snapshot shows the content),
`tauri.ios.conf.json`/`Info.ios.plist`/`gen/apple`; see the iOS appendix
of `docs/BLOCK5-DESIGN.md`.

## Cloud-folder mirror (desktop) and phone link (`src-tauri/src/mirror.rs`)

"Kopie im Cloud-Ordner aktuell halten" (Block 5 stage 2a, decision E7;
two-way since stage 2b) is the desktop's half of the phone's cloud-folder
sync: the PC keeps encrypted copies of its vaults in a folder of the user's
own cloud, the phone links that folder, copies them in and writes its edits
back ("Cloud-folder sync (phone)" under "Mobile (Android)"), and the PC
takes those back. Keystead talks to no cloud service on either side – the
Nextcloud or OneDrive client syncs the folder. Desktop only
(`#[cfg(desktop)]`; the phone ignores the setting).

* **Setting** `Settings.mirrorFolder` (absolute path, `null` = off).
  Settings → "Handy" (desktop only; the phone has its own
  "Synchronisierung"): the switch "Kopie im Cloud-Ordner aktuell halten"
  (on → the folder dialog, `@tauri-apps/plugin-dialog` `open({ directory:
  true })` under the existing `dialog:allow-open`; cancelled → stays off),
  the folder with "Ordner ändern", the status (last copy, last error), the
  three steps to the phone (a folder in the PC's Nextcloud/OneDrive folder,
  the cloud app on the phone, linking the folder in the phone app – with
  "Releases auf GitHub öffnen") and what the provider sees. Off → nothing is
  written or read any more; the copies already in the folder stay (the phone
  keeps using them). A new folder starts with the open vault and a scan; the
  old folder's copies stay where they are, its conflicts and offers are
  forgotten (bases are kept per folder: setting the same folder again keeps
  them – except for a vault whose copy is missing there, which is written
  again: that is how a copy removed from the folder comes back).
* **Checks** (`mirror::check_folder`): `save_settings` checks a *changed*
  folder before anything is saved (an unchanged one is not – an offline
  cloud drive must not block saving another setting), and every pass checks
  again. Not of the folder dialog's shape (relative, `.`/`..`) →
  `invalid_input:mirror_folder`; inside the data directory (this store's or
  the process's default one, resolved like attachment targets:
  `attachments::is_inside`) or a Keystead vaults folder (named `vaults` with
  a `settings.json` beside it, or holding a `*.keystead.bak` /
  `*.keystead.lock` – e.g. a portable copy's working files) →
  `invalid_input:mirror_folder_data_dir`; missing →
  `unsupported:mirror_folder_missing` (never created: a cloud drive that is
  not mounted must not turn into a local folder).
* **Layout** – exactly the phone's contract, at the folder's top level:
  `<id>.keystead`, the vault file byte for byte as it is on disk, and
  `<id>.keystead.attachments/<attachment id>.bin`, the blobs byte for byte.
  The folder is untrusted input here too: its copy of a vault file is read
  at most up to `sync::VAULT_FILE_MAX_BYTES` (64 MiB) – one whose size is
  above it is replaced without being read (a cloud drive's placeholder is
  not downloaded for it), and `delete_vault` leaves it alone like a damaged
  one.
  Nothing is decrypted or re-encrypted, so the folder never holds plaintext.
  Never a `.bak`, `.lock`, export or other file of the data folder; the only
  temporary files are `format::write_atomic`'s hidden `.<name>.<16 hex>.tmp`
  (the phone ignores hidden names). The provider sees the file names (vault
  and attachment ids), sizes and times and the vault file's plain-text
  header (vault name, revision, times, KDF parameters) – never an item, an
  attachment's name or a key.
* **When** (`MirrorTracker` in `Core`; one round at a time on the thread
  `keystead-mirror`, jobs queued meanwhile are merged, the last one per
  vault wins): a **pass** for a vault after every successful change of the
  open vault (`Core::mutate_with` – the UI's commands, the browser
  extension, the icon fetcher, import, rename, master password, recovery
  key; also after a `conflict` reload), whenever a vault is opened
  (`install_vault_since`: UI, extension, recovery key, new vault – this
  catches up on changes made while the app was closed or by the terminal
  UI), when the monitor reloads the open vault after another process saved
  it (also on `KeyChanged`, right before the lock), and for the open vault
  when the folder is set or changed. A **scan** of the folder on every
  unlock, whenever the main window gets the focus (`WindowEvent::Focused`),
  when the folder is set, and every 20 s while the app runs
  (`SCAN_INTERVAL`; the periodic scan reads only files whose size or time –
  or the PC's copy's – changed since the last scan). `delete_vault` removes
  the vault's copy (`<id>.keystead` only if its header has that id, and the
  `<id>.bin` files of its blob folder; nothing while the vault file still
  exists – the phone then reports it as "missing" and keeps its own copy)
  and, once that worked, its base and conflict; a removal that fails (the
  drive not mounted, the cloud app holding the file) is kept in
  `MirrorTracker` and tried again with every later round and when the
  folder setting changes – while the app runs (an exit forgets it); its
  copy is not offered meanwhile. Vaults never opened in the app are not
  copied.
* **A pass** (`mirror::mirror_vault`): read the vault file (gone → nothing
  to do), parse it (`VaultFile::parse`; the id must be the file's), check
  the folder, read the folder's copy (at most 64 MiB, parsed and validated
  – KDF limits included; too large, unreadable, damaged or another vault's
  file under this name → written over like a missing one). Then
  `sync_base::decide_pair` – `keystead_core::sync::decide` with the same
  rule on top as the phone's: the same write under another revision (a
  header changed by hand) is pulled when raised and "older" when lowered –
  on the heads of both copies and the vault's base for this folder
  (`<data>/sync/base/<id>.json`, the folder key is the `mirrorFolder` path):
  * only the PC's copy changed (or the folder has none and the vault was
    never synced with it) → **push** (below), the written head becomes the
    base;
  * the same write → only the blobs (no needless upload); the base is
    recorded;
  * only the folder's changed → **pull** (below);
  * both changed → a **conflict** (below); nothing is written anywhere;
  * the folder's copy below the base while the PC's is unchanged →
    `unsupported:mirror_remote_older` (a restored backup, a cloud app that
    has not fetched the newest file yet) – neither taken nor replaced;
  * the folder's copy removed after it was synced →
    `unsupported:mirror_remote_gone` – not written again (the phone never
    deletes files: someone removed it, or the cloud app has not put the
    newest one there yet); setting the folder again writes it.
  A copy the open vault's keys prove forged or damaged
  (`UnlockedVault::check_copy`, asked under the state lock: its KDF salt is
  this session's but the key or payload does not authenticate – e.g. a
  revision raised by hand in the folder) is remembered as rejected
  (`Trial::rejected`) and replaced with the PC's copy, as before stage 2b –
  never taken and never a conflict; a rejected write is replaced on every
  later pass too. A copy on trial on the PC (pulled while locked, adopted;
  "Copies on trial") is never written to the folder, and only a folder copy
  newer than the one that opened here last is pulled over it.
* **First pass after the update to 2b** (no base for this folder yet, both
  copies there): equal → the base is recorded. Different: before stage 2b
  only the PC wrote the folder, so the **older** copy is what both came
  from and serves as the base – a newer PC copy is pushed, a newer folder
  copy (the phone pushed first) pulled; the same revision with other
  contents is a conflict. A folder copy overwritten that way is kept as a
  conflict copy in `<data>/conflicts/` (it might be another device's write)
  – unless it is the PC's own previous revision (its `.bak`). So existing
  users get no conflict from the update itself.
* **Push**: list the data folder's blobs – after reading the vault file, so
  every blob that revision references is there –, copy those the folder
  lacks or whose size differs (a blob never changes under its id; only
  `<id>.bin` names with a blob's shape, `attachments::looks_like_blob`, at
  most 20 MiB + 45 bytes), read the folder's copy again – if it changed
  since the decision (the phone wrote it meanwhile), nothing is written and
  the next pass decides anew –, write the vault file (`write_atomic`), and
  only then remove the folder's `<id>.bin` files the data folder no longer
  has, and the blob folder once it is empty (anything else in it stays). A
  blob that cannot be written ends the pass before the vault file. So the
  phone, which reads the vault file first and lists its blob folder
  afterwards, never gets a revision without its blobs. Left: the phone
  reads revision N, the PC writes N+1 without an attachment and removes its
  blob before the phone lists the folder – the phone then has N without
  that attachment until its next sync brings N+1.
* **Pull** (`mirror::pull`): the folder's blobs the PC lacks (same checks)
  are copied into `vaults/<id>.keystead.attachments/` first; then, under
  the state lock, the trial lock and the vault file's lock
  (`format::lock_vault_file` – no save of the app or the terminal UI
  interleaves) and only if the PC's file is still the one the decision was
  made on: with the vault **open** its keys check the copy – `Opens` →
  `write_atomic` (keeping the `.bak`), the session is reloaded
  (`reload_if_changed`), unreferenced blobs are swept (an hour old),
  `vault://changed`; `Forged` → replaced with the PC's copy (above);
  `OtherPassword` → not taken, `unsupported:mirror_other_password`. With
  the vault **locked** nothing proves the copy: it is stored **on trial**
  (`sync_trial::begin_trial`: the copy that opened here is kept until the
  new one opens with the master password; one that does not although its
  key does is rejected and the kept copy comes back – "Copies on trial") –
  but never one with another KDF salt or parameters
  (`unsupported:mirror_other_password`: the PC does not take a new master
  password from the folder; the master password is changed on the PC).
  The folder's head becomes the base; `MirrorStatus.lastPulledAt` is set.
* **Offers** (`FolderOffer { vaultId, name, revision, updatedAt }` from the
  plain header): a scan lists the folder's top-level `<id>.keystead` files
  (`sync::vault_id_of_file_name`: a cloud app's `… (1).keystead` copies,
  hidden and other files are ignored). A vault the PC has is synced if it
  has a base for this folder (also while locked – a pull then goes on
  trial); one without a base waits until it is opened here. A vault the PC
  does **not** have (made on the phone) is **never imported on its own**:
  it is offered ("Neuer Tresor im Cloud-Ordner – übernehmen?";
  `folder_offers`, `mirror://offers` whenever the list changes) unless the
  user dismissed it (`dismiss_folder_vault` → `<data>/sync/dismissed.json`,
  a sorted JSON array of vault ids). `adopt_folder_vault` takes it: its
  blobs, then its file into `vaults/` – on trial without a kept copy (it
  opens with its own master password, or – forged – is removed again), its
  head as the base; refused for a vault the PC has
  (`invalid_input:vault_exists`); adopting removes it from the dismissed
  list.
* **Conflicts** – the same record as the phone's (`sync_conflict.rs`,
  `<data>/sync/conflicts.json`, see "Conflicts" under "Cloud-folder sync
  (phone)"; forgotten when the folder changes): found by a pass, `verified`
  once the open vault's keys checked the folder's copy (not forged) – a
  conflict found while locked appears after the next unlock's pass. On the
  PC `SyncConflict.lostAttachments` counts the attachments only the PC's
  copy has (both copies decrypted with the open session's key:
  `UnlockedVault::attachments_missing_in_copy`; 0 – omitted – when
  unknown). `sync_conflicts` / `sync://conflict` / `resolve_sync_conflict`
  are the phone's commands with the same contract (shared registration,
  `sync_conflict::resolve_sync_conflict` dispatches to
  `mirror::resolve_conflict`); "local" is the PC's copy, "remote" the
  folder's. **Keep the folder's** (`remote`): the PC's vault file *and its
  blob folder* are kept as a conflict copy in `<data>/conflicts/`, the
  folder's blobs are copied, its file replaces the PC's
  (`sync_conflict::install_resolved`: the session is replaced by one of
  the new copy; another master password → on trial and the vault locks,
  `keyChanged`), its head becomes the base. **Keep the PC's** (`local`):
  refused for a folder copy with another master password
  (`unsupported:sync_conflict_password`); the folder's vault file and its
  blob folder are kept as the conflict copy, the open vault is saved again
  above the folder's revision (`save_superseding`), the folder's head
  becomes the base and the vault is pushed at once. Both need the two
  copies the conflict was recorded for (`unsupported:sync_conflict_changed`,
  the vault is looked at again) and a folder copy that is not forged
  (`unsupported:sync_copy_rejected`, it is replaced); `locked` for another
  vault than the page's open one, `not_found` without a checked conflict,
  `unsupported:folder_not_linked` with the mirror off. "Später" is nothing:
  the conflict comes back with the next pass.
* **Never blocks saving**: the change returns first, the pass runs on its
  own thread. An error is logged once per distinct code and kept as
  `lastError` until a pass succeeds; `mirror://status` announces every
  change of the status (`MirrorStatus { folder, lastMirroredAt,
  lastPulledAt, lastError }`, also the answer of `mirror_status`;
  `lastMirroredAt` – the last pass that reached the folder – and
  `lastPulledAt` – the last pull – count since the app started or the
  folder was set). A conflict is no error (`sync://conflict`). The UI (`state/mirror.tsx`) shows a toast
  ("Kopie im Cloud-Ordner nicht aktualisiert: …") when a new error appears
  – not again for the same one, nor after a page reload – and the settings
  row shows it (with "Zuletzt aktualisiert …" and, once a pull happened,
  "Zuletzt Änderungen vom Handy übernommen …"); the next change tries
  again. `Core::shutdown` lets the worker finish what is queued (at most
  3 s).
* **Offers in the UI** (`state/mirror.tsx`: `folder_offers` once, then
  `mirror://offers`; `components/FolderOfferDialog.tsx`): on the unlock and
  main screens a dialog asks "Neuer Tresor im Cloud-Ordner – übernehmen?"
  with the vault's name and change time – "Übernehmen" (`adopt_folder_vault`,
  then the vault list is read again and a toast says to unlock it with its
  own master password), "Nicht übernehmen" (`dismiss_folder_vault`, never
  asked again), the X / Escape puts it off until the next start. Settings →
  Handy lists the same offers ("Neue Tresore im Cloud-Ordner") with both
  buttons. Never shown while a pairing request is open.
* **The phone's side**: see "Cloud-folder sync (phone)". A new master
  password reaches the phone with the next revision (it sees the changed
  KDF salt). A vault file above 64 MiB is written but skipped by the phone.
  Unit tests (`mirror.rs`) run the phone's engine (`folder_sync` over the
  same directory) against the PC's: edits going back and forth, and both
  sides editing – the conflict on the side that comes second, resolved
  there, both converging.

## Data locations (`keystead_core::paths`)

* `data_dir()`:
  1. The process-wide override set with `paths::set_data_dir_override(PathBuf)`
     – the mobile app sets its private app folder (Tauri
     `app_local_data_dir()`; Android: `/data/user/0/io.github.cedrickgd.keystead`,
     see "Mobile (Android)") as the first thing at start-up, since a phone
     has neither an environment nor a usable OS default. Only the first call
     counts (a second call, or an empty path, returns `false` and changes
     nothing); `paths::data_dir_override()` reads it back. `is_portable()`
     is `false` while an override is set. Desktop builds never set it.
  2. `$KEYSTEAD_DATA_DIR` if set.
  3. **Portable mode**: a folder named `Keystead-Data` next to the running
     executable, if it exists (the user creates it, or Settings → "Portabler
     Modus" creates it and moves the vaults).
  4. Otherwise the OS local data dir: Windows `%LOCALAPPDATA%\Keystead`,
     Linux `~/.local/share/keystead`, macOS `~/Library/Application Support/Keystead`.
* Vault files: `<data_dir>/vaults/<vault-id>.keystead` (+ `<vault-id>.keystead.bak`
  = previous good version, written before each save – except when the master
  password or recovery key changes: then the `.bak` is replaced by the new
  revision (or removed), so no local copy still opens with the old secret;
  `<vault-id>.keystead.lock` = empty inter-process lock file held only while a
  save runs).
* Attachments: `<data_dir>/vaults/<vault-id>.keystead.attachments/<attachment-id>.bin`
  – one encrypted blob per file attached to an item (see "Attachments");
  the directory appears with the first attached file and goes with the
  vault (`delete_vault`, the portable-mode move). A complete backup is the
  data directory – vault file plus this folder –, since no export contains
  attachments.
* Settings: `<data_dir>/settings.json` (non-secret, see `Settings` below).
* Phone only: `<data_dir>/sync/link.json` – the linked cloud folder and the
  last sync's results (no secrets, see "Cloud-folder sync (phone)");
  `<data_dir>/sync/incoming/` holds a file only while the sync copies it in,
  `<data_dir>/sync/outgoing/` only while a push hands a vault file to the
  folder plugin; `<data_dir>/sync/conflicts.json` – the recorded conflicts
  of the two-way sync (no secrets, see "Conflicts" there);
  `<data_dir>/sync/trial/<vault-id>.json` (+ `<vault-id>.keystead`, the copy
  that opened before) – a synced copy that has not opened yet, or a
  rejected write of the folder's copy ("Copies on trial");
  `<data_dir>/device-unlock/<vault-id>.json` – a vault's master KEK sealed
  by the Android Keystore, only while fingerprint unlock is on for it (see
  "Fingerprint unlock (phone)").
* Two-way cloud-folder sync (stage 2b; `src-tauri/src/sync_base.rs`,
  `sync_conflict.rs`): `<data_dir>/sync/base/<vault-id>.json` – the vault's
  sync base (`keystead_core::sync::SyncBase`: `revision`, `digest`,
  `folder`, `syncedAt`; camelCase, no secrets, atomic write, read
  leniently – unreadable is none): the last head this side wrote to the
  folder or took from it; a base recorded for another `folder` (the phone's
  tree URI, the PC's mirror folder) counts as none. The phone records it
  for every copy it stores or pushes and for equal copies; the PC for every
  copy it writes to or takes from the mirror folder and for equal copies.
  `<data_dir>/sync/conflicts.json` and `<data_dir>/sync/trial/` (see the
  phone's entry above) on the PC too – its conflicts and its copies on
  trial (pulled while locked, adopted). PC only:
  `<data_dir>/sync/dismissed.json` – the ids of folder vaults the user does
  not want offered (a sorted JSON array, no secrets).
  `<data_dir>/conflicts/<vault-id>.keystead.conflict-<YYYYMMDD-HHMMSS>`
  (UTC; `-2`, `-3`, … for another copy in the same second) – the losing
  side's vault file of a resolved sync conflict, never in the cloud folder
  and not in `vaults/` (importable as a `.keystead` file); on the PC with the
  losing side's blobs beside it in `<copy>.attachments/`.
* Desktop, when turned on: the copies in `Settings.mirrorFolder` (outside
  the data directory; `<id>.keystead` and `<id>.keystead.attachments/`, see
  "Cloud-folder mirror (desktop) and phone link") – the app keeps working on
  the data directory's files.
* Paired browser clients: `<data_dir>/bridge-clients.json` (stores only
  SHA-256 hashes of client tokens).
* Browser extension: `<data_dir>/browser-extension` – written and kept up to
  date by the app (see "Browser extension deployment"); the user loads the
  extension unpacked from there.
* Legacy VaultX 1.x vaults: `%LOCALAPPDATA%\VaultX\accounts.json` +
  `vault_*.json` (Linux/macOS: none – only manual file import).
  `$KEYSTEAD_LEGACY_DIR` overrides this directory on every OS (tests).

## Vault file format (versions 1 and 2) – `keystead_core::format`

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
* **Key rotation**: replacing a secret always replaces the vault key, so the
  old secret plus an older copy of the file (`.bak`, a backup, a synced
  version) yields only the old key, which decrypts no later revision:
  * `create_recovery_key` (also "replace") and `remove_recovery_key`: fresh
    vault key, re-wrapped with the unchanged master password (the unlocked
    vault keeps the master KEK in memory for this – no password prompt).
  * `unlock_with_recovery_key`: fresh vault key, wrapped with the new master
    password and again with the recovery code just entered (same recovery
    salt): the recovery key stays valid.
  * `change_master_password`: fresh vault key, wrapped with the new master
    password. The app does not know the recovery code, and the old recovery
    box only opens the old key – so if the vault has a recovery key, it is
    **replaced** by a freshly generated one wrapping the new key, and the
    call returns the new code (`Some`, desktop: `newRecoveryKey`) to be shown
    once like a newly created recovery key. The old code stops working like
    the old password. No recovery key: none is created (`None`/`null`).
  * In each case the previous revision is not kept as `.bak` (replaced by
    the new revision, or removed), and other open sessions follow the new
    key where their cached master KEK still opens it (recovery-key changes)
    or get `Error::KeyChanged` and must unlock again (master password
    changed – including a recovery-key unlock elsewhere).
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
* `version`: derived from the payload on every write (`format::version_for`):
  `1` (`format::FORMAT_VERSION`) for a vault without attachments – new
  vaults, and always the Keystead export –, `2`
  (`format::FORMAT_VERSION_ATTACHMENTS`) iff `VaultData.attachments` is not
  empty, and `1` again with the save that removes the last attachment. Both
  versions have the layout above; version 2 only announces that the payload
  carries attachment entries with their blob keys (see "Attachments"). The
  reader accepts 1 and 2 (`format::SUPPORTED_VERSIONS`). Mixed versions:
  builds that read version 1 only (before beta.9) refuse a vault with
  attachments cleanly (`unsupported:vault format version 2`) instead of
  silently dropping the keys on their next save; beta.9 (this reader, no
  attachments code yet) opens it and carries the `attachments` map it does
  not know through every save: header keys (`VaultFile.extra`) and
  top-level payload keys (`VaultData.extra`) a build does not know are kept
  as they are (`model::UnknownFields`, `#[serde(flatten)]`) and written
  back with every save, re-encryption included; their `Debug` shows only
  the key names, `wipe_data` blanks their strings, and the Keystead export
  leaves them out (a standalone version-1 file). Unknown keys *inside*
  items, folders, icons etc. are not kept. Any other version →
  `unsupported:vault format version <n>`; a missing one →
  `corrupt:missing format version`.
* **Attachment blobs** (`<vault path>.attachments/<attachment id>.bin`, see
  "Attachments"): 5 magic bytes `KSAT1` + 24-byte nonce + XChaCha20-Poly1305
  ciphertext (tag included) of the file bytes, under the attachment's own
  random 32-byte key (base64 in its payload entry), AAD = UTF-8 bytes of
  `"keystead:v1:attachment:" + vault id + ":" + attachment id` – a blob
  moved to another vault or entry fails to open (`corrupt:…`). Written with
  `write_atomic`, no `.bak`. Because the keys live in the payload, a key
  rotation of the vault re-encrypts no blob, and a copy of the vault made
  before a rotation – which still opens with the old secret (see above) –
  still opens the attachments it knew back then.

## Core API – `keystead-core` (Rust)

```rust
pub mod error;    // pub enum Error { Io, Json, WrongPassword, Corrupt(String), Conflict, KeyChanged, Rollback, NotFound(String), InvalidInput(String), Unsupported(String) }  pub type Result<T>
pub mod model;    // see crates/keystead-core/src/model.rs (the data contract); VaultItem.tags = free user tags, normalised and limited by the core (see "Tags")
pub mod crypto;   // KdfParams, derive_key, seal/open (XChaCha20-Poly1305), random bytes
pub mod format;   // VaultFile (serde of the JSON above), read/write atomic
pub mod paths;    // data_dir(), vaults_dir(), settings_path(), legacy_dir(), is_portable(), set_data_dir_override()/data_dir_override() (mobile)
pub mod store;    // VaultStore
pub mod vault;    // UnlockedVault
pub mod generator;// GeneratorOptions, generate(), Wordlist (en/de), presets (built-in + GeneratorPreset)
pub mod totp;     // parse + code generation
pub mod matching; // URL matching for autofill
pub mod health;   // HealthReport, strength()
pub mod pwned;    // leaked-password check: PwnedHash, pwned_hash, pwned_count, breach_candidates (no networking – the desktop app sends the range request, see "Leaked-password check")
pub mod icons;    // website icons: host keys, due/fetchable hosts, prune (no networking – the desktop app fetches)
pub mod attachments; // encrypted file attachments: limits, blob seal/open (KSAT1 + nonce + XChaCha20-Poly1305), prune, sweep; the vault methods are on UnlockedVault (see "Attachments")
pub mod import;   // legacy VaultX 1.x, CSV (Chrome/Edge/Firefox/Bitwarden/generic), Bitwarden JSON, Keystead export; format detection, duplicate/conflict check
pub mod export;   // encrypted .keystead export, CSV, Bitwarden-compatible JSON
pub mod settings; // Settings (non-secret app settings) load/save
pub mod clipboard;// copy_secret(text, clear_after: Option<Duration>), copy_text, clear_pending_secret on a pluggable ClipboardBackend (arboard on desktop, the app's own on mobile; shared by app, TUI & mobile)
pub mod sync;     // VaultHeader + header_of(bytes): id/name/revision of a .keystead file without the password (file import, cloud-folder sync);
                  // CopyIn + decide_copy_in(local, remote): Import | Update | UpToDate | RemoteOlder | OtherVault from the headers alone (the phone's copy-in, see "Cloud-folder sync");
                  // vault_id_of_file_name(name): Some(id) only for `<id>.keystead` with a valid id (no conflict copies, temp files, .bak);
                  // VAULT_FILE_MAX_BYTES = 64 MiB: the largest vault file the phone copies in and the PC's mirror reads from the folder;
                  // two-way sync (stage 2b): SyncHead { id, revision, digest } + head_of(&VaultFile) – digest = lowercase hex SHA-256 of
                  //   "keystead-sync:1\0" ‖ payload.ciphertext ‖ "\0" ‖ wrappedKey.nonce ‖ "\0" ‖ wrappedKey.ciphertext (the base64 strings; equal digest = same write);
                  // SyncBase { revision, digest, folder, synced_at } (camelCase; SyncBase::new(&head, folder, synced_at)): the last head both sides agreed on;
                  // decide(local, remote, base) -> SyncDecision (pure), top to bottom: no copies → UpToDate; only remote → NewRemote; only local → NewLocal (no base) /
                  //   RemoteGone (base); other ids → OtherVault; equal digests → UpToDate; no base → Diverged; local = base ∧ remote.rev < base.rev → RemoteOlder;
                  //   local = base ∧ remote.rev > base.rev → PullRemote; remote = base ∧ local.rev > base.rev → PushLocal; else → Diverged ("=": revision and digest);
                  // conflict_file_name(id, at_ms): `<id>.keystead.conflict-<YYYYMMDD-HHMMSS>` (UTC; never a vault file name)
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
    pub fn unlock_with_recovery_key(&self, vault_id: &str, recovery_key: &str, new_master_password: &str) -> Result<UnlockedVault>; // sets the new master pw, rotates the vault key, recovery key stays valid (re-wrapped)
    pub fn unlock_with_master_kek(&self, vault_id: &str, master_kek: &SecretKey) -> Result<UnlockedVault>; // opens the header's wrapped key with an earlier session's master KEK (UnlockedVault::master_kek), no Argon2; Error::WrongPassword if the master password changed since (the caller discards the stored KEK) – the mobile app's device unlock (biometric key wrap), see "Block 5"
    pub fn delete_vault(&self, vault_id: &str, master_password: &str) -> Result<()>;      // verifies pw first, removes the file (Err if that fails); .bak, leftover temp files and the <id>.keystead.attachments directory then best effort with a short retry (format::remove_with_retry) – the vault counts as deleted once its file is gone
    // create_vault*, unlock and unlock_with_recovery_key also run UnlockedVault::sweep_attachments (best effort) on the vault they return
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
    pub fn search(&self, query: &str) -> Vec<ItemSummary>; // non-trashed; matches name, username, uris, notes, tags, card brand; case-insensitive, all whitespace-separated terms must match
    pub fn logins_for_url(&self, url: &str) -> Vec<ItemSummary>; // non-trashed logins whose URIs match (matching::uri_matches), favorites first
    pub fn save_item(&mut self, item: VaultItem) -> Result<VaultItem>;
        // new if id empty or unknown: assigns uuid, createdAt; always sets updatedAt;
        // for logins: if password changed, pushes old one to password_history (max 10) and sets passwordRevisedAt;
        // normalises: login/card/identity Some/None according to type; tags (see "Tags" below). Persists (save()).
    pub fn trash_item(&mut self, id: &str) -> Result<()>;      // sets deletedAt, persists
    pub fn restore_item(&mut self, id: &str) -> Result<()>;
    pub fn delete_item(&mut self, id: &str) -> Result<()>;     // permanent
    pub fn empty_trash(&mut self) -> Result<usize>;
    pub fn save_folder(&mut self, folder: Folder) -> Result<Folder>;
    pub fn delete_folder(&mut self, id: &str) -> Result<()>;   // items in it get folder_id = None
    pub fn rename_tag(&mut self, from: &str, to: &str) -> Result<usize>; // on every item incl. trash, one save; case-insensitive; `to` validated like a tag; merges into an existing spelling; returns changed items, 0 = nothing written
    pub fn delete_tag(&mut self, tag: &str) -> Result<usize>;            // same matching and return; both: Conflict → reload_if_changed + retry (memory unchanged)
    pub fn add_generated_password(&mut self, password: &str) -> Result<()>; // newest first with a fresh id (UUID v4), history max 50, persists
    pub fn clear_generator_history(&mut self) -> Result<()>;
    pub fn generator_history_info(&self) -> Vec<GeneratedPasswordInfo>;  // { id, createdAt } per entry, newest first – no passwords
    pub fn generated_password(&self, id: &str) -> Option<&GeneratedPassword>; // one entry by id
        // GeneratedPassword { id, password, createdAt } (camelCase; Debug without the password). Entries of older payloads have no id
        // (serde default ""; builds up to 2.0.0 also drop the ids when they save): every load into an UnlockedVault (unlock, create,
        // reload_if_changed) gives entries without an id – or with the id of an earlier entry – a fresh UUID in memory; the next save
        // persists them.
    pub fn icon_hosts_needing_fetch(&self, now: i64) -> Vec<String>; // website icons due (see "Website icons"): none yet, 30 days old, failure 7 days old; fetchable hosts only
    pub fn set_icons(&mut self, results: &[(String, Option<Vec<u8>>)], now: i64) -> Result<usize>; // one save; None = failed; hosts of no login ignored; Conflict → reload + retry
    pub fn icon_for_host(&self, host: &str) -> Option<&str>;      // base64 PNG
    pub fn icon_for_item(&self, item: &VaultItem) -> Option<&str>;
    pub fn clear_icons(&mut self) -> Result<usize>;              // one save
    pub fn attachments_dir(&self) -> PathBuf;                    // <vault path>.attachments – exists once a file was attached (see "Attachments")
    pub fn attachments_of(&self, item_id: &str) -> Vec<AttachmentInfo>; // oldest first (createdAt, then id); empty for an unknown item
    pub fn attachment_info(&self, id: &str) -> Option<AttachmentInfo>;  // the key-free view of one entry
    pub fn add_attachment(&mut self, item_id: &str, name: &str, bytes: &[u8]) -> Result<AttachmentInfo>;
        // checks before writing anything: item exists (NotFound) → not trashed (invalid_input:item_trashed) → fewer than 10 (attachments_too_many)
        // → name trimmed, 1–200 chars (attachment_name_required / attachment_name_too_long) → ≤ 20 MiB (unsupported:attachment_too_large);
        // then seals the bytes under a fresh key into <dir>/<new id>.bin (write_atomic, no .bak) and records the entry in one save;
        // any Err of that save (Conflict too) removes the blob again → reload_if_changed + call again
    pub fn remove_attachment(&mut self, id: &str) -> Result<()>;  // drops the entry (the only key) in one save, then deletes the blob best effort; NotFound for an unknown id
    pub fn read_attachment(&self, id: &str) -> Result<Zeroizing<Vec<u8>>>; // decrypts the blob; NotFound (unknown id, missing file), Corrupt (tampered, swapped, not a blob, larger than 20 MiB + 45 B – never read whole)
    pub fn sweep_attachments(&self) -> usize;                    // deletes unreferenced *.bin and leftover *.tmp at least 1 h old; run by VaultStore after create/unlock
    pub fn import_items(&mut self, items: Vec<VaultItem>, folders: Vec<Folder>) -> Result<usize>; // assigns fresh ids, maps folder ids, persists once (no duplicate check)
    pub fn commit_import(&mut self, plan: ImportPlan, mode: ConflictMode) -> Result<ImportReport>;      // applies an import plan in one save, see "Import: detection, duplicates, conflicts"
    pub fn commit_import_ref(&mut self, plan: &ImportPlan, mode: ConflictMode) -> Result<ImportReport>; // same, keeps the plan (retry after `conflict` + reload_if_changed)
    pub fn rename(&mut self, name: &str) -> Result<()>;
    pub fn verify_master_password(&self, password: &str) -> bool;
    pub fn change_master_password(&mut self, current: &str, new: &str) -> Result<Option<String>>; // rotates the vault key; replaces an existing recovery key and returns the new formatted code (see "Key rotation")
    pub fn create_recovery_key(&mut self) -> Result<String>;   // returns formatted code, replaces an existing one; rotates the vault key
    pub fn remove_recovery_key(&mut self) -> Result<()>;       // rotates the vault key
    pub fn has_recovery_key(&self) -> bool;
    pub fn save(&mut self) -> Result<()>;                      // atomic write, revision check, .bak
    pub fn save_superseding(&mut self, min_revision: u64) -> Result<()>; // like save(), written as revision max(own, min_revision) + 1 (the two-way sync's "keep this side": beats the folder's revision);
                                                               // same Conflict check and .bak; no revision above u64::MAX → Error::Corrupt
    pub fn reload_if_changed(&mut self) -> Result<bool>;       // re-reads file if the revision on disk is newer (opens it with the in-memory master KEK, so a recovery-key rotation elsewhere is followed);
                                                               // older revision → Error::Rollback; master password changed elsewhere → Error::KeyChanged (unlock again); state unchanged on error
    pub fn path(&self) -> &Path;
    pub fn master_kek(&self) -> SecretKey;                    // copy of this session's master key-encryption key (opens the current header's wrapped_key): what device unlock stores wrapped in the platform keystore for VaultStore::unlock_with_master_kek;
                                                               // invalid after any master-password change (fresh salt; also change elsewhere → reload_if_changed = KeyChanged), unchanged by recovery-key changes; never reaches the UI layer
    pub fn kdf_salt(&self) -> &str;                            // the header's base64 KDF salt this session's master KEK was derived with (not secret); a stored KEK whose salt differs from the file's (VaultFile::kdf.salt) is known to be stale without trying it
    pub fn check_copy(&self, file: &VaultFile) -> CopyCheck;  // another copy of this vault's file (the cloud folder's), checked with this session's keys, nothing kept (a decrypted payload is wiped):
                                                               // Opens (genuine write, same master password) | Forged (same KDF salt/params but key or payload does not authenticate, or the KEK opens a key wrapped for another salt) | OtherPassword (cannot tell)
    pub fn payload_nonce(&self) -> &str;                       // base64 nonce of the payload this session opened or last wrote (fresh per write; with the revision it tells two writes apart; not secret)
}
```

### Other core modules
```rust
// generator
#[serde(default, rename_all = "camelCase")] pub struct GeneratorOptions {   // every field optional in JSON (older clients send fewer)
    pub kind: GeneratorKind,          // "password" | "passphrase"
    pub length: u32,                  // password: 4..=128 (PASSWORD_MIN_LEN/MAX_LEN), default 20
    pub uppercase: bool, pub lowercase: bool, pub digits: bool, pub symbols: bool, // defaults true,true,true,true
    pub min_uppercase: u32, pub min_lowercase: u32, pub min_digits: u32, pub min_symbols: u32, // default 1 each; each 0..=length, sum ≤ length;
                                      // a minimum only counts while its class is on (older clients send minSymbols: 1 with symbols: false)
    pub symbol_set: String,           // "symbolSet", default DEFAULT_SYMBOLS "!#$%&*+-=?@^_"; printable ASCII, no letter/digit/space,
                                      // duplicates ignored, input ≤ 256 chars; must not be empty while symbols is on
    pub exclude: String,              // characters removed from every class (any chars, input ≤ 256), default ""
    pub avoid_ambiguous: bool,        // default false (excludes Il1O0, on top of exclude)
    pub start_with_letter: bool,      // "startWithLetter", default false
    pub words: u32,                   // passphrase: 3..=20, default 5
    pub separator: String,            // default "-", ≤ 8 chars
    pub capitalize: bool,             // default true
    pub include_number: bool,         // default true
    pub wordlist: Wordlist,           // passphrase word list "en" (EFF large, 7776 words, default) | "de" (dys2p de-7776 minus 52 words, 7724 words)
}
pub fn generate(opts: &GeneratorOptions) -> Result<String>;   // OsRng, rejection sampling (no modulo bias); see "Generator" below
impl GeneratorOptions {
    pub fn validate(&self) -> Result<()>;                     // exactly the checks of generate(), no randomness
    pub fn normalized(&self) -> Result<GeneratorOptions>;     // validated copy; password: symbol_set/exclude without duplicates (first kept)
    pub fn from_json_lenient(v: &serde_json::Value) -> GeneratorOptions; // field by field: missing/invalid → default, unknown keys ignored; not validated
}
pub fn normalize_symbol_set(input: &str) -> Result<String>;  // invalid_input:symbol_set, or the set without duplicates
pub const DEFAULT_SYMBOLS: &str; pub const PASSWORD_MIN_LEN: u32 = 4; pub const PASSWORD_MAX_LEN: u32 = 128;
pub const PASSPHRASE_MIN_WORDS: u32 = 3; pub const PASSPHRASE_MAX_WORDS: u32 = 20; pub const CHARSET_INPUT_MAX_CHARS: usize = 256;
pub enum Wordlist { En, De }                                  // serde "en" | "de"; Wordlist::ALL
impl Wordlist { pub fn words(self) -> &'static [&'static str]; pub fn bits_per_word(self) -> f64 /* log2(real list size) */ }
pub fn wordlist() -> &'static [&'static str];                 // = Wordlist::En.words()
pub fn passphrase_entropy_bits(opts: &GeneratorOptions) -> Result<f64>; // words × log2(list size) + log2(10 × words) with include_number
// generator presets
pub struct GeneratorPreset { pub id: String, pub name: String, pub options: GeneratorOptions } // camelCase, serde(default)
impl GeneratorPreset { pub fn normalized(&self) -> Result<GeneratorPreset> } // id trimmed, 1..=64 of [A-Za-z0-9_-] and no built-in id (preset_id);
                                                                            // name via validate_preset_name; options normalized()
pub fn validate_preset_name(name: &str) -> Result<String>;    // trimmed; "" → preset_name_required, > 40 chars → preset_name_too_long
pub const BUILTIN_PRESET_IDS: [&str; 5] = ["standard", "alnum", "pin", "long", "passphrase"]; // PRESET_STANDARD … PRESET_PASSPHRASE
pub fn builtin_presets() -> Vec<GeneratorPreset>;            // in that order; `name` is an English fallback – UIs translate by id
pub fn builtin_preset(id: &str) -> Option<GeneratorPreset>; pub fn is_builtin_preset(id: &str) -> bool;
pub fn preset_options(id: &str, user: &[GeneratorPreset]) -> Option<GeneratorOptions>; // built-in or user preset
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
// pwned (see "Leaked-password check"; no networking – the desktop app sends the request)
pub const PREFIX_LEN: usize = 5; pub const SUFFIX_LEN: usize = 35;
pub struct PwnedHash { pub prefix: String /*5 upper-case hex – the only part that goes online*/, pub suffix: Zeroizing<String> /*35 upper-case hex, never leaves the device*/ } // Debug prints the prefix only
pub fn pwned_hash(password: &str) -> PwnedHash;            // SHA-1 as upper-case hex, split 5/35; hasher state and digest are zeroized (`sha1/zeroize`)
pub fn pwned_count(body: &str, hash: &PwnedHash) -> Option<u64>; // range-API answer (`SUFFIX:COUNT` lines, CRLF or LF, any case) → breach count; Some(0) when absent or a padding line; None when no line is well-formed (not a range answer); constant-time compare over every line, malformed lines ignored
pub struct BreachCandidate { pub hash: PwnedHash, pub item_ids: Vec<String> /*sorted*/ }
pub fn breach_candidates(data: &VaultData) -> Vec<BreachCandidate>; // non-trashed logins with a non-empty password, one candidate per distinct password, deterministic order
// attachments (see "Attachments"; the vault methods are on UnlockedVault)
pub const ATTACHMENT_MAX_BYTES: u64 = 20 * 1024 * 1024; pub const ATTACHMENTS_PER_ITEM_MAX: usize = 10; pub const ATTACHMENT_NAME_MAX_CHARS: usize = 200;
pub const DIR_SUFFIX: &str = ".attachments"; pub const BLOB_EXTENSION: &str = "bin"; pub const SWEEP_MIN_AGE: Duration = 1 h; pub const BLOB_OVERHEAD: usize = 45; // magic 5 + nonce 24 + tag 16
pub fn dir_for(vault_path: &Path) -> PathBuf; pub fn blob_path(dir: &Path, attachment_id: &str) -> PathBuf;
pub fn seal_blob(key: &SecretKey, vault_id: &str, attachment_id: &str, plaintext: &[u8]) -> Result<Vec<u8>>;
pub fn open_blob(key: &SecretKey, vault_id: &str, attachment_id: &str, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>>; // Corrupt on any mismatch
pub fn validate_name(name: &str) -> Result<String>; pub fn of_item(data: &VaultData, item_id: &str) -> Vec<AttachmentInfo>;
pub fn prune(data: &mut VaultData) -> Vec<String> /* ids of the removed entries */; pub fn sweep(dir: &Path, data: &VaultData) -> usize;
// model (attachments)
pub struct AttachmentEntry { pub item_id: String, pub name: String, pub size: u64, pub created_at: i64, pub key: String /* base64 blob key */ } // VaultData.attachments: BTreeMap<attachment id, AttachmentEntry>, #[serde(default)]; Debug prints the key's length only
pub struct AttachmentInfo { pub id: String, pub item_id: String, pub name: String, pub size: u64, pub created_at: i64 } // camelCase; the key-free view (AttachmentEntry::info(id)) for UI and TUI
// import (drag & drop flow: detect_import → read_import → plan_import → UnlockedVault::commit_import)
pub struct ImportReport {          // camelCase
    pub imported: usize,           // items added as new items (incl. conflicts kept with keepBoth)
    pub updated: usize,            // existing logins updated from the file (mode update)
    pub skipped: usize,            // invalid rows/entries of the file (each has a warning)
    pub duplicates: Vec<ImportMatch>,        // not imported: already present (existingId "" = twice in the file) – the first IMPORT_LIST_LIMIT (100)
    pub duplicate_count: usize,              // "duplicateCount": all of them
    pub conflicts_skipped: Vec<ImportMatch>, // "conflictsSkipped": conflicts not imported (mode skip, deselected, nothing to take over, or not shown in the preview)
    pub warnings: Vec<String>,               // the first IMPORT_LIST_LIMIT (100)
    pub warning_count: usize,                // "warningCount": all of them
}
pub enum ImportFormat { Legacy, Csv, BitwardenJson, Keystead } // serde "legacy" | "csv" | "bitwarden_json" | "keystead"; FromStr/Display/as_str(), needs_password()
pub struct DetectedImport { pub format: ImportFormat, pub needs_password: bool, pub file_name: String } // camelCase
pub fn detect_import(path: &Path) -> Result<DetectedImport>;        // by content, see below
pub struct ParsedImport { pub items: Vec<VaultItem>, pub folders: Vec<Folder>, pub warnings: Vec<String> /*≤ IMPORT_WARNING_LIMIT (1000)*/,
                          pub warnings_omitted: usize /*further warnings, counted only*/, pub invalid: usize } // Debug without secrets, wiped on drop
pub fn read_import(path: &Path, format: ImportFormat, password: Option<&str>) -> Result<ParsedImport>;
pub fn plan_import(existing: &VaultData, parsed: ParsedImport) -> ImportPlan;
pub struct ImportPlan { pub new_items: Vec<VaultItem>, pub folders: Vec<Folder>, pub duplicates: Vec<ImportMatch>,
                        pub conflicts: Vec<ImportConflict> /*at most one per existing login*/, pub invalid: usize, pub warnings: Vec<String>,
                        pub warnings_omitted: usize, /* + every planned conflict with its incoming item (private) */ }
impl ImportPlan { pub fn preview(&self) -> ImportPreview; pub fn warning_count(&self) -> usize } // Debug prints counts only; items wiped on drop; not Clone/Serialize
pub struct ImportPreview { pub new_count: usize, pub duplicates: Vec<ImportMatch> /*first 100*/, pub duplicate_count: usize, pub conflicts: Vec<ImportConflict> /*all*/,
                           pub invalid: usize, pub warnings: Vec<String> /*first 100*/, pub warning_count: usize } // camelCase, secret-free
pub const IMPORT_WARNING_LIMIT: usize = 1000; pub const IMPORT_LIST_LIMIT: usize = 100;
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
pub fn export_encrypted(data: &VaultData, path: &Path, password: &str) -> Result<()>; // standalone v1 file (own random key/salt), trash excluded; the only export that carries tags
pub fn export_csv(data: &VaultData) -> String;            // Bitwarden-style CSV (logins + notes), trash excluded, no tags; formula-like values get a leading ' (see below)
pub fn export_bitwarden_json(data: &VaultData) -> String; // unencrypted Bitwarden JSON, trash excluded, no tags
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
// clipboard – the policy lives here, the platform access behind a trait
pub trait ClipboardBackend { fn write_text(&self, text: &str, sensitive: bool) -> Result<()>; fn read_text(&self) -> Result<Option<String>>; fn clear(&self) -> Result<()>;
                             fn clear_if_equals(&self, expected: &str) -> Result<bool> /* default: read_text == expected → clear; a backend that cannot read (Android ≥ 10 in the background) overrides it and clears unconditionally */ }
pub fn install_backend(Box<dyn ClipboardBackend + Send + Sync>) -> bool; // once per process (false = one is already in place); phone: the app installs `mobile::PluginClipboard` (tauri-plugin-clipboard-manager) first thing in its setup – `write_secret` for sensitive text, never reads, `clear_if_equals` clears unconditionally; desktop: the arboard backend is installed lazily on first use (one long-lived handle, Windows retry while the clipboard is occupied);
                                                                          // Android/iOS (arboard is not compiled there): no default – the app installs its own at start-up, without one every function below fails with Error::Unsupported("clipboard")
pub fn copy_secret(text: &str, clear_after: Option<Duration>) -> Result<()>; // write_text(sensitive = true): excluded from clipboard history where supported; Windows: CanIncludeInClipboardHistory=0 and CanUploadToCloudClipboard=0 (no Cloud Clipboard sync);
                                                                             // remembered as the pending secret; with a delay a thread calls clear_if_equals(secret) after it, unless something newer was copied through this module (generation counter)
pub fn copy_text(text: &str) -> Result<()>;            // non-secret, no auto-clear; forgets a pending secret
pub fn clear_pending_secret() -> Result<bool>;         // e.g. on lock/quit: clear_if_equals(pending secret) now; false if nothing is pending
pub fn pending_clear_due() -> Option<Instant>;         // when the pending secret's timed clear fires (None: nothing pending, or copied without a timer) – the phone's background lock waits for it
pub fn clear_overdue_secret(now: SystemTime) -> Result<bool>; // catch-up: clears the pending secret if its timer is overdue by the wall clock (the timer thread sleeps on the monotonic clock, which stops while a phone is in deep sleep); the late timer then finds nothing to do
pub fn clear_if_equals(expected: &str) -> Result<bool>; pub fn read_text() -> Result<Option<String>>; // the backend's, see above
// sync (vault files as bytes, no password needed)
pub struct VaultHeader { pub id: String, pub name: String, pub revision: u64, pub updated_at: i64, pub version: u32 } // camelCase
pub fn header_of(bytes: &[u8]) -> Result<VaultHeader>; // VaultFile::parse without touching the keys: damaged/truncated Keystead file → Corrupt; another kind of file (foreign export, legacy VaultX, random bytes) or an unknown format version → Unsupported
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
* **Tags** (`VaultItem.tags`, free strings such as "Bank", "Familie";
  personal data like notes – wiped like them, never in `ItemSummary`):
  `save_item` normalises them – each tag trimmed, empty ones dropped,
  duplicates removed case-insensitively keeping the first spelling, order
  kept; a tag over `vault::TAG_MAX_CHARS` (50) → `invalid_input:tag_too_long`,
  more than `vault::TAGS_MAX` (20) distinct tags → `invalid_input:tags_too_many`.
  Imports (`import_items`, `commit_import`) normalise lossily instead:
  shortened to 50 characters, at most 20 kept. `search` matches tags like
  notes. `rename_tag(from, to)` / `delete_tag(tag)` work on every item
  including the trash in one save (case-insensitive match; `to` trimmed,
  `tag_required` / `tag_too_long`; an item that already has `to` in any
  spelling just loses `from`) and return the number of changed items –
  nothing to change → `Ok(0)` and no new revision, so a retry after a
  `conflict` (`reload_if_changed`, then call again) is safe. The desktop
  UI groups spellings the same way: the sidebar shows one row per tag (the
  most frequent spelling, counts summed) and the tag filter matches any
  spelling (`tagKey` / `sameTag` in `screens/main/model.ts`). The Keystead
  export carries tags and brings them back (normalised lossily on import);
  CSV and Bitwarden JSON have no field for them and leave them out, their
  importers never set them, and duplicate/conflict detection ignores them.
* Every mutation is committed to memory only after the file was written,
  so after `conflict` the in-memory state is unchanged: call
  `reload_if_changed()` and retry. A save never recreates a deleted vault
  file (`not_found`).
* Every save that changes the data also drops website icons whose host no
  longer belongs to a non-trashed login (deleted, trashed, address
  changed), so the vault keeps no list of sites the user no longer has.
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
* **Attachments** (see "Attachments"): every save that changes the data
  also prunes the attachment entries of items that no longer exist
  (`delete_item`, `empty_trash`; trashed items keep theirs) and deletes
  their blobs best effort after the write; `VaultStore::delete_vault`
  removes the side-car directory best effort after the vault file (see
  "Attachments"); the Keystead, CSV and Bitwarden exports
  carry no attachments (`export.rs` builds the exported `VaultData` with an
  empty map) and no import brings any.
* **Generator** (`generator.rs`): passwords meet their minimums by
  construction – each class's minimum is drawn from that class, the rest
  from the union of the enabled classes, then a Fisher–Yates shuffle
  (`random_below`) mixes them. With `startWithLetter` and no letter minimum
  one extra letter (from the enabled letter classes) is drawn and counts
  against the length; after the shuffle a first character that is not a
  letter is swapped with a uniformly chosen letter position (every
  arrangement of the drawn characters that starts with a letter stays
  equally likely). `exclude` and `avoidAmbiguous` remove characters from
  every class; an enabled class left empty → `no_character_set`. Checks of a
  password, in this order: `length` (4..=128) → `symbol_set` (bad
  character, input over 256 characters, empty while symbols is on) →
  `exclude` (input over 256 characters) → `no_character_set` (no class on,
  or an enabled class excluded entirely) → `start_with_letter` (no letter
  class on) → `minimums_exceed_length` (sum of the minimums of the enabled
  classes, plus the extra letter, above the length). A passphrase checks
  `words` and `separator` only; the password fields are ignored (and kept
  by `normalized`). The word lists, their sources and licences (EFF: CC BY
  4.0, dice numbers removed; dys2p: CC0, 52 words removed to keep
  passphrases family-friendly) are documented in
  `crates/keystead-core/src/wordlist/README.md`. Being compiled into every
  binary, their attribution ships too: Settings → *Über Keystead*
  (*Wortlisten für Passphrasen*) and `docs/LIESMICH.txt` in the portable ZIP.
* `invalid_input` details used by the core: `name_required`, `name_too_long`,
  `tag_required`, `tag_too_long`, `tags_too_many`, `item_trashed`,
  `attachments_too_many`, `attachment_name_required`,
  `attachment_name_too_long`,
  `password_empty`, `password_required`, `kdf_params`, `recovery_key_format`,
  `length`, `words`, `separator`, `no_character_set`,
  `minimums_exceed_length`, `symbol_set`, `exclude`, `start_with_letter`,
  `preset_id`, `preset_name_required`, `preset_name_too_long`,
  `totp_secret`, `totp_secret_empty`, `totp_uri`,
  `totp_digits`, `totp_period`, `csv_empty`, `csv_unknown_columns`,
  `bitwarden_json`, `json: …`, `csv: …`, `format <x>`.
* `unsupported` details of the import meant for the UI: `unknown_format`,
  `bitwarden_encrypted` (from `detect_import`, and from `read_import` /
  `import_into` / `import_bitwarden_json` with `bitwarden_json`),
  `file_too_large` (others are English free text, e.g.
  `vault format version 3`, `not a Keystead vault file`).
* `unsupported` details of the attachments: `attachment_too_large` (core:
  more than 20 MiB of bytes; desktop `add_attachment`: already from the
  file's metadata, before reading) and – desktop only –
  `attachment_preview` (`reveal_attachment`, see "Attachments").
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
  imported. Tags come only from Keystead files (normalised lossily, see
  "Tags"); CSV and Bitwarden JSON carry none, so those imports never create
  tags.
* **Detection by content** (`detect_import`); the extension only decides
  which reading is tried first (`.csv`/`.tsv`/`.txt` → CSV first):
  1. JSON object with `"format": "keystead"` → `keystead`, needs password
     (header validated like a vault file, e.g.
     `unsupported:vault format version 3`);
  2. `"encrypted": true` → `unsupported:bitwarden_encrypted` (password- or
     account-protected Bitwarden export);
  3. `items` array and `"encrypted": false` (or a `folders`/`collections`
     array) → `bitwarden_json`;
  4. `Salt` + `Iterations` + `IV`/`Data` (keys case-insensitive, `Mac`
     optional) → `legacy`, needs password (header validated);
  5. a CSV header row of a supported dialect (Bitwarden, Firefox, or the
     generic/Chrome/legacy aliases with at least two recognised columns;
     `,` `;` or tab, Excel `sep=` line, also `sep=<TAB>`) → `csv`;
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
  * Several rows of the file that conflict with the same existing login
    (same site and username, different passwords) form **one** conflict:
    the row with the newest `updatedAt` from the file (Bitwarden
    `revisionDate`, Keystead, Firefox `timePasswordChanged`) is kept, on a
    tie (e.g. Chrome CSV without timestamps) the last row. The other rows
    are listed as duplicates within the file (`existingId` empty,
    `existingName` = the kept row's name), so `update` never writes a
    password into the history that was never the vault's.
  * `ImportMatch`: `incomingName`; `username` = login username (trimmed), for
    other types the list subtitle (card `•••• 1234`, identity name/e-mail,
    empty for notes); `site` = the login host (empty without URI / for other
    types); `itemType`; `existingId`, `existingName`. Never a password, card
    number or note text.
* **Size of what reaches the UI**: a parsed file keeps at most 1000 warnings
  (`IMPORT_WARNING_LIMIT`; further ones are only counted,
  `ParsedImport::warnings_omitted`), so a crafted file with millions of
  invalid entries does not become millions of strings. `ImportPreview`
  (`plan.preview()`) and `ImportReport` carry at most 100 entries of
  `duplicates` and `warnings` (`IMPORT_LIST_LIMIT`, the first ones) and the
  totals in `duplicateCount` / `warningCount`; `conflicts` (preview) and
  `conflictsSkipped` (report) are complete – at most one per existing login,
  and the UI decides about each conflict by its id. The plan itself keeps
  every duplicate (the commit needs none of them).
* **Memory hygiene** (best effort): the JSON tree of a decrypted VaultX 1.x
  payload and of a plaintext Bitwarden export is held in a guard
  (`WipedValue`) that overwrites every string when it is dropped – on every
  path, errors and wrong-key garbage included. CSV rows are read into one
  reused record whose buffer is overwritten with zeros at the end. Not
  covered: serde_json's and the CSV reader's own scratch/read buffers, a
  record buffer that had to grow, and transient string copies while items
  are built (the items themselves are wiped with the plan).
* **Commit** (`commit_import`, `commit_import_ref`): one save = one revision;
  nothing is written when nothing changes. The incoming items are
  classified again against the current vault (it may have changed since the
  preview, e.g. through the browser extension):
  * an item that became a duplicate meanwhile is skipped (`duplicates`);
  * a conflict whose login was deleted, trashed or no longer has the same
    site and username is added as a new item if no other login matches it;
  * the mode applies **only to the conflicts of the preview**, and only
    while they still conflict with the login the preview named
    (`existingId`; it may have been edited). Anything else that conflicts
    at commit time – an item planned as new whose login was saved after the
    preview, or a conflict whose named login is gone while another login
    with the same site and username exists – is not imported and goes to
    `conflictsSkipped` in every mode. The commit never changes or doubles a
    login the user did not decide about;
  * conflicts the caller removed from `plan.conflicts` (deselected) are not
    imported either and are reported in `conflictsSkipped` as planned;
  * a login is updated at most once per commit.
  * `skip` (default): conflicts are not imported (`conflictsSkipped`).
  * `update`: the incoming password replaces the existing one through the
    normal save path (old password → `passwordHistory`, `passwordRevisedAt`
    and `updatedAt` set); the incoming TOTP seed is added if the existing
    login has none. An existing TOTP seed is never replaced and an empty
    incoming password never clears one – such a conflict with nothing to take
    over goes to `conflictsSkipped`. Name, URIs, notes, folder, tags stay.
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
  websiteIcons: boolean;                     // load missing website icons in the background, directly from the websites; default true.
                                             // false = no icon requests (stored icons stay visible). The old `showIcons` (never in the UI) is ignored.
  updateCheck: boolean;                      // background update check (15 s after start, every 6 h); default true
  updateChannel: "beta" | "stable";          // default "stable" (stable releases only); "beta" = betas + stable releases
  lockInBackgroundSeconds: number | null;    // mobile: lock the open vault this many seconds after the app went to the background; null = never, 0 = immediately; default 60, clamped to 24 h. The desktop ignores it
  deviceUnlock: boolean;                     // mobile: fingerprint unlock (opt-in); default false. Only the device-unlock commands change it – save_settings keeps the stored value
                                             // (see "Fingerprint unlock (phone)"). The desktop ignores it
  mirrorFolder: string | null;               // desktop: "Kopie im Cloud-Ordner aktuell halten" – the folder (absolute path) with the encrypted copies of the vaults
                                             // opened in the app, for the phone; null = off (default). A changed value is checked before saving
                                             // (see "Cloud-folder mirror (desktop) and phone link"). The phone ignores it
  generatorPresets: GeneratorPreset[];       // the user's presets { id, name, options: GeneratorOptions }; default []; built-ins are not stored
  generatorDefault: string;                  // id of the default preset (built-in or user); default "standard"
}
```
Loading is lenient: missing fields (settings files of older versions) and
invalid values get their defaults; unknown keys are ignored – and not
written back, so a build up to 2.0.0 that saves the settings drops
`generatorPresets` and `generatorDefault`. `null` is a
value of `lockInBackgroundSeconds` ("never"), not a missing key: it is
written as `null` and read back as `null`. `mirrorFolder`: a path is kept
as written; empty (or only spaces) counts as `null`.
On the phone the desktop-only fields (`lockOnSystemLock`, `minimizeToTray`,
`startInTray`, `browserIntegration`, `websiteIcons`, `updateCheck`,
`updateChannel`, `mirrorFolder`) are kept but do nothing, as do the two
mobile fields on the desktop (see "Mobile (Android)").
`generatorPresets` is parsed preset by preset (`id` and `name` must be
strings; `options` field by field via `GeneratorOptions::from_json_lenient`,
so a value of a later version costs only that field). Loading and saving
(`Settings::normalized`) keep only presets that pass
`GeneratorPreset::normalized` (name 1–40 characters after trimming, id
1–64 of `[A-Za-z0-9_-]` and not a built-in id, valid options), the first of
each id, at most `settings::GENERATOR_PRESETS_MAX` (50); an unknown
`generatorDefault` (e.g. a deleted preset) becomes `"standard"`.
`Settings::default_generator_options()` returns the options of the default
preset. Built-in presets (`generator::builtin_presets`, ids in this order):
`standard` (the `GeneratorOptions` defaults), `alnum` (letters and digits
only), `pin` (6 digits), `long` (32 characters, at least 2 of each class),
`passphrase` (5 English words) – their names are translated by the UIs.
In the desktop app only the preset commands (`generator_presets`,
`save_generator_preset`, `delete_generator_preset`,
`set_default_generator_preset`) change `generatorPresets` and
`generatorDefault`; `save_settings` keeps the stored values of both (see the
command table). The default preset is what the editor's generator popover
and the browser extension (page suggestions, context menu / shortcut, the
popup's generator and its *Login hinzufügen* – unless the user gave the
popup own options) use for every option they do not set themselves (see
"Browser bridge protocol"). The user picks it on the
generator page ("Als Standard verwenden", see "UI/UX principles"); the
settings page has no row for it – one place for everything about presets.

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
| `app_info` | – | `AppInfo { version, dataDir, portable, platform: "windows"\|"linux"\|"macos"\|"android"\|"ios", extensionId: string\|null, readOnly: boolean, attachmentsReadOnly: boolean }` – `extensionId` is `null` and `portable` `false` on the phone (no browser integration, no portable mode there); `readOnly` = the vault cannot be changed from this app (`false` on both platforms since stage 2b); `attachmentsReadOnly` = attachments cannot be added, saved or removed (`true` on the phone: previews only) |
| `list_vaults` | – | `VaultInfo[]` |
| `session_state` | – | `SessionState { unlocked: boolean, vault: VaultInfo \| null }` |
| `create_vault` | `name, masterPassword` | `VaultInfo` (and unlocks it) |
| `unlock_vault` | `vaultId, masterPassword` | `VaultInfo` |
| `unlock_with_recovery` | `vaultId, recoveryKey, newMasterPassword` | `VaultInfo` |
| `lock_vault` | – | `null` |
| `touch_activity` | – | `null` (resets auto-lock timer; UI calls it throttled to ≤1/30 s on input – mouse move/click, key, wheel – while its window is focused and visible) |
| `list_items` | – | `ItemListEntry[]` (all, incl. trash; UI filters) – **redacted**, no secrets, see "Secrets and the webview" |
| `list_folders` | – | `Folder[]` |
| `save_item` † | `item: VaultItem` | `ItemListEntry` (the saved item, redacted) |
| `set_favorite` † | `itemId, favorite: boolean` | `ItemListEntry` – (un)marks a favourite without the UI sending the item back |
| `reveal_secret` | `itemId, field: SecretField, pageVaultId` | `string` – one secret, for an eye toggle |
| `copy_secret_field` | `itemId, field: SecretField, pageVaultId` | `null` – copies a secret like `copy_text` with `sensitive`; the value never reaches the webview |
| `totp_for_item` | `itemId, pageVaultId` | `TotpCode` – the current code of a login's TOTP key (never the key) |
| `get_item_for_edit` | `itemId, pageVaultId` | `VaultItem` – the full item, only for the editor |
| `trash_item` / `restore_item` / `delete_item` † | `id` | `null` |
| `empty_trash` † | – | `number` |
| `save_folder` † | `folder: Folder` | `Folder` |
| `delete_folder` † | `id` | `null` |
| `rename_tag` † | `from, to` | `number` – the changed items; renames the tag on every item incl. trash in one save (case-insensitive, merges into an existing spelling, see "Tags"); `0` if no item has it; `to` empty → `invalid_input:tag_required`, > 50 chars → `tag_too_long`. No event: the UI refetches `list_items` |
| `delete_tag` † | `tag` | `number` – the changed items; removes the tag from every item incl. trash in one save (case-insensitive); `0` if no item has it. No event, like `rename_tag` |
| `add_attachment` † | `itemId, path` | `ItemListEntry` – attaches the file at `path` (the native open dialog's choice; its file name becomes the attachment's name) to the saved item and answers the item's new entry, see "Attachments". Errors: `invalid_input:path_required`, `invalid_input:attachment_source` (a relative path or one with `.`/`..` components, or a file inside the data directory – the app's own files are never attached), `not_found` (file or item), `unsupported:attachment_too_large` (from the file's metadata, before reading), `invalid_input:item_trashed` / `attachments_too_many` / `attachment_name_required` / `attachment_name_too_long`, `io:…` |
| `save_attachment` † | `attachmentId, path` | `null` – decrypts the attachment and writes it to `path` (the native save dialog's choice) atomically; refuses a target inside the vaults directory (the path or its deepest existing ancestor resolved), with the `.keystead` extension, relative or with `.`/`..` components (`invalid_input:attachment_target`); `not_found`, `corrupt:…`, `io:…`; `locked` also when the vault was locked or switched between reading and writing. Changes nothing but counts as user activity; the file is remembered for `show_export` |
| `remove_attachment` † | `attachmentId` | `ItemListEntry` – deletes the attachment (entry, then blob) and answers its item's new entry; `not_found` |
| `reveal_attachment` | `attachmentId, pageVaultId` | `string` – a `data:image/…;base64,…` URL for the in-app preview: only PNG, JPEG, GIF and WebP recognised by their magic bytes and at most 8 MiB, anything else `unsupported:attachment_preview` (a too large file without reading it); `not_found`, `corrupt:…`. The only command that brings attachment bytes into the webview; wiped after serialization |
| `generate_password` † | `options: GeneratorOptions, remember?: boolean` | `string` – a password for `options` (`invalid_input:options` if they do not parse, the generator's `invalid_input:…` codes if invalid). `remember` absent or `false`: a **preview**, nothing is stored – what the UI uses (the generator page and the editor's popover); the page reports a password the user used with `remember_generated`. `true` (compatibility): also added to the history of the page's vault (skipped for another vault or none) |
| `remember_generated` † | `password` | `null` – the user used a previewed password (copied it, took it into an item): added to the page's vault's generator history (newest first, at most 50); the same password as the newest entry is not added again. 1–1024 characters (`invalid_input:password_empty` / `password_too_long`); same rules as the bridge's `remember_generated` |
| `generator_history` | – | `GeneratedPasswordInfo[]` (`{ id, createdAt }`, newest first) – **redacted**, never a password (see "Secrets and the webview") |
| `reveal_generated` | `id, pageVaultId` | `string` – the password of one history entry, for an eye toggle; `not_found` for an unknown id |
| `copy_generated` | `id, pageVaultId` | `null` – copies one history entry like `copy_secret_field`; the password never reaches the webview; `not_found` for an unknown id |
| `clear_generator_history` † | – | `null` |
| `generator_presets` | – | `GeneratorPresets { builtin: GeneratorPreset[], user: GeneratorPreset[], defaultId: string, defaultOptions: GeneratorOptions }` – the built-in presets in display order (`name` is an English fallback; the UI translates by id), the user's presets (`settings.generatorPresets`), the default preset's id and its options (`Settings::default_generator_options`) |
| `save_generator_preset` | `id: string \| null, name, options: GeneratorOptions` | `{ id, presets: GeneratorPresets }` – `id` `null`: a new user preset with a fresh UUID v4 (at most 50, else `invalid_input:presets_too_many`); otherwise replaces name and options of that user preset (`not_found`; a built-in id → `invalid_input:preset_id`). Name trimmed, 1–40 characters (`preset_name_required` / `preset_name_too_long`); a name another user preset has (case-insensitive) → `preset_name_taken`; options normalised (`GeneratorOptions::normalized`, the generator's codes). Saved to `settings.json` |
| `delete_generator_preset` | `id` | `GeneratorPresets` – removes a user preset (`not_found`; a built-in id → `invalid_input:preset_id`); if it was the default, the default becomes `"standard"` |
| `set_default_generator_preset` | `id` | `GeneratorPresets` – makes a built-in or user preset the default (`not_found` for an unknown id) |
| `password_strength` | `password` | `Strength` |
| `totp_code` | `seed` | `TotpCode` (kept for compatibility; the item view has no keys and uses `totp_for_item`) |
| `copy_text` | `text, sensitive: boolean` | `null` (sensitive → excluded from clipboard history, cleared after `clipboardClearSeconds` and on lock/quit – also with `clipboardClearSeconds` = 0 –, only if clipboard still holds it. The UI copies notes of every item type, the generator's preview and the recovery key as sensitive; the secrets of items – passwords, TOTP codes, card number/code, hidden fields, old passwords – go through `copy_secret_field` and the generator history through `copy_generated`, which copy the same way) |
| `health_report` | – | `HealthReport` |
| `check_pwned_password` | `password` | `number` – the password's breach count at Have I Been Pwned (0: not in known breaches), only on the user's button press; only the first 5 hex characters of its SHA-1 leave the process (`GET https://api.pwnedpasswords.com/range/<PREFIX>`, see "Leaked-password check"). No vault needed. Errors: `unsupported:pwned_proxy` (a proxy configuration the app cannot evaluate – it never goes around a proxy), `io:HTTP <status>` (non-2xx, also 429; no retry), `io:<detail>` (network) |
| `check_pwned_vault` | `pageVaultId` | `PwnedReport { checkedAt: number, checked: number, leaked: { itemId, count: number }[], failed: number }` – checks every distinct password of the page's vault's non-trashed logins (one request per distinct prefix, ≤ 4 at a time; `checked` = distinct passwords incl. the failed ones, `leaked` = the items behind the leaked ones sorted by count desc, `failed` = passwords whose request failed or brought no listing after others succeeded). `locked` if another vault is open, or if the vault was locked or switched while the requests ran (the result is discarded); the first HTTP error (`io:HTTP <status>`) or a network error before any answer (`io:<detail>`) ends the run; `unsupported:pwned_proxy` as above. Nothing is cached or written into the vault; counts as user activity |
| `change_master_password` † | `current, newPassword` | `{ newRecoveryKey: string \| null }` – rotates the vault key; a vault with a recovery key gets a new one (the old one stops working), shown once by the UI like a newly created key; `null` without recovery key (see "Key rotation"). Unconfirmed new keys: see below the table |
| `create_recovery_key` † | – | `string` |
| `remove_recovery_key` † | – | `null` |
| `rename_vault` † | `name` | `VaultInfo` |
| `delete_vault` | `vaultId, masterPassword` | `null` (locks if it was the open vault) |
| `legacy_scan` | – | `LegacyVaultInfo[]` |
| `import_data` † | `format: "legacy"\|"csv"\|"bitwarden_json"\|"keystead", path, password: string \| null` | `ImportReport` (into the unlocked vault; one step, conflicts skipped – kept for compatibility, the UI uses the three commands below) |
| `analyze_import` † | `path, password: string \| null` | `ImportAnalysis { importId: string\|null, fileName, format: "legacy"\|"csv"\|"bitwarden_json"\|"keystead", needsPassword: boolean, preview: ImportPreview\|null }` – see "Import with preview" |
| `commit_import` † | `importId, conflictMode: "skip"\|"update"\|"keepBoth"` | `ImportReport` |
| `cancel_import` | `importId` | `null` (unknown ids are ignored) |
| `export_data` † | `format: "keystead"\|"csv"\|"bitwarden_json", path, password: string \| null, masterPassword` | `null` (re-verifies master pw) |
| `show_export` | – | `null` – shows the file the last successful `export_data` or `save_attachment` wrote in the file manager (Windows: Explorer with the file selected; elsewhere its folder); `not_found` if nothing was exported since the start or the file is gone. Never a path the page names. |
| `get_icons` | `pageVaultId` | `{ [host]: "data:image/png;base64,…" }` – the stored website icons of the page's vault (`locked` if another vault is open); hosts as in "Website icons" |
| `clear_icons` † | – | `number` – removes the vault's stored icons (with `websiteIcons` on they are loaded again right away) |
| `get_settings` | – | `Settings` |
| `save_settings` | `settings: Settings` | `Settings` – desktop: a changed `mirrorFolder` is checked first (`invalid_input:mirror_folder`, `invalid_input:mirror_folder_data_dir`, `unsupported:mirror_folder_missing`; nothing saved), then the open vault is copied into the new folder; `generatorPresets` and `generatorDefault` keep their stored values (only the preset commands change them, so a settings object the page read before cannot undo a preset change) |
| `mirror_status` | – | **Desktop only.** `MirrorStatus { folder: string\|null, lastMirroredAt: number\|null, lastPulledAt: number\|null, lastError: string\|null }` – the cloud-folder mirror ("Cloud-folder mirror (desktop) and phone link"): the folder (`Settings.mirrorFolder`), the last pass that reached the folder and the last pull (ms, since the start or since the folder was set) and the last error code (`unsupported:mirror_folder_missing`, `unsupported:mirror_other_password`, `unsupported:mirror_remote_older`, `unsupported:mirror_remote_gone`, `invalid_input:mirror_folder_data_dir`, `io:…`; `null` after a success); not user activity |
| `folder_offers` | – | **Desktop only.** `FolderOffer[]` – `{ vaultId, name, revision, updatedAt }`: the vaults in the mirror folder the PC does not have (e.g. made on the phone) and the user did not dismiss, as the last scan found them (`[]` with the mirror off); no I/O, not user activity |
| `adopt_folder_vault` | `vaultId` | **Desktop only.** `VaultInfo` – takes an offered vault from the mirror folder (blobs first, then the file into `vaults/`, on trial: it opens with its own master password; its head becomes the base; no longer dismissed). `invalid_input:vault_exists` (the PC has that vault), `not_found` (no readable copy of that id in the folder), `unsupported:folder_not_linked` (mirror off), the folder checks' codes, `corrupt:…`/`unsupported:…` of a damaged or foreign file |
| `dismiss_folder_vault` | `vaultId` | **Desktop only.** `null` – the vault is no longer offered (`<data>/sync/dismissed.json`; `adopt_folder_vault` still takes it). `not_found` for an invalid id |
| `browser_status` | – | `BrowserStatus { serverRunning, extensionId, extensionDir, extensionVersion, browsers: BrowserInfo[], clients: PairedClient[] }` (`extensionDir` = `<data_dir>/browser-extension`, `extensionVersion` = the embedded extension's manifest version) |
| `register_browsers` | `browsers: string[]` (ids) | `BrowserStatus` |
| `unregister_browsers` | `browsers: string[]` | `BrowserStatus` |
| `revoke_client` | `clientId` | `BrowserStatus` |
| `respond_pairing` | `requestId, approve: boolean` | `null` |
| `open_terminal` | – | `null` (spawns `Keystead --cli` in a new console) |
| `open_data_dir` | – | `null` |
| `open_extension_dir` | – | `null` (opens `<data_dir>/browser-extension`, writes it first if missing) |
| `set_portable_mode` | `enabled: boolean` | `AppInfo` (moves vault files; requires locked or re-unlock) |
| `check_update` | – | `UpdateInfo` – checks the configured channel now; network/HTTP errors → `io:<detail>` |
| `pending_update` | – | `UpdateInfo \| null` – the update the last check found (no network; a reloaded page shows its banner again) |
| `install_update` | – | `null` – downloads, verifies, locks, installs, restarts (see "In-app updates"); answers only on failure: `unsupported:update_portable` (portable copy), `unsupported:update_platform` (no build for this OS), `not_found` (nothing newer), `invalid_input:update_in_progress`, `io:<detail>` |
| `folder_status` | – | **Phone only** (see "Cloud-folder sync (phone)"). `FolderStatus { linked, writable, syncing, folderName, linkedAt, lastSyncAt, lastError, files }` – the linked folder, whether the app may write it, and what the last sync found (`files` as in `link.json`; without a folder the phone's vaults with outcome `localOnly`); no access to the folder, not user activity |
| `link_folder` | – | **Phone only.** `FolderStatus \| null` – opens the system folder picker (at the linked folder), keeps read and write access to the choice (read only where the provider refuses write: `writable: false`), links it in place of the previous one (whose permission is given back and whose recorded conflicts are forgotten) and syncs – pushing the phone's own vaults; `null` when the picker was closed without a choice. A failed first sync is no error (the status has `lastError`). `unsupported:folder_picker` (no picker on the device), `unsupported:platform` (iOS: no folder plugin yet) |
| `sync_now` | – | **Phone only.** `FolderStatus` – syncs now (after a sync already running); the sync's error as the answer (also recorded as `lastError`): `unsupported:folder_access_lost`, `io:<detail>`; `unsupported:folder_not_linked` without a folder |
| `unlink_folder` | – | **Phone only.** `FolderStatus` (unlinked) – gives the folder's permission back and removes `link.json` and the recorded conflicts; the vault copies and their sync bases stay on the phone (and are no longer synced) |
| `sync_conflicts` | `pageVaultId` | **Both platforms.** `SyncConflict[]` – `{ vaultId, vaultName, localRevision, remoteRevision, localUpdatedAt, remoteUpdatedAt, passwordChanged, lostAttachments? }` ("local" = this side, "remote" = the folder's copy; `lostAttachments`, PC only: how many attachments only the PC's copy has – omitted when 0 or unknown): the conflicts of the page's open vault that its keys checked (`locked` for another vault); no access to the folder, not user activity |
| `resolve_sync_conflict` | `vaultId, pageVaultId, keep: "local"\|"remote"` | **Both platforms** (phone: the linked folder, "Conflicts" under "Cloud-folder sync (phone)"; PC: the mirror folder, "Conflicts" under "Cloud-folder mirror (desktop) and phone link" – the PC's conflict copy carries the losing side's blob folder). `VaultInfo` – keeps one side of the open vault's conflict, the other as a conflict copy in `<data>/conflicts/`; `keep: "remote"` may lock the vault (`vault://locked` `keyChanged`, another master password). Errors: `locked` (not the page's open vault), `not_found` (no checked conflict), `invalid_input:keep`, `unsupported:sync_conflict_password` (`local` against a new master password), `unsupported:sync_conflict_changed`, `unsupported:sync_copy_rejected`, `unsupported:folder_not_linked`, the sync's or mirror's errors |
| `device_unlock_status` | – | **Phone only** (see "Fingerprint unlock (phone)"). `DeviceUnlockStatus { available: boolean, unavailableReason: "noHardware"\|"notEnrolled"\|"other"\|null, enabled: boolean, vaults: string[], invalidated: string[] }` – whether the fingerprint can be used now, `Settings.deviceUnlock`, the vaults set up and the ones reset since the start; removes blobs that no longer fit (feature off, vault gone, another salt). No prompt, not user activity |
| `enable_device_unlock` | `prompt: { title, reason, cancel }, pageVaultId` | **Phone only.** `DeviceUnlockStatus` – the system prompt seals the open vault's master KEK, stores the blob and turns `deviceUnlock` on. `locked` (not the page's vault), `unsupported:device_unlock_unavailable` / `device_unlock_not_enrolled` (no prompt), `unsupported:device_unlock_cancelled` / `_failed` / `_lockout` / `_busy` (nothing stored) |
| `disable_device_unlock` | `vaultId: string \| null` | **Phone only.** `DeviceUnlockStatus` – removes the vault's blob (`null`: all); `deviceUnlock` goes off with the last one |
| `unlock_with_device` | `vaultId, prompt: { title, reason, cancel }` | **Phone only.** `VaultInfo` – unlocks with the fingerprint (system prompt → sealed KEK → `unlock_with_master_kek`). `unsupported:device_unlock_not_set` (none set up, or the feature off), `unsupported:device_unlock_invalid` (reset: the setup no longer opens the vault – removed), `unsupported:device_unlock_needs_password` (the vault's copy is on trial and the setup does not fit it – kept; unlock once with the password), `not_found`, the prompt's errors as for `enable_device_unlock` (the setup stays) |

**On the phone** (Android/iOS) only these commands are registered, everything else is unknown to its invoke handler: `app_info`, `list_vaults`, `session_state`, `create_vault`, `unlock_vault`, `lock_vault`, `touch_activity`, `list_items`, `list_folders`, `save_item`, `set_favorite`, `reveal_secret`, `copy_secret_field`, `totp_for_item`, `get_item_for_edit`, `trash_item`, `restore_item`, `delete_item`, `empty_trash`, `save_folder`, `delete_folder`, `rename_tag`, `delete_tag`, `reveal_attachment`, `generate_password`, `remember_generated`, `generator_history`, `reveal_generated`, `copy_generated`, `clear_generator_history`, `generator_presets`, `save_generator_preset`, `delete_generator_preset`, `set_default_generator_preset`, `password_strength`, `totp_code`, `copy_text`, `health_report`, `rename_vault`, `get_icons`, `get_settings`, `save_settings`, `sync_conflicts`, `resolve_sync_conflict` (the sync conflicts, registered on both platforms), and the phone's own `folder_status`, `link_folder`, `sync_now`, `unlink_folder` (`#[cfg(mobile)]`: the cloud-folder sync) and `device_unlock_status`, `enable_device_unlock`, `disable_device_unlock`, `unlock_with_device` (`#[cfg(mobile)]`: fingerprint unlock). Desktop-only stay the master password, recovery keys, deleting a vault, attachment changes, import/export and the network and desktop integrations. Every change of a vault on the phone is pushed to the linked folder (`Core::request_mirror` → `mobile::sync::push_soon`). (A unit test in `gui.rs` keeps the handler lists and this section in step.)

On the phone both unlocks – the master password's and the fingerprint's –
(and on the desktop the master password's: `Core::unlock` goes through
`sync_trial::unlock_copy` on both platforms; nothing is on trial there yet)
follow the trial rules of synced copies ("Copies on trial" under
"Cloud-folder sync (phone)"): a copy that opens is confirmed; one that does
not although its key opened – or whose key wrapping the kept copy proves
forged – is replaced by the copy before it (which is then opened) or,
without one, removed – error `unsupported:sync_copy_rejected`.

`BrowserInfo { id: "chrome"|"edge"|"brave"|"chromium"|"vivaldi", name, detected: boolean, registered: boolean }`,
`PairedClient { id, name, createdAt, lastSeenAt }`,
`UpdateInfo { available: boolean, currentVersion, version: string|null, notes: string|null, date: string|null /*RFC 3339*/, canInstall: boolean, releaseUrl }`
(`available` = the channel offers a newer version than the running one;
`canInstall` = `available` and this copy can install it – false for the
portable build and Linux/macOS except an AppImage: the UI links to
`releaseUrl` = `https://github.com/CedrickGD/Keystead/releases/tag/v<version>`,
or `…/releases` without a version).

**Events** (backend → frontend, `app.emit`):
* `vault://locked` – payload `{ reason: "manual"|"timeout"|"system"|"background"|"keyChanged" }` (`background` and `keyChanged` only on the phone: see "Background lock (phone)" under "Desktop backend behaviour details" below, and "Cloud-folder sync (phone)" – a synced copy of the open vault has a new master password; the UI shows a carried info toast for `timeout`, `system`, `background` and `keyChanged`)
* `vault://changed` – payload `{}` – items changed outside the UI (extension saved a login, file changed by TUI → reloaded). UI re-fetches.
* `vault://icons` – payload `{}` – the icon fetcher stored new website icons; the UI calls `get_icons` again.
* `bridge://pairing-request` – payload `{ requestId, clientName, code }` – UI shows a modal with the 6-digit code; user approves/denies → `respond_pairing`.
* `bridge://pairing-closed` – payload `{ requestId }` – that request ended
  without a decision (cancelled in the browser / browser closed, replaced by
  a newer request of the same browser, timed out): the UI closes its modal.
  `respond_pairing` for it answers `not_found`.
* `bridge://unlock-request` – payload `{}` – extension asked to open the app; UI focuses the unlock screen.
* `update://available` – payload `UpdateInfo` – the background check found a
  version not announced before (once per version; also not after a manual
  `check_update` found it). The UI shows a banner above the main window and
  the unlock screen.
* `update://progress` – payload `{ downloaded: number, total: number|null }`
  (bytes, at most every 100 ms) while `install_update` downloads.
* `update://ready` – payload `{ version }` – downloaded and verified; the app
  locks, installs and restarts now.
* `vault://unlocked` – payload `VaultInfo` – the vault was unlocked outside
  the UI (extension `unlock`); the UI switches to the unlocked view. Also
  sent when the extension switched to another vault while one was open: the
  UI then leaves the old vault like on lock (page reload, see below; an info
  toast `lock.switchedToast` is carried over) and the boot shows the new one.
  Until then the old view's commands marked † answer `locked`.
  Not sent for the UI's own `unlock_vault`/`create_vault`/`unlock_with_recovery`.
* `mirror://status` – payload `MirrorStatus` (see `mirror_status`) –
  desktop only: a copy into the mirror folder succeeded or failed (sent when
  the status changed – a pull too, `lastPulledAt`), or the folder was set,
  changed or turned off. The UI updates Settings → "Handy" and shows a
  toast when a new error appears.
* `mirror://offers` – payload `FolderOffer[]` (see `folder_offers`) –
  desktop only: the vaults the mirror folder offers changed (a scan found
  one, it was adopted or dismissed, the folder changed). The UI asks
  "Neuer Tresor im Cloud-Ordner – übernehmen?" and lists them in Settings
  → "Handy".
* `sync://status` – payload `FolderStatus` (see `folder_status`) – phone
  only: a sync started or ended, or the folder was linked or unlinked. The
  UI re-reads the vault list (a vault may have arrived) and shows the
  status.
* `sync://conflict` – payload `SyncConflict[]` (see `sync_conflicts`) –
  both platforms: the recorded conflicts changed (one found, checked,
  resolved or gone); the payload is the open vault's checked conflicts
  (`[]` without one). The UI shows the conflict dialog while a vault is
  open.
* `app://suspended` – payload `{}` – phone only: the app left the
  foreground (`WindowEvent::Suspended`, the activity's `onPause`).
  `useRevealedValue` hides revealed secrets and attachment previews on it,
  as it does on `blur`/`visibilitychange` (the webview fires those too; this
  is the second net).

The frontend's `src/lib/api.ts` wraps every command with typed functions and
falls back to an in-memory **mock backend** (`src/lib/mock.ts`, realistic
sample data, password `demo`) when not running inside Tauri
(`!("__TAURI_INTERNALS__" in window)`) – used for `npm run dev` in a browser
and for automated screenshots. Mock-only extras: `?mock=empty` starts without
any vault (first-run screen), `?lang=en` starts in English, `?update=available`
announces a fake update (2.0.0-beta.4 → 2.0.0-beta.5) 1 s after start,
`?update=portable` the same for a portable copy, `?update=none` lets the manual
check find nothing (default: only the manual check finds the fake update; the
stable channel has no release – the mock starts on the beta channel so these
flows show, while the app's default is `stable`), `&updateSpeed=slow` keeps the download progress
on screen (a normal install "restarts" by reloading with the new version), and
`window.__keysteadMock` offers `triggerPairing(name?)`, `simulateTimeoutLock()`,
`simulateExternalChange()`, `simulateUnlockRequest()`,
`simulateUpdate(portable?)`, for the two-way sync `simulateConflict(passwordChanged?)`
(a conflict for the open vault, phone or PC – on the PC with one attachment
only the PC has), `simulatePhonePush()` (PC: a note "Neu vom Handy" arrives
in the open vault, `vault://changed`, `lastPulledAt`) and
`simulatePhoneNewVault()` (PC: "Handy-Tresor" appears in the folder – the
offer dialog; adopted it opens with `demo`), for the desktop's cloud-folder mirror
`simulateMirrorFolderLost(lost?)` (the copies fail with
`unsupported:mirror_folder_missing` until called with `false`; `?mirror=on`
starts with the folder set, the folder dialog picks
`C:\Users\Demo\Nextcloud\Keystead`, every change of the open vault and its
unlock "copy" it ~0.4 s later) and, for drag & drop, `dragFile(path?)` (the
overlay stays until `dropFile` / `cancelDrag()`), `dropFile(path?)` (a string
or an array of paths; default `C:\Users\Demo\Downloads\Chrome-Passwörter.csv`)
and `cancelDrag()`. The mock recognises import files by name
(`src/lib/demo/importFiles.ts`): `*.csv` = a Chrome export with duplicates,
two changed passwords, new logins and one invalid row (against "Privat"),
`*encrypted*.json` = refused encrypted Bitwarden export, other `*.json` =
Bitwarden JSON, `vault_*.json` / paths containing `VaultX` = VaultX 1.x and
`*.keystead` (both password `demo`), anything else = unknown format.
Tags: both sample vaults carry a few ("Privat": Bank, Familie, Reisen,
Streaming, Arbeit; "Arbeit": Entwicklung, Cloud, Intern); the mock
normalises tags like the core (`normalizeTags`) and mirrors `rename_tag` /
`delete_tag`.
Attachments: `src/lib/demo/mockAttachments.ts` is a side store per vault
(name, size, date – no bytes) with the backend's order of checks and error
codes (see "Attachments"); "Telekom Kundencenter" in "Privat" starts with
`Vertrag.pdf` and `Personalausweis.png`; the file picker returns the next of
a few sample paths (`mockPickAttachmentFile`), whose name decides the
pretend size (a video is 25 MiB → `attachment_too_large`); the preview of an
image is a canvas drawing; entries are pruned on `delete_item` /
`empty_trash` and go with `delete_vault`.
Website icons: "Privat" starts with a few stored sample icons
(`src/lib/demo/sampleIcons.ts` – original abstract drawings, not the sites'
real icons); ~1.5 s after an unlock, a save or turning `websiteIcons` on, a
simulated fetch adds the sample icon of hosts that have none and emits
`vault://icons`.
Leaked passwords: `src/lib/demo/pwned.ts` holds a fixed breach table (the
weak sample passwords – "Sommer2019!", "hallo123", "passwort", "admin", … –
with large counts, plus "password", "123456", "qwertz", "hallo" and "demo"
for the editor's button; everything else 0); `check_pwned_password` answers
after ~0.9 s, `check_pwned_vault` builds the report like the backend
(non-trashed logins with a password, grouped by identical password, sorted
by count desc then item id) and answers after ~1 s – `locked` if the
page's vault was locked or switched meanwhile (see "Leaked-password check").
Generator: `src/lib/demo/generator.ts` generates with the checks of
`src/lib/generatorRules.ts` (the core's codes and order) and short demo word
lists (the entropy shown still uses the real list sizes); previews only,
`remember_generated` with the backend's length codes, newest-entry check
and page-vault check, the history redacted, `reveal_generated` /
`copy_generated` by id; the preset commands with the backend's codes, a
deleted default falls back to `"standard"`, `save_settings` keeps the
stored presets and default. "Privat" starts with three history entries, the
settings with one user preset ("Online-Banking": 16 characters, symbols
`!?#`, first character a letter).

Frontend integration requirements for `src-tauri`:
* File pickers use `@tauri-apps/plugin-dialog` (`open`, `save`; `open` with
  `directory: true` for the mirror folder): register
  `tauri-plugin-dialog` and grant `dialog:allow-open` + `dialog:allow-save`
  (plus `core:default` for events) to the main window's desktop capability
  (`capabilities/default.json`; the phone's `mobile.json` grants only
  `core:default` + `core:event:default` – no file dialogs in this stage,
  and never a plugin the webview is not meant to call).
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
  once the commands in flight have answered (≤ 3 s), so the item list (no
  secrets, see "Secrets and the webview"), an item open in the editor, a
  revealed value, the generator history etc. do not linger in the renderer's
  JS heap – hygiene:
  unreachable, not overwritten. Visible toasts marked `carry` (lock reason,
  portable-mode result; never vault data) survive the reload via
  `sessionStorage`. Not with the mock backend (it lives in the same page; a
  vault switch remounts the main screen, which is keyed by the vault id).
* A `locked` answer to the main screen's `list_items`/`list_folders` also
  returns to the unlock screen (not only `vault://locked`).
* Drag & drop of files uses Tauri's webview drag-drop events
  (`getCurrentWebview().onDragDropEvent`: `enter`/`over`/`drop`/`leave` with
  file paths; `dragDropEnabled` stays at its default `true`, the events need
  only `core:event:default`). On Windows this disables HTML5 drag & drop in
  the webview – the UI uses none.

### Secrets and the webview (`src-tauri/src/secrets.rs`)
The page never holds the secrets of the whole vault. A secret reaches the
webview only when the user explicitly reveals one (eye toggle) or edits the
item; copying never sends it there.
* **Redaction contract.** `list_items`, `save_item`, `set_favorite`,
  `add_attachment` and `remove_attachment` answer
  `ItemListEntry` (TS `src/lib/types.ts`, Rust `secrets::ItemListEntry`,
  camelCase): the shape of `VaultItem` with the secrets replaced –
  `login.password` and `login.totp` are always `""` with `hasPassword` /
  `hasTotp` (`totp` not blank); `card.number` is masked – `"•••• 1234"` (the
  last 4 digits) when the number has at least 8 digits, `"••••"` for a
  shorter one (never half of it), `""` for none – with `hasNumber`, and
  `card.code` is `""` with `hasCode`; every custom field has `hasValue`, and
  a `hidden` one has `value: ""` (name and kind stay; `text` and `boolean`
  values stay); `passwordHistory` entries are `{ replacedAt }` only; plus
  `attachments: AttachmentListEntry[]` – `{ id, name, size, createdAt }` of
  the item's attachments, oldest first (never the blob key; not part of
  `VaultItem`, so the editor neither sees nor sends them – see
  "Attachments"). Not
  secret, so unchanged: name, folder, favourite, notes (notes are still
  copied as sensitive), tags, attachment names (personal like notes, wiped
  with them), username, websites, cardholder, brand, expiry,
  identity, dates. The UI's search filters these non-secret fields only
  (name, username, websites, notes, tags, card brand, identity name / e-mail);
  searching by a password, TOTP key, card number or hidden value is not
  supported – it never was (`UnlockedVault::search` has the same fields).
* **`SecretField`** (JSON, serde externally tagged): `"password"`, `"totp"`
  (the stored key), `"totpCode"` (the current code), `"cardNumber"`,
  `"cardCode"`, `{ "custom": i }` (`fields[i]`), `{ "history": i }`
  (`passwordHistory[i]`, newest first). Anything else → `invalid_input:field`.
* **Commands** (all take `pageVaultId`; another open vault or none →
  `locked`, like the † commands; an unknown item, or a field the item does
  not have – another type, an index out of range, no TOTP key for
  `totpCode` – → `not_found`): `reveal_secret` returns the stored value (`""`
  for an empty one; a card number as stored); `copy_secret_field` copies it
  through the sensitive clipboard path of `copy_text` (cleared after
  `clipboardClearSeconds` and on lock/quit; a card number without its
  spaces; an empty value → `not_found`, the clipboard is not touched);
  `totp_for_item` returns `TotpCode` for the login's key (an invalid key →
  the core's `invalid_input:totp_…` codes); `get_item_for_edit` returns the
  full `VaultItem` (incl. the password history, which `save_item` takes over
  from the item it is sent); `set_favorite` † saves the stored item with the
  new `favorite` (`not_found` instead of creating an item for an unknown id).
  `reveal_secret`, `copy_secret_field`, `get_item_for_edit` and
  `set_favorite` count as user activity for auto-lock; `totp_for_item` is
  polled and does not. Their results are wiped after serialization
  (`Wiped`, like `list_items`). The attachment commands (`add_attachment`,
  `save_attachment`, `remove_attachment`, `reveal_attachment` – see
  "Attachments") follow the same rules: `pageVaultId`, user activity,
  results wiped; `reveal_attachment` is the only way attachment bytes reach
  the webview, as a data URL the UI treats like a revealed secret. The
  bridge / extension path is unchanged.
* **Generator** (`src-tauri/src/generator.rs`). Nothing is generated or
  stored behind the user's back: the generator page and the editor's popover
  generate a **preview** only when the user asks (`generate_password` with
  `remember: false`), which is not stored. Only a password the user *used* –
  copied from the generator, or taken into an item – enters the vault's
  generator history, reported by the page with `remember_generated` (the
  browser extension asks the bridge for previews only and reports a
  password the user used – copied from its popup, filled into a page, a
  suggestion taken, saved in a new login – with the bridge's
  `remember_generated`; see "Browser extension behaviour"). `generator_history` answers
  `GeneratedPasswordInfo { id, createdAt }` only; `reveal_generated` (one
  password, for an eye toggle that hides again like the item's – 30 s, blur
  / hidden window, lock, unmount) and `copy_generated` (the sensitive
  clipboard path, lock epoch checked like `copy_secret_field`) address an
  entry by its id and take `pageVaultId` (another open vault or none →
  `locked`, an unknown id → `not_found`). Ids are UUIDs v4; entries of older
  payloads get one when the vault is opened (persisted with the next save).
  The preview itself is the one generator value the page holds – the user
  asked for it, like a revealed secret.
* **UI.** The item list (`MainScreen`) holds only `ItemListEntry`s. Eye
  toggles (`SecretFieldRow`, `useRevealedSecret`) call `reveal_secret` and
  hide the value again after 30 s, when the item or its `updatedAt` changes,
  when the window loses focus or is hidden (on the phone also on
  `app://suspended`), on lock and on unmount; the value
  lives only in that component's state. Copy buttons (password, TOTP code,
  card number/code, hidden fields, old passwords), the list rows' password
  button and Ctrl+Shift+C call `copy_secret_field`. The TOTP row polls
  `totp_for_item`. The attachment preview (`AttachmentRow`,
  `useRevealedAttachment` on the same `useRevealedValue` hook) calls
  `reveal_attachment` and drops the data URL under the same rules (30 s,
  item or `updatedAt` change, blur / hidden window, lock, unmount); the
  `<img>` is not draggable and has no context menu. Files pass only through
  the native dialogs: the page hands the backend paths (`add_attachment`,
  `save_attachment`), never bytes, and the backend refuses the app's own
  files as a source and the vaults directory as a target (see
  "Attachments"). "Bearbeiten" loads `get_item_for_edit` into the editor's
  state, which is dropped after save / cancel (`save_item` answers the
  redacted entry). The password history lists dates and reveals / copies an
  entry on demand. The health report comes from `health_report`; its "no
  2FA" list uses `hasPassword` / `hasTotp`. The generator history
  (`GeneratorPage`) holds only `GeneratedPasswordInfo`s: masked rows with
  the relative time; the eye (`useRevealedGenerated` on the same
  `useRevealedValue` hook) calls `reveal_generated` and hides the password
  again under the same rules (30 s, blur / hidden window, lock, unmount –
  also when the history is cleared), the copy button calls
  `copy_generated`. The generator preview is the one generator value the
  page holds (component state, never in `localStorage`); copying it uses
  `copy_text` with `sensitive` and then `remember_generated`. The mock
  backend redacts the same way (`src/lib/demo/redact.ts`; the generator
  history in `src/lib/mock.ts`). The leaked-password check (see
  "Leaked-password check") brings no secret into the page either: the
  vault-wide run sends only `pageVaultId` and gets item ids and counts back;
  the editor's button sends the password the editor already holds.

### Recovery keys waiting for confirmation (`src/lib/recoveryMarker.ts`)
A new recovery key exists only in the dialog that shows it (created in
Settings or the setup wizard, or replacing the old one after
`change_master_password` – the old one stops working at once). A lock, a
system lock or a vault switch reloads the page (`discardPageData`) and would
drop that dialog with the only copy. The lock is never delayed for it;
instead the UI records **only the vault id** in `localStorage`
(`keystead.recoveryUnconfirmed`, survives reloads and restarts) as soon as
a key is shown, and removes it when the user confirms having stored the key
("Fertig" after the checkbox), or when the key is removed. While the marker
names the open vault and `hasRecoveryKey` is true – and the page is not
showing that key itself (`useRecoveryKeyOnScreen` in `RecoveryKeyReveal`, page
memory only, so after a reload it counts as lost) –, the main window shows a
persistent warning above its content ("Neuer Wiederherstellungsschlüssel
nicht bestätigt …") with "Neuen Schlüssel erstellen", which opens the
replace dialog (a fresh key; confirming it clears the marker). A marker for a
vault without recovery key is dropped silently.

### Import with preview (`src-tauri/src/import_flow.rs`, `src/components/import/`)
* **Flow**: `analyze_import(path, password)` → `detect_import` (by content)
  → if the format needs a password (`legacy`, `keystead`) and none (or "")
  was given: `{ needsPassword: true, preview: null, importId: null }`, no
  error and no plan → else `read_import` (key derivation without the state
  lock; a wrong password → `wrong_password`) → `plan_import` against the page's
  vault (`pageVaultId`, checked under the state lock together with storing
  the plan) → `{ importId, preview: plan.preview() }`. The user decides →
  `commit_import(importId, conflictMode)` (`commit_import_ref` through
  `Core::mutate_for_page`: on `conflict` reload and commit the same plan once
  more) → `ImportReport`, `vault://changed`. `cancel_import` drops the plan.
  Errors of `analyze_import`: `locked`, `invalid_input:path_required`, the
  detection/reading codes of the core (`unsupported:unknown_format`,
  `unsupported:bitwarden_encrypted`, `unsupported:file_too_large`,
  `not_found`, `io:…`, `wrong_password`, `unsupported:vault format version N`,
  `corrupt:…`).
* **The waiting plan** holds the file's decrypted items, so it is kept as
  briefly as possible, in one slot (`Core::import_slot`; lock order: state
  before slot): a new analysis replaces it (also when that analysis fails or
  only asks for a password). Each analysis takes a ticket from the slot when
  it starts; only the newest may store its plan, so an older analysis that
  finishes late (a slow key derivation while the next file was dropped) or
  one during which its vault was closed drops its plan and answers
  `not_found` – the newer preview stays importable (the UI ignores and
  cancels results of analyses it has moved on from, also after the dialog
  was closed). It is dropped – `ImportPlan`'s `Drop` overwrites
  the secrets – after every commit attempt (success or error: the UI analyses
  again), on `cancel_import`, 15 minutes after the analysis (the monitor
  checks every 5 s, and a commit after that answers `not_found`), and when its
  vault closes (`Core::finish_lock`: any lock, a vault switch through the
  extension, `delete_vault`, the portable-mode move, an update).
  `commit_import` answers `not_found` for an unknown, replaced, cancelled,
  committed or expired id and `locked` (dropping the plan) when `pageVaultId`
  is not the plan's vault or not the open one.
* **UI** (`ImportFlow`: as a dialog – `ImportDialog` – in the main window and
  inline as the setup wizard's import step): (1) "Datei wählen": a large drop
  zone ("Datei hierher ziehen oder klicken zum Auswählen", file picker with
  all supported extensions and "Alle Dateien"), the VaultX 1.x vaults
  `legacy_scan` finds as one-click choices (first in the wizard), the
  supported sources; no format choice. (2) Only for encrypted files: the
  password ("Master-Passwort des alten Tresors" with the recovery-password
  hint for VaultX 1.x, "Passwort der Exportdatei" for Keystead) with the
  wrong-password error inline. (3) Preview: file name + detected format
  chip, summary cards (new – always; already there, different password,
  invalid rows – when not 0), for conflicts a list (name, username, site,
  "gespeichert als …") and the choice "Überspringen" (default) / "Passwort
  aktualisieren (altes kommt in den Verlauf)" / "Beide behalten", the
  duplicates and the file's warnings collapsed; "N Elemente importieren"
  (new items + conflicts unless skipped); nothing to do → "Alles ist schon in
  deinem Tresor" and only "Schließen". (4) Result: "N importiert ·
  N aktualisiert · N übersprungen" with the skipped entries (already there /
  different password) collapsed. Lists show at most 40 rows ("… und N
  weitere"). Errors get friendly texts (`error.unsupported.unknown_format`
  lists the supported formats, `bitwarden_encrypted` explains how to export
  unencrypted, `file_too_large`, `io`, a missing file, an expired preview);
  "Erneut prüfen" only where checking the same file again can help. Leaving
  the dialog cancels a waiting plan. Numbers and "… und N weitere" use
  `duplicateCount` / `warningCount` (the lists carry at most 100 entries).
* **Entry points**: the start panel's "Passwörter importieren", Settings →
  Import & Export → "Importieren …", the welcome screen's "Von VaultX
  umsteigen" (wizard step "Import", skippable) and drag & drop.
* **Drag & drop** (`FileDropProvider` around the whole app): while files are
  dragged over the window a full-window overlay shows "Datei hier ablegen, um
  sie zu importieren" with the supported sources; the drop goes to the newest
  registered target – the open import dialog (a new file restarts its
  analysis; while it commits the file is not taken: "„x“ wurde nicht
  geöffnet, weil gerade importiert wird …"), else the main window (opens
  the dialog with the file), else the wizard's import step. Several files:
  the first is used. Both notes appear inside the dialog / wizard step (an
  info box above its content), not as a toast, which would cover the
  dialog's title. Without a target (unlock screen,
  welcome screen) the overlay says "Zum Importieren zuerst den Tresor
  entsperren" ("… einen Tresor anlegen" while no vault exists) and the drop
  does nothing but show that hint.

### Desktop backend behaviour details (implemented in `src-tauri`)
* Commands run on the blocking thread pool; complex arguments (`item`,
  `folder`, `options`, `settings`) that do not parse → `invalid_input:item`
  / `folder` / `options` / `settings`. Mutations that hit `conflict` reload
  the vault and retry once (the UI then also gets `vault://changed`).
* Auto-lock activity = `touch_activity`, deliberate user commands (unlock,
  mutations, `copy_text`, `reveal_secret`, `copy_secret_field`,
  `get_item_for_edit`, the attachment commands, the leaked-password
  checks, `generate_password`, `remember_generated`, `reveal_generated`,
  `copy_generated`, the preset commands, import/export, settings, the
  phone's `link_folder`, `sync_now`, `unlink_folder`, …) and bridge user
  actions. Polled/passive commands (`totp_code`, `totp_for_item`,
  `browser_status`, `session_state`, `app_info`, `list_*`, `get_settings`,
  `get_icons`, `mirror_status`, `generator_history`, `generator_presets`)
  do **not** reset the timer. The monitor checks every 5 s (idle time
  includes sleep).
* `lockOnSystemLock`: Windows uses session notifications
  (`WM_WTSSESSION_CHANGE`/`WTS_SESSION_LOCK`) and `PBT_APMSUSPEND`; on every
  OS a clock jump > 2 min between the monitor's 1-s ticks (suspend /
  hibernate) locks with reason `system` (also while no vault is open, see
  "Lock epoch"). Linux/macOS do not detect a plain
  screen lock without suspend.
* **Background lock (phone)**: `lockInBackgroundSeconds` after
  `WindowEvent::Suspended` (the activity's `onPause`) the monitor locks with
  reason `background` – `null` = never, `0` = at once, default 60 s. Time in
  the background is measured on both clocks like idle time (the phone's
  monotonic clock stops in deep sleep). **Never before a copied secret's
  clipboard-clear timer has fired**: the lock would clear the clipboard and
  spoil the paste into the other app (`monitor::background_lock_due` is the
  pure policy, unit-tested; a secret copied with `clipboardClearSeconds` = 0
  has no timer and does not postpone the lock). Every monitor tick and
  `Resumed` first catch up on a clipboard clear the timer thread slept
  through (`clipboard::clear_overdue_secret`, wall clock); `Resumed` then
  checks the lock a last time – a long absence locks even if Android froze
  the process meanwhile and no tick ran (emulator: frozen after about a
  minute away, see "Mobile (Android)"; with a longer delay the lock then
  lands right on the return) –, then resets the clock; so a return before
  the delay
  (or, with "at once", before the clipboard clear) finds the vault still
  open. Locks also while no vault is open (lock epoch). The app's own
  fingerprint prompt pauses the activity without starting the clock (see
  "Fingerprint unlock (phone)"). The desktop ignores
  the setting; the phone has no `lockOnSystemLock`/clock-jump detection.
* The vault file is checked every 2 s (`reload_if_changed`) → `vault://changed`.
* Locking (any reason) also clears a secret copied through the app
  (`copy_text` with `sensitive`, the bridge's `copy_field`/`copy_secret`) if
  the clipboard still holds it – also with `clipboardClearSeconds` = 0 (the
  app then remembers the secret itself, as `Zeroizing<String>`); so does
  quitting the app. `lock_vault` (the UI's own request) emits no event;
  tray "Sperren" and the extension's `lock` emit `vault://locked {manual}`.
* `list_items`, `save_item`, `set_favorite`, `reveal_secret`,
  `totp_for_item`, `get_item_for_edit`, `generate_password`,
  `reveal_generated`, `add_attachment`, `remove_attachment` and
  `reveal_attachment` overwrite the
  strings of their cloned result once Tauri has serialized it
  (`src/wipe.rs`); the serialized IPC message itself is out of reach.
* Every unlock/create stores the vault as `lastVaultId` (also an
  extension unlock, so the app's unlock screen preselects that vault after
  the next lock). The extension's `unlock` opens its `vaultId`; without one
  `lastVaultId` if it exists, else the only vault (else `not_found`). If
  another vault is open, the new one is unlocked (key derivation) while the
  old one stays open and usable; only on success it replaces it
  (`Core::install_vault_since`): the old vault is dropped (wiped) and a secret it
  copied is cleared from the clipboard like on lock, without
  `vault://locked`; then `vault://unlocked` with the new `VaultInfo`. A failed
  attempt (wrong password, rate limit, unknown id) leaves the open vault
  open. Unlocking the vault that is already open only checks the password
  (no event).
* **Lock epoch** (`Core::lock_epoch`): every lock – any reason, also while
  no vault is open (a system lock or suspend with `lockOnSystemLock` always
  calls `Core::lock`) –, the portable-mode move, `delete_vault` closing the
  open vault, an update (`lock_for_update`) and a vault switch start a new
  epoch – each under the state lock, in the same step that takes or replaces
  the vault (`Core::begin_lock`), and readers take the epoch under that lock
  together with what it protects: an epoch read while a vault was open is
  always older than the epoch of the lock that closes it. (A lock that
  started its epoch before waiting for the state lock let a copy holding that
  lock read the new epoch with the old vault's secret and pass the check
  below after the lock's clipboard clear.) Every unlock path (UI, extension,
  recovery key, new vault) reads the epoch together with the store before
  its key derivation (`Core::unlock_ticket`) and `Core::install_vault_since`
  refuses the vault (wiped, `locked`; the extension gets `locked`, the UI
  shows the lock screen) if the epoch changed meanwhile – so a lock that
  lands during the 0.3–1 s derivation is never undone, and nothing opens
  from the data directory's old location. Copies of a vault secret
  (`copy_secret_field`, `copy_generated`, the bridge's `copy_field` – read
  and copied through `VaultBackend::copy_field`) carry the epoch of their
  read and are dropped (`locked`) if a lock or switch happened before the
  copy; `clear_copied_secret` and the copy hold the same lock, so a copy is
  either refused or cleared by the lock.
* `import_data` and `commit_import` emit `vault://changed` after a
  successful import.
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
  files of `write_atomic` – and the `browser-extension` folder are skipped),
  re-registers the native host, writes the browser extension folder at the
  new location and restarts the bridge. Errors: `unsupported:data_dir_override` (with
  `$KEYSTEAD_DATA_DIR`), `unsupported:portable_installed` (enabling while the
  exe lives in the default data directory itself – the per-user NSIS setup
  installs to `%LOCALAPPDATA%\Keystead`), `invalid_input:target_exists` (a
  vault file with the same id – or its `<id>.keystead.attachments`
  directory – already exists at the destination; the attachment directories
  move with their vault files), `io:…`.
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

## Website icons (`keystead_core::icons`, `src-tauri/src/icons.rs`, `icons/fetch.rs`)

Logins show their website's icon. The desktop app loads missing icons
itself, **directly from the websites** (no icon service, no proxy), in the
background, and keeps them **inside the encrypted vault** – loose files would
reveal which sites the user has accounts with. `Settings.websiteIcons`
(default on) switches the loading off; stored icons are still shown. The
loader (`icons/fetch.rs`) is desktop-only; the phone (no network) only
shows the icons stored in the vault (`get_icons`).

* **Model** (`VaultData.icons`, `#[serde(default)]` – older vaults open
  unchanged): `{ [host]: IconEntry { png: string|null /* base64, 64×64 PNG */,
  fetchedAt: number /* last attempt */, failedAt: number|null /* last failed
  attempt, null after a success */ } }`. A failed refresh keeps the older
  icon. `IconEntry`'s and `ItemSummary`'s (its `icon` data URL) `Debug`
  print only the image length.
* **Host key** (`icons::site_host` / `icon_host`, same as the import's
  "site"; the UI has the same function in `src/lib/icons.ts`): the
  lower-case host of a login's first http(s) URI (scheme-less URIs count as
  `https://`), without a trailing dot and a leading `www.`, port ignored
  (`https://www.GitHub.com:443/login` → `github.com`). Logins only.
* **Due** (`icon_hosts_needing_fetch(now)`): hosts of non-trashed logins
  without an entry, whose last attempt is ≥ 30 days old, or whose last
  failure is ≥ 7 days old – missing ones first, each group alphabetical.
  Never due: hosts that must not be contacted (`is_fetchable_host`): IP
  literals (also `127.1`-style), `localhost`, names ending in `.localhost`,
  `.local`, `.localdomain`, `.internal`, `.intranet`, `.lan`, `.home`,
  `.home.arpa`, `.corp`, `.private`, `.test`, `.invalid`, `.example`,
  `.onion`, single-label names and anything but `[a-z0-9-]` labels.
* **Stored** with `set_icons` (one save per batch; `None` = failed → only
  `fetchedAt`/`failedAt` change; results for hosts no login has any more are
  ignored; not a PNG or > 48 KiB → failure). Every save also prunes icons of
  hosts no non-trashed login uses. Exports (Keystead, CSV, Bitwarden JSON)
  never contain icons and imports never bring any.
* **When** (one background thread, `IconTracker` in `Core`): 5 s after a
  vault is opened (unlock, new vault, switch – also by the extension), ~2 s
  after any successful change through the app or the browser bridge (e.g. a
  login with a new website; debounced), right away when `websiteIcons` is
  turned on, and every 24 h while the vault stays open. A run handles at most
  200 sites, 4 at a time; if more are due the next run follows 1 min later.
  Locking, switching the vault, turning the setting off or quitting cancels
  a run (generation counter) at once: no further request is sent (checked
  before every request) and open requests are dropped (checked every
  100 ms); results are written only into the vault the
  run started for (`mutate_for_page` with its id, checked under the state
  lock – never into another vault), in batches of up to 16 or every 5 s, each
  followed by `vault://icons`. Writes use the normal `conflict` → reload →
  retry path.
* **Fetch** (per site, ≤ 15 s): `GET https://<host>/` (HTML read up to
  512 KiB), the best icon link of its head (`<link rel>` with the token
  `icon` – incl. `shortcut icon` – or `apple-touch-icon[-precomposed]`,
  relative to `<base href>`/the final page URL; comments, `script`, `style`
  etc. skipped; ranking: declared size ≥ 64 px first (closest to 64), then
  32–63, then unknown, then < 32; within each PNG before ICO before others;
  `apple-touch-icon` without `sizes` = 180; SVG – by `type` or `.svg` path –
  and `data:` links other than `data:image/png;base64` are ignored), up to 3
  links tried, then `https://<host>/favicon.ico`. The HTML step is bounded
  (a hostile page must not keep the app busy): at most 1024 `link`/`base`
  tags of the head are read and the first 32 icon links collected; a
  `<base href>` or link `href` longer than 2048 bytes is ignored (`data:`
  links are bounded by the 512 KiB instead); duplicates are found through a
  hash set; and the step runs on the blocking thread pool like the image
  decoding, so the 15 s limit and cancelling apply to it. Every request:
  https only, default port, no credentials in the URL, ≤ 3 redirects – each
  again checked like the first URL –, 5 s timeout, body ≤ 512 KiB (an image
  above that is refused), no cookies, no `Referer`, the generic user agent
  `Mozilla/5.0` (no product name or version: the sites, and CDNs that see
  many of these requests, do not learn that the address runs Keystead or
  which version), `Accept` for HTML resp. images only. Nothing from the
  vault is ever sent – only the host name in the URL.
* **What this reveals**: loading icons contacts the host of every saved
  login (the DNS lookup and the TLS server name are visible on the
  network) – right after an unlock and after an import, also for sites the
  user never visits from this network. DNS resolvers, the network operator
  (e.g. a public Wi-Fi) and CDNs that serve many of the sites can therefore
  see the list of websites in the vault. Settings, README and this section
  say so; turning `websiteIcons` off stops it.
* **Address guard (SSRF)**: besides `is_fetchable_host` for the site and
  every link/redirect host, the HTTP client resolves names through
  `PublicResolver`, which drops every address that is not public (loopback,
  RFC 1918, link-local incl. `169.254.169.254`, CGNAT `100.64/10`,
  documentation/benchmark ranges, multicast/reserved, `::`, `::1`, `fc00::/7`,
  `fe80::/10`, `fec0::/10`, NAT64/6to4/IPv4-mapped forms of those, Teredo,
  ORCHID); a name with only such addresses is refused. The client connects
  only to what the resolver returns – also after a redirect – and uses no
  proxy (otherwise the proxy would resolve the name), so a website cannot
  make the app talk to the local network.
* **Proxy**: the app never goes *around* a proxy either (that would show the
  websites of the user's accounts the user's own address). Before every run
  it reads the proxy settings of the system and the environment
  (`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY`, the Windows Internet settings, the
  macOS network settings) and
  skips every site a proxy would handle – not recorded as failed, the log
  only counts them. If all due sites are skipped, the run does nothing (the
  next one follows the usual triggers, at the latest in 24 h). Networks that
  allow web access only through a proxy therefore get no icons.
  Fail closed: a PAC/auto-config script, or a proxy that is set but not
  understood (e.g. the per-protocol Windows form `http=…;https=…`), makes the
  run fetch nothing at all.
* **Image**: recognised by content (PNG, ICO – its largest frame –, JPEG,
  GIF – first frame –, WebP; SVG and BMP are refused), decoder limits 1024 px
  per side (no favicon needs more; it bounds the work per image) and 64 MiB;
  smaller than 8 px or fully transparent = no icon. Scaled into 64×64
  keeping the aspect ratio (Lanczos down, Catmull-Rom up), centred on a
  transparent square, stored as PNG. Decoding and scaling run on the
  blocking thread pool, never on the async runtime's threads (which also
  serve the app's commands); the 15 s per-site limit includes them.
* **Failures are silent**: a site without a usable icon gets `failedAt`
  (retried after 7 days) and keeps the letter avatar; the log only counts
  (`website icons: 12 loaded, 3 not available`, never host names). A run in
  which no site answered at all (only connection errors) counts as offline
  and records nothing; it is retried after 1 h, doubling up to 24 h.
* **UI**: `get_icons` → `{ host: data URL }`; avatars in the list, the detail
  and editor header and the security report show the icon of
  `iconHost(item)` on a light tile (letter avatar while missing or if the
  image does not decode). Settings → Allgemein: "Website-Icons automatisch
  laden" (hint: loaded directly from the websites, which shows DNS servers
  and the network the list of saved sites; never around a proxy – with a
  proxy set up, no icons are loaded; off = no requests for this) and
  "Gespeicherte Icons löschen" (`clear_icons`). On the very first start (no
  vault yet – the setting is global) the setup wizard's step "Tresor" offers
  the same switch below the master password, prefilled from the setting with
  a short hint (what the loading reveals; changeable under Settings →
  Allgemein) and saved with `save_settings` before `create_vault` runs, so
  an import in the next step (which starts a fetch ~2 s after it finishes)
  already respects it.
* **Browser extension**: `logins_for_url` and `search` rows carry the stored
  icon as `icon` (data URL) for the first 20 rows of a reply, if ≤ 16 KiB
  (see the bridge protocol); the popup shows it, else the letter. The
  extension never loads icons itself.

## Leaked-password check (`keystead_core::pwned`, `src-tauri/src/pwned.rs`)

The security report and the item editor can ask Have I Been Pwned's "Pwned
Passwords" range API whether a password appears in known data breaches.
This is the only network access that involves vault data, so it runs
**only when the user presses the button** – never on unlock, save, opening
the report or a timer; there is no setting that turns it on and nothing
runs in the background. It uses k-anonymity: the device sends the first
5 hex characters of the password's SHA-1, the service answers with every
known hash suffix under that prefix and its breach count (`SUFFIX:COUNT`
lines), and the match happens on the device. The service never sees a
password, a full hash, a user name or a website, and cannot tell which of
the hundreds of suffixes was meant.

* **Core** (`keystead_core::pwned`, no networking, no dependencies beyond
  the existing `sha1`, `data-encoding`, `subtle` and `zeroize`):
  `pwned_hash(password)` → `PwnedHash { prefix /*5 upper-case hex*/,
  suffix: Zeroizing<String> /*35*/ }` (the hasher – its block buffer holds
  the password bytes – is finalized in place and zeroized on drop via
  `sha1`'s `zeroize` feature, the digest after the hex encoding; stack
  copies the compiler makes are best effort. `Debug` prints only the
  prefix – prefix and suffix together are the full hash, brute-forceable
  for a weak password). `pwned_count(body, &hash)` → `Option<u64>` parses
  the answer (CRLF or LF, suffixes in any case, lines trimmed; a line whose
  suffix is not exactly 35 hex characters or whose count is not a `u64` is
  ignored), compares every line's suffix in constant time and visits every
  line, so neither a match nor its position shows in the timing; a missing
  suffix or a padding line with count 0 → `Some(0)`; a body without a
  single well-formed line → `None` – it is not a range answer (a genuine
  one has hundreds of lines), so nothing is known, never "not leaked".
  `breach_candidates(&VaultData)` groups the non-trashed logins with a
  non-empty password by identical password → one `BreachCandidate { hash,
  item_ids /*sorted*/ }` per distinct password, in a deterministic order
  (by id list); notes, cards, identities and the trash are never checked.
* **What leaves the device** (per distinct password):
  `GET https://api.pwnedpasswords.com/range/<PREFIX>` over HTTPS – the
  5 characters in the URL path, no query, no body – with the fixed headers
  `Accept: text/plain` and `Add-Padding: true` and the user agent
  `Keystead/<version>` (like the updater; the service answers without one
  too). `Add-Padding` makes the service pad every answer to ~800–1000
  lines (~80 KB), so the size of an answer says nothing about the prefix.
  No cookie is ever sent (the client has no cookie store; the one
  Cloudflare sets is dropped), no `Referer`, no credentials. Nothing from
  the vault but those 5 characters is sent – not the item, not the user
  name, not the website.
* **Client** (a `reqwest::Client` built per command like
  `update::probe_client`, incl. installing the ring provider that the
  `rustls-no-provider` feature needs – not cached: reqwest takes the system
  proxy settings when the client is built, so a cached client would keep
  the settings of its first use while the proxy rule below reads the
  current ones): 10 s timeout for the connect and for the whole request,
  `referer(false)`, redirects not followed (a 3xx is an error status),
  answer capped at 512 KiB (`Content-Length` checked first, then while
  reading – also for an answer without a length; above it → `io:answer
  larger than 512 KiB`). A non-2xx status is `io:HTTP <status>` – no
  automatic retry, also for 429 (the range API has no documented rate
  limit; checked live 2026-10-10: 200 without a user agent, CRLF,
  upper-case suffixes, padding, a Cloudflare cookie) –, a network error
  `io:<detail>`, where the detail is reqwest's message without the URL (it
  would carry the prefix) plus its cause chain (e.g. the DNS or TLS error).
  A 2xx answer whose body holds no `SUFFIX:COUNT` line (`pwned_count` →
  `None`: a block or captive-portal page, an empty body) is no answer and
  is treated as the network error `io:not a range answer` – never as a
  clean result.
* **Proxy**: the client uses the system proxy (reqwest's `system-proxy`
  feature: `HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY`, the Windows Internet
  settings, the macOS network settings). This differs from the icon
  fetcher, which never uses a proxy, on purpose: its reasons – hosts taken
  from vault data (SSRF), the user's own address shown to the websites of
  their accounts – do not apply to one fixed public host, and a proxy sees
  only `CONNECT api.pwnedpasswords.com:443`, never the prefix. The rule
  "never *around* a proxy" is kept the same way: before the first request
  `icons::proxy_setup_unclear(&icons::proxy_matcher())` is evaluated (on
  the blocking pool – it reads the environment and, on Windows, the
  registry), and a proxy configuration that exists but that reqwest would
  not use (a PAC/auto-config script, the per-protocol Windows form
  `http=…;https=…`) **fails closed** with `unsupported:pwned_proxy` – nothing
  is sent. Client and rule read the settings at the start of the same
  command (see "Client"), so a proxy switched on or off while the app runs
  is seen by both. A vault check without candidates skips the proxy check
  too (nothing would be sent). Networks that allow web access only through a
  proxy the matcher cannot evaluate therefore get no check; the UI text
  says so.
* **Never stored, never logged**: the result is not cached and not written
  into the vault or the settings (a stored count would travel into Keystead
  exports) and does not survive a lock; the full SHA-1 exists only as
  `Zeroizing` strings inside the core during the call; the `password`
  argument of `check_pwned_password` is moved into a `Zeroizing<String>` at
  once; the single log line carries counts only (`pwned check: 12 distinct
  passwords, 2 leaked items, 0 not checked`), never prefixes, hashes or
  item names; error details never contain the URL.
* **Commands** (see the command table; both count as user activity):
  * `check_pwned_password(password)` → `number`: hash in the core, proxy
    check, one request, `pwned_count`. Needs no vault (the editor uses it
    for a new item too).
  * `check_pwned_vault(pageVaultId)` → `PwnedReport { checkedAt /*Unix
    ms*/, checked /*distinct passwords*/, leaked: [{ itemId, count }]
    /*count desc, then itemId; an item at most once*/, failed /*distinct
    passwords not checked*/ }` in three steps. (1) On the blocking pool,
    under the state lock: `page_vault(pageVaultId)` (`locked` if another
    vault is open), `breach_candidates` and the lock epoch (see "Lock
    epoch"); then, lock released, the proxy check. (2) Without the lock:
    one request per **distinct prefix** (passwords sharing a prefix share
    the answer, so never more than one request per distinct password), at
    most 4 at a time (`tokio::task::JoinSet` + `Semaphore`, like
    `icons::fetch_all`); the coordinator polls every 100 ms for a lock, a
    vault switch (epoch change) or app exit → `locked`, open requests
    dropped at once (`JoinSet::shutdown`), no further request sent. The
    first HTTP error ends the run with `io:HTTP <status>` (the task that
    got it records the status before releasing its permit, so waiting
    requests stop without connecting – at most 4 requests reach a server
    that answers 429); a network error **before any answer** ends the run
    with `io:<detail>` (probably offline); one **after** an answer only
    leaves that password's items unchecked (`failed` + 1; the UI notes
    it). A 2xx without a listing (`io:not a range answer`) counts as a
    network error in both places. (3) The epoch is compared again –
    changed → `locked`, result
    discarded – and the counts are mapped to the candidates' item ids. A
    vault without candidates answers `{ checked: 0, leaked: [], failed: 0 }`
    without any request.
* **Errors the UI maps** (`usePwnedErrorText` in `src/state/breaches.tsx`):
  `io:…` → `pwned.errorNetwork` ("Have I Been Pwned ist nicht erreichbar:
  {detail}", like `useUpdateErrorText`), `unsupported:pwned_proxy` →
  `error.unsupported.pwned_proxy` (the system's proxy settings could not be
  evaluated, Keystead never goes around a proxy, nothing was sent), `locked`
  → the unlock screen like every other page command, the rest via
  `errorText`.
* **UI** (strings under `pwned.*` in `src/i18n/de.ts` / `en.ts`). The
  result lives in `BreachesProvider` (`src/state/breaches.tsx`, mounted in
  `App.tsx` inside `IconsProvider`, keyed by the vault id): page memory
  only – a lock (page reload) and a vault switch (remount) drop it. Per
  leaked item it pins the count to the item's `updatedAt` at the start of
  the check; `breachCount(item)` answers only while that still matches, so
  an edit or save of the item (e.g. a new password) expires its entry
  without a re-check. One run at a time.
  * **Security report** (`HealthPage`, `PwnedCard`): the card "Geleakte
    Passwörter" comes first. Idle: a description (what the check does, that
    only 5 characters of a hash leave the device) and the primary button
    "Geleakte Passwörter prüfen". Checking: spinner, "Frage Have I Been
    Pwned an …", button loading; previous rows stay visible. The verdict –
    tone, count badge, text – follows the **report**, not the rows still
    listed. Result with leaks (danger): the report's count, the leaked
    items of the report that still exist outside the trash and are
    unchanged since, in the report's order (count desc), each with "in {n}
    Datenlecks gefunden" (locale number format) and opening the item, "Alle
    n anzeigen" after 4, "Erneut prüfen" and "Zuletzt geprüft:
    <checkedAt>"; the note "{n} geleakte Einträge wurden seit der Prüfung
    geändert oder gelöscht – prüfe erneut." for the leaked items that left
    the list (an edit, the star or the trash bump `updatedAt`; it becomes
    the text when none is left – the card stays red, never turns green on
    a report that found a leak), and the note "{n} Passwörter konnten nicht
    geprüft werden" when `failed` > 0. Clean (ok, only when `failed` = 0):
    "{n} Passwörter geprüft – keins in einem bekannten Datenleck." with
    n = `checked` − `failed`, or "Keine Passwörter zu prüfen.". Partly
    checked (warning: no leak found, `failed` > 0): the same text with the
    answered count and the "konnten nicht geprüft werden" note – or, when
    nothing was answered, that note as the text. Error (danger,
    `role="alert"`): the mapped text, the button retries. The summary shows
    the chip "{n} geleakt" once a report exists, from the report's count
    (red, yellow with 0 while `failed` > 0, else green). Before **every**
    vault-wide run
    – also "Erneut prüfen" – the dialog "Geleakte Passwörter prüfen?"
    (`PwnedConfirmDialog`) says exactly what leaves the device: the first
    5 characters of each password's SHA-1, to api.pwnedpasswords.com over
    HTTPS, nothing else – no password, no user name, no website; the
    service answers with every match for those 5 characters and cannot
    tell which password was meant; the comparison happens on the device
    and nothing is stored. "Abbrechen" / "Jetzt prüfen" and the checkbox
    "Nicht mehr fragen", which is remembered only in `localStorage` as
    `keystead.pwnedConfirmed` = `"1"` (`localPrefs`) – no backend setting,
    no reset in the UI (clearing the webview's site data brings the dialog
    back).
  * **Item view** (`LeakWarning`): a `callout-danger` "Dieses Passwort ist
    in Datenlecks aufgetaucht." with the count and "Passwort ändern" (opens
    the editor) on a non-trashed login the context knows as leaked.
  * **Item editor** (`LeakCheck`): the link button "Auf Leaks prüfen" under
    the strength meter calls `check_pwned_password` with the current
    password (disabled while empty or running); the answer is shown inline –
    "In {n} Datenlecks gefunden – bitte nicht verwenden" or "Nicht in
    bekannten Datenlecks" – or the mapped error; it belongs to the password
    it was asked for: any change hides it, restoring the same password shows
    it again.
  * Not in the TUI (no HTTP dependency) and not in the browser extension;
    the breach count is not part of `HealthReport` or its score (the core
    stays offline and synchronous).
* **Tests**: the core's unit tests use the known vector SHA-1("password")
  = `5BAA6` + `1E4C9B93F3F0682250B6CF8331B7EE68FD8` with canned bodies
  (CRLF/LF, lower case, padding, garbage lines, bodies without a listing);
  the desktop tests run against a local plain-HTTP server (request shape
  and headers, no cookie or referer, error mapping, 512 KiB cap with and
  without `Content-Length`, a 2xx without a listing, prefix sharing,
  concurrency and the stop at the first HTTP error, a lock during a run,
  the proxy fail-closed). The one test against the live service
  (`pwned::tests::live_range_api_knows_password`) is `#[ignore = "needs the
  internet"]` – CI never goes online.

## Attachments (`keystead_core::attachments`, `src-tauri/src/attachments.rs`)

Items can carry files – an ID scan, a contract, a PDF of recovery codes.
The vault payload holds only the bookkeeping (name, size, date and the
blob's key); the bytes live as one encrypted file each **next to the vault
file**, never in plain text on disk. The browser bridge and the extension
are not involved (`ItemSummary` is unchanged).

* **Model** (`VaultData.attachments`, `#[serde(default)]` – older vaults
  open unchanged): `{ [attachment id (UUID v4)]: AttachmentEntry { itemId,
  name /* trimmed, 1–200 chars */, size /* plaintext bytes */, createdAt,
  key /* base64 of the blob's own random 32-byte key */ } }`. `VaultItem`
  is unchanged: attachments act on the saved item, never through the
  editor. `AttachmentEntry`'s `Debug` prints the key's length only;
  `AttachmentInfo { id, itemId, name, size, createdAt }` (camelCase) is the
  key-free view for UI and TUI. Names are personal data like notes (wiped
  with them), not secret.
* **Storage**: the side-car directory `<vault path>.attachments/`
  (`format::sibling`, i.e. `vaults/<id>.keystead.attachments/`), created
  with the first attached file; one blob `<attachment id>.bin` per
  attachment: `KSAT1` + 24-byte nonce + XChaCha20-Poly1305 ciphertext of
  the file bytes, AAD `keystead:v1:attachment:<vault id>:<attachment id>`
  (see "Vault file format"). Because every blob has its own key in the
  payload, a key rotation of the vault re-encrypts nothing, deleting an
  attachment = removing its entry (the file is deleted best effort
  afterwards), and a blob without its entry is worthless.
* **File version**: the vault file is version 2 while the map is not empty
  and version 1 again once it is empty (`format::version_for`). Mixed
  versions: builds before beta.9 refuse a vault with attachments
  (`unsupported:vault format version 2`) instead of silently dropping the
  keys on their next save; beta.9 (reader for 1 and 2, `UnknownFields`)
  opens it and carries the map it does not know through its saves.
* **Limits** (checked in this order, before anything is written): the item
  exists (`not_found`) and is not in the trash (`invalid_input:item_trashed`);
  fewer than `ATTACHMENTS_PER_ITEM_MAX` (10) attachments
  (`invalid_input:attachments_too_many`); the trimmed name has 1 to
  `ATTACHMENT_NAME_MAX_CHARS` (200) characters
  (`invalid_input:attachment_name_required` / `attachment_name_too_long`);
  at most `ATTACHMENT_MAX_BYTES` (20 MiB)
  (`unsupported:attachment_too_large`). No total per vault.
* **Lifecycle** (`UnlockedVault`): `add_attachment` seals the bytes under a
  fresh key, writes the blob (`write_atomic`, no `.bak`, the directory
  created if missing) **before** the payload mutation and records the
  entry in one save; if that save fails – `Conflict` included – the blob is
  removed again and nothing changed, so `reload_if_changed` + retry (the
  desktop's one `mutate_for_page` retry) starts clean. `remove_attachment`
  drops the entry in one save, then deletes the blob best effort.
  `read_attachment` decrypts into a `Zeroizing` buffer – `not_found` for an
  unknown id or a missing file, `corrupt:…` for a tampered, swapped or
  foreign blob; a file larger than 20 MiB + 45 bytes is never read whole.
  Every save prunes the entries of items that no longer exist
  (`delete_item`, `empty_trash`; trashed items keep theirs) next to
  `icons::prune_icons` and deletes their blobs best effort after the
  write. `sweep_attachments` – run by `VaultStore` after every create /
  unlock, best effort – removes `*.bin` files no entry references and
  leftover `*.tmp` files, but only those whose mtime is at least 1 h old
  (`SWEEP_MIN_AGE`): another process (the TUI) may be in the middle of
  attaching. `VaultStore::delete_vault` removes the directory after the
  vault file, best effort with a short retry (`format::remove_with_retry`,
  like the `.bak` and temp files – on Windows a scanner may still hold a
  fresh blob open): the vault counts as deleted once its file is gone, so
  the call succeeds and the desktop closes the vault even if the folder
  resists and stays behind (worthless without its keys; delete it by
  hand). Exports (Keystead, CSV, Bitwarden JSON) never contain attachments
  and imports never bring any – a full backup is the data directory (vault
  file + folder). Portable mode moves the folder with its vault file and
  refuses a target where such a folder already exists
  (`invalid_input:target_exists`).
* **Desktop commands** (all take `pageVaultId` – another open vault or
  none → `locked` –, all count as user activity, results wiped after
  serialization like `list_items`; the phone registers only the read-only
  `reveal_attachment`, the rest is `#[cfg(desktop)]` – attachments stay
  view-only on the phone in 2b, `AppInfo.attachmentsReadOnly`):
  * `add_attachment(itemId, path)` † → `ItemListEntry`: the page passes the
    path the native open dialog chose (a blank one →
    `invalid_input:path_required`); the backend checks the page's vault
    first, then the source – `invalid_input:attachment_source` for a path
    no dialog produces (relative, `.`/`..` components) or a file inside
    the data directory (the store's and `paths::data_dir()`; the path or
    its deepest existing ancestor resolved with `canonicalize`): the vault
    files, their blobs and `settings.json` are the app's own, and a page
    must not be able to copy them out through `save_attachment` or read
    them through `reveal_attachment` without the master password
    `export_data` demands –, takes the file name component as the name
    (`attachment_name_required` without one), checks the size from the
    file's metadata **before** reading (`unsupported:attachment_too_large`;
    missing → `not_found`; a directory or other non-regular file → `io:…`),
    reads the bytes outside the state lock into a `Zeroizing` buffer with a
    `take(limit + 1)` guard, then calls `add_attachment` through
    `Core::mutate_for_page` (the core's cleanup makes its single retry
    safe) and answers the item's redacted entry.
  * `save_attachment(attachmentId, path)` † → `null`: under the state lock
    it checks the page's vault and the target –
    `invalid_input:attachment_target` for a path no dialog produces
    (relative, `.`/`..` components: a `..` behind a folder that does not
    exist yet is opaque to `canonicalize`, but `write_atomic`'s
    `create_dir_all` would make it resolve), for a path inside the vaults
    directory (the store's and `paths::vaults_dir()`, textually and with
    the path or its deepest existing ancestor resolved – symlinks, case and
    8.3 variants included) or with the `.keystead` extension, so a saved
    attachment never overwrites or poses as a vault file –, decrypts the
    bytes and reads the lock epoch; outside the lock it re-checks the epoch
    right before `write_atomic` to the dialog path (a lock or vault switch
    in between → `locked`, nothing written) and remembers the file like
    `export_data`, so `show_export` ("Im Ordner anzeigen") reveals it. No
    plaintext temporary files, no "open with the default app". The backend
    cannot tell a dialog's path from one the page made up (the page opens
    the dialogs via `@tauri-apps/plugin-dialog`); these checks and
    `check_source` are the barrier.
  * `remove_attachment(attachmentId)` † → `ItemListEntry` (`not_found` for an
    unknown id).
  * `reveal_attachment(attachmentId)` → `string`: a `data:<mime>;base64,…`
    URL for the in-app preview – only PNG, JPEG, GIF and WebP, recognised
    by their magic bytes (`image_mime`, hand-written), and at most
    `PREVIEW_MAX_BYTES` (8 MiB, checked against the entry's size before
    reading), anything else `unsupported:attachment_preview`. The only
    command that brings attachment bytes into the webview; the UI treats
    the URL like a revealed secret (see "Secrets and the webview").
* **Redaction**: `ItemListEntry.attachments: AttachmentListEntry[]`
  (`{ id, name, size, createdAt }`, oldest first) in every answer that
  carries an entry (`list_items`, `save_item`, `set_favorite`,
  `add_attachment`, `remove_attachment`); the blob key never leaves the
  core, names are wiped with the entry like notes.
* **UI**: a section "Anhänge" in the detail pane after the custom fields –
  per row a type icon, the name, size (`formatBytes`) and date, and the
  actions "Vorschau" (eye; offered for names ending in
  `.png/.jpg/.jpeg/.gif/.webp`, the backend's magic-byte check decides – a
  mismatch shows the `attachment_preview` toast; the image is rendered
  contained, not draggable, without context menu, and hidden again like a
  revealed secret), "Speichern unter …" (native save dialog with the name
  suggested, then `save_attachment`; the success toast offers "Im Ordner
  anzeigen") and "Entfernen" (confirm dialog, then `remove_attachment`);
  below the list "Datei anhängen …" (native open dialog, then
  `add_attachment`; disabled with a hint at 10) and the limits. A paperclip
  chip with the count sits in the detail header. Trashed items show the
  list read-only (preview and save, no add or remove; hidden when empty);
  so does the phone for every item (`AppInfo.attachmentsReadOnly`: preview
  only, no "Speichern unter …").
  The editor has no attachments. The answers replace the item's entry in
  the list (no refetch); `locked` → unlock screen, `not_found` / `conflict`
  → `list_items` is refetched, other errors are toasts. The export dialog
  notes under the format cards that no export contains attachments;
  Settings → Tresor shows "Anhänge: N Dateien, <size> insgesamt" (summed
  client-side over the list entries, trash included), the backup hint and
  "Ordner öffnen" (`open_data_dir`). The TUI shows "Anhänge: N" under the
  tags line (only when N > 0) and nothing else.
* **Mock backend**: see the mock paragraph under "Desktop backend ↔
  frontend" (`src/lib/demo/mockAttachments.ts`).

## In-app updates (`src-tauri/src/update.rs`)

Desktop only – the phone has no updater (a newer APK is installed over the
old one, see "Mobile (Android)"). `tauri-plugin-updater` 2 +
`tauri-plugin-process` 2. The UI never calls the plugins' JS APIs (no `updater:*`/`process:*` permission in
`capabilities/default.json`); it uses `check_update` / `pending_update` /
`install_update` and the `update://*` events.

* **Signing**: every update is a minisign (Ed25519) signed file.
  `tauri.conf.json` → `plugins.updater.pubkey` holds the public key
  (key id `0DFE3A3BD1834D53`); the private key exists only as the repository
  secret `TAURI_SIGNING_PRIVATE_KEY` (empty password) and is never committed.
  The plugin verifies the downloaded bytes against the signature from
  `latest.json` before anything is written or run. `requireSignedVersion:
  true`: the signature's trusted comment must name exactly the version
  `latest.json` announces (the Tauri CLI ≥ 2.12 binds it when signing), so a
  forged manifest cannot pair a newer version number with an older signed
  setup. Downgrades are never offered (semver `>` only).
* **Channels** (endpoint chosen at runtime, `UpdaterExt::updater_builder()
  .endpoints(…)`, nothing in `tauri.conf.json`):
  * `beta` → `https://github.com/CedrickGD/Keystead/releases/download/updater-beta/latest.json`
    (a fixed pre-release that CI updates with every published build, betas
    and stable releases);
  * `stable` → `https://github.com/CedrickGD/Keystead/releases/latest/download/latest.json`
    (GitHub's "latest" = newest non-pre-release).
  A channel without `latest.json` (404/410) means "nothing available", not
  an error.
* **latest.json** (static format): `{ version, notes, pub_date (RFC 3339),
  platforms: { "windows-x86_64": { signature, url } } }` – `signature` = the
  content of the `.sig` file `tauri build` writes next to the NSIS setup,
  `url` = the setup asset of the versioned release
  (`…/releases/download/v<version>/Keystead-<version>-windows-setup.exe`).
  The updater looks up `windows-x86_64-nsis`, then `windows-x86_64`.
* **Versions** are semver; every CI build has its own (`2.0.0-beta.<run>`,
  see "Releases & in-app updates"), so `2.0.0-beta.4 < 2.0.0-beta.5 < 2.0.0`.
  The app's version is `tauri.conf.json` → `version` (CI sets it per build);
  the About page and the bridge `status` show `KEYSTEAD_VERSION_LABEL` (set
  by CI to the same value) or that version.
* **canInstall** (`update::can_install`, computed once): Windows – the exe
  is an installed copy: its folder is the per-user NSIS install folder
  (`%LOCALAPPDATA%\Keystead`, `installMode: currentUser`) or the folder the
  installer's uninstall entry names – `HKCU\Software\Microsoft\Windows\
  CurrentVersion\Uninstall\Keystead` (`UNINSTKEY` = product name in the Tauri
  NSIS template), values `InstallLocation` (quoted) and `UninstallString`
  (`update::is_installed_copy`, compared case-insensitively). Anything else is
  the portable build: it never runs the setup (that would install a second
  copy) and shows a download link. Linux: only an AppImage
  (`$APPIMAGE`); macOS: never.
* **Check** (`update::check`): fetches the channel's `latest.json` (30 s
  timeout). The version comparator records the release before the platform
  entry is looked up, so a release without a build for this OS is still
  reported (`available`, `canInstall: false`). The plugin reports every
  non-2xx answer as "no release" and drops the status, so the app then
  requests `latest.json` once more (`update::missing_release`): 404/410 → not
  available; any other status (403/429 rate limit, 5xx) → `io:HTTP <status>`
  (e.g. `io:HTTP 503 Service Unavailable`). Network errors and an unreadable
  manifest → `io:<detail>`. The UI shows `io:` as "Der Update-Server ist
  nicht erreichbar: <detail>"; the background check only logs it.
* **Background check**: 15 s after start, then every 6 h, while
  `updateCheck` is on; turning it on or changing the channel checks at once.
  A version not announced before → `update://available`; errors are only
  logged (`[keystead] update check failed: …`). The last result is kept
  for `pending_update`; changing the channel or turning the check on drops
  it and lets the next find be announced again.
* **Install** (`install_update`): one at a time. Windows: while a terminal
  UI (`Keystead.exe --cli`, detected by its mutex `Local\Keystead-TerminalUI`)
  runs in the session → `invalid_input:update_tui_running` ("Bitte zuerst die
  Terminal-Version von Keystead beenden …"), checked before the download and
  again right before the vault is closed – the passive setup would end it
  without asking (Restart Manager), losing its unsaved input and leaving a
  copied secret on the clipboard. Checks again, downloads
  (`update://progress`), verifies, emits `update://ready`, then
  `Core::lock_for_update`: from now on no vault can be opened
  (`Core::install_vault_since` – the one path of every unlock, recovery-key unlock
  and new vault – answers `invalid_input:update_in_progress`, checked under
  the state lock), the browser bridge stops (waiting pairings are denied),
  the vault is locked (wiping it, no `vault://locked` – the banner shows the
  restart) and a copied secret still in the clipboard is cleared. Then it
  installs: on Windows the plugin writes the setup, runs our `on_before_exit`
  hook (set in `update::build_updater`; it replaces the plugin's default, so
  it calls `cleanup_before_exit` itself – tray icon, resources – after
  repeating `lock_for_update`), starts the setup with `/P /UPDATE /R /ARGS
  <current args>` (`installMode: passive`: progress window, no questions) and
  ends the process with `exit(0)` – no `RunEvent::Exit`, so `Core::shutdown`
  does not run; `lock_for_update` covers what matters of it. The setup closes
  leftovers (native host processes) via the Restart Manager, replaces the
  files in place and restarts Keystead. AppImage: the file is replaced and
  the app restarts (`AppHandle::request_restart`, i.e. `RunEvent::Exit` →
  `Core::shutdown` first). If the installation fails after the vault was
  closed (also when the setup cannot be started),
  `Core::resume_after_failed_update` allows unlocking again and restarts the
  bridge (if browser integration is on), and `vault://locked {manual}` sends
  the UI back to the unlock screen.
  Vault files, settings and the extension folder are untouched by the setup
  (the NSIS uninstaller in update mode only replaces program files).
* **UI** (`src/state/update.tsx`, `src/components/UpdateBanner.tsx`): banner
  "Keystead <version> ist verfügbar" with [Jetzt aktualisieren] (portable:
  [Herunterladen] → `releaseUrl` in the system browser), [Später] (hides that
  version for the window's lifetime, `sessionStorage`) and "Was ist neu?"
  (the notes); progress bar; "Keystead wird neu gestartet …". Settings → Über
  Keystead: installed version, "Nach Updates suchen" with the result,
  "Automatisch nach Updates suchen", "Update-Kanal", and "Manuell
  herunterladen": the releases page (`links.ts` `RELEASES_URL`) as a button
  that opens it in the system browser plus a copy button for the link – for
  networks that block Keystead's own update check but not the browser.

## Browser extension deployment (`src-tauri/build.rs`, `src-tauri/src/extension.rs`)

* `build.rs` embeds every file of `extension/chrome` (hidden files skipped)
  as `include_bytes!` pairs; the directory is a `rerun-if-changed` input.
  `extensionVersion` = `version` of the embedded `manifest.json`
  (1–4 dot-separated integers ≤ 65535; CI sets `<x.y.z>.<run number>`).
* On every start (background thread), after a portable-mode switch and in
  `open_extension_dir`, the app makes `<data_dir>/browser-extension` hold
  exactly the embedded files (missing, other version, or a file changed or
  deleted → rewrite; additional files of the user are ignored while it is
  current): write a temporary sibling `.browser-extension.<id>.tmp`, rename
  the old folder to `.browser-extension.<id>.old`, rename the new one into
  place, delete the old one. Renames and writes are retried with growing
  pauses (Windows: a browser or virus scanner may hold a file); if the old
  folder cannot be moved, its files are overwritten in place. Leftover
  `.browser-extension.*.tmp|.old` folders of an interrupted run are removed
  on the next run; nothing else is touched (a symlinked folder is replaced,
  never followed). Errors are logged; the app works without the folder.
* The portable-mode move skips the top-level `browser-extension` folder and
  these work folders (the browser may still use the old copy); the app
  writes the folder again at the new location, and the extension points the
  user there (see below). Disabling portable mode deletes `Keystead-Data`
  with its copy, as before.
* Updating the extension: the bridge `status` tells paired clients the
  delivered version and folder – only once this process has written the
  folder (or found it current) at the current data directory
  (`extension::deployed_version`); while the startup thread is still writing
  it, after a failed write and after a portable-mode switch until the new
  folder is written, both are `null`, so the extension never reloads itself
  from an old, half-written or missing folder. `manifest.json` is written
  last (also by the in-place fallback). The extension's service worker compares it
  with `chrome.runtime.getManifest().version` (numeric, `lib/version.js`);
  newer → `chrome.runtime.reload()` after 2.5 s, at most once per target
  version (`chrome.storage.local.extensionReloadedFor`), never while a
  pairing waits. Still older after that (loaded from another folder, e.g. the
  ZIP's `browser-extension`) → the popup shows "Neue Plugin-Version verfügbar
  (<version>)" (collapsed, "Details": the folder with a copy button and how to
  load it once); while the reload is pending it says so.
* Settings → Browser-Integration, step 2: load the extension unpacked from
  that folder, with "Ordner öffnen" (`open_extension_dir`) and "Pfad kopieren".
  Just in case the folder does not work, a link "Browser-Erweiterung
  herunterladen" opens the release's ZIP in the system browser:
  `https://github.com/CedrickGD/Keystead/releases/download/v<version>/Keystead-<version>-browser-extension.zip`
  for release versions (`x.y.z`, `x.y.z-beta.N`, `-rc.N`, `-alpha.N`;
  `src/lib/links.ts`), `…/releases` for development builds (Vite dev server
  or any other version string).
* The setup wizard's last step ("Dein Tresor ist bereit") shows an optional,
  compact card "Browser-Erweiterung einrichten" below the recovery-key offer:
  the folder path, "Ordner öffnen" and "Anleitung" – a toggle; once the
  wizard is finished the main window opens on Settings → Browser-Integration
  (`src/lib/startView.ts`). Its first step ("Tresor") carries the switch
  "Website-Icons automatisch laden" on the very first start only (see
  "Website icons").

## Releases & in-app updates (CI, `.github/workflows/build.yml`)

* **test** (Linux): `cargo fmt --check`, clippy `-D warnings`, `cargo test`,
  `npm run build`, `node --check` of the extension and `.github/scripts`,
  `node --test extension/tests/*.test.mjs .github/scripts/*.test.mjs
  apps/desktop/tests/*.test.mjs`.
* **windows**: `.github/scripts/release-version.mjs --write` first: version
  `v<semver>` tag → that version, otherwise `<tauri.conf.json version>-beta.
  <run number>` (e.g. `2.0.0-beta.57`), written into `tauri.conf.json`
  (`version`: app, NSIS setup – `VIProductVersion` drops the pre-release,
  `ProductVersion`/`DisplayVersion` keep it – and updater); the extension
  manifest gets `<x.y.z>.<run number>` (also for tags, so a stable release is
  newer than the betas before it for Chrome too). Then `tauri build`. Only
  builds that publish (the **release** job's condition: tags `v*`, pushes
  with `[release]`) get `TAURI_SIGNING_PRIVATE_KEY` (secret) and an empty
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` → `…-setup.exe` + `.sig`. Every other
  build (branch pushes, pull requests, `workflow_dispatch`) never sees the
  secret and builds with `--config` `bundle.createUpdaterArtifacts=false` –
  no `.sig`/`latest.json`, so no unreviewed build is a valid update. A
  publishing build without the secret does the same with a `::warning::`.
  Third-party actions (`dtolnay/rust-toolchain` with `toolchain: 1.97.0`,
  `Swatinem/rust-cache`, `softprops/action-gh-release`) and the `actions/*`
  ones are pinned to full commit SHAs (version in a comment); bump them
  deliberately.
  Packaging renames the setup to `Keystead-<version>-windows-setup.exe`
  (+ `.sig`) and writes `latest.json` (`.github/scripts/latest-json.mjs`,
  notes = the commit subject without `[release]`) pointing at exactly that
  asset name. The portable ZIP and the extension ZIP stay.
* **android** (Ubuntu, every push and pull request like **windows**; details
  in "Mobile (Android)" → "Build, CI and signing"): `release-version.mjs
  --write` (the same version; `versionCode` = run number into
  `tauri.android.conf.json`), clippy for `aarch64-linux-android`, `npx tauri
  android build --apk --target aarch64 --ci` – with the release keystore
  from the secrets `ANDROID_KEYSTORE_BASE64`/`ANDROID_KEYSTORE_PASSWORD`/
  `ANDROID_KEY_ALIAS` only on builds that publish (the **release** job's
  condition; a missing secret fails them), else signed with the debug key –,
  then the gates of `.github/scripts/android-apk.mjs verify` (signature –
  on publishing builds exactly the release certificate
  `ANDROID_RELEASE_CERT_SHA256` –, 16 KB alignment, package, versionName/
  versionCode, minSdk, no `INTERNET`, not debuggable, arm64) and the
  artifact `Keystead-<version>-android` with
  `Keystead-<version>-android.apk`.
* **release** (tags `v*`, or pushes whose commit message contains
  `[release]`; needs **test**, **windows** and **android**, and fails if the
  two builds report different versions): versions with a `-suffix` are published as **pre-release**
  with `make_latest: false` (so the stable channel – GitHub's "latest"
  release – never sees a beta); a tag without suffix (`v2.0.0`) becomes a normal
  release marked latest. All assets incl. `.sig`, `latest.json` and the
  APK are uploaded (the notes list them, the APK with its certificate's
  SHA-256). Then the beta channel: the pre-release `updater-beta` (created if
  missing, kept pre-release and not latest, title "Keystead Updater
  (Beta-Kanal)", technical description) gets the new `latest.json` – unless
  it already offers a newer version (re-runs never move the channel back).
  `updater-beta` does not match the `v*` tag trigger.
* Required repository secret: **`TAURI_SIGNING_PRIVATE_KEY`** (content of the
  private key file generated with `tauri signer generate` with an empty
  password; its public key is the `pubkey` above). Losing it means installed
  copies can no longer be updated in-app (a new key needs a manual update).
  For the APK: **`ANDROID_KEYSTORE_BASE64`** (the release keystore,
  base64), **`ANDROID_KEYSTORE_PASSWORD`**, **`ANDROID_KEY_ALIAS`**
  (`keystead`) – the keystore lives in the private repo `CedrickGD/keys`;
  losing it means installed phone apps can never be updated again.
* The `versionCode` is the run number of this workflow: **never rename
  `build.yml`** (a new workflow counts from 1 and every installed app would
  refuse the next APK). `discord-notify.yml` links a release's Windows
  setup (and the APK on a line of its own), not its first asset.
* **Release checklist** (stable version `X.Y.Z`):
  1. Tag the commit `vX.Y.Z` and push the tag → stable release (latest),
     also offered on the beta channel.
  2. Right after that, set `version` in `apps/desktop/src-tauri/tauri.conf.json`
     to the next version (e.g. `X.Y.(Z+1)`) and commit it. Branch builds are
     `<that version>-beta.<run>`; without the bump the next betas would be
     `X.Y.Z-beta.<run>`, which sorts *below* the released `X.Y.Z` – no
     updater would offer them (the beta channel keeps `X.Y.Z`). The windows
     job checks this (`release-version.mjs --check-tags`: a `v*` tag above
     the build's version) – a `::warning::`, and an error for builds that
     publish (`[release]`), so no such pre-release is published.
  The same holds for a release-candidate tag (`vX.Y.Z-rc.1` sorts above
  `X.Y.Z-beta.<run>`).

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
  the bridge stays off. Connection attempts (the host's 2 s, the 500 ms probe
  of a taken name) go through `interprocess`'s typed named-pipe API
  (`socket::connect_named_pipe`), because its local-socket layer (2.4.4)
  ignores the wait mode on Windows and waits unboundedly: a pipe instance
  that holds a connection the server has not accepted yet (a client that
  connected and hung up – "dead on arrival") would then block every further
  attempt until the app accepts – forever on a frozen app, hanging the host
  and the browser.
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
| `status` | no | – | `{ appVersion, generatorVersion: number, paired: bool, unlocked: bool, vaultName: string\|null, vaultId: string\|null, extensionVersion: string\|null, extensionDir: string\|null }` (`generatorVersion`: the generator interface, 2; absent in apps before the generator rework) |
| `pair` | no | `clientName` (e.g. "Chrome – DESKTOP-1"), `code` (6 digits shown in the popup) | `{ clientId, token }` once the user approved in the app (the request blocks up to 120 s) |
| `list_vaults` | yes (locked or unlocked) | – | `{ vaults: { id, name }[], currentVaultId: string\|null, lastVaultId: string\|null }` – sorted by name (case-insensitive); `currentVaultId` = the unlocked vault |
| `unlock` | yes | `password`, `vaultId?` | `{ vaultName, vaultId }` – opens `vaultId` (switching if another vault is open), without it the app's last used vault |
| `lock` | yes | – | `null` |
| `focus_app` | no | – | `null` (shows/raises the window) |
| `logins_for_url` | yes, unlocked | `url` | `ItemSummary[]` (optional `icon`, see below) |
| `search` | yes, unlocked | `query` | `ItemSummary[]` (max 50; optional `icon`) |
| `get_login` | yes, unlocked | `itemId` | `{ id, name, username, password, totp: TotpCode\|null, uris: string[] }` |
| `get_totp` | yes, unlocked | `itemId` | `TotpCode` |
| `generate_password` | yes | `options?` (GeneratorOptions partial), `remember?: bool` | `string` – every option the request does not set comes from the app's default generator preset (see below); stored in the generator history if unlocked – unless `remember: false`: a preview, not stored, no user activity. The current extension sends `remember: false` on every request (extensions up to 2.0.0 only for page suggestions): the popup's own options in full, or no `options` / `{}` for the default preset |
| `remember_generated` | yes, unlocked | `password` (1–1024 chars) | `null` – adds a password the user used from a preview (extension: copied in the popup, filled into a page from the popup / context menu / shortcut, a suggestion taken, saved in a new login) to the open vault's generator history; the newest entry is not added twice |
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
* `ItemSummary` = `{ id, type, name, subtitle, uri, favorite, hasTotp,
  folderId, icon? }` (`keystead_core::model::ItemSummary`). `icon` is the
  stored website icon as `data:image/png;base64,…`; the desktop app adds it
  only to the first 20 rows of a `logins_for_url` / `search` reply and only
  if it is ≤ 16 KiB (the reply to the browser is capped at 1 MiB). Without an
  icon the key is absent; older apps never send it and clients treat it as
  optional (the popup then shows the letter avatar). The extension popup's
  CSP (`script-src 'self'; object-src 'self'; base-uri 'none'`, no
  `img-src`) allows `data:` images.
* `status`: `vaultName` and `vaultId` are only revealed to paired clients
  (else `null`); invalid credentials just give `paired: false`.
  `extensionVersion` (manifest version of the extension the app delivers)
  and `extensionDir` (absolute path of `<data_dir>/browser-extension`) come
  from `VaultBackend::extension_info` and are also only sent to paired
  clients (`null` otherwise, from apps without the method, and – desktop app
  – until the folder holds that version, see "Browser extension
  deployment"). Clients
  treat missing fields as `null`.
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
* `unlock`: after 5 wrong passwords it answers `wrong_password` for 30 s
  without trying; every further failure restarts the 30 s. Attempts are
  serialised. Failures are counted per vault (the requested `vaultId`, a
  request without one under its own key) and the limit applies to their sum
  – it is shared by all vaults (a wrong password while switching counts
  too). A success clears only the failures of the vault it opened or
  re-checked (and of the no-id key if the request had no `vaultId`): the
  known password of one vault – e.g. re-checking the open one, or opening it
  again – never resets the count of guesses against another. `vaultId` absent or
  `null` = the backend picks the vault (previous behaviour); `""` →
  `invalid_request`; an unknown id → `not_found` (does not count as a
  failure). A failed unlock never closes the open vault.
* `VaultBackend::on_activity` runs after successful *user actions* only
  (`unlock`, `search`, `get_login`, `get_totp`, `generate_password` unless
  `remember: false`, `remember_generated`, `save_login`, `update_password`,
  `copy_field`, `copy_secret`) – `status` polling, the automatic
  `logins_for_url`, `check_login_password` (a page can trigger it by
  submitting forms) and password previews (a page can focus its signup field)
  must not keep the vault from auto-locking.
* `generate_password` with `remember: false` calls
  `VaultBackend::preview_password` (default: the core generator, nothing
  stored); absent, `null` or `true` keep the old behaviour
  (`generate_password`: stored in the history if unlocked). Apps before 2.0.0
  ignore the field (unknown fields are ignored) and store the preview; they
  answer `remember_generated` with `invalid_request` (unknown type), which
  the extension treats as "already stored". `remember_generated` checks the
  length (empty or > 1024 characters → `invalid_request`) and calls
  `VaultBackend::remember_generated` (default `invalid_request`; the desktop
  app adds it to the open vault's history like the page's
  `remember_generated` – the same password as the newest entry is not added
  again). The app 2.0.0 knows both but adds every `remember_generated`, the
  same password twice too – the extension's popup therefore reports each
  preview only once (see "Browser extension behaviour", generator).
* `status` reports `generatorVersion` = `protocol::GENERATOR_VERSION` (2 since
  the generator rework: the newer `GeneratorOptions` fields, passwords from 4
  characters, the default preset for missing fields, the newest-entry check
  of `remember_generated`), also to unpaired clients. Apps before it send
  none; the extension counts that as 1 – such an app ignores `minUppercase`,
  `minLowercase`, `symbolSet`, `exclude`, `startWithLetter` and `wordlist`
  (unknown fields) and rejects lengths below 5 – and sends it no options
  that need version 2 (`lib/generator.js` `olderAppIssue`).
* `generate_password` options: the request's `options` are read as a
  `protocol::GeneratorOptionsPatch` – exactly what `GeneratorOptions`
  accepts (a known field with a value of the wrong type → `invalid_request`,
  unknown keys ignored), but it remembers which fields were sent.
  `dispatcher::generator_options_for` applies them over
  `VaultBackend::default_generator_options()` (default: the generator
  defaults; the desktop app: its settings' default preset,
  `Settings::default_generator_options`); no `options` (absent, `null`) or
  `{}` → the default preset itself. If the sent fields make the preset
  invalid while the same fields over the generator defaults are valid (e.g.
  `symbols: true` over a preset with an empty symbol set), the defaults stand
  in for the preset – a request that worked before presets existed keeps
  working; invalid either way → `invalid_request`. This holds for previews
  and stored passwords alike.
* `copy_secret` calls `VaultBackend::copy_secret`, `copy_field` calls
  `VaultBackend::copy_field` (default: `get_login` / `get_totp`, then
  `copy_secret`). The default `copy_secret`
  copies with `keystead_core::clipboard::copy_secret` in the app process:
  excluded from clipboard history, cleared after `clipboardClearSeconds` from
  the settings file (0 = never) if the clipboard still holds it, and – being
  the same pending secret – cleared when the app locks. (The desktop app
  overrides both with the path of its own sensitive `copy_text`; its
  `copy_field` reads the field and the lock epoch in one step and copies
  nothing – `locked` – if the vault was locked or switched in between.) TOTP codes
  are copied without spaces. `copy_field` with another `field` → `invalid_request`; a
  login without TOTP seed → `not_found`.
* `check_login_password` compares without an early exit and answers only
  `true`/`false` (unknown id or non-login → `not_found`). The extension binds
  the save/update prompt it computes with it to the open vault (`status`
  `vaultId` after the comparison): it drops the prompt on lock or vault
  switch and never saves it into another vault (see
  `extension/chrome/README.md`).
* Memory hygiene (best effort): request passwords (`unlock`, `save_login`,
  `update_password`, `check_login_password`, `copy_secret`,
  `remember_generated`) are held in
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
  app). On Windows `stop()` disconnects them right away (`DisconnectNamedPipe`
  on a duplicate handle of every open connection, then it waits up to 2 s for
  their threads to close) – an open server-side pipe instance keeps the pipe
  name taken (`FILE_FLAG_FIRST_PIPE_INSTANCE`), so a restart of the server
  (browser integration toggled, portable-mode switch, failed update) would
  otherwise fail with "possible hijack" while the native host's connection
  lives.
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
    fn extension_info(&self) -> Option<ExtensionInfo> { None }                  // { version, dir } of the delivered extension (status, paired only)
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
    fn default_generator_options(&self) -> GeneratorOptions { /* default: GeneratorOptions::default(); the app: its default preset */ }
    fn generate_password(&self, options: GeneratorOptions) -> Result<String, BridgeError>; // + stored in the history if unlocked
    fn preview_password(&self, options: GeneratorOptions) -> Result<String, BridgeError> { /* default: core generator, not stored */ }
    fn remember_generated(&self, password: &str) -> Result<(), BridgeError> { /* default: Err(InvalidRequest) */ }
    fn save_login(&self, name: &str, url: &str, username: &str, password: &str) -> Result<String, BridgeError>;
    fn update_password(&self, item_id: &str, password: &str) -> Result<String, BridgeError>;
    fn copy_secret(&self, text: &str) -> Result<(), BridgeError> { /* default: core clipboard::copy_secret, clipboardClearSeconds from settings.json */ }
    fn copy_field(&self, item_id: &str, field: CopyField) -> Result<Option<u32>, BridgeError> { /* default: get_login/get_totp + copy_secret; the app reads and copies in one step (lock epoch) */ }
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

## Browser extension behaviour (`extension/chrome`)

The extension's security model and file roles are in
`extension/chrome/README.md`; the behaviour other components rely on:

* **Form detection** (`lib/forms.js`, pure DOM helpers): password fields are
  grouped by their `<form>` or the nearest container with another login field
  (or a button). A field's role: `autocomplete="new-password"` / name, id or
  label words new, confirm, repeat, neu, wiederholen, bestätigen … → *new*;
  `current-password` / current, old, aktuell, alt … → *current*. One field →
  `login` (or `signup` if it asks for a new password and the form's own words
  do not clearly say "log in", or the form – else the page title/path without
  login words – reads like a registration); two →
  `change` if the first asks for the current password, else `signup`
  (password + confirmation); three or more → `change`. A lone e-mail/username
  field with login context (form/button words, title, path) is a `username`
  step (multi-step logins) – unless it lies inside a password form found
  before (a signup form's username after the password, a second identifier
  of a login) or its container (its form, else the nearest element with a
  button) holds such a form's password field (div-based signups). If the
  step reads like a registration, the field is a `signup` form without
  password fields (a registration's first step). The first of these that
  says log in/sign in/anmelden/Anmeldung (wins) or sign up/create account/
  Konto erstellen/Neuanmeldung decides: the step's own words (action, id,
  class, name, its submit buttons – in a `<form>` the submit buttons, else
  all; a button with login/signup words beside a plain Weiter/Next is
  left out), its headings (h1–h3, legend, role=heading), the last heading
  before it in its nearest ancestors (up to 3, below `<body>`, none holding
  another form's fields), the page (title, path).
  "E-Mail-Adresse"/"email address" does not count as a postal address.
  Login icons go to username/current-password fields, password suggestions
  to the first new-password field of `signup`/`change` forms (never to
  login forms, username steps or search fields). A stored login
  (`rankForms`) goes to the focused `login`/`change`/`username` form, else
  the first by that order. A `signup` form takes one only when the user
  picked it in the popup (*Ausfüllen*, `popup:fill`) and no frame of the tab
  has another form: the service worker then sends `bg:autofill` a second
  time with `explicit: true`, and `fillForm(form,
  credentials, { explicit: true })` writes the username and the
  password into the new-password field and its confirmations, so a login
  form taken for a registration can still be filled. `Ctrl+Shift+L` (the
  service worker picks the login) never fills a `signup` form; when no frame
  has a form to fill it toasts `csNoFields`, which the top frame shows as
  `csNoFieldsSignup` (pick the login in the popup) if the popup could fill
  one of its forms. Without the explicit pick `fillForm` never writes a
  stored password into a field that asks for a new one.
* **Web components**: everything above also works inside *open* shadow roots
  (composed tree: a shadow root's content belongs to its host; a host's
  `name`/`id`/`label`/`autocomplete` are hints for its input). The content
  script finds roots in added subtrees, in the composed path of trusted
  focus/input events and with a throttled sweep (no main-world
  `attachShadow` hook; while the page is visible every 3 s for 30 s, up to
  60 s while custom elements wait for their definition, then every 12 s –
  `shadowSweepDue`), observes them and listens for `submit` in them.
  Closed shadow roots are not traversed. Filling uses the native value setter
  and `composed` input/change/keyup events.
* **Password suggestion**: on a focus that follows a user gesture
  (transient user activation) on a suggestion field while unlocked, the
  service worker generates a preview (`generate_password`, `remember: false`,
  the popup's own generator options, else the app's default generator
  preset) and the page shows it
  masked in a bubble in the closed shadow root. *Verwenden* (trusted click,
  500 ms dwell) fills the field and its confirmation fields, counts as user
  input for the save prompt and sends `remember_generated` (skipped if
  another vault was opened meanwhile). Locked / not paired: only a click on
  the field icon explains how to get suggestions.
* **Generator** (popup tab *Generator*, *Login hinzufügen*, context menu,
  `Ctrl+Shift+9`): every password is a preview (`generate_password` with
  `remember: false`, `background.js` `generatePreview`); `remember_generated`
  (`rememberGenerated`, best effort: `locked` or the `invalid_request` of an
  app before 2.0.0 – it stored the preview already – are ignored) follows
  only a use: *Kopieren* in the popup (`popup:copy-generated`: `copy_secret`,
  then `remember_generated`), *Ausfüllen* in the popup and the context menu /
  shortcut once a field took the password (`popup:fill-generated`,
  `generateInto`; nothing is remembered when no field filled), a suggestion's
  *Verwenden*, and *Login hinzufügen* saved with the password its wand put
  into the field (`popup:save-login` with `generated: true`). Each preview is
  reported once: once the app confirmed it, the popup's later *Kopieren* /
  *Ausfüllen* of the same preview send `remember: false` and the service
  worker skips `remember_generated` (`remembered: true`) – the app 2.0.0 would
  store it again, newer apps skip a repeat of the newest entry anyway. The popup tab
  shows no password until *Generieren* (also Enter outside buttons, selects
  and switches, and Ctrl+G); changing an option marks a shown preview as
  outdated ("Die Optionen haben sich geändert …") and never generates.
  Options: without own options (default) the tab says *Standard-Vorlage der
  App* and requests carry no options of their own (`{}`); *Anpassen* starts
  **own options** for this browser from the generator defaults – stored in
  `chrome.storage.local` `generatorOptions` as a full `GeneratorOptions`
  (`lib/generator.js`), and only while they are valid – which the
  suggestions, the context menu / shortcut and *Login hinzufügen* then use
  too; *App-Vorlage nutzen* removes them. The popup of an extension up to
  2.0.0 stored the same key on every option change, without *Anpassen*:
  after the update such options count as own options as well (they drove the
  suggestions in 2.0.0 too) and win over the app's default preset until
  *App-Vorlage nutzen*. The own options cover everything
  the app's generator page offers except presets: length 4–128, the four
  groups with a minimum each, allowed symbols, excluded characters, first
  letter, look-alikes; words, separator (≤ 8), word list (English /
  German), capitalisation, number. `lib/generator.js` mirrors the core's
  checks (`generatorIssue`, the same `invalid_input` codes in the same
  order – the bridge only says `invalid_request`), shows them inline and
  disables *Generieren* while the options are invalid, and estimates the
  entropy (real word list sizes 7776 / 7724). Stored options – also the
  partial ones of older extensions – are read field by field over the
  defaults (`storedOptions` / `lenientOptions`) by the popup and the service
  worker alike, so every request for the same own options sends the same
  complete set (a missing field is never left to the app's default preset);
  each request sends only known fields with acceptable values
  (`requestOptions`). Against an app whose `status` has no
  `generatorVersion` ≥ 2 (`olderAppIssue`), own options it would ignore or
  reject – length below 5, an own symbol set, excluded characters it could
  draw, a first letter, minimums above 1 for upper- or lower-case letters,
  the German word list – are kept but not sent: the popup names the problem
  and disables *Generieren*, the service worker answers `app_outdated` (the
  context menu / shortcut toast *csGenerateOutdated*, the suggestion bubble
  says so). The bridge has no preset list, so the popup can name the app's
  default preset only as such.
* **2FA after a fill**: when a login with a TOTP seed is filled (popup,
  dropdown, `Ctrl+Shift+L`) the service worker stores a 2FA offer for the tab
  (`chrome.storage.session` `totp:<tabId>` = item id, site, vault id, expiry
  in 2 minutes – never the code) and, if the setting *2FA-Code nach dem
  Ausfüllen kopieren* is on (default), has the app copy the code
  (`copy_field` `totp`); the top frame shows a toast with the seconds left.
  One-time code fields (`autocomplete="one-time-code"`, otp/totp/2fa/code …
  names with a 6–8 character limit, or 6–8 single-digit boxes) of the same
  site get *2FA-Code einfügen*; its click (trusted, dwell) makes the service
  worker check the offer and the login's match against that frame's URL, read
  the current code (`get_login`) and drop the offer. Lock or vault switch
  drops all offers.
* **Clickjacking dwell feedback**: our controls act only after 500 ms fully
  visible (IntersectionObserver v2) and with our host at the click point;
  a rejected click shakes the control and shows "Einen Moment …" or
  "Verdeckt – Seite blockiert Keystead-Menü" (then the layer is raised again);
  dropdown items show a progress ring during the 500 ms.
* **Popup**: 2FA countdown per login on "Diese Seite" (`popup:totp-timers`:
  the service worker calls `get_totp` and passes on only period and seconds
  left), ↑/↓/Enter to pick and fill, settings screen (`chrome.storage.local`
  `settings`: `autoCopyTotp`, `suggestPasswords`).

## UI/UX principles (desktop + extension)

* Look & feel of a modern password manager (Bitwarden/1Password-like):
  left sidebar (Alle Elemente, Favoriten, Typen, Ordner, Tags, Papierkorb,
  Werkzeuge: Generator, Sicherheitsbericht), middle item list with search,
  right detail pane. Edit inline in the detail pane. The phone (and any
  window up to 700 px) uses the same screens in one column – sidebar as a
  drawer, detail stacked over the list, back gesture like Escape – see
  "Phone layout" under "Mobile (Android)".
* Tags: the sidebar group lists every tag outside the trash – one row per
  tag whatever its spellings ("Bank" and "bank", as the core compares them),
  showing the most frequent spelling and the summed item count – with
  rename / delete on hover (`rename_tag` / `delete_tag`, then `list_items`
  is refetched; an item open in the editor gets the same change in its draft
  and baseline, so saving it cannot bring the old tag back; a tag filter
  whose last item loses the tag falls back to "Alle Elemente"); the tag
  filter matches any spelling and is titled with the shown one; the detail
  header shows an item's tags as chips that open that filter (plain chips on
  a trashed item: the filter never shows the trash, and a tag only the trash
  carries has no filter); the editor has a "Tags" section (chips, Enter /
  comma adds, Backspace on the empty input removes the last one, the
  vault's tags as suggestions, the 50-character / 20-tag limits checked
  inline before the backend sees them; text still in the tag input joins the
  item on blur and on Ctrl+S / Ctrl+Enter, like a click on "Speichern"); a
  new item created inside a tag filter starts with that tag. List rows show
  no tags.
* Attachments belong to the saved item: a section "Anhänge" in the detail
  pane after the custom fields (name, size, date; "Vorschau" for images,
  "Speichern unter …", "Entfernen"; "Datei anhängen …" below), a paperclip
  chip with the count in the detail header, read-only on trashed items and on the phone; the
  editor has no attachments section. Files go only through the native
  open / save dialogs, a preview is hidden again like a revealed secret,
  and the export dialog says that no export contains attachments (a backup
  is the data directory). List rows show no attachments.
* Conflict dialog (two-way cloud-folder sync, both platforms –
  `components/SyncConflictDialog.tsx`, `state/syncConflicts.tsx`,
  `lib/syncConflict.mjs`): while a vault is open the provider fetches
  `sync_conflicts` and follows `sync://conflict` (an event also brings back
  a dialog put off with "Später"). One conflict at a time: "Am PC und am
  Handy geändert", the vault name, both states as cards ("Stand vom PC",
  "Stand vom Handy", "dieses Gerät" on this side's) with the change time in
  plain words and the revision as "Version n". Buttons in the same words
  on both devices – you take the other side's state and keep your own:
  phone "Stand vom PC übernehmen" (`remote`) / "Stand vom Handy behalten"
  (`local`), PC "Stand vom PC behalten" (`local`) / "Stand vom Handy
  übernehmen" (`remote`) – plus "Später" (closes until the next event or
  unlock). `passwordChanged` hides exactly the `local` button (the backend
  refuses it: `unsupported:sync_conflict_password`) and says why; on the PC
  `lostAttachments` warns what taking the phone's state drops. A note says
  the losing state stays as a conflict copy on this device (PC: data
  folder, `conflicts`, restorable through Settings → Importieren; phone:
  import is PC-only). Errors are toasts; `unsupported:sync_conflict_changed`
  just fetches the list again. On the phone the buttons stack, "Später"
  last.
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
  "eye" toggle for secrets (hidden again after 30 s), a generate button in every password field,
  keyboard shortcuts (Ctrl+F search, Ctrl+N new, Ctrl+L lock,
  Ctrl+Shift+C on a selected item copies its password, Ctrl+B the username,
  Ctrl+S saves while editing, Esc cancels editing / closes dialogs).
* Generator (`src/screens/GeneratorPage.tsx`, `src/components/Generator*.tsx`):
  nothing is generated behind the user's back. The page opens with an empty
  state ("Noch nichts generiert") and the primary button "Generieren"
  (focused, so Enter works at once; Enter in an option's input and Ctrl+G
  while the focus is on the page do the same – not in a dialog); changing
  an option never generates – a preview made with other options shows "Die
  Optionen haben sich geändert …" until "Neu generieren". The preview
  (`generate_password`, `remember: false`) is shown with the strength meter,
  "Neu generieren" and "Kopieren"; only copying stores it (`copy_text`
  sensitive, then `remember_generated`; the backend skips a repeat of the
  newest entry). Options: password / passphrase; length 4–128 (slider and
  number); the four character groups (at least one stays on) with a
  minimum per group (0 … length); the allowed symbols (an input, "Standard
  wiederherstellen" back to `!#$%&*+-=?@^_`); "Diese Zeichen nie verwenden"
  (removed from every group); "Mit einem Buchstaben beginnen"; "Verwechselbare
  Zeichen vermeiden". Passphrase: words 3–20, separator (≤ 8 characters),
  word list Englisch / Deutsch, capitalise, number. Problems are shown
  inline at their field, checked like the core in the core's order
  (`src/lib/generatorRules.ts`, the same `invalid_input` codes; an emptied
  group names the group), and disable "Generieren"; below the options the
  estimated entropy – password: length × log2(characters left to draw
  from), passphrase: like `passphrase_entropy_bits` with the real list
  sizes 7776 / 7724 – with a strength word. Presets: a picker above the
  options (built-ins with translated names, then "Eigene Vorlagen"; the
  default carries ★); choosing one loads its options (no generation);
  edited options show "Geändert" with "Zurücksetzen" (and "Änderungen
  speichern" for a user preset); the "…" menu saves the options as a new
  preset (name dialog, the backend's codes inline), renames, deletes
  (confirmation; a deleted default falls back to "Standard") and makes the
  preset the default ("Als Standard verwenden", also as a link under the
  picker: "Standard für den Editor und die Browser-Erweiterung"). The page
  remembers its preset id and options per device in `localStorage`
  (`keystead.generator.page`, non-secret); without it the page starts with
  the default preset. History: masked rows, newest first, eye and copy per
  row (see "Secrets and the webview"), "Verlauf leeren", and the note that
  only copied or used passwords are kept (at most 50); it is re-read after
  a copy and on `vault://changed`. The editor's popover (the "Generieren"
  link of the password field) starts with the default preset's options
  (`generator_presets().defaultOptions`), has the preset picker and the
  options in compact form ("Weitere Optionen" opens minimums, symbols,
  exclusions and the checkboxes), generates only on its button (Enter in its
  inputs / Ctrl+G inside it – Enter never submits the editor), and
  "Verwenden" – or copying – reports the password with
  `remember_generated`.
* German first, English second (`src/i18n/de.ts`, `src/i18n/en.ts`).
* Logins show their website's icon (see "Website icons") on a light rounded
  tile, otherwise a tinted letter tile; cards, identities and notes a type
  icon. Same in the extension popup.
* Nothing goes online behind the user's back: the leaked-password check
  (see "Leaked-password check") is a button in the security report – the
  first card, "Geleakte Passwörter" – and a link button under the strength
  meter in the editor; a dialog before the vault-wide run says exactly what
  leaves the device ("Nicht mehr fragen" is remembered locally); the result
  is shown as the card's rows, as a red callout with "Passwort ändern" on
  the item and inline in the editor, and is never stored.
* No hidden options: settings is one page with clear sections.
* Import: drop a file anywhere or pick one – the format is detected, nothing
  is written before the preview is confirmed, and existing entries are never
  imported twice (see "Import with preview"). Export: one dialog – a format
  card (Keystead encrypted, recommended / CSV / Bitwarden JSON), the export
  password twice or a clear warning for unencrypted formats, the master
  password, "Speichern unter …" with `Keystead-<vault>-<YYYY-MM-DD>.<ext>` as
  the suggested name; the success toast offers "Im Ordner anzeigen"
  (`show_export`).
* While a dialog is open, toasts appear at the top so they never cover its
  buttons. Notes that belong to a dialog (e.g. the import dialog's "only one
  file" / "import in progress" notes for dropped files) are shown inside it
  instead, so nothing covers its title.
