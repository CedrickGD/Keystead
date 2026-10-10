/*
 * Keystead content script (classic script, runs after lib/forms.js in every
 * http/https frame).
 *
 * - Detects login forms (lib/forms.js) – also inside open shadow roots of web
 *   components –, shows a Keystead icon inside username / password fields
 *   when the vault has matching logins or is locked; the icon opens a dropdown
 *   with the matching logins.
 * - Signup / password change forms: a trusted focus on the new-password field
 *   shows a suggestion bubble with a strong password (masked until the user
 *   reveals it). "Verwenden" fills it into the field and its confirmation and
 *   marks it as the user's own input (save prompt after submitting). Locked or
 *   not connected: the field icon only explains how to get suggestions.
 * - After a login with 2FA was filled the app copies the current code (toast),
 *   and a one-time code field on this or the next page offers
 *   "2FA-Code einfügen".
 * - Fills only after a user gesture (click in our UI, popup button, keyboard
 *   shortcut). The service worker matches the login against the URL of this
 *   frame as known by the browser – nothing the page says is trusted. Clicks
 *   and keys in our UI only count while the control has been fully visible
 *   (unoccluded, no opacity/filter effects; IntersectionObserver v2) for a
 *   moment, so a page cannot trick the user into using our UI hidden under its
 *   own content (clickjacking). A click that comes too early or onto a covered
 *   control does nothing but says why (shake + tooltip).
 * - Offers to save / update logins after a form is submitted – only for a
 *   password the user typed (or inserted from the generator / a suggestion)
 *   themselves.
 *
 * All UI lives in a closed shadow root with its own styles. Secrets are never
 * logged and never written anywhere but into the chosen input fields (a
 * suggested password is only shown in our own closed shadow root before the
 * user takes it).
 */
