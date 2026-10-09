//! German/English UI strings.
//!
//! A simple match-based table: every [`M`] variant has a German and an
//! English text. Placeholders are written `{name}` and filled with
//! [`Lang::tf`].

use vaultx_core::settings::Language;
use vaultx_core::Error;

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

impl From<Language> for Lang {
    fn from(l: Language) -> Self {
        match l {
            Language::De => Lang::De,
            Language::En => Lang::En,
        }
    }
}

macro_rules! messages {
    ($($key:ident => $de:expr, $en:expr;)*) => {
        /// Message keys.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum M { $($key),* }

        impl M {
            /// Every key (used by the consistency test).
            #[cfg(test)]
            pub const ALL: &'static [M] = &[$(M::$key),*];

            fn de(self) -> &'static str {
                match self { $(M::$key => $de),* }
            }

            fn en(self) -> &'static str {
                match self { $(M::$key => $en),* }
            }
        }
    };
}

messages! {
    // ---- general ----------------------------------------------------------
    Yes => "Ja", "Yes";
    No => "Nein", "No";
    YesKey => "j", "y";
    NoKey => "n", "n";
    Items => "Elemente", "Items";
    ItemCountOne => "1 Element", "1 item";
    ItemCount => "{n} Elemente", "{n} items";
    MatchCount => "{n} von {total}", "{n} of {total}";
    Search => "Suche", "Search";
    SearchPlaceholder => "Tippen zum Suchen", "Type to search";
    NoItems => "Noch keine Elemente. Mit n legst du ein Login an.", "No items yet. Press n to add a login.";
    NoMatches => "Keine Treffer.", "No matches.";
    Details => "Details", "Details";
    NoSelection => "Kein Element ausgewählt.", "No item selected.";
    TooSmall => "Fenster zu klein", "Window too small";
    // ---- item types -------------------------------------------------------
    TypeLogin => "Login", "Login";
    TypeCard => "Karte", "Card";
    TypeIdentity => "Identität", "Identity";
    TypeNote => "Sichere Notiz", "Secure note";
    LetterLogin => "L", "L";
    LetterCard => "K", "C";
    LetterIdentity => "I", "I";
    LetterNote => "N", "N";
    // ---- fields -----------------------------------------------------------
    FieldName => "Name", "Name";
    FieldUsername => "Benutzername", "Username";
    FieldPassword => "Passwort", "Password";
    FieldTotp => "Einmalcode", "One-time code";
    FieldTotpKey => "2FA-Schlüssel", "2FA key";
    FieldWebsite => "Website", "Website";
    FieldNotes => "Notizen", "Notes";
    FieldFolder => "Ordner", "Folder";
    FieldFavorite => "Favorit", "Favorite";
    FieldCardholder => "Karteninhaber", "Cardholder";
    FieldBrand => "Marke", "Brand";
    FieldCardNumber => "Kartennummer", "Card number";
    FieldExpiry => "Gültig bis", "Expires";
    FieldExpMonth => "Monat (MM)", "Month (MM)";
    FieldExpYear => "Jahr (JJJJ)", "Year (YYYY)";
    FieldSecurityCode => "Sicherheitscode", "Security code";
    FieldTitle => "Anrede", "Title";
    FieldFirstName => "Vorname", "First name";
    FieldLastName => "Nachname", "Last name";
    FieldEmail => "E-Mail", "Email";
    FieldPhone => "Telefon", "Phone";
    FieldCompany => "Firma", "Company";
    FieldAddress => "Adresse", "Address";
    FieldAddress1 => "Straße und Nr.", "Address line 1";
    FieldAddress2 => "Adresszusatz", "Address line 2";
    FieldPostalCode => "PLZ", "Postal code";
    FieldCity => "Ort", "City";
    FieldState => "Bundesland", "State";
    FieldCountry => "Land", "Country";
    FieldUpdated => "Geändert", "Updated";
    CustomFields => "Weitere Felder", "Custom fields";
    TotpRemaining => "noch {s} s", "{s}s left";
    TotpInvalid => "Ungültiger 2FA-Schlüssel", "Invalid 2FA key";
    ShowSecretsHint => "s: anzeigen", "s: show";
    // ---- relative time ----------------------------------------------------
    JustNow => "gerade eben", "just now";
    MinutesAgo => "vor {n} min", "{n} min ago";
    HoursAgo => "vor {n} h", "{n} h ago";
    DaysAgoOne => "vor 1 Tag", "1 day ago";
    DaysAgo => "vor {n} Tagen", "{n} days ago";
    MonthsAgoOne => "vor 1 Monat", "1 month ago";
    MonthsAgo => "vor {n} Monaten", "{n} months ago";
    YearsAgoOne => "vor 1 Jahr", "1 year ago";
    YearsAgo => "vor {n} Jahren", "{n} years ago";
    // ---- key hints --------------------------------------------------------
    HintHelp => "Hilfe", "Help";
    HintSearch => "Suchen", "Search";
    HintUsername => "Benutzer", "User";
    HintPassword => "Passwort", "Password";
    HintTotp => "Code", "Code";
    HintOpen => "Öffnen", "Open";
    HintNew => "Neu", "New";
    HintEdit => "Bearbeiten", "Edit";
    HintTrash => "Löschen", "Delete";
    HintGenerator => "Generator", "Generator";
    HintLock => "Sperren", "Lock";
    HintQuit => "Beenden", "Quit";
    HintBack => "Zurück", "Back";
    HintDone => "Fertig", "Done";
    HintClear => "Leeren", "Clear";
    HintSelect => "Auswahl", "Select";
    HintUnlock => "Entsperren", "Unlock";
    HintOpenVault => "Öffnen", "Open";
    HintNewVault => "Neuer Tresor", "New vault";
    HintNextField => "Nächstes Feld", "Next field";
    HintSave => "Speichern", "Save";
    HintCancel => "Abbrechen", "Cancel";
    HintGenerate => "Generieren", "Generate";
    HintReveal => "Anzeigen", "Reveal";
    HintNewline => "Neue Zeile", "New line";
    HintOption => "Option", "Option";
    HintChange => "Ändern", "Change";
    HintToggle => "Umschalten", "Toggle";
    HintRegenerate => "Neu", "New";
    HintCopy => "Kopieren", "Copy";
    KeySpace => "Leertaste", "Space";
    KeyCtrl => "Strg", "Ctrl";
    // ---- status messages --------------------------------------------------
    WhatUsername => "Benutzername", "Username";
    WhatPassword => "Passwort", "Password";
    WhatCode => "Code", "Code";
    WhatCardNumber => "Kartennummer", "Card number";
    Copied => "{what} kopiert", "{what} copied";
    CopiedClears => "{what} kopiert – wird in {s} s aus der Zwischenablage gelöscht", "{what} copied – clipboard will be cleared in {s}s";
    NoUsername => "Dieses Element hat keinen Benutzernamen.", "This item has no username.";
    NoPassword => "Dieses Element hat kein Passwort.", "This item has no password.";
    NoTotp => "Für dieses Element ist kein 2FA-Schlüssel hinterlegt.", "This item has no 2FA key.";
    NoUrl => "Für dieses Element ist keine Website hinterlegt.", "This item has no website.";
    OpenedUrl => "Im Browser geöffnet: {url}", "Opened in browser: {url}";
    UrlNotAllowed => "Nur http- und https-Adressen können geöffnet werden.", "Only http and https addresses can be opened.";
    OpenFailed => "Der Browser konnte nicht geöffnet werden: {err}", "Could not open the browser: {err}";
    ClipboardError => "Zwischenablage nicht verfügbar: {err}", "Clipboard not available: {err}";
    Locked => "Tresor gesperrt.", "Vault locked.";
    AutoLocked => "Tresor nach {m} min Inaktivität gesperrt.", "Vault locked after {m} min of inactivity.";
    ExternalChange => "Der Tresor wurde an anderer Stelle geändert und neu geladen.", "The vault was changed elsewhere and has been reloaded.";
    ConflictRetried => "Der Tresor wurde zwischenzeitlich geändert – neu geladen und erneut gespeichert.", "The vault had been changed elsewhere – reloaded and saved again.";
    ItemCreated => "Element erstellt", "Item created";
    ItemSaved => "Änderungen gespeichert", "Changes saved";
    ItemTrashed => "„{name}“ in den Papierkorb verschoben", "Moved “{name}” to the trash";
    SecretsShown => "Geheime Felder werden angezeigt", "Secrets are visible";
    SecretsHidden => "Geheime Felder werden verborgen", "Secrets are hidden";
    VaultFileError => "Tresordatei konnte nicht neu geladen werden: {err}", "Could not reload the vault file: {err}";
    PasswordGenerated => "Neues Passwort eingesetzt – Strg+R zeigt es an.", "New password inserted – Ctrl+R reveals it.";
    SettingsSaveFailed => "Einstellungen konnten nicht gespeichert werden: {err}", "Could not save the settings: {err}";
    // ---- unlock / vault picker / create vault -----------------------------
    UnlockTitle => "Tresor entsperren", "Unlock vault";
    VaultLabel => "Tresor", "Vault";
    MasterPassword => "Master-Passwort", "Master password";
    Unlocking => "Entsperre …", "Unlocking …";
    WrongPassword => "Falsches Master-Passwort. Bitte versuche es erneut.", "Wrong master password. Please try again.";
    PasswordRequired => "Bitte gib das Passwort ein.", "Please enter the password.";
    PickVaultTitle => "Tresor auswählen", "Choose a vault";
    LastUsed => "zuletzt verwendet", "last used";
    VaultHintNotFound => "Kein Tresor namens „{name}“ gefunden.", "No vault named “{name}” found.";
    CreateVaultTitle => "Neuen Tresor erstellen", "Create a new vault";
    NoVaultYet => "Noch kein Tresor vorhanden – lege jetzt einen an.", "No vault yet – create one now.";
    VaultName => "Name des Tresors", "Vault name";
    DefaultVaultName => "Privat", "Personal";
    ConfirmMaster => "Wiederholen", "Repeat";
    TooShort => "Das Master-Passwort braucht mindestens {min} Zeichen.", "The master password needs at least {min} characters.";
    TooWeak => "Zu leicht zu erraten – nimm z. B. mehrere zufällige Wörter.", "Too easy to guess – try several random words.";
    Mismatch => "Die Passwörter stimmen nicht überein.", "The passwords do not match.";
    Creating => "Tresor wird erstellt …", "Creating vault …";
    RememberMaster => "Merke dir dein Master-Passwort gut: Es wird nirgends gespeichert und kann nicht zurückgesetzt werden.", "Remember your master password: it is stored nowhere and cannot be reset.";
    StrengthLabel => "Stärke", "Strength";
    Strength0 => "Sehr schwach", "Very weak";
    Strength1 => "Schwach", "Weak";
    Strength2 => "Mittel", "Fair";
    Strength3 => "Stark", "Strong";
    Strength4 => "Sehr stark", "Very strong";
    // ---- item form --------------------------------------------------------
    NewLoginTitle => "Neues Login", "New login";
    EditTitle => "„{name}“ bearbeiten", "Edit “{name}”";
    Save => "Speichern", "Save";
    Cancel => "Abbrechen", "Cancel";
    DiscardChanges => "Ungespeicherte Änderungen verwerfen?", "Discard unsaved changes?";
    InvalidMonth => "Monat: bitte 1 bis 12 eingeben.", "Month: please enter 1 to 12.";
    InvalidYear => "Jahr: bitte vierstellig eingeben, z. B. 2028.", "Year: please enter four digits, e.g. 2028.";
    TotpHint => "Base32-Schlüssel oder otpauth://-Link", "Base32 key or otpauth:// link";
    // ---- trash confirmation -------------------------------------------------
    ConfirmTrash => "„{name}“ in den Papierkorb verschieben?", "Move “{name}” to the trash?";
    ConfirmTitle => "Bestätigen", "Confirm";
    // ---- generator ----------------------------------------------------------
    GeneratorTitle => "Passwort-Generator", "Password generator";
    GenMode => "Modus", "Mode";
    GenModePassword => "Passwort", "Password";
    GenModePassphrase => "Passphrase", "Passphrase";
    GenLength => "Länge", "Length";
    GenUppercase => "Großbuchstaben (A-Z)", "Uppercase (A-Z)";
    GenLowercase => "Kleinbuchstaben (a-z)", "Lowercase (a-z)";
    GenDigits => "Ziffern (0-9)", "Digits (0-9)";
    GenSymbols => "Sonderzeichen (!#$%&*)", "Symbols (!#$%&*)";
    GenAvoidAmbiguous => "Ähnliche Zeichen meiden (Il1O0)", "Avoid look-alikes (Il1O0)";
    GenWords => "Wörter", "Words";
    GenSeparator => "Trennzeichen", "Separator";
    GenCapitalize => "Großschreibung", "Capitalize";
    GenIncludeNumber => "Zahl ergänzen", "Include a number";
    GenSeparatorSpace => "Leerzeichen", "space";
    // ---- help -----------------------------------------------------------------
    HelpTitle => "Tastenkürzel", "Keyboard shortcuts";
    HelpNavigate => "Element auswählen (seitenweise, Anfang, Ende)", "Select item (by page, first, last)";
    HelpEnter => "Details öffnen (Pfeile scrollen)", "Focus details (arrows scroll)";
    HelpSearch => "Suchen (oder einfach lostippen)", "Search (or just start typing)";
    HelpEsc => "Suche leeren / zurück / beenden", "Clear search / back / quit";
    HelpCopyUser => "Benutzernamen kopieren", "Copy username";
    HelpCopyPassword => "Passwort kopieren", "Copy password";
    HelpCopyTotp => "Einmalcode (2FA) kopieren", "Copy one-time code (2FA)";
    HelpOpen => "Website im Browser öffnen", "Open website in browser";
    HelpShow => "Geheime Felder anzeigen/verbergen", "Show/hide secrets";
    HelpNew => "Neues Login", "New login";
    HelpEdit => "Element bearbeiten", "Edit item";
    HelpTrash => "In den Papierkorb verschieben", "Move to trash";
    HelpGenerator => "Passwort-Generator", "Password generator";
    HelpLock => "Tresor sperren", "Lock vault";
    HelpQuit => "Beenden", "Quit";
    HelpClipboard => "Kopierte Geheimnisse werden nach {s} s aus der Zwischenablage gelöscht, spätestens beim Sperren/Beenden.", "Copied secrets are removed from the clipboard after {s}s, at the latest when locking/quitting.";
    HelpClipboardNever => "Kopierte Geheimnisse werden beim Sperren/Beenden aus der Zwischenablage gelöscht.", "Copied secrets are removed from the clipboard when locking/quitting.";
    HelpAutoLock => "Automatische Sperre nach {m} min ohne Eingabe.", "Locks automatically after {m} min without input.";
    HelpClose => "Beliebige Taste schließt diese Hilfe.", "Press any key to close this help.";
    // ---- errors ---------------------------------------------------------------
    ErrConflict => "Der Tresor wurde an anderer Stelle geändert. Bitte versuche es noch einmal.", "The vault was changed elsewhere. Please try again.";
    ErrNotFound => "Nicht gefunden.", "Not found.";
    ErrInvalidInput => "Ungültige Eingabe: {detail}", "Invalid input: {detail}";
    ErrIo => "Datei konnte nicht gelesen oder geschrieben werden: {detail}", "Could not read or write a file: {detail}";
    ErrCorrupt => "Die Datei ist beschädigt oder kein gültiger Tresor: {detail}", "The file is damaged or not a valid vault: {detail}";
    ErrUnsupported => "Nicht unterstützt: {detail}", "Not supported: {detail}";
    ErrNameRequired => "Bitte gib einen Namen ein.", "Please enter a name.";
    ErrNameTooLong => "Der Name ist zu lang (max. 200 Zeichen).", "The name is too long (max. 200 characters).";
    ErrPasswordEmpty => "Das Passwort darf nicht leer sein.", "The password must not be empty.";
    ErrLength => "Die Passwortlänge muss zwischen 5 und 128 liegen.", "The password length must be between 5 and 128.";
    ErrWords => "Die Passphrase braucht 3 bis 20 Wörter.", "A passphrase needs 3 to 20 words.";
    ErrSeparator => "Das Trennzeichen ist ungültig.", "The separator is invalid.";
    ErrNoCharset => "Wähle mindestens eine Zeichengruppe aus.", "Select at least one character set.";
    ErrMinimums => "Die Mindestanzahl an Ziffern und Sonderzeichen ist größer als die Länge.", "The minimum number of digits and symbols exceeds the length.";
    ErrTotp => "Der 2FA-Schlüssel ist ungültig.", "The 2FA key is invalid.";
    ErrTotpUri => "Der otpauth://-Link ist ungültig.", "The otpauth:// link is invalid.";
    // ---- command line -----------------------------------------------------------
    CliAbout => "VaultX – lokaler Passwort-Tresor im Terminal. Ohne Befehl startet die interaktive Oberfläche.", "VaultX – local password vault in the terminal. Without a command the interactive UI starts.";
    CliAfterHelp => "Umgebungsvariablen:\n  VAULTX_DATA_DIR          Datenordner (Standard: Ordner der VaultX-App)\n  VAULTX_MASTER_PASSWORD   Master-Passwort für Skripte. UNSICHER: andere Programme\n                           und die Shell-Historie können es sehen – nur verwenden,\n                           wenn es nicht anders geht.\n\nExit-Codes: 0 = Erfolg, 1 = Fehler, 2 = falscher Aufruf", "Environment variables:\n  VAULTX_DATA_DIR          data folder (default: the VaultX app's folder)\n  VAULTX_MASTER_PASSWORD   master password for scripts. INSECURE: other programs\n                           and the shell history may see it – use only if there\n                           is no other way.\n\nExit codes: 0 = success, 1 = error, 2 = invalid usage";
    CliUsage => "Aufruf", "Usage";
    CliCommands => "Befehle", "Commands";
    CliOptions => "Optionen", "Options";
    CliArguments => "Argumente", "Arguments";
    CliHelp => "Hilfe anzeigen", "Show help";
    CliVersion => "Version anzeigen", "Show version";
    CliVaultArg => "Tresor (Name oder ID)", "Vault (name or id)";
    CliList => "Elemente eines Tresors auflisten", "List the items of a vault";
    CliGet => "Ein Feld eines Elements ausgeben oder kopieren", "Print or copy a field of an item";
    CliGetQuery => "Name, Teil des Namens oder ID des Elements", "Name, part of the name or id of the item";
    CliGetField => "Feld: password (Standard), username, totp, notes, uri", "Field: password (default), username, totp, notes, uri";
    CliCopy => "In die Zwischenablage kopieren statt ausgeben", "Copy to the clipboard instead of printing";
    CliGenerate => "Passwort oder Passphrase erzeugen", "Generate a password or passphrase";
    CliGenLength => "Passwortlänge (5–128, Standard 20)", "Password length (5–128, default 20)";
    CliGenPassphrase => "Passphrase aus Wörtern statt Zeichen", "Passphrase of words instead of characters";
    CliGenWords => "Anzahl Wörter der Passphrase (3–20, Standard 5)", "Number of passphrase words (3–20, default 5)";
    CliGenNoSymbols => "Keine Sonderzeichen verwenden", "Do not use symbols";
    CliVaults => "Alle Tresore auflisten", "List all vaults";
    CliNoVaults => "Kein Tresor gefunden in {dir}. Lege zuerst einen an (VaultX-App oder vaultx-cli ohne Befehl).", "No vault found in {dir}. Create one first (VaultX app or vaultx-cli without a command).";
    CliVaultNotFound => "Kein Tresor namens „{name}“ gefunden. Verfügbar: {list}", "No vault named “{name}” found. Available: {list}";
    CliVaultAmbiguous => "Mehrere Tresore heißen „{name}“ – bitte die ID angeben.", "Several vaults are named “{name}” – please use the id.";
    CliChooseVault => "Mehrere Tresore vorhanden – wähle einen mit --vault NAME: {list}", "Several vaults exist – choose one with --vault NAME: {list}";
    CliPasswordPrompt => "Master-Passwort für „{name}“: ", "Master password for “{name}”: ";
    CliNoTty => "Kein Terminal für die Passworteingabe verfügbar ({err}). Für Skripte kann VAULTX_MASTER_PASSWORD gesetzt werden.", "No terminal available for the password prompt ({err}). Scripts may set VAULTX_MASTER_PASSWORD.";
    CliItemNotFound => "Kein Element gefunden für „{query}“.", "No item found for “{query}”.";
    CliItemAmbiguous => "Mehrere Elemente passen zu „{query}“ – bitte genauer angeben oder die ID verwenden:", "Several items match “{query}” – please be more specific or use the id:";
    CliFieldEmpty => "„{name}“ hat keinen Wert für „{field}“.", "“{name}” has no value for “{field}”.";
    CliCopiedWait => "{what} kopiert. Die Zwischenablage wird in {s} s geleert – Taste drücken, um sie sofort zu leeren.", "{what} copied. The clipboard will be cleared in {s}s – press any key to clear it now.";
    CliCountdown => "Zwischenablage wird in {s} s geleert …", "Clearing the clipboard in {s}s …";
    CliCleared => "Zwischenablage geleert.", "Clipboard cleared.";
    CliHoldClipboard => "{what} kopiert. Taste drücken zum Beenden – der Inhalt bleibt nur so lange verfügbar, wie dieses Programm läuft.", "{what} copied. Press any key to quit – the content stays available only while this program runs.";
    CliCopiedDone => "{what} kopiert.", "{what} copied.";
    CliEmptyVault => "Der Tresor „{name}“ enthält keine Elemente.", "The vault “{name}” contains no items.";
    CliNeedsTerminal => "Die interaktive Oberfläche braucht ein Terminal. Für Skripte gibt es Befehle wie „list“ und „get“ (siehe --help).", "The interactive UI needs a terminal. For scripts use commands such as “list” and “get” (see --help).";
    CliTerminalError => "Das Terminal konnte nicht eingerichtet werden: {err}", "Could not set up the terminal: {err}";
    CliError => "Fehler", "Error";
    ColName => "NAME", "NAME";
    ColType => "TYP", "TYPE";
    ColUser => "BENUTZER / INFO", "USER / INFO";
    ColWebsite => "WEBSITE", "WEBSITE";
    ColId => "ID", "ID";
    FieldNameUri => "Website", "website";
}

