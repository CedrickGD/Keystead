// Keystead extension service worker.
//
// Owns the native messaging connection to the Keystead desktop app and is the
// only place that sees secrets coming from the app. Popup and content scripts
// talk to it with runtime messages ({ type, ... } → { ok, data | error }).
// Credentials are only released to a content script for a login that the app
// matches against the URL of *that* frame (sender.url, as known by the
// browser) – never against a URL the page or a content script provides.
//
// MV3 service workers are stopped when idle: all state that matters lives in
// chrome.storage (see lib/store.js); in-memory state here is only a cache or
// short-lived (an autofill request window, an open pairing port).

import { NativeBridge, BridgeError, PAIR_TIMEOUT_MS } from "./lib/bridge.js";
import * as store from "./lib/store.js";
import { parseWebUrl, siteKey, displayHost } from "./lib/sites.js";
import { extensionUpdateAction, parseVersion } from "./lib/version.js";

const bridge = new NativeBridge();
const EXTENSION_ORIGIN = new URL(chrome.runtime.getURL("")).origin;
const BADGE_COLOR = "#2f6fed";
/** After these failures, automatic (non-user) requests pause so we don't spawn failing hosts on every page. */
const BACKOFF_MS = { host_missing: 60_000, host_forbidden: 60_000, app_unavailable: 20_000, timeout: 20_000 };
/** How long frames of a tab may claim credentials after an autofill was triggered. */
const AUTOFILL_WINDOW_MS = 6_000;
/** Requests that work without pairing. */
const UNPAIRED_TYPES = new Set(["status", "pair", "focus_app"]);
/** Delay before the extension reloads itself after the app delivered a newer version (lets an open popup say so). */
const SELF_UPDATE_DELAY_MS = 2_500;
/** Requests whose success proves the vault is unlocked. */
const UNLOCKED_TYPES = new Set([
  "logins_for_url",
  "search",
  "get_login",
  "get_totp",
  "save_login",
  "update_password",
  "check_login_password",
  "copy_field",
  "copy_secret",
]);

const t = (key, substitutions) => chrome.i18n.getMessage(key, substitutions) || key;

function errorCode(err) {
  return err instanceof BridgeError ? err.code : "internal";
}

// ---------------------------------------------------------------------------
// Status and requests
// ---------------------------------------------------------------------------

/** Popup/content state for an error code. */
function stateForError(code) {
  switch (code) {
    case "host_missing":
    case "host_forbidden":
      return "host_missing";
    case "not_paired":
      return "not_paired";
    case "locked":
      return "locked";
    default:
      return "app_unavailable";
  }
}

async function updateStatus(patch) {
  const previous = (await store.getStatus()) || {};
  const status = {
    state: "unknown",
    vaultName: null,
    vaultId: null,
    appVersion: "",
    extensionVersion: null,
    extensionDir: null,
    error: null,
    ...previous,
    ...patch,
    ts: Date.now(),
  };
  await store.setStatus(status);
  const wasUnlocked = previous.state === "unlocked";
  // Badge counts belong to the vault that was open: drop them when it is
  // locked or another vault replaced it (the active tab is counted again by
  // the popup / refreshActiveTab).
  const switched = wasUnlocked && status.state === "unlocked" && previous.vaultId && status.vaultId && previous.vaultId !== status.vaultId;
  if ((wasUnlocked && status.state !== "unlocked") || switched) await clearAllBadges();
  return status;
}

/** The app's vault list with only well-formed entries ({ id, name } strings; ids it does not list → null). */
function vaultList(data) {
  const vaults = (Array.isArray(data?.vaults) ? data.vaults : [])
    .filter((v) => v && typeof v.id === "string" && v.id && typeof v.name === "string")
    .map((v) => ({ id: v.id, name: v.name }));
  const known = (id) => (typeof id === "string" && vaults.some((v) => v.id === id) ? id : null);
  return { vaults, currentVaultId: known(data?.currentVaultId), lastVaultId: known(data?.lastVaultId) };
}

async function clearCredentialsIfCurrent(used) {
  const current = await store.getCredentials();
  if (current && used && current.clientId === used.clientId && current.token === used.token) {
    await store.clearCredentials();
  }
}

