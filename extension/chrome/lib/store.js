// Persistent and session state of the service worker.
//
// MV3 service workers are stopped when idle, so nothing essential may live
// only in memory:
// * chrome.storage.local   – pairing credentials, "never save" sites, last
//                            used login per site, generator options, the id
//                            of the vault last unlocked from this browser
//                            (written by the popup, lib/popup-api.js).
// * chrome.storage.session – cached status, pairing progress, pending save
//                            prompts (contain a password; in memory only and
//                            not readable by content scripts), per-tab hints.

const local = chrome.storage.local;
const session = chrome.storage.session;

/** How long a captured login waits for the user's save decision. */
export const PENDING_TTL_MS = 2 * 60_000;
/** How long the username of a username-only login step is remembered. */
const USERNAME_STEP_TTL_MS = 5 * 60_000;
const MAX_LAST_USED = 300;
const MAX_NEVER_SITES = 1000;

// Serialises read-modify-write sequences per key (message handlers run concurrently).
const locks = new Map();
async function withLock(key, fn) {
  const previous = locks.get(key) || Promise.resolve();
  let release;
  const current = new Promise((resolve) => {
    release = resolve;
  });
  const chained = previous.then(() => current);
  locks.set(key, chained);
  await previous;
  try {
    return await fn();
  } finally {
    release();
    if (locks.get(key) === chained) locks.delete(key);
  }
}

async function getKey(area, key, fallback) {
  try {
    const result = await area.get(key);
    return result[key] ?? fallback;
  } catch {
    return fallback;
  }
}

/** Restricts storage.session to extension pages and the service worker. */
export async function restrictSessionAccess() {
  try {
    await session.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
  } catch {
    // Older browsers: TRUSTED_CONTEXTS is the default anyway.
  }
}

// ---------------------------------------------------------------------------
// Pairing credentials
// ---------------------------------------------------------------------------

export async function getCredentials() {
  const value = await getKey(local, "credentials", null);
  if (value && typeof value.clientId === "string" && typeof value.token === "string" && value.clientId && value.token) {
    return { clientId: value.clientId, token: value.token };
  }
  return null;
}

export async function setCredentials(clientId, token) {
  await local.set({ credentials: { clientId, token } });
}

export async function clearCredentials() {
  await local.remove("credentials");
}

// ---------------------------------------------------------------------------
// Status cache & pairing progress (session)
// ---------------------------------------------------------------------------

export async function getStatus() {
  return getKey(session, "status", null);
}

export async function setStatus(status) {
  await session.set({ status });
}

export async function getPairing() {
  return getKey(session, "pairing", null);
}

export async function setPairing(pairing) {
  if (pairing) await session.set({ pairing });
  else await session.remove("pairing");
}

// ---------------------------------------------------------------------------
// "Never save on this site"
// ---------------------------------------------------------------------------

export async function getNeverSites() {
  const list = await getKey(local, "neverSites", []);
  return Array.isArray(list) ? list.filter((h) => typeof h === "string") : [];
}

export async function isNeverSite(host) {
  return (await getNeverSites()).includes(host);
}

export function addNeverSite(host) {
  return withLock("neverSites", async () => {
    const list = await getNeverSites();
    if (!list.includes(host)) {
      list.push(host);
      await local.set({ neverSites: list.slice(-MAX_NEVER_SITES) });
    }
  });
}

export function removeNeverSite(host) {
  return withLock("neverSites", async () => {
    const list = await getNeverSites();
    await local.set({ neverSites: list.filter((h) => h !== host) });
  });
}

// ---------------------------------------------------------------------------
// Most recently used login per site (item ids only, no secrets)
// ---------------------------------------------------------------------------

export async function getLastUsed(site) {
  const map = await getKey(local, "lastUsed", {});
  const entry = map && typeof map === "object" ? map[site] : null;
  return entry && typeof entry.id === "string" ? entry.id : null;
}

export function setLastUsed(site, itemId) {
  return withLock("lastUsed", async () => {
    const map = await getKey(local, "lastUsed", {});
    const next = map && typeof map === "object" ? { ...map } : {};
    next[site] = { id: itemId, at: Date.now() };
    const entries = Object.entries(next);
    if (entries.length > MAX_LAST_USED) {
      entries.sort((a, b) => (b[1]?.at || 0) - (a[1]?.at || 0));
      await local.set({ lastUsed: Object.fromEntries(entries.slice(0, MAX_LAST_USED)) });
    } else {
      await local.set({ lastUsed: next });
    }
  });
}

// ---------------------------------------------------------------------------
// Generator options (non-secret)
// ---------------------------------------------------------------------------

export async function getGeneratorOptions() {
  const value = await getKey(local, "generatorOptions", null);
  return value && typeof value === "object" ? value : null;
}

// ---------------------------------------------------------------------------
// Per-tab session data: pending save prompts, username steps, focused frame
// ---------------------------------------------------------------------------

function tabKey(prefix, tabId) {
  return `${prefix}:${tabId}`;
}

export async function getPending(tabId) {
  const pending = await getKey(session, tabKey("pending", tabId), null);
  if (!pending) return null;
  if (Date.now() - pending.createdAt > PENDING_TTL_MS) {
    await session.remove(tabKey("pending", tabId));
    return null;
  }
  return pending;
}

export async function setPending(tabId, pending) {
  await session.set({ [tabKey("pending", tabId)]: pending });
}

export async function removePending(tabId, id = null) {
  await withLock(tabKey("pending", tabId), async () => {
    const pending = await getKey(session, tabKey("pending", tabId), null);
    if (pending && (id === null || pending.id === id)) await session.remove(tabKey("pending", tabId));
  });
}

export async function getUsernameStep(tabId, site) {
  const step = await getKey(session, tabKey("username", tabId), null);
  if (!step || step.site !== site || Date.now() - step.at > USERNAME_STEP_TTL_MS) return "";
  return typeof step.username === "string" ? step.username : "";
}

export async function setUsernameStep(tabId, site, username) {
  await session.set({ [tabKey("username", tabId)]: { site, username, at: Date.now() } });
}

export async function getFocusedFrame(tabId) {
  const entry = await getKey(session, tabKey("focus", tabId), null);
  return entry && Number.isInteger(entry.frameId) ? entry : null;
}

export async function setFocusedFrame(tabId, frameId) {
  await session.set({ [tabKey("focus", tabId)]: { frameId, at: Date.now() } });
}

/** Drops everything remembered for a closed tab. */
export async function forgetTab(tabId) {
  await session.remove([tabKey("pending", tabId), tabKey("username", tabId), tabKey("focus", tabId)]);
}

/** Removes expired per-tab entries (called on service worker start). */
export async function pruneSession() {
  try {
    const all = await session.get(null);
    const now = Date.now();
    const stale = Object.entries(all)
      .filter(([key, value]) => {
        if (key.startsWith("pending:")) return !value || now - value.createdAt > PENDING_TTL_MS;
        if (key.startsWith("username:")) return !value || now - value.at > USERNAME_STEP_TTL_MS;
        return false;
      })
      .map(([key]) => key);
    if (stale.length) await session.remove(stale);
  } catch {
    // Best effort.
  }
}