impl Lang {
    /// The text of `m` in this language.
    pub fn t(self, m: M) -> &'static str {
        match self {
            Lang::De => m.de(),
            Lang::En => m.en(),
        }
    }

    /// The text of `m` with `{key}` placeholders replaced.
    pub fn tf(self, m: M, args: &[(&str, &str)]) -> String {
        let mut s = self.t(m).to_owned();
        for (key, value) in args {
            s = s.replace(&format!("{{{key}}}"), value);
        }
        s
    }

    /// `s` in typographic quotes („…“ / “…”).
    pub fn quote(self, s: &str) -> String {
        match self {
            Lang::De => format!("„{s}“"),
            Lang::En => format!("“{s}”"),
        }
    }

    /// "1 Element" / "n Elemente".
    pub fn item_count(self, n: usize) -> String {
        if n == 1 {
            self.t(M::ItemCountOne).to_owned()
        } else {
            self.tf(M::ItemCount, &[("n", &n.to_string())])
        }
    }

    /// Strength label for a zxcvbn score 0..=4.
    pub fn strength(self, score: u8) -> &'static str {
        self.t(match score {
            0 => M::Strength0,
            1 => M::Strength1,
            2 => M::Strength2,
            3 => M::Strength3,
            _ => M::Strength4,
        })
    }

    /// Relative time like "vor 3 Tagen" for a timestamp in Unix ms.
    pub fn relative_time(self, then_ms: i64, now_ms: i64) -> String {
        let secs = (now_ms.saturating_sub(then_ms) / 1000).max(0);
        let n = |v: i64| v.to_string();
        let minutes = secs / 60;
        let hours = minutes / 60;
        let days = hours / 24;
        if minutes < 1 {
            self.t(M::JustNow).to_owned()
        } else if hours < 1 {
            self.tf(M::MinutesAgo, &[("n", &n(minutes))])
        } else if days < 1 {
            self.tf(M::HoursAgo, &[("n", &n(hours))])
        } else if days < 31 {
            if days == 1 {
                self.t(M::DaysAgoOne).to_owned()
            } else {
                self.tf(M::DaysAgo, &[("n", &n(days))])
            }
        } else if days < 365 {
            let months = (days / 30).max(1);
            if months == 1 {
                self.t(M::MonthsAgoOne).to_owned()
            } else {
                self.tf(M::MonthsAgo, &[("n", &n(months))])
            }
        } else {
            let years = days / 365;
            if years == 1 {
                self.t(M::YearsAgoOne).to_owned()
            } else {
                self.tf(M::YearsAgo, &[("n", &n(years))])
            }
        }
    }

    /// User-facing text for a core error. Never contains secrets (core
    /// errors never do).
    pub fn error(self, err: &Error) -> String {
        match err {
            Error::WrongPassword => self.t(M::WrongPassword).to_owned(),
            Error::Conflict => self.t(M::ErrConflict).to_owned(),
            Error::NotFound(_) => self.t(M::ErrNotFound).to_owned(),
            Error::InvalidInput(detail) => {
                let known = match detail.as_str() {
                    "name_required" => Some(M::ErrNameRequired),
                    "name_too_long" => Some(M::ErrNameTooLong),
                    "password_empty" | "password_required" => Some(M::ErrPasswordEmpty),
                    "length" => Some(M::ErrLength),
                    "words" => Some(M::ErrWords),
                    "separator" => Some(M::ErrSeparator),
                    "no_character_set" => Some(M::ErrNoCharset),
                    "minimums_exceed_length" => Some(M::ErrMinimums),
                    "totp_secret" | "totp_secret_empty" | "totp_digits" | "totp_period" => {
                        Some(M::ErrTotp)
                    }
                    "totp_uri" => Some(M::ErrTotpUri),
                    _ => None,
                };
                match known {
                    Some(m) => self.t(m).to_owned(),
                    None => self.tf(M::ErrInvalidInput, &[("detail", detail)]),
                }
            }
            Error::Io(e) => self.tf(M::ErrIo, &[("detail", &e.to_string())]),
            Error::Json(e) => self.tf(M::ErrCorrupt, &[("detail", &e.to_string())]),
            Error::Corrupt(d) => self.tf(M::ErrCorrupt, &[("detail", d)]),
            Error::Unsupported(d) => self.tf(M::ErrUnsupported, &[("detail", d)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut out: Vec<String> = s
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(k, _)| k.to_owned()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn every_message_is_translated_with_the_same_placeholders() {
        for &m in M::ALL {
            let (de, en) = (Lang::De.t(m), Lang::En.t(m));
            assert!(!de.is_empty() && !en.is_empty(), "{m:?} is empty");
            assert_eq!(placeholders(de), placeholders(en), "{m:?} placeholders");
        }
    }

    #[test]
    fn formatting_and_plurals() {
        assert_eq!(
            Lang::De.tf(M::Copied, &[("what", "Passwort")]),
            "Passwort kopiert"
        );
        assert_eq!(Lang::De.item_count(1), "1 Element");
        assert_eq!(Lang::En.item_count(3), "3 items");
        assert_eq!(Lang::En.strength(9), "Very strong");
    }

    #[test]
    fn relative_times() {
        let now = 1_000_000_000_000;
        assert_eq!(Lang::De.relative_time(now - 10_000, now), "gerade eben");
        assert_eq!(Lang::De.relative_time(now - 5 * 60_000, now), "vor 5 min");
        assert_eq!(Lang::En.relative_time(now - 3 * 3_600_000, now), "3 h ago");
        assert_eq!(Lang::De.relative_time(now - 86_400_000, now), "vor 1 Tag");
        assert_eq!(
            Lang::De.relative_time(now - 400 * 86_400_000, now),
            "vor 1 Jahr"
        );
        // Clock skew: a timestamp in the future is "just now".
        assert_eq!(Lang::En.relative_time(now + 50_000, now), "just now");
    }

    #[test]
    fn error_texts() {
        assert_eq!(
            Lang::De.error(&Error::InvalidInput("name_required".into())),
            "Bitte gib einen Namen ein."
        );
        assert!(Lang::En
            .error(&Error::InvalidInput("weird".into()))
            .contains("weird"));
        assert_eq!(
            Lang::En.error(&Error::WrongPassword),
            "Wrong master password. Please try again."
        );
    }
}
