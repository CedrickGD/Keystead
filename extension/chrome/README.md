# Keystead browser extension (Chrome, Edge, Brave, Chromium, Vivaldi)

Manifest V3, plain JavaScript, no build step. It fills logins from the Keystead
desktop app on this computer via Native Messaging (host `com.keystead.bridge`) –
nothing leaves the machine.

## Install

1. Start the Keystead desktop app → *Einstellungen → Browser-Integration* →
   click *Aktivieren* next to your browser (registers the native host).
2. Open `chrome://extensions` (Edge: `edge://extensions`), enable developer
   mode, *Load unpacked* → select this folder. The `key` in `manifest.json`
   pins the extension ID to `imfndemblnaalppnmdplagajjielnaok`.
3. Click the toolbar icon → *Mit Keystead verbinden* and confirm the 6-digit
   code in the app.

Shortcuts (change them at `chrome://extensions/shortcuts`): `Ctrl+Shift+Y`
popup, `Ctrl+Shift+L` fill the login for the current page, `Ctrl+Shift+9`
generate a password into the focused field. Right-click on an input field:
*Passwort generieren und einfügen*, *Keystead öffnen*.

## Files

| File | Role |
|---|---|
| `background.js` | Service worker: native port (`lib/bridge.js`), status cache, pairing, badge, context menu, commands, credential release to content scripts, save/update prompts. |
| `lib/bridge.js` | Native messaging client: lazy port, reconnect, per-request timeouts (10 s; 12 s while the host may still launch the app; `pair` 125 s on its own port). |
| `lib/store.js` | `chrome.storage` state (MV3 workers are stopped when idle). `local`: pairing credentials, "never save" sites, last used login per site, generator options. `session`: status, pairing progress, pending save prompts. |
| `content.js` | Inline icon + dropdown, autofill, capture of submitted logins, save bar – UI in a closed shadow root. |
| `lib/forms.js` | Pure DOM helpers for form detection and filling (`globalThis.KeysteadForms`); loadable on its own in a test page. |
| `popup.html/.css/.js` | Popup (setup guide, pairing, unlock, *Diese Seite* / *Suche* / *Generator*, add login). `popup.html?demo=<state>` renders fake data only (no storage, no native host) for design work and screenshots; states: `host_missing`, `app_unavailable`, `not_paired`, `pairing`, `paired`, `denied`, `locked`, `unlocked`, `unlocked_empty`, `not_web`, `insecure`. |
| `_locales/de`, `_locales/en` | All UI strings. |

## Security model

* Only the service worker talks to the app. Content scripts get secrets only
  through `cs:fill-request`, and only for a login the app matches against the
  URL of *that* frame as reported by the browser (`sender.url`) – never a URL
  supplied by the page or the content script.
* Fills happen only after a user gesture: a trusted click in our dropdown, a
  key in the dropdown, the popup, or a keyboard command. The autofill
  command/popup fills the top frame and same-origin frames only.
* Clickjacking: the inline icon, dropdown items, Enter in the dropdown and the
  buttons of the save/update bar only act if that control has been really
  visible for at least 500 ms – IntersectionObserver v2 (`trackVisibility`)
  reports it as unoccluded and free of opacity/filter/transform effects
  (a page's `pointer-events: none` cover in the top layer counts as
  occluding) – and, for clicks, our host is the element at the click point
  with no opacity, filter, clip-path or mask. Without IntersectionObserver v2
  these controls do nothing (fill from the popup instead).
* Generated passwords go only into the field that currently has the focus:
  synthetic `focusin` events are ignored, a frame re-reports its field after
  losing the focus, and a frame without a focused field fills nothing.
* Save/update prompts only for a password the user typed or pasted (trusted
  `input`) or inserted from the generator, and that no script changed since;
  the comparison with the stored password is done by the app
  (`check_login_password`, no secret is returned, no auto-lock activity). An
  *update* prompt is only shown on the origin it was captured on and names
  the host.
* Logins stored for `https` are never filled into `http` pages; invisible,
  disabled and read-only fields are never filled.
* Captured passwords for the save prompt wait in `chrome.storage.session`
  (memory only, not readable by content scripts) for at most 2 minutes.
* Popup-only messages (search, copy password, unlock, …) are rejected when
  they come from a content script. Secrets are never logged.
* Copying a password or TOTP code (and the generator's copy button) is done
  by the desktop app (`copy_field` / `copy_secret`): excluded from clipboard
  history and cleared after the app's clipboard timeout and on lock. Stored
  passwords never reach the popup; only usernames are copied by the popup.
