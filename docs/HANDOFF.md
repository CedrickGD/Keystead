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
  aktualisiert sich nicht selbst und sucht auch nicht nach Updates (keine
  `INTERNET`-Berechtigung); neue APK über die alte installieren, „Über
  Keystead“ verlinkt die Releases. Release-Notes und README nennen sie
  „frühe Vorschau“: nur lesen, die Tresore kommen über den Cloud-Ordner
  (PC-Einstellung „Kopie im Cloud-Ordner aktuell halten“), einen Tresor
  anlegen kann sie nicht.
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
(Paketname `io.github.cedrickgd.keystead`, minSdk 29, Handy nur lesend,
Sperre im Hintergrund, `allowBackup=false`, Sync nur über den eigenen
Cloud-Ordner). **Stufen 1a, 1b und 2a sind fertig** (Commits `WIP: mobile
1a/1b/2a – … [skip ci]`, Stand 2026-10-17: bis `47e9aab` auf
`origin/claude/mobile-android`, `02005c3` und die Doku nur lokal – vor
jedem Umschreiben der Historie `git status -sb` prüfen, gepushte Commits
nicht umschreiben; der Vertrag steht in `ARCHITECTURE.md`, Abschnitt
„Mobile (Android)“, mit einer Übersichtstabelle am Anfang).

**Was die Handy-App jetzt kann** (Android 10+; ab dem nächsten
veröffentlichten Build hängt die APK an jedem Release – bisher hat noch
kein Release sie):
Tresore aus einem Ordner der eigenen Cloud übernehmen (nur lesend, nur in
Richtung Handy; der PC füllt den Ordner mit „Kopie im Cloud-Ordner aktuell
halten“), entsperren per Master-Passwort oder – opt-in – per Fingerabdruck,
suchen und filtern, aufdecken und kopieren, 2FA-Codes, Bildvorschau von
Anhängen, Sicherheitsbericht, Sperre im Hintergrund; eigenes Handy-Layout
mit Zurück-Geste. Ohne `INTERNET`-Berechtigung, ohne Android-Backup, mit
`FLAG_SECURE`. **Nicht**: bearbeiten (Stufe 2b), Autofill (Stufe 3, als
Nächstes), Update-Prüfung, Icons laden, Leak-Prüfung.

**Bitte bestätigen** (in den Läufen so entschieden bzw. umgesetzt, in
`BLOCK5-DESIGN.md` vermerkt):
* Die Handy-App bleibt **ohne `INTERNET`-Berechtigung** – auch keine
  Update-Prüfung über ein Kotlin-Plugin, wie E6 sie für Stufe 2 vorsah;
  „Über Keystead“ bietet „Releases auf GitHub öffnen“ (der Browser öffnet
  sich, dafür braucht Keystead keine Berechtigung).
* Das Handy verknüpft einen **Ordner** statt einer einzelnen Datei (damit
  die Anhänge mitkommen); der geplante Einzeldatei-Import
  `import_vault_file` entfällt.
* PC-Spiegel: nur Tresore, die in der App geöffnet werden, kommen in den
  Ordner; Löschen eines Tresors entfernt seine Kopie dort (das Handy behält
  seine); Ausschalten oder ein Ordnerwechsel lässt die bisherigen Kopien
  liegen; eine höhere Revision im Ordner wird nie überschrieben.

Die Stufen im Einzelnen (Stufe 1a in Punkt 1–6, Stufe 1b in 7 und 10,
Stufe 2a in 8 und 9):

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
   der nächsten Stufe (seit Punkt 8 steht dort „Tresor aus Cloud-Ordner
   holen“). **Android-Shell**: `MainActivity` polstert die
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

