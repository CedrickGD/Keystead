<!-- Arbeitsdokument, Stand 2026-10-10. Entwurf aus drei unabhängigen Design-Durchgängen (Sicherheit / Nutzer / Wartbarkeit) und einer Zusammenführung; Code-Verweise (Datei:Zeile) wurden gegen den Branch claude/quirky-archimedes-i4qa3j geprüft. Die Entscheidungen D1–D8 sind noch offen und werden mit dem Besitzer geklärt; danach wandern die festgelegten Teile nach ARCHITECTURE.md und dieses Dokument wird entfernt oder gekürzt. -->

# Block 3 „Neue Funktionen“ – zusammengeführter Entwurf

Drei unabhängige Entwürfe (Sicherheit, Nutzer, Wartbarkeit) wurden gegen den Code geprüft und zu einem Vorschlag verschmolzen. Alle drei waren sich in den großen Linien einig; die Abweichungen stehen unten jeweils unter „bewusst ausgelassen“ bzw. in den Entscheidungen.

Gemeinsame Leitplanken (unverändert aus CLAUDE.md/ARCHITECTURE.md): Core bleibt netzwerkfrei; Geheimnisse erreichen die Webview nur auf ausdrückliche Aktion; jeder schreibende Command läuft über `mutate_for_page` + `pageVaultId` (state.rs:619-651); Speicher wird erst nach erfolgreichem Schreiben übernommen (vault.rs:700-788); jede UI-Zeichenkette über de.ts/en.ts; ARCHITECTURE.md und HANDOFF.md werden im selben Commit nachgezogen.

---

## Dateianhänge

### Gewählter Entwurf
- **Ablage als Side-Car, nicht im Payload.** Verzeichnis `<vaults_dir>/<id>.keystead.attachments/` über das vorhandene `format::sibling(path, ".attachments")` (format.rs:481-485, derselbe Mechanismus wie `.bak`/`.lock`). Eine Datei pro Anhang, Name `<uuid>.bin` (nie aus dem Dateinamen des Nutzers abgeleitet). `list_vaults` ignoriert den Ordner bereits (store.rs:76-80), der Portable-Umzug nimmt ihn ohne Codeänderung mit (portable.rs:151-190).
- **Blob-Format:** Magic `KSAT1` + 24-Byte-Nonce + XChaCha20-Poly1305-Ciphertext, geschrieben mit `format::write_atomic` (format.rs:495-505). **Jeder Anhang hat einen eigenen zufälligen 32-Byte-Schlüssel** (`crypto::random_key`, crypto.rs:118-122), AAD = `keystead:v1:attachment:<vault-id>:<attachment-id>` (Muster format.rs:282-292) – Blobs lassen sich nicht zwischen Anhängen oder Tresoren vertauschen.
- **Metadaten + Schlüssel als Map auf `VaultData`, `VaultItem` bleibt unverändert** (Wartbarkeits-Entwurf):
  `VaultData.attachments: BTreeMap<String /*attachment id*/, AttachmentEntry { item_id, name, size, created_at, key /*base64*/ }>` mit `#[serde(default)]` wie `icons` (model.rs:13-24); `AttachmentEntry` bekommt ein handgeschriebenes `Debug` ohne Schlüssel (Muster `IconEntry`, model.rs:39-48) und eine schlüsselfreie Sicht `AttachmentInfo { id, item_id, name, size, created_at }`.
  Begründung gegenüber „Feld auf `VaultItem`“: `store_item` ersetzt das gespeicherte Item komplett durch das von der Page gesendete (verifiziert, vault.rs:829 `std::mem::replace`; übernommen werden nur `created_at`/`deleted_at`/`password_revised_at`) – ein Item-Feld bräuchte zusätzlichen Übernahme-Code; `get_item_for_edit` liefert das ganze `VaultItem` an die Webview (secrets.rs:438-448) – mit der Map kann konstruktionsbedingt kein Schlüsselmaterial dort landen; TUI-Formular (`try_build` ab `original.clone()`, form.rs:287), Bridge-`ItemSummary`, Bitwarden-Exporte und `dedup_key` bleiben unberührt; `export_encrypted_with_params` baut `VaultData` als Struct-Literal (export.rs:54-61) – das neue Feld muss dort explizit `Default::default()` bekommen, der Compiler erzwingt also die Export-Ausnahme.
