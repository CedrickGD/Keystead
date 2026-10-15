// Keystead popup. States: host missing → app unavailable → not paired (pairing)
// → locked → unlocked (tabs "Diese Seite", "Suche", "Generator", add login,
// settings). All work happens in the service worker; this page only renders.
// Lists: ↑/↓ select a login, Enter fills it; logins with 2FA show a countdown
// of the current code (the code itself never reaches the popup).
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

const SVG_NS = "http://www.w3.org/2000/svg";

function svg(tag, attrs = {}) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, String(value));
  return node;
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

/** A website icon from the app: a PNG data URL of reasonable size. */
function isIconUrl(value) {
  return typeof value === "string" && value.startsWith("data:image/png;base64,") && value.length <= 32 * 1024;
}

/**
 * Row avatar: the website icon the app sent (`iconUrl`), else – also if it
 * does not decode – a tinted letter tile.
 */
function avatar(name, size = "", iconUrl = null) {
  if (isIconUrl(iconUrl)) {
    const node = h("span", { class: `avatar site ${size}`.trim(), "aria-hidden": "true" });
    const img = h("img", { src: iconUrl, alt: "", draggable: "false" });
    img.addEventListener("error", () => node.replaceWith(avatar(name, size)), { once: true });
    node.append(img);
    return node;
  }
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

/** Counts mounted screens (except the loading screen), so async screen builders can tell they were overtaken. */
let mountSeq = 0;

/** Notice about a newer extension version the app delivers (shown above every screen), or null. */
let extensionNotice = null;

function mount(node, name) {
  if (name !== "pairing") {
    stopPairingWatch?.();
    stopPairingWatch = null;
    clearInterval(pairingPoll);
    pairingPoll = 0;
  }
  if (name !== "loading") mountSeq += 1;
  app.dataset.screen = name;
  app.replaceChildren(...(extensionNotice && name !== "loading" ? [extensionNotice] : []), node);
}

/**
 * The app has a newer version of this extension in its extension folder
 * (`status.extensionUpdate`): either the service worker is about to reload
 * from there, or – it was loaded from another folder – the user has to load
 * the app's folder once.
 */
function setExtensionUpdate(update) {
  if (!update || typeof update.version !== "string") {
    extensionNotice = null;
    return;
  }
  if (update.reloading) {
    extensionNotice = h(
      "div",
      { class: "ext-update", role: "status" },
      icon("refresh"),
      h("span", { text: t("extUpdateReloading", update.version) }),
    );
    return;
  }
  const dir = typeof update.dir === "string" ? update.dir : "";
  // Collapsed to one line by default: the screens below must keep fitting.
  const details = h(
    "div",
    { class: "ext-update-details", id: "ext-update-details", hidden: true },
    h("p", { text: t("extUpdateText") }),
    dir
      ? h(
          "div",
          { class: "ext-update-path" },
          h("code", { text: dir, title: dir }),
          iconButton("copy", t("extUpdateCopyPath"), () =>
            api
              .copy(dir)
              .then(() => toast(t("extUpdatePathCopied")))
              .catch(() => toast(t("errorGeneric", "clipboard"), "error")),
            "sm",
          ),
        )
      : null,
    h("p", { class: "ext-update-hint", text: t("extUpdateHint") }),
  );
  const toggle = h(
    "button",
    {
      class: "link-btn ext-update-toggle",
      type: "button",
      "aria-expanded": "false",
      "aria-controls": "ext-update-details",
      onclick: () => {
        const open = details.hidden;
        details.hidden = !open;
        toggle.setAttribute("aria-expanded", String(open));
        extensionNotice?.classList.toggle("open", open);
      },
    },
    t("extUpdateDetails"),
    icon("chevron"),
  );
  extensionNotice = h(
    "div",
    { class: "ext-update notice-card", role: "status" },
    h("div", { class: "ext-update-head" }, icon("alert"), h("strong", { text: t("extUpdateTitle", update.version) }), toggle),
    details,
  );
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
    setExtensionUpdate(status?.extensionUpdate);
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
// Vaults & unlocking (locked screen, switching vaults)
// ---------------------------------------------------------------------------

const CONNECTION_ERRORS = new Set(["not_paired", "host_missing", "host_forbidden", "app_unavailable", "timeout"]);

/**
 * The app's vaults ({ vaults: [{ id, name }], currentVaultId, lastVaultId }), or null if it
 * cannot list them (an app without `list_vaults` answers invalid_request): the popup then
 * unlocks the app's last used vault as before. Pairing and connection errors are thrown.
 */
async function loadVaults() {
  try {
    const list = await api.listVaults();
    return list && Array.isArray(list.vaults) && list.vaults.length ? list : null;
  } catch (err) {
    if (CONNECTION_ERRORS.has(err?.code)) throw err;
    return null;
  }
}

/** The open vault, else the one last unlocked in this browser, else the app's last used one, else the first. */
async function preselectedVault(list) {
  const known = (id) => list.vaults.find((v) => v.id === id) ?? null;
  return known(list.currentVaultId) ?? known(await api.loadChosenVault()) ?? known(list.lastVaultId) ?? list.vaults[0];
}

function vaultChip(name) {
  return h("div", { class: "vault-chip", title: name }, icon("vault"), h("span", { text: name }));
}

/** Locked: the master password form, with a vault selector if the app has several vaults. */
async function showLocked(error = null) {
  const seq = mountSeq;
  let list;
  try {
    list = await loadVaults();
  } catch (err) {
    if (seq === mountSeq && !routeError(err)) showAppUnavailable();
    return;
  }
  const selected = list ? await preselectedVault(list) : null;
  if (seq !== mountSeq) return; // another screen was shown meanwhile
  const open = list?.vaults.find((v) => v.id === list.currentVaultId);
  if (open) {
    // Unlocked in the meantime (e.g. in the app).
    showUnlocked({ state: "unlocked", vaultName: open.name, vaultId: open.id });
    return;
  }
  renderUnlock({ vaults: list?.vaults ?? [], selected, error });
}

/**
 * The master password form.
 * Locked: several `vaults` → a selector (preselected with `selected`); one → its name as
 * subtitle; none known → the app picks its last used vault.
 * Switching (`switching` = { from, back }): unlocks `selected` while the vault `from` stays
 * open until the app accepted the password; "Zurück" (`back`) returns to it.
 */
function renderUnlock({ vaults, selected, error = null, switching = null }) {
  const { input, wrap } = secretInput({
    id: "master",
    autocomplete: "current-password",
    placeholder: t("masterPassword"),
    "aria-label": t("masterPassword"),
  });
  const errorNode = h("p", { class: "field-error", role: "alert", hidden: !error, text: error || "" });
  const submit = button(t("unlock"), { kind: "primary", type: "submit", wide: true });
  const showError = (text, invalid = false) => {
    errorNode.textContent = text;
    errorNode.hidden = false;
    input.classList.toggle("invalid", invalid);
  };

  let select = null;
  if (!switching && vaults.length > 1) {
    select = h(
      "select",
      { id: "vault", class: "input" },
      vaults.map((v) => h("option", { value: v.id, text: v.name })),
    );
    select.value = selected?.id ?? vaults[0].id;
    select.addEventListener("change", () => {
      errorNode.hidden = true;
      input.classList.remove("invalid");
    });
  }
  const vaultId = () => (select ? select.value : selected?.id ?? null);
  const vaultName = () => (select ? select.selectedOptions[0]?.textContent : selected?.name) || null;

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
        if (select) select.disabled = true;
        errorNode.hidden = true;
        input.classList.remove("invalid");
        try {
          const result = await api.unlock(password, vaultId());
          input.value = "";
          if (result?.vaultId) await api.saveChosenVault(result.vaultId);
          showUnlocked({ state: "unlocked", vaultName: result?.vaultName || vaultName(), vaultId: result?.vaultId ?? null });
        } catch (err) {
          setBusy(submit, false);
          if (select) select.disabled = false;
          if (err?.code === "wrong_password") {
            // The selection stays; with a switch the open vault stays unlocked.
            showError(t("wrongPassword"), true);
            input.select();
            input.focus();
          } else if (err?.code === "not_found" && vaultId()) {
            // Deleted meanwhile (in the app or the terminal UI).
            if (switching) {
              switching.back();
              toast(t("vaultGone"), "error");
            } else {
              showLocked(t("vaultGone"));
            }
          } else if (!routeError(err)) {
            // Without a vault id the app answers `not_found` when it cannot tell which vault
            // to open (an app without `list_vaults`, several vaults, none used yet).
            showError(err?.code === "not_found" ? t("unlockNoVault") : errorText(err?.code));
          }
        }
      },
    },
    select
      ? [
          h(
            "div",
            { class: "field" },
            h("label", { for: "vault", text: t("vault") }),
            h("div", { class: "select-wrap" }, icon("vault"), select, icon("chevron", "ico chevron")),
          ),
          h("div", { class: "field" }, h("label", { for: "master", text: t("masterPassword") }), wrap),
        ]
      : wrap,
    errorNode,
    submit,
    switching ? button(t("back"), { kind: "ghost", wide: true, onclick: switching.back }) : null,
  );

  const subtitle = switching || !select ? (selected ? vaultChip(selected.name) : null) : null;
  let lead = t("lockedText");
  if (switching) lead = t("switchText", [selected.name, switching.from.name]);
  else if (select) lead = t("lockedTextChoose");
  mount(
    centered({
      artwork: switching ? art("vault", "accent") : brandArt(),
      title: switching ? t("switchVault") : t("lockedTitle"),
      body: [subtitle, h("p", { class: "lead", text: lead }), form],
      footer: switching
        ? null
        : h(
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
    switching ? "switch" : "locked",
  );
  requestAnimationFrame(() => input.focus());
}

// ---------------------------------------------------------------------------
// Unlocked
// ---------------------------------------------------------------------------

function showUnlocked(status, initialTab = "page") {
  const ctx = {
    vaultName: status?.vaultName || "Keystead",
    vaultId: status?.vaultId ?? null,
    tab: initialTab,
    tabInfo: null,
    matches: null,
    matchIds: new Set(),
    query: "",
    /** itemId → { period, remaining } of logins with 2FA (countdown only). */
    totp: new Map(),
    /** Index of the selected row of the shown list (keyboard). */
    active: 0,
  };

  // Becomes a vault switcher once the app reports more than one vault (setupVaultSwitcher).
  const vaultNameNode = h("div", { class: "vault-name", text: ctx.vaultName, title: ctx.vaultName });
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
        vaultNameNode,
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
      iconButton("sliders", t("settings"), () => showSettings(ctx)),
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
    if (tab !== ctx.tab) ctx.active = 0;
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

  // Passwords and TOTP codes are copied by the app (cleared after the user's
  // clipboard timeout, excluded from clipboard history); see api.copyField.
  function copyPassword(item) {
    return guarded(async () => {
      await api.copyField(item.id, "password");
      toast(t("copiedPassword"));
    });
  }

  function copyTotp(item) {
    return guarded(async () => {
      const result = await api.copyField(item.id, "totp");
      toast(t("copiedTotp", String(result?.remaining ?? "")));
    });
  }

  function copyGenerated(password) {
    if (!password) return undefined;
    return guarded(async () => {
      await api.copySecret(password);
      toast(t("copied"));
    });
  }

  // --- 2FA countdown ------------------------------------------------------------

  const RING_LENGTH = 2 * Math.PI * 9.5;

  /** Countdown ring of a login's 2FA code ("Diese Seite"); a click copies the code through the app. */
  function totpRing(item) {
    const progress = svg("circle", { class: "progress", cx: 12, cy: 12, r: 9.5, "stroke-dasharray": RING_LENGTH.toFixed(2) });
    const ring = svg("svg", { viewBox: "0 0 24 24", "aria-hidden": "true" });
    ring.append(svg("circle", { class: "track", cx: 12, cy: 12, r: 9.5 }), progress);
    const node = h("button", { class: "totp-ring pending", type: "button", "data-totp": item.id, onclick: () => copyTotp(item) }, ring, h("span", { class: "totp-sec" }));
    updateRing(node);
    return node;
  }

  /** Seconds left follow the clock (TOTP periods start at multiples of the period since 1970). */
  function updateRing(node) {
    const timer = ctx.totp.get(node.dataset.totp);
    const progress = node.querySelector(".progress");
    const label = node.querySelector(".totp-sec");
    if (!timer) {
      node.title = t("copyTotp");
      node.setAttribute("aria-label", t("copyTotp"));
      progress.style.strokeDashoffset = RING_LENGTH.toFixed(2);
      return;
    }
    const remaining = timer.period - (Math.floor(Date.now() / 1000) % timer.period);
    node.classList.remove("pending");
    node.classList.toggle("low", remaining <= 5);
    label.textContent = String(remaining);
    // A new code: jump back to full instead of animating backwards.
    progress.style.transition = remaining === timer.period || node.dataset.shown !== "1" ? "none" : "";
    progress.style.strokeDashoffset = (RING_LENGTH * (1 - remaining / timer.period)).toFixed(2);
    node.dataset.shown = "1";
    node.title = t("totpCountdown", String(remaining));
    node.setAttribute("aria-label", node.title);
  }

  function updateRings() {
    for (const node of content.querySelectorAll(".totp-ring")) updateRing(node);
  }

  async function loadTotpTimers(items) {
    const ids = items.filter((item) => item.hasTotp && !ctx.totp.has(item.id)).map((item) => item.id);
    if (!ids.length) return;
    try {
      const timers = await api.totpTimers(ids);
      for (const timer of Array.isArray(timers) ? timers : []) {
        if (timer && typeof timer.id === "string" && Number.isInteger(timer.period) && timer.period > 0) ctx.totp.set(timer.id, timer);
      }
      updateRings();
    } catch {
      // The rings stay empty; copying the code still works.
    }
  }

  const ticker = setInterval(() => {
    if (!screen.isConnected) {
      clearInterval(ticker);
      return;
    }
    updateRings();
  }, 1000);

  // --- Keyboard: ↑/↓ select a login, Enter fills it -------------------------------

  function listRows() {
    return [...content.querySelectorAll(".list .row")];
  }

  function setActive(index, scroll = true) {
    const rows = listRows();
    if (!rows.length) return;
    ctx.active = Math.max(0, Math.min(rows.length - 1, index));
    rows.forEach((row, i) => {
      row.classList.toggle("active", i === ctx.active);
      row.setAttribute("aria-selected", String(i === ctx.active));
    });
    if (scroll) rows[ctx.active].scrollIntoView({ block: "nearest" });
  }

  function onListKeys(ev) {
    if (!screen.isConnected) {
      document.removeEventListener("keydown", onListKeys);
      return;
    }
    if (ev.defaultPrevented || ev.altKey || ev.ctrlKey || ev.metaKey || ev.shiftKey || ctx.tab === "generator") return;
    if (ev.key !== "ArrowDown" && ev.key !== "ArrowUp" && ev.key !== "Enter") return;
    const target = ev.target;
    const inSearch = target instanceof HTMLInputElement && target.type === "search";
    // Buttons, menus and other inputs keep their own keys.
    if (!inSearch && target !== document.body && !target?.closest?.(".list")) return;
    const rows = listRows();
    if (!rows.length) return;
    if (ev.key === "Enter") {
      if (target instanceof HTMLButtonElement) return;
      const fillButton = rows[ctx.active]?.querySelector(".btn-fill");
      if (!fillButton || fillButton.disabled) return;
      ev.preventDefault();
      fillButton.click();
      return;
    }
    ev.preventDefault();
    setActive(ctx.active + (ev.key === "ArrowDown" ? 1 : -1));
  }
  document.addEventListener("keydown", onListKeys);

  function loginRow(item) {
    const canFill = ctx.tabInfo?.web === true && ctx.matchIds.has(item.id);
    const ring = ctx.tab === "page" && item.hasTotp ? totpRing(item) : null;
    const copyButtons = h(
      "div",
      { class: "row-copy" },
      item.subtitle ? iconButton("user", t("copyUsername"), () => copy(item.subtitle, t("copiedUsername")), "sm") : null,
      iconButton("key", t("copyPassword"), () => copyPassword(item), "sm"),
      item.hasTotp && !ring ? iconButton("clock", t("copyTotp"), () => copyTotp(item), "sm") : null,
    );
    const actions = h("div", { class: "row-actions" });
    if (canFill) {
      actions.append(...[ring, h("button", { class: "btn-fill", type: "button", onclick: (ev) => fill(item, ev.currentTarget) }, t("fill"))].filter(Boolean));
    } else {
      actions.append(...[copyButtons, ring].filter(Boolean));
    }
    let rowClass = "row";
    if (canFill) rowClass += ring ? " with-fill with-totp" : " with-fill";
    return h(
      "div",
      { class: rowClass, role: "option", "aria-selected": "false" },
      avatar(item.name, "", item.icon),
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
    return h("div", { class: "list", role: "listbox", "aria-label": t("tabPage") }, items.map(loginRow));
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
        avatar(info.host, "sm", ctx.matches?.find((m) => isIconUrl(m.icon))?.icon),
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
    else if (ctx.matches.length) {
      content.append(list(ctx.matches));
      setActive(ctx.active, false);
      if (ctx.matches.some((m) => ctx.matchIds.has(m.id))) content.append(h("p", { class: "kbd-hint", text: t("keyboardHint") }));
    } else content.append(emptyState("key", t("noPageLoginsTitle"), t("noPageLoginsText")));

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
        setActive(0, false);
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
        iconButton("copy", t("copy"), () => copyGenerated(password)),
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

    const copyButton = button(t("copy"), { kind: "secondary", iconName: "copy", onclick: () => copyGenerated(password) });
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
      loadTotpTimers(ctx.matches);
    } catch (err) {
      if (!routeError(err)) {
        ctx.matches = [];
        if (ctx.tab === "page") select("page");
        toast(errorText(err?.code), "error");
      }
    }
  }

  // --- Switching vaults ----------------------------------------------------------

  async function setupVaultSwitcher() {
    let list;
    try {
      list = await loadVaults();
    } catch {
      return; // the page data requests route to the right screen
    }
    const open = list?.vaults.find((v) => v.id === list.currentVaultId);
    if (!open || !screen.isConnected) return;
    ctx.vaultId = open.id;
    ctx.vaultName = open.name;
    if (list.vaults.length < 2) {
      vaultNameNode.textContent = open.name;
      vaultNameNode.title = open.name;
      return;
    }
    vaultNameNode.replaceWith(vaultSwitcher(list.vaults, open, showSwitch));
  }

  /** The unlock form for `target`; the open vault stays unlocked until it succeeds. */
  function showSwitch(target) {
    const from = { id: ctx.vaultId, name: ctx.vaultName };
    renderUnlock({
      vaults: [],
      selected: target,
      switching: { from, back: () => showUnlocked({ state: "unlocked", vaultName: from.name, vaultId: from.id }, ctx.tab) },
    });
  }

  select(initialTab);
  loadPageData();
  setupVaultSwitcher();
}

/**
 * The vault name in the header as a menu button: lists all vaults with the open one
 * checked; picking another calls `onPick(vault)`. Keyboard: Enter/Space/↓ open, ↑/↓/Home/End
 * move, Esc closes.
 */
function vaultSwitcher(vaults, open, onPick) {
  const trigger = h(
    "button",
    {
      class: "vault-switch",
      type: "button",
      title: t("switchVault"),
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      "aria-label": t("switchVaultLabel", open.name),
    },
    h("span", { class: "vault-name", text: open.name }),
    icon("chevron", "ico chevron"),
  );
  const wrap = h("div", { class: "vault-switcher" }, trigger);
  let menu = null;

  const items = () => [...menu.querySelectorAll('[role="menuitemradio"]')];
  const onOutside = (ev) => {
    if (!wrap.contains(ev.target)) close();
  };
  function close(focusTrigger = false) {
    if (!menu) return;
    menu.remove();
    menu = null;
    trigger.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", onOutside, true);
    if (focusTrigger) trigger.focus();
  }
  function openMenu(focusLast = false) {
    if (menu) return;
    menu = h(
      "div",
      { class: "menu", role: "menu", "aria-label": t("switchVault") },
      h("div", { class: "menu-caption", "aria-hidden": "true", text: t("vaults") }),
      vaults.map((vault) => {
        const checked = vault.id === open.id;
        return h(
          "button",
          {
            class: "menu-item",
            type: "button",
            role: "menuitemradio",
            "aria-checked": String(checked),
            tabindex: "-1",
            title: vault.name,
            onclick: () => {
              close(checked);
              if (!checked) onPick(vault);
            },
          },
          h("span", { class: "menu-check", "aria-hidden": "true" }, checked ? icon("check") : null),
          h("span", { class: "menu-text", text: vault.name }),
        );
      }),
    );
    menu.addEventListener("keydown", (ev) => {
      const list = items();
      const index = list.indexOf(document.activeElement);
      const focus = (i) => list[(i + list.length) % list.length].focus();
      if (ev.key === "ArrowDown") focus(index + 1);
      else if (ev.key === "ArrowUp") focus(index - 1);
      else if (ev.key === "Home") focus(0);
      else if (ev.key === "End") focus(list.length - 1);
      else if (ev.key === "Escape") close(true);
      else if (ev.key === "Tab") {
        close();
        return;
      } else return;
      ev.preventDefault();
      ev.stopPropagation();
    });
    wrap.append(menu);
    trigger.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", onOutside, true);
    const list = items();
    (focusLast ? list[list.length - 1] : list.find((n) => n.getAttribute("aria-checked") === "true") || list[0]).focus();
  }

  trigger.addEventListener("click", () => (menu ? close() : openMenu()));
  trigger.addEventListener("keydown", (ev) => {
    if (ev.key !== "ArrowDown" && ev.key !== "ArrowUp") return;
    ev.preventDefault();
    openMenu(ev.key === "ArrowUp");
  });
  return wrap;
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
// Settings
// ---------------------------------------------------------------------------

/** The extension's settings (lib/settings.js): stored right away when a switch changes. */
async function showSettings(ctx) {
  const settings = await api.loadSettings();
  const back = () => showUnlocked({ state: "unlocked", vaultName: ctx.vaultName, vaultId: ctx.vaultId }, ctx.tab);
  const setting = (key, label, hint) => {
    const input = h("input", { type: "checkbox", role: "switch", "aria-describedby": `hint-${key}` });
    input.checked = settings[key] === true;
    input.addEventListener("change", async () => {
      settings[key] = input.checked;
      try {
        await api.saveSettings({ ...settings });
        toast(t("settingsSaved"));
      } catch {
        input.checked = !input.checked;
        settings[key] = input.checked;
        toast(errorText("storage"), "error");
      }
    });
    return h(
      "label",
      { class: "setting" },
      h(
        "span",
        { class: "setting-text" },
        h("span", { class: "setting-label", text: label }),
        h("span", { class: "setting-hint", id: `hint-${key}`, text: hint }),
      ),
      h("span", { class: "switch" }, input, h("span", { class: "track", "aria-hidden": "true" })),
    );
  };
  mount(
    h(
      "section",
      { class: "screen main" },
      h("header", { class: "subbar" }, iconButton("back", t("back"), back), h("h1", { text: t("settingsTitle") })),
      h(
        "div",
        { class: "content" },
        h(
          "div",
          { class: "panel settings" },
          setting("autoCopyTotp", t("settingTotpCopy"), t("settingTotpCopyHint")),
          setting("suggestPasswords", t("settingSuggest"), t("settingSuggestHint")),
        ),
        versionFooter(),
      ),
    ),
    "settings",
  );
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

  const back = () => showUnlocked({ state: "unlocked", vaultName: ctx.vaultName, vaultId: ctx.vaultId }, "page");
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
    setExtensionUpdate(status?.extensionUpdate);
    route(status);
  } catch (err) {
    clearTimeout(loadingTimer);
    if (!routeError(err)) showAppUnavailable();
  }
}

boot();