async function noteSuccess(type, data) {
  if (type === "unlock") {
    await updateStatus({
      state: "unlocked",
      vaultName: typeof data?.vaultName === "string" ? data.vaultName : null,
      vaultId: typeof data?.vaultId === "string" ? data.vaultId : null,
      error: null,
    });
    return;
  }
  if (type === "lock") {
    await updateStatus({ state: "locked", vaultName: null, vaultId: null, error: null });
    return;
  }
  if (type === "list_vaults") {
    // The list tells which vault is open (if any): keep the cache in step.
    const list = vaultList(data);
    const open = list.vaults.find((v) => v.id === list.currentVaultId);
    await updateStatus(
      open
        ? { state: "unlocked", vaultName: open.name, vaultId: open.id, error: null }
        : { state: "locked", vaultName: null, vaultId: null, error: null },
    );
    return;
  }
  const cached = await store.getStatus();
  if (UNLOCKED_TYPES.has(type)) {
    if (cached?.state !== "unlocked" || cached.error) {
      await updateStatus({ state: "unlocked", error: null });
      // Learn the vault name in the background; failures only affect the header text.
      if (!cached?.vaultName || !cached?.vaultId) refreshStatus().catch(() => undefined);
    }
  } else if (cached?.error) {
    await updateStatus({ error: null });
  }
}

async function noteFailure(code, credentials) {
  switch (code) {
    case "host_missing":
    case "host_forbidden":
    case "app_unavailable":
    case "timeout":
      await updateStatus({ state: stateForError(code), error: code, vaultName: null, vaultId: null });
      break;
    case "locked":
      await updateStatus({ state: "locked", error: null, vaultName: null, vaultId: null });
      break;
    case "not_paired":
      // The app no longer knows this client (revoked in the app): pair again.
      await clearCredentialsIfCurrent(credentials);
      await updateStatus({ state: "not_paired", error: null, vaultName: null, vaultId: null });
      break;
    default:
      break;
  }
}

/**
 * Sends a request to the app with the stored pairing credentials.
 * `auto` marks background traffic (page loads, badge): it never runs
 * unpaired and pauses for a while after the host/app was unreachable.
 */
async function call(type, payload = {}, { auto = false } = {}) {
  const credentials = await store.getCredentials();
  if (!credentials && !UNPAIRED_TYPES.has(type)) throw new BridgeError("not_paired");
  if (auto) {
    const cached = await store.getStatus();
    const wait = cached?.error ? BACKOFF_MS[cached.error] : 0;
    if (wait && Date.now() - cached.ts < wait) throw new BridgeError(cached.error);
  }
  let data;
  try {
    data = await bridge.request(type, payload, credentials);
  } catch (err) {
    await noteFailure(errorCode(err), credentials).catch(() => undefined);
    throw err instanceof BridgeError ? err : new BridgeError("internal");
  }
  // The status cache is only a hint; failing to update it must not fail the request.
  await noteSuccess(type, data).catch(() => undefined);
  return data;
}

/** Asks the app for its status and caches the result. */
async function refreshStatus() {
  const credentials = await store.getCredentials();
  try {
    const data = await bridge.request("status", {}, credentials);
    const paired = data?.paired === true;
    if (!paired && credentials) await clearCredentialsIfCurrent(credentials);
    const state = !paired ? "not_paired" : data.unlocked === true ? "unlocked" : "locked";
    const status = await updateStatus({
      state,
      error: null,
      vaultName: state === "unlocked" && typeof data.vaultName === "string" ? data.vaultName : null,
      vaultId: state === "unlocked" && typeof data.vaultId === "string" ? data.vaultId : null,
      appVersion: typeof data?.appVersion === "string" ? data.appVersion : "",
      // Only paired clients get these (older apps: absent).
      extensionVersion: paired && parseVersion(data.extensionVersion) ? data.extensionVersion : null,
      extensionDir: paired && typeof data.extensionDir === "string" ? data.extensionDir.slice(0, 1024) : null,
    });
    await maybeSelfUpdate(status).catch(() => undefined);
    return status;
  } catch (err) {
    const code = errorCode(err);
    return updateStatus({ state: stateForError(code), error: code, vaultName: null, vaultId: null });
  }
}

function publicStatus(status) {
  return {
    state: status?.state ?? "unknown",
    vaultName: status?.vaultName ?? null,
    vaultId: status?.vaultId ?? null,
    appVersion: status?.appVersion ?? "",
    error: status?.error ?? null,
  };
}

// ---------------------------------------------------------------------------
// Self-update: the app keeps this extension's folder up to date
// ---------------------------------------------------------------------------

let selfUpdateScheduled = false;

/**
 * The app delivers a newer extension version than the one running (it has
 * rewritten its extension folder – most likely the one we were loaded from):
 * reload once for that version. A reload that did not help (loaded from
 * another folder) is not repeated; the popup shows a notice instead.
 */
async function maybeSelfUpdate(status) {
  if (selfUpdateScheduled || !status?.extensionVersion) return;
  const running = chrome.runtime.getManifest().version;
  const action = extensionUpdateAction(status.extensionVersion, running, await store.getReloadedFor());
  if (action !== "reload") return;
  // Never interrupt a pairing that waits for the user's confirmation.
  if ((await store.getPairing())?.state === "pending") return;
  await store.setReloadedFor(status.extensionVersion);
  selfUpdateScheduled = true;
  setTimeout(() => chrome.runtime.reload(), SELF_UPDATE_DELAY_MS);
}

