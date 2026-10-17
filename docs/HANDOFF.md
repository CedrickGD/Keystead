# Übergabe – Stand Keystead (2026-10-17)

Kurzfassung für eine neue (lokale) Claude-Sitzung. Projektregeln stehen in
[`CLAUDE.md`](../CLAUDE.md), die technischen Verträge in
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## Repository & Branches

| | |
|---|---|
| Repo | `CedrickGD/Keystead` (öffentlich; früher `Vault-X`, alte URLs leiten weiter) |
| `main` | **Keystead 2.0.0** (Tag `v2.0.0`, 2026-10-15, PR #12 als Merge-Commit mit voller Historie). VaultX 1.x ist auf Wunsch des Besitzers vollständig entfernt (`legacy/`, `version.yml`). |
| Entwicklung | Feature-Branches (`claude/…`), Merge per PR mit Merge-Commit nach `main`. Der alte Branch `claude/quirky-archimedes-i4qa3j` ist gemergt und kann gelöscht werden. |
| Schlüssel | privates Repo `CedrickGD/keys`, Ordner `keystead/` (Update-Signaturschlüssel, Erweiterungs-Schlüssel, Android-Keystore `keystead-android-release.jks` mit Passwortdatei, README) |
| GitHub-Secrets | `TAURI_SIGNING_PRIVATE_KEY` (Inhalt von `keys/keystead/tauri-updater.key`); für die APK `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` (= `keystead`) – alle im Keystead-Repo gesetzt (die Android-Secrets am 2026-10-16) |

Lokal starten:
```sh
git clone https://github.com/CedrickGD/Keystead
cd Keystead
cd apps/desktop && npm ci && npx tauri dev
```
Voraussetzungen unter Windows: Rust (rustup, Version kommt aus
`rust-toolchain.toml`), Node 22, Visual Studio Build Tools (MSVC + Windows SDK),
WebView2 (ist auf Win 10/11 vorhanden).

## Releases & Updates

- Commit auf dem Branch mit **`[release]`** in der Commit-Nachricht → GitHub Actions
  (`.github/workflows/build.yml`) baut auf `windows-latest`, versioniert als
  `<version aus tauri.conf.json>-beta.<Run-Nummer>` (seit v2.0.0 also
  `2.0.1-beta.…`), signiert das Setup und veröffentlicht eine
  **Pre-Release** (portable ZIP, Setup, Erweiterungs-ZIP, `.sig`, `latest.json`).
  Außerdem wird `latest.json` im festen Pre-Release `updater-beta` ersetzt –
  daraus holen installierte Apps ihre Updates (Beta-Kanal).
- Ab dem nächsten veröffentlichten Build hängt an jedem Release zusätzlich
  `Keystead-<version>-android.apk` (Job `android`, mit dem Release-Keystore
  signiert; CI bricht ab, wenn das Zertifikat nicht
  `B7:3D:75:…:3C:AA` ist). `versionCode` = Run-Nummer – deshalb
  `.github/workflows/build.yml` **nie umbenennen**. Die Android-App
  aktualisiert sich nicht selbst; neue APK über die alte installieren. Bis
  `import_vault_file` da ist, kann die veröffentlichte APK keinen Tresor
  anlegen oder übernehmen – Release-Notes und README nennen sie deshalb
  „frühe Vorschau“, die sich nur installieren und starten lässt.
- **`[skip ci]`** für Zwischenstände (kein Build, keine Discord-Meldung).
- Stabile Version: Tag `vX.Y.Z` → Release als „Latest“. Danach `version` in
  `apps/desktop/src-tauri/tauri.conf.json` hochzählen (sonst sortieren spätere
  Betas darunter; CI warnt).
- Betas sind Pre-Releases (nie „Latest“); der Stabil-Kanal – seit 2.0.0 der
  Standard für neue Installationen – sieht sie nicht. Bestehende
  Installationen behalten ihren gespeicherten Kanal.
- Prüfen eines Releases: `latest.json` von
  `https://github.com/CedrickGD/Keystead/releases/download/updater-beta/latest.json`
  laden; Signatur gegen `keys/keystead/tauri-updater.key.pub` prüfen (minisign,
  Ed25519 über BLAKE2b-512 der Datei).
- Die App legt die Browser-Erweiterung selbst nach
  `%LOCALAPPDATA%\Keystead\browser-extension` (Version `<x.y.z>.<Run>`) und die
  Erweiterung lädt sich nach App-Updates selbst neu. Der Besitzer hat sie seit
  beta.7 aus diesem Ordner geladen.

Veröffentlicht: beta.2 … **beta.11** (beta.7: Sicherheits-Block, beta.8: Plugin
smarter, beta.9: Tags, Assistenten-Schalter, Windows-Bridge-Fixes, beta.10:
Dateianhänge, beta.11: Prüfung auf geleakte Passwörter) und **v2.0.0** als
stabiles Release (2026-10-15, „Latest“). Der Besitzer testet unter Windows
(Installer-Version, Brave/Chromium-Browser) und bekommt neue Versionen per
In-App-Update (sein Kanal: wie in den Einstellungen gespeichert).

## Was fertig ist

- Desktop-App: Tresore (Logins, Karten, Identitäten, Notizen, Ordner, Favoriten,
  Papierkorb, Zusatzfelder, Passwortverlauf), Generator, Sicherheitsbericht,
  Auto-Sperre, Tray, Wiederherstellungsschlüssel, mehrere Tresore,
  portabler Modus, Deutsch/Englisch, hell/dunkel.
- Import per Drag & Drop mit Formaterkennung (Chrome/Edge/Firefox/Bitwarden CSV,
  Bitwarden JSON, Keystead-Export, VaultX 1.x), Vorschau, Duplikat-/Konflikt-
  Behandlung (überspringen/aktualisieren/beide behalten); vereinfachter Export.
- Website-Icons (direkt von den Websites, verschlüsselt im Tresor, abschaltbar
  unter Einstellungen → Allgemein und beim allerersten Start im Schritt „Tresor“
  des Einrichtungsassistenten; bei Proxy „fail closed“).
- Tags zusätzlich zu Ordnern: freie Schlagwörter je Eintrag (höchstens 20 à
  50 Zeichen; der Core trimmt, entfernt leere und doppelte – ohne Rücksicht
  auf Groß-/Kleinschreibung, die erste Schreibweise bleibt). Sidebar-Gruppe
  „Tags“ mit Zähler als Filter (Schreibweisen „Bank“/„bank“ sind eine Zeile
  und ein Filter, wie im Core), Umbenennen/Löschen eines Tags über alle
  Einträge inkl. Papierkorb (ein offener Editor übernimmt die Änderung),
  Chips in der Detailansicht (klickbar → Filter; im Papierkorb nur Anzeige)
  und im Editor (Enter/Komma, Vorschläge, Limits inline; Strg+S übernimmt
  noch getippten Text wie der Speichern-Knopf), die Suche findet Tags, ein
  neuer Eintrag im Tag-Filter startet mit dem Tag. Keystead-Export nimmt
  Tags mit, CSV/Bitwarden-JSON nicht. Die TUI zeigt sie nur an (Zeile
  „Tags: …“ unter der Typzeile) und bewahrt sie beim Bearbeiten.
  Veröffentlicht mit beta.9.
- Verschlüsselte Dateianhänge an Einträgen (Block 3, D2–D6; Entscheidungen
  des Besitzers vom 2026-10-10): bis zu 10 Dateien à 20 MiB je Eintrag, jede
  als eigene verschlüsselte Datei (`KSAT1` + Nonce + XChaCha20-Poly1305,
  eigener Zufallsschlüssel je Anhang im Tresor-Payload – eine Schlüsselrotation
  des Tresors fasst die Dateien nicht an) im Ordner
  `vaults/<id>.keystead.attachments/<anhang-id>.bin`; die Tresordatei trägt
  Version 2, solange Anhänge da sind, danach wieder 1. Der Core schreibt den
  Blob vor der Payload-Änderung und räumt ihn bei jedem Fehler (auch
  `Conflict`) wieder ab, löscht Einträge gelöschter Elemente bei jedem
  Speichern (Papierkorb behält seine), fegt verwaiste Dateien älter als 1 h
  beim Entsperren weg und entfernt den Ordner mit dem Tresor. Desktop:
  Abschnitt „Anhänge“ in der Detailansicht (Bildvorschau bis 8 MiB wie ein
  aufgedecktes Geheimnis – nach 30 s, bei Fokusverlust, Sperre und
  Elementwechsel wieder weg; „Speichern unter …“ und „Datei anhängen …“ nur
  über die nativen Dialoge, die Seite reicht nur Pfade weiter; „Entfernen“
  mit Rückfrage; Büroklammer-Chip mit Anzahl; Papierkorb nur lesend; kein
  Editor-Teil), Hinweis im Export-Dialog, Zeile mit Anzahl/Gesamtgröße plus
  Sicherungshinweis unter Einstellungen → Tresor; Mock-Backend mit
  Beispieldaten („Telekom Kundencenter“). Die TUI zeigt nur „Anhänge: N“
  unter der Tags-Zeile. Vertrag in `ARCHITECTURE.md` (Abschnitt
  „Attachments“). Veröffentlicht mit beta.10 und vom Besitzer am 2026-10-15
  in der echten App mit einer echten Datei geprüft (anhängen, Vorschau,
  „Speichern unter …“, Entfernen – funktioniert). Review-Fixes
  (2026-10-11): `save_attachment` lehnt relative Pfade und `.`/`..`-Bestandteile
  ab und löst den tiefsten existierenden Ordner auf (Symlinks, Groß-/
  Kleinschreibung, 8.3-Namen – vorher ließ sich mit `..` hinter einem noch
  nicht existierenden Ordner ein Blob im Tresor-Ordner überschreiben);
  `add_attachment` lehnt Dateien aus dem Datenordner ab
  (`invalid_input:attachment_source`), damit eine kompromittierte Seite die
  Tresordatei nicht über Anhängen + „Speichern unter …“ ohne Master-Passwort
  herauskopieren kann; `delete_vault` räumt `.bak`, Temp-Dateien und den
  Anhang-Ordner nach der Tresordatei best effort mit kurzem Retry ab – der
  Tresor gilt mit seiner Datei als gelöscht, die App schließt ihn auch, wenn
  der Ordner (Virenscanner) noch widersteht; Mock-Vorschau je Dateiname.
  Offen für den Besitzer: die Seite benennt weiterhin die Dialog-Pfade (so
  entschieden); wer das weiter einengen will, öffnet die Dialoge aus Rust
  (`tauri_plugin_dialog::DialogExt`, Befehle ohne `path`) oder verlangt für
  „Speichern unter …“ das Master-Passwort wie beim Export.
- Prüfung auf geleakte Passwörter (Block 3, D5/D6; Have I Been Pwned,
  k-Anonymität): nur auf Knopfdruck, kein Setting, nichts läuft automatisch.
  Core `crates/keystead-core/src/pwned.rs` (SHA-1 in Präfix 5 / Suffix 35,
  Hasher-Zustand und Digest werden genullt – `sha1` mit Feature `zeroize`;
  `Debug` zeigt nur den Präfix; Antwort-Parser mit Vergleich in konstanter
  Zeit über alle Zeilen, `Option<u64>`: `None`, wenn keine einzige Zeile
  `SUFFIX:COUNT` ist, also gar keine Range-Antwort vorliegt; Kandidaten je
  unterschiedlichem Passwort der nicht gelöschten Logins; ohne Netzwerk).
  Desktop `src-tauri/src/pwned.rs`: je Kommando ein neuer `reqwest::Client`
  wie beim Updater – reqwest liest den System-Proxy beim Bauen des Clients,
  ein gecachter Client würde also eine später ein- oder ausgeschaltete
  Proxy-Einstellung ignorieren, während die Proxy-Prüfung sie frisch liest
  (User-Agent `Keystead/<version>`, `Add-Padding: true`, `Accept:
  text/plain`, 10 s, 512-KiB-Deckel auch ohne `Content-Length`, keine
  Redirects, keine Cookies, kein Referer), System-Proxy mit „fail closed“
  bei PAC/unklarer Konfiguration (`unsupported:pwned_proxy`, geprüft vor der
  ersten Anfrage über `icons::proxy_setup_unclear`), Nicht-2xx →
  `io:HTTP <n>` ohne Retry, Netzfehler → `io:<detail>` ohne URL, ein 2xx
  ohne Trefferliste (Sperrseite, leerer Body) → `io:not a range answer`
  statt „nicht geleakt“;
  `check_pwned_password(password)` → Zähler und `check_pwned_vault(pageVaultId)`
  → `PwnedReport { checkedAt, checked, leaked: [{ itemId, count }], failed }`
  (Kandidaten + Lock-Epoche unter dem State-Lock, dann höchstens 4 parallele
  Anfragen je unterschiedlichem Präfix, Abbruch bei Sperre/Tresorwechsel/
  App-Ende (`locked`) und beim ersten HTTP-Fehler; ein Netzfehler vor der
  ersten Antwort bricht ab, einer danach zählt nur `failed` hoch; nach dem
  Lauf wird die Epoche erneut geprüft). UI: erste Karte „Geleakte Passwörter“
  im Sicherheitsbericht (Zustände nicht geprüft / prüfend / Ergebnis mit
  Zeilen nach Treffern sortiert / sauber / teilweise geprüft (gelb, wenn
  `failed` > 0 und kein Treffer; „{n} geprüft“ zählt nur beantwortete) /
  Fehler; Urteil, Zähler und Chip „{n} geleakt“ folgen dem Report – ein seit
  der Prüfung geänderter, markierter oder gelöschter Treffer verschwindet
  aus der Liste, die Karte bleibt rot mit Hinweis „… seit der Prüfung
  geändert oder gelöscht – prüfe erneut“; „Erneut prüfen“, „Zuletzt
  geprüft“, Hinweis bei `failed` > 0),
  Bestätigungsdialog vor jedem tresorweiten Lauf mit „Nicht mehr fragen“
  nur in `localStorage` (`keystead.pwnedConfirmed`), roter Callout mit
  „Passwort ändern“ in der Detailansicht, Link „Auf Leaks prüfen“ unter dem
  Stärke-Meter im Editor (Ergebnis hängt am geprüften Passwort). Ergebnis
  nur im Seitenspeicher je Tresor (`state/breaches.tsx`): verfällt je
  Element, sobald sich sein `updatedAt` ändert, und ganz beim Sperren oder
  Tresorwechsel; nichts wird gecacht oder in den Tresor geschrieben, Logs
  enthalten nur Zähler. Mock mit fester Treffertabelle
  (`src/lib/demo/pwned.ts`). Fehlertexte: `io:` → „Have I Been Pwned ist
  nicht erreichbar: …“, Proxy-Fall mit eigener Erklärung. Vertrag: Abschnitt
  „Leaked-password check“ in `ARCHITECTURE.md`. Geprüft: Core- und
  Desktop-Tests gegen einen lokalen Server (der einzige Live-Test ist
  `#[ignore]`), einmal lokal live gegen api.pwnedpasswords.com („password“:
  52.372.427 Treffer), Mock-Ablauf im Browser durchgeklickt. Review-Befunde
  (Proxy-Snapshot im gecachten Client, SHA-1-Zustand ungenullt, 2xx ohne
  Liste als „sauber“, grüne Karte trotz `failed` bzw. nach Änderung eines
  Treffers, Deckel-Test nur über `Content-Length`, Einleitungssatz in
  `ARCHITECTURE.md`) am 2026-10-15 behoben. Veröffentlicht mit beta.11 und
  vom Besitzer am 2026-10-15 in der echten App geprüft – funktioniert.
- Browser-Erweiterung: Kopplung mit Code, Entsperren, Tresor-Auswahl/-Wechsel,
  Ausfüllen (Popup, Inline-Menü, Strg+Umschalt+L), Speichern-/Aktualisieren-
  Leiste, Generator, Clickjacking-Schutz.
- Signierte In-App-Updates (Beta-/Stabil-Kanal), Erweiterung wird von der App
  ausgeliefert.
- Terminal-Version (`keystead-cli`, `Keystead.exe --cli`).
- Zwei Sicherheitsprüfungen mit gegengeprüften Funden, alle bestätigten behoben.
  Block 1 („Sicherheit vertiefen“): keine Geheimnisse mehr dauerhaft in der
  Oberfläche (redigierte Liste, Anzeigen/Kopieren/Bearbeiten auf Abruf),
  Schlüsselrotation beim Master-Passwort-Wechsel inkl. neuem
  Wiederherstellungsschlüssel.

## Abgeschlossen: Block 2 „Plugin smarter machen“ (beta.8)

Passwortvorschlag auf Registrierungs- und Passwort-ändern-Seiten,
2FA-Code nach dem Ausfüllen kopieren + „2FA-Code einfügen“ am Code-Feld,
Erkennung in offenen Shadow Roots (Web-Components), sichtbare Rückmeldung bei zu
frühen/verdeckten Klicks, 2FA-Countdown + Tastatursteuerung im Popup,
Plugin-Einstellungen (Popup → Einstellungen); Bridge:
`generate_password.remember`, `remember_generated`.

Erkennung Registrierung vs. Login (`extension/chrome/lib/forms.js`, Tests in
`extension/tests/forms.test.mjs`): Ein einzelnes Benutzer-/E-Mail-Feld ist
kein Login-Schritt, wenn sein Container schon ein Passwortfeld eines erkannten
Formulars hält (GitHub-artige Registrierung) oder wenn sich der Schritt nach
Registrierung liest. Es entscheiden die nächsten Wörter (Login-Wörter gewinnen
bei beidem): eigene Wörter des Schritts → seine Überschriften → die Überschrift
direkt über ihm auf seiner eigenen Vorfahren-Kette (nicht aus Seitenleisten,
`<aside>`/`<header>`-Promos oder Nachbarspalten) → Seitentitel/Pfad.
Popup-„Ausfüllen“ füllt auf ausdrücklichen Wunsch auch ein als Registrierung
erkanntes Formular; Inline-Menü und `Strg+Umschalt+L` bleiben dort aus.

Bekannte Grenze: Die Erkennung ist heuristisch. Fehlklassifizierte Seiten
lassen sich immer über das Popup ausfüllen.

## Lokal verifiziert (Windows, 2026-10-10)

Die Arbeit läuft seit beta.8 lokal auf dem Windows-PC des Besitzers (Rust
1.97.0 aus `rust-toolchain.toml`, MSVC aus Visual Studio Community 2026,
Node 22). Erster vollständiger Lauf der Prüfkette unter Windows: fmt, clippy,
`cargo test --workspace`, `npm run build`, `node --test` – grün, nachdem zwei
Windows-Fehler in `crates/keystead-bridge` behoben wurden (veröffentlicht mit
beta.9):

1. **Verbindungs-Timeout wurde ignoriert** (`socket.rs`): Die Local-Socket-
   Schicht von `interprocess` 2.4.4 verwirft unter Windows den Wait-Modus und
   wartet unbegrenzt (`WaitNamedPipe` mit `NMPWAIT_WAIT_FOREVER`). Hält die
   einzige Pipe-Instanz eine Verbindung, die der Server noch nicht angenommen
   hat, blockiert jeder weitere Verbindungsversuch – Host und Browser hingen
   bei einer eingefrorenen App endlos; drei Tests hingen deshalb. Jetzt wird
   über die typisierte Named-Pipe-API mit Timeout verbunden
   (`socket::connect_named_pipe`).
2. **Neustart des Bridge-Servers schlug fehl** (`server.rs`): Offene
   Host-Verbindungen halten unter Windows ihre Pipe-Instanz und damit den
   Namen (`FILE_FLAG_FIRST_PIPE_INSTANCE`); `stop()` + `start_server()`
   (Browser-Integration aus/ein, Portable-Modus, fehlgeschlagenes Update)
   endete mit „possible hijack“ und die Bridge blieb aus. `stop()` trennt
   offene Verbindungen unter Windows jetzt sofort (`DisconnectNamedPipe` über
   ein Handle-Duplikat) und wartet bis zu 2 s auf ihr Ende.

Die Entscheidungen des Besitzers zu Block 3 (D1–D8: Dateiversion, Limits,
Vorschau, Exporte, Auslösen und Netzwerkregel der Leak-Prüfung, Tag-Verwaltung,
Assistent) stehen als Protokoll in `docs/BLOCK3-DESIGN.md`; die Verträge dazu
in `ARCHITECTURE.md` (Abschnitte „Attachments“ und „Leaked-password check“,
Tags im Modell- und Command-Teil). Nicht erneut aufmachen.

## Nächste Schritte (vom Besitzer so priorisiert)

3. **Block 3 „Neue Funktionen“** ist abgeschlossen und veröffentlicht: Tags
   zusätzlich zu Ordnern (beta.9), verschlüsselte Dateianhänge an Einträgen
   (beta.10, vom Besitzer am 2026-10-15 in der echten App geprüft) und die
   Prüfung auf geleakte Passwörter (beta.11). Alles vom Besitzer in der
   echten App geprüft; nichts mehr offen.
4. **Block 4 „Stabile 2.0“ ist erledigt** (2026-10-15, Entscheidungen des
   Besitzers): PR #12 als Merge-Commit mit voller Historie nach `main`, Tag
   `v2.0.0` → stabiles Release („Latest“, auch auf dem Beta-Kanal), VaultX 1.x
   komplett entfernt (kein Hinweis-Release für 1.x-Nutzer; deren Updater
   findet keine `version.yml` mehr und meldet still kein Update, der Umstieg
   läuft über den Import), Standard-Update-Kanal „stabil“, danach `version`
   in `tauri.conf.json` auf 2.0.1.
5. **Block 5 „Mobile“** (Wunsch des Besitzers vom 2026-10-10, Entscheidungen
   vom 2026-10-16 in `BLOCK5-DESIGN.md`): Android-App, Stand und nächste
   Schritte im nächsten Abschnitt.

## In Arbeit: Block 5 „Mobile“ (Branch `claude/mobile-android`)

Entwurf und Entscheidungen des Besitzers: [`BLOCK5-DESIGN.md`](BLOCK5-DESIGN.md)
(Paketname `io.github.cedrickgd.keystead`, minSdk 29, Stufe 1 ohne Netz und
nur lesend, Sperre im Hintergrund, `allowBackup=false`). **Stufe 1a ist
fertig** (Commits `WIP: mobile 1a – … [skip ci]`; bis `97a389b` liegen sie
auf `origin/claude/mobile-android`, die späteren nur lokal – vor jedem
Umschreiben der Historie `git status -sb` prüfen, gepushte Commits nicht
umschreiben; der Vertrag steht in `ARCHITECTURE.md`, Abschnitt „Mobile
(Android)“):

1. **Core** (Commit `80ff650`): Clipboard-Backend-Trait (arboard nur auf dem
   Desktop), `paths::set_data_dir_override`, Settings-Felder für das Handy.
2. **App-Crate** (`ab87cc9`): `keystead_lib` (lib + bin), `cfg(desktop)`/
   `cfg(mobile)`-Gates, Handy-Kommandoliste (read-only), `src/mobile.rs`
   (Datenordner, Clipboard über `tauri-plugin-clipboard-manager`, Links
   über `tauri-plugin-opener`).
3. **Android-Projekt** (`97a389b`): `tauri.android.conf.json`,
   `gen/android` aus `npx tauri android init` (Gradle 9.6.1, AGP 9.3.1,
   Kotlin 2.2.10, SDK 37) committet und gehärtet – kein `INTERNET` im
   Hauptmanifest (nur `src/debug/` für `tauri android dev`),
   `allowBackup=false` + `dataExtractionRules`, `FLAG_SECURE`,
   WebView ohne Datei-/Content-Zugriff, Zurück-Geste als DOM-Event
   `keystead:back` (sonst `moveTaskToBack`), Release-Signatur aus
   `gen/android/keystore.properties` (gitignored; Fallback Debug-Key),
   Capabilities nach Plattform getrennt (`capabilities/mobile.json`);
   Guard-Tests in `gui.rs`. Debug-APK für den Emulator:
   `cd apps/desktop && npx tauri android build --apk --debug --target x86_64`
   (JAVA_HOME = JDK 17, ANDROID_HOME, NDK_HOME gesetzt; Ausgabe
   `gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`,
   ~200 MB wegen Debug-Symbolen; `adb install` über Streamed Install schlug
   mit leerer Meldung fehl – `adb push … /data/local/tmp/` + `pm install -r`
   funktioniert). Im Emulator MB_S26 (API 36, x86_64) startet die App und
   zeigt den Willkommensbildschirm (noch Desktop-Layout, Englisch nach
   Systemsprache); `adb screencap` liefert wegen `FLAG_SECURE` schwarz,
   `adb emu screenrecord screenshot <Ordner>` zeigt das Bild.
4. **Lebenszyklus** (`11b6cb4`): `WindowEvent::Suspended`/`Resumed`
   (`onPause`/`onResume`) → `MonitorSignal::Background`/`Foreground`;
   Sperre nach `lockInBackgroundSeconds` (Standard 60 s, 0 = sofort,
   null = nie) mit Grund `background` (Toast „Tresor gesperrt, weil
   Keystead im Hintergrund war“), aber nie bevor der Clear-Timer eines
   kopierten Geheimnisses abgelaufen ist (`monitor::background_lock_due`,
   reine Funktion mit Tests; Hintergrundzeit wie `idle_for` auf beiden
   Uhren); `Resumed` holt zuerst ein verschlafenes Leeren nach
   (`clipboard::clear_overdue_secret`, Wanduhr), prüft die Sperre ein
   letztes Mal und setzt dann zurück; jeder Monitor-Tick holt ein
   überfälliges Leeren ebenfalls nach. Event `app://suspended` blendet
   aufgedeckte Werte aus (`useRevealedValue`, jetzt auch während ein
   Aufdecken noch lädt). Android-Clipboard-Backend: im Hintergrund erst
   Leertext schreiben, dann `clear` (das Plugin-`clear` prüft
   `hasPrimaryClip()`, das ohne Fokus false ist, und täte sonst nichts).
   Einstellungszeile „Sperren im Hintergrund“ (Sofort / 1 min / 5 min /
   Nie) nur auf dem Handy; Mock: `?platform=android` (Sichtbarkeit der
   Seite = Vorder-/Hintergrund, `__keysteadMock.simulateBackgroundLock()`).
   (Das Leeren der Zwischenablage wurde in Punkt 6 geändert: jetzt immer
   erst Leertext, nicht nur im Hintergrund.)
   **Im Emulator geprüft** (MB_S26, Debug-APK, Test-Tresor per `run-as` in
   `/data/user/0/io.github.cedrickgd.keystead/vaults/` gelegt, Befehle per
   WebView-DevTools): Home → `app://suspended` sofort, `vault://locked
   {background}` nach der eingestellten Zeit (10 s bzw. 30 s, auch mit
   anderen Apps im Vordergrund); „Sofort“ + kopiertes Passwort mit 20 s
   Timer → Sperre erst nach 20,8 s, die Zwischenablage ist dann leer (das
   Leeren aus dem Hintergrund wirkt); Rückkehr vor Ablauf → keine Sperre;
   nach ~60–70 s im Hintergrund friert Android den Prozess ein
   (`isFrozen=true`), die Sperre (150 s) kam dann direkt bei der Rückkehr;
   Toast „Vault locked because Keystead was in the background“ (Englisch
   nach Systemsprache) sichtbar. Desktop-Kette grün, APK baut.

5. **CI + Doku** (`e9c9a06`): neuer Job `android` in
   `.github/workflows/build.yml` (ubuntu-latest, bei jedem Push und PR wie
   `windows`): dieselbe Version wie der Windows-Job (`release-version.mjs
   --write`, neu `android_version_code` = Run-Nummer →
   `bundle.android.versionCode` in `tauri.android.conf.json`; Tests:
   monoton, 1 … 2,1 Mrd., Fehler bei 0/NaN/Bruch), Temurin 17
   (`actions/setup-java` v5.7.0, SHA-gepinnt), Android-SDK des Runners plus
   NDK 28.2.13676358, Plattform `android-37.0` und Build-Tools 36.0.0 per
   `sdkmanager`, falls das Image sie nicht hat, Rust 1.97.0 mit beiden
   Android-Targets, `rust-cache` (Key `android`), Frontend, `cargo clippy -p
   keystead-core -p keystead-desktop --target aarch64-linux-android -- -D
   warnings`, Keystore nur bei der Publish-Bedingung (`android-apk.mjs
   keystore`: Secrets nur aus der Umgebung, Keystore nach `$RUNNER_TEMP`,
   `keystore.properties` mit `java.util.Properties`-Escaping, beides 0600
   und nach dem Build gelöscht; fehlt ein Secret, bricht der Build ab),
   `npx tauri android build --apk --target aarch64 --ci`, dann die Prüfungen
   `android-apk.mjs verify` (apksigner – bei Publish genau das Zertifikat
   `B7:3D:…:3C:AA` –, `zipalign -c -P 16 -v 4`, `aapt2 dump badging`: Paket,
   versionName/versionCode, minSdk 29, kein `INTERNET`, nicht debuggable,
   `arm64-v8a`; APK-SHA-256 und Zertifikat in der Job-Zusammenfassung) und
   das Artefakt `Keystead-<version>-android`. Release-Job: braucht jetzt auch
   `android`, lädt die APK, vergleicht die Versionen beider Builds und
   nennt die APK samt Fingerabdruck in den Release-Notes.
   `discord-notify.yml` verlinkt das Windows-Setup statt des ersten Assets
   (das wäre alphabetisch jetzt die APK) und die APK in einer eigenen
   Zeile. Ein Node-Test hält Fingerabdruck (Workflow, README de/en,
   ARCHITECTURE), Paketname und minSdk über alle Dateien gleich. Doku:
   ARCHITECTURE neuer Abschnitt „Mobile (Android)“ (der bisherige Teil
   „Android project“ ist dorthin umgezogen) plus Releases, Settings,
   Datenorte, Updates, Repository-Layout; README de/en Abschnitt
   „Android-App (Vorschau)“, Datenorte, Build from source, Project layout
   (veraltete `legacy`-Zeile entfernt); CLAUDE.md (Layout, Befehle, Regel
   „`build.yml` nie umbenennen“).
   **Lokal geprüft:** den CI-Ablauf nachgestellt – `release-version.mjs
   --write` mit Run 4242, `gen/android` ohne alle gitignorierten Dateien (wie
   ein frischer Checkout), `npx tauri android build --apk --target aarch64
   --ci` → grün in 16 min, `app-universal-release.apk` 11 MB (R8, ohne
   Debug-Symbole); `android-apk.mjs verify` darauf: versionCode 4242 und
   versionName 2.0.1-beta.4242 (das CLI liest `versionCode` aus
   `tauri.android.conf.json`), minSdk 29, targetSdk 37, nur `arm64-v8a`,
   16-KB-Ausrichtung in Ordnung, keine `INTERNET`-Berechtigung, nicht
   debuggable; mit dem lokalen Debug-Schlüssel signiert → ohne `--signer`
   grün, mit dem Release-Fingerabdruck wie gewollt abgelehnt. Das Escaping
   von `keystore.properties` ist mit Javas `Properties.load` gegengeprüft
   (Leerzeichen vorn, Backslash am Ende, `=`, `:`, `#`, Umlaute, Emoji).
   Der Publish-Weg mit einem **Wegwerf-Keystore** (PKCS12, Passwort mit
   Leerzeichen, Backslash und `=`, nicht der echte Schlüssel): `android-apk.mjs
   keystore` aus den Umgebungsvariablen → `gen/android/keystore.properties` →
   Gradle signiert die Release-APK damit → `verify --signer <Wegwerf-Fingerabdruck>`
   grün, dieselbe APK gegen `B7:3D:…` abgelehnt; Wegwerf-Keystore und
   `keystore.properties` danach gelöscht. Ein fehlendes Secret bricht mit
   klarer Meldung ab.
   **Nicht im Emulator geprüft:** der Emulator hing diesmal beim Start an
   einem Dialog, der die Einwilligung zu einem Absturzbericht (von einem
   früheren Emulator-Absturz) verlangt – ob die mit R8 gebaute Release-APK
   startet, ist deshalb offen (Punkt 2 der Checkliste); die Debug-APK für
   x86_64 baut weiter grün.
   **Nicht lokal prüfbar:** der echte CI-Lauf (Runner-Image, Laufzeit,
   Gradle-Cache, Paketnamen im `sdkmanager` des Images) und die Signatur
   mit den echten Secrets – das zeigt der erste Push ohne `[skip ci]` bzw.
   der erste `[release]`-Commit.

6. **Review-Fixes** (dieser Stand, `WIP: mobile 1a - review fixes`):
   * Zwischenablage: `PluginClipboard::clear` schreibt **immer** erst einen
     Leertext und ruft dann `clear` – nicht mehr nur im Hintergrund.
     Android erlaubt den Zugriff nach **Fensterfokus**, nicht nach
     Pause/Resume: gleich nach `Resumed` (vor dem Fokus – genau dann holt
     der Monitor ein verschlafenes Leeren nach), im geteilten Bildschirm
     mit der anderen App im Fokus und unter der Benachrichtigungsleiste
     ist die App „resumed“, aber ohne Fokus, und das Plugin-`clear` täte
     still nichts (das Geheimnis wäre danach nie mehr geleert worden).
     Preis: Android 13+ zeigt im Vordergrund evtl. seine „Kopiert“-Blase
     für den Leertext. `mobile::set_in_background` ist weg.
   * Links auf dem Handy: `platform::open_url` ruft das Opener-Plugin in
     einem eigenen Thread – der Navigations-Hook läuft auf dem UI-Thread,
     und der Plugin-Aufruf wartet auf genau diesen Thread (Deadlock/ANR
     beim Antippen einer Website). Der JS-Klick-Hook des Opener-Plugins ist
     aus (`open_js_links_on_click(false)`), sonst liefen
     `target="_blank"`-Links ins Leere (die Webview darf das Plugin nicht
     aufrufen).
   * Zurück-Geste: `MainActivity` setzt ihren Callback in
     `onWebViewCreate` erneut ganz nach oben – Tauris `AppPlugin` hat einen
     eigenen, der sonst zuerst liefe und bei vorhandener WebView-Historie
     `goBack()` machte statt `keystead:back` zu schicken.
   * `data_extraction_rules.xml`: statt der ungültigen Domain `device`
     jetzt `device_root`, `device_file`, `device_database`,
     `device_sharedpref` (Liste aus AGP-Lint `FullBackupContentDetector`).
   * Keystore: `*.jks`, `*.keystore`, `*.p12` in `.gitignore` (Wurzel und
     `gen/android`); `app/build.gradle.kts` bricht ab, wenn `storeFile`
     im Repository liegt.
   * APK-Gate: `android-apk.mjs` erkennt auch `uses-permission-sdk-23`
     (z. B. `INTERNET` aus einer Bibliothek) – mit Test.
   * `reveal_attachment` (Bildvorschau, nur lesend) ist jetzt auch auf dem
     Handy registriert (`attachments.rs` ist pro Funktion gegatet; Design
     „Stufe 1 – Drin“).
   * „Über Keystead“ zeigt „Android“/„iOS“ statt „Linux“.
   * Release-Notes und README de/en: die APK heißt „frühe Vorschau“ und
     sagt klar, dass sie noch keinen Tresor anlegen oder übernehmen kann;
     „Was schon geht“ steht jetzt unter „Sobald ein Tresor auf dem Handy
     liegt“.

   **Im Emulator geprüft** (MB_S26, API 36, Debug-APK x86_64, Test-Tresor
   mit Website und PNG-Anhang per `run-as`, Befehle per WebView-DevTools):
   Benachrichtigungsleiste heruntergezogen (Fokus bei `NotificationShade`,
   Keystead bleibt `topResumedActivity`), Passwort mit 8-s-Timer kopiert →
   danach ist die Zwischenablage leer; ebenso im Vordergrund und nach der
   Home-Taste (Gegenprobe: direkt nach dem Kopieren liefert Einfügen das
   Geheimnis). Website-Knopf und ein `<a target="_blank">` öffnen Chrome,
   Keystead reagiert weiter, kein ANR. Zurück-Geste mit WebView-Historie
   (`pushState`): mit Listener kommt sie als `keystead:back` an, die
   Historie bleibt unberührt; ohne Listener geht die App in den
   Hintergrund, der Prozess lebt, der Tresor ist bei der Rückkehr offen.
   Bildvorschau des Anhangs erscheint (`data:image/png`). „Über Keystead“
   zeigt „Android · …“. Gradle: `storeFile` im Repository → Abbruch mit
   Meldung, außerhalb → Konfiguration läuft. Nicht geprüft: geteilter
   Bildschirm/Freiform (gleiche Fokus-Lage wie die Benachrichtigungsleiste),
   der Release-Build (R8) mit diesen Änderungen.

7. **Handy-Layout** (`WIP: mobile 1b - phone layout`, Vertrag:
   ARCHITECTURE „Mobile (Android)“ → „Phone layout“): dieselbe React-App,
   `data-layout="phone"` auf `<html>` bei `platform` android/ios oder
   Fenster ≤ 700 px (`src/lib/layout.mjs` + Node-Test
   `apps/desktop/tests/layout.test.mjs`, Hooks in `src/lib/phone.ts`, Stile
   nur unter dem Attribut in `src/styles/phone.css`). Einspaltig:
   Seitenleiste als Schublade (Menü-Knopf vor der Suche bzw. in der Leiste
   der Seiten, Scrim, schließt bei jeder Auswahl, Escape und Zurück), Detail
   über der Liste gestapelt (Zurück-Pfeil; die Liste behält ihre
   Scrollposition), Tippflächen ≥ 44 px, Kopierknöpfe der Liste immer
   sichtbar, Hover-Hintergründe der gemeinsamen Stylesheets in
   `@media (hover: hover)` (mit Maus unverändert), Dialoge unten in voller
   Breite, Menüs bleiben im Fenster, kein Autofokus auf Textfelder.
   **Zurück-Geste** (`keystead:back`): `lib/layers.ts` behandelt sie wie
   Escape (Dialog/Menü/Schublade zuerst), dann `useBackHandler` der
   Hauptansicht: Editor → Detail → Suche leeren → von Seite/Filter zu „Alle
   Elemente“ → sonst Android (App in den Hintergrund). **Nur lesen**
   (`readOnly`): kein Neu/Bearbeiten/Favorit-Schalter (stattdessen Chip
   „Favorit“)/Papierkorb-Aktionen/Ordner- und Tag-Verwaltung/Anhänge
   ändern/Generator/Import, kein Master-Passwort, Wiederherstellungsschlüssel,
   Tresor umbenennen/löschen, auf dem Entsperrbildschirm kein „Passwort
   vergessen?“/„Neuen Tresor erstellen“; Chip „Nur lesen“ in der Schublade;
   jede Schreibaktion in `MainScreen` kehrt zusätzlich früh zurück.
   **Nur Desktop** (Plattform): Browser-Integration, Import/Export,
   Erweitert, Infobereich-Schalter, „Beim Sperren des Computers sperren“,
   Website-Icons, HIBP-Karte, „Speichern unter …“, Update-Banner/-Prüfung
   (auch `pending_update` beim Start). „Über Keystead“: „Android · Daten im
   App-Speicher“ und **„Releases auf GitHub öffnen“** (Entscheidung dieses
   Laufs: die App bleibt **ohne `INTERNET`-Berechtigung**, also keine
   Update-Prüfung auf dem Handy; der Link öffnet den Browser, dafür braucht
   die App keine Berechtigung). Willkommen ohne Tresor: Erklärung plus
   markierter Platz (`data-slot="vault-source"`) für „Tresor vom PC holen“
   der nächsten Stufe. **Android-Shell**: `MainActivity` polstert die
   Content-View um Statusleiste, Gestenleiste, Kamera-Aussparung und
   Tastatur (Insets-Listener, verbraucht die Insets) – die Webview sah sie
   nicht (`env()` lieferte nur die Aussparung, 0 für die Gestenleiste) und
   `adjustResize` wirkt bei Edge-to-Edge nicht; Fensterhintergrund =
   `--bg-app` hell/dunkel (`res/values*/themes.xml`, `colors.xml`);
   Guard-Test in `gui.rs`. Mock: `?platform=android` lehnt jedes
   Nicht-Handy-Kommando ab wie Tauri („Command … not found“, Warnung in der
   Konsole), `__keysteadMock.simulateBack()`.
   **Im Emulator geprüft** (MB_S26, Debug-APK x86_64, zweiter Test-Tresor
   „Handy-Demo“ mit 9 erfundenen Einträgen, Ordnern, Tags und TOTP per
   `run-as` – Passwort `demo`; „Handy-Test“ ist unverändert da):
   Kopfzeile unter der Statusleiste, Webview 360×724 CSS-px, mit offener
   Tastatur 449 px – das Passwortfeld bleibt sichtbar; kein Tastatur-Popup
   beim Öffnen; Entsperren per Tippen; Liste, Schublade, Detail (Aufdecken,
   2FA-Code kopieren, Toast), Einstellungen (nur Allgemein, Sicherheit,
   Über); echte Zurück-Taste: Schublade zu → Detail zu → von den
   Einstellungen zur Liste → an der Wurzel geht die App in den Hintergrund
   (Prozess lebt). Screenshots `phone-1b-*.png` im Scratchpad des Laufs.
   Auffällig: Androids eigene „Kopiert“-Blase (Android 13+) lag über dem
   Toast – Toasts sitzen auf dem Handy deshalb höher. **Nicht geprüft:**
   TalkBack (nicht eingeschaltet); `uiautomator dump` zeigte die per echtem
   Tippen geöffnete Schublade nicht, obwohl Chromiums Accessibility-Baum
   (DevTools) sie enthält und eine per Skript geöffnete auch im
   Android-Baum erscheint – vermutlich Chromes automatisches Abschalten der
   Accessibility bei Touch-Eingaben ohne Screenreader; mit TalkBack
   gegenprüfen. Querformat, Tablets, iOS.

8. **Cloud-Ordner-Sync, Handy-Seite** (Stufe 2a, `WIP: mobile 2a - folder
   sync`; Vertrag: ARCHITECTURE „Mobile (Android)“ → „Cloud-folder sync
   (phone)“): Das Handy verknüpft **einen Ordner** (nicht eine Datei) der
   eigenen Cloud über den System-Ordnerdialog (`ACTION_OPEN_DOCUMENT_TREE`,
   Nextcloud/OneDrive-App als DocumentsProvider) mit dauerhafter
   **Lese**-Freigabe – so kommen `<id>.keystead` **und** der Anhang-Ordner
   `<id>.keystead.attachments/` mit (die offene Frage zu den Anhängen ist
   damit gelöst). Nur Kopieren nach innen, nie zurück.
   * **Kotlin-Plugin** `KeysteadFolderPlugin.kt` in `gen/android`
     (`pickFolder`, `releaseFolder`, `listChildren`, `copyFile`;
     Provider-Arbeit auf eigenem Thread, Kopien nur nach
     `<app>/sync/incoming/`, kein Schreibzugriff auf den Ordner), aus Rust
     registriert als Plugin `keystead-folder` (`src/mobile/sync.rs`) – die
     Webview darf es nicht aufrufen (keine Capability, Tauri prüft die ACL
     für jedes `plugin:`-Kommando). Statt Base64 kopiert das Plugin die
     Dateien in einen Eingangsordner, Rust liest, prüft und löscht sie dort
     (Anhänge bis 20 MiB ohne JNI-Riesenstrings).
   * **Engine** `src/folder_sync.rs` (plattformneutral, Tests laufen auf dem
     PC): Tresordatei ≤ 64 MiB, Name `<id>.keystead` passend zur ID im
     Header, `VaultFile::parse` vor jedem Schreiben; Entscheidung
     `keystead_core::sync::decide_copy_in` (rein, getestet): unbekannt →
     neu, höhere Revision → atomar ersetzen, gleich → nichts, niedriger →
     Handy behält seine („Kopie am PC ist älter“), anderer Tresor an der
     Stelle → nie anfassen. Anhänge: fehlende Blobs (bzw. andere Größe)
     werden **vor** dem Ersetzen der Tresordatei gespeichert; nur `<id>.bin`
     mit Blob-Form (`attachments::looks_like_blob`). Offener Tresor: Datei
     unter dem State-Lock ersetzen und `reload_if_changed` →
     `vault://changed`; neues Master-Passwort → Sperre mit neuem Grund
     `keyChanged` (Toast „am PC geändert“); ein geändertes KDF-Salz markiert
     die Datei `passwordChanged` → der Entsperrbildschirm erklärt ein
     „falsches Passwort“.
   * **Wann**: beim Start, bei `Resumed` (höchstens alle 60 s, eigener
     Thread), nach dem Verknüpfen und auf „Jetzt synchronisieren“. Zustand
     in `<data>/sync/link.json` (Ordner-URI, Name, Zeiten, letzter Fehler,
     Ergebnis je Datei – keine Geheimnisse). Kommandos (nur Handy):
     `folder_status`, `link_folder`, `sync_now`, `unlink_folder`; Event
     `sync://status`.
   * **UI**: Willkommen (Platz `data-slot="vault-source"`): drei Schritte
     (PC-Einstellung „Kopie im Cloud-Ordner aktuell halten“, Cloud-App,
     Ordner wählen) + „Cloud-Ordner wählen“; verknüpft ohne Tresor: Hinweis,
     Ergebnisliste, „Jetzt synchronisieren“/„Anderen Ordner wählen“. Kommt
     ein Tresor an, wechselt die App zum Entsperren. Entsperren: Statuszeile
     (zuletzt synchronisiert + Knopf; Ordner verloren → „Ordner neu wählen“;
     nicht verbunden → „Verbinden“ – das ist auch der Weg zu weiteren
     Tresoren: sie liegen einfach im selben Ordner). Einstellungen →
     „Synchronisierung“ (nur Handy): Ordner, letzte Synchronisierung, Tresore
     im Ordner mit Ergebnis, „Jetzt synchronisieren“, „Ordner ändern“,
     „Verbindung trennen“ (mit Rückfrage; Kopien bleiben). Mock:
     `?platform=android` (+ `?mock=empty` / `?sync=linked`),
     `__keysteadMock.simulatePcSave()`, `simulatePcPasswordChange()` (neues
     Passwort `demo2`), `simulateFolderLost()`.
   * **Entscheidung dieses Laufs** (für den Besitzer): Die Handy-App bleibt
     **ohne `INTERNET`-Berechtigung** – keine Update-Prüfung auf Android,
     auch nicht über ein Kotlin-Plugin (E6 sah das für Stufe 2 vor);
     „Über Keystead“ bietet weiter „Releases auf GitHub öffnen“ (der Browser
     öffnet sich, keine Berechtigung nötig).
   **Im Emulator geprüft** (MB_S26, Debug-APK x86_64, Test-Tresor
   „Cloud-Test“ – Passwort `demo`, danach `demo2` – mit PNG-Anhang am PC
   erzeugt und nach `/sdcard/Download/KeysteadSync/` gelegt; „Handy-Test“
   und „Handy-Demo“ bleiben unverändert): Entsperren ohne Verknüpfung zeigt
   „Kein Cloud-Ordner verbunden · Verbinden“ → System-Dialog → Download →
   KeysteadSync → „Diesen Ordner verwenden“ → „Zulassen“ → Tresor und
   Anhang kopiert (`link.json`: `imported`, 1 Anhang), Bildvorschau des
   Anhangs funktioniert. Neue Revision am PC, App kurz verlassen und zurück
   → automatischer Sync lädt den offenen Tresor neu (neuer Eintrag
   sichtbar). Master-Passwort am PC geändert → „Jetzt synchronisieren“ →
   Sperre mit Toast; altes Passwort → Hinweis „am PC geändert“, neues
   entsperrt. Ordner verschoben → „Keystead kann den Cloud-Ordner nicht
   mehr lesen“ + „Ordner neu wählen“; zurückgeschoben und App beendet
   (`force-stop`) → der Start-Sync liest mit der dauerhaften Freigabe, der
   Fehler ist weg. „Verbindung trennen“ → `link.json` weg, Freigabe
   zurückgegeben, Tresore bleiben. Dialog abbrechen → nichts ändert sich.
   Screenshots `phone-2a-*.png` im Scratchpad. **Nicht geprüft:** echte
   Nextcloud/OneDrive-Apps (das Emulator-Image hat keinen Play Store – nur
   der eingebaute Speicher-Provider), das Entziehen der Freigabe über die
   Android-Einstellungen, Querformat, der R8-Release-Build mit dem Plugin
   (die Consumer-ProGuard-Regeln von tauri-android behalten
   `@TauriPlugin`/`@InvokeArg`-Klassen).
   **Bekannt:** Der Ordnerdialog ist eine andere Activity – mit „Sperren im
   Hintergrund: Sofort“ sperrt ein offener Tresor beim Ordnerwechsel in den
   Einstellungen (Verknüpfen und Sync laufen trotzdem zu Ende). Der Toast
   „Synchronisiert …“ geht verloren, wenn derselbe Sync den Tresor wegen
   `keyChanged` sperrt (die Seite lädt neu; der Sperr-Toast bleibt).
   Verknüpft man einen anderen Ordner, bleiben die Tresore des alten auf dem
   Handy (ohne „nicht mehr im Ordner“-Eintrag).

### Emulator-Checkliste für den Besitzer

Ohne Handy prüfst du die App im Emulator `MB_S26` (API 36, x86_64). Befehle
in Git Bash; vorher `ANDROID_HOME="$LOCALAPPDATA/Android/Sdk"` setzen,
`adb` liegt in `$ANDROID_HOME/platform-tools`.

1. Emulator starten: `"$ANDROID_HOME/emulator/emulator.exe" -avd MB_S26
   -no-snapshot-load` – der Kaltstart dauert hier mehrere Minuten; fertig,
   wenn `adb shell getprop sys.boot_completed` `1` liefert. Fragt der
   Emulator nach einem Absturzbericht, erst den Dialog beantworten – vorher
   bootet er nicht weiter.
2. APK wählen: die selbst gebaute Debug-APK (`npx tauri android build
   --apk --debug --target x86_64`, Pfad
   `apps/desktop/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`)
   oder – für den echten Release-Weg – `Keystead-<version>-android.apk`
   aus dem Release bzw. dem CI-Artefakt (arm64 – ob das x86_64-Image ARM-Apps übersetzt, zeigt
   `adb shell getprop ro.product.cpu.abilist`: steht dort `arm64-v8a`,
   läuft sie; das ist auch der Test, ob der R8-optimierte Release-Build
   startet).
3. Bei einem Wechsel zwischen lokal gebauter, CI- und Release-APK erst
   deinstallieren (andere Signatur; lokale Builds haben außerdem den
   höheren versionCode 2000001): `adb uninstall io.github.cedrickgd.keystead`
   – das löscht die Daten der App im Emulator.
4. Installieren: `adb push <apk> /data/local/tmp/keystead.apk` und
   `adb shell pm install -r /data/local/tmp/keystead.apk` (`adb install`
   scheiterte hier mit leerer Meldung).
5. Tresor aufs Handy holen (Cloud-Ordner-Sync; nimm einen
   **Test-Tresor**, nicht deinen echten): `<id>.keystead` und ggf. den Ordner
   `<id>.keystead.attachments` in einen Ordner auf dem Emulator legen, z. B.
   `adb shell mkdir -p /sdcard/Download/KeysteadSync` und `adb push
   <id>.keystead /sdcard/Download/KeysteadSync/` (Git Bash:
   `MSYS_NO_PATHCONV=1` davor). In der App „Cloud-Ordner wählen“ (bzw.
   „Verbinden“ auf dem Entsperrbildschirm) → Download → KeysteadSync →
   „Diesen Ordner verwenden“ → „Zulassen“. Eine neuere Kopie pushen und die
   App kurz verlassen (oder Einstellungen → Synchronisierung → „Jetzt
   synchronisieren“) – die App übernimmt sie. Geht mit Debug- und
   Release-APK; den Ordner `Download` selbst lässt Android nicht freigeben,
   nur einen Unterordner.
6. Durchklicken: Start → Willkommen/Entsperren → Liste und Suche →
   Passwort aufdecken (nach 30 s wieder verborgen) → kopieren (die
   Zwischenablage des PCs spiegelt die des Emulators; nach 30 s leer) →
   2FA-Code → Website eines Eintrags antippen (der Browser öffnet sich,
   Keystead friert nicht ein) → Bildanhang ansehen (nur wenn der Ordner
   `<id>.keystead.attachments` mitkopiert wurde) → Sicherheitsbericht →
   Einstellungen → „Sperren im Hintergrund“ (Sofort / 1 min / 5 min / Nie)
   → „Über Keystead“ zeigt „Android“.
7. Home-Taste: nach 1 Minute ist der Tresor gesperrt (Toast beim
   Zurückkehren); die Zurück-Geste schickt die App in den Hintergrund, statt
   sie zu schließen; die Vorschau in „Zuletzt verwendet“ und ein Screenshot
   (`adb exec-out screencap -p > shot.png`) sind schwarz (`FLAG_SECURE`).
   Ein Bild für Fehlerberichte: `adb emu screenrecord screenshot <Ordner>`.
8. Logs: `adb logcat -s RustStdoutStderr:* Tauri:*` – erwartet sind nur
   Chromium-Zeilen des WebView, keine Panik.

Erwartete Grenzen in dieser Stufe: die automatische Kopie vom PC in den
Cloud-Ordner fehlt noch (nächste Stufe, E7) – die Tresordatei kommt bis
dahin von Hand in den Ordner; bearbeiten geht nur am PC (die
Handy-Oberfläche zeigt keine Bearbeiten-Knöpfe).

### Nächste Schritte (laut Design, Reihenfolge des Besitzers)

1. **Stufe 2a, PC-Seite** (E7): Einstellung „Kopie im Cloud-Ordner aktuell
   halten“ (Ordner wählen; nach jedem erfolgreichen Speichern
   `<id>.keystead` per `write_atomic` in den Ordner, die Blobs aus
   `<id>.keystead.attachments/` **vorher** – nie `.bak`, `.lock`, `.tmp`).
   Die Handy-Seite ist fertig (Punkt 8); ein eigener Einzeldatei-Import
   (`import_vault_file`) ist damit überflüssig geworden. Englischer Name der
   Einstellung, wie ihn die Handy-Texte schon nennen: „Keep a copy in a
   cloud folder up to date“.
2. **Stufe 1b**: Entsperren per Fingerabdruck (Opt-in, `deviceUnlock`,
   KEK-Wrap im Android-Keystore über `unlock_with_master_kek`; E5). Ein
   gespeicherter KEK wird nach `keyChanged` (Sync) wertlos – dann löschen.
3. Update-Hinweis auf dem Handy: nach der Entscheidung dieses Laufs **nicht**
   (keine `INTERNET`-Berechtigung, siehe Punkt 8); nur wieder aufnehmen,
   wenn der Besitzer das will.
4. Danach **Autofill** (E11), dann **2b** Bearbeiten am Handy +
   Zwei-Wege-Sync mit Konfliktdialog.
5. **iOS-Simulator-Job** in CI (E13, kostenloser macOS-Runner, ohne
   Signatur, nur Screenshots).

Bekannte Grenzen: Zwischenablage auf dem Handy – wird der Prozess
eingefroren (nach ~1 min im Hintergrund) oder vom System beendet, läuft
der Clear-Timer nicht; ein Geheimnis mit langem Timer bleibt bis zur
Rückkehr, zum nächsten Kopieren oder bis Android es selbst verwirft (ab
Android 13 nach einer Stunde); der Standard (30 s) ist nicht betroffen
(Härtung: Kotlin-`AlarmManager`-Receiver). Die Tests des App-Crates
kompilieren für Android nicht (`--all-targets` scheitert an Desktop-Tests
in `update.rs`, `commands.rs`, `secrets.rs`, `state.rs`, `error.rs`) –
der CI-Job lintet deshalb ohne `--all-targets`. Jede nicht
veröffentlichende CI-APK ist mit dem Debug-Schlüssel des Runners signiert,
der von Lauf zu Lauf wechseln kann: zwischen zwei solchen APKs ggf.
deinstallieren.

## Bekannte Grenzen / offene Punkte

- EXE ist nicht code-signiert → SmartScreen-Warnung. Der Besitzer will nichts
  Kostenpflichtiges; eine kostenlose Option für Open-Source-Projekte wäre
  SignPath Foundation (Antrag durch den Besitzer nötig).
- Windows-spezifischer Code wurde in der Cloud nur per Cross-Clippy geprüft.
  Seit 2026-10-10 läuft die komplette Prüfkette (fmt, clippy, alle Tests, npm,
  node) auch lokal auf dem Windows-PC des Besitzers – dabei kamen zwei reine
  Windows-Fehler der Bridge zum Vorschein, beide behoben (siehe „Lokal
  verifiziert“). CI testet weiterhin nur unter Linux; Windows-Tests laufen nur
  lokal.
- Website-Icons verraten Netzwerk-Mitlesern die Liste der Websites (dokumentiert,
  Schalter vorhanden); hinter Firmen-Proxys werden bewusst keine Icons geladen.
- Prüfung auf geleakte Passwörter: nur auf Knopfdruck, keine automatische
  Prüfung und kein Setting dafür; Have I Been Pwned ist nur direkt oder über
  einen Proxy erreichbar, dessen Konfiguration die App auswerten kann – bei
  PAC-Skript oder unverständlicher Proxy-Einstellung sendet sie nichts
  (`unsupported:pwned_proxy`); das Ergebnis wird nicht gespeichert (nach dem
  Sperren oder einem Tresorwechsel wieder weg); nicht in TUI und Erweiterung;
  „Nicht mehr fragen“ lässt sich nur durch Löschen der WebView-Daten
  zurücksetzen.
- Verschlüsselte Bitwarden-Exporte, 1Password-/KeePass-Formate: nicht unterstützt.
- TUI: Ordner, Tags, Zusatzfelder, mehrere Websites, Papierkorb nicht
  bearbeitbar (Tags werden angezeigt und beim Bearbeiten bewahrt); Anhänge
  nur als Anzahl („Anhänge: N“), kein Anhängen/Speichern.
- Anhänge: in keinem Export enthalten (Sicherung = Datenordner, also
  Tresordatei plus Ordner `<id>.keystead.attachments`); kein „Öffnen mit“ –
  nur die Bildvorschau in der App (PNG/JPEG/GIF/WebP bis 8 MiB) und
  „Speichern unter …“ über den nativen Dialog; ältere Keystead-Versionen vor
  beta.9 öffnen einen Tresor mit Anhängen nicht (Dateiversion 2,
  `unsupported:vault format version 2`), beta.9 öffnet ihn und bewahrt die
  ihr unbekannten Einträge; keine Gesamtgrenze je Tresor, 20 MiB je Datei,
  10 je Eintrag.
- `Keystead.exe --cli <Unterbefehl>` öffnet eine Konsole, die sich sofort
  schließt – für Skripte `keystead-cli.exe` in einem Terminal nutzen.
- Erweiterung: geschlossene Shadow Roots sind nicht erreichbar (technische
  Grenze); Klicks in den ersten 500 ms nach Erscheinen werden bewusst ignoriert
  (mit Rückmeldung).
- CI: GitHub warnt wegen Node-20-Actions (`actions/checkout@v4`,
  `actions/setup-node@v4`) – irgendwann auf neuere Major-Versionen heben.
- Die Ende-zu-Ende-Testskripte (Playwright + echter Chromium + Debug-App) lagen
  nur im Scratch-Verzeichnis der Cloud-Sitzung und sind nicht im Repo. Für
  künftige Arbeit ggf. neu unter `tests/e2e/` anlegen.
