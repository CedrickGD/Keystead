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
| Generator | `claude/generator` (abgezweigt vom Stand 2.0.0, lokal im eigenen Worktree `Keystead-generator`): Überarbeitung des Generators, siehe „Was fertig ist“. Stand 2026-10-17: WIP-Commits mit `[skip ci]`, nicht gepusht, nicht gemergt, nicht veröffentlicht. |
| Schlüssel | privates Repo `CedrickGD/keys`, Ordner `keystead/` (Update-Signaturschlüssel, Erweiterungs-Schlüssel, README) |
| GitHub-Secret | `TAURI_SIGNING_PRIVATE_KEY` im Keystead-Repo ist gesetzt (Inhalt von `keys/keystead/tauri-updater.key`) |

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
- **Generator-Überarbeitung** (Branch `claude/generator`, noch nicht gemergt
  und nicht veröffentlicht). Anlass: Beschwerde des Besitzers vom 2026-10-17 –
  der Generator erzeugte bei jedem Öffnen, jeder Optionsänderung und jedem
  Klick ein Passwort und speicherte es (bis zu 50 unbenutzte im Verlauf), der
  Verlauf stand im Klartext in der Oberfläche (verstieß gegen die
  Block-1-Regel), und es fehlte Anpassung. Die Entscheidungen des Besitzers
  dazu sind umgesetzt – nicht erneut aufmachen:
  - **Nichts automatisch**: Die Generator-Seite zeigt kein Passwort, bis
    „Generieren“ geklickt wird (auch Enter oder Strg+G, solange der Fokus auf
    der Seite liegt und kein Dialog offen ist). Eine Optionsänderung erzeugt
    nichts; eine ältere Vorschau bekommt den Hinweis „Die Optionen haben sich
    geändert …“. Das angezeigte Passwort ist eine Vorschau
    (`generate_password` mit `remember: false`) und wird nicht gespeichert.
    Der Editor-Popover (Link „Generieren“ am Passwortfeld) arbeitet genauso.
  - **Verlauf nur für benutzte Passwörter** (höchstens 50, im Tresor): In den
    Verlauf kommt ein Passwort nur, wenn es kopiert (Seite, Popover) oder per
    „Verwenden“ in einen Eintrag übernommen wurde (`remember_generated`); in
    der Erweiterung nach Kopieren, Ausfüllen (Popup, Kontextmenü,
    Strg+Umschalt+9 – nur wenn ein Feld es genommen hat), „Verwenden“ eines
    Vorschlags und „Login hinzufügen“ mit dem Passwort des Zauberstabs.
    Dasselbe Passwort wie der neueste Eintrag wird nicht noch einmal
    gespeichert (zweimal kopiert = eine Benutzung).
  - **Verlauf nie im Klartext in der Oberfläche**: `generator_history`
    liefert nur `{ id, createdAt }`, die Zeilen sind maskiert. Das Auge holt
    ein Passwort per `reveal_generated` (wieder weg nach 30 s, bei
    Fokusverlust, Sperre und beim Verlassen der Seite), Kopieren läuft über
    `copy_generated` (sensibler Zwischenablage-Pfad, der Wert erreicht die
    Seite nicht). Einträge bestehender Tresore bekommen beim Öffnen eine ID,
    gespeichert mit dem nächsten Speichern.
  - **Anpassung**: Länge 4–128 (`PASSWORD_MIN_LEN` jetzt 4), eigene
    Sonderzeichen (Standard `!#$%&*+-=?@^_`, „Standard wiederherstellen“),
    „Diese Zeichen nie verwenden“ (für alle Gruppen), Mindestanzahl für
    Groß-, Kleinbuchstaben, Ziffern und Sonderzeichen (je 0 … Länge, Summe
    ≤ Länge, zählt nur bei eingeschalteter Gruppe), „Mit einem Buchstaben
    beginnen“, „Verwechselbare Zeichen vermeiden“; Passphrase: 3–20 Wörter,
    Trennzeichen bis 8 Zeichen, Wortliste Englisch/Deutsch, Großschreibung,
    Zahl. Probleme stehen am Feld (dieselben Codes in derselben Reihenfolge
    wie im Core, `src/lib/generatorRules.ts`) und sperren „Generieren“;
    darunter eine Entropie-Schätzung (Passphrasen mit den echten
    Listengrößen 7776 / 7724).
  - **Vorlagen**: eingebaut „Standard“, „Nur Buchstaben und Ziffern“, „PIN“
    (6 Ziffern), „Lang und stark“ (32 Zeichen, je Gruppe mindestens 2),
    „Passphrase“ (5 englische Wörter); eigene über das „…“-Menü der Seite
    (Als Vorlage speichern, Änderungen speichern, Umbenennen, Löschen; bis
    50, Name 1–40 Zeichen, kein Name doppelt) in `settings.json` (nicht
    geheim, nicht im Tresor). Eine Vorlage ist der Standard – gewählt nur auf
    der Generator-Seite („Als Standard verwenden“, keine Zeile in den
    Einstellungen); ihn nutzen der Editor-Popover und die Erweiterung
    (Vorschläge, Kontextmenü/Kürzel, Popup ohne eigene Optionen). Eine
    gelöschte Standard-Vorlage fällt auf „Standard“ zurück. Nur die
    Vorlagen-Befehle ändern Vorlagen und Standard; `save_settings` behält sie.
  - **Erweiterung**: Der Popup-Generator verhält sich wie die App (nur auf
    Klick, Vorschau-Hinweis, Kopieren/Ausfüllen). Ohne eigene Optionen gilt
    die Standard-Vorlage der App; „Anpassen“ legt eigene Optionen für diesen
    Browser an (`chrome.storage.local` `generatorOptions`, nur solange
    gültig), „App-Vorlage nutzen“ entfernt sie.
  - **Kompatibilität**: Die neuen `GeneratorOptions`-Felder haben
    Serde-Standardwerte. `generate_password` der Bridge ohne Optionen nimmt
    die Standard-Vorlage, gesendete Felder gelten darüber; vollständige
    Optionen älterer Erweiterungen funktionieren weiter (notfalls über die
    Generator-Standards). `remember_generated` und `generate_password` mit
    `remember: true` bleiben.
  - **Deutsche Wortliste – gefunden und eingebettet**: `de-7776-v1.txt` aus
    <https://github.com/dys2p/wordlists-de>. Lizenz an der Quelle geprüft:
    Unlicense, CC0 oder BSD-3 nach Wahl; Keystead nutzt CC0 1.0, das passt
    zur proprietären Lizenz. 52 Wörter wurden entfernt, damit Passphrasen
    familientauglich bleiben (vulgär, sexuell, gewalttätig, NS-belastet,
    abwertend) → 7724 Wörter ohne Umlaute/ß, log2 ≈ 12,915 Bit je Wort (EFF:
    12,925). Herkunft, Commit-Stände, Prüfsummen und die entfernten Wörter
    stehen in `crates/keystead-core/src/wordlist/README.md`, die Credits im
    README.
  - Geprüft (2026-10-17): fmt, clippy, `cargo test --workspace`,
    `npm run build`, `node --test` grün; Desktop-Oberfläche und
    Erweiterungs-Popup wurden in ihren Stufen im Browser mit Mock-Backend
    durchgeklickt (Deutsch/Englisch). **Noch nicht in der echten App**
    (Tauri + echte Erweiterung) geprüft. Das README-Bild
    `docs/screenshots/keystead-generator.png` ist neu (Mock-Backend:
    Vorschau, Vorlagen, maskierter Verlauf). Vertrag in `ARCHITECTURE.md`
    (Core-API „generator“, „Settings“, Command-Tabelle, „Secrets and the
    webview → Generator“, Bridge `generate_password` / `remember_generated`,
    Erweiterung „Password suggestion“ / „Generator“, UI/UX „Generator“).
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
5. **Idee für später, Block 5 „Mobile“** (Wunsch des Besitzers vom 2026-10-10,
   erst nach Block 4): Keystead auf dem Handy. Rahmen: nur Android (kein Mac,
   nichts Kostenpflichtiges – also auch kein Apple-Entwicklerkonto und keine
   Play-Store-Gebühr; APK über die GitHub-Releases), Tauri-2-Android-App mit
   Handy-Layout zum Nachschlagen/Kopieren, Tresor-Sync über den eigenen
   Cloud-Ordner des Nutzers (Datei ist ohnehin verschlüsselt, Revisionen
   fangen Konflikte ab; Alternative: Sync im Heimnetz per QR-Kopplung),
   Autofill-Dienst zuletzt. Offen: ob Sync über den eigenen Cloud-Ordner für
   den Besitzer in Ordnung ist.