/** For the popup: { version, dir, reloading } if the app delivers a newer extension, else null. */
async function extensionUpdateInfo(status) {
  if (!status?.extensionVersion) return null;
  const running = chrome.runtime.getManifest().version;
  const action = extensionUpdateAction(status.extensionVersion, running, await store.getReloadedFor());
  if (action === "current") return null;
  return {
    version: status.extensionVersion,
    dir: status.extensionDir ?? "",
    reloading: selfUpdateScheduled,
  };
}

// ---------------------------------------------------------------------------
// Badge, tabs, content script notifications
// ---------------------------------------------------------------------------

async function setBadge(tabId, count) {
  const text = count > 99 ? "99+" : count > 0 ? String(count) : "";
  try {
    await chrome.action.setBadgeText({ tabId, text });
  } catch {
    // The tab is gone.
  }
}

async function clearAllBadges() {
  try {
    const tabs = await chrome.tabs.query({});
    await Promise.all(tabs.map((tab) => setBadge(tab.id, 0)));
  } catch {
    // Best effort.
  }
}

/** Shows the number of matching logins for a tab (only while unlocked). */
async function refreshBadge(tab) {
  if (!tab || tab.id === undefined || !tab.active) return;
  const status = await store.getStatus();
  if (status?.state !== "unlocked" || !parseWebUrl(tab.url)) {
    await setBadge(tab.id, 0);
    return;
  }
  try {
    const matches = await call("logins_for_url", { url: tab.url }, { auto: true });
    await setBadge(tab.id, Array.isArray(matches) ? matches.length : 0);
  } catch {
    await setBadge(tab.id, 0);
  }
}

async function activeTab() {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  return tab ?? null;
}

async function refreshActiveTab() {
  const tab = await activeTab();
  if (!tab || tab.id === undefined) return;
  await refreshBadge(tab);
  if (parseWebUrl(tab.url)) notifyTab(tab.id, { type: "bg:refresh" });
}

/** Sends a message to a tab's content scripts (all frames unless frameId is given). */
function notifyTab(tabId, message, frameId) {
  const options = frameId === undefined ? undefined : { frameId };
  return chrome.tabs.sendMessage(tabId, message, options).catch(() => undefined);
}

function toast(tabId, key) {
  notifyTab(tabId, { type: "bg:toast", key }, 0);
}

async function openPopupOrApp() {
  if (typeof chrome.action.openPopup === "function") {
    try {
      await chrome.action.openPopup();
      return "popup";
    } catch {
      // No focused window, or not allowed in this browser version.
    }
  }
  try {
    await bridge.request("focus_app");
    return "app";
  } catch {
    return "none";
  }
}

// ---------------------------------------------------------------------------
// Pairing (runs here, not in the popup: the popup closes as soon as the user
// switches to the app to confirm the code)
// ---------------------------------------------------------------------------

let pairingConnection = null;

async function getPairingState() {
  const pairing = await store.getPairing();
  if (pairing?.state === "pending" && (!pairingConnection || Date.now() - pairing.startedAt > PAIR_TIMEOUT_MS)) {
    // The service worker was restarted while waiting – the request is gone.
    const failed = { ...pairing, state: "error", error: "interrupted" };
    await store.setPairing(failed);
    return failed;
  }
  return pairing;
}

async function startPairing(code, clientName) {
  const name = typeof clientName === "string" ? clientName.trim().slice(0, 100) : "";
  if (typeof code !== "string" || !/^\d{6}$/.test(code) || !name) throw new BridgeError("invalid_request");
  const current = await getPairingState();
  if (current?.state === "pending") return current;

  const pairing = { state: "pending", code, clientName: name, startedAt: Date.now() };
  await store.setPairing(pairing);
  const connection = bridge.dedicated();
  pairingConnection = connection;
  connection
    .request("pair", { clientName: name, code }, null, PAIR_TIMEOUT_MS)
    .then(async (data) => {
      if (pairingConnection !== connection) return;
      if (!data || typeof data.clientId !== "string" || typeof data.token !== "string" || !data.clientId || !data.token) {
        throw new BridgeError("internal");
      }
      await store.setCredentials(data.clientId, data.token);
      await store.setPairing({ ...pairing, state: "success" });
      await refreshStatus();
      await refreshActiveTab();
    })
    .catch(async (err) => {
      if (pairingConnection !== connection) return;
      const failure = errorCode(err);
      if (failure === "host_missing" || failure === "host_forbidden" || failure === "app_unavailable") {
        await updateStatus({ state: stateForError(failure), error: failure, vaultName: null, vaultId: null });
      }
      await store.setPairing({ ...pairing, state: failure === "pairing_denied" ? "denied" : "error", error: failure });
    })
    .finally(() => {
      connection.close();
      if (pairingConnection === connection) pairingConnection = null;
    });
  return pairing;
}

