<!-- Arbeitsdokument, Stand 2026-10-16. Entwurf aus fünf Leseberichten (Tauri-Backend, Core, UI, Android-Plattform, CI/Doku), drei unabhängigen Entwürfen (Sicherheit / Nutzer / Wartbarkeit) und einer Zusammenführung; dazu eine iOS-Recherche. Die Entscheidungen des Besitzers stehen oben; der Rest ist Entwurfstext mit Begründungen. Umgesetzte Teile wandern nach ARCHITECTURE.md. -->

# Block 5 „Mobile“ – Entwurf und Entscheidungen

## Entscheidungen des Besitzers (2026-10-16)

| Nr. | Frage | Entscheidung |
|---|---|---|
| – | Plattformen | **Android voll** (Emulator auf dem PC, APK über die GitHub-Releases); **iOS nur als CI-Build** auf dem kostenlosen macOS-Runner (Simulator + Screenshots) – kein Mac, kein Apple-Konto, nichts Kostenpflichtiges; der Besitzer hat kein Handy zum Testen, nur Emulatoren |
| – | Sync | Tresor über den **eigenen Cloud-Ordner** (Nextcloud/OneDrive-App am Handy, Storage Access Framework); kein Keystead-Server, kein Heimnetz-Sync |
| E1 | Paketname | `io.github.cedrickgd.keystead` nur für Android (`tauri.android.conf.json`); Desktop behält `com.keystead.desktop` |
| E2 | Mindest-Android | API 29 (Android 10) |
| E3 | Sperre im Hintergrund | Standard 1 Minute, einstellbar (Sofort / 1 min / 5 min / Nie) |
| E4 | Stufe 1 und 2a | Handy nur lesend |
| E5 | Fingerabdruck | Opt-in, Standard aus; Plugin-Rust-API wenn verifiziert, sonst eigenes Kotlin-Plugin |
| E6 | Netz | Stufe 1 ohne INTERNET-Berechtigung; ab Stufe 2 Update-Hinweis über das Mini-Kotlin-Plugin; Website-Icons und HIBP auf Android dauerhaft aus |
| E7 | Desktop-Seite des Syncs | neue Einstellung „Kopie im Cloud-Ordner aktuell halten“ |
| E8 | Backup | `allowBackup="false"` + dataExtractionRules |
| E9 | CI | Android bei jedem Push mitbauen |
| E10 | Keystore | vom Assistenten auf dem PC des Besitzers erzeugt, im privaten Repo `CedrickGD/keys/keystead/` abgelegt, Secrets `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` gesetzt (2026-10-16) |
| E11 | Reihenfolge nach 2a | **Autofill zuerst**, dann Bearbeiten + Zwei-Wege-Sync |
| E12 | Werkzeuge | installiert (JDK 17, NDK 28.2.13676358, Plattform 37.0, Rust-Targets, Umgebungsvariablen) |
| E13 | iOS | siehe „Plattformen“: CI-Simulator-Job nach Android |

---

# Block 5 „Mobile“ – zusammengeführter Vorschlag (Android, kostenlos, lokal)

Stand 2026-10-16, Branch `claude/mobile-android` (= `main` nach 2.0.0). Drei unabhängige Entwürfe (Sicherheit, Nutzer, Wartbarkeit) sind hier zu **einem** Plan verschmolzen. Alles, was im Text als Fakt steht, habe ich im Code oder in der lokalen Cargo-Registry nachgelesen; was ich nicht prüfen konnte, ist als **„zu verifizieren“** markiert.

## Ziel

Eine Android-App „Keystead“ als **APK über die GitHub-Releases**, die denselben Tresor wie der PC öffnet: nachschlagen, suchen, Benutzername/Passwort/2FA-Code kopieren, per Fingerabdruck entsperren. Keine Keystead-Server, keine eigene Cloud, kein Play Store, nichts Kostenpflichtiges. Der Tresor kommt über **deinen eigenen Cloud-Ordner** (Nextcloud/OneDrive-App → Storage Access Framework) aufs Handy – das hast du am 2026-10-16 so entschieden, ich frage das nicht erneut. Autofill kommt zuletzt.

Drei Grundsätze, die alle drei Entwürfe teilen und die ich übernehme:

1. **Ein Rust-Crate, eine React-App.** `apps/desktop/src-tauri` bekommt eine `src/lib.rs` als Mobile-Einstieg; Desktop-Teile werden mit `#[cfg(desktop)]` ausgeblendet. Kein zweites Crate, kein zweiter UI-Einstieg.
2. **Sicherheitsposture 1:1.** Geheimnisse erreichen das WebView nur bei Aufdecken/Bearbeiten, Kopieren bleibt Backend-Sache, Lock-Epoche und `Wiped<T>` bleiben. Was auf Android wegfällt (Windows-`contentProtected`, arboard-Zwischenablage, WTS-Session-Lock), wird gleichwertig ersetzt (`FLAG_SECURE`, `EXTRA_IS_SENSITIVE` + Timer, Sperre bei `WindowEvent::Suspended`).
3. **Erst klein, dann wachsen.** Stufe 1 ist ein **nur lesender** Nachschlage-Client **ohne Netz**. Alles, was ein Sync-Modell voraussetzt (Bearbeiten, Konflikte), kommt in Stufe 2.

### Was ich gegenüber den Entwürfen korrigiert habe (nachgeprüft)

