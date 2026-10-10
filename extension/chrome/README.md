# Keystead browser extension (Chrome, Edge, Brave, Chromium, Vivaldi)

Manifest V3, plain JavaScript, no build step. It fills logins from the Keystead
desktop app on this computer via Native Messaging (host `com.keystead.bridge`) –
nothing leaves the machine.

## Install

1. Start the Keystead desktop app → *Einstellungen → Browser-Integration* →
   click *Aktivieren* next to your browser (registers the native host).
2. Open `chrome://extensions` (Edge: `edge://extensions`), enable developer
   mode, *Load unpacked* → select the folder the app shows in step 2
   (`<data dir>/browser-extension`, e.g. `%LOCALAPPDATA%\Keystead\browser-extension`;
   buttons *Ordner öffnen* / *Pfad kopieren*). The app embeds this extension and
   writes it there on start. The `key` in `manifest.json` pins the extension ID
   to `imfndemblnaalppnmdplagajjielnaok`.
3. Click the toolbar icon → *Mit Keystead verbinden* and confirm the 6-digit
   code in the app.

Updates: the app reports the version it delivers (`extensionVersion`) and the
folder (`extensionDir`) in `status`. When that version is newer than the running
one, the service worker reloads the extension once for that version
(`chrome.runtime.reload()`, remembered in `chrome.storage.local`
`extensionReloadedFor`, no loops). If it is still older afterwards – loaded from
another folder, e.g. an unzipped download – the popup shows *Neue
Plugin-Version verfügbar* with the app's folder (copy button) and how to load it
from there once. `popup.html?demo=<state>&ext=notice|reloading` shows the notice.

Shortcuts (change them at `chrome://extensions/shortcuts`): `Ctrl+Shift+Y`
popup, `Ctrl+Shift+L` fill the login for the current page, `Ctrl+Shift+9`
generate a password into the focused field. Right-click on an input field:
*Passwort generieren und einfügen*, *Keystead öffnen*.

## Smart filling

* **Password suggestions** – signup and password change forms are told apart
  from logins: a password field asks for a *new* password if it has
  `autocomplete="new-password"`, or name/id/label words like new, confirm,
  repeat, neu, wiederholen, bestätigen, festlegen …; two password fields where
  the first does not ask for the current one are "password + confirmation";
  a single unmarked field counts as signup only if the form (action, buttons)
  or – without login words in the form – the page title/path reads like a
  registration, and a single `new-password` field in a form whose own
  buttons say "log in" (sites that mark login fields to keep browsers from
  filling them) stays a login. A user's focus (click, Tab: the browser's transient user
  activation; a page focusing the field by script on load gets nothing) on the
  first new-password field shows a bubble *Starkes Passwort vorschlagen*
  beside the field (below it if there is no room): the value masked (eye to
  reveal), *Verwenden* and ✕. The value is generated with the generator
  options of the popup (else the app's defaults: 20 characters, all classes)
  as a *preview* (`generate_password` with `remember: false`: not in the
  generator history, no auto-lock activity). *Verwenden* fills the field and
  every confirmation field, marks the value as the user's own input (the save
  bar after submitting offers exactly this password) and stores it in the
  generator history of the vault it was suggested for (`remember_generated`;
  apps without it already stored the preview). ✕, Escape or typing an own
  password closes it for that field; the field icon (Keystead mark with an
  amber sparkle) opens it again. Locked or not connected: no bubble on focus –
  a click on the field icon says *Keystead entsperren für Passwortvorschläge*
  (or *verbinden*) with a button that opens the popup. Never on login forms,
  username steps or search fields. Signup forms get no login icon (also not a
  username asked for after the password, as on GitHub) and never take a
  stored login: popup *Ausfüllen* / `Ctrl+Shift+L` fill the page's login form
  instead, else say that there is none – a stored password is never written
  into a field that asks for a new one.
* **Multi-step logins** – a lone e-mail/username field in a form or
  container whose words say log in/sign in/Konto/passwor… (e.g. Amazon's
  *E-Mail-Adresse oder Mobiltelefonnummer* + *Weiter*) is a username-only
  step: it gets the login icon, the username is remembered for the password
  page. "E-Mail-Adresse"/"email address" is no postal address (street, city,
  *Lieferadresse* … still rule a field out). A field inside a detected
  password form (a customer number beside the username, the username of a
  signup form) is never a separate step.