async function cancelPairing() {
  const connection = pairingConnection;
  pairingConnection = null;
  connection?.close();
  await store.setPairing(null);
}

// ---------------------------------------------------------------------------
// Filling
// ---------------------------------------------------------------------------

/** nonce → autofill window of one tab (see fillTab). In memory: lives only seconds. */
const autofillWindows = new Map();

/**
 * Releases the credentials of `itemId` for a page URL – only if the app
 * matches the login to that URL, and never into plain http pages unless the
 * login itself is stored for http.
 */
async function releaseLogin(itemId, pageUrl) {
  const parsed = parseWebUrl(pageUrl);
  if (!parsed) throw new BridgeError("not_fillable");
  const matches = await call("logins_for_url", { url: pageUrl });
  if (!Array.isArray(matches) || !matches.some((m) => m && m.id === itemId)) throw new BridgeError("not_found");
  const login = await call("get_login", { itemId });
  const uris = Array.isArray(login?.uris) ? login.uris : [];
  if (parsed.protocol === "http:" && !uris.some((u) => typeof u === "string" && /^http:\/\//i.test(u.trim()))) {
    throw new BridgeError("insecure");
  }
  await store.setLastUsed(siteKey(parsed.hostname), itemId);
  return {
    username: typeof login?.username === "string" ? login.username : "",
    password: typeof login?.password === "string" ? login.password : "",
  };
}

/**
 * Fills `itemId` into the top frame of a tab and its same-origin frames.
 * The frames are asked to claim the credentials themselves (cs:fill-request
 * with a nonce), so each frame is checked with its own URL.
 */
async function fillTab(tabId, itemId) {
  if (typeof itemId !== "string" || !itemId) throw new BridgeError("invalid_request");
  const tab = await chrome.tabs.get(tabId);
  const top = parseWebUrl(tab.url);
  if (!top) throw new BridgeError("not_fillable");

  const nonce = crypto.randomUUID();
  const outcome = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      autofillWindows.delete(nonce);
      reject(new BridgeError("no_fields"));
    }, AUTOFILL_WINDOW_MS);
    autofillWindows.set(nonce, {
      tabId,
      itemId,
      topOrigin: top.origin,
      expires: Date.now() + AUTOFILL_WINDOW_MS,
      settled: false,
      settle(err) {
        if (this.settled) return;
        this.settled = true;
        clearTimeout(timer);
        // Keep the window open briefly for further same-origin frames (split username/password frames).
        setTimeout(() => autofillWindows.delete(nonce), 1500);
        if (err) reject(err);
        else resolve({ filled: true });
      },
    });
  });
  outcome.catch(() => undefined);

  // Frames with a fillable form answer { hasForm: true }; no answer means no form anywhere.
  const probe = await chrome.tabs.sendMessage(tabId, { type: "bg:autofill", nonce, origin: top.origin }).catch(() => null);
  if (!probe?.hasForm) {
    autofillWindows.get(nonce)?.settle(new BridgeError("no_fields"));
  }
  return outcome;
}

async function handleFillRequest(msg, sender) {
  const tabId = sender.tab?.id;
  const url = parseWebUrl(sender.url);
  if (tabId === undefined || !url) throw new BridgeError("invalid_request");

  if (typeof msg.nonce === "string") {
    const pending = autofillWindows.get(msg.nonce);
    if (!pending || pending.tabId !== tabId || Date.now() > pending.expires) throw new BridgeError("expired");
    // Autofill (popup button / keyboard shortcut) only fills same-origin frames.
    const frameOrigin = sender.origin || url.origin;
    if (frameOrigin !== pending.topOrigin) throw new BridgeError("cross_origin");
    try {
      const credentials = await releaseLogin(pending.itemId, sender.url);
      pending.settle(null);
      return credentials;
    } catch (err) {
      pending.settle(err);
      throw err;
    }
  }

  // A click in the inline dropdown of this frame.
  if (typeof msg.itemId !== "string" || !msg.itemId) throw new BridgeError("invalid_request");
  return releaseLogin(msg.itemId, sender.url);
}

async function focusedFrameId(tabId) {
  const entry = await store.getFocusedFrame(tabId);
  return entry ? entry.frameId : 0;
}

/**
 * Inserts a generated password into the focused (or right-clicked) field of one
 * frame. A frame fills only the field that currently has its focus (see
 * content.js), so a frame that merely claimed the focus earlier gets nothing.
 */
