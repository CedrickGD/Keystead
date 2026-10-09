/*
 * VaultX – login form detection and filling.
 *
 * Pure DOM helpers without any extension API, so they can be loaded on their
 * own in a test page. This is a classic script (manifest content scripts
 * cannot be ES modules): it defines `globalThis.VaultXForms`. In the extension
 * it runs in the content script's isolated world, before content.js.
 */
(() => {
  "use strict";

  /** Upper bound of inspected inputs per scan (protects huge pages). */
  const MAX_INPUTS = 800;
  /** Input types that can hold a username. */
  const USERNAME_TYPES = new Set(["text", "email", "tel"]);
  /** Input types a generated password may be inserted into. */
  const TEXT_ENTRY_TYPES = new Set(["text", "password", "email", "search", "tel", "url"]);

  /** Attribute hints that suggest a username / e-mail field. */
  const USERNAME_HINT = /user|login|logon|e-?mail|account|signin|benutzer|anmeld|kennung|identifi|nutzer|konto|nick|member|kunde|customer|uid\b/;
  /** Hints that rule a field out as username (other personal data). */
  const NOT_USERNAME_HINT = /first.?name|last.?name|full.?name|vorname|nachname|surname|street|stra(ss|ß)e|address|adresse|city|stadt|zip|postal|plz|birth|geburt|company|firma|amount|betrag|iban|card|karte/;
  /** Fields that are never login fields (search, captcha, one-time codes …). */
  const EXCLUDED_HINT = /search|suche|captcha|one.?time|\botp\b|totp|2fa|mfa|verification|verif|sms.?code|auth.?code|coupon|promo|gutschein|newsletter|subscribe|abonn/;
  /** Context that marks a lone e-mail/username field as the first step of a login. */
  const LOGIN_CONTEXT = /log\s*-?\s*in|logon|sign\s*-?\s*in|signin|anmeld|einlogg|auth|account|konto|identifier|session|sso|passwor/;
  /** Labels of buttons that submit a login (or signup / password change). */
  const SUBMIT_TEXT = /log\s*-?\s*in|logon|sign\s*-?\s*(in|on|up)|anmeld|einlogg|submit|absenden|senden|weiter|next|continue|fortfahren|best[äa]tig|confirm|registr|create|erstellen|speichern|save|[äa]ndern|change|update|aktualisier|verify|enter|\bok\b|\bgo\b/;

  /** Inputs seen as type=password; a "show password" toggle turns them into type=text. */
  const knownPasswordFields = new WeakSet();

  const inputValueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  const textAreaValueSetter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;

  // ---------------------------------------------------------------------------
  // Field classification
  // ---------------------------------------------------------------------------

  function isInput(el) {
    return !!el && el.nodeType === 1 && el.localName === "input";
  }

  function typeOf(el) {
    return String(el.type || "text").toLowerCase();
  }

  function autocompleteTokens(el) {
    return String(el.getAttribute("autocomplete") || "")
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean);
  }

  function hasToken(el, token) {
    return autocompleteTokens(el).includes(token);
  }

  /** name + id (the most reliable hints), lower-cased. */
  function idHints(el) {
    return `${el.getAttribute("name") || ""} ${el.id || ""}`.toLowerCase();
  }

  /** Visible descriptions: placeholder, aria-label, labels. */
  function labelHints(el) {
    const parts = [el.getAttribute("placeholder"), el.getAttribute("aria-label"), el.getAttribute("title")];
    try {
      for (const label of el.labels || []) parts.push(label.textContent);
    } catch {
      // `labels` throws on some exotic inputs; hints are best effort.
    }
    const labelledBy = el.getAttribute("aria-labelledby");
    if (labelledBy) {
      for (const id of labelledBy.split(/\s+/).slice(0, 4)) {
        parts.push(el.ownerDocument.getElementById(id)?.textContent);
      }
    }
    return parts
      .filter(Boolean)
      .map((p) => String(p).slice(0, 120))
      .join(" ")
      .toLowerCase();
  }

  /** True for password inputs (including ones a "show password" toggle switched to text). */
  function isPasswordInput(el) {
    if (!isInput(el)) return false;
    const type = typeOf(el);
    if (type === "password") {
      knownPasswordFields.add(el);
      return true;
    }
    return type === "text" && knownPasswordFields.has(el);
  }

  /** True for fields that must never be treated as login fields. */
  function isExcluded(el) {
    const tokens = autocompleteTokens(el);
    if (tokens.includes("one-time-code") || tokens.some((t) => t.startsWith("cc-"))) return true;
    if (String(el.getAttribute("name") || "").toLowerCase() === "q") return true;
    return EXCLUDED_HINT.test(idHints(el)) || EXCLUDED_HINT.test(labelHints(el));
  }

  /**
   * True if the element is rendered and visible to the user: has a size, is
   * not hidden via CSS/opacity and is not moved off the page (honeypots).
   */
  function isVisible(el) {
    if (!el || el.nodeType !== 1 || !el.isConnected) return false;
    const view = el.ownerDocument.defaultView;
    if (!view) return false;
    const rect = el.getBoundingClientRect();
    if (rect.width < 5 || rect.height < 5) return false;
    if (typeof el.checkVisibility === "function") {
      const visible = el.checkVisibility({
        checkOpacity: true,
        checkVisibilityCSS: true,
        opacityProperty: true,
        visibilityProperty: true,
      });
      if (!visible) return false;
    } else {
      for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
        const style = view.getComputedStyle(node);
        if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) {
          return false;
        }
      }
    }
    const root = el.ownerDocument.documentElement;
    const left = rect.left + view.scrollX;
    const top = rect.top + view.scrollY;
    const width = Math.max(root.scrollWidth, view.innerWidth);
    const height = Math.max(root.scrollHeight, view.innerHeight);
    return left + rect.width > 0 && top + rect.height > 0 && left < width && top < height;
  }

  /** Visible, enabled and editable – i.e. it is safe to write into it now. */
  function isFillableNow(el) {
    return !!el && el.isConnected && !el.disabled && !el.readOnly && isVisible(el);
  }

  /** Inputs and textareas a (generated) password can be typed into. */
  function isTextEntry(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.localName === "textarea") return true;
    return isInput(el) && (TEXT_ENTRY_TYPES.has(typeOf(el)) || isPasswordInput(el));
  }

  function isPasswordCandidate(el) {
    return isPasswordInput(el) && !el.disabled && !el.readOnly && !hasToken(el, "one-time-code") && isVisible(el);
  }

  function isUsernameCandidate(el) {
    return (
      isInput(el) &&
      USERNAME_TYPES.has(typeOf(el)) &&
      !isPasswordInput(el) &&
      !el.disabled &&
      !el.readOnly &&
      !isExcluded(el) &&
      isVisible(el)
    );
  }

  /** How strongly a field looks like a username/e-mail login field. */
  function usernameScore(el) {
    const tokens = autocompleteTokens(el);
    if (tokens.includes("username")) return 100;
    let score = 0;
    if (tokens.includes("email")) score += 60;
    if (typeOf(el) === "email") score += 40;
    const ids = idHints(el);
    const labels = labelHints(el);
    if (USERNAME_HINT.test(ids)) score += 35;
    else if (USERNAME_HINT.test(labels)) score += 25;
    if (NOT_USERNAME_HINT.test(ids) || NOT_USERNAME_HINT.test(labels)) score -= 60;
    return score;
  }

  // ---------------------------------------------------------------------------
  // Form discovery
  // ---------------------------------------------------------------------------

  /** True if `a` comes before `b` in document order. */
  function precedes(a, b) {
    return !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
  }

  function hasButton(node) {
    return !!node.querySelector('button, input[type="submit"], input[type="image"], [role="button"]');
  }

  /**
   * The element that groups a field with its siblings: its <form>, or the
   * nearest ancestor that also contains another login-ish field (div-based
   * SPA forms), or the nearest ancestor with a button.
   */
  function scopeFor(el) {
    if (el.form) return el.form;
    const doc = el.ownerDocument;
    let withButton = null;
    let node = el.parentElement;
    for (let depth = 0; node && node !== doc.body && node !== doc.documentElement && depth < 8; depth += 1) {
      const inputs = node.querySelectorAll("input");
      // A container with dozens of inputs is a whole page section, not a login form.
      if (inputs.length > 40) break;
      for (const other of inputs) {
        if (other !== el && (isPasswordCandidate(other) || isUsernameCandidate(other))) return node;
      }
      if (!withButton && hasButton(node)) withButton = node;
      node = node.parentElement;
    }
    return withButton || el.parentElement || doc.body || doc.documentElement;
  }

  /** The username field belonging to a password field: the nearest suitable preceding field. */
  function usernameFor(password, scope) {
    const candidates = [];
    for (const input of scope.querySelectorAll("input")) {
      if (input !== password && precedes(input, password) && isUsernameCandidate(input)) candidates.push(input);
    }
    if (!candidates.length) return null;
    const explicit = candidates.filter((c) => usernameScore(c) >= 100);
    if (explicit.length) return explicit[explicit.length - 1];
    const plausible = candidates.filter((c) => usernameScore(c) > -30);
    return plausible.length ? plausible[plausible.length - 1] : null;
  }

  function scopeContext(scope) {
    const doc = scope.ownerDocument;
    const parts = [
      scope.getAttribute?.("action"),
      scope.id,
      typeof scope.className === "string" ? scope.className : "",
      scope.getAttribute?.("aria-label"),
      scope.getAttribute?.("name"),
      doc.title,
      doc.location ? doc.location.pathname : "",
    ];
    let buttons = 0;
    for (const btn of scope.querySelectorAll('button, input[type="submit"], [role="button"]')) {
      parts.push(btn.textContent || btn.value || btn.getAttribute("aria-label"));
      buttons += 1;
      if (buttons >= 6) break;
    }
    return parts
      .filter(Boolean)
      .map((p) => String(p).slice(0, 160))
      .join(" ")
      .toLowerCase();
  }

  /** The container of a lone username field: its form or the nearest ancestor with a button. */
  function stepScopeFor(el) {
    if (el.form) return el.form;
    const doc = el.ownerDocument;
    let node = el.parentElement;
    for (let depth = 0; node && node !== doc.body && depth < 6; depth += 1) {
      if (hasButton(node)) return node;
      node = node.parentElement;
    }
    return el.parentElement || doc.body;
  }

  /** True for the username/e-mail field of a username-only login step. */
  function isUsernameStepField(el) {
    if (!isUsernameCandidate(el)) return false;
    const score = usernameScore(el);
    if (score < 35) return false;
    const scope = stepScopeFor(el);
    let textFields = 0;
    for (const input of scope.querySelectorAll("input")) {
      if (USERNAME_TYPES.has(typeOf(input)) && !isPasswordInput(input) && isVisible(input)) textFields += 1;
    }
    // A login step asks for one identifier (rarely a second field, e.g. a tenant).
    if (textFields > 2) return false;
    if (score >= 100) return true;
    return LOGIN_CONTEXT.test(scopeContext(scope));
  }

  function classify(passwords) {
    const current = passwords.filter((p) => !hasToken(p, "new-password"));
    const fresh = passwords.filter((p) => hasToken(p, "new-password"));
    if (passwords.length === 1) return fresh.length ? "signup" : "login";
    if (passwords.length >= 3) return "change";
    if (current.length === 1 && fresh.length >= 1 && passwords[0] === current[0]) return "change";
    return "signup";
  }

  /**
   * Finds login-related forms in a document.
   *
   * Returns `LoginForm[]` with `{ scope, kind, username, password, passwords }`:
   * `kind` is "login" (one password field), "change" (current + new password),
   * "signup" (new password + confirmation) or "username" (username-only step
   * of a multi-step login); `password` is the field for the *current*
   * password (null for signup/username steps); `passwords` are all visible
   * password fields of the form in document order.
   */
  function findLoginForms(doc = document) {
    const inputs = Array.from(doc.querySelectorAll("input")).slice(0, MAX_INPUTS);
    const groups = new Map();
    for (const input of inputs) {
      if (!isPasswordCandidate(input)) continue;
      const scope = scopeFor(input);
      const group = groups.get(scope);
      if (group) group.push(input);
      else groups.set(scope, [input]);
    }

    const forms = [];
    const used = new Set();
    for (const [scope, passwords] of groups) {
      const kind = classify(passwords);
      const username = usernameFor(passwords[0], scope);
      const password = kind === "login" || kind === "change" ? passwords[0] : null;
      forms.push({ scope, kind, username, password, passwords });
      if (username) used.add(username);
    }

    for (const input of inputs) {
      if (used.has(input) || !isUsernameStepField(input)) continue;
      forms.push({ scope: stepScopeFor(input), kind: "username", username: input, password: null, passwords: [] });
      used.add(input);
    }
    return forms;
  }

  /** The form an element belongs to (as a field, or inside its scope). */
  function formForElement(forms, el) {
    if (!el) return null;
    return (
      forms.find((f) => f.username === el || f.passwords.includes(el)) ||
      forms.find((f) => f.scope === el || f.scope.contains(el) || el.contains(f.scope)) ||
      null
    );
  }

  /** Fields that get the inline VaultX icon (signup forms get none). */
  function iconFields(forms) {
    const fields = [];
    for (const form of forms) {
      if (form.kind === "signup") continue;
      if (form.username) fields.push(form.username);
      if (form.password) fields.push(form.password);
    }
    return fields;
  }

  /** Forms ordered by how likely they are the one to autofill. */
  function rankForms(forms, activeElement) {
    const priority = { login: 0, change: 1, username: 2, signup: 3 };
    const active = formForElement(forms, activeElement);
    return forms
      .filter((f) => (f.username && isFillableNow(f.username)) || f.passwords.some(isFillableNow))
      .sort((a, b) => (b === active) - (a === active) || priority[a.kind] - priority[b.kind]);
  }

  // ---------------------------------------------------------------------------
  // Filling
  // ---------------------------------------------------------------------------

  /**
   * Writes a value like a user would: through the native value setter (so
   * framework value trackers like React's notice the change) followed by
   * input/change/keyup events.
   */
  function setValue(el, value) {
    try {
      el.focus({ preventScroll: true });
    } catch {
      // Focus is cosmetic; the value is what matters.
    }
    const setter = el.localName === "textarea" ? textAreaValueSetter : inputValueSetter;
    if (setter) setter.call(el, value);
    else el.value = value;
    el.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, inputType: "insertReplacementText" }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    el.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, composed: true, key: "Unidentified" }));
    return el.value === value;
  }

  /**
   * Fills a login form with `{ username, password }`. Only writes into fields
   * that are visible and editable. Returns the number of filled fields.
   */
  function fillForm(form, credentials) {
    let filled = 0;
    const username = typeof credentials.username === "string" ? credentials.username : "";
    const password = typeof credentials.password === "string" ? credentials.password : "";
    if (form.username && username && isFillableNow(form.username)) {
      if (setValue(form.username, username)) filled += 1;
    }
    const passwordField = form.password || form.passwords[0] || null;
    if (passwordField && password && isFillableNow(passwordField)) {
      if (setValue(passwordField, password)) filled += 1;
    }
    return filled;
  }

  /**
   * Inserts a generated password into `target`; if it is a password field,
   * also into the directly following empty confirmation field of the same
   * form. Returns the number of filled fields.
   */
  function fillGenerated(target, password) {
    if (!isTextEntry(target) || !isFillableNow(target)) return 0;
    let filled = setValue(target, password) ? 1 : 0;
    if (filled && isPasswordInput(target)) {
      const scope = scopeFor(target);
      const confirm = Array.from(scope.querySelectorAll("input")).find(
        (p) => p !== target && precedes(target, p) && isPasswordCandidate(p),
      );
      if (confirm && !confirm.value && setValue(confirm, password)) filled += 1;
      try {
        target.focus({ preventScroll: true });
      } catch {
        // ignore
      }
    }
    return filled;
  }

  // ---------------------------------------------------------------------------
  // Capturing (save / update prompt)
  // ---------------------------------------------------------------------------

  /** The new password of a signup/change form (confirmation must match if present). */
  function newPassword(form) {
    const fields = form.kind === "change" ? form.passwords.slice(1) : form.passwords;
    const values = fields.map((f) => f.value).filter(Boolean);
    if (!values.length) return "";
    if (values.length >= 2 && values[values.length - 1] !== values[values.length - 2]) return "";
    return values[values.length - 1];
  }

  /**
   * Reads what the user entered: `{ username, password, kind }`, or null if
   * the form holds no usable data. For username-only steps `password` is "".
   */
  function readCredentials(form) {
    const username = form.username ? String(form.username.value || "").trim() : "";
    let password = "";
    if (form.kind === "login") password = form.password ? form.password.value : "";
    else if (form.kind === "signup" || form.kind === "change") password = newPassword(form);
    if (!password && !(form.kind === "username" && username)) return null;
    return { username, password, kind: form.kind };
  }

  /** The submit-like button/link an event target belongs to, or null. */
  function submitButtonFor(target) {
    if (!target || target.nodeType !== 1) return null;
    const btn = target.closest('button, input[type="submit"], input[type="image"], input[type="button"], [role="button"], a');
    if (!btn) return null;
    if (btn.localName === "button" && btn.type === "submit") return btn;
    if (isInput(btn) && (typeOf(btn) === "submit" || typeOf(btn) === "image")) return btn;
    const text = `${btn.textContent || ""} ${btn.value || ""} ${btn.getAttribute("aria-label") || ""} ${btn.id || ""}`
      .trim()
      .slice(0, 120)
      .toLowerCase();
    return SUBMIT_TEXT.test(text) ? btn : null;
  }

  globalThis.VaultXForms = Object.freeze({
    findLoginForms,
    formForElement,
    iconFields,
    rankForms,
    isVisible,
    isFillableNow,
    isTextEntry,
    isPasswordInput,
    isUsernameCandidate,
    usernameScore,
    setValue,
    fillForm,
    fillGenerated,
    readCredentials,
    submitButtonFor,
  });
})();