- **Auf deinem PC gibt es bereits ein JDK**: `C:\Program Files\Android\openjdk\jdk-21.0.8` (Microsoft OpenJDK 21.0.8 LTS, laut `release`-Datei). Nur `JAVA_HOME`/`ANDROID_HOME`/`NDK_HOME` sind leer und `java` ist nicht im PATH. Gradle 9 braucht JVM 17–27 → **kein Temurin-Download nötig**.
- **Die Android-Rust-Targets sind schon da**: `rustup +1.97.0 target list --installed` zeigt `aarch64-linux-android`, `armv7-linux-androideabi`, `i686-linux-android`, `x86_64-linux-android` (die Entwürfe hatten nur die `stable`-Toolchain angeschaut). Nur `rust-toolchain.toml` (Zeile 6) kennt sie noch nicht.
- **Emulatoren existieren**: `~/.android/avd/` enthält `MB_API24` (android-24) und `MB_S26`, `MB_S26_B/C/D`, `ConvertX_Test` (android-36); System-Images android-24 und android-36 liegen im SDK. **Es fehlt nur das NDK** (`%LOCALAPPDATA%\Android\Sdk\ndk\` gibt es nicht) und evtl. Platform 37.
- **Lebenszyklus ohne eigenes Plugin**: `tauri::WindowEvent::Suspended`/`Resumed` existieren in tauri 2.12.2 (`src/app.rs` L160/L169, Mapping L192/L194, `#[cfg(mobile)]`, „Android: onPause/onResume“). Die Web-Recherche hatte das verneint – sie meinte `RunEvent`, das tatsächlich keine Suspended-Variante hat. Der bestehende `.on_window_event`-Hook (`gui.rs` L106-120) reicht.
- **Billiges Gating der Commands**: `tauri::generate_handler!` parst äußere Attribute pro Eintrag (`tauri-macros-2.7.1/src/command/handler.rs` L13-21, Ausgabe L177) – `#[cfg(desktop)] commands::open_terminal,` funktioniert direkt in der Liste.
- **Die Settings-TOC-Zeilenvariante ist nicht tot**: `pages.css` L1687-1718 setzt `.settings-toc { flex-direction: row; overflow-x: auto; … }` unter `@media (max-width: 1240px)`; `SettingsPage.tsx` L867 rechnet genau damit. Für das Handy ist diese Regel fast fertig.

## Stufe 1 – Android-App (nachschlagen, kopieren, 2FA, Fingerabdruck)

### Umfang

**Drin:** Tresore auflisten, entsperren (Master-Passwort oder Wiederherstellungsschlüssel – `UnlockScreen.tsx` ist schon einspaltig), Tresor neu anlegen, Suche/Filter (Alle, Favoriten, Typen, Ordner, Tags), Detailansicht mit Auge-Umschalter (`reveal_secret`, 30 s) und Kopieren (`copy_secret_field`, `copy_text`), TOTP mit Sekundenring (`totp_for_item`), Favorit setzen, Anhänge nur als Anzahl + Bildvorschau (`reveal_attachment` liefert Data-URLs), Sicherheitsbericht ohne HIBP, gespeicherte Website-Icons (`get_icons` liest nur Daten), Einstellungen-Teilmenge (Design, Sprache, Auto-Sperre, Zwischenablage, Tresor umbenennen/löschen, Über), **ein** neues Datei-Kommando `import_vault_file(uri)` (eine `.keystead`-Datei einmalig über den System-Picker in den App-Ordner kopieren – sonst wäre die App nur mit einem am Handy angelegten Tresor testbar).

**Bewusst nicht drin (Stufe 1):** Bearbeiten/Neu/Papierkorb/Tag-Verwaltung (Handy **read-only**, siehe Entscheidung E4), Datei anhängen/„Speichern unter“ (pfadbasiert, `attachments.rs` `is_dialog_path` lehnt `content://` ab), Import mit Vorschau/Export, Website-Icon-Abruf, HIBP, Update-Prüfung (alle drei brauchen TLS – siehe Netz), Browser-Integration/Pairing, Tray, portabler Modus, Terminal, Drag & Drop.

**Stufe 1b (zweite APK):** biometrisches Entsperren + „Sperren beim Verlassen der App“.

> **Stand 2026-10-17 – Fingerabdruck (E5) umgesetzt** (Vertrag: ARCHITECTURE „Fingerprint unlock (phone)“, `src-tauri/src/device_unlock.rs`, `src/mobile/biometric.rs`): Variante **(a)** – die Quellen von `tauri-plugin-biometric` 2.5.0 bestätigen `biometric_cipher` (AES-256-GCM im Android-Keystore, `setUserAuthenticationRequired(true)` je Nutzung, `BiometricPrompt` mit `CryptoObject`, starke Biometrie, ungültig nach neuem Fingerabdruck). Abweichungen vom Text unten: der Blob speichert zusätzlich das KDF-Salz des Tresorkopfs (ein am PC geändertes Passwort wird ohne Prompt erkannt) und versiegelt die Tresor-ID mit dem KEK; ein ungültiger Blob wird gelöscht und der Entsperrbildschirm bietet nach dem Passwort an, ihn neu einzurichten; der Prompt der App startet die Hintergrund-Sperre nicht (eigene Activity des Plugins). Was (a) nicht hat: StrongBox, `setUnlockedDeviceRequired`, eigener Alias, Daten als `ByteArray` – (b) bleibt als Härtung, sinnvoll zusammen mit Autofill.

### Architektur (ein Crate mit Gates)

- `apps/desktop/src-tauri/Cargo.toml` (heute nur `[[bin]] name = "Keystead"`, L8-10): zusätzlich `[lib] name = "keystead_lib" crate-type = ["staticlib", "cdylib", "rlib"]` (Name ≠ Binärname). `src/lib.rs` trägt alle `mod`-Zeilen aus `main.rs` L11-27 (`#[cfg(desktop)] mod bridge; mod extension; mod portable; mod tray;`), `pub mod gui; pub mod platform;` und `#[cfg(mobile)] #[tauri::mobile_entry_point] pub fn run() { let _ = gui::run(false); }`. `main.rs` behält die Drei-Modi-Weiche (L34-51) und ruft `keystead_lib::gui::run`; `std::process::exit` bleibt nur dort. `App::run_return` ist in tauri 2.12.2 nicht Android-gegated.
- `build.rs`: `embed_browser_extension()` (L4) nur wenn `CARGO_CFG_TARGET_OS != "android"`; `tauri_build::build()` bleibt – es setzt die cfg-Aliase `desktop`/`mobile` (`tauri-build-2.7.1/src/lib.rs` L659-661) und `TAURI_ANDROID_PACKAGE_NAME_PREFIX`/`_APP_NAME` (L695/L704).
- Abhängigkeiten target-gaten: `keystead-bridge`, `keystead-tui`, `tauri-plugin-single-instance` (Crate ist komplett `#![cfg(not(any(android, ios)))]`, `lib.rs` L16), `tauri-plugin-updater` (Android `none`), `tauri-plugin-process`, `open`, `reqwest`, `hyper-util`, `http`, `image`, `rustls`, `tokio`, `semver`, `time`, `raw-window-handle` nach `[target.'cfg(not(target_os = "android"))'.dependencies]`; `tauri`-Feature `tray-icon` nur im Desktop-Target. **Warum reqwest raus:** reqwest 0.13.5 mit `rustls-no-provider` benutzt zwingend `rustls-platform-verifier` (`reqwest-0.13.5/Cargo.toml` L174-177), und der panict auf Android ohne JNI-Init (`rustls-platform-verifier-0.7.1/src/android.rs` L90 `expect("Expect rustls-platform-verifier to be initialized")`). Ohne reqwest im APK gibt es diese Falle nicht.
- Neu unter `[target.'cfg(target_os = "android")'.dependencies]`: `tauri-plugin-fs = "2.6"` (im Cargo.lock; Rust-API `Fs::open(FilePath::Url(content://…), OpenOptions)` → `std::fs::File`, `src/android.rs` L36-58, Modi `r/w/wt/wa/rw/rwt` `lib.rs` L370-381), `tauri-plugin-clipboard-manager = "2"`, `tauri-plugin-opener = "2"`, `log` + `android_logger` (`state::log` schreibt nur stderr, das nicht in logcat landet – **zu verifizieren**, ob Tauris `android_binding!` schon einen Logger installiert). `tauri-plugin-clipboard-manager`, `-opener`, `-biometric` liegen **nicht** in der lokalen Registry – Versionen und Rust-APIs (`write_secret`, `clear`, `OpenerExt::open_url`, `biometric_cipher`) sind **zu verifizieren**.
- **Alle Plugins werden nur aus Rust gerufen.** `capabilities/default.json` bekommt keine `clipboard-manager:`/`opener:`/`fs:`/`biometric:`-Rechte – wie heute bei updater/process (`gui.rs` L29-32, Test L311-326 wird um die Präfixe erweitert).
- Plattform-Konfiguration `tauri.android.conf.json` (Tauri merged `tauri.<platform>.conf.json`): `identifier` (E1), `bundle.android.minSdkVersion` (E2), Fenster ohne `visible:false`/`create:false`/`minWidth`. **Zu verifizieren:** ob `tauri android init` den Identifier aus der Plattformdatei nimmt – sonst `--config` im CLI-Aufruf.
- `gen/android` (aus `npx tauri android init`) wird **committet** (Gradle, Manifest, `MainActivity.kt`; das Template-`.gitignore` hält `build/`, `keystore.properties`, `tauri.properties` draußen – nach `init` prüfen). `src-tauri/.gitignore` ignoriert heute nur `/gen/schemas`.

### Kern-Änderungen in `keystead-core`

1. **Zwischenablage** (`clipboard.rs`): arboard 3.6.1 hat **kein Android-Backend** (`arboard-3.6.1/src/platform/mod.rs` L1-17: nur linux/windows/osx), `clipboard.rs` L33/L45-50/L63 referenziert `arboard::Clipboard` unbedingt → der Core **baut heute nicht für Android**. Umbau: Trait `ClipboardBackend { set_text(text, sensitive); clear_if_equals(expected) -> bool; clear() }` + `install_backend(Box<dyn …>)` (OnceLock); arboard wird Default-Backend unter `cfg(not(target_os = "android"))`, `Cargo.toml` L30 wandert in eine Target-Tabelle. GENERATION/PENDING/Timer bleiben im Core – damit wird die bisher nur manuell geprüfte Timer-Logik mit einem Mock-Backend **erstmals in CI testbar**. Android-Backend (App-Seite) nutzt `write_secret` (setzt `ClipDescription.EXTRA_IS_SENSITIVE`, Android 13+) und löscht nach Ablauf **ohne Inhaltsvergleich** (Android ≥ 10 verbietet Hintergrund-Lesen).
2. **Datenordner** (`paths.rs`): `data_dir()` L26-34 prüft `$KEYSTEAD_DATA_DIR` → portabel → `default_data_dir()` L65-74 (`dirs::data_local_dir` → `home_dir` → `temp_dir`). Auf Android liefert dirs ohne `$HOME` nichts → `temp_dir()` = nicht beschreibbar. Neu: `paths::set_data_dir_override(PathBuf)` (OnceLock, vor der Env-Prüfung). `gui::setup` (L142-146) setzt ihn `#[cfg(mobile)]` als **erste Zeile** mit `app.path().app_local_data_dir()` (`tauri-2.12.2/src/path/android.rs` L156-163: `getDataDir` + `/<identifier>`; genauer Pfad **im Emulator prüfen**). Danach laufen `Settings::load()`, `VaultStore::open_default()`, `Core::new` unverändert.
3. **Biometrie-API** (Stufe 1b): `UnlockedVault::master_kek()` (Feld `vault.rs` L54 privat) und `VaultStore::unlock_with_master_kek(id, &SecretKey)` analog `unlock` (`store.rs` L146) ohne `derive_master_kek` (`format.rs` L336 pub(crate)); `unwrap_key_with_kek` (L343) → `WrongPassword`/`KeyChanged`. ~25 Zeilen + Lifecycle-Tests.
4. **Settings** (`settings.rs` L71-86): neue Felder `lockInBackgroundSeconds: u32` und `deviceUnlock: bool` als Struct-Felder (nicht Nebendatei: `from_json_lenient` L141 liest nur bekannte Keys, `save_to` L192 verwirft Unbekanntes). Desktop ignoriert sie; Vertrag in ARCHITECTURE L687-707 und `types.ts` L404-420 nachziehen.
5. `sync::header_of(bytes)` über `VaultFile::parse` (`format.rs` L205) + `info()` (L304) – schon in Stufe 1 für die Validierung von `import_vault_file`.

### App-Crate im Einzelnen

- `gui.rs`: single-instance (L21-27), updater (L33), process (L34) unter `#[cfg(desktop)]`; `#[cfg(mobile)]` Plugins fs/clipboard-manager/opener; Handler-Liste L35-101 mit `#[cfg(desktop)]` vor Bridge/Pairing/Terminal/Portable/Extension/`install_update`/`show_export`/`add_attachment`/`save_attachment`/Import/Export/`legacy_scan`/`check_pwned_*`; `setup()`: Tray L159, Bridge L166-181, Extension L184-187, `update::spawn_background_checks` L188, `icons::spawn_scheduler` L190, `hidden`-Block L194-198 unter `#[cfg(desktop)]`; `on_window_event`: `CloseRequested` desktop, `#[cfg(mobile)] WindowEvent::Suspended/Resumed` → Monitor-Signale. `build_main_window` bleibt (`.content_protected(true)` L241 ist auf Android ein No-op: tao 0.37.1 `window.rs` L1216 „iOS / Android / Linux: Unsupported“).
- `state.rs`: Bridge-Import L15, Felder L87-88, `TrayMenu` L26/L194, `set_tray/with_tray` L313-324 unter `cfg(desktop)`; `tray_available()` auf mobile `false`; `signal_monitor` L344 verliert `allow(dead_code)`.
- `commands.rs`: Bridge/Terminal/Portable-Commands gegated; `AppInfo.platform` bekommt `"android"` (`platform.rs` L12-20 liefert heute „linux“; `types.ts` L234 Union erweitern); `AppInfo.read_only: bool` (ein Flag statt zehn Plattform-Abfragen in der UI); `apply_settings` Tray/Bridge-Seiteneffekte gegated.
- `update.rs`: Plugin-Teile unter `cfg(desktop)`; `version_label` (L84-89, `KEYSTEAD_VERSION_LABEL`), `endpoint` L66, `release_url` L74, `UpdateInfo` L94 bleiben.
- `platform.rs`: `open_url` (L68-79, `open`-Crate = xdg-open auf Android, wirkungslos) bekommt `#[cfg(mobile)]`-Zweig über `tauri_plugin_opener::OpenerExt` (Signatur um `&AppHandle` erweitern); `open_folder`/`open_terminal` desktop.
- `pwned.rs`: Commands bleiben registriert, Rumpf `#[cfg(mobile)]` → `unsupported:platform`.
- Neues `src/mobile.rs` (`#[cfg(mobile)]`): Datenordner, Logger, Clipboard-Backend, `import_vault_file(uri)`: `fs().open(FilePath::Url, read)` → ≤ 64 MiB lesen → `header_of` → Ziel `<data>/vaults/<id>.keystead` per `format::write_atomic` (L530); gleiche id nur bei höherer Revision ersetzen (sonst `conflict`), nie während der Tresor offen ist (`locked`). Kern als pfadbasierte Funktion → auf dem Host testbar.
- **Lebenszyklus/Sperre:** `monitor.rs` (`MonitorSignal` L31-34) bekommt `Background`/`Foreground`; Sperre nach `lockInBackgroundSeconds`, **aber frühestens, wenn kein kopiertes Geheimnis mehr auf seinen Clear-Timer wartet** (sonst zerstört „Sperren löscht die Zwischenablage“, `state.rs` L506-519, das Einfügen in die andere App). So bleibt der dokumentierte Vertrag unverändert. Vorsicht: `RunEvent::Exit`/`Core::shutdown` (`gui.rs` L130-136, `state.rs` L738) laufen auf Android praktisch nie – nichts Sicherheitsrelevantes darf daran hängen.
- **Kotlin in `gen/android`:** `MainActivity.kt`: `window.setFlags(FLAG_SECURE, FLAG_SECURE)` in `onCreate` (Ersatz für `WDA_EXCLUDEFROMCAPTURE`; Guard-Test per `include_str!` wie `gui.rs` L272-282), `onWebViewCreate`: `allowFileAccess=false`, `allowContentAccess=false`, Geolocation/Popups aus, Safe Browsing aus (**zu verifizieren**, was `RustWebView.kt` setzt); `AndroidManifest.xml`: `android:allowBackup="false"` + `dataExtractionRules` (E8), `windowSoftInputMode="adjustResize"`, **keine `INTERNET`-Permission** im Release-Manifest (nur in `src/debug/` für `tauri android dev`), `OnBackPressedCallback` → Event an die Seite (Zurück-Geste).
- **Biometrie (1b):** nur als **Schlüssel-Wrap**: 32-Byte-Master-KEK → Android-Keystore (AES/GCM, `setUserAuthenticationRequired(true)`, BIOMETRIC_STRONG, Invalidierung bei neuer Biometrie) → `<data>/device-unlock/<vault-id>.json`. Entsperren: Prompt → KEK → `unlock_with_master_kek` → `install_vault_since` (gleicher Pfad wie das Passwort). Jeder Fehler (Passwort am PC geändert → `KeyChanged`; Keystore-Key ungültig) löscht den Blob und fällt aufs Passwort zurück. **Kein PIN/Muster-Fallback.** `authenticate()` ohne Cipher gilt nie als Entsperren. Der KEK läuft **nie durchs WebView** – Rust-API des Plugins **zu verifizieren**, sonst eigenes ~100-Zeilen-Kotlin-Plugin (E5).

### UI (eine React-App, additive Handy-Schicht)

- Schalter: `info.platform === "android"` (oder `matchMedia("(max-width: 700px)")` im Mock) → `document.documentElement.dataset.layout = "phone"` neben `data-theme` (`App.tsx` L33). Neue `styles/phone.css` nur unter `:root[data-layout="phone"]`: `.shell` (`layout.css` L3-8) und `.vault-view` (L259-264) einspaltig, Sidebar als Drawer, `.icon-btn` ≥ 44 px (heute 32/28, `components.css` L140-179), Body 16 px, `.fieldrow-actions` Abstand 8 px (heute 2 px), `.row-actions`/`.nav-row-actions` immer sichtbar statt hover (L451-464, L172-190), `:hover` in `@media (hover: hover)`, `.kbd`/`.search-kbd`/StartPanel-Kürzel aus, `100vh` → `100dvh` (`layout.css` L6, `pages.css` L68, `components.css` L1031), `viewport-fit=cover` + `env(safe-area-inset-*)` (`index.html` L5 hat es nicht; ob das WebView `env()` liefert: **zu verifizieren**). Settings-TOC: die Zeilenvariante aus `pages.css` L1687-1718 greift, Breakpoint ggf. auf das Phone-Selector ausweiten.
- `MainScreen.tsx` L675-731: Stapel statt Grid – Liste wenn `selectedId === null`, sonst Detail; Zurück = bestehender Escape-Zweig L603-607 (`setSelectedId(null)`); Hardware-Zurück → Escape-Layer (`lib/layers.ts` L17-56: Modal → Menü → Drawer → Detail → App in den Hintergrund). `ItemList.tsx` L325-351 bekommt einen Menü-Knopf, `ItemView` einen Zurück-Pfeil.
- Gates nach `platform`/`readOnly`: `FileDropProvider` nicht mounten (`App.tsx` L308), `PairingModal` + `bridge://*` weg (L160-180, L326-332), `StartPanel` ohne `browser_status` (L93-106), `WelcomeScreen` ohne `ExtensionCard` (L317-383), `SettingsPage` ohne `browser`/`data`/`advanced` und ohne Tray-/Systemsperre-Schalter, About-Label „Android“, `UpdateBanner` aus (Stufe 1). Neu/Bearbeiten/Papierkorb/Anhänge-Schreiben hinter `readOnly`.
- `MainScreen.tsx` L124 `document.hasFocus()`-Gate für `touch_activity`: auf Android auf `visibilityState === "visible"` reduzieren (**zu verifizieren**, ob `hasFocus()` im WebView true ist – sonst greift die Auto-Sperre mitten im Tippen).
- `useRevealedSecret.ts` (L8 = 30 s, L54-55 `blur`/`visibilitychange`) passt zu `onPause`, wenn das WebView `visibilitychange` feuert (**im Emulator prüfen**); doppelter Boden: Rust emittiert bei `Suspended` ein `app://suspended`, auf das derselbe Hook hört.
- Autofokus (`UnlockScreen.tsx` L108, `Modal.tsx`) auf dem Handy aus, sonst springt die Tastatur bei jedem Dialog auf. Unlock-Screen: Fingerabdruck-Knopf + Auto-Prompt (1b), „Tresordatei öffnen …“ (ruft `pickOpenFile` aus `api.ts` L360-363 – auf Android kommt eine `content://`-URI zurück).
- i18n: neue Keys in **beiden** Katalogen (`nav.menu`, `common.back`-Verwendung, `unlock.biometric*`, `lock.backgroundToast`, `settings.lockInBackground*`, `settings.deviceUnlock*`, `attachments.phoneHint`, `about.platformAndroid`, `vaultFile.*`, `phone.readOnlyHint`, `error.unsupported:device_unlock_invalid`).
- Entwicklung ohne Gerät: `npm run dev` + Chrome-DevTools-Gerätemodus 360×800/412×915 (`?mock=empty`, `?lang=en`, Passwort `demo`), dann Emulator `MB_S26`.

### Netz in Stufe 1: keins

Keine `INTERNET`-Permission, kein reqwest im APK. Preis: keine Update-Meldung, keine Icons, kein HIBP auf dem Handy. Gewinn: beweisbar offline (Manifest), keine TLS-Panikfalle, Android-Proxys (die `icons.rs` L334-350 nicht sehen kann) sind kein Thema. Die Update-Meldung kommt mit Stufe 2 (siehe E6).

## Stufe 2 – Tresor aufs Handy und zurück (eigener Cloud-Ordner)

Grundlage für alles: der Core kennt nur Dateipfade; `write_atomic` (Temp-Datei + rename), `.bak`, `<vault>.lock` (`File::try_lock`, `format.rs` L739) und der Side-car `<id>.keystead.attachments/` funktionieren **nicht** auf `content://`-URIs. Deshalb: **Arbeitskopie bleibt im App-Ordner, Sync = ganze Dateibytes kopieren + Revisionsvergleich.** In den Cloud-Ordner kommt nur `<id>.keystead` – nie `.bak`/`.lock`/`.tmp`/Export-Dateien.

### Stufe 2a – Einweg PC → Handy, Handy bleibt lesend (keine Konflikte möglich)

> **Stand 2026-10-17 – Handy-Seite umgesetzt** (Vertrag: ARCHITECTURE „Cloud-folder sync (phone)“), mit zwei Abweichungen vom Text unten: das Handy verknüpft einen **Ordner** (`ACTION_OPEN_DOCUMENT_TREE`, nur Lesefreigabe), damit der Anhang-Ordner mitkommt – das „spätere Inkrement“ ist also schon drin; der Zustand liegt in `<data>/sync/link.json` (ein Ordner, nicht je Tresor). **Kein Update-Hinweis**: die App bleibt ohne `INTERNET` (Entscheidung dieses Laufs, E6 dafür zurückgestellt). Offen: die PC-Seite (E7).
>
> **Stand 2026-10-17 – PC-Seite (E7) umgesetzt** (Vertrag: ARCHITECTURE „Cloud-folder mirror (desktop) and phone link“, `src-tauri/src/mirror.rs`): Einstellung `mirrorFolder` unter Einstellungen → „Handy“. Abweichungen vom Text unten: der Spiegel schreibt auch den Anhang-Ordner (Blobs vor der Tresordatei, veraltete Blobs erst danach entfernt); er läuft auf einem eigenen Thread (Fehler blockieren das Speichern nie, ein Toast je neuem Fehler); gespiegelt werden die in der App geöffneten Tresore – nach jedem erfolgreichen Speichern, beim Öffnen und nach einer vom Monitor übernommenen Fremdänderung; eine höhere Revision im Ordner wird nie überschrieben (`unsupported:mirror_newer`); Löschen eines Tresors entfernt seine Kopie. Die Doku-Pflicht-Formulierung („Deine Daten bleiben verschlüsselt bei dir …“) steht jetzt in Willkommen, „Über Keystead“ und README.

- **PC:** neue Einstellung „Kopie im Cloud-Ordner aktuell halten“ (Ordner wählen, z. B. `…\Nextcloud\Keystead\`): nach jedem erfolgreichen `commit` (Hook in `Core::mutate`, `state.rs` L605-651) Kopie von `vault.path()` nach `<Ordner>/<id>.keystead` per `write_atomic`. ~150 Zeilen Rust + eine Settings-Zeile (E7).
- **Handy, erstes Mal:** Welcome-Kachel „Tresor vom PC holen“ → eigenes **Mini-Kotlin-Plugin** `KeysteadSafPlugin` in `gen/android` (~100-150 Zeilen): `ACTION_OPEN_DOCUMENT` + `FLAG_GRANT_PERSISTABLE_URI_PERMISSION` + `takePersistableUriPermission(READ|WRITE)`, `release(uri)`. Der offizielle Dialog taugt dafür nicht: `DialogPlugin.kt` L62-63 nutzt `ACTION_GET_CONTENT` (Kommentar `// TODO: ACTION_OPEN_DOCUMENT ??`), L204 `ACTION_CREATE_DOCUMENT`, kein `takePersistableUriPermission` (grep in dialog/fs: 0 Treffer). Der Nutzer tippt im System-Picker auf „Nextcloud“ (die App ist ein `DocumentsProvider`), wählt die Datei → Kopie nach `<data>/vaults/<id>.keystead`, Verknüpfung in `<data>/sync/<vault-id>.json` `{uri, lastSyncedRevision, lastSyncedDigest, lastSyncedAt}` (nicht in `settings.json`, s. o.).
- **Danach:** beim Start, bei `Resumed` (max. alle 30 s) und auf „Jetzt abgleichen“: Remote über `tauri-plugin-fs` (`FilePath::Url`, Modus `r`) lesen → `header_of` → Revision vergleichen: neuer → Copy-in (`write_atomic`; wenn entsperrt `reload_if_changed()` `vault.rs` L900 → `vault://changed`; `KeyChanged` → sperren, Biometrie-Blob löschen, Toast „Master-Passwort wurde am PC geändert“); gleich → nichts; älter → nichts, Status „Cloud-Kopie ist älter als dein Handy“. Offline-first: die lokale Kopie bleibt immer benutzbar. Nie im Hintergrund ohne offene App, kein Service, keine `READ/WRITE_EXTERNAL_STORAGE`.
- **Fallback ohne Cloud (billig, offizielle Plugins):** `bundle.fileAssociations` (`ext: keystead`, `mimeType` Pflicht für den Android-Intent-Filter) → `RunEvent::Opened { urls }` → Dialog „Tresor ‚X‘ Revision N übernehmen?“ → `import_vault_file`. URL-Format auf Android **zu verifizieren** (vermutlich `content://`).
- **Update-Meldung** kommt hier mit (E6): das Mini-Plugin bekommt `fetchText(url)` über `HttpsURLConnection` (System-TLS, System-Proxy, ~30 Zeilen); Rust parst `latest.json` (`endpoint()` `update.rs` L66), vergleicht per `semver`, liefert `UpdateInfo { canInstall: false, releaseUrl }` – die bestehende „portabel“-UI (`update.tsx` L109-110, `UpdateBanner.tsx` L114/L140-146) öffnet die APK-URL im Systembrowser. Ab hier `INTERNET`-Permission; reqwest bleibt trotzdem draußen.
- Anhänge: nur die Tresordatei wird synchronisiert; Einträge bleiben, `read_attachment` → `not_found` → UI „Anhänge sind nur am PC verfügbar“. Ordner-Sync (`ACTION_OPEN_DOCUMENT_TREE`, Blobs **vor** der Tresordatei und vor dem Entsperren kopieren, sonst räumt `sweep` nach 1 h weg) = späteres Inkrement.

### Stufe 2b – Bearbeiten am Handy + Rückweg

- Read-only aufheben (Schreib-Commands registrieren, Editor im Phone-Layout). Nach jedem Speichern Copy-out über dieselbe URI (Modus `wt`, ganze Bytes; Nextcloud lädt beim Schließen des Descriptors asynchron hoch), danach Remote erneut lesen und Revision bestätigen.
- **Konflikterkennung im Core** (`sync::decide`, rein, host-testbar): ein Revisionszähler erkennt keine Divergenz (PC 12→13 und Handy 12→13 getrennt → gleiche Revision, anderer Inhalt; `reload_if_changed` meldet `Ok(false)`). Deshalb `SyncHead { id, revision, digest }` mit `digest` = SHA-256 über `payload.ciphertext` + `wrappedKey` (ohne Passwort berechenbar) und die persistierte Basis `lastSyncedRevision`: ids verschieden → `OtherVault`; remote > local == base → `PullRemote`; local > remote == base → `PushLocal`; gleich + gleicher Digest → `UpToDate`; sonst `Diverged`. **Keine Formatänderung** (ARCHITECTURE L135-237 bleibt).
- **Konfliktdialog** (Handy und PC gleich, in einfachen Worten): „Der Tresor wurde am PC und am Handy geändert, seit beide zuletzt abgeglichen wurden.“ Knöpfe: [Stand vom PC übernehmen] / [Stand vom Handy behalten] / [Später]. Die verlierende Seite wird als `<id>.keystead.conflict-<Datum>` im jeweiligen Datenordner behalten (nie im Cloud-Ordner) und ist über den bestehenden Import mit Dublettenprüfung nachholbar. **Kein automatischer Merge** – das wäre neuer Core-Code mit Regeln für Ordner/Tags/Icons/Historie/Anhänge. Rollback: eine ältere Remote-Revision wird nie still eingespielt.
- Cloud-Datei ist **untrusted input**: Größenlimit 64 MiB, `parse` + KDF-Grenzen vor jedem Schreiben in `vaults/`; Header-Felder (`kdf`, `wrappedKey`, `name`) sind nicht authentifiziert, aber die AAD bindet `id:revision` – ohne Schlüssel kein Klartext. Der Cloud-Anbieter sieht Tresorname (Klartext im Header!), Revision und Änderungszeiten – das muss in die Doku.
- Passwortwechsel nur am PC (Handy-UI hat die Zeile nicht), damit `KeyChanged` nur in eine Richtung auftritt.

### Alternativen (verworfen / aufgeschoben)

- `tauri-plugin-android-fs` (Community, v30.x, ein Maintainer): spart das Mini-Plugin, bringt aber ein großes Fremd-Crate in die Vertrauensbasis eines Passwort-Managers – alle drei Entwürfe bevorzugen die eigenen ~120 Zeilen Kotlin.
- Syncthing(-Fork): für Keystead identisch zum SAF-Fall, nichts zu bauen; offizielles syncthing-android ist eingestellt (Okt 2024), Fork mit Vertrauensdiskussion – nur dokumentieren.
- LAN-Sync per QR: eigenes Protokoll, mDNS/MulticastLock, `ACCESS_LOCAL_NETWORK` ab targetSdk 37, Zertifikat-Pinning, Windows-Firewall – größter Aufwand, größte Angriffsfläche; du hast es „vorerst nicht“ gesetzt.
- Datenordner direkt im Cloud-Ordner (`KEYSTEAD_DATA_DIR`): `.lock`/`.tmp`/`.bak` im Sync + Cloud-Client-Konflikte mit dem rename – nein.

## Stufe 3 – Android-Autofill (zuletzt)

Machbar, aber reines Kotlin + JNI ohne Tauri-Vorlage (kein Tauri-Passwort-Manager mit Autofill bekannt; Vorbilder KeePassDX, Bitwarden mit UniFFI). Voraussetzungen: `minSdkVersion ≥ 26`, Stufe 1b (Keystore-KEK, sonst Passwort bei jedem Formular), Stufe 2 (aktuelle Daten). Chrome: seit 137 nur über „Autofill mit anderem Dienst“.

- `KeysteadAutofillService : AutofillService` (Manifest: `BIND_AUTOFILL_SERVICE`, Intent-Filter, `<meta-data android:name="android.autofill">`), **kein** `android:process` (gleicher Prozess wie die Rust-Lib). `onFillRequest`: `AssistStructure` → Felder mit `autofillHints`/`HtmlInfo autocomplete` → `webDomain` (API 26; nur https) oder `packageName` (nur Treffer, wenn der Eintrag eine `androidapp://<package>`-URI trägt – kein automatisches Paket↔Domain-Mapping, Phishing-Vektor).
- Rust-Seite: `#[no_mangle] extern "C" fn Java_<pkg>_autofill_Native_loginsFor(...)` (jni 0.21, bereits über Tauri da), `static CORE: OnceLock<Weak<Core>>`; ohne laufende Activity öffnet ein schlanker Pfad `VaultStore::new(data_dir).unlock_with_master_kek` nach `BiometricPrompt` in einer eigenen `AutofillAuthActivity` (FLAG_SECURE, kein WebView). Datasets ohne Secrets, Passwort erst bei Auswahl per zweitem JNI-Aufruf (`CharArray`, danach genullt); `logins_for_url` (`vault.rs` L309) liefert die Kandidaten.
- Minimalvariante zuerst (≈40 % des Aufwands): nur Browser/Domain, Nutzername + Passwort, kein `SaveInfo`/`onSaveRequest`, keine Inline-Suggestions. Aufwand realistisch 2-3 Wochen (Kotlin ~600-900 Zeilen, Rust ~200-400, Doku), E2E nur manuell im Emulator.

## Werkzeuge, CI, Verteilung

### CI (`.github/workflows/build.yml`)

Neuer Job `android` (ubuntu-24.04) parallel zu `windows` (L71-189), gleiche SHA-Pins: checkout → setup-node → `node .github/scripts/release-version.mjs --write` (deterministisch, gleiche `version`; neu: Output `android_version_code` und `--write` setzt `bundle.android.versionCode` – `setJsonVersion` L114-119 um einen verschachtelten Schlüssel erweitern) → `actions/setup-java` (temurin 21) → NDK aus dem Runner-Image (`ANDROID_NDK_LATEST_HOME`, **zu verifizieren**) oder `nttld/setup-ndk` r28+ → `dtolnay/rust-toolchain` 1.97.0 mit `targets: aarch64-linux-android` → `Swatinem/rust-cache` (`key: android`) → `npm ci` → Keystore nur unter der Publish-Bedingung (Muster L96/L120-121): Secret `ANDROID_KEYSTORE_BASE64` → `$RUNNER_TEMP/keystead.jks` + `gen/android/keystore.properties` → `npx tauri android build --apk --target aarch64 --ci` mit `KEYSTEAD_VERSION_LABEL` in der Env (Compile-Zeit-`option_env!`, `update.rs` L84-89) → Gates: `apksigner verify --print-certs` gegen den im Workflow gepinnten SHA-256-Fingerprint (fängt einen falschen Keystore ab), `zipalign -c -P 16 -v 4` (16-KB-Pages), `aapt2 dump badging` (minSdk, keine `INTERNET` in Stufe 1, nicht debuggable) → `release/Keystead-<version>-android.apk` + `sha256sum` → `upload-artifact`. Nicht-publizierende Builds: `--debug` (Debug-Keystore, `debugApplicationIdSuffix ".debug"`, installierbar neben der Release-App) als Workflow-Artefakt, kein Release-Asset. Zusätzlich im Job: `cargo clippy -p keystead-desktop --target aarch64-linux-android -- -D warnings` als Lint-Abdeckung für `cfg(mobile)`-Code (tauris `build.rs` verlangt `WRY_ANDROID_*` nur bei gesetztem `WRY_ANDROID_KOTLIN_FILES_OUT_DIR`, `tauri-2.12.2/build.rs` L286-288 – **zu verifizieren**, dass ein nackter Clippy-Lauf durchgeht).

`release`-Job: `needs: [test, windows, android]` (E9), zweiter `download-artifact`, Release-Notes (L229-241) um eine APK-Zeile mit Installationshinweis (Wortlaut der Android-Dialoge **im Emulator prüfen**), `files: release/*` nimmt die APK mit. `latest.json`/`latest-json.mjs` (L42-44 nur `windows-x86_64`) und der `updater-beta`-Schritt bleiben **unverändert** – Android prüft die APK-Signatur selbst, die App leitet die APK-URL wie `links.ts` L13-16 aus der Version ab.

### Versionierung

`versionName` = `version` (Pre-Release-Suffix erlaubt). `versionCode` = `GITHUB_RUN_NUMBER`: Tauris Formel `major*1e6+minor*1e3+patch` (`tauri-utils-2.10.1/src/config.rs` L3656-3657, Default `minSdkVersion` 24 L3690) gäbe **allen** 2.0.1-Builds denselben Code 2000001. Run-Nummern wachsen streng, Re-Runs behalten sie. Regel für CLAUDE.md: `build.yml` nie umbenennen (könnte den Zähler zurücksetzen → `INSTALL_FAILED_VERSION_DOWNGRADE`; **zu verifizieren**), `autoIncrementVersionCode` nicht nutzen (zweiter Zähler).

### Signatur-Keystore

Erzeugst du selbst mit dem vorhandenen JDK (Befehl unten): PKCS12, RSA-4096, 30 Jahre, Alias `keystead`. Ablage in `CedrickGD/keys/keystead/keystead-android-release.jks` + README-Eintrag (Alias, SHA-256-Fingerprint). GitHub-Secrets `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`. **Verlust wiegt schwerer als beim minisign-Schlüssel**: ohne denselben Key kann keine installierte App je aktualisiert werden. Fingerprint im README veröffentlichen.

### Verteilung

Asset `Keystead-<version>-android.apk` am Release (nur arm64-v8a; x86_64 nur lokal für den Emulator). Installation: APK im Handy-Browser laden → „Aus dieser Quelle zulassen“ → ggf. Play Protect „Trotzdem installieren“. Updates: App meldet neue Version (ab Stufe 2), Browser lädt die APK, Paketinstaller aktualisiert (gleicher Key + höherer versionCode). Nebeneffekt: `discord-notify.yml` L66-67 verlinkt das **erste** Release-Asset – alphabetisch künftig die APK → Filter auf `-windows-setup.exe`.

### Doku-Pflicht (Vertrag)

ARCHITECTURE: Intro L3-13 (**„No vault data ever leaves the machine“** präzisieren: verschlüsselte Datei an einem von dir gewählten Ort, kein Keystead-Server, Metadaten beim Anbieter sichtbar), Repository layout L31-44 (`gen/android`), Executable modes L46-71, Data locations L104-133 (Android-Zeile: app-privat, `sync/`, `device-unlock/`, kein portabler Modus/Bridge/Extension), Settings L687-707, Command-Tabelle ab L709 („nicht auf Android“ + neue Commands), neuer Abschnitt „Android“ (Lifecycle, Clipboard-Semantik, FLAG_SECURE, Mini-Plugin, Threat-Model), In-app updates L1653, Releases L1809 (android-Job, versionCode, Keystore). HANDOFF L256-264 ersetzen, Keys-Tabelle L9-15, lokale Voraussetzungen, Emulator-Checkliste. README de/en (Tagline, Funktionen, Download/Installation, Updates, Datenorte, Build from source, Project layout – dort `legacy`-Zeile entfernen). CLAUDE.md (Layout, `npx tauri android dev/build`, Rules: Keystore nie committen, Identifier nie ändern, Workflow nie umbenennen, `gen/android` committen). `de.ts` L129/L839 „Deine Daten verlassen nie dieses Gerät“ → „Deine Daten bleiben verschlüsselt bei dir – kein Keystead-Server, kein Konto. Du entscheidest, ob eine verschlüsselte Kopie in deiner eigenen Cloud liegt.“ (de + en).

## Was auf deinem PC installiert bzw. gesetzt werden muss (alles kostenlos)

Laut Memory frage ich vor dem Installieren – deshalb hier die exakten Befehle (PowerShell), die du freigibst oder selbst ausführst:

```powershell
# 1. Umgebungsvariablen (JDK 21 ist schon da, SDK ist schon da)
[Environment]::SetEnvironmentVariable("JAVA_HOME", "C:\Program Files\Android\openjdk\jdk-21.0.8", "User")
[Environment]::SetEnvironmentVariable("ANDROID_HOME", "$env:LOCALAPPDATA\Android\Sdk", "User")
# (neue Shell öffnen, damit die Variablen gelten)

# 2. NDK + Platform 37 nachinstallieren (sdkmanager braucht JAVA_HOME)
& "$env:LOCALAPPDATA\Android\Sdk\cmdline-tools\latest\bin\sdkmanager.bat" --list | Select-String "ndk;"   # verfügbare r28+-Version ablesen
& "$env:LOCALAPPDATA\Android\Sdk\cmdline-tools\latest\bin\sdkmanager.bat" --install "ndk;28.2.13676358" "platforms;android-37"
[Environment]::SetEnvironmentVariable("NDK_HOME", "$env:LOCALAPPDATA\Android\Sdk\ndk\28.2.13676358", "User")

# 3. Rust-Targets: in 1.97.0 bereits installiert (verifiziert); nur rust-toolchain.toml ergänzen.
rustup target add --toolchain 1.97.0 aarch64-linux-android x86_64-linux-android   # no-op, zur Sicherheit

# 4. Ausgangslage beweisen (erwartet: Fehler in clipboard.rs wegen arboard)
cargo check -p keystead-core --target aarch64-linux-android

# 5. Android-Projekt erzeugen, Emulator, Dev-Loop
cd apps\desktop; npm ci; npx tauri android init
& "$env:LOCALAPPDATA\Android\Sdk\emulator\emulator.exe" -avd MB_S26
npx tauri android dev
npx tauri android build --apk --debug --target x86_64   # Emulator-APK
adb install -r <apk>; adb logcat -s RustStdoutStderr:* Tauri:*

# 6. Signatur-Keystore (einmalig, Passwort merken; Ablage in CedrickGD/keys/keystead/)
& "C:\Program Files\Android\openjdk\jdk-21.0.8\bin\keytool.exe" -genkeypair -v -keystore keystead-android-release.jks -storetype PKCS12 -alias keystead -keyalg RSA -keysize 4096 -validity 10950 -dname "CN=Keystead, O=CedrickGD"
& "C:\Program Files\Android\openjdk\jdk-21.0.8\bin\keytool.exe" -list -v -keystore keystead-android-release.jks   # SHA-256-Fingerprint für README/CI
```

Die genaue NDK-Version (`28.2.13676358` stammt aus der AGP-9.4-Tabelle) **im `--list` nachsehen**; Platform 37 verlangt das Tauri-CLI-2.12-Template (`compileSdk = 37`) – Gradle lädt sie evtl. selbst nach (`licenses/` ist akzeptiert). Emulator-Fingerabdruck: Extended Controls → Fingerprint → Touch, oder `adb -e emu finger touch 1`.

## Risiken (die wichtigsten)

1. **Core baut nicht für Android**, bis arboard gegated ist – erster Schritt ist ein Compile-Check, keine UI-Arbeit. Weitere Crates (reqwest wird entfernt; `zxcvbn`, `psl`, `sys-locale`, `cpufeatures`) sind nur per cfg-Lektüre plausibel.
2. **TLS-Panikfalle** bei jedem reqwest-Pfad auf Android – darum reqwest target-gated und Netz erst in Stufe 2 über Kotlin. Ein versehentlich nicht gegateter Aufruf (Icons-Scheduler, HIBP) stürzt die App beim ersten Netzzugriff ab.
3. **Zwischenablage**: Timer-Thread stirbt mit dem Prozess, `Core::shutdown` läuft nie, Hintergrund-Lesen verboten → unbedingtes Löschen; ob `clearPrimaryClip()` aus dem Hintergrund wirkt, ist undokumentiert (Fallback: beim nächsten `Resumed`; Härtung: `AlarmManager`-Receiver). Vor API 33 keine Vorschau-Unterdrückung.
4. **Screenshot-/Recents-Schutz** hängt an drei Zeilen Kotlin in `gen/android` – Guard-Test per `include_str!` ist Pflicht.
5. **Datenordner**: ohne Override landet alles im `temp_dir()`; der Override muss die erste Zeile in `setup()` sein; exakter Pfad im Emulator prüfen.
6. **Sync**: Revisionszähler erkennt keine Divergenz (Digest + Basis nötig); Nextcloud lädt asynchron hoch und loggt Konflikte nur; **OneDrive als DocumentsProvider ist unbelegt** und im `google_apis`-Emulator (kein Play Store) nicht testbar; URI-Rechte verfallen beim Verschieben/Löschen der Datei.
7. **Kein echtes Gerät**: Fingerabdruck, Play-Protect-Dialoge, 16-KB-Geräte, arm64-APKs (laufen im x86_64-Emulator nicht) sind nur indirekt prüfbar → zusätzlich ein x86_64-Release-Build als CI-Artefakt für den signierten Pfad.
8. **Keystore/versionCode**: Verlust = keine Updates mehr; falsches Secret = nicht updatefähige APK (Fingerprint-Gate); Workflow-Umbenennung könnte den Zähler zurücksetzen.
9. **Toolchain-Drift**: Tauri-CLI-Updates heben Gradle/AGP/compileSdk an; mit committetem `gen/android` bewusst nachziehen. 16-KB-Alignment ohne NDK ≥ r28 → Warndialoge/Abbrüche; `zipalign -P 16` in CI.
10. **Argon2id 64 MiB/t3/p4** auf dem Handy: Dauer unbekannt (1-3 s geschätzt), nicht pro Gerät senkbar (gemeinsamer Header) – Biometrie mildert.
11. **cfg(mobile)-Blindheit**: der Linux-`test`-Job kompiliert Mobile-Code nie; reine Logik plattformneutral halten + Android-Clippy im CI-Job.
12. **Doku-Versprechen** „No vault data ever leaves the machine“ kollidiert mit Stufe 2 – Wortlaut überall präzisieren.

## Entscheidungen für dich

**E1 – Paketname/Identifier (blockierend vor der ersten APK, danach unveränderlich).** (a) `io.github.cedrickgd.keystead` nur für Android über `tauri.android.conf.json`; (b) `de.cedrickgd.keystead`; (c) `com.keystead.desktop` behalten (heutiger Identifier, `tauri.conf.json` L6; „desktop“ im Namen einer Handy-App, Domain nicht in deinem Besitz); (d) Identifier global ändern – **nicht** empfohlen, der WebView2-Datenordner am PC wandert mit. **Empfehlung: (a)**; vorher verifizieren, dass die Plattform-Konfigurationsdatei den Identifier überschreibt (sonst `--config` in CI).

**E2 – Mindest-Android (`bundle.android.minSdkVersion`).** (a) 24 = Android 7, Tauri-Default (dein AVD `MB_API24`); (b) 26 = Android 8 (Autofill braucht genau das); (c) 29 = Android 10 (Zwischenablage für Hintergrund-Apps nicht lesbar, `clearPrimaryClip` ab 28, Scoped Storage). **Empfehlung: 29** – ein echter Sicherheitsunterschied für einen Passwort-Manager, und Geräte von 2019 sind noch dabei. Später änderbar, daher nicht blockierend.

**E3 – Sperren beim Verlassen der App (Standardwert).** Einstellung „Sofort / 1 min / 5 min / Nie“; die Sperre wartet in jedem Fall, bis ein kopiertes Geheimnis seinen Clear-Timer hinter sich hat (sonst kein Einfügen in die andere App). **Empfehlung: 1 min** als Standard (Passwort einfügen und für den 2FA-Code zurückkommen geht ohne erneutes Entsperren); „Sofort“ erst sinnvoll mit Fingerabdruck.

**E4 – Handy in Stufe 1 und 2a nur lesend?** (a) Ja: keine Schreib-Commands, null Konfliktfälle, Bearbeiten kommt mit 2b; (b) Nein: ab Stufe 1 bearbeiten, Handy-Änderungen leben bis Stufe 2 nur auf dem Handy, 2a muss sofort Divergenz behandeln. **Empfehlung: (a)** – alle drei Entwürfe stimmen überein.

**E5 – Fingerabdruck-Entsperrung (Stufe 1b).** (a) KEK-Wrap im Android-Keystore über `tauri-plugin-biometric`, falls dessen Rust-API `biometric_cipher` existiert (nicht lokal prüfbar); (b) eigenes ~100-Zeilen-Kotlin-Plugin (StrongBox, `setUnlockedDeviceRequired`, eigener Alias; später vom Autofill-Dienst mitnutzbar); (c) keine Biometrie, immer Master-Passwort (Argon2 1-3 s + Tippen). In (a)/(b) **kein** PIN/Muster-Fallback. **Empfehlung: (a) wenn verifiziert, sonst (b)**; Opt-in, Standard aus.

**E6 – Darf die Handy-App ins Netz (Update-Meldung)?** (a) Stufe 1 ohne `INTERNET`, ab Stufe 2 `latest.json`-Check über Kotlin-`HttpsURLConnection` im Mini-Plugin (System-TLS/-Proxy, reqwest bleibt draußen); (b) reqwest + `webpki-roots` via `use_preconfigured_tls` (reqwest im APK, keine System-CAs); (c) `rustls-platform-verifier` per JNI initialisieren (Gradle-Dependency + Kotlin; würde auch Icons/HIBP erlauben, deren Proxy-Regel auf Android aber blind bleibt); (d) nie online, Update-Hinweise über Obtainium/GitHub. **Empfehlung: (a)**; Icons und HIBP auf Android dauerhaft aus.

**E7 – Desktop-Seite des Syncs.** (a) Neue Einstellung „Kopie im Cloud-Ordner aktuell halten“ (Spiegel nach jedem Speichern, nur `<id>.keystead`); (b) du kopierst die Datei aus `%LOCALAPPDATA%\Keystead\vaults\` selbst. **Empfehlung: (a)** – neuer Code in der veröffentlichten 2.0-App, deshalb deine Zustimmung.

**E8 – `allowBackup=false` (kein Google-Auto-Backup/Gerätetransfer der App-Daten).** (a) Ja: Tresor, Settings und Biometrie-Blob werden nie von Android gesichert; Sicherung = PC/Cloud-Datei; (b) Nein: Standard-Backup (bis 25 MB in Google Drive). **Empfehlung: (a)**; Konsequenz: ein nur am Handy angelegter Tresor ist bei Geräteverlust weg, wenn er nicht synchronisiert/exportiert wurde.

**E9 – CI-Umfang.** (a) Android bei jedem Push bauen (Debug-APK als Artefakt, +10-20 min), Release wartet auf `android`; (b) APK nur bei `[release]`/Tags, auf Branches nur `cargo clippy --target aarch64-linux-android` + Guard-Tests; (c) Android nicht blockierend (`if: always()`), Releases auch ohne APK. **Empfehlung: (a)** mit rust-cache – Konsistenz schlägt Minuten; (b), wenn dir die Laufzeit zu hoch wird.

**E10 – Keystore erzeugen und verwahren (blockierend für das erste Release).** (a) Du erzeugst ihn lokal mit dem vorhandenen JDK (Befehl oben), legst ihn in `CedrickGD/keys/keystead/` ab und setzt die drei Secrets; (b) in CI erzeugen und herunterladen (privater Schlüssel entsteht auf einem fremden Runner). **Empfehlung: (a)**; RSA-4096, PKCS12, 30 Jahre, Fingerprint veröffentlichen.

**E11 – Reihenfolge nach Stufe 2a.** (a) 2b zuerst (Bearbeiten + Zwei-Wege + Konfliktdialog); (b) Autofill zuerst (rein lesend, passt auf 2a, größter Komfortgewinn beim Einloggen). **Empfehlung: (b)**, wenn du hauptsächlich nachschlägst/einloggst; (a), wenn du unterwegs Einträge pflegst.

**E12 – Werkzeuge auf dem PC (blockierend für lokale Android-Arbeit).** (a) Jetzt: Env-Variablen setzen, NDK + Platform 37 per sdkmanager (Befehle oben); (b) nur `cargo check --target aarch64-linux-android` (geht schon heute), Rest später. **Empfehlung: (a)** – alles kostenlos, nichts weiter nötig (kein Temurin, keine Rust-Targets).

**E13 – iOS-CI-Build (Widerspruch Memory „iOS nur als CI-Build“ vs. Aufgabe „Android ONLY“).** (a) Block 5 = nur Android; Gates heißen `mobile`/`desktop`, Android-Spezifisches `target_os = "android"`, damit ein späterer iOS-CI-Job wenig kostet; (b) iOS-CI-Job (macOS-Runner, Simulator-Screenshots) gleich mit einplanen. **Empfehlung: (a)**.


---

# Anhang: iOS-Recherche (CI-Build ohne Apple-Konto)


Facts gathered by the research agent (sources in the agent transcript; items marked "verify" are unconfirmed).

## Build without Apple account
- iOS commands only on macOS (Xcode). GitHub `macos-latest` = macOS 26 arm64, Xcode 26.x, simulator runtimes iOS 26.x, Rust/rustup, Node, CocoaPods, Homebrew; xcodegen NOT preinstalled. Free for public repos (no minute multiplier for public repos).
- Tauri CLI 2.12: `npx tauri ios init --ci` (installs xcodegen/cocoapods via brew without prompting) generates `gen/apple` (project.yml, xcodeproj, Sources, Assets.xcassets, LaunchScreen.storyboard, Podfile, ExportOptions.plist). `tauri ios build` never regenerates: commit `gen/apple` (generated once in CI, minus xcuserdata/build/Externals) or run init in every job.
- Unsigned simulator build: `npx tauri ios build --ci --debug --target aarch64-sim --no-sign` (`--no-sign` since CLI 2.11.0 → CODE_SIGNING_ALLOWED=NO etc.). Output `apps/desktop/src-tauri/gen/apple/build/arm64-sim/Keystead.app` (folder name = target arch; verify). Missing certificates only produce a warning; device SDK builds would fail without a team – simulator does not.
- Rust targets: aarch64-apple-ios-sim (sim), aarch64-apple-ios (device, only for an unsigned IPA nobody here can install), x86_64-apple-ios.
- Minimum iOS default 15.0 (`bundle.iOS.minimumSystemVersion`).

## Simulator in CI
- `xcrun simctl create/boot/bootstatus -b/install/launch --console/io screenshot/ui appearance dark/shutdown/delete` (flags verify on Xcode 26); or `futureware-tech/simulator-action` (pin SHA). No touch input without XCUITest → launch + screenshots only; a CI-only screenshot mode in the app (demo vault like mock.ts) makes the shots meaningful.
- Runtime ~15–25 min cold, 8–12 min warm (rust-cache keyed on target). Gate on workflow_dispatch / mobile paths.

## Plugin status on iOS
- biometric: works, but pass/fail only (no keychain-backed secret) → biometric unlock needs a custom Swift plugin (kSecAccessControlBiometryCurrentSet) – same class of work as Android Keystore.
- clipboard-manager: plain text only; expiry/localOnly pasteboard options not exposed → ~30 lines Swift if wanted.
- dialog: file picker yes (UIDocumentPicker, `file://` URLs, scoped mode), no folder picker; save via export.
- fs: app folder by default; security-scoped access only for the session – NO persistent bookmarks → keeping access to a vault in Files/Nextcloud/OneDrive needs a custom Swift plugin (bookmarkData minimalBookmark). Needs PrivacyInfo.xcprivacy (file timestamp reason C617.1).
- opener: URLs only. notification: ok. share: no official plugin. updater: desktop only (gate). single-instance: compile error on mobile → gate the dependency and `.plugin()`.
- Lifecycle: no reliable Rust-side background hook on iOS (tauri RunEvent has no Suspended) → lock-on-background via `visibilitychange`/`pagehide` in the webview (+ optional Swift notification observer). `contentProtected` has no iOS implementation → app-switcher snapshot shows content; mitigate with an overlay on pagehide.
- WKWebView origin `tauri://localhost` (gui.rs is_app_url already allows); safe area: `viewport-fit=cover` + env(safe-area-inset-*), use 100dvh, visualViewport handling for the keyboard.

## Distribution reality
- Paid Apple Developer Program (99 USD/yr) needed for App Store/TestFlight/ad hoc. Free Apple ID: personal team in Xcode on a Mac only (7-day profiles, 3 devices) or AltStore/SideStore from Windows with a physical iPhone. → CI-built unsigned .app runs only in the Simulator; an unsigned device IPA (`--target aarch64 --no-sign`) could be attached to releases with a disclaimer for people who sideload (untested here).

## Repo blockers (shared with Android unless noted)
- Binary-only crate → `[lib] name = "app_lib", crate-type = ["staticlib","cdylib","rlib"]`, `src/lib.rs` with `#[cfg_attr(mobile, tauri::mobile_entry_point)] pub fn run()`, main.rs keeps native-host/--cli dispatch under `#[cfg(desktop)]`.
- `tauri` feature `tray-icon` + tray.rs/TrayMenu → `#[cfg(desktop)]`; single-instance + updater + update.rs → target-gated dependencies (`[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]`; `cfg(desktop)`/`cfg(mobile)` aliases exist in code only, not in Cargo.toml tables).
- keystead-core `arboard` → pulls x11rb on iOS: gate clipboard.rs and the dependency on not(any(ios, android)); dirs/sys-locale fine; paths.rs falls into the Linux branch on iOS (check layout).
- keystead-bridge/keystead-tui not needed on phones → desktop-only deps; bridge.rs, extension.rs, monitor.rs, portable.rs, platform.rs, import_flow.rs (`open::that_detached`) → `#[cfg(desktop)]`; build.rs extension embedding gate on CARGO_CFG_TARGET_OS.
- iOS-only: `tauri.ios.conf.json` (plain visible window; no center/decorations/minWidth; contentProtected n/a), `src-tauri/Info.ios.plist` (NSFaceIDUsageDescription, UIFileSharingEnabled, LSSupportsOpeningDocumentsInPlace), `gen/apple/PrivacyInfo.xcprivacy`, AppIcon in Assets.xcassets (`tauri icon`, verify), identifier `com.keystead.desktop` becomes the bundle id (decide now; changing later orphans data).
- `cargo check --target aarch64-apple-ios` on Linux: practically no (ring/cc need the iOS SDK, swift-rs) → iOS compile errors only in the macOS job; mitigate by running clippy for aarch64-linux-android on Linux to cover shared `cfg(mobile)` code.

## CI job sketch (`ios-simulator`, macos-latest, timeout 60 min)
checkout → rust-toolchain 1.97.0 + targets aarch64-apple-ios-sim → rust-cache(ios-sim) → setup-node 22 + npm ci → (brew install xcodegen; `tauri ios init --ci --skip-targets-install` if gen/apple not committed) → `tauri ios build --ci --debug --target aarch64-sim --no-sign` → sanity ls/plutil → simctl create+boot(+status_bar override) → install, launch --console (log), wait for a ready log line, screenshots light+dark → upload-artifact (shots, log, zipped .app; retention 14 d) → simctl shutdown/delete (always).

## Effort beyond Android
~1–2 days for the build/CI pipeline, ~1–2 days blind UX (safe area, keyboard, switcher overlay), ~0.5–1 day per native Swift plugin (bookmarks, keychain+biometrics, pasteboard options). CI +~30 % wall-clock on mobile PRs. Money: zero.
