// Popup → service worker messaging (see background.js "popupHandlers").

import { normalizeSettings } from "./settings.js";

export class ApiError extends Error {
  constructor(code) {
    super(code);
    this.name = "ApiError";
    this.code = code;
  }
}

async function call(type, payload = {}) {
  let response;
  try {
    response = await chrome.runtime.sendMessage({ ...payload, type });
  } catch {
    throw new ApiError("internal");
  }
  if (!response) throw new ApiError("internal");
  if (response.ok !== true) throw new ApiError(typeof response.error === "string" ? response.error : "internal");
  return response.data;
}

/** The real API used by popup.js. lib/demo.js implements the same interface with fake data. */
export function createApi() {
  return {
    demo: false,
    extensionId: chrome.runtime.id,
    version: chrome.runtime.getManifest().version,

    status: (fresh = false) => call("popup:status", { fresh }),
    pairStart: (code, clientName) => call("popup:pair-start", { code, clientName }),
    pairState: () => call("popup:pair-state"),
    pairReset: () => call("popup:pair-reset"),
    pairCancel: () => call("popup:pair-cancel"),
    /** Calls `listener(pairing | null)` whenever the pairing progress changes. Returns an unsubscribe function. */
    onPairingChange(listener) {
      const handler = (changes, area) => {
        if (area === "session" && changes.pairing) listener(changes.pairing.newValue ?? null);
      };
      chrome.storage.onChanged.addListener(handler);
      return () => chrome.storage.onChanged.removeListener(handler);
    },

    /** { vaults: [{ id, name }] (sorted by name), currentVaultId, lastVaultId } */
    listVaults: () => call("popup:list-vaults"),
    /** Unlocks `vaultId` (null: the app picks its last used vault); resolves to { vaultName, vaultId }. */
    unlock: (password, vaultId = null) => call("popup:unlock", { password, vaultId }),
    lock: () => call("popup:lock"),
    focusApp: () => call("popup:focus-app"),

    tabInfo: () => call("popup:tab-info"),
    matches: (tabId) => call("popup:matches", { tabId }),
    search: (query) => call("popup:search", { query }),
    fill: (tabId, itemId) => call("popup:fill", { tabId, itemId }),
    /**
     * A preview from the app – not stored in the generator history. `options`:
     * the popup's own generator options; `null`: those stored here, else the
     * app's default generator preset.
     */
    generate: (options) => call("popup:generate", { options: options ?? null }),
    /** Fills a preview into the page's focused field; then it is in the generator history. Resolves to { filled, remembered }. */
    fillGenerated: (tabId, password) => call("popup:fill-generated", { tabId, password }),
    /** Countdown data of 2FA codes: [{ id, period, remaining }] (the codes stay in the service worker). */
    totpTimers: (itemIds) => call("popup:totp-timers", { itemIds }),
    /** `{ name, url, username, password, generated }` – `generated`: the password is a preview taken into this login (it goes into the generator history). */
    saveLogin: (login) => call("popup:save-login", login),
    neverRemove: (hostname) => call("popup:never-remove", { hostname }),

    /** The popup's own generator options (lib/generator.js), or null: the app's default preset applies. */
    async loadGeneratorOptions() {
      try {
        const { generatorOptions } = await chrome.storage.local.get("generatorOptions");
        return generatorOptions && typeof generatorOptions === "object" ? generatorOptions : null;
      } catch {
        return null;
      }
    },
    async saveGeneratorOptions(options) {
      try {
        await chrome.storage.local.set({ generatorOptions: options });
      } catch {
        // Options are a convenience; generation still works with the current values.
      }
    },
    /** Drops the own options: suggestions, the context menu and the popup use the app's default preset again. */
    async clearGeneratorOptions() {
      try {
        await chrome.storage.local.remove("generatorOptions");
      } catch {
        // Best effort: the stored options then stay in use for suggestions until removed.
      }
    },
    /** Extension settings (lib/settings.js); the service worker and the pages pick up changes. */
    async loadSettings() {
      try {
        const { settings } = await chrome.storage.local.get("settings");
        return normalizeSettings(settings);
      } catch {
        return normalizeSettings(null);
      }
    },
    async saveSettings(settings) {
      await chrome.storage.local.set({ settings: normalizeSettings(settings) });
    },
    /** Id of the vault last unlocked from this browser (preselected when locked), or null. */
    async loadChosenVault() {
      try {
        const { chosenVaultId } = await chrome.storage.local.get("chosenVaultId");
        return typeof chosenVaultId === "string" && chosenVaultId ? chosenVaultId : null;
      } catch {
        return null;
      }
    },
    /** Only the id is stored (no vault name). */
    async saveChosenVault(vaultId) {
      try {
        await chrome.storage.local.set({ chosenVaultId: vaultId });
      } catch {
        // A convenience only: the selector then falls back to the app's last vault.
      }
    },
    /**
     * Secrets are copied by the desktop app, not by the popup: the app excludes
     * them from clipboard history and clears them after the user's
     * "clipboardClearSeconds" (and on lock). A password never reaches the popup.
     * Resolves to { remaining } (seconds a copied TOTP code stays valid, else null).
     */
    copyField: (itemId, field) => call("popup:copy-field", { itemId, field }),
    /** Copies the generator's preview through the app; then it is in the generator history. Resolves to { remembered }. */
    copyGenerated: (password) => call("popup:copy-generated", { password }),
    /** Non-secret text only (e.g. a username): stays on the clipboard. */
    copy: (text) => navigator.clipboard.writeText(text),
    close: () => window.close(),
  };
}