9. **Cloud-Ordner-Sync, PC-Seite** (Stufe 2a, E7, `WIP: mobile 2a - desktop
   mirror`; Vertrag: ARCHITECTURE „Cloud-folder mirror (desktop) and phone
   link“): neue Einstellung `mirrorFolder` – **„Kopie im Cloud-Ordner aktuell
   halten“** unter Einstellungen → **Handy** (neuer Abschnitt, nur Desktop):
   Schalter → Ordnerdialog (`open({ directory: true })` mit der vorhandenen
   Berechtigung `dialog:allow-open`; abgebrochen → bleibt aus), Ordner mit
   „Ordner ändern“, Status (zuletzt aktualisiert bzw. der Fehler), drei
   Schritte zum Handy (Ordner im Nextcloud-/OneDrive-Ordner, Cloud-App am
   Handy, Ordner in der Handy-App verbinden – mit „Releases auf GitHub
   öffnen“) und was der Cloud-Anbieter sieht.
   * **Engine** `src-tauri/src/mirror.rs` (`#[cfg(desktop)]`, eigener Thread
     `keystead-mirror`): ein Auftrag nach jedem erfolgreichen Speichern
     (`Core::mutate_with` – UI, Erweiterung, Icon-Abruf, Import, Umbenennen,
     Master-Passwort, Wiederherstellungsschlüssel), beim Öffnen eines Tresors
     (`install_vault_since`, holt auch nach, was die TUI oder ein früherer
     Start geändert hat), wenn der Monitor die Änderung eines anderen
     Prozesses übernimmt (auch bei `KeyChanged`) und beim Setzen des Ordners;
     `delete_vault` entfernt die Kopie (nur `<id>.keystead` mit dieser ID im
     Kopf und die `<id>.bin` des Anhang-Ordners). Ein Durchgang:
     Tresordatei lesen und prüfen → Blobs des Datenordners listen →
     fehlende bzw. andere Blobs kopieren → Tresordatei per `write_atomic`
     (die Bytes wie auf der Platte, nichts wird entschlüsselt) → **erst
     danach** Blobs entfernen, die der Datenordner nicht mehr hat. Gleiche
     Revision → nur die Blobs (kein unnötiger Upload); höhere Revision im
     Ordner → `unsupported:mirror_newer`, nichts wird angefasst. Nie `.bak`,
     `.lock` oder Exporte.
   * **Prüfungen** (`check_folder`): ein Ordner im Datenordner oder ein
     Keystead-`vaults`-Ordner (heißt `vaults` und hat `settings.json` daneben,
     oder enthält `*.keystead.bak`/`.lock`, z. B. der eines portablen Sticks)
     → `invalid_input:mirror_folder_data_dir`; fehlt er →
     `unsupported:mirror_folder_missing` (wird nie angelegt). `save_settings`
     prüft nur einen **geänderten** Ordner (ein gerade nicht erreichbares
     Laufwerk blockiert sonst das Speichern des Designs), jeder Durchgang
     prüft erneut.
   * **Fehler blockieren das Speichern nie**: je Code einmal geloggt, Event
     `mirror://status` (dazu das Kommando `mirror_status`), ein Toast je
     **neuem** Fehler („Kopie im Cloud-Ordner nicht aktualisiert: …“, nicht
     nach einem Neuladen der Seite wiederholt), der nächste Speichervorgang
     versucht es erneut. Beim Beenden wartet die App höchstens 3 s auf offene
     Kopien.
   * **Texte** (Doku-Pflicht aus BLOCK5-DESIGN): „Deine Daten verlassen nie
     dieses Gerät“ ist in Willkommen („Lokal & offline“) und „Über Keystead“
     ersetzt (de/en: „Deine Daten bleiben verschlüsselt bei dir – kein
     Keystead-Server, kein Konto …“); README de/en: Einleitung, Funktion
     „Aufs Handy“, Schritte PC → Cloud-App → Handy, was der Anbieter sieht
     (verschlüsselte Daten, Dateinamen = Tresor-IDs, Klartext-Kopf mit
     Tresorname/Revision/Zeiten), Datenorte, Sicherheit; Release-Notes-Zeile
     in `build.yml`. ARCHITECTURE: neuer Abschnitt, Settings, Kommandotabelle
     (`mirror_status`), Events, Datenorte, Mock, Intro.
   * **Mock**: `?mirror=on` (Ordner schon gesetzt), der Ordnerdialog liefert
     `C:\Users\Demo\Nextcloud\Keystead`, jede Änderung „kopiert“ nach
     ~0,4 s, `__keysteadMock.simulateMirrorFolderLost(lost?)`.
   * **Tests**: 11 neue Rust-Tests in `mirror.rs` (Tresordatei und Anhänge
     byte-genau, keine Hilfsdateien im Ordner, veraltete Blobs entfernt,
     fremde Dateien bleiben, ein nicht schreibbarer Blob hält die alte
     Tresordatei, fehlender Ordner einmal gemeldet und danach wieder heil,
     Datenordner/`vaults` verweigert, neuere Kopie nie ersetzt, Löschen
     entfernt nur die eigene Kopie, Ordnerwechsel, Worker-Thread samt Warten
     beim Beenden, und ein **Vertragstest**: das `folder_sync` des Handys
     übernimmt, was der PC geschrieben hat – Import samt Anhang, dann die
     neue Revision); Core-Test für `mirrorFolder` (nachsichtig, leer = aus).
   **Im Browser geprüft** (Mock, `npm run dev`): Abschnitt „Handy“ mit
   Schalter, Ordner, Status, Schritten und Datenschutz-Hinweis; Fehler →
   genau ein Toast, Status zeigt den Fehler, danach wieder „Zuletzt
   aktualisiert“; aus/an mit Ordnerdialog; mit `?platform=android` kein
   Abschnitt „Handy“ und kein Aufruf von `mirror_status`.
   **Entscheidung dieses Laufs** (für den Besitzer): die Handy-App bleibt
   **ohne `INTERNET`-Berechtigung** – keine Update-Prüfung auf Android;
   „Über Keystead“ bietet „Releases auf GitHub öffnen“ (der Browser öffnet
   sich, keine Berechtigung nötig); das Hauptmanifest bleibt ohne jede
   Berechtigung.
   **So umgesetzt, bitte bestätigen:** Nur Tresore, die in der App geöffnet
   werden, kommen in den Ordner (nicht von selbst alle im Datenordner).
   Löschen eines Tresors entfernt auch seine Kopie im Ordner (das Handy
   behält seine, Status „nicht mehr im Ordner“). Ausschalten oder ein
   Ordnerwechsel lässt die bisherigen Kopien liegen.
   **Nicht geprüft:** mit einer echten Nextcloud-/OneDrive-App am PC (Tests
   und Mock nutzen lokale Ordner) und Ende-zu-Ende echte Desktop-App →
   Emulator (der Vertragstest deckt Format und Reihenfolge ab, die
   Handy-Seite wurde in Punkt 8 im Emulator geprüft). Eine Änderung, die die
   TUI an einem Tresor macht, den die App gerade nicht offen hat, kommt erst
   beim nächsten Öffnen in der App in den Ordner.