async function fillGenerated(tabId, frameId, password, target) {
  if (typeof password !== "string" || !password) throw new BridgeError("invalid_request");
  const fillFrame = async (id) => {
    const response = await chrome.tabs
      .sendMessage(tabId, { type: "bg:fill-generated", password, target }, { frameId: id })
      .catch(() => null);
    return Number.isInteger(response?.filled) ? response.filled : 0;
  };
  let filled = await fillFrame(frameId);
  // The remembered frame may be stale (the focus went back to the page): the
  // top frame fills only if the focused field is its own.
  if (!filled && frameId !== 0 && target === "focused") filled = await fillFrame(0);
  if (!filled) throw new BridgeError("no_field");
  return { filled };
}

// ---------------------------------------------------------------------------
// Generator options
// ---------------------------------------------------------------------------

const GENERATOR_RULES = {
  kind: (v) => v === "password" || v === "passphrase",
  length: (v) => Number.isInteger(v) && v >= 5 && v <= 128,
  uppercase: (v) => typeof v === "boolean",
  lowercase: (v) => typeof v === "boolean",
  digits: (v) => typeof v === "boolean",
  symbols: (v) => typeof v === "boolean",
  minDigits: (v) => Number.isInteger(v) && v >= 0 && v <= 128,
  minSymbols: (v) => Number.isInteger(v) && v >= 0 && v <= 128,
  avoidAmbiguous: (v) => typeof v === "boolean",
  words: (v) => Number.isInteger(v) && v >= 3 && v <= 20,
  separator: (v) => typeof v === "string" && v.length <= 5,
  capitalize: (v) => typeof v === "boolean",
  includeNumber: (v) => typeof v === "boolean",
};

/** Keeps only known GeneratorOptions fields with valid values (the app fills in defaults). */
function sanitizeGeneratorOptions(raw) {
  const options = {};
  if (!raw || typeof raw !== "object") return options;
  for (const [key, valid] of Object.entries(GENERATOR_RULES)) {
    if (valid(raw[key])) options[key] = raw[key];
  }
  return options;
}

async function generatePassword(rawOptions) {
  const options = sanitizeGeneratorOptions(rawOptions ?? (await store.getGeneratorOptions()));
  const password = await call("generate_password", { options });
  if (typeof password !== "string" || !password) throw new BridgeError("internal");
  return password;
}

// ---------------------------------------------------------------------------
// Save / update prompts
// ---------------------------------------------------------------------------

function publicPending(pending) {
  return {
    id: pending.id,
    kind: pending.kind,
    host: displayHost(pending.host),
    username: pending.username,
    itemName: pending.itemName,
  };
}

/** A login was submitted in a frame: decide whether to offer saving or updating it. */
async function handleCapture(msg, sender) {
  const tabId = sender.tab?.id;
  const url = parseWebUrl(sender.url);
  if (tabId === undefined || !url) return { prompt: false };
  const password = typeof msg.password === "string" ? msg.password : "";
  let username = typeof msg.username === "string" ? msg.username.trim() : "";
  if (!password || password.length > 4096 || username.length > 1024) return { prompt: false };

  const host = url.hostname;
  const site = siteKey(host);
  if (await store.isNeverSite(host)) return { prompt: false };
  if (!username) username = await store.getUsernameStep(tabId, site);

  let matches;
  try {
    matches = await call("logins_for_url", { url: sender.url }, { auto: true });
  } catch {
    return { prompt: false }; // locked, not paired or app unavailable: nothing to compare with
  }
  const logins = (Array.isArray(matches) ? matches : []).filter((m) => m && typeof m.id === "string");
  const wanted = username.toLowerCase();
  const sameUser = logins.filter((m) => String(m.subtitle ?? "").trim().toLowerCase() === wanted);
  if (!username && !sameUser.length && logins.length > 1) return { prompt: false }; // ambiguous
  const candidates = sameUser.length ? sameUser : !username && logins.length === 1 ? logins : [];

  let item = null;
  for (const candidate of candidates) {
    // The app only answers "same or not": the stored password never comes
    // here, and the comparison does not count as user activity (a page that
    // submits forms must not keep the vault from auto-locking).
    let same;
    try {
      same = await call("check_login_password", { itemId: candidate.id, password }, { auto: true });
    } catch {
      return { prompt: false };
    }
    if (same === true) return { prompt: false }; // already stored
    item = item ?? candidate;
  }

  const pending = {
    id: crypto.randomUUID(),
    kind: item ? "update" : "save",
    host,
    site,
    origin: url.origin,
    username,
    password,
    itemId: item ? item.id : null,
    itemName: item ? String(item.name ?? "") : "",
    createdAt: Date.now(),
  };
  await store.setPending(tabId, pending);
  // If the page is navigating, the next page asks for it via cs:pending.
  notifyTab(tabId, { type: "bg:save-bar", bar: publicPending(pending) }, 0);
  return { prompt: true };
}

