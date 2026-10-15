// Native messaging client for the Keystead desktop app (host "com.keystead.bridge").
//
// Protocol (docs/ARCHITECTURE.md, "Browser bridge protocol"):
//   request  { id, type, clientId?, token?, ...payload }
//   response { id, ok: true, data } | { id, ok: false, error }
// The host answers the requests of one port in order. A `pair` request blocks
// for up to 120 s, so it runs on a dedicated port (see `dedicated()`).

export const HOST_NAME = "com.keystead.bridge";

/** Default per-request timeout. */
export const REQUEST_TIMEOUT_MS = 10_000;
/** Requests sent before a fresh port answered anything: the host may first have to launch the app (≤ 8 s). */
export const FIRST_REQUEST_TIMEOUT_MS = 12_000;
/** `pair` waits for the user's decision in the app (≤ 120 s). */
export const PAIR_TIMEOUT_MS = 125_000;
/** An idle port is closed so the host process ends and the service worker may sleep. */
const IDLE_DISCONNECT_MS = 90_000;

/** Error with a stable protocol/extension error code (e.g. "locked", "host_missing"). */
export class BridgeError extends Error {
  constructor(code) {
    super(code);
    this.name = "BridgeError";
    this.code = code;
  }
}

/** Maps `chrome.runtime.lastError` of a disconnected native port to an error code. */
export function disconnectCode(message) {
  const text = String(message || "");
  // "Specified native messaging host not found." – not registered for this browser.
  if (/native messaging host not found/i.test(text)) return "host_missing";
  // "Access to the specified native messaging host is forbidden." – registered for another extension ID.
  if (/forbidden/i.test(text)) return "host_forbidden";
  // "Failed to start native messaging host." – the registered executable is gone (moved portable app).
  if (/failed to start/i.test(text)) return "host_missing";
  // "Native host has exited.", "Error when communicating with the native messaging host.", …
  return "app_unavailable";
}

/** One native messaging port with request/response correlation and timeouts. */
class NativeConnection {
  constructor(hostName, { idleDisconnect = true } = {}) {
    this.hostName = hostName;
    this.idleDisconnect = idleDisconnect;
    this.port = null;
    this.answered = false;
    this.pending = new Map();
    this.idleTimer = null;
  }

  /** Sends one request object and resolves with its `data` (or rejects with a BridgeError). */
  request(message, timeoutMs) {
    return new Promise((resolve, reject) => {
      let port;
      try {
        port = this.ensurePort();
      } catch {
        reject(new BridgeError("host_missing"));
        return;
      }
      const timeout = timeoutMs ?? (this.answered ? REQUEST_TIMEOUT_MS : FIRST_REQUEST_TIMEOUT_MS);
      const timer = setTimeout(() => this.onTimeout(message.id), timeout);
      this.pending.set(message.id, { resolve, reject, timer });
      this.clearIdleTimer();
      try {
        port.postMessage(message);
      } catch {
        // The port died synchronously (e.g. host missing); onDisconnect reports the reason.
        this.settle(message.id, null, new BridgeError("app_unavailable"));
      }
    });
  }

  close(code = "app_unavailable") {
    this.clearIdleTimer();
    const port = this.port;
    this.port = null;
    if (port) {
      try {
        port.disconnect();
      } catch {
        // Already disconnected.
      }
    }
    this.rejectAll(code);
  }

  ensurePort() {
    if (this.port) return this.port;
    const port = chrome.runtime.connectNative(this.hostName);
    this.port = port;
    this.answered = false;
    port.onMessage.addListener((msg) => this.onMessage(port, msg));
    port.onDisconnect.addListener(() => this.onDisconnect(port));
    return port;
  }

  onMessage(port, msg) {
    if (port !== this.port) return;
    this.answered = true;
    if (!msg || typeof msg !== "object" || typeof msg.id !== "string") return;
    if (msg.ok === true) {
      this.settle(msg.id, msg.data ?? null, null);
    } else {
      const code = typeof msg.error === "string" && msg.error ? msg.error : "internal";
      this.settle(msg.id, null, new BridgeError(code));
    }
  }

  onDisconnect(port) {
    // Reading lastError marks it as checked (avoids "Unchecked runtime.lastError").
    const code = disconnectCode(chrome.runtime.lastError?.message);
    if (port !== this.port) return;
    this.port = null;
    this.clearIdleTimer();
    this.rejectAll(code);
  }

  onTimeout(id) {
    if (!this.pending.has(id)) return;
    // Replies arrive in order, so everything queued behind a hung request is
    // stuck as well: drop the port (the next request starts a fresh host).
    this.settle(id, null, new BridgeError("timeout"));
    this.close("timeout");
  }

  settle(id, data, error) {
    const entry = this.pending.get(id);
    if (!entry) return;
    this.pending.delete(id);
    clearTimeout(entry.timer);
    if (error) entry.reject(error);
    else entry.resolve(data);
    if (this.pending.size === 0) this.armIdleTimer();
  }

  rejectAll(code) {
    const entries = [...this.pending.values()];
    this.pending.clear();
    for (const entry of entries) {
      clearTimeout(entry.timer);
      entry.reject(new BridgeError(code));
    }
  }

  armIdleTimer() {
    if (!this.idleDisconnect || !this.port) return;
    this.clearIdleTimer();
    this.idleTimer = setTimeout(() => {
      if (this.pending.size === 0) this.close();
    }, IDLE_DISCONNECT_MS);
  }

  clearIdleTimer() {
    if (this.idleTimer) clearTimeout(this.idleTimer);
    this.idleTimer = null;
  }
}

/** Builds a protocol request. Credentials are only attached when given. */
export function buildRequest(type, payload = {}, credentials = null) {
  const message = { ...payload, id: crypto.randomUUID(), type };
  if (credentials) {
    message.clientId = credentials.clientId;
    message.token = credentials.token;
  }
  return message;
}

/** Shared bridge client used by the service worker. */
export class NativeBridge {
  constructor(hostName = HOST_NAME) {
    this.hostName = hostName;
    this.main = new NativeConnection(hostName);
  }

  /**
   * Sends `type` with `payload` (and pairing credentials if given) and
   * resolves with the response `data`. Rejects with a BridgeError whose code
   * is a protocol error code or one of "host_missing", "host_forbidden",
   * "app_unavailable", "timeout".
   */
  request(type, payload = {}, credentials = null, timeoutMs = undefined) {
    return this.main.request(buildRequest(type, payload, credentials), timeoutMs);
  }

  /** A separate port (own host process) for long-running requests such as `pair`. */
  dedicated() {
    const connection = new NativeConnection(this.hostName, { idleDisconnect: false });
    return {
      request: (type, payload = {}, credentials = null, timeoutMs = undefined) =>
        connection.request(buildRequest(type, payload, credentials), timeoutMs),
      close: () => connection.close(),
    };
  }
}