- **Schlüsselrotation kostet nichts:** Die Schlüssel liegen im Payload, den `commit(.., Some(Rekey))` neu verschlüsselt (vault.rs:737-788). Löschen eines Anhangs = Schlüssel aus dem Payload entfernen (im selben Save wie die Metadaten, „Crypto-Shredding“); die Datei danach best effort löschen.
- **Lebenszyklus:** Blob wird **vor** dem `mutate` geschrieben und bei jedem `Err` (auch `Conflict`) wieder entfernt → der einmalige Retry von `mutate_for_page` ist sauber. `attachments::prune` entfernt Einträge verschwundener Items dort, wo `icons::prune_icons` läuft (vault.rs:717) → `delete_item`/`empty_trash` brauchen keinen Sonderfall; Papierkorb behält die Einträge. `sweep_attachments` räumt verwaiste `<uuid>.bin`/`.tmp` auf, **aber nur wenn älter als 1 h (mtime)**, damit ein Anhang, den gerade die TUI schreibt, nie überholt wird (Sicherheits-/Nutzer-Entwurf). `VaultStore::delete_vault` (store.rs:178-198) entfernt zusätzlich den Ordner.
- **Core-API** (neues Modul `crates/keystead-core/src/attachments.rs` + Methoden auf `UnlockedVault`): `add_attachment(item_id, name, bytes) -> AttachmentInfo`, `remove_attachment(id)`, `read_attachment(id) -> Zeroizing<Vec<u8>>`, `attachments_of(item_id)`, `sweep_attachments()`. Fehlercodes: `invalid_input:attachment_name_required|attachment_name_too_long|attachments_too_many`, `unsupported:attachment_too_large`, `corrupt:…` bei Manipulation, `not_found` bei fehlender Datei.
- **Desktop-Commands** (neues Modul `src-tauri/src/attachments.rs`, alle mit `pageVaultId`, alle Aktivität):
  - `add_attachment(itemId, path)` → `ItemListEntry`: Datei **außerhalb** des State-Locks nach dem Import-Muster lesen (Metadaten-Längencheck, `take(limit+1)` in `Zeroizing`, import.rs:246-257), dann `mutate_for_page`.
  - `save_attachment(attachmentId, path)`: Schlüssel/Handle + `lock_epoch` unter dem Lock lesen, entschlüsseln außerhalb, **Epoche vor dem Schreiben erneut prüfen** (Muster `copy_to_clipboard_since`, state.rs:478-501), `write_atomic` an den Dialog-Pfad; Ziel innerhalb `vaults_dir()` oder mit Endung `.keystead` ablehnen; Pfad über `import_flow::remember_export` merken, damit das vorhandene `show_export` („Im Ordner anzeigen“) ihn öffnet.
  - `remove_attachment(attachmentId)` → `ItemListEntry`.
- **Redaktion:** `secrets::ItemListEntry` (secrets.rs:40-58) bekommt `attachments: Vec<{ id, name, size, createdAt }>` – Dateinamen gelten wie Notizen als persönlich, nicht geheim (ARCHITECTURE.md Redaktionsvertrag); Namen werden in allen `Wipe`-Impls genullt. Bytes erreichen die Page nie, außer über die optionale Bild-Vorschau (Entscheidung D3).
- **UI:** Abschnitt „Anhänge“ in der Detailansicht (`ItemView.tsx` nach `CustomFields`), je Zeile Name, Größe (`formatBytes`, neu), Datum, Aktionen „Speichern unter …“ und „Entfernen“ (bei Papierkorb ausgeblendet), Button „Datei anhängen …“ (nativer Dialog), Büroklammer-Chip mit Anzahl im Kopf. Der Editor bleibt unberührt – Anhänge sind Backend-Aktionen auf dem gespeicherten Item, nicht Teil des Entwurfs. Export-Dialog zeigt einen Hinweis, dass Anhänge nicht im Export enthalten sind; Einstellungen → Tresor zeigt Anzahl/Größe und den Hinweis „Sicherung = Tresordatei + Ordner“. TUI: eine schreibgeschützte Zeile „Anhänge: n“ hinter dem Notizblock.
- **Limits:** 20 MiB pro Datei (vor dem Lesen aus den Metadaten geprüft), 10 pro Element, Name ≤ 200 Zeichen (Entscheidung D2).

