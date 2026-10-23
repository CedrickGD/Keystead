// A small in-memory stand-in for the chrome.* APIs the service worker uses,
// with a scripted desktop app behind chrome.runtime.connectNative. Enough to
// load background.js in node and drive its message handlers, context menu and
// commands (extension/tests/background-generator.test.mjs).

const EXTENSION_ID = "imfndemblnaalppnmdplagajjielnaok";

function event() {
  const listeners = [];
  return {
    listeners,
    addListener: (fn) => listeners.push(fn),
    removeListener: (fn) => {
      const i = listeners.indexOf(fn);
      if (i >= 0) listeners.splice(i, 1);
    },
  };
}

function storageArea(onChanged, name) {
  let data = {};
  const pick = (keys) => {
    if (keys === null || keys === undefined) return { ...data };
    const list = typeof keys === "string" ? [keys] : Array.isArray(keys) ? keys : Object.keys(keys);
    return Object.fromEntries(list.filter((k) => k in data).map((k) => [k, structuredClone(data[k])]));
  };
  const notify = (changes) => {
    if (Object.keys(changes).length) for (const fn of onChanged.listeners) fn(changes, name);
  };
  return {
    get: async (keys) => pick(keys),
    set: async (items) => {
      const changes = {};
      for (const [k, v] of Object.entries(items)) {
        changes[k] = { oldValue: data[k], newValue: structuredClone(v) };
        data[k] = structuredClone(v);
      }
      notify(changes);
    },
    remove: async (keys) => {
      const changes = {};
      for (const k of typeof keys === "string" ? [keys] : keys) {
        if (k in data) changes[k] = { oldValue: data[k] };
        delete data[k];
      }
      notify(changes);
    },
    clear: async () => {
      data = {};
    },
    setAccessLevel: async () => undefined,
    /** Test helper: the raw stored values. */
    dump: () => structuredClone(data),
  };
}

/**
 * Installs `globalThis.chrome`. `app(request)` answers every native request
 * ({ type, ...payload } without id/credentials): return the `data`, or throw
 * `{ error: "<code>" }`. `tabMessage(tabId, message, options)` answers
 * chrome.tabs.sendMessage. Returns the fake for inspection.
 */
export function installFakeChrome() {
  const fake = {
    /** Requests the app received, oldest first (without id and credentials). */
    requests: [],
    /** Messages sent to tabs: { tabId, message, options }. */
    tabMessages: [],
    app: () => null,
    tabMessage: () => undefined,
    tabs: [],
    ports: [],
  };

  const storageChanged = event();
  const runtime = {
    id: EXTENSION_ID,
    lastError: undefined,
    getURL: (path) => `chrome-extension://${EXTENSION_ID}/${path}`,
    getManifest: () => ({ version: "2.0.1" }),
    reload: () => undefined,
    onMessage: event(),
    onInstalled: event(),
    connectNative: () => {
      const onMessage = event();
      const onDisconnect = event();
      const port = {
        onMessage,
        onDisconnect,
        connected: true,
        postMessage(message) {
          const { id, clientId: _clientId, token: _token, ...request } = message;
          fake.requests.push(request);
          Promise.resolve()
            .then(() => fake.app(request))
            .then(
              (data) => ({ id, ok: true, data: data ?? null }),
              (err) => ({ id, ok: false, error: err?.error ?? "internal" }),
            )
            .then((reply) => {
              if (port.connected) for (const fn of onMessage.listeners) fn(reply);
            });
        },
        disconnect() {
          port.connected = false;
        },
      };
      fake.ports.push(port);
      return port;
    },
  };

  globalThis.chrome = {
    runtime,
    storage: {
      local: storageArea(storageChanged, "local"),
      session: storageArea(storageChanged, "session"),
      onChanged: storageChanged,
    },
    i18n: { getMessage: (key) => key },
    tabs: {
      query: async () => fake.tabs,
      get: async (tabId) => fake.tabs.find((t) => t.id === tabId) ?? Promise.reject(new Error("no tab")),
      sendMessage: async (tabId, message, options) => {
        fake.tabMessages.push({ tabId, message, options });
        return fake.tabMessage(tabId, message, options);
      },
      onActivated: event(),
      onUpdated: event(),
      onRemoved: event(),
    },
    action: {
      setBadgeText: async () => undefined,
      setBadgeBackgroundColor: async () => undefined,
      setBadgeTextColor: async () => undefined,
      openPopup: async () => undefined,
    },
    contextMenus: { create: () => undefined, removeAll: async () => undefined, onClicked: event() },
    commands: { onCommand: event() },
    scripting: { executeScript: async () => [] },
  };

  /** Sends a runtime message as `sender` and resolves with the service worker's response. */
  fake.send = (message, sender) =>
    new Promise((resolve) => {
      let async = false;
      for (const fn of runtime.onMessage.listeners) async = fn(message, sender, resolve) || async;
      if (!async) setTimeout(() => resolve(undefined), 0);
    });
  /** A message from the popup (an extension page). */
  fake.fromPopup = (message) => fake.send(message, { id: EXTENSION_ID, url: `chrome-extension://${EXTENSION_ID}/popup.html` });
  /** A message from a content script in `url` (tab `tabId`, top frame). */
  fake.fromPage = (message, url, tabId = 7) => fake.send(message, { id: EXTENSION_ID, url, frameId: 0, tab: { id: tabId } });
  /** Waits until queued promise callbacks and timers of 0 ms ran. */
  fake.settle = () => new Promise((resolve) => setTimeout(resolve, 5));
  /** Ends every native port (as if the host exited), so no idle timer keeps node alive. */
  fake.shutdown = () => {
    for (const port of fake.ports) {
      if (!port.connected) continue;
      port.connected = false;
      for (const fn of port.onDisconnect.listeners) fn();
    }
  };
  return fake;
}
