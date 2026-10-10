/*
 * Keystead – login form detection and filling.
 *
 * Pure DOM helpers without any extension API, so they can be loaded on their
 * own in a test page (and in extension/tests with a small fake DOM). This is a
 * classic script (manifest content scripts cannot be ES modules): it defines
 * `globalThis.KeysteadForms`. In the extension it runs in the content script's
 * isolated world, before content.js.
 *
 * Web components: fields inside *open* shadow roots are found as well. The
 * caller passes the open shadow roots it knows (`options.shadowRoots`, see
 * content.js), otherwise the document is walked for them. Scopes, document
 * order and "is inside" checks follow the composed tree (a shadow root's
 * content belongs to its host). Closed shadow roots are the component's
 * private DOM and are not traversed (their fields are not detected).
 */
(() => {
  "use strict";

  /** Upper bound of inspected inputs per scan (protects huge pages). */
  const MAX_INPUTS = 800;
  /** Upper bound of open shadow roots taken into account. */
  const MAX_SHADOW_ROOTS = 400;
  /** Upper bound of elements visited while looking for shadow roots. */
  const MAX_WALK = 60_000;
  /** The periodic shadow-root sweep (every 3 s) walks the page on each of its first sweeps (30 s) … */
  const SWEEP_FULL_RATE = 10;
  /** … and up to this many (60 s) while custom elements wait for their definition; then on every 4th. */
  const SWEEP_UPGRADE_WAIT = 20;
  /** Node.DOCUMENT_POSITION_FOLLOWING */
  const FOLLOWING = 4;
  /** Input types that can hold a username. */
  const USERNAME_TYPES = new Set(["text", "email", "tel"]);
  /** Input types a generated password may be inserted into. */
  const TEXT_ENTRY_TYPES = new Set(["text", "password", "email", "search", "tel", "url"]);
  /** Input types a one-time code may be typed into. */
  const OTP_TYPES = new Set(["text", "tel", "number", "password"]);
  const BUTTONS = 'button, input[type="submit"], input[type="image"], [role="button"]';

  /** Attribute hints that suggest a username / e-mail field. */
  const USERNAME_HINT = /user|login|logon|e-?mail|account|signin|benutzer|anmeld|kennung|identifi|nutzer|konto|nick|member|kunde|customer|uid\b/;
  /** Hints that rule a field out as username (other personal data). */
  const NOT_USERNAME_HINT = /first.?name|last.?name|full.?name|vorname|nachname|surname|street|stra(ss|ß)e|address|adresse|city|stadt|zip|postal|plz|birth|geburt|company|firma|amount|betrag|iban|card|karte/;
  /**
   * An e-mail address ("E-Mail-Adresse", "emailAddress", "adresse e-mail") is
   * no postal address: removed before NOT_USERNAME_HINT is applied.
   */
  const EMAIL_ADDRESS = /mail[\s_.\-\u2010\u2011]*(?:adresse|address)|(?:adresse|address)[\s_.\-\u2010\u2011]*(?:e-?\s*mail|mail|[ée]lectronique)/g;
  /** Fields that are never login fields (search, captcha, one-time codes …). */
  const EXCLUDED_HINT = /search|suche|captcha|one.?time|\botp\b|totp|2fa|mfa|verification|verif|sms.?code|auth.?code|coupon|promo|gutschein|newsletter|subscribe|abonn/;
  /** Context that marks a lone e-mail/username field as the first step of a login. */
  const LOGIN_CONTEXT = /log\s*-?\s*in|logon|sign\s*-?\s*in|signin|anmeld|einlogg|auth|account|konto|identifier|session|sso|passwor/;
  /** Labels of buttons that submit a login (or signup / password change). */
  const SUBMIT_TEXT = /log\s*-?\s*in|logon|sign\s*-?\s*(in|on|up)|anmeld|einlogg|submit|absenden|senden|weiter|next|continue|fortfahren|best[äa]tig|confirm|registr|create|erstellen|speichern|save|[äa]ndern|change|update|aktualisier|verify|enter|\bok\b|\bgo\b/;

  /** A password field that asks for a new password or repeats it (name/id/label). */
  const NEW_PASSWORD_HINT =
    /new|neu|confirm|repeat|retype|re-?enter|re-?type|wiederhol|best[äa]tig|nochmal|erneut|again|verif|second|create|choose|w[äa]hle|festleg|vergeb|(pass|pw|pwd|kennwort)\w*(2|two)\b/;
  /** A password field that asks for the current password (change forms, logins). */
  const CURRENT_PASSWORD_HINT =
    /current|existing|bisherig|aktuell|derzeit|(?:^|[^a-z])(?:old|alte[sn]?)(?:[^a-z]|$)|oldp|passwordold|passwortalt|altpass/;
  /** Context (form action, id, buttons, title, path) of a registration form. */
  const SIGNUP_CONTEXT =
    /regist|sign\s*-?\s*up|create\s*(an?\s*|your\s*)?(free\s*)?account|new\s*account|join\s*(now|us|free|\b)|enrol|(konto|account)\s*(anlegen|erstellen|er[öo]ffnen)|neues?\s*konto|neu-?anmeld|mitglied\s*werden|get\s*started/;
  /**
   * Context of a login (wins over signup words when both appear): log in,
   * sign in, anmelden, Anmeldung – not "Neuanmeldung" (a registration).
   */
  const LOGIN_ACTION = /log\s*-?\s*in|logon|sign\s*-?\s*in|signin|einlogg|(?<!neu-?)anmeld(?:en|ung)\b/;
  /** A button label that only moves on to the next step: "Weiter", "Next", "Continue ›" … */
  const CONTINUE_ONLY = /^[\s›»>→.…]*(?:weiter|next|continue|fortfahren|proceed)[\s›»>→.…]*$/;
  /** Headings of a form or section. */
  const HEADINGS = 'h1, h2, h3, legend, [role="heading"]';

  /** One-time code hints in name/id (with a 6–8 character limit). */
  const OTP_ID_HINT = /otp|totp|2fa|mfa|one.?time|onetime|verification.?code|verify.?code|auth.?code|security.?code|token|code/;
  /** Unambiguous one-time code hints (name/id/label). */
  const OTP_STRONG_HINT =
    /(?:^|[^a-z])otp|otp(?:[^a-z]|$)|otp.?code|totp|2fa|two.?factor|zwei.?faktor|mfa|one.?time|onetime|einmal.?code|authenticator|verification.?code|auth.?code|sicherheitscode|best[äa]tigungscode/;
  /** Code fields that are not one-time codes. */
  const NOT_OTP_HINT =
    /zip|postal|post.?code|plz|postleit|country|area.?code|phone|promo|coupon|voucher|gutschein|rabatt|discount|gift|captcha|search|suche|invit|referr|iban|bic|blz|sort.?code|cvc|cvv|csc|card|karte|\btan\b|recovery|backup|wiederherstell/;

  /** Inputs seen as type=password; a "show password" toggle turns them into type=text. */
  const knownPasswordFields = new WeakSet();

  const inputValueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  const textAreaValueSetter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;

  // ---------------------------------------------------------------------------
  // Composed tree (open shadow roots)
  // ---------------------------------------------------------------------------

  function isShadowRoot(node) {
    return !!node && node.nodeType === 11 && !!node.host;
  }

  /** The parent in the composed tree: the parent element, or the host of a shadow root. */
  function composedParent(node) {
    if (!node) return null;
    if (node.parentElement) return node.parentElement;
    const parent = node.parentNode;
    return isShadowRoot(parent) ? parent.host : null;
  }

  /** True if `node` is `container` or inside it, also across open shadow roots. */
  function composedContains(container, node) {
    if (!container || !node) return false;
    for (let n = node; n; n = n.parentNode || (isShadowRoot(n) ? n.host : null)) {
      if (n === container) return true;
    }
    return false;
  }

  /** The host of the shadow root `el` lives in (null in the document itself). */
  function shadowHost(el) {
    const root = typeof el.getRootNode === "function" ? el.getRootNode() : null;
    return isShadowRoot(root) ? root.host : null;
  }

  /** `node`, then the hosts of the shadow roots around it, innermost first. */
  function hostChain(node) {
    const chain = [node];
    for (let root = node.getRootNode(); isShadowRoot(root); root = root.host.getRootNode()) chain.push(root.host);
    return chain;
  }

  /** Document order in the composed tree (a host comes before its shadow content): -1, 0 or 1. */
  function compareComposed(a, b) {
    if (a === b) return 0;
    if (a.getRootNode() === b.getRootNode()) return a.compareDocumentPosition(b) & FOLLOWING ? -1 : 1;
    const chainA = hostChain(a);
    const chainB = hostChain(b);
    for (let i = 0; i < chainA.length; i += 1) {
      const root = chainA[i].getRootNode();
      const j = chainB.findIndex((n) => n.getRootNode() === root);
      if (j < 0) continue;
      if (chainA[i] === chainB[j]) return i < j ? -1 : 1;
      return chainA[i].compareDocumentPosition(chainB[j]) & FOLLOWING ? -1 : 1;
    }
    return 0;
  }

  /** True if `a` comes before `b` in (composed) document order. */
  function precedes(a, b) {
    return compareComposed(a, b) < 0;
  }

  /**
   * All open shadow roots inside `root` (an element's own one included,
   * nested ones too), at most `limit`. Closed shadow roots are not visible
   * here (`element.shadowRoot` is null for them).
   */
  function openShadowRoots(root = document, limit = MAX_SHADOW_ROOTS) {
    const found = new Set();
    let visited = 0;
    const visit = (node) => {
      if (!node || typeof node.querySelectorAll !== "function") return;
      const all = node.querySelectorAll("*");
      for (let i = 0; i < all.length; i += 1) {
        if (found.size >= limit || visited >= MAX_WALK) return;
        visited += 1;
        const shadow = all[i].shadowRoot;
        if (shadow && !found.has(shadow)) {
          found.add(shadow);
          visit(shadow);
        }
      }
    };
    if (root && root.nodeType === 1 && root.shadowRoot && limit > 0) {
      found.add(root.shadowRoot);
      visit(root.shadowRoot);
    }
    visit(root);
    return [...found];
  }

  /**
   * Whether the `count`-th periodic sweep for open shadow roots (content.js:
   * every 3 s, counted while the page is visible) walks the document: the
   * first 10 (30 s) do, up to the 20th (60 s) while custom elements still
   * wait for their definition (a component defined late attaches its shadow
   * root without any DOM mutation), then every 4th (12 s). Elements that are
   * never defined (Angular's <app-root>, any unregistered hyphenated tag) do
   * not keep the full rate for good – on a large page a sweep walks up to
   * MAX_WALK elements.
   */
  function shadowSweepDue(count, doc = document) {
    if (count % 4 === 0 || count <= SWEEP_FULL_RATE) return true;
    if (count > SWEEP_UPGRADE_WAIT) return false;
    try {
      return !!doc.querySelector(":not(:defined)");
    } catch {
      return false; // :defined unsupported
    }
  }

  /** Where fields are looked up: a document and the open shadow roots in it. */
  function makeTree(doc, shadowRoots) {
    const roots = (Array.isArray(shadowRoots) ? shadowRoots : openShadowRoots(doc)).filter(
      (r) => isShadowRoot(r) && r.host.isConnected,
    );
    return { doc, roots };
  }

  /** Elements matching `selector` in `node`, including inside the open shadow roots within it. */
  function queryDeep(tree, node, selector, sorted = false) {
    const result = Array.from(node.querySelectorAll(selector));
    if (!tree.roots.length) return result;
    let extra = false;
    for (const root of tree.roots) {
      if (root === node || !composedContains(node, root.host)) continue;
      const found = root.querySelectorAll(selector);
      if (found.length) {
        result.push(...found);
        extra = true;
      }
    }
    return extra && sorted ? result.sort(compareComposed) : result;
  }

  function hasButton(tree, node) {
    if (node.querySelector(BUTTONS)) return true;
    return tree.roots.some((root) => root !== node && composedContains(node, root.host) && !!root.querySelector(BUTTONS));
  }

  /** `el.closest(selector)`, continued at the host of each shadow root. */
  function closestComposed(el, selector) {
    for (let node = el; node; ) {
      const hit = node.closest(selector);
      if (hit) return hit;
      node = shadowHost(node);
    }
    return null;
  }

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
    let value = el.getAttribute("autocomplete");
    // Web components often take the hint on the host element.
    if (!value) value = shadowHost(el)?.getAttribute("autocomplete");
    return String(value || "")
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean);
  }

  function hasToken(el, token) {
    return autocompleteTokens(el).includes(token);
  }

  /** name + id (the most reliable hints), lower-cased; plus those of a web component's host. */
  function idHints(el) {
    let hints = `${el.getAttribute("name") || ""} ${el.id || ""}`;
    const host = shadowHost(el);
    if (host) hints += ` ${host.getAttribute("name") || ""} ${host.id || ""}`;
    return hints.toLowerCase();
  }

  /** Visible descriptions: placeholder, aria-label, labels (and a web component host's label). */
  function labelHints(el) {
    const parts = [el.getAttribute("placeholder"), el.getAttribute("aria-label"), el.getAttribute("title")];
    try {
      for (const label of el.labels || []) parts.push(label.textContent);
    } catch {
      // `labels` throws on some exotic inputs; hints are best effort.
    }
    const labelledBy = el.getAttribute("aria-labelledby");
    if (labelledBy) {
      const root = el.getRootNode();
      for (const id of labelledBy.split(/\s+/).slice(0, 4)) {
        parts.push((typeof root.getElementById === "function" ? root.getElementById(id) : null)?.textContent);
      }
    }
    const host = shadowHost(el);
    if (host) parts.push(host.getAttribute("label"), host.getAttribute("aria-label"), host.getAttribute("placeholder"));
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
    if (typeOf(el) === "search" || closestComposed(el, '[role="search"]')) return true;
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
      for (let node = el; node && node.nodeType === 1; node = composedParent(node)) {
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

  /** A masked field that really asks for a one-time code (never a login password). */
  function isMaskedOtp(el) {
    return hasToken(el, "one-time-code") || OTP_STRONG_HINT.test(idHints(el));
  }

  function isPasswordCandidate(el) {
    return isPasswordInput(el) && !el.disabled && !el.readOnly && !isMaskedOtp(el) && isVisible(el);
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

  /** True if hints name other personal data (a postal address, a name …) – an e-mail address does not count. */
  function notUsernameHint(hints) {
    return NOT_USERNAME_HINT.test(hints.replace(EMAIL_ADDRESS, " "));
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
    if (notUsernameHint(ids) || notUsernameHint(labels)) score -= 60;
    return score;
  }

  /**
   * What a password field asks for: "new" (autocomplete=new-password, or
   * new/confirm/repeat/neu/wiederholen/bestätigen … in name, id or label),
   * "current" (autocomplete=current-password, current/old/aktuell/alt …) or
   * null (no hint – decided by the form, see classify).
   */
  function passwordRole(el) {
    const tokens = autocompleteTokens(el);
    if (tokens.includes("new-password")) return "new";
    if (tokens.includes("current-password")) return "current";
    const ids = idHints(el);
    const labels = labelHints(el);
    if (CURRENT_PASSWORD_HINT.test(ids) || CURRENT_PASSWORD_HINT.test(labels)) return "current";
    if (NEW_PASSWORD_HINT.test(ids) || NEW_PASSWORD_HINT.test(labels)) return "new";
    return null;
  }

  // ---------------------------------------------------------------------------
  // Form discovery
  // ---------------------------------------------------------------------------

  /**
   * The element that groups a field with its siblings: its <form>, or the
   * nearest ancestor that also contains another login-ish field (div-based
   * SPA forms, web components), or the nearest ancestor with a button.
   */
  function scopeFor(el, tree) {
    if (el.form) return el.form;
    const doc = el.ownerDocument;
    let withButton = null;
    let node = composedParent(el);
    for (let depth = 0; node && node !== doc.body && node !== doc.documentElement && depth < 8; depth += 1) {
      const inputs = queryDeep(tree, node, "input");
      // A container with dozens of inputs is a whole page section, not a login form.
      if (inputs.length > 40) break;
      for (const other of inputs) {
        if (other !== el && (isPasswordCandidate(other) || isUsernameCandidate(other))) return node;
      }
      if (!withButton && hasButton(tree, node)) withButton = node;
      node = composedParent(node);
    }
    return withButton || composedParent(el) || doc.body || doc.documentElement;
  }

  /** The username field belonging to a password field: the nearest suitable preceding field. */
  function usernameFor(password, scope, tree) {
    const candidates = [];
    for (const input of queryDeep(tree, scope, "input", true)) {
      if (input !== password && precedes(input, password) && isUsernameCandidate(input)) candidates.push(input);
    }
    if (!candidates.length) return null;
    const explicit = candidates.filter((c) => usernameScore(c) >= 100);
    if (explicit.length) return explicit[explicit.length - 1];
    const plausible = candidates.filter((c) => usernameScore(c) > -30);
    return plausible.length ? plausible[plausible.length - 1] : null;
  }

  /** The words of a form or container itself: action, id, class, aria-label, name. */
  function scopeAttributes(scope) {
    return [
      scope.getAttribute?.("action"),
      scope.id,
      typeof scope.className === "string" ? scope.className : "",
      scope.getAttribute?.("aria-label"),
      scope.getAttribute?.("name"),
    ];
  }

  function buttonLabel(btn) {
    return btn.textContent || btn.value || btn.getAttribute("aria-label");
  }

  function joinWords(parts) {
    return parts
      .filter(Boolean)
      .map((p) => String(p).slice(0, 160))
      .join(" ")
      .toLowerCase();
  }

  /** The form's own words: action, id, class, name, aria-label and its buttons' labels. */
  function ownContext(scope, tree) {
    const parts = scopeAttributes(scope);
    let buttons = 0;
    for (const btn of queryDeep(tree, scope, 'button, input[type="submit"], [role="button"]')) {
      parts.push(buttonLabel(btn));
      buttons += 1;
      if (buttons >= 6) break;
    }
    return joinWords(parts);
  }

  function isSubmitButton(btn) {
    if (btn.localName === "button") return String(btn.type || "submit").toLowerCase() === "submit";
    return btn.localName === "input";
  }

  /**
   * The labels of the buttons that act on a step itself: in a <form> its
   * submit buttons (all of its buttons if none submits), elsewhere every
   * button. A button leading to another flow ("Konto erstellen", "Anmelden
   * mit Passkey") beside one that only moves on ("Weiter", "Next") is left
   * out: Google's "Konto erstellen" next to "Weiter" says nothing about the
   * step.
   */
  function stepButtonLabels(scope, tree) {
    const buttons = queryDeep(tree, scope, 'button, input[type="submit"], [role="button"]').slice(0, 10);
    let primary = scope.localName === "form" ? buttons.filter(isSubmitButton) : buttons;
    if (!primary.length) primary = buttons;
    let labels = primary.map((btn) => String(buttonLabel(btn) || "").trim().slice(0, 160).toLowerCase()).filter(Boolean);
    if (labels.some((label) => CONTINUE_ONLY.test(label))) {
      labels = labels.filter((label) => !SIGNUP_CONTEXT.test(label) && !LOGIN_ACTION.test(label));
    }
    return labels;
  }

  /** A step's own words: action, id, class, name, aria-label and the labels of its own buttons (stepButtonLabels). */
  function stepOwnContext(scope, tree) {
    return joinWords([...scopeAttributes(scope), ...stepButtonLabels(scope, tree)]);
  }

  /** The page's words: title and path. */
  function pageContext(doc) {
    return `${doc.title || ""} ${doc.location ? doc.location.pathname : ""}`.slice(0, 300).toLowerCase();
  }

  function scopeContext(scope, tree) {
    return `${ownContext(scope, tree)} ${pageContext(scope.ownerDocument)}`;
  }

  /** The container of a lone username field: its form or the nearest ancestor with a button. */
  function stepScopeFor(el, tree) {
    if (el.form) return el.form;
    const doc = el.ownerDocument;
    let node = composedParent(el);
    for (let depth = 0; node && node !== doc.body && depth < 6; depth += 1) {
      if (hasButton(tree, node)) return node;
      node = composedParent(node);
    }
    return composedParent(el) || doc.body;
  }

  function headingText(h) {
    return String(h.textContent || "").slice(0, 120);
  }

  /** The first headings (h1–h3, legend, role=heading) inside a container: "Konto erstellen", "Anmelden" … */
  function headingContext(scope, tree) {
    return queryDeep(tree, scope, HEADINGS).slice(0, 3).map(headingText).join(" ").toLowerCase();
  }

  /**
   * The heading a step sits under outside its own container: the last one
   * before it in the nearest of its ancestors (up to 3, below <body>) that
   * hold no other login field – "Neues Kundenkonto erstellen" above a
   * shop's registration form, "Anmelden" above Google's. An ancestor that
   * also holds another form's fields (a shop's login form beside its
   * registration) is beyond the step: its headings say nothing about it.
   */
  function headingBefore(scope, tree) {
    const doc = scope.ownerDocument;
    let node = composedParent(scope);
    for (let depth = 0; node && node !== doc.body && node !== doc.documentElement && depth < 3; depth += 1) {
      const inputs = queryDeep(tree, node, "input");
      if (inputs.length > 40) return "";
      if (inputs.some((i) => !composedContains(scope, i) && (isPasswordCandidate(i) || isUsernameCandidate(i)))) return "";
      const before = queryDeep(tree, node, HEADINGS, true).filter((h) => !composedContains(scope, h) && precedes(h, scope));
      if (before.length) return headingText(before[before.length - 1]).toLowerCase();
      node = composedParent(node);
    }
    return "";
  }

  /** "login" if the words say log in / anmelden (also beside signup words), "signup" if they only say sign up, else null. */
  function wordsIntent(words) {
    if (LOGIN_ACTION.test(words)) return "login";
    return SIGNUP_CONTEXT.test(words) ? "signup" : null;
  }

  /**
   * True if the step of a lone username/e-mail field reads like a
   * registration. Decided by the nearest words that say log in / sign in /
   * anmelden or sign up / create account / Konto erstellen … (login words
   * win when both appear), in this order: the step's own words (action, id,
   * name, its own buttons – see stepButtonLabels), its headings, the
   * heading it sits under (headingBefore), the page (title, path). So a
   * registration step (action /register, "Neues Kundenkonto erstellen")
   * stays one on a page titled "Kasse – Anmelden", and "Anmelden oder Konto
   * erstellen" or Google's "Konto erstellen" button beside "Weiter" on
   * "Anmeldung – Google Konten" stay login steps.
   */
  function signupStep(scope, tree) {
    const tiers = [
      () => stepOwnContext(scope, tree),
      () => headingContext(scope, tree),
      () => headingBefore(scope, tree),
      () => pageContext(scope.ownerDocument),
    ];
    for (const words of tiers) {
      const intent = wordsIntent(words());
      if (intent) return intent === "signup";
    }
    return false;
  }

  /**
   * The kind of step a lone username/e-mail field is: "username" (the first
   * step of a multi-step login), "signup" (the first step of a registration:
   * e-mail + "Weiter" under "Konto erstellen" – no login icon, see
   * rankForms) or null (no step). `passwordFields`: the password fields of
   * the forms found on the page – a field whose container holds one of them
   * belongs to that form (a div-based signup asking for the username in a
   * section of its own, after the password), it is no step of its own.
   */
  function stepKind(el, tree, passwordFields = []) {
    if (!isUsernameCandidate(el)) return null;
    const score = usernameScore(el);
    if (score < 35) return null;
    const scope = stepScopeFor(el, tree);
    if (passwordFields.some((p) => composedContains(scope, p))) return null;
    let textFields = 0;
    for (const input of queryDeep(tree, scope, "input")) {
      if (USERNAME_TYPES.has(typeOf(input)) && !isPasswordInput(input) && isVisible(input)) textFields += 1;
    }
    // A login step asks for one identifier (rarely a second field, e.g. a tenant).
    if (textFields > 2) return null;
    if (signupStep(scope, tree)) return "signup";
    return score >= 100 || LOGIN_CONTEXT.test(scopeContext(scope, tree)) ? "username" : null;
  }

  /** True if the form's own words (buttons, action …) say "log in" and nothing about signing up. */
  function clearlyLogin(scope, tree) {
    const own = ownContext(scope, tree);
    return LOGIN_ACTION.test(own) && !SIGNUP_CONTEXT.test(own);
  }

  /** "signup" if a form without field hints reads like a registration, else "login". */
  function formIntent(scope, tree) {
    const own = ownContext(scope, tree);
    const signupOwn = SIGNUP_CONTEXT.test(own);
    const loginOwn = LOGIN_ACTION.test(own);
    if (signupOwn !== loginOwn) return signupOwn ? "signup" : "login";
    if (!signupOwn) {
      const page = pageContext(scope.ownerDocument);
      if (SIGNUP_CONTEXT.test(page) && !LOGIN_ACTION.test(page)) return "signup";
    }
    return "login";
  }

  /**
   * The kind of a form from its visible password fields:
   * - one field: "signup" if it asks for a new password (unless the form
   *   clearly logs in – some sites mark login fields "new-password" to keep
   *   browsers from filling them) or the form reads like a registration,
   *   else "login";
   * - two fields: "change" if the first asks for the current password, else
   *   "signup" (the second one confirms the first);
   * - three or more: "change" (current, new, confirmation) unless the first
   *   one already asks for a new password.
   */
  function classify(passwords, scope, tree) {
    const roles = passwords.map(passwordRole);
    if (passwords.length === 1) {
      if (roles[0] === "new") return clearlyLogin(scope, tree) ? "login" : "signup";
      if (roles[0] === "current") return "login";
      return formIntent(scope, tree);
    }
    if (passwords.length === 2) return roles[0] === "current" ? "change" : "signup";
    return roles[0] === "new" ? "signup" : "change";
  }

  /**
   * Finds login-related forms in a document (and the open shadow roots in
   * it; pass the known ones as `options.shadowRoots`).
   *
   * Returns `LoginForm[]` with `{ scope, kind, username, password, passwords,
   * newPasswords }`: `kind` is "login" (one password field), "change"
   * (current + new password), "signup" (new password, usually + confirmation;
   * or the e-mail/username-only first step of a registration, without
   * password fields) or "username" (username-only step of a multi-step
   * login); `password` is
   * the field for the *current* password (null for signup/username steps);
   * `passwords` are all visible password fields of the form in document
   * order, `newPasswords` the ones that ask for the new password (the first
   * one is where a password is suggested, the others confirm it).
   */
  function findLoginForms(doc = document, options = {}) {
    const tree = makeTree(doc, options.shadowRoots);
    const inputs = queryDeep(tree, doc, "input", true).slice(0, MAX_INPUTS);
    const groups = new Map();
    for (const input of inputs) {
      if (!isPasswordCandidate(input)) continue;
      const scope = scopeFor(input, tree);
      const group = groups.get(scope);
      if (group) group.push(input);
      else groups.set(scope, [input]);
    }

    const forms = [];
    const used = new Set();
    for (const [scope, passwords] of groups) {
      const kind = classify(passwords, scope, tree);
      const username = usernameFor(passwords[0], scope, tree);
      const password = kind === "login" || kind === "change" ? passwords[0] : null;
      const newPasswords = kind === "signup" ? passwords.slice() : kind === "change" ? passwords.slice(1) : [];
      forms.push({ scope, kind, username, password, passwords, newPasswords });
      if (username) used.add(username);
    }

    // Username-only steps (of a login, or of a registration). A field inside
    // a form found above belongs to that form (e.g. the username asked for
    // after the password on a signup form, a customer number beside a
    // login's username), and so does a field whose container holds such a
    // form's password (a div-based signup with the username in a section of
    // its own): no step.
    const passwordForms = forms.slice();
    const passwordFields = passwordForms.flatMap((f) => f.passwords);
    for (const input of inputs) {
      if (used.has(input) || passwordForms.some((f) => composedContains(f.scope, input))) continue;
      const kind = stepKind(input, tree, passwordFields);
      if (!kind) continue;
      forms.push({ scope: stepScopeFor(input, tree), kind, username: input, password: null, passwords: [], newPasswords: [] });
      used.add(input);
    }
    return forms;
  }

  /** The form an element belongs to (as a field, or inside its scope). */
  function formForElement(forms, el) {
    if (!el) return null;
    return (
      forms.find((f) => f.username === el || f.passwords.includes(el)) ||
      forms.find((f) => composedContains(f.scope, el) || composedContains(el, f.scope)) ||
      null
    );
  }

  /** Fields that get the inline Keystead login icon (login fields only; signup forms get none). */
  function iconFields(forms) {
    const fields = [];
    for (const form of forms) {
      if (form.kind === "signup") continue;
      if (form.username) fields.push(form.username);
      if (form.password) fields.push(form.password);
    }
    return fields;
  }

  /**
   * Where a strong password can be suggested: per signup/change form the
   * first new-password field (`field`) and the fields that confirm it
   * (`confirm`). Login forms, username steps and search fields never appear.
   */
  function suggestionTargets(forms) {
    return forms
      .filter((f) => f.newPasswords && f.newPasswords.length)
      .map((f) => ({ form: f, field: f.newPasswords[0], confirm: f.newPasswords.slice(1) }));
  }

  /**
   * The forms a stored login may be filled into, most likely first: the form
   * with the focus, then login, change (its current-password field),
   * username step. Signup forms ask for a new password, which Keystead
   * suggests instead (see suggestionTargets): Ctrl+Shift+L (which picks the
   * login itself) never fills them. Only when the user picked the login in
   * the popup ("Ausfüllen", `options.explicit` – the service worker sets it
   * only after no frame had another form) does a signup form qualify – after
   * every other form, whatever has the focus – so a login form taken for a
   * registration can still be filled.
   */
  function rankForms(forms, activeElement, options = {}) {
    const explicit = options.explicit === true;
    const priority = { login: 0, change: 1, username: 2, signup: 3 };
    const active = formForElement(forms, activeElement);
    const fillable = (f) => (f.username && isFillableNow(f.username)) || f.passwords.some(isFillableNow);
    const signup = (f) => (f.kind === "signup" ? 1 : 0);
    return forms
      .filter((f) => (explicit || f.kind !== "signup") && fillable(f))
      .sort((a, b) => signup(a) - signup(b) || (b === active) - (a === active) || priority[a.kind] - priority[b.kind]);
  }

  // ---------------------------------------------------------------------------
  // One-time code (2FA) fields
  // ---------------------------------------------------------------------------

  function numericInput(el) {
    const mode = String(el.getAttribute("inputmode") || "").toLowerCase();
    const pattern = String(el.getAttribute("pattern") || "");
    return mode === "numeric" || mode === "decimal" || typeOf(el) === "tel" || typeOf(el) === "number" || /\\d|\[0-9\]/.test(pattern);
  }

  /** Character limit of a field (-1 if none). */
  function maxLengthOf(el) {
    const value = Number(el.getAttribute("maxlength"));
    return Number.isInteger(value) && value > 0 ? value : -1;
  }

  /** True if a field looks like it asks for a one-time code (visibility aside). */
  function looksLikeOtp(el) {
    if (!isInput(el) || el.disabled || el.readOnly) return false;
    const type = typeOf(el);
    if (!OTP_TYPES.has(type)) return false;
    if (hasToken(el, "one-time-code")) return true;
    const ids = idHints(el);
    const labels = labelHints(el);
    if (NOT_OTP_HINT.test(ids) || NOT_OTP_HINT.test(labels)) return false;
    // A masked field is a password unless its name says otherwise (a PIN is not a 2FA code).
    if (type === "password") return OTP_STRONG_HINT.test(ids);
    const max = maxLengthOf(el);
    const sized = max >= 6 && max <= 8;
    if (sized && OTP_ID_HINT.test(ids)) return true;
    return (OTP_STRONG_HINT.test(ids) || OTP_STRONG_HINT.test(labels)) && (sized || (max === -1 && numericInput(el)));
  }

  /** A visible one-time code field: autocomplete=one-time-code, or otp/totp/2fa/code … with 6–8 characters. */
  function isOtpField(el) {
    return looksLikeOtp(el) && isVisible(el);
  }

  /** Single-character boxes of a split code input (one box per digit). */
  function isCodeBox(el) {
    if (!isInput(el) || el.disabled || el.readOnly || maxLengthOf(el) !== 1) return false;
    if (!OTP_TYPES.has(typeOf(el)) || typeOf(el) === "password") return false;
    const ids = idHints(el);
    return !NOT_OTP_HINT.test(ids) && isVisible(el);
  }

  /**
   * One-time code fields of a document (and its open shadow roots):
   * `[{ field, group }]` – `group` is the field itself, or the 6–8 boxes of
   * a code that is typed one digit per box (`field` = the first box).
   */
  function findOtpFields(doc = document, options = {}) {
    const tree = makeTree(doc, options.shadowRoots);
    const inputs = queryDeep(tree, doc, "input", true).slice(0, MAX_INPUTS);
    const result = [];
    const inGroup = new Set();
    for (let i = 0; i < inputs.length; i += 1) {
      if (inGroup.has(inputs[i]) || !isCodeBox(inputs[i])) continue;
      const group = [inputs[i]];
      const parent = composedParent(inputs[i]);
      const grand = composedParent(parent);
      for (let j = i + 1; j < inputs.length && group.length < 8; j += 1) {
        const box = inputs[j];
        const near = composedParent(box) === parent || composedParent(composedParent(box)) === grand;
        if (!near || !isCodeBox(box)) break;
        group.push(box);
      }
      const hinted = group.some(
        (box) => hasToken(box, "one-time-code") || numericInput(box) || OTP_ID_HINT.test(idHints(box)) || OTP_STRONG_HINT.test(labelHints(box)),
      );
      if (group.length >= 6 && hinted) {
        for (const box of group) inGroup.add(box);
        result.push({ field: group[0], group });
      }
    }
    for (const input of inputs) {
      if (!inGroup.has(input) && isOtpField(input)) result.push({ field: input, group: [input] });
    }
    return result;
  }

  // ---------------------------------------------------------------------------
  // Filling
  // ---------------------------------------------------------------------------

  /**
   * Writes a value like a user would: through the native value setter (so
   * framework value trackers like React's notice the change) followed by
   * input/change/keyup events. The events are `composed`, so they also reach
   * listeners outside a web component's shadow root.
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
    el.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
    el.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, composed: true, key: "Unidentified" }));
    return el.value === value;
  }

  /**
   * Fills a login form with `{ username, password }`. Only writes into fields
   * that are visible and editable. A stored password never goes into a field
   * that asks for a new one (signup forms get the username at most) – unless
   * the user picked this login in the popup (`options.explicit`, see
   * rankForms): then a signup form gets it in its new-password field and every
   * field confirming it (a login form taken for a registration, or a
   * registration with a login prepared in Keystead). Returns the number of
   * filled fields.
   */
  function fillForm(form, credentials, options = {}) {
    let filled = 0;
    const username = typeof credentials.username === "string" ? credentials.username : "";
    const password = typeof credentials.password === "string" ? credentials.password : "";
    if (form.username && username && isFillableNow(form.username)) {
      if (setValue(form.username, username)) filled += 1;
    }
    if (form.kind === "signup") {
      if (options.explicit !== true || !password) return filled;
      for (const field of form.newPasswords.length ? form.newPasswords : form.passwords) {
        if (isFillableNow(field) && setValue(field, password)) filled += 1;
      }
      return filled;
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
  function fillGenerated(target, password, options = {}) {
    if (!isTextEntry(target) || !isFillableNow(target)) return 0;
    let filled = setValue(target, password) ? 1 : 0;
    if (filled && isPasswordInput(target)) {
      const tree = makeTree(target.ownerDocument, options.shadowRoots);
      const scope = scopeFor(target, tree);
      const confirm = queryDeep(tree, scope, "input", true).find(
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

  /**
   * Puts a suggested password into a suggestion target (see
   * suggestionTargets): the new-password field and every field confirming
   * it. Returns the fields that now hold it.
   */
  function fillSuggestion(target, password) {
    const filled = [];
    for (const field of [target.field, ...target.confirm]) {
      if (isFillableNow(field) && setValue(field, password)) filled.push(field);
    }
    try {
      target.field.focus({ preventScroll: true });
    } catch {
      // ignore
    }
    return filled;
  }

  /** Types a one-time code into an OTP target (see findOtpFields); returns true on success. */
  function fillOtp(target, code) {
    const digits = String(code || "").replace(/\s+/g, "");
    if (!digits || !target || !target.group.length) return false;
    if (target.group.length > 1) {
      if (digits.length > target.group.length) return false;
      let ok = true;
      target.group.forEach((box, i) => {
        if (!isFillableNow(box) || !setValue(box, digits[i] ?? "")) ok = false;
      });
      return ok;
    }
    return isFillableNow(target.field) && setValue(target.field, digits);
  }

  // ---------------------------------------------------------------------------
  // Capturing (save / update prompt)
  // ---------------------------------------------------------------------------

  /** The new password of a signup/change form (confirmation must match if present). */
  function newPassword(form) {
    const fields = form.newPasswords && form.newPasswords.length ? form.newPasswords : form.kind === "change" ? form.passwords.slice(1) : form.passwords;
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

  /** The submit-like button/link an event target belongs to (also across shadow roots), or null. */
  function submitButtonFor(target) {
    if (!target || target.nodeType !== 1) return null;
    const btn = closestComposed(target, 'button, input[type="submit"], input[type="image"], input[type="button"], [role="button"], a');
    if (!btn) return null;
    if (btn.localName === "button" && btn.type === "submit") return btn;
    if (isInput(btn) && (typeOf(btn) === "submit" || typeOf(btn) === "image")) return btn;
    const text = `${btn.textContent || ""} ${btn.value || ""} ${btn.getAttribute("aria-label") || ""} ${btn.id || ""}`
      .trim()
      .slice(0, 120)
      .toLowerCase();
    return SUBMIT_TEXT.test(text) ? btn : null;
  }

  globalThis.KeysteadForms = Object.freeze({
    findLoginForms,
    formForElement,
    iconFields,
    suggestionTargets,
    rankForms,
    passwordRole,
    findOtpFields,
    isOtpField,
    isVisible,
    isFillableNow,
    isTextEntry,
    isPasswordInput,
    isUsernameCandidate,
    usernameScore,
    openShadowRoots,
    shadowSweepDue,
    composedContains,
    composedParent,
    compareComposed,
    setValue,
    fillForm,
    fillGenerated,
    fillSuggestion,
    fillOtp,
    readCredentials,
    submitButtonFor,
  });
})();