### Begründung
Der Payload wird vom Monitor alle 2 s komplett gelesen und zweimal geparst (monitor.rs:136-143 → vault.rs:661 → format.rs:172-196), bei jeder Mutation komplett geklont (vault.rs:704) und bei jedem Save hübsch gedruckt plus als `.bak` kopiert (format.rs:509-513). Ein einziger 5-MB-Scan würde jeden Favoriten-Klick und jede Icon-Charge zu einem Mehrfach-Megabyte-Schreibvorgang machen. Side-Car + Schlüssel im Payload ist der einzige Entwurf, bei dem Rotation, Konflikt/Reload, Papierkorb, Portable-Umzug und Export-Ausnahme aus vorhandenen Mechanismen folgen.

### Bewusst ausgelassen
- Anhänge in Exporten (Keystead/CSV/Bitwarden) – siehe D4; vollständige Sicherung ist das Datenverzeichnis.
- „Mit Standard-App öffnen“: bräuchte eine Klartext-Temp-Datei, die Windows-Viewer offen halten, Indexer/Backups sehen und Keystead nicht zuverlässig löschen kann (keine Temp-Verwaltung vorhanden, platform.rs kennt nur `open_url`/`open_folder`).
- Drag & Drop auf ein Element (kollidiert mit dem globalen Import-Drop-Ziel, ARCHITECTURE.md:886-898).
- Neuverschlüsselung aller Blobs bei Rotation: eine alte Tresorkopie + altes Passwort gibt die Schlüssel der damals vorhandenen Anhänge preis – exakt die Daten, die die Kopie ohnehin „enthielt“ (ARCHITECTURE.md:173-174); wird dokumentiert, kann später als „Anhänge neu verschlüsseln“ nachgerüstet werden.
- Pro-Tresor-Gesamtlimit, Fortschrittsereignisse, MIME-Sniffing/neue Crates (`infer`/`mime` nur transitiv).

---

## Geleakte Passwörter (HIBP)

### Gewählter Entwurf
- **Aufteilung:** Reine Logik im Core, Netzwerk nur in `src-tauri` (Webview-CSP `connect-src ipc: http://ipc.localhost`, tauri.conf.json:29).
  - Core `crates/keystead-core/src/pwned.rs` (kein neues Crate; `sha1 0.11` und `data-encoding` sind da): `pwned_hash(password) -> PwnedHash { prefix: String /*5 Hex*/, suffix: Zeroizing<String> /*35 Hex*/ }` (Debug zeigt nur den Prefix), `pwned_count(body, &hash) -> u64` (Zeilen `SUFFIX:COUNT`, CRLF/LF, Groß-/Kleinschreibung egal, Vergleich mit `crypto::ct_eq`, Padding-Zeilen mit 0 = nicht geleakt), `breach_candidates(&VaultData)` gruppiert nicht-gelöschte Logins nach identischem Passwort (wie `by_password` in health.rs:99-123) → eine Anfrage pro **unterschiedlichem** Passwort. Unit-Test mit dem bekannten Vektor SHA-1(„password“) = `5BAA6` + `1E4C9B93F3F0682250B6CF8331B7EE68FD8`.
  - Backend `src-tauri/src/pwned.rs`: ein `reqwest::Client` im `OnceLock` nach dem Muster `update::probe_client` (update.rs:210-221, inkl. ring-Provider-Installation wegen `rustls-no-provider`), Timeout 10 s, `referer(false)`, keine Redirects, Body-Cap 512 KiB, Header `Add-Padding: true` und `Accept: text/plain`, URL ausschließlich `https://api.pwnedpasswords.com/range/<PREFIX>` (kein Query, kein Body). Nicht-2xx → `io:HTTP <status>` (auch 429, kein automatischer Retry).
- **Commands** (beide Nutzeraktion):
  - `check_pwned_password(password)` – für den Editor; Argument sofort in `Zeroizing`, Hash im Core, nur der Prefix verlässt den Prozess.
  - `check_pwned_vault(pageVaultId)` → `{ checkedAt, checked, leaked: [{ itemId, count }], failed }` – Schritt 1 unter dem State-Lock: Kandidaten + `lock_epoch`; Schritt 2 ohne Lock: max. 4 parallele Anfragen (JoinSet wie `fetch_all`, icons.rs:455-500) mit Abbruch bei `is_exiting()`/Epochenwechsel/erstem 429; Schritt 3: Zuordnung zu Item-Ids; Epochenwechsel → `locked`, Ergebnis verworfen.