* **2FA codes** – after a login with a 2FA seed was filled (popup *Ausfüllen*,
  inline dropdown, `Ctrl+Shift+L`) the app copies the current code
  (`copy_field` `totp`: cleared after `clipboardClearSeconds`, excluded from
  clipboard history) and the page shows *2FA-Code kopiert – einfügen mit
  Strg+V (noch N s gültig)*. For 2 minutes a one-time code field of that site
  on this or the next page – `autocomplete="one-time-code"`, or otp/totp/2fa/
  mfa/code/token in name/id with a 6–8 character limit (postal, promo,
  voucher, TAN, PIN … excluded), or 6–8 single-digit boxes – offers
  *2FA-Code einfügen* beside it. Popup → Einstellungen: *2FA-Code nach dem
  Ausfüllen kopieren* (default on) and *Starke Passwörter vorschlagen*
  (default on), stored in `chrome.storage.local` `settings`.
* **Web components** – fields inside *open* shadow roots are detected and
  filled (scope, document order and "inside" checks follow the composed tree;
  a host's `name`/`label`/`autocomplete` count as hints for its input). An
  isolated world cannot hook `attachShadow`, so roots are found in added
  subtrees, in the composed path of focus/input events and by a throttled
  sweep (while the page is visible: every 3 s for the first 30 s – up to
  60 s while custom elements wait for their definition – then every 12 s;
  elements that are never defined, like Angular's `<app-root>`, do not keep
  it fast); known roots are observed and get their own `submit` listener.
  Values are written through the native setter with
  `composed` input/change events, so frameworks notice. Closed shadow roots are
  the component's private DOM: their fields are not detected.
* **Dwell feedback** – a click (or Enter) our UI rejects under the
  clickjacking rule below does nothing as before, but the control shakes and
  a tooltip says *Einen Moment …* (not visible for 500 ms yet) or *Verdeckt –
  Seite blockiert Keystead-Menü* (covered/faded by the page; our layer is
  raised again and waits anew). Dropdown items show a small progress ring,
  primary buttons and the 2FA pill a thin bar while the 500 ms run.
* **Popup** – logins with 2FA show a countdown ring of the current code on
  *Diese Seite* (click = copy); the code stays in the service worker
  (`popup:totp-timers` → `get_totp`, only period and seconds left reach the
  popup; counts as activity like any popup use). ↑/↓ select a login (the
  first is preselected), Enter fills it.

Several vaults: the locked popup shows a *Tresor* selector above the master
password (preselected: the vault last unlocked in this browser, else the
app's last used one). While unlocked, the vault name in the header opens a
menu of all vaults; picking one asks for its master password and switches
the app to it – the open vault stays unlocked until the new password was
accepted (*Zurück* returns to it).

## Files

| File | Role |
|---|---|
| `background.js` | Service worker: native port (`lib/bridge.js`), status cache, pairing, badge, context menu, commands, credential release to content scripts, save/update prompts. |
| `lib/bridge.js` | Native messaging client: lazy port, reconnect, per-request timeouts (10 s; 12 s while the host may still launch the app; `pair` 125 s on its own port). |
| `lib/version.js` | Extension version compare and the self-update decision (`extensionUpdateAction`); tested by `extension/tests/version.test.mjs` (`node --test extension/tests/*.test.mjs`). |
| `lib/pending.js` | Whether a save/update prompt still belongs to the open vault (`pendingVaultState`); tested by `extension/tests/pending.test.mjs`. |
| `lib/settings.js` | Extension settings and their defaults (`autoCopyTotp`, `suggestPasswords`); tested by `extension/tests/settings.test.mjs`. |
| `lib/store.js` | `chrome.storage` state (MV3 workers are stopped when idle). `local`: pairing credentials, "never save" sites, last used login per site, generator options, extension settings, the id of the vault last unlocked here (`chosenVaultId`, written by the popup), the app's extension version a self-update reload was tried for (`extensionReloadedFor`). `session`: status (incl. the open vault's name and id, the extension version and folder the app delivers), pairing progress, pending save prompts, 2FA offers (`totp:<tabId>`: item id, site, vault, expiry – no code). |
| `content.js` | Inline icons + dropdown, password suggestion bubble, 2FA toast and *2FA-Code einfügen*, dwell feedback, autofill, capture of submitted logins, save bar – UI in a closed shadow root; open shadow roots of the page are discovered and observed. |
| `lib/forms.js` | Pure DOM helpers (`globalThis.KeysteadForms`): login/signup/change classification (`passwordRole`, `suggestionTargets`), one-time code fields (`findOtpFields`, `fillOtp`), composed-tree helpers for open shadow roots, filling. Loadable on its own in a test page; tested by `extension/tests/forms.test.mjs` on a small fake DOM (`extension/tests/helpers/mini-dom.mjs`). |
| `popup.html/.css/.js` | Popup (setup guide, pairing, unlock, *Diese Seite* / *Suche* / *Generator*, add login). `popup.html?demo=<state>` renders fake data only (no storage, no native host) for design work and screenshots; states: `host_missing`, `app_unavailable`, `not_paired`, `pairing`, `paired`, `denied`, `locked`, `locked_single`, `unlocked`, `unlocked_single`, `unlocked_empty`, `not_web`, `insecure` (`_single`: one vault, else three; password `demo`). |
| `_locales/de`, `_locales/en` | All UI strings. |

## Security model

* Only the service worker talks to the app. Content scripts get secrets only
  through `cs:fill-request`, and only for a login the app matches against the
  URL of *that* frame as reported by the browser (`sender.url`) – never a URL
  supplied by the page or the content script.
* Fills happen only after a user gesture: a trusted click in our dropdown, a
  key in the dropdown, the popup, or a keyboard command. The autofill
  command/popup fills the top frame and same-origin frames only.
* Clickjacking: the inline icons, dropdown items, Enter in the dropdown, the
  *Verwenden* button of a suggestion, *2FA-Code einfügen* and the buttons of
  the save/update bar only act if that control has been really
  visible for at least 500 ms – IntersectionObserver v2 (`trackVisibility`)
  reports it as unoccluded and free of opacity/filter/transform effects
  (a page's `pointer-events: none` cover in the top layer counts as
  occluding) – and, for clicks, our host is the element at the click point
  with no opacity, filter, clip-path or mask. Without IntersectionObserver v2
  these controls do nothing (fill from the popup instead). A rejected click
  only gets visible feedback (see "Dwell feedback"); the rule is unchanged.
* Generated passwords go only into the field that currently has the focus:
  synthetic `focusin` events are ignored, a frame re-reports its field after
  losing the focus, and a frame without a focused field fills nothing.
* A suggested password reaches the page only through *Verwenden* (a trusted
  click after the dwell); before that it lives in the content script's
  isolated world and – masked unless revealed – in our closed shadow root.
  Previews are neither stored nor activity, so a page that keeps focusing its
  signup field neither fills the history nor keeps the vault unlocked.
* A 2FA offer holds no code: *2FA-Code einfügen* asks the service worker,
  which checks the offer (same tab, same site, same vault, ≤ 2 minutes, used
  once) and – like for credentials – that the login matches the URL of *that*
  frame (and the http/https rule) before it returns the current code.
* Save/update prompts only for a password the user typed or pasted (trusted
  `input`) or inserted from the generator or a suggestion, and that no script changed since;
  the comparison with the stored password is done by the app
  (`check_login_password`, no secret is returned, no auto-lock activity). An
  *update* prompt is only shown on the origin it was captured on and names
  the host.
* Logins stored for `https` are never filled into `http` pages; invisible,
  disabled and read-only fields are never filled.
* Captured passwords for the save prompt wait in `chrome.storage.session`
  (memory only, not readable by content scripts) for at most 2 minutes, and
  are dropped – like 2FA offers – as soon as the vault locks or another vault
  is opened.
* A prompt belongs to the vault it was computed against (`vaultId`, asked
  fresh from the app after the comparison; no prompt if the vault changed or
  locked meanwhile): after a vault switch it is neither shown again
  (`cs:pending`) nor saved (`cs:save-decision` asks the app's status first
  and answers `not_found`), so a login is never saved into, or an update
  aimed at, the other vault.
* Popup-only messages (search, copy password, unlock, …) are rejected when
  they come from a content script. Secrets are never logged.
* Copying a password or TOTP code (and the generator's copy button) is done
  by the desktop app (`copy_field` / `copy_secret`): excluded from clipboard
  history and cleared after the app's clipboard timeout and on lock. Stored
  passwords never reach the popup; only usernames are copied by the popup.
