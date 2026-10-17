# Keystead – notes for Claude

Keystead (formerly VaultX) is a local-only, Bitwarden-like password manager:
Tauri 2 desktop app (Rust + React), terminal UI, and a Chromium MV3 extension that
talks to the app via Native Messaging. **No server, no cloud.** The owner is German;
talk to them in German and ask when a request is ambiguous.

Start with `docs/HANDOFF.md` (current state, open work) and `docs/ARCHITECTURE.md`
(the binding contract: data model, file format, Tauri commands, bridge protocol,
updater, release flow). Keep both in sync with every interface change.

## Layout
- `crates/keystead-core` – crypto (Argon2id + XChaCha20-Poly1305), vault format,
  storage, import/export, generator, TOTP, health, icons model. No networking.
- `crates/keystead-bridge` – native-messaging host, local socket IPC, pairing/auth
  dispatcher, browser registration.
- `crates/keystead-tui` – terminal UI + `keystead-cli` binary.
- `apps/desktop/src-tauri` – Tauri backend (library `keystead_lib` + `Keystead.exe`):
  commands, updater, extension deployment, import flow, icon fetcher, tray,
  auto-lock; `src/mobile.rs` + `tauri.android.conf.json` + `gen/android`
  (committed Android project, hardened – see "Android project" in
  ARCHITECTURE) for the phone (Block 5, `docs/BLOCK5-DESIGN.md`).
- `apps/desktop/src` – React/TS UI (German-first i18n `src/i18n/de.ts`/`en.ts`,
  mock backend `src/lib/mock.ts` for `npm run dev` in a browser, password `demo`).
- `extension/chrome` – MV3 extension, plain JS, no build step.

## Commands
```sh
cd apps/desktop && npm ci && npm run dev      # UI with mock backend
cd apps/desktop && npx tauri dev              # real app
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd apps/desktop && npm run build)
node --test extension/tests/*.test.mjs .github/scripts/*.test.mjs
cd apps/desktop && npx tauri android build --apk --debug --target x86_64   # emulator APK (JAVA_HOME=JDK 17, ANDROID_HOME, NDK_HOME set)
```
Toolchain is pinned in `rust-toolchain.toml` (1.97.0) – bump deliberately (newer
clippy lints break CI otherwise).

## Rules
- The GitHub repo is **public**. Never commit private keys, tokens or vault files.
  Signing keys live in the private repo `CedrickGD/keys` (folder `keystead/`); CI
  uses the secret `TAURI_SIGNING_PRIVATE_KEY`.
- `main` holds the released state (2.0.0 and later). Work on a feature branch
  and merge it via pull request with a merge commit; the owner decides when to
  merge and when to tag.
- Commit message markers: `[release]` publishes a signed beta pre-release
  (`v<version in tauri.conf.json>-beta.<run>`, e.g. `v2.0.1-beta.12`) that
  installed apps on the beta channel receive via in-app update; `[skip ci]` for
  work-in-progress pushes. Tags `vX.Y.Z` publish stable releases; right after a
  tag, bump `version` in `apps/desktop/src-tauri/tauri.conf.json`.
- The extension ID `imfndemblnaalppnmdplagajjielnaok` is fixed by the `key` in
  `extension/chrome/manifest.json` – never change that key.
- Android: never change the identifier `io.github.cedrickgd.keystead`
  (`tauri.android.conf.json`); `gen/android` is committed (do not re-run
  `tauri android init` over it); the signing keystore and
  `gen/android/keystore.properties` never enter the repo; the main manifest
  stays without `INTERNET` (debug manifest only).
- Security posture to preserve: secrets never reach the webview except on explicit
  reveal/edit; page commands carry `pageVaultId`; extension UI in closed shadow
  roots with isTrusted + visibility/dwell checks; fills only into frames whose own
  URL matches; updates must be signature-verified.
- Every UI string goes through i18n (de + en).