- **Kein neues Settings-Feld, nur auf Knopfdruck** (Entscheidung D5): Vor dem tresorweiten Lauf ein Bestätigungsdialog, der genau sagt, was das Gerät verlässt, mit Häkchen „Nicht mehr fragen“ (nur `localPrefs`, kein Backend-Setting). Nichts läuft automatisch, nichts im Hintergrund.
- **UI:** Fünfte `HealthCard` „Geleakte Passwörter“ (tone danger, als erste Karte) mit den Zuständen „Noch nicht geprüft“ / prüfend / Ergebnis (Zeilen nach Count sortiert, „in {n} Datenlecks gefunden“, `failed`-Warnung), Chip „{n} geleakt“ in der Zusammenfassung. Ergebnis lebt in einem kleinen Per-Tresor-Kontext (`state/breaches.tsx`, Muster `IconsProvider`), damit es das Hin-und-Her zwischen Bericht und Element überlebt; Einträge eines Elements verfallen, sobald dessen `updatedAt` sich ändert; alles verfällt beim Sperren (Page-Reload). `ItemView` zeigt bei bekanntem Treffer einen `callout-danger` mit „Passwort ändern“. Editor: Link-Button „Auf Leaks prüfen“ unter dem Stärke-Meter, Ergebnis verschwindet bei jeder Änderung. `io:`-Fehler werden als „Have I Been Pwned ist nicht erreichbar: {detail}“ gerendert (Muster `useUpdateErrorText`), nicht als Dateifehler.
- **Proxy/User-Agent** (Entscheidung D6): durch den System-Proxy (Feature `system-proxy` ist aktiv), **fail closed** bei PAC/unverständlicher Konfiguration über das vorhandene `icons::proxy_setup_unclear` (icons.rs:756-765, `pub(crate)` machen) → `unsupported:pwned_proxy`; User-Agent `Keystead/<version>` wie der Updater.
- **Datenschutz:** Es verlassen das Gerät exakt 5 Hex-Zeichen im URL-Pfad plus feste Header. Suffix-Vergleich lokal, der volle SHA-1 existiert nur als `Zeroizing` im Core, wird nie geloggt/persistiert/gesendet. Padding verhindert Rückschlüsse aus der Antwortgröße. Nichts wird gecacht oder in den Tresor geschrieben (ein `leakedCount` im Payload würde in Keystead-Exporte wandern, export.rs:54-61). Logs enthalten nur Zähler.
- **Doku:** Neuer Abschnitt „Leaked-password check“ in ARCHITECTURE.md hinter „Website icons“, Einleitungssatz Z. 8-10 um den dritten (opt-in) Netzwerkzugriff ergänzen, README-Datenschutzabsätze (de+en), ein Satz in LIESMICH.txt.

### Begründung
„Standardmäßig aus bzw. nur auf Knopfdruck“ ist mit Button-only wörtlich erfüllt, ohne Settings-Feld, TS-Spiegel, `json_shape`-Test (settings.rs:302 erwartet 12 Schlüssel) und `apply_settings`-Plumbing. Der Bestätigungsdialog ist der Ort für die Datenschutz-Erklärung. Der Icon-Fetcher meidet Proxys, weil die Zielhosts aus Tresordaten stammen (SSRF, eigene Adresse gegenüber Konto-Websites) – beides trifft auf einen festen öffentlichen Host nicht zu; ein Proxy sieht per HTTPS nur `CONNECT api.pwnedpasswords.com:443`, nie den Prefix. Fail-closed bei PAC hält die Regel „nie am Proxy vorbei“.

### Bewusst ausgelassen
- Automatische Prüfung beim Speichern/Öffnen des Berichts (später als `Settings.breachCheck`, Default false, additiv nachrüstbar).
- Treffer im `HealthReport`/Score (Core bleibt synchron und offline; die rote Karte kommuniziert Dringlichkeit).
- Persistenter Cache, In-Memory-Prefix-Cache, Prüfung einzelner gespeicherter Logins aus der Detailansicht (der tresorweite Lauf deckt das ab; nachrüstbar).
- HIBP in TUI/CLI (TUI hat keine HTTP-Abhängigkeit) und in der Erweiterung.

---

## Tags

