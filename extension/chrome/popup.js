// Keystead popup. States: host missing → app unavailable → not paired (pairing)
// → locked → unlocked (tabs "Diese Seite", "Suche", "Generator", add login).
// All work happens in the service worker; this page only renders.
// `?demo=<state>` swaps in lib/demo.js (fake data, no storage, no native host).

import { t, uiLanguage } from "./lib/i18n.js";
import { icon, logo } from "./lib/icons.js";
import { createApi } from "./lib/popup-api.js";

const demoState = new URLSearchParams(location.search).get("demo");
const api = demoState ? (await import("./lib/demo.js")).createDemoApi(demoState) : createApi();

const app = document.getElementById("app");
const toastNode = document.getElementById("toast");

const AVATAR_HUES = [214, 262, 330, 20, 145, 188, 282, 350, 38, 168, 236, 4];
const GENERATOR_DEFAULTS = {
  kind: "password",
  length: 20,
  uppercase: true,
  lowercase: true,
  digits: true,
  symbols: true,
  minDigits: 1,
  minSymbols: 1,
  avoidAmbiguous: false,
  words: 5,
  separator: "-",
  capitalize: true,
  includeNumber: true,
};

// ---------------------------------------------------------------------------
// DOM helpers
// ---------------------------------------------------------------------------