10. **Entsperren per Fingerabdruck** (Stufe 1b, E5, `WIP: mobile 1b -
    fingerprint unlock`; Vertrag: ARCHITECTURE „Mobile (Android)“ →
    „Fingerprint unlock (phone)“): Opt-in unter Einstellungen → Sicherheit →
    **„Mit Fingerabdruck entsperren“** (nur Handy, für den offenen Tresor),
    Standard aus, **kein PIN/Muster-Ersatz** (`allowDeviceCredential =
    false`).
    * **Plugin geprüft** (Quellen von `tauri-plugin-biometric` 2.5.0 in der
      Cargo-Registry gelesen): `biometric_cipher` verschlüsselt/entschlüsselt
      mit einem AES-256-GCM-Schlüssel im Android-Keystore
      (`setUserAuthenticationRequired(true)` ohne Gültigkeitsdauer → jede
      Nutzung braucht einen **starken** Fingerabdruck über `BiometricPrompt`
      mit `CryptoObject`; ein neu eingerichteter Fingerabdruck oder eine
      entfernte Displaysperre machen ihn ungültig). Damit gilt E5 (a): das
      offizielle Plugin, nur aus Rust gerufen (keine `biometric:`-Berechtigung
      für die Webview, Guard-Test), nur unter `cfg(target_os = "android")`.
      Was es **nicht** kann (für eine spätere Härtung mit eigenem
      Kotlin-Plugin, E5 b – z. B. zusammen mit Autofill): StrongBox,
      `setUnlockedDeviceRequired`, ein eigener Schlüssel-Alias; es nimmt und
      liefert Text, deshalb läuft der KEK als Base64 über JSON/JNI in
      Kotlin-Strings und als Intent-Extra in die Plugin-Activity (über
      Androids Activity-Manager) – nicht löschbar, aber nie auf der Platte,
      nie in der Webview.
    * **Gespeichert**: `<data>/device-unlock/<vault-id>.json` mit dem vom
      Keystore versiegelten `keystead-device-unlock:1:<id>:<KEK>` (die ID
      bindet den KEK an seinen Tresor) und dem KDF-Salz des Tresorkopfs
      (neu: `UnlockedVault::kdf_salt`, Core-Test). Entsperren: Salz prüfen →
      Prompt → KEK → `unlock_with_master_kek` → `install_vault_since`.
    * **Zurückgesetzt** (Blob gelöscht, Fehler
      `unsupported:device_unlock_invalid`, Hinweis auf dem Entsperrbildschirm,
      nach dem nächsten Passwort-Entsperren das Angebot „Fingerabdruck wieder
      einrichten?“): anderes Salz in der Tresordatei (Master-Passwort am PC
      geändert – geprüft nach jeder Sync-Kopie, bei `KeyChanged` im Monitor,
      im Status und vor jedem Prompt), Keystore-Schlüssel ungültig (neuer
      Fingerabdruck), beschädigter/fremder Blob, KEK öffnet den Tresor nicht.
      Abbrechen, nicht erkannter Finger und Sperre des Sensors behalten ihn.
      Ein gelöschter Tresor verliert seinen Blob. Der Wiederherstellungsweg
      existiert auf dem Handy nicht (am PC setzt er ein neues Passwort →
      neues Salz → zurückgesetzt).
    * **Hintergrund-Sperre**: der Prompt ist eine eigene Activity und
      pausiert `MainActivity`; solange der eigene Prompt offen ist (max.
      120 s) und 2 s danach startet das kein Hintergrund-Timer
      (`monitor::prompt_covers_pause`, getestet). Verlässt man die App aus
      dem Prompt heraus, läuft der Timer ab dessen Schließen.
    * **UI**: Entsperrbildschirm mit „Mit Fingerabdruck entsperren“ und
      automatischem Prompt (einmal je Entsperrbildschirm, sobald die App
      sichtbar ist – beim Start und bei der Rückkehr nach der
      Hintergrund-Sperre; **nicht** nach „Sperren“ von Hand); Schließen des
      Prompts ist kein Fehler. Texte de/en (Du-Form), Prompt-Texte aus der UI.
    * **Mock**: `?platform=android&fingerprint=on` (bzw. `none` /
      `notEnrolled`), `__keysteadMock.setFingerprintResult("cancel" | "fail" |
      "lockout")`, `simulateNewFingerprint()`, `simulatePcPasswordChange()`
      setzt die Einrichtung des Tresors zurück.
    * **Tests**: 10 Rust-Tests in `device_unlock.rs` (Blob-Dateien inkl.
      manipulierter/fremder/beschädigter Blobs, Bindung an die Tresor-ID,
      Einrichten + Entsperren, abgebrochener Prompt behält alles, neuer
      Fingerabdruck/manipulierte Daten/falscher KEK setzen zurück, am PC
      geändertes Passwort ohne Prompt, Ausschalten, gelöschter Tresor,
      Sperre während des Prompts, Einordnung der Plugin-Antworten), dazu
      `folder_sync` (Sync mit neuem Passwort löscht den Blob), `monitor`
      (Prompt-Fenster), `commands` (`save_settings` ändert `deviceUnlock`
      nicht), Core-Test für `kdf_salt`, Guard-Tests (Kommandoliste,
      keine `biometric:`-Berechtigung).
    **Im Emulator geprüft** (MB_S26, API 36, Debug-APK x86_64, Test-Tresor
    „Handy-Test“, Passwort `demo`): Displaysperre per `locksettings set-pin
    1234`, Fingerabdruck über `am start -a android.settings.FINGERPRINT_ENROLL`
    + PIN + `adb -e emu finger touch 1` (automatisierbar, ~6 Berührungen);
    Einstellungen → Sicherheit → Schalter → System-Prompt „Fingerabdruck
    einrichten“ → Finger → Toast, Blob liegt in `device-unlock/` (Salz und
    versiegelter Text, kein KEK im Klartext), `deviceUnlock: true`; `force-stop` + Start →
    Prompt öffnet sich von selbst → Finger → Tresor offen; „Sperren“ von
    Hand → kein Prompt, Knopf da → Knopf → Prompt → Abbrechen ohne
    Fehlermeldung; Ausschalten löscht Blob und Ordner; mit „Sperren im
    Hintergrund: Sofort“ bleibt der Tresor beim Einrichten (Prompt 5 s
    offen) offen, Home → gesperrt, zurück → Prompt → Finger → offen; zweiten
    Fingerabdruck eingerichtet → zurück in Keystead → Hinweis „…
    zurückgesetzt“, Blob weg → Passwort → „Fingerabdruck wieder
    einrichten?“ → Einrichten → Finger 2 → wieder aktiv. Screenshots
    `phone-1b-bio-*.png` im Scratchpad. Danach „Sperren im Hintergrund“
    wieder 1 min, Fingerabdruck an (PIN des Emulators: 1234).
    **Gefunden und behoben**: nach „Sperren“ von Hand öffnete sich der
    Prompt doch (die Notiz wurde vom Entsperrbildschirm vor dem Neuladen der
    Seite verbraucht) – jetzt trägt sie die Seitenladung, die sie schrieb.
    **Nicht geprüft**: ein echtes Gerät (Sensor, StrongBox-lose Geräte mit
    nur schwacher Gesichtserkennung), Android 10, der R8-Release-Build mit
    dem Plugin (seine Activity steht im Manifest, R8 behält sie), ein am PC
    geändertes Passwort Ende-zu-Ende (Unit-Tests decken es ab), TalkBack.
    **Auffällig**: der System-Prompt zeigt das Launcher-Icon der App – das
    ist noch Tauris Standard-Icon (`gen/android/app/src/main/res/mipmap-*`),
    nicht das Keystead-Logo. Während des Tests kamen zusätzliche Eingaben
    auf den Emulator, die nicht von mir stammten (Ordnerdialog geöffnet,
    Zurück-Wischen, Theme auf Dunkel) – ich habe jeden Schritt per Screenshot
    gegengeprüft.