### Gewählter Entwurf
- **Modell:** `VaultItem.tags: Vec<String>` (Container `#[serde(default)]` macht ältere Payloads kompatibel, model.rs:60-61). Normalisierung **im Core** (`normalize_tags`, aufgerufen in `save_item` nach `validate_name`, vault.rs:253-258, und verlustbehaftet in `add_imported`): trimmen, Leerstrings weg, 50 Zeichen (`invalid_input:tag_too_long`), 20 pro Element (`invalid_input:tags_too_many`), Duplikate **groß-/kleinschreibungsunabhängig** entfernen und die erste Schreibweise behalten, Reihenfolge unverändert (der Dirty-Check `itemsEqual` ist reihenfolgeabhängig). Tags werden in `wipe_item`, `Wipe for VaultItem`, `Wipe for ItemListEntry` genullt (persönlich wie Notizen).
- **Suche:** `matches_terms` (vault.rs:917-934) und UI-`matchesSearch` (utils.ts:137-151) nehmen Tags in den Heuhaufen – damit suchen Desktop, TUI-Liste und Bridge-`search` identisch.
- **Spiegel:** `secrets::ItemListEntry.tags`, TS `VaultItem`/`ItemListEntry`, `redact.ts`, `newItem`, Mock `normalizeItem`/`saveItem`, `sampleData` mit ein paar getaggten Demo-Einträgen. **`ItemSummary` bleibt unverändert** – kein Bridge-/Erweiterungs-Protokollwechsel, die Exact-JSON-Tests (protocol_json.rs:415-450) bleiben grün.
- **UI:** Sidebar-Gruppe „Tags“ zwischen Ordnern und Papierkorb (ein `NavItem` pro Tag mit Zähler, abgeleitet aus den Items, Zählung ohne Papierkorb, Sortierung mit dem vorhandenen `Intl.Collator`); Filtermodell `{ kind: 'tag', tag }` in `model.ts` inkl. `sameFilter`/`filterItems`/`Counts.tags`; `startNew` übernimmt den Tag des aktiven Filters; wird der gefilterte Tag leer, zurück auf „Alle“ (wie bei gelöschtem Ordner). Detailansicht: klickbare Chips im Kopf → Tag-Filter. Editor: eigener Abschnitt „Tags“ vor den Zusatzfeldern mit Chips (X zum Entfernen), Eingabe mit Enter/Komma, Backspace entfernt den letzten Chip, Vorschläge aus den vorhandenen Tags des Tresors; Limit-Feedback inline. Listenzeilen bleiben ruhig (keine Chips).
- **TUI:** eine Zeile `Tags: Bank, Familie` **nach** der Meta-Zeile (hält den Rendering-Test `Login · * Favorit`, ui/tests.rs:79), neuer `M::FieldTags`-Schlüssel; keine Bearbeitung in der TUI (reiht sich in HANDOFF Z. 123 ein; `try_build` bewahrt Tags bei Edits).
- **Export/Import:** Keystead-Export trägt Tags automatisch und bringt sie zurück (normalisiert); CSV/Bitwarden-JSON lassen sie weg (dokumentiert, Hinweis im Export-Dialog); `dedup_key`/`take_over` bleiben tag-blind.
- **Umbenennen/Löschen eines Tags** über die Sidebar (Entscheidung D7, empfohlen): `rename_tag`/`delete_tag` als Core-Methoden über **alle** Items inkl. Papierkorb in einem `mutate_if_changed` (idempotent, Retry-sicher) plus zwei Dagger-Commands nach dem Vorbild `delete_folder`.

### Begründung
Freie Strings auf dem Item brauchen keine Id-Tabelle, keine neuen CRUD-Commands, wandern automatisch durch den Keystead-Export und können nicht „baumeln“. Normalisierung im Core schützt jeden Schreiber (TUI, Bridge, Importe). Das ist das kleinste Feature und übt einmal jede Spiegelschicht – deshalb kommt es zuerst.

### Bewusst ausgelassen
- `#tag`-Suchsyntax (müsste in Core, UI und Mock identisch gepflegt werden).
- Tags auf `ItemSummary`/im Popup, `--tag`-Flag in der CLI, Mehrfach-Tag-Filter, Tag-Chips in Listenzeilen, Tags als Bitwarden-Custom-Field.

---

## Einrichtungsassistent: Website-Icons

