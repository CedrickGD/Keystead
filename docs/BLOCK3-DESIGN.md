# Block 3 „Neue Funktionen“ – Entscheidungen (Protokoll)

Stand 2026-10-15. Der ursprüngliche Entwurf vom 2026-10-10 (drei unabhängige
Design-Durchgänge – Sicherheit, Nutzer, Wartbarkeit – und eine Zusammenführung)
ist vollständig umgesetzt; die verbindlichen Verträge stehen in
[`ARCHITECTURE.md`](ARCHITECTURE.md) (Abschnitte „Attachments“,
„Leaked-password check“, Tags im Modell/Command-Teil, Website-Icons für den
Assistenten), der Stand in [`HANDOFF.md`](HANDOFF.md). Hier bleibt nur das
Protokoll der Entscheidungen des Besitzers, damit sie nicht erneut aufgemacht
werden.

| Nr. | Frage | Entscheidung (2026-10-10) | Umgesetzt in |
|---|---|---|---|
| D1 | Tresordatei-Version bei Anhängen | Version 2 nur, solange ein Tresor Anhänge hat, sonst 1; der Leser für 1 und 2 kam schon mit den Tags (beta.9) und bewahrt unbekannte Felder in Kopf und Payload beim Speichern (`model::UnknownFields`) | beta.9 (Leser), beta.10 (Schreiber) |
| D2 | Limits für Anhänge | 20 MiB pro Datei, 10 pro Element, kein Tresor-Gesamtlimit, Name ≤ 200 Zeichen | beta.10 |
| D3 | Anhänge ansehen | „Speichern unter …“ + „Im Ordner anzeigen“ und eine Bild-Vorschau in der App (PNG/JPEG/GIF/WebP ≤ 8 MiB, Regeln wie bei aufgedeckten Geheimnissen); kein „Öffnen mit“ über Klartext-Temp-Dateien | beta.10 |
| D4 | Anhänge in Exporten | Kein Exportformat enthält Anhänge; vollständige Sicherung = Datenverzeichnis (Tresordatei + Anhang-Ordner); Hinweise im Export-Dialog und in den Einstellungen | beta.10 |
| D5 | Auslösen der Leak-Prüfung (Have I Been Pwned) | Nur auf Knopfdruck (Sicherheitsbericht, Editor), Bestätigungsdialog mit „Nicht mehr fragen“ (nur lokal), kein Setting, nichts automatisch | beta.11 |
| D6 | Netzwerkverhalten der Leak-Prüfung | Durch den System-Proxy, „fail closed“ bei PAC/unklarer Konfiguration (`unsupported:pwned_proxy`), User-Agent `Keystead/<Version>`; nur die ersten 5 Hex-Zeichen des SHA-1 verlassen den Rechner, `Add-Padding: true` | beta.11 |
| D7 | Tag umbenennen/löschen über die Seitenleiste | Ja, über alle Einträge inkl. Papierkorb | beta.9 |
| D8 | Website-Icons im Einrichtungsassistenten | Ja, Schalter im Schritt „Tresor“, nur beim allerersten Start, Standard bleibt „an“ | beta.9 |

Gewählte Architektur in einem Satz je Funktion:

- **Tags**: freie Strings am Element, im Core normalisiert (50 Zeichen, 20 je
  Element, Duplikate ohne Rücksicht auf Groß-/Kleinschreibung); Keystead-Export
  nimmt sie mit, CSV/Bitwarden-JSON nicht; `ItemSummary` (Bridge/Erweiterung)
  unverändert.
- **Anhänge**: je Anhang eine einzeln verschlüsselte Datei neben der
  Tresordatei (`<id>.keystead.attachments/<uuid>.bin`, XChaCha20-Poly1305,
  AAD bindet Tresor- und Anhang-Id), eigener Zufallsschlüssel je Anhang im
  verschlüsselten Payload (Rotation kostet nichts, Löschen = Schlüssel weg);
  Blob vor der Payload-Änderung schreiben und bei jedem Fehler wieder
  entfernen; verwaiste Dateien älter als 1 h werden beim Entsperren weggefegt.
- **Leak-Prüfung**: Hash-Logik netzwerkfrei im Core (`pwned.rs`), HTTP nur in
  `src-tauri` (Client je Aufruf, damit Proxy-Regel und Client dieselben
  Einstellungen sehen); Ergebnis nur im Seitenspeicher, nie im Tresor.

Bewusst ausgelassen (nachrüstbar): automatische Leak-Prüfung per Setting,
Treffer im `HealthReport`-Score, Anhänge in einem Container-Export,
`#tag`-Suchsyntax, Tags im Erweiterungs-Popup.