11. **Review-Fixes 1b/2a** (`WIP: mobile 1b/2a - review fixes`; nach dem
    PC-Reset aus den Transkripten rekonstruiert und am 2026-10-22 fertig
    gemacht). Die 12 Funde des Reviews: Kopien aus dem Cloud-Ordner sind
    „auf Probe“, bis sie am Handy aufgehen (`folder_sync::Trial`,
    `UnlockedVault::check_copy`, `unlock_copy`); Größenlimit 64 MiB auch im
    PC-Spiegel, Byte-Vergleich bei gleicher Revision, Wiederholung
    fehlgeschlagener Löschungen; Klartext-KEK im Biometric-Plugin wird
    überschrieben; Start-Sync-Race im Willkommensbildschirm; Mock mit
    Auto-Sync; README/Release-Notes: OneDrive/Nextcloud am Handy ungeprüft;
    `cfg(target_os = "android")` statt `cfg(mobile)`; CLAUDE.md und
    ARCHITECTURE (Crate-API). Dazu aus einem zweiten Review: eine Kopie auf
    Probe setzt die Fingerabdruck-Einrichtung nicht mehr zurück
    (`device_unlock_needs_password`, Rücksetzen erst nach dem Öffnen mit
    dem Passwort), eine gefälschte Schlüsselhülle mit gleichem Salz wird
    über die behaltene Kopie erkannt (`kept_copy_opens`), und die Sperre
    einer abgelehnten Kopie wird vor dem Speichern noch einmal geprüft.
    **Geprüft** (Windows): fmt, clippy `-D warnings`, `cargo test
    --workspace`, `node --test` (70/70), `npm run build`, `cargo clippy -p
    keystead-core -p keystead-desktop --target aarch64-linux-android -D
    warnings`, Debug-APK x86_64.
    **Im Emulator geprüft** (2026-10-22, neu angelegtes AVD `MB_S26`:
    Pixel-9-Profil, API 36 google_apis x86_64, Sprache Englisch; Test-Tresor
    „Cloud-Test“, Passwort `demo2`, mit einem Hilfsprogramm über
    `keystead-core` erzeugt, Fälschungen per Header-Änderung): Willkommen →
    Ordner verbinden → Import → Entsperrbildschirm erscheint von selbst;
    Passwort → Kopie bestätigt (`sync/trial/` leer). Gefälschte Revision
    (50) bei gesperrtem Tresor → auf Probe, behaltene Rev. 3 → Passwort →
    „the previous copy is back“, Rev. 50 `rejected`. Gefälschte
    Schlüsselhülle bei gleichem Salz (60): falsches Passwort bleibt falsch
    (nichts ändert sich), richtiges öffnet die behaltene Kopie, Rev. 60
    `rejected`, `sync/probe` wieder weg. Echte Rev. 4 bei offenem Tresor →
    sofort in der Liste, kein Probe-Eintrag. Fingerabdruck eingerichtet
    (Prompt, Blob, kein Log „may not be cleared“ → Scrub des Plugins wirkt),
    Neustart → Prompt von selbst → Finger → offen. Gefälschte Schlüsselhülle
    (70) + Finger → behaltene Kopie offen, Einrichtung bleibt. Gefälschtes
    Salz (80) → Hinweis `device_unlock_needs_password`, Einrichtung bleibt;
    das Passwort öffnet diese Kopie nicht („am PC geändert“ – bekannte
    Grenze); die nächste echte Speicherung des PCs (Rev. 5) → „Jetzt
    synchronisieren“ → Finger → offen, bestätigt, Einrichtung weiter da.
    Keine Panik im Log. **Nicht geprüft**: Release-APK (R8), echtes Gerät,
    echte Nextcloud-/OneDrive-App, PC-Spiegel gegen einen echten
    Cloud-Ordner. **Auffällig**: Einstellungen → Synchronisierung zeigt
    nach einer beim Entsperren abgelehnten Kopie bis zum nächsten Sync
    weiter „Updated“ (das Ergebnis des Syncs, nicht des Entsperrens).