### Gewählter Entwurf
Ja – **ein `Switch`-Zeile im ersten Schritt „Tresor“** (`CreateVaultStep`, WelcomeScreen.tsx:191-269, zwischen Passwortfeldern und Warn-Callout), **nur beim allerersten Start** (`vaults.length === 0`; die Einstellung ist global), vorbelegt mit `settings.websiteIcons`, gespeichert über `updateSettings({ websiteIcons })` aus `useApp()` **bevor** `api.createVault` läuft. Texte: vorhandener Titel `settings.websiteIcons`, ein kurzer neuer Hinweis `wizard.websiteIconsHint` („Dafür kontaktiert Keystead jede gespeicherte Website … jederzeit änderbar unter Einstellungen → Allgemein“). Kein Backend-Code, ~30 Zeilen UI, Prüfung manuell mit `?mock=empty`. Standardwert bleibt **an** (Entscheidung D8).

### Begründung
Timing ist das ganze Argument, und es ist verifiziert: `install_vault_since` ruft `icons.vault_opened()` (state.rs:565, `OPEN_DELAY` 5 s, icons.rs:64) – harmlos, weil ein frischer Tresor keine Logins hat –, aber **jeder erfolgreiche `mutate_with` ruft `icons.vault_changed()`** (state.rs:646-649, `CHANGE_DELAY` 2 s, icons.rs:67), also auch `commit_import` im Import-Schritt des Assistenten. Im Pfad „Von VaultX umsteigen“ werden damit alle importierten Hosts kontaktiert, während der Nutzer noch den Schritt „Absicherung“ sieht – bevor er Einstellungen → Allgemein je erreichen kann. Eine Karte auf dem letzten Schritt (wie die Erweiterungs-Karte) käme zu spät; ein eigener Schritt bläht den bewusst schlanken Assistenten auf. Der Tresor-Schritt ist die einzige Stelle, die in beiden Pfaden vor dem Import liegt und schon ein Formular ist.

### Bewusst ausgelassen
Eigener „Privatsphäre“-Schritt, Karte auf dem letzten Schritt, Checklisten-Zeile im StartPanel (alle nach dem ersten Netzwerkkontakt), Backend-Sperre des Fetchers bis zum Assistenten-Ende (kein Vorteil gegenüber der Zeile).

---

## Entscheidungen für dich

Nur Punkte, bei denen deine Antwort die Umsetzung wirklich ändert. Alles andere ist oben festgelegt und wird bei Widerspruch gern angepasst.

**D1 – Formatversion bei Anhängen (Block: Anhänge, blockierend für den Anhang-Core).** Ein älteres Binary (v. a. eine alte `keystead-cli.exe` aus einem älteren ZIP – die liegt separat im Portable-Paket, build.yml:155), das den Tresor speichert, wirft unbekannte Felder still weg (serde ignoriert unbekannte Schlüssel). Bei Tags ist das ärgerlich, bei der Anhang-Map **unwiederbringlich** (Schlüssel weg → Blobs unlesbar).
- a) **Empfohlen:** Tresordatei bekommt `version: 2`, sobald die Anhang-Map nicht leer ist; der Leser für 1 **und** 2 wird schon mit dem Tags-Release ausgeliefert. Ältere Builds verweigern dann solche Tresore sauber (`unsupported:vault format version 2`, format.rs:185-191) statt Schlüssel zu vernichten; Tresore ohne Anhänge bleiben v1 und überall öffenbar.
- b) Bei Version 1 bleiben, Risiko in README/LIESMICH/HANDOFF dokumentieren („`keystead-cli.exe` immer aus derselben Version wie die App“).
- c) Version 2 ab sofort für jeden Save (bricht alle älteren Betas sofort).

**D2 – Limits für Anhänge (Block: Anhänge, nicht blockierend).**
- a) **Empfohlen:** 20 MiB pro Datei, 10 pro Element, kein Tresor-Gesamtlimit (Nutzung in Einstellungen → Tresor sichtbar).
- b) 10 MiB / 10 / 256 MiB Gesamtlimit (strenger, hält Portable-Umzüge klein).
- c) 25 MiB / 10 / 500 MiB.
Fester Rahmen: Einmal-AEAD hält maximal ~2× Dateigröße im RAM; `switch_portable_mode` hält den State-Lock während des gesamten Kopierens.