async function handlePendingQuery(sender) {
  const tabId = sender.tab?.id;
  const url = parseWebUrl(sender.url);
  if (sender.frameId !== 0 || tabId === undefined || !url) return null;
  const pending = await store.getPending(tabId);
  if (!pending) return null;
  // An update (it overwrites a stored password) is only offered on the origin
  // it was captured on: a sibling subdomain must not get its prompt shown on
  // the real site. A new login may follow the user across the site (log in on
  // accounts.example.com, land on www.example.com).
  if (pending.kind === "update" ? pending.origin !== url.origin : pending.site !== siteKey(url.hostname)) return null;
  return publicPending(pending);
}

async function handleSaveDecision(msg, sender) {
  const tabId = sender.tab?.id;
  if (tabId === undefined || typeof msg.id !== "string") throw new BridgeError("invalid_request");
  const pending = await store.getPending(tabId);
  if (!pending || pending.id !== msg.id) throw new BridgeError("not_found");
  switch (msg.action) {
    case "dismiss":
      await store.removePending(tabId, pending.id);
      return { kind: pending.kind };
    case "never":
      await store.addNeverSite(pending.host);
      await store.removePending(tabId, pending.id);
      return { kind: pending.kind };
    case "save":
      if (pending.kind === "update" && pending.itemId) {
        await call("update_password", { itemId: pending.itemId, password: pending.password });
      } else {
        await call("save_login", {
          name: displayHost(pending.host),
          url: pending.origin,
          username: pending.username,
          password: pending.password,
        });
      }
      await store.removePending(tabId, pending.id);
      refreshBadge(sender.tab).catch(() => undefined);
      notifyTab(tabId, { type: "bg:refresh" });
      return { kind: pending.kind };
    default:
      throw new BridgeError("invalid_request");
  }
}

// ---------------------------------------------------------------------------
// Message handlers
// ---------------------------------------------------------------------------

function requireString(value, max = 4096) {
  if (typeof value !== "string" || value.length > max) throw new BridgeError("invalid_request");
  return value;
}

function requireTabId(value) {
  if (!Number.isInteger(value) || value < 0) throw new BridgeError("invalid_request");
  return value;
}

/** Only logins: the popup's actions (fill, copy username/password) apply to logins. */
function loginSummaries(list) {
  return (Array.isArray(list) ? list : []).filter((s) => s && s.type === "login" && typeof s.id === "string");
}

async function tabInfo() {
  const tab = await activeTab();
  if (!tab || tab.id === undefined) return { tabId: null, web: false };
  const url = parseWebUrl(tab.url);
  if (!url) return { tabId: tab.id, web: false };
  return {
    tabId: tab.id,
    web: true,
    url: tab.url,
    origin: url.origin,
    host: displayHost(url.hostname),
    hostname: url.hostname,
    insecure: url.protocol === "http:",
    neverSave: await store.isNeverSite(url.hostname),
  };
}

