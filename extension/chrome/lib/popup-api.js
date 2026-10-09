// Popup → service worker messaging (see background.js "popupHandlers").

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

    unlock: (password) => call("popup:unlock", { password }),
    lock: () => call("popup:lock"),
    focusApp: () => call("popup:focus-app"),

    tabInfo: () => call("popup:tab-info"),
    matches: (tabId) => call("popup:matches", { tabId }),
    search: (query) => call("popup:search", { query }),
    getLogin: (itemId) => call("popup:get-login", { itemId }),
    getTotp: (itemId) => call("popup:get-totp", { itemId }),
    fill: (tabId, itemId) => call("popup:fill", { tabId, itemId }),
    generate: (options) => call("popup:generate", { options }),
    fillGenerated: (tabId, password) => call("popup:fill-generated", { tabId, password }),
    saveLogin: (login) => call("popup:save-login", login),
    neverRemove: (hostname) => call("popup:never-remove", { hostname }),

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
    copy: (text) => navigator.clipboard.writeText(text),
    close: () => window.close(),
  };
}