### Emulator-Checkliste für den Besitzer

Ohne Handy prüfst du die App im Emulator `MB_S26` (API 36, x86_64). Befehle
in Git Bash; vorher `ANDROID_HOME="$LOCALAPPDATA/Android/Sdk"` setzen,
`adb` liegt in `$ANDROID_HOME/platform-tools`. Befehle mit Pfaden auf dem
Gerät (`/data/…`, `/sdcard/…`) brauchen in Git Bash `MSYS_NO_PATHCONV=1`
davor.

Stand des Emulators nach den Läufen vom 2026-10-17 (laut deren Berichten):
in der App liegen die Test-Tresore „Handy-Test“ und „Handy-Demo“ (Passwort
`demo`) und „Cloud-Test“ (Passwort `demo2`, aus dem Ordner
`/sdcard/Download/KeysteadSync`); Displaysperre-PIN `1234`, Fingerabdrücke
1 und 2 eingerichtet, Entsperren per Fingerabdruck für „Handy-Test“ an,
„Sperren im Hintergrund“ 1 min, Sprache Deutsch, Design dunkel. Ob der
Cloud-Ordner gerade verbunden ist, zeigt Einstellungen → Synchronisierung.

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
   **Test-Tresor**, nicht deinen echten): am PC unter Einstellungen → Handy
   „Kopie im Cloud-Ordner aktuell halten“ einschalten und einen Ordner wählen
   – Keystead legt `<id>.keystead` und ggf. den Ordner
   `<id>.keystead.attachments` hinein. Der Emulator hat keine Cloud-App, also
   diese Dateien von Hand in einen Ordner auf dem Emulator legen, z. B.
   `adb shell mkdir -p /sdcard/Download/KeysteadSync` und `adb push
   <id>.keystead /sdcard/Download/KeysteadSync/` (Git Bash:
   `MSYS_NO_PATHCONV=1` davor). In der App „Cloud-Ordner wählen“ (bzw.
   „Verbinden“ auf dem Entsperrbildschirm) → Download → KeysteadSync →
   „Diesen Ordner verwenden“ → „Zulassen“. Eine neuere Kopie pushen und die
   App kurz verlassen (oder Einstellungen → Synchronisierung → „Jetzt
   synchronisieren“) – die App übernimmt sie. Geht mit Debug- und
   Release-APK; den Ordner `Download` selbst lässt Android nicht freigeben,
   nur einen Unterordner.
