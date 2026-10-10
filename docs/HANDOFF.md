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

Veröffentlicht: beta.2 … **beta.8** (beta.7: Sicherheits-Block, beta.8: Plugin smarter). Der Besitzer
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
Windows-Fehler in `crates/keystead-bridge` behoben wurden (noch nicht
veröffentlicht, gehen mit dem nächsten `[release]` raus):

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

Außerdem liegt in `docs/BLOCK3-DESIGN.md` der zusammengeführte Entwurf für
Block 3 (Anhänge, HIBP, Tags, Assistent) mit den offenen Entscheidungen D1–D8;
nach der Entscheidung des Besitzers wandern die festgelegten Teile nach
`ARCHITECTURE.md`.

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
- Windows-spezifischer Code wurde in der Cloud nur per Cross-Clippy geprüft.
  Seit 2026-10-10 läuft die komplette Prüfkette (fmt, clippy, alle Tests, npm,
  node) auch lokal auf dem Windows-PC des Besitzers – dabei kamen zwei reine
  Windows-Fehler der Bridge zum Vorschein, beide behoben (siehe „Lokal
  verifiziert“). CI testet weiterhin nur unter Linux; Windows-Tests laufen nur
  lokal.
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