**D3 – Anhänge ansehen (Block: Anhänge, nicht blockierend – kann als letzter Anhang-Schritt kommen).**
- a) **Empfohlen:** „Speichern unter …“ + „Im Ordner anzeigen“ **plus** eine Bild-Vorschau direkt in der App: nur Bilder (per Magic-Bytes erkannt, kein neues Crate), ≤ 8 MiB, als `data:`-URL (CSP erlaubt `img-src data:`), mit denselben Regeln wie enthüllte Geheimnisse (30 s, Blur, Sperre, Unmount; `useRevealedSecret`-Muster). Deckt „Ausweis-Scan kurz ansehen“ im `contentProtected`-Fenster ab.
- b) Nur „Speichern unter …“ + „Im Ordner anzeigen“ (keine Bytes je in der Webview).
- c) Zusätzlich „Öffnen“ über Klartext-Temp-Datei – **nicht empfohlen** (siehe oben).

**D4 – Exporte ohne Anhänge (Block: Anhänge, nicht blockierend, eher Bestätigung).**
- a) **Empfohlen:** Kein Exportformat enthält in Block 3 Anhänge; Export-Dialog und Einstellungen sagen es; vollständige Sicherung = Datenverzeichnis (Tresordatei + Ordner).
- b) Neues Container-Format „.keystead-bundle“ mit Import (eigener Block: Format, `IMPORT_MAX_BYTES` 50 MiB, Streaming).

**D5 – HIBP-Schalter (Block: HIBP, nicht blockierend – ein Setting ist additiv nachrüstbar).**
- a) **Empfohlen:** Nur auf Knopfdruck, Bestätigungsdialog mit „Nicht mehr fragen“ (lokal), kein Settings-Feld.
- b) Settings-Feld `breachCheck` (Default aus) als **harte Sperre** im Backend: ohne Opt-in antworten die Commands `unsupported:breach_check_off`, die Buttons führen zuerst in die Einstellungen.
- c) Settings-Feld als **Automatik-Stufe**: Buttons immer nutzbar; eingeschaltet prüft der Sicherheitsbericht beim Öffnen automatisch und der Editor beim Verlassen des Passwortfelds.

**D6 – Netzwerkverhalten des HIBP-Clients (Block: HIBP, nicht blockierend).**
- a) **Empfohlen:** durch den System-Proxy, fail closed bei PAC/unklarer Konfiguration (`unsupported:pwned_proxy`), direkt nur ohne konfigurierten Proxy; User-Agent `Keystead/<version>` wie der Updater.
- b) Wie der Icon-Fetcher: nur direkt, bei irgendeinem Proxy gar keine Anfrage, User-Agent `Mozilla/5.0` (Button in Firmennetzen wirkungslos; HIBP verlangt laut Doku einen identifizierenden UA – offline nicht verifiziert).
- c) Wie der Updater: System-Proxy, aber bei PAC direkt (geht am Proxy vorbei).
Beide Abweichungen vom Icon-Fetcher werden in ARCHITECTURE.md als bewusste Ausnahme festgehalten.

**D7 – Tag umbenennen/löschen über die Sidebar (Block: Tags, nicht blockierend).**
- a) **Empfohlen:** Ja – `rename_tag`/`delete_tag` nach dem Ordner-Muster, ein Save über alle Items inkl. Papierkorb (~80 Zeilen quer durch Core/Commands/UI).
- b) Nein – Tags nur pro Element bearbeiten; ein Tippfehler auf 30 importierten Elementen wäre mühsam.

**D8 – Website-Icons im Assistenten (Block: Assistent, blockierend nur für diesen Schritt – deine offene Frage).**
- a) **Empfohlen:** Switch-Zeile im Tresor-Schritt, nur beim ersten Start, Standard bleibt **an**.
- b) Dasselbe, aber Standard **aus** (Opt-in; ändert den dokumentierten Vertrag in ARCHITECTURE.md:584 und README).
- c) Nichts im Assistenten; Einstellungen bleiben der einzige Ort.

**Release-Takt (kein eigener Punkt, nur zur Info):** geplant ist ein `[release]` pro Feature – Tags (beta.9), Anhänge (beta.10), HIBP (beta.11), die Assistenten-Zeile mit dem jeweils nächsten. Die erste Zeile des Release-Commits ist der „Was ist neu?“-Text (latest-json.mjs) und wird deutsch formuliert. Sag Bescheid, wenn du lieber ein Sammel-Release willst.