6. Durchklicken: Start → Willkommen/Entsperren (die Tastatur springt nicht
   von selbst auf) → Liste und Suche → Menü-Knopf öffnet die Seitenleiste
   als Schublade (Ordner, Tags, Typen; Chip „Nur lesen“) → Eintrag antippen
   (Detail über der Liste, Zurück-Pfeil) → Passwort aufdecken (nach 30 s
   wieder verborgen) → kopieren (die Zwischenablage des PCs spiegelt die des
   Emulators; nach 30 s leer) → 2FA-Code → Website eines Eintrags antippen
   (der Browser öffnet sich, Keystead friert nicht ein) → Bildanhang ansehen
   (nur wenn der Ordner `<id>.keystead.attachments` mitkopiert wurde) →
   Sicherheitsbericht → Einstellungen (nur Allgemein, Sicherheit,
   Synchronisierung, Über; nirgends ein Bearbeiten-Knopf) → „Sperren im
   Hintergrund“ (Sofort / 1 min / 5 min / Nie) → „Über Keystead“ zeigt
   „Android · Daten im App-Speicher“, „Releases auf GitHub öffnen“ öffnet
   den Browser. Zurück-Taste (`adb shell input keyevent 4`): schließt
   Dialog/Schublade, dann das Detail, geht von den Einstellungen zur Liste,
   an der Wurzel in den Hintergrund.
   Fingerabdruck: Displaysperre (`adb shell locksettings set-pin 1234`) und
   Fingerabdruck einrichten (Android-Einstellungen → Sicherheit, oder `adb
   shell am start -a android.settings.FINGERPRINT_ENROLL`; der Emulator
   „berührt“ den Sensor mit `adb -e emu finger touch 1`, mehrmals) → in
   Keystead Einstellungen → Sicherheit → „Mit Fingerabdruck entsperren“ →
   `adb -e emu finger touch 1` → App beenden und starten: der Prompt kommt
   von selbst, `finger touch 1` entsperrt. Ein zweiter Fingerabdruck setzt
   das Entsperren zurück (Hinweis, danach Passwort und Angebot zum neuen
   Einrichten).
