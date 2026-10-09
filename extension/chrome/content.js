/*
 * VaultX content script (classic script, runs after lib/forms.js in every
 * http/https frame).
 *
 * - Detects login forms (lib/forms.js), shows a VaultX icon inside username /
 *   password fields when the vault has matching logins or is locked; the icon
 *   opens a dropdown with the matching logins.
 * - Fills only after a user gesture (click in our dropdown, popup button,
 *   keyboard shortcut). The service worker matches the login against the URL
 *   of this frame as known by the browser – nothing the page says is trusted.
 * - Offers to save / update logins after a form is submitted.
 *
 * All UI lives in a closed shadow root with its own styles. Secrets are never
 * logged and never written anywhere but into the chosen input fields.
 */
(() => {
  "use strict";

  const Forms = globalThis.VaultXForms;
  if (!Forms || !globalThis.chrome?.runtime?.id) return;

  // Re-injection after an extension update: replace the previous instance.
  const previous = globalThis.__vaultxContent;
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
  const AVATAR_HUES = [214, 262, 330, 20, 145, 188, 282, 350, 38, 168, 236, 4];

  const t = (key, substitutions) => {
    try {
      return chrome.i18n.getMessage(key, substitutions) || "";
    } catch {
      return "";
    }
  };

  let alive = true;
  /** Detected login forms (see VaultXForms.findLoginForms). */
  let forms = [];
  /** { state, matches: [{ id, name, username }], insecure, at } from the service worker. */
  let pageInfo = null;
  let pageInfoRunning = false;
  let pageInfoQueued = false;
  let lastUrl = location.href;
  let lastFocused = null;
  let lastReportedFocus = null;
  let lastContextTarget = null;
  /** Credentials typed into a form (for SPA logins without a submit event). */
  let snapshot = null;
  let lastCapture = { key: "", at: 0 };
  /** Open dropdown: { field, node, index }. */
  let dropdown = null;
  const cleanups = [];
  const api = { teardown };
  globalThis.__vaultxContent = api;

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
      display: none; cursor: pointer; border-radius: 5px; opacity: 0.9;
      transition: opacity 120ms ease, box-shadow 120ms ease;
    }
    .icon:hover { opacity: 1; box-shadow: 0 0 0 3px rgba(47, 111, 237, 0.22); }
    .icon svg { width: 100%; height: 100%; }
    .icon.locked .tile { fill: #7d8592; }
    .icon.locked .hole { fill: #7d8592; }

    .dropdown {
      background: var(--bg); border: 1px solid var(--border); border-radius: 12px; box-shadow: var(--shadow);
      padding: 6px; max-height: 320px; overflow-y: auto; overscroll-behavior: contain;
      animation: vx-pop 120ms ease-out;
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
      animation: vx-slide 160ms ease-out;
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
      animation: vx-slide 160ms ease-out;
    }
    .toast svg { width: 20px; height: 20px; flex: none; }
    .leaving { opacity: 0; transition: opacity 180ms ease; }

    @keyframes vx-pop { from { opacity: 0; transform: var(--pos) scale(0.98); } to { opacity: 1; transform: var(--pos); } }
    @keyframes vx-slide { from { opacity: 0; transform: var(--pos) translateY(-6px); } to { opacity: 1; transform: var(--pos); } }
    @media (prefers-reduced-motion: reduce) { .dropdown, .bar, .toast { animation: none; } }
  `;

  let host = null;
  let shadow = null;
  let layer = null;

  function ensureUi() {
    if (!host) {
      // Random tag: a page cannot pre-define it as a custom element or target it with CSS.
      const suffix = Array.from(crypto.getRandomValues(new Uint8Array(4)), (b) => b.toString(16).padStart(2, "0")).join("");
      host = document.createElement(`vaultx-${suffix}`);
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

  /** The VaultX mark (shield with keyhole on a rounded tile), as in the desktop app. */
  function logo() {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 32 32");
    svg.setAttribute("aria-hidden", "true");
    const parts = [
      ["rect", { class: "tile", width: "32", height: "32", rx: "8", fill: "#2f6fed" }],
      ["path", { class: "shield", d: "M16 5.75 24 8.6v6.35c0 5.1-3.4 9.2-8 10.95-4.6-1.75-8-5.85-8-10.95V8.6z", fill: "#fff" }],
      ["circle", { class: "hole", cx: "16", cy: "14", r: "2.4", fill: "#2f6fed" }],
      ["path", { class: "hole", d: "M14.85 15.3h2.3l.65 4.7h-3.6z", fill: "#2f6fed" }],
    ];
    for (const [tag, attrs] of parts) {
      const node = document.createElementNS(SVG_NS, tag);
      for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, value);
      svg.append(node);
    }
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

  /**
   * True if a click really hit our visible UI: the page cannot overlay,
   * hide or fade our host to trick the user into clicking it (clickjacking).
   */
  function uiIsTrustworthy(ev) {
    if (!host || !host.isConnected) return false;
    const style = getComputedStyle(host);
    if (style.opacity !== "1" || style.visibility !== "visible" || style.display === "none") return false;
    if (Number(getComputedStyle(document.documentElement).opacity) < 1) return false;
    // Keyboard activation has no pointer position.
    if (ev.detail === 0 && ev.clientX === 0 && ev.clientY === 0) return true;
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
    button.title = "VaultX";
    button.setAttribute("aria-label", t("csIconLabel"));
    button.append(logo());
    // Keep the focus (and caret) in the page's field.
    button.addEventListener("mousedown", (ev) => ev.preventDefault());
    button.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!ev.isTrusted) return;
      if (dropdown?.field === field) closeDropdown();
      else openDropdown(field);
    });
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
    node.setAttribute("aria-label", "VaultX");
    node.addEventListener("mousedown", (ev) => ev.preventDefault());
    dropdown = { field, node, index: 0 };
    ensureUi().append(node);
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
    dropdown.node.remove();
    dropdown = null;
  }

  function dropdownMatches() {
    return pageInfo?.state === "unlocked" ? pageInfo.matches : [];
  }

  function renderDropdown() {
    if (!dropdown) return;
    const { node } = dropdown;
    node.replaceChildren();

    const head = el("div", "dd-head");
    head.append(logo(), el("span", "", "VaultX"), el("span", "host", location.hostname.replace(/^www\./, "")));
    node.append(head);

    if (pageInfo?.state === "locked") {
      const box = el("div", "dd-locked");
      box.append(el("div", "title", t("csLocked")), el("div", "hint", t("csLockedHint")));
      const unlock = el("button", "btn btn-primary", t("csUnlock"));
      unlock.type = "button";
      unlock.addEventListener("click", (ev) => {
        if (!ev.isTrusted || !uiIsTrustworthy(ev)) return;
        closeDropdown();
        send({ type: "cs:open-popup" }).catch(() => undefined);
      });
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
        if (!ev.isTrusted || !uiIsTrustworthy(ev)) return;
        chooseMatch(match);
      });
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
      case "Enter":
        if (matches.length) chooseMatch(matches[dropdown.index]);
        else handled = false;
        break;
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
      const filled = form ? Forms.fillForm(form, credentials) : 0;
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
    const detail = isUpdate ? `${info.itemName || info.host} · ${username}` : `${username} · ${info.host}`;
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

    node.append(logo(), texts, actions);
    ensureUi().append(node);
    bar = { node, info, texts, actions, timer: setTimeout(closeBar, BAR_TTL_MS) };
    positionBar();
  }

  function setBarStatus(text, isError) {
    if (!bar) return;
    bar.actions.replaceChildren(el("span", isError ? "status error" : "status", text));
  }

  async function decide(action, ev) {
    if (!bar || !ev.isTrusted || !uiIsTrustworthy(ev)) return;
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
          if (!e.isTrusted || !uiIsTrustworthy(e)) return;
          send({ type: "cs:open-popup" }).catch(() => undefined);
          restoreBarActions(current);
        });
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

  function captureForm(form) {
    snapshot = null;
    if (form) submitCredentials(Forms.readCredentials(form));
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
    if (!ev.isTrusted) return; // our own fills are untrusted events
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    const form = Forms.formForElement(forms, target);
    if (!form) return;
    const credentials = Forms.readCredentials(form);
    snapshot = credentials ? { form, credentials, at: Date.now(), href: location.href } : null;
  }

  // ---------------------------------------------------------------------------
  // Focus tracking (target of generated passwords)
  // ---------------------------------------------------------------------------

  function onFocusIn(ev) {
    const target = eventTarget(ev);
    if (!Forms.isTextEntry(target)) return;
    lastFocused = target;
    if (lastReportedFocus !== target) {
      lastReportedFocus = target;
      send({ type: "cs:focus" }).catch(() => undefined);
    }
  }

  function onFocusOut(ev) {
    if (dropdown && eventTarget(ev) === dropdown.field) {
      // Clicks into our dropdown keep the focus (mousedown is prevented), so this is a real blur.
      setTimeout(() => {
        if (dropdown && deepActiveElement() !== dropdown.field) closeDropdown();
      }, 120);
    }
  }

  function onContextMenu(ev) {
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
      const active = deepActiveElement();
      target = Forms.isTextEntry(active) ? active : lastFocused;
    }
    if (!target || !target.isConnected) return 0;
    return Forms.fillGenerated(target, msg.password);
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
        if (!Forms.fillForm(form, credentials)) toast(t("csFillFailed"));
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
    if (globalThis.__vaultxContent === api) delete globalThis.__vaultxContent;
  }

  chrome.runtime.onMessage.addListener(onMessage);
  listen(document, "submit", onSubmit, true);
  listen(document, "keydown", onKeyDown, true);
  listen(window, "keydown", onDropdownKeys, true);
  listen(document, "click", onClick, true);
  listen(document, "input", onInput, true);
  listen(document, "focusin", onFocusIn, true);
  listen(document, "focusout", onFocusOut, true);
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