/** Messages from extension pages (the popup). */
const popupHandlers = {
  "popup:status": async (msg) => {
    let status = null;
    if (!msg.fresh) {
      const cached = await store.getStatus();
      if (cached && Date.now() - cached.ts < 3000) status = cached;
    }
    status ??= await refreshStatus();
    return { ...publicStatus(status), extensionUpdate: await extensionUpdateInfo(status) };
  },
  "popup:pair-start": (msg) => startPairing(msg.code, msg.clientName),
  "popup:pair-state": () => getPairingState(),
  "popup:pair-reset": async () => {
    const pairing = await getPairingState();
    if (pairing?.state !== "pending") await store.setPairing(null);
    return null;
  },
  "popup:pair-cancel": () => cancelPairing(),
  "popup:list-vaults": async () => vaultList(await call("list_vaults")),
  "popup:unlock": async (msg) => {
    const payload = { password: requireString(msg.password) };
    // Without a vault id the app opens its last used vault (older popups, older apps).
    if (msg.vaultId !== undefined && msg.vaultId !== null) payload.vaultId = requireString(msg.vaultId, 200);
    const data = await call("unlock", payload);
    // Badge and inline icons now show the logins of the (possibly other) vault.
    refreshActiveTab().catch(() => undefined);
    return {
      vaultName: typeof data?.vaultName === "string" ? data.vaultName : "",
      vaultId: typeof data?.vaultId === "string" ? data.vaultId : null,
    };
  },
  "popup:lock": async () => {
    await call("lock");
    await clearAllBadges();
    refreshActiveTab().catch(() => undefined);
    return null;
  },
  "popup:focus-app": async () => {
    await call("focus_app");
    return null;
  },
  "popup:tab-info": () => tabInfo(),
  "popup:matches": async (msg) => {
    const tab = await chrome.tabs.get(requireTabId(msg.tabId));
    if (!parseWebUrl(tab.url)) return [];
    const matches = loginSummaries(await call("logins_for_url", { url: tab.url }));
    if (tab.active) await setBadge(tab.id, matches.length);
    return matches;
  },
  "popup:search": async (msg) => {
    const query = requireString(msg.query, 200).trim();
    if (!query) return [];
    return loginSummaries(await call("search", { query }));
  },
  // Secrets are copied by the app (clipboard history exclusion, clearing after
  // clipboardClearSeconds and on lock); the popup never receives them.
  "popup:copy-field": async (msg) => {
    const field = msg.field === "password" || msg.field === "totp" ? msg.field : null;
    if (!field) throw new BridgeError("invalid_request");
    const data = await call("copy_field", { itemId: requireString(msg.itemId, 200), field });
    return { remaining: Number.isInteger(data?.remaining) ? data.remaining : null };
  },
  "popup:copy-secret": async (msg) => {
    const text = requireString(msg.text);
    if (!text) throw new BridgeError("invalid_request");
    await call("copy_secret", { text });
    return null;
  },
  "popup:fill": (msg) => fillTab(requireTabId(msg.tabId), requireString(msg.itemId, 200)),
  "popup:generate": (msg) => generatePassword(msg.options),
  "popup:fill-generated": async (msg) => {
    const tabId = requireTabId(msg.tabId);
    return fillGenerated(tabId, await focusedFrameId(tabId), requireString(msg.password), "focused");
  },
  "popup:save-login": async (msg) => {
    const name = requireString(msg.name, 200).trim();
    const url = requireString(msg.url, 2048).trim();
    const username = requireString(msg.username, 1024).trim();
    const password = requireString(msg.password);
    if (!name || !password) throw new BridgeError("invalid_request");
    const result = await call("save_login", { name, url, username, password });
    refreshActiveTab().catch(() => undefined);
    return result;
  },
  "popup:never-remove": async (msg) => {
    await store.removeNeverSite(requireString(msg.hostname, 300));
    return null;
  },
};

/** Messages from content scripts (any http/https frame). */
const contentHandlers = {
  "cs:page-info": async (_msg, sender) => {
    const url = parseWebUrl(sender.url);
    if (!url) return { state: "unsupported", matches: [] };
    try {
      const matches = loginSummaries(await call("logins_for_url", { url: sender.url }, { auto: true }));
      if (sender.frameId === 0 && sender.tab?.active) setBadge(sender.tab.id, matches.length);
      return {
        state: "unlocked",
        insecure: url.protocol === "http:",
        matches: matches.map((m) => ({ id: m.id, name: String(m.name ?? ""), username: String(m.subtitle ?? "") })),
      };
    } catch (err) {
      return { state: stateForError(errorCode(err)), matches: [] };
    }
  },
  "cs:fill-request": (msg, sender) => handleFillRequest(msg, sender),
  "cs:focus": async (_msg, sender) => {
    await store.setFocusedFrame(sender.tab.id, sender.frameId);
    return null;
  },
  "cs:capture": (msg, sender) => handleCapture(msg, sender),
  "cs:username-step": async (msg, sender) => {
    const username = requireString(msg.username, 1024).trim();
    const url = parseWebUrl(sender.url);
    if (username && url) await store.setUsernameStep(sender.tab.id, siteKey(url.hostname), username);
    return null;
  },
  "cs:pending": (_msg, sender) => handlePendingQuery(sender),
  "cs:save-decision": (msg, sender) => handleSaveDecision(msg, sender),
  "cs:open-popup": async () => ({ opened: await openPopupOrApp() }),
};

function isExtensionPage(sender) {
  return sender.id === chrome.runtime.id && typeof sender.url === "string" && sender.url.startsWith(`${EXTENSION_ORIGIN}/`);
}

function isContentScript(sender) {
  return (
    sender.id === chrome.runtime.id &&
    sender.tab?.id !== undefined &&
    Number.isInteger(sender.frameId) &&
    parseWebUrl(sender.url) !== null
  );
}

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (!msg || typeof msg !== "object" || typeof msg.type !== "string") return false;
  let handler;
  if (isExtensionPage(sender)) handler = popupHandlers[msg.type];
  else if (isContentScript(sender)) handler = contentHandlers[msg.type];
  if (!handler) {
    sendResponse({ ok: false, error: "invalid_request" });
    return false;
  }
  Promise.resolve()
    .then(() => handler(msg, sender))
    .then(
      (data) => sendResponse({ ok: true, data: data ?? null }),
      (err) => sendResponse({ ok: false, error: errorCode(err) }),
    );
  return true; // async response
});