7. Home-Taste: nach 1 Minute ist der Tresor gesperrt (Toast beim
   Zurückkehren); die Zurück-Geste schickt die App in den Hintergrund, statt
   sie zu schließen; die Vorschau in „Zuletzt verwendet“ und ein Screenshot
   (`adb exec-out screencap -p > shot.png`) sind schwarz (`FLAG_SECURE`).
   Ein Bild für Fehlerberichte: `adb emu screenrecord screenshot <Ordner>`.
8. Logs: `adb logcat -s RustStdoutStderr:* Tauri:*` – erwartet sind nur
   Chromium-Zeilen des WebView, keine Panik.

Erwartete Grenzen in dieser Stufe: bearbeiten geht nur am PC (die
Handy-Oberfläche zeigt keine Bearbeiten-Knöpfe); im Emulator ersetzt `adb
push` die Cloud-App (auf einem echten Handy synchronisiert die Nextcloud-
oder OneDrive-App den Ordner, den der PC füllt).

### Nächste Schritte (laut Design, Reihenfolge des Besitzers)

Stufe 1a ist mit Punkt 6 fertig, 1b mit den Punkten 7 und 10, 2a mit den
Punkten 8 (Handy) und 9 (PC) – ein eigener Einzeldatei-Import
`import_vault_file` ist damit überflüssig. Vorher: die Punkte unter „Bitte
bestätigen“ oben mit dem Besitzer klären, und einmal Ende-zu-Ende mit der
echten Desktop-App und dem Emulator prüfen (Checkliste oben, Schritt 5).