function h(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
    else if (typeof value === "boolean" || typeof value === "number") {
      if (key in node) node[key] = value;
      else node.setAttribute(key, String(value));
    } else node.setAttribute(key, value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function button(label, { kind = "secondary", iconName, onclick, type = "button", wide = false, title } = {}) {
  return h(
    "button",
    { class: `btn btn-${kind}${wide ? " wide" : ""}`, type, onclick, title },
    iconName ? icon(iconName) : null,
    h("span", { text: label }),
  );
}

function iconButton(name, label, onclick, size = "") {
  return h("button", { class: `icon-btn ${size}`.trim(), type: "button", title: label, "aria-label": label, onclick }, icon(name));
}

/** Shows a spinner in a button while an action runs. */
function setBusy(btn, busy) {
  if (!btn) return;
  btn.disabled = busy;
  if (busy && !btn.querySelector(".spinner")) {
    btn.dataset.label = btn.textContent;
    btn.prepend(h("span", { class: "spinner", "aria-hidden": "true" }));
  } else if (!busy) {
    btn.querySelector(".spinner")?.remove();
  }
}

function avatar(name, size = "") {
  const text = String(name || "").trim();
  let hash = 0;
  for (const ch of text.toLowerCase()) hash = (hash * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  const letter = text.match(/[\p{L}\p{N}]/u);
  const node = h("span", { class: `avatar ${size}`.trim(), "aria-hidden": "true", text: letter ? letter[0].toLocaleUpperCase() : "?" });
  node.style.setProperty("--h", String(AVATAR_HUES[hash % AVATAR_HUES.length]));
  return node;
}

function emptyState(iconName, title, text) {
  return h(
    "div",
    { class: "empty" },
    h("div", { class: "empty-ico" }, icon(iconName)),
    h("div", { class: "empty-title", text: title }),
    text ? h("p", { class: "empty-text", text }) : null,
  );
}

function skeletonList(rows = 2) {
  return h(
    "div",
    { class: "list", "aria-busy": "true" },
    Array.from({ length: rows }, () =>
      h(
        "div",
        { class: "skeleton" },
        h("div", { class: "sk-avatar" }),
        h("div", { class: "sk-lines" }, h("div", { class: "sk-line" }), h("div", { class: "sk-line short" })),
      ),
    ),
  );
}

/** Password input with show/hide toggle (and optional extra buttons). */
function secretInput(props, extraButtons = []) {
  const input = h("input", { type: "password", class: "input secret", spellcheck: "false", autocomplete: "off", ...props });
  const toggle = iconButton("eye", t("showPassword"), () => {
    const show = input.type === "password";
    input.type = show ? "text" : "password";
    toggle.replaceChildren(icon(show ? "eyeOff" : "eye"));
    toggle.title = show ? t("hidePassword") : t("showPassword");
    toggle.setAttribute("aria-label", toggle.title);
  });
  toggle.tabIndex = -1;
  const wrap = h(
    "div",
    { class: extraButtons.length ? "input-wrap two" : "input-wrap" },
    input,
    h("div", { class: "input-actions" }, toggle, ...extraButtons),
  );
  return { input, wrap };
}

// ---------------------------------------------------------------------------
// Toast, errors, routing
// ---------------------------------------------------------------------------

let toastTimer = 0;

function toast(text, tone = "info") {
  clearTimeout(toastTimer);
  toastNode.replaceChildren(icon(tone === "error" ? "alert" : "check"), h("span", { text }));
  toastNode.className = `toast ${tone}`;
  toastNode.hidden = false;
  requestAnimationFrame(() => toastNode.classList.add("show"));
  toastTimer = setTimeout(() => {
    toastNode.classList.remove("show");
    toastTimer = setTimeout(() => {
      toastNode.hidden = true;
    }, 200);
  }, 2400);
}

function errorText(code) {
  switch (code) {
    case "no_fields":
      return t("errorNoFields");
    case "no_field":
      return t("noFocusedField");
    case "insecure":
      return t("errorInsecure");
    case "not_found":
      return t("errorNotFound");
    default:
      return t("errorGeneric", code || "internal");
  }
}

/** Switches to the matching screen for state errors; returns false for other errors. */
function routeError(err) {
  switch (err?.code) {
    case "locked":
      showLocked();
      return true;
    case "not_paired":
      showNotPaired();
      return true;
    case "host_missing":
    case "host_forbidden":
      showHostMissing();
      return true;
    case "app_unavailable":
    case "timeout":
      showAppUnavailable(err.code);
      return true;
    default:
      return false;
  }
}

/** Runs an action; state errors switch screens, other errors show a toast. */
async function guarded(action) {
  try {
    return await action();
  } catch (err) {
    if (!routeError(err)) toast(errorText(err?.code), "error");
    return undefined;
  }
}

let stopPairingWatch = null;
let pairingPoll = 0;
/** The pairing shown on screen ("<state>:<code>"), to ignore repeated updates. */
let shownPairing = "";
const pairingKey = (pairing) => (pairing ? `${pairing.state}:${pairing.code}` : "");

function mount(node, name) {
  if (name !== "pairing") {
    stopPairingWatch?.();
    stopPairingWatch = null;
    clearInterval(pairingPoll);
    pairingPoll = 0;
  }
  app.dataset.screen = name;
  app.replaceChildren(node);
}

function route(status) {
  switch (status?.state) {
    case "host_missing":
      return showHostMissing();
    case "not_paired":
      return showNotPaired();
    case "locked":
      return showLocked();
    case "unlocked":
      return showUnlocked(status);
    default:
      return showAppUnavailable(status?.error);
  }
}

async function recheck() {
  const loadingTimer = setTimeout(showLoading, 150);
  try {
    const status = await api.status(true);
    clearTimeout(loadingTimer);
    route(status);
  } catch (err) {
    clearTimeout(loadingTimer);
    if (!routeError(err)) showAppUnavailable();
  }
}

// ---------------------------------------------------------------------------
// Centered screens
// ---------------------------------------------------------------------------

function centered({ artwork, title, text, body = [], actions = [], footer = null }) {
  return h(
    "section",
    { class: "screen center" },
    h("div", { class: "center-body" }, artwork, h("h1", { text: title }), text ? h("p", { class: "lead", text }) : null, body),
    actions.length ? h("div", { class: "center-actions" }, actions) : null,
    footer,
  );
}

function art(iconName, tone) {
  return h("div", { class: `art art-${tone}` }, icon(iconName));
}

function brandArt(size = 56, extraClass = "") {
  return h("div", { class: `art-logo ${extraClass}`.trim() }, logo(size));
}

function versionFooter() {
  return h("div", { class: "meta" }, `Keystead · ${t("versionLabel", api.version)}`);
}

function showLoading() {
  mount(
    h(
      "section",
      { class: "screen center" },
      h("div", { class: "center-body" }, brandArt(48, "pulse"), h("p", { class: "muted", text: t("loading") })),
    ),
    "loading",
  );
}

function showHostMissing() {
  mount(
    centered({
      artwork: art("plug", "neutral"),
      title: t("hostMissingTitle"),
      text: t("hostMissingText"),
      body: h(
        "ol",
        { class: "steps" },
        ["hostStep1", "hostStep2", "hostStep3"].map((key, i) =>
          h("li", {}, h("span", { class: "step-num", text: String(i + 1) }), h("span", { text: t(key) })),
        ),
      ),
      actions: [button(t("retryCheck"), { kind: "primary", iconName: "refresh", onclick: recheck })],
      footer: h("div", { class: "meta" }, `${t("extensionId")}: `, h("code", { text: api.extensionId })),
    }),
    "host_missing",
  );
}

function showAppUnavailable(code) {
  mount(
    centered({
      artwork: art("alert", "warning"),
      title: t("appUnavailableTitle"),
      text: code === "timeout" ? t("appTimeoutText") : t("appUnavailableText"),
      actions: [button(t("retry"), { kind: "primary", iconName: "refresh", onclick: recheck })],
      footer: versionFooter(),
    }),
    "app_unavailable",
  );
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

/** Uniformly random 6-digit code (rejection sampling avoids modulo bias). */
function randomCode() {
  const limit = Math.floor(0x100000000 / 1_000_000) * 1_000_000;
  const buf = new Uint32Array(1);
  do crypto.getRandomValues(buf);
  while (buf[0] >= limit);
  return String(buf[0] % 1_000_000).padStart(6, "0");
}

/** "<Browser> – <platform>", shown in the app's pairing dialog and client list. */
function clientName() {
  const brands = (navigator.userAgentData?.brands || []).map((b) => b.brand);
  const ua = navigator.userAgent;
  let browser = "Chromium";
  if (navigator.brave) browser = "Brave";
  else if (brands.some((b) => /edge/i.test(b)) || /Edg\//.test(ua)) browser = "Edge";
  else if (brands.some((b) => /opera/i.test(b)) || /OPR\//.test(ua)) browser = "Opera";
  else if (brands.some((b) => /vivaldi/i.test(b)) || /Vivaldi/.test(ua)) browser = "Vivaldi";
  else if (brands.some((b) => /google chrome/i.test(b))) browser = "Chrome";
  const raw = navigator.userAgentData?.platform || (/(Windows|Mac|Linux|CrOS)/.exec(ua)?.[1] ?? "");
  const platform = { Mac: "macOS", CrOS: "ChromeOS", "Chrome OS": "ChromeOS" }[raw] || raw;
  return platform ? `${browser} – ${platform}` : browser;
}

function showNotPaired() {
  mount(
    centered({
      artwork: brandArt(),
      title: t("pairTitle"),
      text: t("pairText"),
      actions: [button(t("pairButton"), { kind: "primary", onclick: (ev) => startPairing(ev.currentTarget) })],
      footer: versionFooter(),
    }),
    "not_paired",
  );
}

async function startPairing(btn) {
  setBusy(btn, true);
  try {
    showPairing(await api.pairStart(randomCode(), clientName()));
  } catch (err) {
    setBusy(btn, false);
    if (!routeError(err)) toast(errorText(err?.code), "error");
  }
}

function codeDisplay(code) {
  const digits = String(code).split("");
  return h(
    "div",
    { class: "code", role: "img", "aria-label": `${t("pairCodeLabel")}: ${digits.join(" ")}` },
    digits.slice(0, 3).map((d) => h("span", { class: "digit", text: d })),
    h("span", { class: "gap" }),
    digits.slice(3).map((d) => h("span", { class: "digit", text: d })),
  );
}

function renderPairing(pairing) {
  shownPairing = pairingKey(pairing);
  if (!pairing) {
    recheck();
    return;
  }
  if (pairing.state === "pending") {
    mount(
      centered({
        artwork: brandArt(48),
        title: t("pairWaitingTitle"),
        text: t("pairWaitingText"),
        body: [
          codeDisplay(pairing.code),
          h("div", { class: "waiting" }, h("span", { class: "spinner", "aria-hidden": "true" }), t("pairWaiting")),
        ],
        actions: [
          button(t("cancel"), {
            kind: "secondary",
            onclick: async () => {
              await guarded(() => api.pairCancel());
              showNotPaired();
            },
          }),
        ],
      }),
      "pairing",
    );
    return;
  }

  if (pairing.state === "success") {
    const proceed = async () => {
      await guarded(() => api.pairReset());
      recheck();
    };
    mount(
      centered({
        artwork: art("check", "success"),
        title: t("pairSuccessTitle"),
        text: t("pairSuccessText"),
        actions: [button(t("continue"), { kind: "primary", onclick: proceed })],
      }),
      "pairing",
    );
    if (!api.demo) setTimeout(() => app.dataset.screen === "pairing" && proceed(), 1400);
    return;
  }

  // Denied or failed. Connection problems get their own screens.
  if (["host_missing", "host_forbidden", "app_unavailable", "timeout"].includes(pairing.error)) {
    api.pairReset().catch(() => undefined);
    routeError({ code: pairing.error });
    return;
  }
  const denied = pairing.state === "denied";
  mount(
    centered({
      artwork: art(denied ? "close" : "alert", denied ? "danger" : "warning"),
      title: denied ? t("pairDeniedTitle") : t("pairErrorTitle"),
      text: denied ? t("pairDeniedText") : t("pairErrorText"),
      actions: [
        button(t("retry"), {
          kind: "primary",
          iconName: "refresh",
          onclick: async (ev) => {
            const btn = ev.currentTarget;
            await guarded(() => api.pairReset());
            startPairing(btn);
          },
        }),
        button(t("cancel"), {
          kind: "ghost",
          onclick: async () => {
            await guarded(() => api.pairReset());
            showNotPaired();
          },
        }),
      ],
    }),
    "pairing",
  );
}

function showPairing(pairing) {
  renderPairing(pairing);
  if (app.dataset.screen !== "pairing") return;
  if (!stopPairingWatch) {
    // Only progress updates are rendered; a removed pairing (null) is always
    // the result of an action on this screen, which navigates by itself.
    stopPairingWatch = api.onPairingChange((next) => {
      if (next && app.dataset.screen === "pairing" && pairingKey(next) !== shownPairing) renderPairing(next);
    });
  }
  // Fallback in case a storage event is missed (e.g. service worker restart).
  if (!pairingPoll && !api.demo) {
    pairingPoll = setInterval(async () => {
      if (app.dataset.screen !== "pairing") return;
      try {
        const next = await api.pairState();
        if (next && app.dataset.screen === "pairing" && pairingKey(next) !== shownPairing) renderPairing(next);
      } catch {
        // The next tick tries again.
      }
    }, 2000);
  }
}

// ---------------------------------------------------------------------------
// Locked
// ---------------------------------------------------------------------------

function showLocked() {
  const { input, wrap } = secretInput({
    id: "master",
    autocomplete: "current-password",
    placeholder: t("masterPassword"),
    "aria-label": t("masterPassword"),
  });
  const error = h("p", { class: "field-error", role: "alert", hidden: true });
  const submit = button(t("unlock"), { kind: "primary", type: "submit", wide: true });

  const form = h(
    "form",
    {
      class: "unlock-form",
      onsubmit: async (ev) => {
        ev.preventDefault();
        const password = input.value;
        if (!password) {
          input.focus();
          return;
        }
        setBusy(submit, true);
        error.hidden = true;
        input.classList.remove("invalid");
        try {
          const result = await api.unlock(password);
          input.value = "";
          showUnlocked({ state: "unlocked", vaultName: result?.vaultName || null });
        } catch (err) {
          setBusy(submit, false);
          if (err?.code === "wrong_password") {
            error.textContent = t("wrongPassword");
            error.hidden = false;
            input.classList.add("invalid");
            input.select();
            input.focus();
          } else if (!routeError(err)) {
            // The app answers `not_found` when it cannot tell which vault to open
            // (several vaults, none used in the app yet).
            error.textContent = err?.code === "not_found" ? t("unlockNoVault") : errorText(err?.code);
            error.hidden = false;
          }
        }
      },
    },
    wrap,
    error,
    submit,
  );

  mount(
    centered({
      artwork: brandArt(),
      title: t("lockedTitle"),
      text: t("lockedText"),
      body: form,
      footer: h(
        "div",
        { class: "center-links" },
        h(
          "button",
          { class: "link-btn", type: "button", onclick: () => guarded(() => api.focusApp()) },
          icon("external"),
          h("span", { text: t("openInApp") }),
        ),
      ),
    }),
    "locked",
  );
  requestAnimationFrame(() => input.focus());
}

// ---------------------------------------------------------------------------
// Unlocked
// ---------------------------------------------------------------------------

function showUnlocked(status, initialTab = "page") {
  const ctx = {
    vaultName: status?.vaultName || "Keystead",
    tab: initialTab,
    tabInfo: null,
    matches: null,
    matchIds: new Set(),
    query: "",
  };

  const header = h(
    "header",
    { class: "topbar" },
    h(
      "div",
      { class: "brand" },
      logo(30),
      h(
        "div",
        { class: "brand-text" },
        h("div", { class: "vault-name", text: ctx.vaultName, title: ctx.vaultName }),
        h("div", { class: "vault-state" }, h("span", { class: "dot", "aria-hidden": "true" }), t("unlocked")),
      ),
    ),
    h(
      "div",
      { class: "topbar-actions" },
      iconButton("external", t("openApp"), () => guarded(() => api.focusApp())),
      iconButton("lock", t("lock"), () =>
        guarded(async () => {
          await api.lock();
          showLocked();
        }),
      ),
    ),
  );

  const tabs = [
    ["page", t("tabPage")],
    ["search", t("tabSearch")],
    ["generator", t("tabGenerator")],
  ].map(([id, label]) =>
    h("button", { class: "tab", type: "button", role: "tab", "data-tab": id, onclick: () => select(id) }, label),
  );
  const content = h("div", { class: "content", role: "tabpanel" });
  const footer = h("div", { class: "footer", hidden: true });
  const screen = h("section", { class: "screen main" }, header, h("nav", { class: "tabs", role: "tablist" }, tabs), content, footer);
  mount(screen, "unlocked");

  function select(tab) {
    ctx.tab = tab;
    for (const node of tabs) node.setAttribute("aria-selected", String(node.dataset.tab === tab));
    content.replaceChildren();
    footer.replaceChildren();
    footer.hidden = true;
    content.scrollTop = 0;
    if (tab === "page") renderPage();
    else if (tab === "search") renderSearch();
    else renderGenerator();
  }

  function setFooter(...nodes) {
    footer.replaceChildren(...nodes);
    footer.hidden = nodes.length === 0;
  }

  // --- Actions on logins -----------------------------------------------------

  async function fill(item, btn) {
    setBusy(btn, true);
    try {
      await api.fill(ctx.tabInfo.tabId, item.id);
      if (api.demo) {
        setBusy(btn, false);
        toast(t("filled"));
      } else {
        api.close();
      }
    } catch (err) {
      setBusy(btn, false);
      if (!routeError(err)) toast(errorText(err?.code), "error");
    }
  }

  async function copy(text, message) {
    try {
      await api.copy(text);
      toast(message);
    } catch {
      toast(errorText("clipboard"), "error");
    }
  }

  function copyPassword(item) {
    return guarded(async () => {
      const login = await api.getLogin(item.id);
      await copy(login.password, t("copiedPassword"));
    });
  }

  function copyTotp(item) {
    return guarded(async () => {
      const code = await api.getTotp(item.id);
      await copy(String(code.code).replace(/\s+/g, ""), t("copiedTotp", String(code.remaining)));
    });
  }

  function loginRow(item) {
    const canFill = ctx.tabInfo?.web === true && ctx.matchIds.has(item.id);
    const copyButtons = h(
      "div",
      { class: "row-copy" },
      item.subtitle ? iconButton("user", t("copyUsername"), () => copy(item.subtitle, t("copiedUsername")), "sm") : null,
      iconButton("key", t("copyPassword"), () => copyPassword(item), "sm"),
      item.hasTotp ? iconButton("clock", t("copyTotp"), () => copyTotp(item), "sm") : null,
    );
    const actions = h("div", { class: "row-actions" });
    if (canFill) {
      actions.append(h("button", { class: "btn-fill", type: "button", onclick: (ev) => fill(item, ev.currentTarget) }, t("fill")));
    } else {
      actions.append(copyButtons);
    }
    return h(
      "div",
      { class: canFill ? "row with-fill" : "row" },
      avatar(item.name),
      h(
        "div",
        { class: "row-text" },
        h("div", { class: "row-name", text: item.name || item.subtitle || "—", title: item.name }),
        h("div", { class: "row-sub", text: item.subtitle || t("noUsername"), title: item.subtitle }),
      ),
      canFill ? copyButtons : null,
      actions,
    );
  }

  function list(items) {
    return h("div", { class: "list" }, items.map(loginRow));
  }

  // --- "Diese Seite" -------------------------------------------------------------

  function renderPage() {
    const info = ctx.tabInfo;
    if (!info) {
      content.append(skeletonList(2));
      return;
    }
    if (!info.web) {
      content.append(emptyState("globe", t("notWebPageTitle"), t("notWebPageText")));
      return;
    }
    const count = ctx.matches ? ctx.matches.length : null;
    content.append(
      h(
        "div",
        { class: "site" },
        avatar(info.host, "sm"),
        h(
          "div",
          { class: "site-text" },
          h("div", { class: "site-host", text: info.host, title: info.url }),
          h("div", { class: "site-count", text: count === null ? "…" : count === 1 ? t("loginCountOne") : t("loginCount", String(count)) }),
        ),
        info.insecure ? null : h("span", { class: "secure", title: t("secureConnection") }, icon("shield")),
      ),
    );
    if (info.insecure) content.append(h("div", { class: "notice warning" }, icon("alert"), h("span", { text: t("insecurePage") })));

    if (!ctx.matches) content.append(skeletonList(2));
    else if (ctx.matches.length) content.append(list(ctx.matches));
    else content.append(emptyState("key", t("noPageLoginsTitle"), t("noPageLoginsText")));

    if (info.neverSave) {
      content.append(
        h(
          "div",
          { class: "notice info" },
          h("span", { text: t("neverSaveActive") }),
          h(
            "button",
            {
              class: "link-btn",
              type: "button",
              onclick: () =>
                guarded(async () => {
                  await api.neverRemove(info.hostname);
                  info.neverSave = false;
                  if (ctx.tab === "page") select("page");
                }),
            },
            t("neverSaveUndo"),
          ),
        ),
      );
    }
    setFooter(button(t("addLogin"), { kind: "secondary", iconName: "plus", onclick: () => showAddLogin(ctx) }));
  }

  // --- "Suche" -------------------------------------------------------------------

  function renderSearch() {
    const input = h("input", {
      type: "search",
      class: "input",
      placeholder: t("searchPlaceholder"),
      "aria-label": t("searchPlaceholder"),
      spellcheck: "false",
      autocomplete: "off",
    });
    input.value = ctx.query;
    const results = h("div", { class: "results" });
    content.append(h("div", { class: "search-box" }, icon("search"), input), results);

    let timer = 0;
    let seq = 0;
    const run = async () => {
      const query = input.value.trim();
      ctx.query = input.value;
      const mine = ++seq;
      if (!query) {
        results.replaceChildren(emptyState("search", t("searchHintTitle"), t("searchHint")));
        return;
      }
      try {
        const found = await api.search(query);
        if (mine !== seq || ctx.tab !== "search") return;
        results.replaceChildren(found.length ? list(found) : emptyState("search", t("noResults", query), ""));
      } catch (err) {
        if (mine === seq && !routeError(err)) results.replaceChildren(emptyState("alert", errorText(err?.code), ""));
      }
    };
    input.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(run, 200);
    });
    run();
    requestAnimationFrame(() => input.focus());
  }

  // --- "Generator" -------------------------------------------------------------

  async function renderGenerator() {
    const stored = await api.loadGeneratorOptions();
    if (ctx.tab !== "generator") return;
    const options = { ...GENERATOR_DEFAULTS };
    for (const key of Object.keys(GENERATOR_DEFAULTS)) {
      if (stored && typeof stored[key] === typeof GENERATOR_DEFAULTS[key]) options[key] = stored[key];
    }
    options.length = clamp(options.length, 5, 128);
    options.words = clamp(options.words, 3, 20);

    let password = "";
    let seq = 0;
    let debounce = 0;

    const output = h("div", { class: "gen-output", "aria-live": "polite" });
    const box = h(
      "div",
      { class: "gen-box" },
      output,
      h(
        "div",
        { class: "gen-box-actions" },
        iconButton("refresh", t("regenerate"), () => generate()),
        iconButton("copy", t("copy"), () => password && copy(password, t("copied"))),
      ),
    );
    const kindButtons = [
      ["password", t("generatorPassword")],
      ["passphrase", t("generatorPassphrase")],
    ].map(([kind, label]) =>
      h("button", {
        type: "button",
        "aria-pressed": String(options.kind === kind),
        text: label,
        onclick: () => {
          if (options.kind === kind) return;
          options.kind = kind;
          kindButtons.forEach((b, i) => b.setAttribute("aria-pressed", String(["password", "passphrase"][i] === kind)));
          renderControls();
          changed(true);
        },
      }),
    );
    const controls = h("div", { class: "panel" });
    content.append(box, h("div", { class: "seg", role: "group" }, kindButtons), controls);
    renderControls();

    const copyButton = button(t("copy"), { kind: "secondary", iconName: "copy", onclick: () => password && copy(password, t("copied")) });
    const fillButton = button(t("fillGenerated"), {
      kind: "primary",
      onclick: async (ev) => {
        if (!password) return;
        const btn = ev.currentTarget;
        setBusy(btn, true);
        try {
          if (!ctx.tabInfo?.web) throw Object.assign(new Error("no_field"), { code: "no_field" });
          await api.fillGenerated(ctx.tabInfo.tabId, password);
          if (api.demo) toast(t("insertedPassword"));
          else api.close();
        } catch (err) {
          if (!routeError(err)) toast(errorText(err?.code), "error");
        } finally {
          setBusy(btn, false);
        }
      },
    });
    setFooter(copyButton, fillButton);
    generate();

    function renderControls() {
      controls.replaceChildren();
      if (options.kind === "password") {
        controls.append(
          rangeOption(t("length"), "length", 5, 128),
          charsetChips(),
          switchOption(t("avoidAmbiguous"), "avoidAmbiguous"),
        );
      } else {
        const separator = h("input", { class: "num", type: "text", maxlength: 3, "aria-label": t("separator"), spellcheck: "false" });
        separator.value = options.separator;
        separator.addEventListener("input", () => {
          options.separator = separator.value;
          changed();
        });
        controls.append(
          rangeOption(t("words"), "words", 3, 20),
          h("label", { class: "opt" }, h("span", { class: "opt-label", text: t("separator") }), separator),
          switchOption(t("capitalize"), "capitalize"),
          switchOption(t("includeNumber"), "includeNumber"),
        );
      }
    }

    function rangeOption(label, key, min, max) {
      const range = h("input", { type: "range", min, max, step: 1, "aria-label": label });
      const number = h("input", { class: "num", type: "number", min, max, "aria-label": label });
      range.value = String(options[key]);
      number.value = String(options[key]);
      range.addEventListener("input", () => {
        number.value = range.value;
        options[key] = Number(range.value);
        changed();
      });
      number.addEventListener("change", () => {
        const value = clamp(Number.parseInt(number.value, 10), min, max);
        number.value = String(value);
        range.value = String(value);
        options[key] = value;
        changed(true);
      });
      return h("div", { class: "opt opt-range" }, h("span", { class: "opt-label", text: label }), number, range);
    }

    /** Toggle chips for the character sets; at least one must stay enabled. */
    function charsetChips() {
      const sets = [
        ["uppercase", "A–Z"],
        ["lowercase", "a–z"],
        ["digits", "0–9"],
        ["symbols", "!@#"],
      ];
      const chips = sets.map(([key, label]) =>
        h("button", {
          type: "button",
          class: "chip",
          text: label,
          title: t(key),
          "aria-label": t(key),
          "aria-pressed": String(options[key] === true),
          onclick: (ev) => {
            const enable = !options[key];
            if (!enable && !sets.some(([other]) => other !== key && options[other])) return;
            options[key] = enable;
            ev.currentTarget.setAttribute("aria-pressed", String(enable));
            changed(true);
          },
        }),
      );
      return h("div", { class: "opt chips", role: "group" }, chips);
    }

    function switchOption(label, key) {
      const input = h("input", { type: "checkbox", role: "switch" });
      input.checked = options[key] === true;
      input.addEventListener("change", () => {
        options[key] = input.checked;
        changed(true);
      });
      return h(
        "label",
        { class: "opt" },
        h("span", { class: "opt-label", text: label }),
        h("span", { class: "switch" }, input, h("span", { class: "track", "aria-hidden": "true" })),
      );
    }

    function changed(immediate = false) {
      api.saveGeneratorOptions({ ...options });
      clearTimeout(debounce);
      // Each generation is stored in the vault's generator history: wait until the user settled.
      debounce = setTimeout(generate, immediate ? 0 : 350);
    }

    async function generate() {
      const mine = ++seq;
      output.classList.add("busy");
      try {
        const next = await api.generate({ ...options });
        if (mine !== seq) return;
        password = next;
        output.classList.toggle("phrase", options.kind === "passphrase");
        renderPassword(output, next);
      } catch (err) {
        if (mine === seq && !routeError(err)) toast(errorText(err?.code), "error");
      } finally {
        if (mine === seq) output.classList.remove("busy");
      }
    }
  }

  // --- Data loading --------------------------------------------------------------

  async function loadPageData() {
    try {
      ctx.tabInfo = await api.tabInfo();
      if (ctx.tab === "page") select("page");
      ctx.matches = ctx.tabInfo.web ? await api.matches(ctx.tabInfo.tabId) : [];
      ctx.matchIds = new Set(ctx.matches.map((m) => m.id));
      if (ctx.tab === "page") select("page");
    } catch (err) {
      if (!routeError(err)) {
        ctx.matches = [];
        if (ctx.tab === "page") select("page");
        toast(errorText(err?.code), "error");
      }
    }
  }

  select(initialTab);
  loadPageData();
}

function clamp(value, min, max) {
  return Number.isFinite(value) ? Math.min(max, Math.max(min, Math.round(value))) : min;
}

/** Shows a password with digits and symbols colored (text nodes only, no HTML). */
function renderPassword(node, password) {
  node.replaceChildren();
  let run = "";
  let runClass = null;
  const flush = () => {
    if (!run) return;
    node.append(runClass ? h("span", { class: runClass, text: run }) : document.createTextNode(run));
    run = "";
  };
  for (const ch of password) {
    const cls = /\d/.test(ch) ? "d" : /[\p{L}]/u.test(ch) ? null : "s";
    if (cls !== runClass) {
      flush();
      runClass = cls;
    }
    run += ch;
  }
  flush();
}

// ---------------------------------------------------------------------------
// Add login
// ---------------------------------------------------------------------------

function showAddLogin(ctx) {
  const info = ctx.tabInfo?.web ? ctx.tabInfo : null;
  const name = h("input", { class: "input", id: "add-name", type: "text", autocomplete: "off", spellcheck: "false" });
  const url = h("input", { class: "input", id: "add-url", type: "url", autocomplete: "off", spellcheck: "false" });
  const username = h("input", { class: "input", id: "add-user", type: "text", autocomplete: "off", spellcheck: "false" });
  name.value = info ? info.host : "";
  url.value = info ? info.origin : "";
  let passwordInput = null;
  const generateBtn = iconButton("wand", t("generate"), async () => {
    const password = await guarded(() => api.generate(null));
    if (typeof password === "string" && passwordInput) {
      passwordInput.value = password;
      passwordInput.type = "text";
    }
  });
  generateBtn.tabIndex = -1;
  const secret = secretInput({ id: "add-pass", "aria-label": t("password") }, [generateBtn]);
  passwordInput = secret.input;
  const error = h("p", { class: "field-error", role: "alert", hidden: true });

  const back = () => showUnlocked({ state: "unlocked", vaultName: ctx.vaultName }, "page");
  const saveButton = button(t("save"), { kind: "primary", type: "submit" });
  const form = h(
    "form",
    {
      class: "form",
      id: "add-form",
      onsubmit: async (ev) => {
        ev.preventDefault();
        error.hidden = true;
        if (!name.value.trim()) {
          error.textContent = t("nameRequired");
          error.hidden = false;
          name.focus();
          return;
        }
        if (!secret.input.value) {
          error.textContent = t("passwordRequired");
          error.hidden = false;
          secret.input.focus();
          return;
        }
        setBusy(saveButton, true);
        try {
          await api.saveLogin({
            name: name.value.trim(),
            url: url.value.trim(),
            username: username.value.trim(),
            password: secret.input.value,
          });
          toast(t("saved"));
          back();
        } catch (err) {
          setBusy(saveButton, false);
          if (!routeError(err)) {
            error.textContent = errorText(err?.code);
            error.hidden = false;
          }
        }
      },
    },
    field(t("name"), "add-name", name),
    field(t("website"), "add-url", url),
    field(t("username"), "add-user", username),
    field(t("password"), "add-pass", secret.wrap),
    error,
  );
  saveButton.setAttribute("form", "add-form");

  mount(
    h(
      "section",
      { class: "screen main" },
      h("header", { class: "subbar" }, iconButton("back", t("back"), back), h("h1", { text: t("addTitle") })),
      h("div", { class: "content" }, form),
      h("div", { class: "footer" }, button(t("cancel"), { kind: "secondary", onclick: back }), saveButton),
    ),
    "add",
  );
  requestAnimationFrame(() => (username.value ? secret.input : username).focus());
}

function field(label, id, control) {
  return h("div", { class: "field" }, h("label", { for: id, text: label }), control);
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function boot() {
  document.documentElement.lang = uiLanguage();
  document.title = "Keystead";
  const loadingTimer = setTimeout(showLoading, 150);
  try {
    const pairing = await api.pairState().catch(() => null);
    if (pairing) {
      clearTimeout(loadingTimer);
      showPairing(pairing);
      return;
    }
    const status = await api.status(true);
    clearTimeout(loadingTimer);
    route(status);
  } catch (err) {
    clearTimeout(loadingTimer);
    if (!routeError(err)) showAppUnavailable();
  }
}

boot();
