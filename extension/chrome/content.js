/*
 * Keystead content script (classic script, runs after lib/forms.js in every
 * http/https frame).
 *
 * - Detects login forms (lib/forms.js), shows a Keystead icon inside username /
 *   password fields when the vault has matching logins or is locked; the icon
 *   opens a dropdown with the matching logins.
 * - Fills only after a user gesture (click in our dropdown, popup button,
 *   keyboard shortcut). The service worker matches the login against the URL
 *   of this frame as known by the browser – nothing the page says is trusted.
 *   Clicks and keys in our UI only count while the control has been fully
 *   visible (unoccluded, no opacity/filter effects; IntersectionObserver v2)
 *   for a moment, so a page cannot trick the user into using our UI hidden
 *   under its own content (clickjacking).
 * - Offers to save / update logins after a form is submitted – only for a
 *   password the user typed (or inserted from the generator) themselves.
 *
 * All UI lives in a closed shadow root with its own styles. Secrets are never
 * logged and never written anywhere but into the chosen input fields.
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
  const AVATAR_HUES = [214, 262, 330, 20, 145, 188, 282, 350, 38, 168, 236, 4];

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
  /** { state, matches: [{ id, name, username }], insecure, at } from the service worker. */
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
   * the user inserted with the generator. A password is only offered for saving
   * if a field still holds exactly that value: a page cannot make us prompt for
   * a password it filled in (or replaced) by script.
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

  function refreshForms() {
    if (!allInputs.length) {
      forms = [];
      return forms;
    }
    try {
      forms = Forms.findLoginForms(document);
    } catch {
      forms = [];
    }
    return forms;
  }

  function scan() {
    if (!alive) return;
    lastScanAt = Date.now();
    if (document.hidden) return; // rescanned on visibilitychange
    refreshForms();
    checkSnapshot();
    const urlChanged = location.href !== lastUrl;
    if (urlChanged) {
      lastUrl = location.href;
      pageInfo = null;
    }
    if (forms.length && !pageInfo) requestPageInfo();
    placeUi();
    renderIcons();
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
        matches: Array.isArray(info?.matches) ? info.matches.filter((m) => m && typeof m.id === "string") : [],
        at: Date.now(),
      };
    } catch {
      pageInfo = { state: "error", insecure: false, matches: [], at: Date.now() };
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
      --bg: #ffffff; --bg-hover: #f0f3f8; --bg-active: #e6edfb; --border: #e1e4ea; --text: #181b21;
      --text-2: #4f5663; --text-3: #626976; --accent: #2f6fed; --accent-hover: #2861d6; --accent-soft: #eaf1fe;
      --warning: #8a4b06; --warning-soft: #fdf5e7; --success: #146c36; --danger: #b42318;
      --shadow: 0 12px 32px rgba(16, 24, 40, 0.18), 0 2px 6px rgba(16, 24, 40, 0.08);
      --av-bg-s: 78%; --av-bg-l: 93%; --av-fg-s: 58%; --av-fg-l: 36%;
      position: absolute; top: 0; left: 0; width: 0; height: 0;
      font: 400 13px/1.4 "Segoe UI Variable Text", "Segoe UI", Inter, system-ui, -apple-system, "Helvetica Neue", Arial, sans-serif;
      color: var(--text); letter-spacing: normal; text-align: left; -webkit-font-smoothing: antialiased;
    }
    @media (prefers-color-scheme: dark) {
      .layer {
        --bg: #22262d; --bg-hover: #2c3139; --bg-active: rgba(79, 140, 255, 0.18); --border: #353b45; --text: #e8eaee;
        --text-2: #aab1bd; --text-3: #8c94a1; --accent: #4f8cff; --accent-hover: #3b7af2; --accent-soft: rgba(79, 140, 255, 0.14);
        --warning: #f3be5e; --warning-soft: rgba(240, 167, 58, 0.12); --success: #6ad891; --danger: #ff8a8a;
        --shadow: 0 12px 32px rgba(0, 0, 0, 0.5), 0 2px 6px rgba(0, 0, 0, 0.35);
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
    .icon svg { width: 100%; height: 100%; }
    .icon.locked .tile { fill: #7d8592; }

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
      display: flex; align-items: center; gap: 10px; width: max-content; max-width: min(380px, calc(100vw - 32px));
      padding: 10px 14px 10px 12px;
      background: var(--bg); border: 1px solid var(--border); border-radius: 12px; box-shadow: var(--shadow);
      animation: ks-slide 160ms ease-out;
    }
    .toast svg { width: 20px; height: 20px; flex: none; }
    .leaving { opacity: 0; transition: opacity 180ms ease; }

    @keyframes ks-pop { from { opacity: 0; transform: var(--pos) scale(0.98); } to { opacity: 1; transform: var(--pos); } }
    @keyframes ks-slide { from { opacity: 0; transform: var(--pos) translateY(-6px); } to { opacity: 1; transform: var(--pos); } }
    @media (prefers-reduced-motion: reduce) { .dropdown, .bar, .toast { animation: none; } }
    /* Clickable controls must stay free of opacity, filters and non-translate transforms once shown:
       IntersectionObserver v2 treats those as "not visible" (see uiIsTrustworthy). */
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

  function closeIcon() {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", "M6 6l12 12M18 6 6 18");
    path.setAttribute("stroke", "currentColor");
    path.setAttribute("stroke-width", "2");
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("fill", "none");
    svg.append(path);
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
  /** Tracked control → { visible, since } (since: when it last became visible). */
  const visibility = new WeakMap();
  let visibilityObserver = null;

  function onVisibilityEntries(entries) {
    for (const entry of entries) {
      const state = visibility.get(entry.target);
      if (!state) continue;
      const visible = entry.isVisible === true && entry.isIntersecting;
      if (visible && !state.visible) state.since = entry.time;
      state.visible = visible;
    }
  }

  /** Starts tracking an actionable control (a leaf: a container's own children would count as covering it). */
  function trackVisibility(node) {
    visibility.set(node, { visible: false, since: 0 });
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

  /** Stops tracking the controls inside `root` (and `root` itself). */
  function untrackVisibility(root) {
    if (!root) return;
    for (const node of [root, ...root.querySelectorAll("button")]) {
      if (!visibility.has(node)) continue;
      visibility.delete(node);
      visibilityObserver?.unobserve(node);
    }
  }

  /** True if `node` has been fully visible for at least VISIBLE_DWELL_MS (fails closed without IntersectionObserver v2). */
  function seenLongEnough(node) {
    if (!visibilityObserver || !node) return false;
    // Apply changes that were computed but not delivered yet (e.g. a cover that just appeared).
    onVisibilityEntries(visibilityObserver.takeRecords());
    const state = visibility.get(node);
    return !!state && state.visible && performance.now() - state.since >= VISIBLE_DWELL_MS;
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
   * True if a click on (or a key for) `node` really comes from our visible UI:
   * the page cannot overlay, hide or fade our host to trick the user into using
   * it (clickjacking). `ev` is the click; null for keyboard actions (no position).
   */
  function uiIsTrustworthy(ev, node) {
    if (!host || !host.isConnected || !node || !node.isConnected) return false;
    const style = getComputedStyle(host);
    if (style.opacity !== "1" || style.visibility !== "visible" || style.display === "none" || hasVisualEffect(style)) return false;
    if (Number(getComputedStyle(document.documentElement).opacity) < 1) return false;
    if (!seenLongEnough(node)) return false;
    // Keyboard activation has no pointer position.
    if (!ev || (ev.detail === 0 && ev.clientX === 0 && ev.clientY === 0)) return true;
    return document.elementFromPoint(ev.clientX, ev.clientY) === host;
  }

  // ---------------------------------------------------------------------------
  // Inline icons
  // ---------------------------------------------------------------------------

  /** field → icon button */
  const icons = new Map();
  const resizeObserver = new ResizeObserver(() => schedulePosition());
  let positionFrame = 0;
  let positionInterval = 0;

  function iconsWanted() {
    if (!pageInfo) return false;
    return pageInfo.state === "locked" || (pageInfo.state === "unlocked" && pageInfo.matches.length > 0);
  }

  function renderIcons() {
    if (!alive) return;
    const fields = iconsWanted() ? Forms.iconFields(forms) : [];
    for (const [field, button] of icons) {
      if (!fields.includes(field)) {
        untrackVisibility(button);
        button.remove();
        icons.delete(field);
        resizeObserver.unobserve(field);
        if (dropdown?.field === field) closeDropdown();
      }
    }
    const locked = pageInfo?.state === "locked";
    for (const field of fields) {
      let button = icons.get(field);
      if (!button) {
        button = createIcon(field);
        ensureUi().append(button);
        icons.set(field, button);
        resizeObserver.observe(field);
      }
      button.classList.toggle("locked", locked);
    }
    if (icons.size && !positionInterval) {
      // Catches layout shifts that cause neither scroll, resize nor DOM mutations (transitions).
      positionInterval = setInterval(schedulePosition, 1500);
    } else if (!icons.size && positionInterval) {
      clearInterval(positionInterval);
      positionInterval = 0;
    }
    positionAll();
  }

  function createIcon(field) {
    const button = el("button", "icon");
    button.type = "button";
    button.tabIndex = -1;
    button.title = "Keystead";
    button.setAttribute("aria-label", t("csIconLabel"));
    button.append(logo());
    // Keep the focus (and caret) in the page's field.
    button.addEventListener("mousedown", (ev) => ev.preventDefault());
    button.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!ev.isTrusted) return;
      if (dropdown?.field === field) closeDropdown();
      else if (uiIsTrustworthy(ev, button)) openDropdown(field);
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
    for (const [field, button] of icons) positionIcon(field, button);
    if (dropdown) positionDropdown();
  }

  /** True if the point shows the field (not another element on top of it). */
  function showsField(field, x, y) {
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return false;
    const hit = document.elementFromPoint(x, y);
    if (!hit) return false;
    if (hit === host || hit === field || field.contains(hit) || hit.contains(field)) return true;
    // Hit inside an open shadow root of the page (web component inputs).
    return hit.shadowRoot?.contains(field) === true;
  }

  function positionIcon(field, button) {
    const rect = field.getBoundingClientRect();
    const size = Math.round(Math.max(14, Math.min(20, rect.height - 12)));
    const pad = Math.round(Math.max(4, Math.min(8, (rect.height - size) / 2)));
    const x = rect.right - size - pad;
    const y = rect.top + (rect.height - size) / 2;
    const visible =
      rect.width >= 80 &&
      rect.height >= 20 &&
      rect.top >= -1 &&
      rect.bottom <= innerHeight + 1 &&
      rect.left >= -1 &&
      rect.right <= innerWidth + 1 &&
      Forms.isVisible(field) &&
      (showsField(field, rect.left + rect.width / 2, y + size / 2) || showsField(field, x - 4, y + size / 2));
    if (!visible) {
      button.style.display = "none";
      return;
    }
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
      const unlock = el("button", "btn btn-primary", t("csUnlock"));
      unlock.type = "button";
      unlock.addEventListener("click", (ev) => {
        if (!ev.isTrusted || !uiIsTrustworthy(ev, unlock)) return;
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
      const item = el("button", i === dropdown.index ? "item active" : "item");
      item.type = "button";
      item.tabIndex = -1;
      item.setAttribute("role", "option");
      const texts = el("span", "texts");
      texts.append(el("span", "name", match.name || match.username || "—"), el("span", "user", match.username || t("csNoUsername")));
      item.append(avatar(match.name || match.username), texts, el("span", "fill-hint", t("csFill")));
      item.addEventListener("mousemove", () => highlight(i));
      item.addEventListener("click", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        if (!ev.isTrusted || !uiIsTrustworthy(ev, item)) return;
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
    const left = Math.min(Math.max(8, rect.left), innerWidth - width - 8);
    const height = node.offsetHeight;
    let top = rect.bottom + 6;
    if (top + height > innerHeight - 8 && rect.top - 6 - height >= 8) top = rect.top - 6 - height;
    place(node, left, top);
  }

  /** Keys while the dropdown is open (registered on window, capture phase, so the page doesn't submit). */
  function onDropdownKeys(ev) {
    if (!dropdown || !ev.isTrusted || eventTarget(ev) !== dropdown.field) return;
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
        if (uiIsTrustworthy(null, item)) chooseMatch(matches[dropdown.index]);
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
  // Toasts and the save / update bar
  // ---------------------------------------------------------------------------

  let toastNode = null;
  let toastTimer = 0;

  function toast(text) {
    if (!text || !alive) return;
    const root = ensureUi();
    toastNode?.remove();
    clearTimeout(toastTimer);
    const node = el("div", "toast");
    node.setAttribute("role", "status");
    node.append(logo(), el("span", "", text));
    root.append(node);
    raiseUi(true);
    toastNode = node;
    place(node, Math.max(8, innerWidth - node.offsetWidth - 16), 16);
    toastTimer = setTimeout(() => {
      node.classList.add("leaving");
      setTimeout(() => node.remove(), 200);
      if (toastNode === node) toastNode = null;
    }, 4500);
  }

  /** { node, info, timer } while the bar is shown. */
  let bar = null;

  function closeBar() {
    if (!bar) return;
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
    const save = el("button", "btn btn-primary", isUpdate ? t("csUpdate") : t("csSave"));
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
    if (!bar || !ev.isTrusted || !uiIsTrustworthy(ev, ev.currentTarget)) return;
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
        const unlock = el("button", "btn btn-primary", t("csUnlock"));
        unlock.type = "button";
        unlock.addEventListener("click", (e) => {
          if (!e.isTrusted || !uiIsTrustworthy(e, unlock)) return;
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
   * is one the user typed/pasted (or inserted from the generator) and no script
   * changed since; null otherwise. Username-only steps are returned as is.
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
    const form = refreshForms().find((f) => f.scope === target || target.contains(f.scope));
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
    const button = Forms.submitButtonFor(eventTarget(ev));
    if (!button) return;
    const current = refreshForms();
    const form = Forms.formForElement(current, button) || (current.length === 1 ? current[0] : null);
    captureForm(form);
  }

  function onInput(ev) {
    if (!ev.isTrusted) return; // our own fills and the page's synthetic events are untrusted
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    userValues.set(target, target.value);
    const form = Forms.formForElement(forms, target);
    if (!form) return;
    const credentials = userCredentials(form);
    snapshot = credentials ? { form, credentials, at: Date.now(), href: location.href } : null;
  }

  // ---------------------------------------------------------------------------
  // Focus tracking (target of generated passwords)
  // ---------------------------------------------------------------------------

  function onFocusIn(ev) {
    // Synthetic focus events would let a frame (e.g. a cross-origin ad) claim the
    // "focused frame" slot and receive the next generated password.
    if (!ev.isTrusted) return;
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    if (lastReportedFocus !== target) {
      lastReportedFocus = target;
      send({ type: "cs:focus" }).catch(() => undefined);
    }
  }

  function onFocusOut(ev) {
    // Report the field again when it regains the focus: another frame may have claimed it meanwhile.
    if (ev.isTrusted && eventTarget(ev) === lastReportedFocus) lastReportedFocus = null;
    if (dropdown && eventTarget(ev) === dropdown.field) {
      // Clicks into our dropdown keep the focus (mousedown is prevented), so this is a real blur.
      setTimeout(() => {
        if (dropdown && deepActiveElement() !== dropdown.field) closeDropdown();
      }, 120);
    }
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
    if (!dropdown) return;
    const path = ev.composedPath();
    if ((host && path.includes(host)) || path.includes(dropdown.field)) return;
    closeDropdown();
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
    const filled = Forms.fillGenerated(target, msg.password);
    if (filled) {
      // The user chose to insert it: offer to save it like a typed password.
      const form = Forms.formForElement(refreshForms(), target);
      for (const field of form ? [target, ...form.passwords] : [target]) {
        if (field.value === msg.password) userValues.set(field, msg.password);
      }
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
        if (forms.length) requestPageInfo();
        else scheduleScan();
        return false;
      case "bg:toast":
        if (typeof msg.key === "string" && /^cs[A-Z][A-Za-z]+$/.test(msg.key)) toast(t(msg.key));
        return false;
      default:
        return false;
    }
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  const observer = new MutationObserver((mutations) => {
    if (mutations.every((m) => m.target === host)) return;
    scheduleScan();
  });

  function onVisibility() {
    if (document.hidden) return;
    scheduleScan();
    if (forms.length && (!pageInfo || Date.now() - pageInfo.at > PAGE_INFO_MAX_AGE_MS)) requestPageInfo();
  }

  function onViewportChange() {
    if (icons.size || dropdown) schedulePosition();
    if (bar) positionBar();
  }

  function teardown() {
    if (!alive) return;
    alive = false;
    observer.disconnect();
    resizeObserver.disconnect();
    visibilityObserver?.disconnect();
    clearTimeout(scanTimer);
    clearTimeout(toastTimer);
    clearInterval(positionInterval);
    if (positionFrame) cancelAnimationFrame(positionFrame);
    if (bar) clearTimeout(bar.timer);
    for (const cleanup of cleanups.splice(0)) cleanup();
    try {
      chrome.runtime.onMessage.removeListener(onMessage);
    } catch {
      // Context already invalidated.
    }
    host?.remove();
    icons.clear();
    dropdown = null;
    bar = null;
    snapshot = null;
    userValues = new WeakMap();
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

  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["type", "style", "class", "hidden", "disabled", "aria-hidden", "open"],
  });

  scan();
  if (isTop) {
    // A login submitted on the previous page of this site may still wait for a decision.
    send({ type: "cs:pending" })
      .then((info) => info && showSaveBar(info))
      .catch(() => undefined);
  }
})();