6. **Generator-Überarbeitung** (2026-10-17, Branch `claude/generator`,
   siehe „Was fertig ist“): fertig bis auf die Prüfung in der echten App
   (Tauri + Erweiterung). Danach entscheidet der Besitzer über PR nach
   `main` und eine Beta mit `[release]`. Möglicher Folgeschritt: den
   Generator der Terminal-Version angleichen (siehe „Bekannte Grenzen“).

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
- Generator (Branch `claude/generator`):
  - Die Terminal-Version ist nicht mit überarbeitet: Ihr Generator (`g`)
    erzeugt beim Öffnen und bei jeder Optionsänderung einen Wert (nur im
    Terminal sichtbar, nicht gespeichert) und speichert beim Kopieren (`c`)
    ohne die Prüfung auf den neuesten Eintrag. Sie kennt weder Vorlagen noch
    die Standard-Vorlage noch die neuen Optionen (eigene Sonderzeichen,
    Ausschlüsse, Mindestanzahlen, Buchstabe am Anfang, deutsche Wortliste)
    und nimmt die Generator-Standards; `keystead-cli generate --length`
    erlaubt 5–128 (Core: 4–128).
  - Der Verlauf auf der Generator-Seite aktualisiert sich nicht von selbst,
    wenn die Erweiterung ein Passwort speichert (die Bridge sendet dafür
    kein `vault://changed`); sichtbar nach erneutem Öffnen der Seite.
  - „Verwenden“ im Editor-Popover speichert das Passwort sofort im Verlauf,
    auch wenn der Eintrag danach nicht gespeichert wird („Verwenden“ zählt
    als Benutzung, so entschieden).
  - Vorlagen gehören zum Gerät (`settings.json`), nicht zum Tresor: kein
    Export, kein Import. Versionen bis 2.0.0 verwerfen beim Speichern der
    Einstellungen die eigenen Vorlagen und die Standard-Wahl; die IDs des
    Verlaufs lassen sie beim Speichern des Tresors weg (die neue Version
    vergibt beim Öffnen neue – harmlos).
  - Die Erweiterung bekommt keine Vorlagenliste (die Bridge hat keine
    Nachricht dafür): Das Popup nennt nur „Standard-Vorlage der App“, eigene
    Optionen gelten je Browser.
  - Die deutsche Wortliste hat 7724 statt 7776 Wörter (52 entfernt) – pro
    Wort 0,01 Bit weniger als die EFF-Liste; die Anzeige rechnet mit der
    echten Größe.
- Erweiterung: geschlossene Shadow Roots sind nicht erreichbar (technische
  Grenze); Klicks in den ersten 500 ms nach Erscheinen werden bewusst ignoriert
  (mit Rückmeldung).
- CI: GitHub warnt wegen Node-20-Actions (`actions/checkout@v4`,
  `actions/setup-node@v4`) – irgendwann auf neuere Major-Versionen heben.
- Die Ende-zu-Ende-Testskripte (Playwright + echter Chromium + Debug-App) lagen
  nur im Scratch-Verzeichnis der Cloud-Sitzung und sind nicht im Repo. Für
  künftige Arbeit ggf. neu unter `tests/e2e/` anlegen.