(() => {
  "use strict";

  const Forms = globalThis.KeysteadForms;
  if (!Forms || !globalThis.chrome?.runtime?.id) return;

  // Re-injection after an extension update: replace the previous instance.
  const previous = globalThis.__keysteadContent;
  if (previous && typeof previous.teardown === "function") {
    try {
      previous.teardown();
    } catch {
      // The old instance belongs to an invalidated extension context.
    }
  }

  const isTop = window === window.top;
  const SCAN_THROTTLE_MS = 400;
  const PAGE_INFO_MAX_AGE_MS = 15_000;
  const CAPTURE_DEDUP_MS = 10_000;
  const SNAPSHOT_TTL_MS = 60_000;
  const BAR_TTL_MS = 2 * 60_000;
  const SVG_NS = "http://www.w3.org/2000/svg";
  /** How long a control must have been fully visible before it accepts a click or key. */
  const VISIBLE_DWELL_MS = 500;
  /** A click on a control that is not visible yet this soon after it appeared is "too early", later "covered". */
  const EARLY_GRACE_MS = VISIBLE_DWELL_MS + 400;
  /** Open shadow roots are looked for again this often (custom elements upgraded later attach theirs silently). */
  const SHADOW_SWEEP_MS = 3_000;
  const MAX_SHADOW_ROOTS = 400;
  /** Added subtrees beyond this many per scan: one sweep of the whole document instead. */
  const MAX_PENDING_SUBTREES = 60;
  /** The tab's 2FA offer is asked again after this long (or when the service worker announces one). */
  const OTP_OFFER_RECHECK_MS = 20_000;
  const TOTP_TOAST_MS = 6_000;
  const AVATAR_HUES = [214, 262, 330, 20, 145, 188, 282, 350, 38, 168, 236, 4];
  const OBSERVE_OPTIONS = {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["type", "style", "class", "hidden", "disabled", "aria-hidden", "open", "autocomplete", "maxlength"],
  };

  const t = (key, substitutions) => {
    try {
      return chrome.i18n.getMessage(key, substitutions) || "";
    } catch {
      return "";
    }
  };

  let alive = true;
  /** Detected login forms (see KeysteadForms.findLoginForms). */
  let forms = [];
  /** One-time code fields (see KeysteadForms.findOtpFields). */
  let otpTargets = [];
  /** { state, matches: [{ id, name, username }], insecure, suggest, at } from the service worker. */
  let pageInfo = null;
  let pageInfoRunning = false;
  let pageInfoQueued = false;
  let lastUrl = location.href;
  /** The text field this frame last reported as focused (cs:focus); null after it lost the focus. */
  let lastReportedFocus = null;
  let lastContextTarget = null;
  /** Credentials typed into a form (for SPA logins without a submit event). */
  let snapshot = null;
  /**
   * field → the value the user gave it (trusted input: typing, pasting) or that
   * the user inserted with the generator or a suggestion. A password is only
   * offered for saving if a field still holds exactly that value: a page cannot
   * make us prompt for a password it filled in (or replaced) by script.
   */
  let userValues = new WeakMap();
  let lastCapture = { key: "", at: 0 };
  /** Open dropdown: { field, node, index }. */
  let dropdown = null;
  const cleanups = [];
  const api = { teardown };
  globalThis.__keysteadContent = api;

  class ContentError extends Error {
    constructor(code) {
      super(code);
      this.code = code;
    }
  }

  // ---------------------------------------------------------------------------
  // Messaging
  // ---------------------------------------------------------------------------

  async function send(message) {
    if (!alive || !chrome.runtime?.id) {
      teardown();
      throw new ContentError("inactive");
    }
    let response;
    try {
      response = await chrome.runtime.sendMessage(message);
    } catch {
      // "Extension context invalidated" after an update/uninstall.
      if (!chrome.runtime?.id) teardown();
      throw new ContentError("inactive");
    }
    if (!response || response.ok !== true) throw new ContentError(response?.error || "internal");
    return response.data;
  }

  function listen(target, type, handler, options) {
    target.addEventListener(type, handler, options);
    cleanups.push(() => target.removeEventListener(type, handler, options));
  }

  function errorMessage(code) {
    switch (code) {
      case "insecure":
        return t("csInsecureBlocked");
      case "locked":
        return t("csLockedHint");
      case "not_paired":
      case "host_missing":
      case "host_forbidden":
      case "app_unavailable":
      case "timeout":
        return t("csNotConnected");
      case "inactive":
        return "";
      default:
        return t("csFillFailed");
    }
  }

  // ---------------------------------------------------------------------------
  // Open shadow roots (web components)
  // ---------------------------------------------------------------------------
  //
  // An isolated world cannot hook attachShadow (that needs a main-world
  // script), so roots are found by looking: in subtrees added to the page (and
  // to known roots), in the composed path of focus/input events, and by a
  // throttled sweep of the document (custom elements that are defined later
  // attach their shadow root without any DOM mutation). Known roots are
  // observed like the document. Closed shadow roots stay out of reach.

  /** Known open shadow roots of this frame. */
  const shadowRoots = new Set();
  /** Element subtrees added since the last scan, to look for shadow roots in. */
  let addedSubtrees = [];
  /** Sweeps while the page was visible (see Forms.shadowSweepDue). */
  let sweepCount = 0;

  function addShadowRoot(root) {
    if (!root || shadowRoots.has(root) || shadowRoots.size >= MAX_SHADOW_ROOTS || root.host === host) return false;
    shadowRoots.add(root);
    try {
      observer.observe(root, OBSERVE_OPTIONS);
    } catch {
      // Observing is a refinement; the sweep still finds changes.
    }
    // `submit` does not leave a shadow root (not composed).
    root.addEventListener("submit", onSubmit, true);
    return true;
  }

  function discoverShadowRoots(node) {
    let added = false;
    try {
      for (const root of Forms.openShadowRoots(node, MAX_SHADOW_ROOTS)) if (addShadowRoot(root)) added = true;
    } catch {
      // A node that went away meanwhile.
    }
    return added;
  }

  /** Forgets roots whose host left the page (re-observes the rest). */
  function pruneShadowRoots() {
    let removed = false;
    for (const root of shadowRoots) {
      if (root.host.isConnected) continue;
      shadowRoots.delete(root);
      root.removeEventListener("submit", onSubmit, true);
      removed = true;
    }
    if (!removed) return;
    observer.disconnect();
    observer.observe(document.documentElement, OBSERVE_OPTIONS);
    for (const root of shadowRoots) observer.observe(root, OBSERVE_OPTIONS);
  }

  function processAddedSubtrees() {
    const pending = addedSubtrees;
    addedSubtrees = [];
    if (pending.length > MAX_PENDING_SUBTREES) {
      discoverShadowRoots(document);
      return;
    }
    for (const node of pending) if (node.isConnected) discoverShadowRoots(node);
  }

  /** Shadow roots on the composed path of a trusted event (e.g. focus moving into a web component). */
  function noteShadowPath(ev) {
    let added = false;
    for (const node of ev.composedPath()) {
      if (node instanceof ShadowRoot && node.mode === "open" && addShadowRoot(node)) added = true;
    }
    return added;
  }

  /** Every 3 s; walks the document only when due (full rate at first, then every 12 s, see shadowSweepDue). */
  function sweepShadowRoots() {
    if (!alive || document.hidden) return;
    sweepCount += 1;
    if (!Forms.shadowSweepDue(sweepCount, document)) return;
    if (discoverShadowRoots(document)) scheduleScan();
  }

  const knownRoots = () => [...shadowRoots];

  // ---------------------------------------------------------------------------
  // Detection
  // ---------------------------------------------------------------------------

  let scanTimer = null;
  let lastScanAt = 0;

  function scheduleScan() {
    if (!alive || scanTimer) return;
    const wait = Math.max(0, SCAN_THROTTLE_MS - (Date.now() - lastScanAt));
    scanTimer = setTimeout(() => {
      scanTimer = null;
      scan();
    }, wait);
  }

  /** Live collection: a cheap check whether the page has any input at all. */
  const allInputs = document.getElementsByTagName("input");
  const hasFields = () => allInputs.length > 0 || shadowRoots.size > 0;

  function refreshForms() {
    if (!hasFields()) {
      forms = [];
      return forms;
    }
    try {
      forms = Forms.findLoginForms(document, { shadowRoots: knownRoots() });
    } catch {
      forms = [];
    }
    return forms;
  }

  function refreshOtpTargets() {
    if (!hasFields()) {
      otpTargets = [];
      return;
    }
    try {
      otpTargets = Forms.findOtpFields(document, { shadowRoots: knownRoots() });
    } catch {
      otpTargets = [];
    }
  }

  function scan() {
    if (!alive) return;
    lastScanAt = Date.now();
    if (document.hidden) return; // rescanned on visibilitychange
    processAddedSubtrees();
    pruneShadowRoots();
    refreshForms();
    refreshOtpTargets();
    checkSnapshot();
    const urlChanged = location.href !== lastUrl;
    if (urlChanged) {
      lastUrl = location.href;
      pageInfo = null;
      otpOffer = null;
    }
    if (forms.length && !pageInfo) requestPageInfo();
    if (otpTargets.length) checkOtpOffer();
    placeUi();
    renderIcons();
    renderPills();
    if (bubble) positionBubble();
  }

  async function requestPageInfo() {
    if (pageInfoRunning) {
      pageInfoQueued = true;
      return;
    }
    pageInfoRunning = true;
    try {
      const info = await send({ type: "cs:page-info" });
      pageInfo = {
        state: typeof info?.state === "string" ? info.state : "error",
        insecure: info?.insecure === true,
        suggest: info?.suggest !== false,
        matches: Array.isArray(info?.matches) ? info.matches.filter((m) => m && typeof m.id === "string") : [],
        at: Date.now(),
      };
    } catch {
      pageInfo = { state: "error", insecure: false, suggest: false, matches: [], at: Date.now() };
    } finally {
      pageInfoRunning = false;
    }
    if (!alive) return;
    if (pageInfoQueued) {
      pageInfoQueued = false;
      requestPageInfo();
      return;
    }
    renderIcons();
    if (dropdown) renderDropdown();
    if (pendingSuggestion) {
      const field = pendingSuggestion;
      pendingSuggestion = null;
      if (deepActiveElement() === field && canSuggest(field)) openBubble(field);
    }
  }

  /** The focused element, looking into open shadow roots of the page. */
  function deepActiveElement() {
    let active = document.activeElement;
    while (active && active.shadowRoot && active.shadowRoot.activeElement) active = active.shadowRoot.activeElement;
    return active;
  }

  function eventTarget(ev) {
    const path = typeof ev.composedPath === "function" ? ev.composedPath() : [];
    return path.length ? path[0] : ev.target;
  }

  // ---------------------------------------------------------------------------
  // UI host (closed shadow root)
  // ---------------------------------------------------------------------------

  // Applied through CSSOM (style.setProperty), which a page's CSP does not block.
  const HOST_STYLE = [
    ["all", "initial"],
    ["position", "fixed"],
    ["top", "0"],
    ["left", "0"],
    ["right", "auto"],
    ["bottom", "auto"],
    ["width", "0"],
    ["height", "0"],
    ["margin", "0"],
    ["padding", "0"],
    ["border", "0"],
    ["background", "transparent"],
    ["overflow", "visible"],
    ["display", "block"],
    ["opacity", "1"],
    ["visibility", "visible"],
    ["pointer-events", "none"],
    ["transform", "none"],
    ["filter", "none"],
    ["z-index", "2147483647"],
  ];

  const CSS = `
    :host { all: initial; }
    .layer {
      --bg: #ffffff; --bg-hover: #f0f3f8; --bg-active: #e6edfb; --bg-sunken: #f5f7fa; --border: #e1e4ea; --text: #181b21;
      --text-2: #4f5663; --text-3: #626976; --accent: #2f6fed; --accent-hover: #2861d6; --accent-soft: #eaf1fe;
      --accent-text: #1c54c2; --accent-border: #c9dafb; --digit: #1c54c2; --symbol: #b4540a;
      --warning: #8a4b06; --warning-soft: #fdf5e7; --success: #146c36; --danger: #b42318;
      --tip-bg: #1d2128; --tip-text: #f3f4f6; --tip-warn: #f3be5e;
      --shadow: 0 12px 32px rgba(16, 24, 40, 0.18), 0 2px 6px rgba(16, 24, 40, 0.08);
      --shadow-sm: 0 4px 12px rgba(16, 24, 40, 0.14), 0 1px 2px rgba(16, 24, 40, 0.08);
      --av-bg-s: 78%; --av-bg-l: 93%; --av-fg-s: 58%; --av-fg-l: 36%;
      position: absolute; top: 0; left: 0; width: 0; height: 0;
      font: 400 13px/1.4 "Segoe UI Variable Text", "Segoe UI", Inter, system-ui, -apple-system, "Helvetica Neue", Arial, sans-serif;
      color: var(--text); letter-spacing: normal; text-align: left; -webkit-font-smoothing: antialiased;
    }
    @media (prefers-color-scheme: dark) {
      .layer {
        --bg: #22262d; --bg-hover: #2c3139; --bg-active: rgba(79, 140, 255, 0.18); --bg-sunken: #1a1d22; --border: #353b45; --text: #e8eaee;
        --text-2: #aab1bd; --text-3: #8c94a1; --accent: #4f8cff; --accent-hover: #3b7af2; --accent-soft: rgba(79, 140, 255, 0.14);
        --accent-text: #8fb6ff; --accent-border: rgba(79, 140, 255, 0.4); --digit: #8fb6ff; --symbol: #f3a35e;
        --warning: #f3be5e; --warning-soft: rgba(240, 167, 58, 0.12); --success: #6ad891; --danger: #ff8a8a;
        --tip-bg: #3a404b; --tip-text: #f3f4f6; --tip-warn: #f3be5e;
        --shadow: 0 12px 32px rgba(0, 0, 0, 0.5), 0 2px 6px rgba(0, 0, 0, 0.35);
        --shadow-sm: 0 4px 12px rgba(0, 0, 0, 0.45), 0 1px 2px rgba(0, 0, 0, 0.3);
        --av-bg-s: 38%; --av-bg-l: 23%; --av-fg-s: 85%; --av-fg-l: 78%;
      }
    }
    .layer > * { position: absolute; top: 0; left: 0; box-sizing: border-box; pointer-events: auto; }
    .layer *, .layer *::before, .layer *::after { box-sizing: border-box; }
    button { font: inherit; color: inherit; margin: 0; }
    svg { display: block; }

    .icon {
      all: unset; position: absolute; top: 0; left: 0; box-sizing: border-box; pointer-events: auto;
      display: none; cursor: pointer; border-radius: 5px;
      transition: box-shadow 120ms ease;
    }
    .icon:hover { box-shadow: 0 0 0 3px rgba(47, 111, 237, 0.22); }
    .icon > svg:first-child { width: 100%; height: 100%; }
    .icon.locked .tile { fill: #7d8592; }
    .icon .badge { position: absolute; right: -3px; bottom: -3px; width: 55%; height: 55%; }

    .dropdown {
      background: var(--bg); border: 1px solid var(--border); border-radius: 12px; box-shadow: var(--shadow);
      padding: 6px; max-height: 320px; overflow-y: auto; overscroll-behavior: contain;
      animation: ks-pop 120ms ease-out;
    }
    .dd-head {
      display: flex; align-items: center; gap: 8px; padding: 4px 6px 8px; margin-bottom: 4px;
      border-bottom: 1px solid var(--border); color: var(--text-2); font-size: 12px; font-weight: 600;
    }
    .dd-head svg { width: 16px; height: 16px; flex: none; }
    .dd-head .host { margin-left: auto; font-weight: 400; color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 55%; }
    .dd-warn { margin: 2px 4px 6px; padding: 6px 8px; border-radius: 8px; background: var(--warning-soft); color: var(--warning); font-size: 12px; }
    .item {
      all: unset; display: flex; align-items: center; gap: 10px; width: 100%; box-sizing: border-box;
      padding: 7px 8px; border-radius: 8px; cursor: pointer;
    }
    .item:hover { background: var(--bg-hover); }
    .item.active { background: var(--bg-active); }
    .avatar {
      --h: 214; flex: none; width: 28px; height: 28px; border-radius: 8px; display: grid; place-items: center;
      background: hsl(var(--h) var(--av-bg-s) var(--av-bg-l)); color: hsl(var(--h) var(--av-fg-s) var(--av-fg-l));
      font-weight: 700; font-size: 12.5px; line-height: 1;
    }
    .texts { min-width: 0; flex: 1; display: flex; flex-direction: column; }
    .name, .user { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .name { font-weight: 600; color: var(--text); }
    .user { font-size: 12px; color: var(--text-3); }
    .fill-hint { flex: none; font-size: 11.5px; color: var(--accent); font-weight: 600; opacity: 0; }
    .item:hover .fill-hint, .item.active .fill-hint { opacity: 1; }
    .dd-locked { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 10px 10px 8px; text-align: center; }
    .dd-locked .title { font-weight: 600; }
    .dd-locked .hint { color: var(--text-3); font-size: 12px; margin-bottom: 8px; }
    .dd-empty { padding: 10px; color: var(--text-3); text-align: center; }

    /* Dwell: a control accepts clicks once it has been fully visible for 500 ms. */
    .ring { display: none; flex: none; width: 16px; height: 16px; }
    .ring circle { fill: none; stroke-width: 2.4; }
    .ring .track { stroke: var(--border); }
    .ring .progress { stroke: var(--accent); stroke-linecap: round; stroke-dasharray: 37.7; stroke-dashoffset: 37.7; transform: rotate(-90deg); transform-origin: 50% 50%; }
    .item.arming .ring { display: block; }
    .item.arming .fill-hint { display: none; }
    .item.arming .ring .progress { animation: ks-ring ${VISIBLE_DWELL_MS}ms linear var(--dwell-delay, 0ms) both; }
    .btn.dwell, .pill.dwell { overflow: hidden; }
    .btn.dwell { position: relative; }
    .btn.dwell::after, .pill.dwell::after {
      content: ""; position: absolute; left: 0; right: 0; bottom: 0; height: 2px; background: currentColor; opacity: 0.55;
      transform: scaleX(0); transform-origin: left center; pointer-events: none;
    }
    .btn.dwell.arming::after, .pill.dwell.arming::after { animation: ks-bar ${VISIBLE_DWELL_MS}ms linear var(--dwell-delay, 0ms) both; }

    .btn {
      all: unset; display: inline-flex; align-items: center; justify-content: center; gap: 6px; height: 30px;
      padding: 0 12px; border-radius: 8px; font-weight: 600; font-size: 12.5px; cursor: pointer; white-space: nowrap;
      box-sizing: border-box;
    }
    .btn:focus-visible { box-shadow: 0 0 0 3px rgba(47, 111, 237, 0.35); }
    .btn[disabled] { opacity: 0.6; cursor: default; }
    .btn-primary { background: var(--accent); color: #fff; }
    .btn-primary:hover:not([disabled]) { background: var(--accent-hover); }
    .btn-ghost { color: var(--text-2); }
    .btn-ghost:hover:not([disabled]) { background: var(--bg-hover); color: var(--text); }
    .btn-icon { width: 30px; padding: 0; }
    .btn-icon svg { width: 16px; height: 16px; }

    .bubble {
      display: flex; flex-direction: column; gap: 8px; padding: 10px 10px 10px 12px;
      background: var(--bg); border: 1px solid var(--border); border-radius: 12px; box-shadow: var(--shadow);
      animation: ks-pop 120ms ease-out;
    }
    .bubble-head { display: flex; align-items: center; gap: 8px; min-height: 22px; }
    .bubble-head > svg { width: 18px; height: 18px; flex: none; }
    .bubble-head .title { flex: 1; min-width: 0; font-weight: 600; font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .bubble-head .btn-icon { width: 24px; height: 24px; margin-right: -4px; border-radius: 6px; }
    .bubble-head .btn-icon svg { width: 14px; height: 14px; }
    .bubble-row { display: flex; align-items: center; gap: 10px; }
    .secret-box {
      display: flex; align-items: center; gap: 2px; min-height: 34px; padding: 2px 2px 2px 10px;
      background: var(--bg-sunken); border: 1px solid var(--border); border-radius: 8px;
    }
    .secret {
      flex: 1; min-width: 0; padding: 3px 0; overflow-wrap: anywhere; word-break: break-all;
      font: 500 13px/1.35 "Cascadia Mono", "Cascadia Code", Consolas, "JetBrains Mono", "DejaVu Sans Mono", ui-monospace, Menlo, monospace;
      color: var(--text);
    }
    .secret.masked { color: var(--text-2); letter-spacing: 0.06em; white-space: nowrap; overflow: hidden; }
    .secret .d { color: var(--digit); }
    .secret .s { color: var(--symbol); }
    .secret.error { font: inherit; font-size: 12px; color: var(--danger); }
    .secret-box .btn-icon { width: 28px; height: 28px; border-radius: 6px; }
    .bubble-note { flex: 1; min-width: 0; font-size: 11.5px; line-height: 1.35; color: var(--text-3); }
    .bubble.hint { flex-direction: row; align-items: center; gap: 10px; padding: 8px 8px 8px 10px; }
    .bubble.hint > svg { width: 22px; height: 22px; flex: none; }
    .bubble.hint .hint-text { flex: 1; min-width: 0; font-size: 12.5px; color: var(--text-2); }

    .pill {
      all: unset; position: absolute; top: 0; left: 0; box-sizing: border-box; pointer-events: auto;
      display: inline-flex; align-items: center; gap: 7px; height: 30px; padding: 0 12px 0 5px; border-radius: 999px;
      background: var(--bg); border: 1px solid var(--accent-border); box-shadow: var(--shadow-sm);
      color: var(--accent-text); font-weight: 600; font-size: 12.5px; white-space: nowrap; cursor: pointer;
      animation: ks-pop 120ms ease-out;
    }
    .pill:hover { background: var(--accent-soft); }
    .pill:focus-visible { box-shadow: 0 0 0 3px rgba(47, 111, 237, 0.35); }
    .pill > svg { width: 20px; height: 20px; flex: none; }
    .pill[disabled] { cursor: default; color: var(--text-3); }

    .tip {
      display: flex; align-items: center; gap: 7px; width: max-content; max-width: min(300px, calc(100vw - 24px));
      padding: 7px 11px; border-radius: 8px; background: var(--tip-bg); color: var(--tip-text);
      font-size: 12px; font-weight: 600; box-shadow: var(--shadow-sm); pointer-events: none !important;
      animation: ks-slide 140ms ease-out;
    }
    .tip svg { width: 14px; height: 14px; flex: none; }
    .tip.occluded svg, .tip.unsupported svg { color: var(--tip-warn); }
    .ks-shake { animation: ks-shake 380ms cubic-bezier(0.36, 0.07, 0.19, 0.97) both; }

    .bar {
      display: flex; align-items: center; gap: 12px; padding: 12px 12px 12px 14px;
      background: var(--bg); border: 1px solid var(--border); border-radius: 14px; box-shadow: var(--shadow);
      animation: ks-slide 160ms ease-out;
    }
    .bar > svg { width: 28px; height: 28px; flex: none; }
    .bar .texts { gap: 1px; }
    .bar .title { font-weight: 600; font-size: 13.5px; }
    .bar .detail { color: var(--text-3); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .bar .actions { display: flex; align-items: center; gap: 4px; flex: none; }
    .bar .status { color: var(--success); font-weight: 600; }
    .bar .status.error { color: var(--danger); }

    .toast {
      display: flex; align-items: center; gap: 10px; width: max-content; max-width: min(460px, calc(100vw - 32px));
      padding: 10px 14px 10px 12px;
      background: var(--bg); border: 1px solid var(--border); border-radius: 12px; box-shadow: var(--shadow);
      animation: ks-slide 160ms ease-out;
    }
    .toast > svg { width: 20px; height: 20px; flex: none; }
    .toast .text b { font-variant-numeric: tabular-nums; }
    .leaving { opacity: 0; transition: opacity 180ms ease; }

    @keyframes ks-pop { from { opacity: 0; transform: var(--pos) scale(0.98); } to { opacity: 1; transform: var(--pos); } }
    @keyframes ks-slide { from { opacity: 0; transform: var(--pos) translateY(-6px); } to { opacity: 1; transform: var(--pos); } }
    @keyframes ks-ring { from { stroke-dashoffset: 37.7; } to { stroke-dashoffset: 0; } }
    @keyframes ks-bar { from { transform: scaleX(0); } to { transform: scaleX(1); } }
    @keyframes ks-shake {
      10%, 90% { translate: -1px 0; } 20%, 80% { translate: 2px 0; }
      30%, 50%, 70% { translate: -4px 0; } 40%, 60% { translate: 4px 0; }
    }
    @media (prefers-reduced-motion: reduce) {
      .dropdown, .bar, .toast, .bubble, .pill, .tip, .ks-shake { animation: none; }
    }
    /* Clickable controls must stay free of opacity, filters and non-translate transforms once shown:
       IntersectionObserver v2 treats those as "not visible" (see uiCheck). The dwell indicators
       (ring, bar) are drawn by children / pseudo-elements, the shake only translates. */
  `;

  let host = null;
  let shadow = null;
  let layer = null;

  function ensureUi() {
    if (!host) {
      // Random tag: a page cannot pre-define it as a custom element or target it with CSS.
      const suffix = Array.from(crypto.getRandomValues(new Uint8Array(4)), (b) => b.toString(16).padStart(2, "0")).join("");
      host = document.createElement(`keystead-${suffix}`);
      for (const [property, value] of HOST_STYLE) host.style.setProperty(property, value, "important");
      shadow = host.attachShadow({ mode: "closed" });
      layer = document.createElement("div");
      layer.className = "layer";
      try {
        // Constructed stylesheets are not subject to the page's CSP (style-src).
        const sheet = new CSSStyleSheet();
        sheet.replaceSync(CSS);
        shadow.adoptedStyleSheets = [sheet];
      } catch {
        const style = document.createElement("style");
        style.textContent = CSS;
        shadow.append(style);
      }
      shadow.append(layer);
      if (typeof host.showPopover === "function") host.setAttribute("popover", "manual");
    }
    placeUi();
    return layer;
  }

  /**
   * Where the host must live: inside the page's open modal <dialog> if there
   * is one (everything outside a modal dialog is inert, even top-layer
   * elements), else directly under <html>.
   */
  function uiParent() {
    try {
      const modals = document.querySelectorAll("dialog:modal");
      if (modals.length) return modals[modals.length - 1];
    } catch {
      // :modal unsupported
    }
    return document.documentElement || document.body;
  }

  /** (Re)attaches the host where it is interactive and shows it in the top layer. */
  function placeUi() {
    if (!host) return;
    const parent = uiParent();
    if (host.parentNode !== parent) {
      parent.append(host);
      raiseUi(true);
    }
  }

  /** Shows the host as a popover: top layer, positioned against the viewport. */
  function raiseUi(force = false) {
    if (!host || typeof host.showPopover !== "function") return;
    try {
      if (host.matches(":popover-open")) {
        if (!force) return;
        host.hidePopover();
      }
      host.showPopover();
    } catch {
      // Not connected – the fixed position and z-index still apply.
    }
  }

  /** Viewport offset of the layer (non-zero only if an ancestor transform captured our fixed host). */
  function layerOrigin() {
    const rect = host ? host.getBoundingClientRect() : null;
    return rect ? { x: rect.left, y: rect.top } : { x: 0, y: 0 };
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  /** Shield with the keyhole cut out (even-odd) on the 512 × 512 grid of assets/keystead.svg. */
  const SHIELD_WITH_KEYHOLE =
    "M256 100 388 150v104c0 82-56 136-132 164-76-28-132-82-132-164V150ZM242.61 265.06a32 32 0 1 1 26.78 0L278 336h-44Z";
  let logoCount = 0;

  function svgNode(tag, attrs) {
    const node = document.createElementNS(SVG_NS, tag);
    for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, String(value));
    return node;
  }

  /** The Keystead mark (white shield with keyhole on a blue→violet rounded tile), as in the desktop app. */
  function logo() {
    // Unique gradient ids: a reference to a gradient inside a hidden SVG (e.g. a hidden field icon) does not render.
    const id = `ks-logo-${++logoCount}`;
    const svg = svgNode("svg", { viewBox: "0 0 512 512", "aria-hidden": "true" });
    const defs = svgNode("defs", {});
    const bg = svgNode("linearGradient", { id: `${id}-bg`, x1: 0, y1: 0, x2: 1, y2: 1 });
    bg.append(svgNode("stop", { offset: 0, "stop-color": "#2f6fed" }), svgNode("stop", { offset: 1, "stop-color": "#7c4dff" }));
    const hl = svgNode("linearGradient", { id: `${id}-hl`, x1: 0, y1: 0, x2: 0, y2: 1 });
    hl.append(
      svgNode("stop", { offset: 0, "stop-color": "#fff", "stop-opacity": 0.22 }),
      svgNode("stop", { offset: 0.55, "stop-color": "#fff", "stop-opacity": 0 }),
    );
    defs.append(bg, hl);
    svg.append(
      defs,
      svgNode("rect", { class: "tile", width: 512, height: 512, rx: 116, fill: `url(#${id}-bg)` }),
      svgNode("rect", { width: 512, height: 512, rx: 116, fill: `url(#${id}-hl)` }),
      svgNode("path", { fill: "#fff", "fill-rule": "evenodd", d: SHIELD_WITH_KEYHOLE }),
    );
    return svg;
  }

  /** A stroke icon on the 24 × 24 grid (like lib/icons.js in the popup). */
  function strokeIcon(d, width = 2) {
    const svg = svgNode("svg", { viewBox: "0 0 24 24", "aria-hidden": "true" });
    svg.append(svgNode("path", { d, stroke: "currentColor", "stroke-width": width, "stroke-linecap": "round", "stroke-linejoin": "round", fill: "none" }));
    return svg;
  }

  const closeIcon = () => strokeIcon("M6 6l12 12M18 6 6 18");
  const eyeIcon = (off) =>
    strokeIcon(
      off
        ? "m3 3 18 18M10.6 5.6c.5-.1.9-.1 1.4-.1 6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.9 3.7M6.6 6.6C4 8.3 2.5 12 2.5 12S6 18.5 12 18.5c1.7 0 3.2-.5 4.5-1.2M9.9 9.9a3 3 0 0 0 4.2 4.2"
        : "M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
      1.8,
    );
  const clockIcon = () => strokeIcon("M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 7.5V12l3 2", 1.8);
  const alertIcon = () => strokeIcon("M12 3.5 21.5 20h-19L12 3.5zM12 10v4.5M12 17.2v.1", 1.8);

  /** Small amber sparkle on the field icon of a password suggestion. */
  function suggestBadge() {
    const svg = svgNode("svg", { class: "badge", viewBox: "0 0 10 10", "aria-hidden": "true" });
    svg.append(
      svgNode("circle", { cx: 5, cy: 5, r: 4.6, fill: "#f59e0b", stroke: "#fff", "stroke-width": 0.8 }),
      svgNode("path", { fill: "#fff", d: "M5 2.1l.75 2.15L7.9 5l-2.15.75L5 7.9l-.75-2.15L2.1 5l2.15-.75z" }),
    );
    return svg;
  }

  /** The dwell progress ring of a dropdown item (see armDwell). */
  function dwellRing() {
    const svg = svgNode("svg", { class: "ring", viewBox: "0 0 16 16", "aria-hidden": "true" });
    svg.append(svgNode("circle", { class: "track", cx: 8, cy: 8, r: 6 }), svgNode("circle", { class: "progress", cx: 8, cy: 8, r: 6 }));
    return svg;
  }

  function avatar(name) {
    const node = el("span", "avatar");
    let hash = 0;
    for (const ch of String(name).trim().toLowerCase()) hash = (hash * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
    node.style.setProperty("--h", String(AVATAR_HUES[hash % AVATAR_HUES.length]));
    const letter = String(name).trim().match(/[\p{L}\p{N}]/u);
    node.textContent = letter ? letter[0].toLocaleUpperCase() : "?";
    return node;
  }

  function place(node, x, y) {
    const origin = layerOrigin();
    const pos = `translate(${Math.round(x - origin.x)}px, ${Math.round(y - origin.y)}px)`;
    node.style.setProperty("--pos", pos);
    node.style.transform = pos;
  }

  /** Places `node` (of `width` × `height`) under `rect`, or above it if there is no room below. */
  function placeNear(node, rect, width, height, gap = 6) {
    const left = Math.min(Math.max(8, rect.left), innerWidth - width - 8);
    let top = rect.bottom + gap;
    if (top + height > innerHeight - 8 && rect.top - gap - height >= 8) top = rect.top - gap - height;
    place(node, left, top);
  }

  // ---------------------------------------------------------------------------
  // Clickjacking protection
  // ---------------------------------------------------------------------------

  // elementFromPoint ignores `pointer-events: none`, so a page could cover our UI
  // with its own opaque top-layer element and let the user's clicks fall through.
  // IntersectionObserver v2 (trackVisibility) reports whether an element is really
  // visible: not covered by anything, no opacity < 1, no filter, no distorting
  // transform – on itself or any ancestor (also across frames).
  const VISIBILITY_SUPPORTED =
    typeof IntersectionObserver === "function" &&
    typeof IntersectionObserverEntry === "function" &&
    "isVisible" in IntersectionObserverEntry.prototype;
  /** Tracked control → { visible, since, trackedAt, armTimer } (since: when it last became visible). */
  const visibility = new WeakMap();
  let visibilityObserver = null;

  function onVisibilityEntries(entries) {
    for (const entry of entries) {
      const state = visibility.get(entry.target);
      if (!state) continue;
      const visible = entry.isVisible === true && entry.isIntersecting;
      if (visible && !state.visible) {
        state.since = entry.time;
        state.visible = true;
        armDwell(entry.target, state);
      } else if (!visible && state.visible) {
        state.visible = false;
        disarmDwell(entry.target, state);
      }
    }
  }

  /**
   * Shows the dwell progress (ring on dropdown items, a thin bar on other
   * `.dwell` controls) from the moment the control became fully visible: the
   * user sees when a click will count. Purely visual – uiCheck decides.
   */
  function armDwell(node, state) {
    if (!node.classList.contains("dwell")) return;
    clearTimeout(state.armTimer);
    node.classList.remove("arming", "armed");
    const elapsed = Math.max(0, performance.now() - state.since);
    if (elapsed >= VISIBLE_DWELL_MS) {
      node.classList.add("armed");
      return;
    }
    node.style.setProperty("--dwell-delay", `${-Math.round(elapsed)}ms`);
    void node.offsetWidth; // restart the animation
    node.classList.add("arming");
    state.armTimer = setTimeout(() => {
      node.classList.remove("arming");
      node.classList.add("armed");
    }, VISIBLE_DWELL_MS - elapsed);
  }

  function disarmDwell(node, state) {
    clearTimeout(state.armTimer);
    node.classList.remove("arming", "armed");
  }

  /** Starts tracking an actionable control (a leaf: a container's own children would count as covering it). */
  function trackVisibility(node) {
    visibility.set(node, { visible: false, since: 0, trackedAt: performance.now(), armTimer: 0 });
    if (!VISIBILITY_SUPPORTED) return;
    try {
      visibilityObserver ??= new IntersectionObserver(onVisibilityEntries, {
        trackVisibility: true,
        delay: 100,
        threshold: [0, 1],
      });
      visibilityObserver.observe(node);
    } catch {
      // Stays "not visible": the control does nothing (filling from the popup still works).
    }
  }

  /** A tracked control was just shown again (display): a click before IntersectionObserver noticed is "early". */
  function markShown(node) {
    const state = visibility.get(node);
    if (state) state.trackedAt = performance.now();
  }

  /** Stops tracking the controls inside `root` (and `root` itself). */
  function untrackVisibility(root) {
    if (!root) return;
    for (const node of [root, ...root.querySelectorAll("button")]) {
      const state = visibility.get(node);
      if (!state) continue;
      clearTimeout(state.armTimer);
      visibility.delete(node);
      visibilityObserver?.unobserve(node);
    }
  }

  /** A computed style value that hides or distorts what the user sees. */
  function hasVisualEffect(style) {
    const none = (value) => !value || value === "none";
    return (
      !none(style.filter) ||
      !none(style.clipPath) ||
      !none(style.getPropertyValue("mask-image")) ||
      !none(style.getPropertyValue("-webkit-mask-image"))
    );
  }

  /**
   * Whether a click on (or a key for) `node` really comes from our visible UI:
   * the page cannot overlay, hide or fade our host to trick the user into using
   * it (clickjacking). `ev` is the click; null for keyboard actions (no
   * position). Returns null if it may act, else why not: "early" (not yet
   * visible for VISIBLE_DWELL_MS), "occluded" (covered, faded or distorted by
   * the page) or "unsupported" (no IntersectionObserver v2: never acts).
   */
  function uiCheck(ev, node) {
    if (!host || !host.isConnected || !node || !node.isConnected) return "occluded";
    const style = getComputedStyle(host);
    if (style.opacity !== "1" || style.visibility !== "visible" || style.display === "none" || hasVisualEffect(style)) return "occluded";
    if (Number(getComputedStyle(document.documentElement).opacity) < 1) return "occluded";
    if (!visibilityObserver) return "unsupported";
    // Apply changes that were computed but not delivered yet (e.g. a cover that just appeared).
    onVisibilityEntries(visibilityObserver.takeRecords());
    const state = visibility.get(node);
    if (!state) return "occluded";
    const now = performance.now();
    if (!state.visible) return now - state.trackedAt < EARLY_GRACE_MS ? "early" : "occluded";
    if (now - state.since < VISIBLE_DWELL_MS) return "early";
    // Keyboard activation has no pointer position.
    if (!ev || (ev.detail === 0 && ev.clientX === 0 && ev.clientY === 0)) return null;
    return document.elementFromPoint(ev.clientX, ev.clientY) === host ? null : "occluded";
  }

  /** For click handlers: true if a trusted click may act; otherwise explains why not (see rejected). */
  function trustedClick(ev, node) {
    if (!ev.isTrusted) return false;
    const reason = uiCheck(ev, node);
    if (!reason) {
      closeTip();
      return true;
    }
    rejected(node, reason);
    return false;
  }

  /** { node, timer } of the feedback tooltip. */
  let tip = null;

  function closeTip() {
    if (!tip) return;
    clearTimeout(tip.timer);
    tip.node.remove();
    tip = null;
  }

  /**
   * Visible feedback for a click the dwell rule rejected (the rule itself is
   * unchanged): the control shakes and a tooltip says "Einen Moment …" or
   * that the page covers our menu. The tooltip sits beside the control's panel
   * so it never covers (and thereby "occludes") one of our controls.
   */
  function rejected(node, reason) {
    if (!alive || !node?.isConnected) return;
    node.classList.remove("ks-shake");
    void node.offsetWidth; // restart the animation
    node.classList.add("ks-shake");
    setTimeout(() => node.classList.remove("ks-shake"), 420);
    closeTip();
    const text = reason === "early" ? t("csDwellWait") : reason === "unsupported" ? t("csDwellUnsupported") : t("csDwellOccluded");
    const tipNode = el("div", `tip ${reason}`);
    tipNode.setAttribute("role", "status");
    tipNode.append(reason === "early" ? clockIcon() : alertIcon(), el("span", "", text));
    ensureUi().append(tipNode);
    // Covered by the page: bring our layer back on top (its controls then wait again).
    if (reason === "occluded") raiseUi(true);
    const anchor = node.closest(".dropdown, .bar, .bubble") || node;
    const rect = anchor.getBoundingClientRect();
    const width = tipNode.offsetWidth;
    const height = tipNode.offsetHeight;
    const left = Math.min(Math.max(8, rect.left + rect.width / 2 - width / 2), innerWidth - width - 8);
    let top = rect.bottom + 8;
    if (top + height > innerHeight - 8) top = Math.max(8, rect.top - height - 8);
    place(tipNode, left, top);
    tip = { node: tipNode, timer: setTimeout(closeTip, 2200) };
  }

  // ---------------------------------------------------------------------------
  // Inline icons
  // ---------------------------------------------------------------------------

  /** field → { button, kind: "login" | "suggest" } */
  const icons = new Map();
  const resizeObserver = new ResizeObserver(() => schedulePosition());
  let positionFrame = 0;
  let positionInterval = 0;

  /** Which fields get which icon: login fields (logins to fill / locked) and new-password fields (suggestions). */
  function wantedIcons() {
    const wanted = new Map();
    if (!pageInfo) return wanted;
    if (pageInfo.state === "locked" || (pageInfo.state === "unlocked" && pageInfo.matches.length > 0)) {
      for (const field of Forms.iconFields(forms)) wanted.set(field, "login");
    }
    if (pageInfo.suggest && ["unlocked", "locked", "not_paired"].includes(pageInfo.state)) {
      for (const target of Forms.suggestionTargets(forms)) if (!wanted.has(target.field)) wanted.set(target.field, "suggest");
    }
    return wanted;
  }

  function renderIcons() {
    if (!alive) return;
    const wanted = wantedIcons();
    for (const [field, entry] of icons) {
      if (wanted.get(field) === entry.kind) continue;
      untrackVisibility(entry.button);
      entry.button.remove();
      icons.delete(field);
      if (!pills.has(field)) resizeObserver.unobserve(field);
      if (dropdown?.field === field) closeDropdown();
      if (bubble?.field === field) closeBubble();
    }
    const greyed = pageInfo?.state !== "unlocked";
    for (const [field, kind] of wanted) {
      let entry = icons.get(field);
      if (!entry) {
        entry = { button: createIcon(field, kind), kind };
        ensureUi().append(entry.button);
        icons.set(field, entry);
        resizeObserver.observe(field);
      }
      entry.button.classList.toggle("locked", greyed);
    }
    updatePositionInterval();
    positionAll();
  }

  function updatePositionInterval() {
    const needed = icons.size > 0 || pills.size > 0;
    if (needed && !positionInterval) {
      // Catches layout shifts that cause neither scroll, resize nor DOM mutations (transitions).
      positionInterval = setInterval(schedulePosition, 1500);
    } else if (!needed && positionInterval) {
      clearInterval(positionInterval);
      positionInterval = 0;
    }
  }

  function createIcon(field, kind) {
    const button = el("button", kind === "suggest" ? "icon suggest" : "icon");
    button.type = "button";
    button.tabIndex = -1;
    button.title = "Keystead";
    button.setAttribute("aria-label", kind === "suggest" ? t("csSuggestIconLabel") : t("csIconLabel"));
    button.append(logo());
    if (kind === "suggest") button.append(suggestBadge());
    // Keep the focus (and caret) in the page's field.
    button.addEventListener("mousedown", (ev) => ev.preventDefault());
    button.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!ev.isTrusted) return;
      if (kind === "suggest") {
        if (bubble?.field === field) closeBubble();
        else if (trustedClick(ev, button)) openBubble(field, { fromIcon: true });
        return;
      }
      if (dropdown?.field === field) closeDropdown();
      else if (trustedClick(ev, button)) openDropdown(field);
    });
    trackVisibility(button);
    return button;
  }

  function schedulePosition() {
    if (positionFrame || !alive) return;
    positionFrame = requestAnimationFrame(() => {
      positionFrame = 0;
      positionAll();
    });
  }

  function positionAll() {
    for (const [field, entry] of icons) positionIcon(field, entry.button);
    for (const entry of pills.values()) positionPill(entry);
    if (dropdown) positionDropdown();
    if (bubble) positionBubble();
  }

  /** The page element at a point, looking into open shadow roots (web component inputs). */
  function deepElementFromPoint(x, y) {
    let hit = document.elementFromPoint(x, y);
    for (let depth = 0; hit && hit.shadowRoot && depth < 32; depth += 1) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    return hit;
  }

  /** True if the point shows the field (not another element on top of it). */
  function showsField(field, x, y) {
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return false;
    const hit = deepElementFromPoint(x, y);
    if (!hit) return false;
    if (hit === host || hit === field) return true;
    return Forms.composedContains(field, hit) || Forms.composedContains(hit, field);
  }

  /** True if `field` is on screen, big enough and not covered by the page at its centre. */
  function fieldShown(field, rect, minWidth = 80) {
    return (
      rect.width >= minWidth &&
      rect.height >= 20 &&
      rect.top >= -1 &&
      rect.bottom <= innerHeight + 1 &&
      rect.left >= -1 &&
      rect.right <= innerWidth + 1 &&
      Forms.isVisible(field)
    );
  }

  function positionIcon(field, button) {
    const rect = field.getBoundingClientRect();
    const size = Math.round(Math.max(14, Math.min(20, rect.height - 12)));
    const pad = Math.round(Math.max(4, Math.min(8, (rect.height - size) / 2)));
    const x = rect.right - size - pad;
    const y = rect.top + (rect.height - size) / 2;
    const visible =
      fieldShown(field, rect) && (showsField(field, rect.left + rect.width / 2, y + size / 2) || showsField(field, x - 4, y + size / 2));
    if (!visible) {
      button.style.display = "none";
      return;
    }
    if (button.style.display !== "block") markShown(button);
    button.style.display = "block";
    button.style.width = `${size}px`;
    button.style.height = `${size}px`;
    place(button, x, y);
  }

  // ---------------------------------------------------------------------------
  // Dropdown
  // ---------------------------------------------------------------------------

  function openDropdown(field) {
    closeDropdown();
    closeBubble();
    const node = el("div", "dropdown");
    node.setAttribute("role", "listbox");
    node.setAttribute("aria-label", "Keystead");
    node.addEventListener("mousedown", (ev) => ev.preventDefault());
    dropdown = { field, node, index: 0 };
    ensureUi().append(node);
    raiseUi(true); // start above any top-layer element of the page
    renderDropdown();
    try {
      field.focus({ preventScroll: true });
    } catch {
      // ignore
    }
    // The vault may have been unlocked (or changed) since the page loaded.
    if (!pageInfo || pageInfo.state !== "unlocked" || Date.now() - pageInfo.at > 5000) requestPageInfo();
  }

  function closeDropdown() {
    if (!dropdown) return;
    closeTip();
    untrackVisibility(dropdown.node);
    dropdown.node.remove();
    dropdown = null;
  }

  function dropdownMatches() {
    return pageInfo?.state === "unlocked" ? pageInfo.matches : [];
  }

  function renderDropdown() {
    if (!dropdown) return;
    const { node } = dropdown;
    untrackVisibility(node);
    node.replaceChildren();

    const head = el("div", "dd-head");
    head.append(logo(), el("span", "", "Keystead"), el("span", "host", location.hostname.replace(/^www\./, "")));
    node.append(head);

    if (pageInfo?.state === "locked") {
      const box = el("div", "dd-locked");
      box.append(el("div", "title", t("csLocked")), el("div", "hint", t("csLockedHint")));
      const unlock = el("button", "btn btn-primary dwell", t("csUnlock"));
      unlock.type = "button";
      unlock.addEventListener("click", (ev) => {
        if (!trustedClick(ev, unlock)) return;
        closeDropdown();
        send({ type: "cs:open-popup" }).catch(() => undefined);
      });
      trackVisibility(unlock);
      box.append(unlock);
      node.append(box);
      positionDropdown();
      return;
    }

    const matches = dropdownMatches();
    if (!matches.length) {
      node.append(el("div", "dd-empty", pageInfo ? t("csNoMatches") : "…"));
      positionDropdown();
      return;
    }
    if (pageInfo.insecure) node.append(el("div", "dd-warn", t("csInsecureWarning")));
    dropdown.index = Math.min(dropdown.index, matches.length - 1);
    matches.forEach((match, i) => {
      const item = el("button", i === dropdown.index ? "item dwell active" : "item dwell");
      item.type = "button";
      item.tabIndex = -1;
      item.setAttribute("role", "option");
      const texts = el("span", "texts");
      texts.append(el("span", "name", match.name || match.username || "—"), el("span", "user", match.username || t("csNoUsername")));
      item.append(avatar(match.name || match.username), texts, el("span", "fill-hint", t("csFill")), dwellRing());
      item.addEventListener("mousemove", () => highlight(i));
      item.addEventListener("click", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        if (!trustedClick(ev, item)) return;
        chooseMatch(match);
      });
      trackVisibility(item);
      node.append(item);
    });
    positionDropdown();
  }

  function highlight(index) {
    if (!dropdown || dropdown.index === index) return;
    dropdown.index = index;
    dropdown.node.querySelectorAll(".item").forEach((item, i) => item.classList.toggle("active", i === index));
  }

  function positionDropdown() {
    if (!dropdown) return;
    const { field, node } = dropdown;
    if (!field.isConnected || !Forms.isVisible(field)) {
      closeDropdown();
      return;
    }
    const rect = field.getBoundingClientRect();
    const width = Math.min(Math.max(rect.width, 264), 360, innerWidth - 16);
    node.style.width = `${width}px`;
    placeNear(node, rect, width, node.offsetHeight);
  }

  /** Keys while the dropdown or the suggestion bubble is open (window, capture phase, so the page doesn't submit). */
  function onDropdownKeys(ev) {
    if (!ev.isTrusted) return;
    if (bubble && ev.key === "Escape" && eventTarget(ev) === bubble.field) {
      quietFields.add(bubble.field);
      closeBubble();
      ev.preventDefault();
      ev.stopImmediatePropagation();
      return;
    }
    if (!dropdown || eventTarget(ev) !== dropdown.field) return;
    const matches = dropdownMatches();
    let handled = true;
    switch (ev.key) {
      case "ArrowDown":
        if (matches.length) highlight((dropdown.index + 1) % matches.length);
        break;
      case "ArrowUp":
        if (matches.length) highlight((dropdown.index - 1 + matches.length) % matches.length);
        break;
      case "Enter": {
        if (!matches.length) {
          handled = false;
          break;
        }
        // No pointer position: the highlighted item itself must be visible (else the key is swallowed).
        const item = dropdown.node.querySelectorAll(".item")[dropdown.index];
        const reason = uiCheck(null, item);
        if (!reason) chooseMatch(matches[dropdown.index]);
        else rejected(item, reason);
        break;
      }
      case "Escape":
        closeDropdown();
        break;
      default:
        handled = false;
        if (ev.key === "Tab") closeDropdown();
    }
    if (handled) {
      ev.preventDefault();
      ev.stopImmediatePropagation();
    }
  }

  async function chooseMatch(match) {
    const field = dropdown?.field;
    closeDropdown();
    if (!field) return;
    try {
      const credentials = await send({ type: "cs:fill-request", itemId: match.id });
      const form = Forms.formForElement(refreshForms(), field);
      const filled = form ? fillStored(form, credentials) : 0;
      if (!filled) toast(t("csFillFailed"));
    } catch (err) {
      toast(errorMessage(err.code));
    }
  }

  // ---------------------------------------------------------------------------
  // Password suggestion (signup / password change)
  // ---------------------------------------------------------------------------

  /** Open bubble: { field, node, kind: "suggest" | "hint", password, revealed, secret, eye, use }. */
  let bubble = null;
  /** field → { password }: the suggestion shown for it (the same one when the bubble opens again). */
  let suggestions = new WeakMap();
  /** Fields whose bubble the user closed, answered or typed into: no bubble on focus any more (the icon still opens it). */
  let quietFields = new WeakSet();
  /** A suggestion field focused before the page info arrived. */
  let pendingSuggestion = null;

  function canSuggest(field) {
    return pageInfo?.state === "unlocked" && pageInfo.suggest && !quietFields.has(field);
  }

  /**
   * A trusted focus (or click) on a field: shows the suggestion bubble if it
   * is the new-password field of a signup / change form. Only right after a
   * user gesture (click, Tab …): a page that focuses its signup field by
   * script on load gets no bubble – the field icon still offers it.
   */
  function maybeSuggest(field) {
    if (!field || bubble?.field === field || quietFields.has(field)) return;
    if (navigator.userActivation && !navigator.userActivation.isActive) return;
    const target = Forms.suggestionTargets(refreshForms()).find((x) => x.field === field);
    if (!target) return;
    if (!pageInfo) {
      pendingSuggestion = field;
      requestPageInfo();
      return;
    }
    if (canSuggest(field)) openBubble(field);
  }

  function openBubble(field, { fromIcon = false } = {}) {
    closeDropdown();
    closeBubble();
    const state = pageInfo?.state;
    if (state === "unlocked" && pageInfo.suggest) renderSuggestion(field);
    else if (fromIcon && (state === "locked" || state === "not_paired")) renderSuggestionHint(field, state);
  }

  function closeBubble() {
    if (!bubble) return;
    closeTip();
    untrackVisibility(bubble.node);
    bubble.node.remove();
    bubble = null;
  }

  function bubbleCloseButton(field) {
    const close = el("button", "btn btn-ghost btn-icon");
    close.type = "button";
    close.title = t("csClose");
    close.setAttribute("aria-label", t("csClose"));
    close.append(closeIcon());
    close.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!ev.isTrusted) return;
      quietFields.add(field);
      closeBubble();
    });
    return close;
  }

  function newBubbleNode(className, label) {
    const node = el("div", className);
    node.setAttribute("role", "dialog");
    node.setAttribute("aria-label", label);
    // Keep the focus (and caret) in the page's field.
    node.addEventListener("mousedown", (ev) => ev.preventDefault());
    return node;
  }

  function renderSuggestion(field) {
    const node = newBubbleNode("bubble", t("csSuggestTitle"));
    const head = el("div", "bubble-head");
    head.append(logo(), el("span", "title", t("csSuggestTitle")), bubbleCloseButton(field));

    const secret = el("span", "secret masked", "…");
    const eye = el("button", "btn btn-ghost btn-icon");
    eye.type = "button";
    eye.setAttribute("aria-pressed", "false");
    eye.title = t("csSuggestShow");
    eye.setAttribute("aria-label", t("csSuggestShow"));
    eye.append(eyeIcon(false));
    const box = el("div", "secret-box");
    box.append(secret, eye);

    const use = el("button", "btn btn-primary dwell", t("csSuggestUse"));
    use.type = "button";
    use.disabled = true;
    const row = el("div", "bubble-row");
    row.append(el("div", "bubble-note", t("csSuggestNote")), use);
    node.append(head, box, row);

    const current = { field, node, kind: "suggest", password: null, revealed: false, secret, eye, use };
    eye.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!ev.isTrusted || !current.password) return;
      current.revealed = !current.revealed;
      eye.setAttribute("aria-pressed", String(current.revealed));
      const label = current.revealed ? t("csSuggestHide") : t("csSuggestShow");
      eye.title = label;
      eye.setAttribute("aria-label", label);
      eye.replaceChildren(eyeIcon(current.revealed));
      renderSecret(current);
    });
    use.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!current.password || !trustedClick(ev, use)) return;
      useSuggestion(current);
    });
    trackVisibility(use);

    bubble = current;
    ensureUi().append(node);
    raiseUi(true); // start above any top-layer element of the page
    positionBubble();
    loadSuggestion(current);
  }

  /** The value in the bubble: dots until revealed; revealed with digits and symbols coloured. */
  function renderSecret(current) {
    const { secret, password } = current;
    secret.classList.remove("error");
    secret.classList.toggle("masked", !current.revealed);
    if (!current.revealed) {
      secret.textContent = "•".repeat(Math.min(password.length, 20));
      return;
    }
    secret.replaceChildren();
    let run = "";
    let runClass = null;
    const flush = () => {
      if (!run) return;
      secret.append(runClass ? el("span", runClass, run) : document.createTextNode(run));
      run = "";
    };
    for (const ch of password) {
      const cls = /\d/.test(ch) ? "d" : /\p{L}/u.test(ch) ? null : "s";
      if (cls !== runClass) {
        flush();
        runClass = cls;
      }
      run += ch;
    }
    flush();
  }

  async function loadSuggestion(current) {
    let suggestion = suggestions.get(current.field);
    if (!suggestion) {
      try {
        const result = await send({ type: "cs:suggest-password" });
        if (typeof result?.password !== "string" || !result.password) throw new ContentError("internal");
        suggestion = { password: result.password };
        suggestions.set(current.field, suggestion);
      } catch (err) {
        if (bubble !== current) return;
        if (err.code === "locked" || err.code === "not_paired") {
          // Locked meanwhile (e.g. in the app): explain instead.
          if (pageInfo) pageInfo.state = err.code;
          closeBubble();
          renderSuggestionHint(current.field, err.code);
          renderIcons();
        } else if (err.code === "disabled" || err.code === "inactive") {
          closeBubble();
        } else {
          current.secret.classList.remove("masked");
          current.secret.classList.add("error");
          current.secret.textContent = t("csSuggestFailed");
        }
        return;
      }
    }
    if (bubble !== current) return;
    current.password = suggestion.password;
    renderSecret(current);
    current.use.disabled = false;
    positionBubble();
  }

  /**
   * "Verwenden": the field and its confirmation get the password; it counts as
   * the user's own input (save prompt after submitting) and goes into the
   * generator history of the vault it was suggested for (the service worker
   * checks that it suggested it in this tab).
   */
  function useSuggestion(current) {
    const { field, password } = current;
    // Answered: filling moves the focus between the fields, which must not open it again.
    quietFields.add(field);
    suggestions.delete(field);
    closeBubble();
    const target = Forms.suggestionTargets(refreshForms()).find((x) => x.field === field) || { field, confirm: [] };
    const filled = Forms.fillSuggestion(target, password);
    for (const f of filled) userValues.set(f, password);
    if (!filled.length) {
      toast(t("csFillFailed"));
      return;
    }
    const form = Forms.formForElement(forms, field);
    const credentials = form ? userCredentials(form) : null;
    if (credentials) snapshot = { form, credentials, at: Date.now(), href: location.href };
    toast(t("csSuggestUsed"));
    send({ type: "cs:suggestion-used", password }).catch(() => undefined);
  }

  /** Locked / not connected, after a click on the field icon: how to get suggestions. */
  function renderSuggestionHint(field, state) {
    const pair = state === "not_paired";
    const node = newBubbleNode("bubble hint", t("csSuggestTitle"));
    const action = el("button", "btn btn-primary dwell", pair ? t("csConnect") : t("csUnlock"));
    action.type = "button";
    action.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!trustedClick(ev, action)) return;
      closeBubble();
      send({ type: "cs:open-popup" }).catch(() => undefined);
    });
    trackVisibility(action);
    node.append(logo(), el("span", "hint-text", pair ? t("csSuggestHintPair") : t("csSuggestHintLocked")), action, bubbleCloseButton(field));
    bubble = { field, node, kind: "hint" };
    ensureUi().append(node);
    raiseUi(true);
    positionBubble();
  }

  /**
   * Beside the field if there is room – the fields below it (confirmation,
   * submit button) stay visible and clickable –, else under (or above) it.
   */
  function positionBubble() {
    if (!bubble) return;
    const { field, node } = bubble;
    if (!field.isConnected || !Forms.isVisible(field)) {
      closeBubble();
      return;
    }
    const rect = field.getBoundingClientRect();
    const side = bubble.kind === "hint" ? 484 : 310;
    if (rect.right + 10 + side <= innerWidth - 8) {
      node.style.width = `${side}px`;
      const top = Math.min(Math.max(8, rect.top - 6), innerHeight - node.offsetHeight - 8);
      place(node, rect.right + 10, top);
      return;
    }
    const width = Math.min(Math.max(rect.width, side), 360, innerWidth - 16);
    node.style.width = `${width}px`;
    placeNear(node, rect, width, node.offsetHeight);
  }

  // ---------------------------------------------------------------------------
  // 2FA: one-time code fields
  // ---------------------------------------------------------------------------

  /** This frame's view of the tab's 2FA offer: { available, until, checkedAt, href }. */
  let otpOffer = null;
  let otpOfferRunning = false;
  let otpExpiryTimer = 0;
  /** OTP field → { node, target } of the "2FA-Code einfügen" pill. */
  const pills = new Map();

  async function checkOtpOffer(force = false) {
    if (otpOfferRunning) return;
    const fresh = otpOffer && otpOffer.href === location.href && Date.now() - otpOffer.checkedAt < OTP_OFFER_RECHECK_MS;
    if (fresh && !force) return;
    otpOfferRunning = true;
    try {
      const result = await send({ type: "cs:otp-offer" });
      const expiresIn = Number(result?.expiresIn);
      otpOffer = {
        available: result?.available === true,
        until: Date.now() + (Number.isFinite(expiresIn) ? expiresIn : 0),
        checkedAt: Date.now(),
        href: location.href,
      };
    } catch {
      otpOffer = { available: false, until: 0, checkedAt: Date.now(), href: location.href };
    } finally {
      otpOfferRunning = false;
    }
    renderPills();
  }

  function otpOfferActive() {
    return !!otpOffer?.available && Date.now() < otpOffer.until;
  }

  /** "2FA-Code einfügen" next to every visible, still empty one-time code field while the tab has a 2FA offer. */
  function renderPills() {
    if (!alive) return;
    const wanted = otpOfferActive() ? otpTargets.filter((target) => !target.group.some((box) => box.value)) : [];
    for (const [field, entry] of pills) {
      if (wanted.some((target) => target.field === field)) continue;
      untrackVisibility(entry.node);
      entry.node.remove();
      pills.delete(field);
      if (!icons.has(field)) resizeObserver.unobserve(field);
    }
    for (const target of wanted) {
      const entry = pills.get(target.field);
      if (entry) entry.target = target;
      else createPill(target);
    }
    clearTimeout(otpExpiryTimer);
    if (pills.size) otpExpiryTimer = setTimeout(renderPills, Math.max(0, otpOffer.until - Date.now()) + 50);
    updatePositionInterval();
    for (const entry of pills.values()) positionPill(entry);
  }

  function createPill(target) {
    const node = el("button", "pill dwell");
    node.type = "button";
    node.tabIndex = -1;
    node.append(logo(), el("span", "", t("csOtpInsert")));
    const entry = { node, target };
    node.addEventListener("mousedown", (ev) => ev.preventDefault());
    node.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (node.disabled || !trustedClick(ev, node)) return;
      insertOtp(entry);
    });
    trackVisibility(node);
    ensureUi().append(node);
    pills.set(target.field, entry);
    resizeObserver.observe(target.field);
  }

  /** Union of the rects of an OTP target's boxes. */
  function targetRect(target) {
    const rects = target.group.map((box) => box.getBoundingClientRect());
    const left = Math.min(...rects.map((r) => r.left));
    const top = Math.min(...rects.map((r) => r.top));
    const right = Math.max(...rects.map((r) => r.right));
    const bottom = Math.max(...rects.map((r) => r.bottom));
    return { left, top, right, bottom, width: right - left, height: bottom - top };
  }

  /** Right of the field if there is room, else below it (above if there is no room below). */
  function positionPill(entry) {
    const { node, target } = entry;
    const field = target.field;
    const rect = targetRect(target);
    const visible = fieldShown(field, rect, 24) && showsField(field, field.getBoundingClientRect().left + 4, rect.top + rect.height / 2);
    if (!visible) {
      node.style.display = "none";
      return;
    }
    if (node.style.display !== "inline-flex") markShown(node);
    node.style.display = "inline-flex";
    const width = node.offsetWidth;
    const height = node.offsetHeight;
    if (rect.right + 8 + width <= innerWidth - 8) place(node, rect.right + 8, rect.top + (rect.height - height) / 2);
    else placeNear(node, rect, width, height);
  }

  async function insertOtp(entry) {
    entry.node.disabled = true;
    let result;
    try {
      result = await send({ type: "cs:otp-fill" });
    } catch (err) {
      entry.node.disabled = false;
      if (err.code === "not_found") {
        otpOffer = { available: false, until: 0, checkedAt: Date.now(), href: location.href };
        renderPills();
        toast(t("csOtpGone"));
      } else {
        toast(errorMessage(err.code));
      }
      return;
    }
    // The offer is used up (the service worker dropped it).
    otpOffer = { available: false, until: 0, checkedAt: Date.now(), href: location.href };
    const ok = Forms.fillOtp(entry.target, result?.code);
    renderPills();
    toast(ok ? t("csOtpInserted") : t("csFillFailed"));
  }

  // ---------------------------------------------------------------------------
  // Toasts and the save / update bar
  // ---------------------------------------------------------------------------

  let toastNode = null;
  let toastTimer = 0;
  let toastTicker = 0;

  /** Shows a short message at the top right; returns its text node (or null). */
  function toast(text, { duration = 4500, icon = null } = {}) {
    if (!text || !alive) return null;
    const root = ensureUi();
    toastNode?.remove();
    clearTimeout(toastTimer);
    clearInterval(toastTicker);
    const node = el("div", "toast");
    node.setAttribute("role", "status");
    const textNode = el("span", "text", text);
    node.append(icon || logo(), textNode);
    root.append(node);
    raiseUi(true);
    toastNode = node;
    place(node, Math.max(8, innerWidth - node.offsetWidth - 16), 16);
    toastTimer = setTimeout(() => {
      clearInterval(toastTicker);
      node.classList.add("leaving");
      setTimeout(() => node.remove(), 200);
      if (toastNode === node) toastNode = null;
    }, duration);
    return textNode;
  }

  /** "2FA-Code kopiert – einfügen mit Strg+V (noch N s gültig)", counting down while shown. */
  function totpToast(remaining) {
    const keys = /mac|iphone|ipad/i.test(navigator.userAgentData?.platform || navigator.platform || "") ? "⌘V" : t("csPasteKeys");
    if (!Number.isInteger(remaining) || remaining < 1) {
      toast(t("csTotpCopiedShort", [keys]), { duration: TOTP_TOAST_MS });
      return;
    }
    const started = Date.now();
    const text = (left) => t("csTotpCopied", [keys, String(left)]);
    const textNode = toast(text(remaining), { duration: TOTP_TOAST_MS });
    if (!textNode) return;
    toastTicker = setInterval(() => {
      const left = remaining - Math.floor((Date.now() - started) / 1000);
      if (!textNode.isConnected || left < 1) {
        clearInterval(toastTicker);
        return;
      }
      textNode.textContent = text(left);
    }, 1000);
  }

  /** { node, info, timer } while the bar is shown. */
  let bar = null;

  function closeBar() {
    if (!bar) return;
    closeTip();
    clearTimeout(bar.timer);
    untrackVisibility(bar.node);
    bar.node.remove();
    bar = null;
  }

  function positionBar() {
    if (!bar) return;
    const width = Math.min(600, innerWidth - 24);
    bar.node.style.width = `${width}px`;
    place(bar.node, (innerWidth - width) / 2, 12);
  }

  function showSaveBar(info) {
    if (!isTop || !info || typeof info.id !== "string") return;
    if (bar?.info.id === info.id) return;
    closeBar();
    const isUpdate = info.kind === "update";
    const node = el("div", "bar");
    node.setAttribute("role", "dialog");
    node.setAttribute("aria-label", isUpdate ? t("csUpdateTitle") : t("csSaveTitle"));

    const texts = el("div", "texts");
    const username = typeof info.username === "string" && info.username ? info.username : t("csNoUsername");
    // The host is always shown: an update prompt from a sibling subdomain must be recognisable.
    const detail = isUpdate ? [info.itemName, username, info.host].filter(Boolean).join(" · ") : `${username} · ${info.host}`;
    texts.append(el("div", "title", isUpdate ? t("csUpdateTitle") : t("csSaveTitle")), el("div", "detail", detail));

    const actions = el("div", "actions");
    const save = el("button", "btn btn-primary dwell", isUpdate ? t("csUpdate") : t("csSave"));
    save.type = "button";
    save.addEventListener("click", (ev) => decide("save", ev));
    actions.append(save);
    if (!isUpdate) {
      const never = el("button", "btn btn-ghost", t("csNever"));
      never.type = "button";
      never.addEventListener("click", (ev) => decide("never", ev));
      actions.append(never);
    }
    const close = el("button", "btn btn-ghost btn-icon");
    close.type = "button";
    close.title = t("csClose");
    close.setAttribute("aria-label", t("csClose"));
    close.append(closeIcon());
    close.addEventListener("click", (ev) => decide("dismiss", ev));
    actions.append(close);

    for (const button of actions.querySelectorAll("button")) trackVisibility(button);
    node.append(logo(), texts, actions);
    ensureUi().append(node);
    raiseUi(true);
    bar = { node, info, texts, actions, timer: setTimeout(closeBar, BAR_TTL_MS) };
    positionBar();
  }

  function setBarStatus(text, isError) {
    if (!bar) return;
    untrackVisibility(bar.actions);
    bar.actions.replaceChildren(el("span", isError ? "status error" : "status", text));
  }

  async function decide(action, ev) {
    if (!bar || !trustedClick(ev, ev.currentTarget)) return;
    const current = bar;
    current.actions.querySelectorAll("button").forEach((b) => {
      b.disabled = true;
    });
    try {
      const result = await send({ type: "cs:save-decision", id: current.info.id, action });
      if (bar !== current) return;
      if (action === "dismiss") {
        closeBar();
        return;
      }
      if (action === "never") setBarStatus(t("csNeverDone"), false);
      else setBarStatus(result?.kind === "update" ? t("csUpdated") : t("csSaved"), false);
      clearTimeout(current.timer);
      current.timer = setTimeout(closeBar, 2200);
    } catch (err) {
      if (bar !== current) return;
      if (err.code === "not_found" || err.code === "inactive") {
        closeBar();
        return;
      }
      if (err.code === "locked") {
        setBarStatus(t("csSaveLocked"), true);
        const unlock = el("button", "btn btn-primary dwell", t("csUnlock"));
        unlock.type = "button";
        unlock.addEventListener("click", (e) => {
          if (!trustedClick(e, unlock)) return;
          send({ type: "cs:open-popup" }).catch(() => undefined);
          restoreBarActions(current);
        });
        trackVisibility(unlock);
        current.actions.append(unlock);
      } else {
        setBarStatus(t("csSaveFailed"), true);
        clearTimeout(current.timer);
        current.timer = setTimeout(closeBar, 4000);
      }
    }
  }

  /** Shows the bar's buttons again (after the user went to unlock the vault). */
  function restoreBarActions(current) {
    if (bar !== current) return;
    const info = current.info;
    closeBar();
    showSaveBar(info);
  }

  // ---------------------------------------------------------------------------
  // Capturing submitted logins
  // ---------------------------------------------------------------------------

  function submitCredentials(credentials) {
    if (!credentials) return;
    if (!credentials.password) {
      // Username-only step of a multi-step login: remember the username for the password page.
      if (credentials.username) send({ type: "cs:username-step", username: credentials.username }).catch(() => undefined);
      return;
    }
    const key = `${credentials.username}\u0000${credentials.password}`;
    const now = Date.now();
    if (key === lastCapture.key && now - lastCapture.at < CAPTURE_DEDUP_MS) return;
    lastCapture = { key, at: now };
    setTimeout(() => {
      if (lastCapture.key === key) lastCapture = { key: "", at: 0 };
    }, CAPTURE_DEDUP_MS);
    send({ type: "cs:capture", username: credentials.username, password: credentials.password }).catch(() => undefined);
  }

  /**
   * What the user entered in `form`: the credentials, but only if the password
   * is one the user typed/pasted (or inserted from the generator or a
   * suggestion) and no script changed since; null otherwise. Username-only
   * steps are returned as is.
   */
  function userCredentials(form) {
    const credentials = form ? Forms.readCredentials(form) : null;
    if (!credentials?.password) return credentials;
    const fromUser = form.passwords.some((f) => f.value === credentials.password && userValues.get(f) === f.value);
    return fromUser ? credentials : null;
  }

  /** Fills stored credentials; the fields no longer hold user input. */
  function fillStored(form, credentials) {
    for (const field of [form.username, ...form.passwords]) if (field) userValues.delete(field);
    return Forms.fillForm(form, credentials);
  }

  function captureForm(form) {
    snapshot = null;
    if (form) submitCredentials(userCredentials(form));
  }

  /** SPA logins: the form disappeared or the URL changed after the user typed credentials. */
  function checkSnapshot() {
    if (!snapshot) return;
    if (Date.now() - snapshot.at > SNAPSHOT_TTL_MS) {
      snapshot = null;
      return;
    }
    const fields = [...snapshot.form.passwords, snapshot.form.username].filter(Boolean);
    const gone = !fields.some((f) => f.isConnected && Forms.isVisible(f));
    if (gone || location.href !== snapshot.href) {
      const { credentials } = snapshot;
      snapshot = null;
      submitCredentials(credentials);
    }
  }

  function onSubmit(ev) {
    const target = ev.target;
    if (!target || target.nodeType !== 1) return;
    const form = refreshForms().find((f) => Forms.composedContains(target, f.scope));
    captureForm(form);
  }

  function onKeyDown(ev) {
    if (ev.key !== "Enter" || !ev.isTrusted || dropdown) return;
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    captureForm(Forms.formForElement(refreshForms(), target));
  }

  function onClick(ev) {
    if (!ev.isTrusted || (host && ev.composedPath().includes(host))) return;
    const target = eventTarget(ev);
    if (Forms.isTextEntry(target)) {
      // A click into an already focused signup field (e.g. focused by the page on load).
      if (deepActiveElement() === target) maybeSuggest(target);
      return;
    }
    const button = Forms.submitButtonFor(target);
    if (!button) return;
    const current = refreshForms();
    const form = Forms.formForElement(current, button) || (current.length === 1 ? current[0] : null);
    captureForm(form);
  }

  function onInput(ev) {
    if (!ev.isTrusted) return; // our own fills and the page's synthetic events are untrusted
    if (noteShadowPath(ev)) refreshForms();
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    userValues.set(target, target.value);
    if (bubble && bubble.field === target) {
      // The user types a password of their own.
      quietFields.add(target);
      closeBubble();
    }
    if (pills.size) renderPills();
    const form = Forms.formForElement(forms, target);
    if (!form) return;
    const credentials = userCredentials(form);
    snapshot = credentials ? { form, credentials, at: Date.now(), href: location.href } : null;
  }

  // ---------------------------------------------------------------------------
  // Focus tracking (target of generated passwords, suggestions)
  // ---------------------------------------------------------------------------

  function onFocusIn(ev) {
    // Synthetic focus events would let a frame (e.g. a cross-origin ad) claim the
    // "focused frame" slot and receive the next generated password.
    if (!ev.isTrusted) return;
    if (noteShadowPath(ev)) scheduleScan();
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    if (lastReportedFocus !== target) {
      lastReportedFocus = target;
      send({ type: "cs:focus" }).catch(() => undefined);
    }
    maybeSuggest(target);
  }

  function onFocusOut(ev) {
    // Report the field again when it regains the focus: another frame may have claimed it meanwhile.
    const target = eventTarget(ev);
    if (ev.isTrusted && target === lastReportedFocus) lastReportedFocus = null;
    // Clicks into our UI keep the focus (mousedown is prevented), so these are real blurs.
    if (dropdown && target === dropdown.field) {
      setTimeout(() => {
        if (dropdown && deepActiveElement() !== dropdown.field) closeDropdown();
      }, 120);
    }
    if (bubble && target === bubble.field) {
      setTimeout(() => {
        if (bubble && deepActiveElement() !== bubble.field) closeBubble();
      }, 150);
    }
    if (pendingSuggestion === target) pendingSuggestion = null;
  }

  function onWindowBlur(ev) {
    if (ev.target === window) lastReportedFocus = null;
  }

  function onContextMenu(ev) {
    if (!ev.isTrusted) return;
    const target = eventTarget(ev);
    lastContextTarget = Forms.isTextEntry(target) ? target : null;
  }

  function onOutsidePointer(ev) {
    if (!dropdown && !bubble) return;
    const path = ev.composedPath();
    if (host && path.includes(host)) return;
    if (dropdown && !path.includes(dropdown.field)) closeDropdown();
    if (bubble && !path.includes(bubble.field)) closeBubble();
  }

  function fillGeneratedPassword(msg) {
    if (typeof msg.password !== "string" || !msg.password) return 0;
    let target;
    if (msg.target === "context") target = lastContextTarget;
    else {
      // Only the field that has the focus right now (it keeps it while the popup is
      // open). A frame that does not own the focus has no focused field and fills
      // nothing – never a field that was focused at some earlier point.
      const active = deepActiveElement();
      target = Forms.isTextEntry(active) ? active : null;
    }
    if (!target || !target.isConnected) return 0;
    const filled = Forms.fillGenerated(target, msg.password, { shadowRoots: knownRoots() });
    if (filled) {
      // The user chose to insert it: offer to save it like a typed password.
      const form = Forms.formForElement(refreshForms(), target);
      for (const field of form ? [target, ...form.passwords] : [target]) {
        if (field.value === msg.password) userValues.set(field, msg.password);
      }
      if (bubble?.field === target) closeBubble();
    }
    return filled;
  }

  // ---------------------------------------------------------------------------
  // Messages from the service worker
  // ---------------------------------------------------------------------------

  function handleAutofill(msg, sendResponse) {
    if (typeof msg.nonce !== "string") return false;
    // Autofill only fills the top frame and frames of the same origin.
    if (typeof msg.origin !== "string" || msg.origin !== location.origin) return false;
    const [form] = Forms.rankForms(refreshForms(), deepActiveElement());
    if (!form) return false; // no answer: another frame may have the form
    sendResponse({ hasForm: true });
    send({ type: "cs:fill-request", nonce: msg.nonce })
      .then((credentials) => {
        if (!fillStored(form, credentials)) toast(t("csFillFailed"));
      })
      .catch((err) => {
        if (err.code === "insecure") toast(t("csInsecureBlocked"));
      });
    return false;
  }

  function onMessage(msg, sender, sendResponse) {
    if (!alive || sender.id !== chrome.runtime.id || !msg || typeof msg.type !== "string") return false;
    switch (msg.type) {
      case "bg:autofill":
        return handleAutofill(msg, sendResponse);
      case "bg:fill-generated":
        sendResponse({ filled: fillGeneratedPassword(msg) });
        return false;
      case "bg:save-bar":
        showSaveBar(msg.bar);
        return false;
      case "bg:refresh":
        pageInfo = null;
        otpOffer = null;
        if (forms.length) requestPageInfo();
        else scheduleScan();
        if (otpTargets.length) checkOtpOffer(true);
        return false;
      case "bg:toast":
        if (typeof msg.key === "string" && /^cs[A-Z][A-Za-z]+$/.test(msg.key)) toast(t(msg.key));
        return false;
      case "bg:totp-copied":
        totpToast(msg.remaining);
        return false;
      case "bg:otp-offer":
        // A login with 2FA was just filled in this tab: one-time code fields may offer the code.
        otpOffer = null;
        if (otpTargets.length) checkOtpOffer(true);
        return false;
      default:
        return false;
    }
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  const observer = new MutationObserver((mutations) => {
    let relevant = false;
    for (const m of mutations) {
      if (m.target === host) continue;
      if (m.type === "childList") {
        const ours = [...m.addedNodes, ...m.removedNodes].every((n) => n === host);
        if (ours && (m.addedNodes.length || m.removedNodes.length)) continue;
        for (const node of m.addedNodes) if (node.nodeType === 1 && node !== host) addedSubtrees.push(node);
      }
      relevant = true;
    }
    if (relevant) scheduleScan();
  });

  function onVisibility() {
    if (document.hidden) return;
    scheduleScan();
    if (forms.length && (!pageInfo || Date.now() - pageInfo.at > PAGE_INFO_MAX_AGE_MS)) requestPageInfo();
  }

  function onViewportChange() {
    if (icons.size || dropdown || bubble || pills.size) schedulePosition();
    if (bar) positionBar();
    if (tip) closeTip();
  }

  function teardown() {
    if (!alive) return;
    alive = false;
    observer.disconnect();
    resizeObserver.disconnect();
    visibilityObserver?.disconnect();
    clearTimeout(scanTimer);
    clearTimeout(toastTimer);
    clearInterval(toastTicker);
    clearTimeout(otpExpiryTimer);
    clearInterval(positionInterval);
    clearInterval(sweepTimer);
    if (tip) clearTimeout(tip.timer);
    if (positionFrame) cancelAnimationFrame(positionFrame);
    if (bar) clearTimeout(bar.timer);
    for (const cleanup of cleanups.splice(0)) cleanup();
    for (const root of shadowRoots) root.removeEventListener("submit", onSubmit, true);
    shadowRoots.clear();
    try {
      chrome.runtime.onMessage.removeListener(onMessage);
    } catch {
      // Context already invalidated.
    }
    host?.remove();
    icons.clear();
    pills.clear();
    dropdown = null;
    bubble = null;
    bar = null;
    tip = null;
    snapshot = null;
    userValues = new WeakMap();
    suggestions = new WeakMap();
    quietFields = new WeakSet();
    if (globalThis.__keysteadContent === api) delete globalThis.__keysteadContent;
  }

  chrome.runtime.onMessage.addListener(onMessage);
  listen(document, "submit", onSubmit, true);
  listen(document, "keydown", onKeyDown, true);
  listen(window, "keydown", onDropdownKeys, true);
  listen(document, "click", onClick, true);
  listen(document, "input", onInput, true);
  listen(document, "focusin", onFocusIn, true);
  listen(document, "focusout", onFocusOut, true);
  listen(window, "blur", onWindowBlur);
  listen(document, "contextmenu", onContextMenu, true);
  listen(document, "mousedown", onOutsidePointer, true);
  listen(document, "visibilitychange", onVisibility);
  listen(window, "scroll", onViewportChange, { capture: true, passive: true });
  listen(window, "resize", onViewportChange, { passive: true });
  listen(window, "pageshow", () => scheduleScan());

  observer.observe(document.documentElement, OBSERVE_OPTIONS);
  discoverShadowRoots(document);
  const sweepTimer = setInterval(sweepShadowRoots, SHADOW_SWEEP_MS);

  scan();
  if (isTop) {
    // A login submitted on the previous page of this site may still wait for a decision.
    send({ type: "cs:pending" })
      .then((info) => info && showSaveBar(info))
      .catch(() => undefined);
  }
})();
