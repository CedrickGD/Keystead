# Übergabe – Stand Keystead (2026-10-10)

Kurzfassung für eine neue (lokale) Claude-Sitzung. Projektregeln stehen in
[`CLAUDE.md`](../CLAUDE.md), die technischen Verträge in
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## Repository & Branches

| | |
|---|---|
| Repo | `CedrickGD/Keystead` (öffentlich; früher `Vault-X`, alte URLs leiten weiter) |
| Entwicklungs-Branch | `claude/quirky-archimedes-i4qa3j` – **hier liegt die gesamte Keystead-2-Arbeit** |
| `main` | noch VaultX 1.x (PowerShell). `version.yml` dort steuert das Auto-Update der alten 1.x-Installationen. Noch nicht gemergt. |
| Schlüssel | privates Repo `CedrickGD/keys`, Ordner `keystead/` (Update-Signaturschlüssel, Erweiterungs-Schlüssel, README) |
| GitHub-Secret | `TAURI_SIGNING_PRIVATE_KEY` im Keystead-Repo ist gesetzt (Inhalt von `keys/keystead/tauri-updater.key`) |

Lokal starten:
```sh
git clone https://github.com/CedrickGD/Keystead
cd Keystead
git checkout claude/quirky-archimedes-i4qa3j
cd apps/desktop && npm ci && npx tauri dev
```
Voraussetzungen unter Windows: Rust (rustup, Version kommt aus
`rust-toolchain.toml`), Node 22, Visual Studio Build Tools (MSVC + Windows SDK),
WebView2 (ist auf Win 10/11 vorhanden).

## Releases & Updates

- Commit auf dem Branch mit **`[release]`** in der Commit-Nachricht → GitHub Actions
  (`.github/workflows/build.yml`) baut auf `windows-latest`, versioniert als
  `2.0.0-beta.<Run-Nummer>`, signiert das Setup und veröffentlicht eine
  **Pre-Release** (portable ZIP, Setup, Erweiterungs-ZIP, `.sig`, `latest.json`).
  Außerdem wird `latest.json` im festen Pre-Release `updater-beta` ersetzt –
  daraus holen installierte Apps ihre Updates (Beta-Kanal).
- **`[skip ci]`** für Zwischenstände (kein Build, keine Discord-Meldung).
- Stabile Version: Tag `vX.Y.Z` → Release als „Latest“. Danach `version` in
  `apps/desktop/src-tauri/tauri.conf.json` hochzählen (sonst sortieren spätere
  Betas darunter; CI warnt).
- Betas sind Pre-Releases → das alte VaultX-1.x-Auto-Update (folgt „Latest“)
  springt nicht darauf.
- Prüfen eines Releases: `latest.json` von
  `https://github.com/CedrickGD/Keystead/releases/download/updater-beta/latest.json`
  laden; Signatur gegen `keys/keystead/tauri-updater.key.pub` prüfen (minisign,
  Ed25519 über BLAKE2b-512 der Datei).
- Die App legt die Browser-Erweiterung selbst nach
  `%LOCALAPPDATA%\Keystead\browser-extension` (Version `2.0.0.<Run>`) und die
  Erweiterung lädt sich nach App-Updates selbst neu. Der Besitzer hat sie seit
  beta.7 aus diesem Ordner geladen.

Veröffentlicht: beta.2 … **beta.7** (letzte: Sicherheits-Block). Der Besitzer
testet unter Windows (Installer-Version, Brave/Chromium-Browser) und bekommt
neue Betas per In-App-Update.

## Was fertig ist

- Desktop-App: Tresore (Logins, Karten, Identitäten, Notizen, Ordner, Favoriten,
  Papierkorb, Zusatzfelder, Passwortverlauf), Generator, Sicherheitsbericht,
  Auto-Sperre, Tray, Wiederherstellungsschlüssel, mehrere Tresore,
  portabler Modus, Deutsch/Englisch, hell/dunkel.
- Import per Drag & Drop mit Formaterkennung (Chrome/Edge/Firefox/Bitwarden CSV,
  Bitwarden JSON, Keystead-Export, VaultX 1.x), Vorschau, Duplikat-/Konflikt-
  Behandlung (überspringen/aktualisieren/beide behalten); vereinfachter Export.
- Website-Icons (direkt von den Websites, verschlüsselt im Tresor, abschaltbar
  unter Einstellungen → Allgemein; bei Proxy „fail closed“).
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

## Laufende Arbeit: Block 2 „Plugin smarter machen“