// ---------------------------------------------------------------------------
// Keyboard commands and context menu
// ---------------------------------------------------------------------------

function fillErrorToast(code) {
  switch (code) {
    case "insecure":
      return "csInsecureBlocked";
    case "no_fields":
      return "csNoFields";
    case "locked":
      return "csLockedHint";
    case "not_paired":
    case "host_missing":
    case "host_forbidden":
    case "app_unavailable":
    case "timeout":
      return "csNotConnected";
    default:
      return "csFillFailed";
  }
}

async function autofillCommand(tab) {
  if (!tab || tab.id === undefined || !parseWebUrl(tab.url)) return;
  if (!(await store.getCredentials())) {
    await openPopupOrApp();
    return;
  }
  let matches;
  try {
    matches = loginSummaries(await call("logins_for_url", { url: tab.url }));
  } catch (err) {
    if (errorCode(err) === "locked" || errorCode(err) === "not_paired") await openPopupOrApp();
    else toast(tab.id, fillErrorToast(errorCode(err)));
    return;
  }
  if (!matches.length) {
    toast(tab.id, "csNoMatches");
    return;
  }
  const lastId = await store.getLastUsed(siteKey(parseWebUrl(tab.url).hostname));
  const chosen = matches.find((m) => m.id === lastId) || matches[0];
  try {
    await fillTab(tab.id, chosen.id);
  } catch (err) {
    toast(tab.id, fillErrorToast(errorCode(err)));
  }
}

async function generateInto(tab, frameId, target) {
  if (!tab || tab.id === undefined || !parseWebUrl(tab.url)) return;
  let password;
  try {
    password = await generatePassword(null);
  } catch (err) {
    const code = errorCode(err);
    toast(tab.id, code === "invalid_request" || code === "internal" ? "csGenerateFailed" : "csNotConnected");
    return;
  }
  try {
    await fillGenerated(tab.id, frameId, password, target);
  } catch {
    toast(tab.id, "csNoField");
  }
}

chrome.commands.onCommand.addListener((command, tab) => {
  const run = async () => {
    const target = tab?.id !== undefined ? tab : await activeTab();
    if (command === "autofill") await autofillCommand(target);
    else if (command === "generate" && target?.id !== undefined) {
      await generateInto(target, await focusedFrameId(target.id), "focused");
    }
  };
  run().catch(() => undefined);
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "keystead-generate") {
    generateInto(tab, Number.isInteger(info.frameId) ? info.frameId : 0, "context").catch(() => undefined);
  } else if (info.menuItemId === "keystead-open") {
    openPopupOrApp().catch(() => undefined);
  }
});

async function setupContextMenus() {
  await chrome.contextMenus.removeAll();
  chrome.contextMenus.create({ id: "keystead-generate", title: t("ctxGenerate"), contexts: ["editable"] });
  chrome.contextMenus.create({ id: "keystead-open", title: t("ctxOpen"), contexts: ["editable"] });
}

/** Declared content scripts only reach pages loaded after install; inject into the open ones. */
async function injectIntoOpenTabs() {
  const tabs = await chrome.tabs.query({ url: ["http://*/*", "https://*/*"] });
  await Promise.all(
    tabs
      .filter((tab) => tab.id !== undefined && !tab.discarded)
      .map((tab) =>
        chrome.scripting
          .executeScript({ target: { tabId: tab.id, allFrames: true }, files: ["lib/forms.js", "content.js"] })
          .catch(() => undefined),
      ),
  );
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

chrome.runtime.onInstalled.addListener((details) => {
  setupContextMenus().catch(() => undefined);
  if (details.reason === "install" || details.reason === "update") injectIntoOpenTabs().catch(() => undefined);
});

chrome.tabs.onActivated.addListener(({ tabId }) => {
  chrome.tabs
    .get(tabId)
    .then(refreshBadge)
    .catch(() => undefined);
});

chrome.tabs.onUpdated.addListener((_tabId, changeInfo, tab) => {
  if (tab.active && (changeInfo.status === "complete" || changeInfo.url)) refreshBadge(tab).catch(() => undefined);
});

chrome.tabs.onRemoved.addListener((tabId) => {
  store.forgetTab(tabId).catch(() => undefined);
});

chrome.action.setBadgeBackgroundColor({ color: BADGE_COLOR }).catch(() => undefined);
if (typeof chrome.action.setBadgeTextColor === "function") {
  chrome.action.setBadgeTextColor({ color: "#ffffff" }).catch(() => undefined);
}
store.restrictSessionAccess();
store.pruneSession();