1. **Autofill** (Stufe 3, E11 – der Besitzer will es vor dem Bearbeiten):
   ein Android-`AutofillService` in Kotlin (gleicher Prozess wie die
   Rust-Bibliothek, JNI), zuerst die Minimalvariante – nur Browser/Domain,
   Benutzername + Passwort, kein Speichern; ohne offene App entsperrt eine
   eigene Activity per Fingerabdruck (Keystore-KEK aus Stufe 1b) – siehe
   `BLOCK5-DESIGN.md`, „Stufe 3“. Dabei sinnvoll: der Fingerabdruck als
   eigenes Kotlin-Plugin (E5 b: StrongBox, eigener Alias, `ByteArray` statt
   Text), weil Autofill denselben Keystore-KEK braucht.
2. **2b** Bearbeiten am Handy + Zwei-Wege-Sync mit Konfliktdialog
   (Schreibkommandos registrieren, Editor im Handy-Layout, Copy-out in den
   Ordner – dafür braucht das Handy dann eine Schreibfreigabe –, Konflikt-
   erkennung `sync::decide` mit Digest und Basis-Revision; der PC-Spiegel
   ersetzt bisher nie eine höhere Revision im Ordner – 2b braucht dort
   dieselbe Konfliktlogik).
3. Kleinere offene Punkte: das Launcher-Icon von `gen/android` (noch
   Tauris Standard, auch im Fingerabdruck-Prompt sichtbar) durch das
   Keystead-Logo ersetzen; Zwischenablage mit langem Timer härten
   (Kotlin-`AlarmManager`, siehe „Bekannte Grenzen“ unten); TalkBack und
   der R8-Release-Build mit Ordner- und Biometrie-Plugin im Emulator.
4. Update-Hinweis auf dem Handy: nach der Entscheidung der Läufe **nicht**
   (keine `INTERNET`-Berechtigung, siehe „Bitte bestätigen“); nur wieder
   aufnehmen, wenn der Besitzer das will.
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