Gebaut (auf dem Branch, siehe `git log`): Passwortvorschlag auf Registrierungs-
und Passwort-ändern-Seiten, 2FA-Code nach dem Ausfüllen kopieren +
„2FA-Code einfügen“ am Code-Feld, Erkennung in offenen Shadow Roots
(Web-Components), sichtbare Rückmeldung bei zu frühen/verdeckten Klicks,
2FA-Countdown + Tastatursteuerung im Popup, Plugin-Einstellungen; Bridge:
`generate_password.remember`, `remember_generated`.

Funde der zweiten Prüfrunde – behoben (Unit-Tests + echter Browser):
1. Div-basierte Registrierung (ohne `<form>`, Benutzername in eigenem
   Abschnitt nach dem Passwort, GitHub-artig): Ein Feld, dessen Container
   (Formular, sonst nächstes Element mit Button) ein Passwortfeld eines
   erkannten Formulars enthält, ist kein Login-Schritt mehr.
2. „Konto erstellen“-Seite mit zuerst nur E-Mail + „Weiter“: Liest sich der
   Schritt nach Registrierung, ist es kein Login-Schritt. Es entscheiden die
   nächsten Wörter (Anmelden/Anmeldung gewinnt bei beidem): erst die eigenen
   des Schritts (Action, ID, Name, Submit-Buttons – ein „Konto erstellen“
   neben einem reinen „Weiter“ zählt nicht), dann seine Überschriften, dann
   die Überschrift über dem Formular, zuletzt die Seite (Titel, Pfad). So
   bleibt der Registrierungsschritt eines Shops neben dessen Login auch
   unter dem Titel „Kasse – Anmelden“ eine Registrierung, und „Anmelden
   oder Konto erstellen“ sowie Google („Konto erstellen“-Button neben
   „Weiter“, Titel „Anmeldung – Google Konten“) bleiben Login-Schritte.
3. Entscheidung umgesetzt: Popup-„Ausfüllen“ (explizite Auswahl) füllt auch
   ein als Registrierung erkanntes Formular, wenn die Seite kein anderes hat
   (Benutzername + Passwort ins Neues-Passwort-Feld und die Bestätigung).
   Inline-Icon/-Menü und `Strg+Umschalt+L` bleiben dort aus;
   `Strg+Umschalt+L` weist per Hinweis auf das Popup hin.

**Wenn im `git log` ein Commit „… [release]“ nach „Smarter browser extension“
(beta.8) steht, ist Block 2 abgeschlossen.** Sonst: die Änderungen
(`extension/chrome/lib/forms.js`, `content.js`, `background.js`, Locales,
Tests, Doku) committen und mit `[release]` veröffentlichen.

## Nächste Schritte (vom Besitzer so priorisiert)

3. **Block 3 „Neue Funktionen“**: verschlüsselte Dateianhänge an Einträgen
   (z. B. Ausweis-Scan), optionale Prüfung auf geleakte Passwörter
   (Have I Been Pwned, k-Anonymität – nur die ersten 5 Zeichen des SHA-1 gehen
   raus; standardmäßig aus oder nur auf Knopfdruck), Tags zusätzlich zu Ordnern.
4. **Block 4 „Stabile 2.0“**: Branch sauber nach `main` bringen (PR, ggf. Squash
   der vielen WIP-Commits), `v2.0.0` als Latest veröffentlichen, VaultX-1.x-
   Nutzern den Umstieg anbieten. Achtung: Das 1.x-Updater liest `version.yml` auf
   `main` und ersetzt seine EXE/PS1 direkt – am sichersten eine letzte
   VaultX 1.1.x, die nur einen Hinweis „Keystead ist da“ mit Link zeigt.
   **Vor Merge/Tag den Besitzer fragen.**

## Bekannte Grenzen / offene Punkte

- EXE ist nicht code-signiert → SmartScreen-Warnung. Lösung später z. B. über
  Azure Trusted Signing (kostenpflichtig, Konto des Besitzers nötig).
- Windows-spezifischer Code wurde in der Cloud nur per Cross-Clippy geprüft; real
  getestet hat nur der Besitzer (Installer, Updates, Plugin-Kopplung laufen).
- Website-Icons verraten Netzwerk-Mitlesern die Liste der Websites (dokumentiert,
  Schalter vorhanden); hinter Firmen-Proxys werden bewusst keine Icons geladen.
- Verschlüsselte Bitwarden-Exporte, 1Password-/KeePass-Formate: nicht unterstützt.
- TUI: Ordner, Zusatzfelder, mehrere Websites, Papierkorb nicht bearbeitbar.
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
