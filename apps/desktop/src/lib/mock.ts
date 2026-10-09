// In-memory mock of the Tauri backend (docs/ARCHITECTURE.md, "Desktop backend
// ↔ frontend"). Used automatically when the UI runs outside of Tauri, e.g.
// `npm run dev` in a normal browser and the automated screenshots.
//
// Two demo vaults ("Privat", "Arbeit"), master password "demo" for both.
// URL parameters: `?mock=empty` starts without any vault (first-run screen).
// `window.__keysteadMock` exposes helpers to simulate backend events.

import type {
  AppInfo,
  BrowserId,
  BrowserInfo,
  BrowserStatus,
  ExportFormat,
  Folder,
  GeneratedPassword,
  GeneratorOptions,
  HealthReport,
  ImportFormat,
  ImportReport,
  LegacyVaultInfo,
  PairedClient,
  PairingRequest,
  SessionState,
  Settings,
  VaultData,
  VaultInfo,
  VaultItem,
} from "./types";
import { DEFAULT_GENERATOR_OPTIONS } from "./types";
import { totpNow } from "./demo/totp";
import { estimateStrength } from "./demo/strength";
import { generate } from "./demo/generator";
import { legacyImportItems, privateVault, workVault, type SampleVault } from "./demo/sampleData";

const DEMO_PASSWORD = "demo";
const DEMO_RECOVERY_KEY = "VXDMO-2K7QF-9MZ4T-H8WRC-31PNA";
const EXTENSION_ID = "imfndemblnaalppnmdplagajjielnaok";
const DAY = 86_400_000;

interface MockVault {
  info: VaultInfo;
  password: string;
  recoveryKey: string | null;
  data: VaultData;
}

interface MockState {
  vaults: Map<string, MockVault>;
  unlockedId: string | null;
  settings: Settings;
  appInfo: AppInfo;
  browsers: BrowserInfo[];
  clients: PairedClient[];
  pairings: Map<string, PairingRequest>;
  lastActivity: number;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fail(code: string): never {
  // Tauri rejects invoke() with the plain error string; mimic that.
  throw code;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function uuid(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = ((b[6] ?? 0) & 0x0f) | 0x40;
  b[8] = ((b[8] ?? 0) & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function str(args: Record<string, unknown>, key: string): string {
  const value = args[key];
  if (typeof value !== "string") fail(`invalid_input:missing argument ${key}`);
  return value;
}

function optStr(args: Record<string, unknown>, key: string): string | null {
  const value = args[key];
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") fail(`invalid_input:${key} must be a string`);
  return value;
}

function bool(args: Record<string, unknown>, key: string): boolean {
  const value = args[key];
  if (typeof value !== "boolean") fail(`invalid_input:missing argument ${key}`);
  return value;
}

function dataDir(portable: boolean): string {
  return portable ? "D:\\Keystead\\Keystead-Data" : "C:\\Users\\Demo\\AppData\\Local\\Keystead\\v2";
}

function vaultPath(id: string, portable: boolean): string {
  return `${dataDir(portable)}\\vaults\\${id}.keystead`;
}

function makeRecoveryKey(): string {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32
  const bytes = crypto.getRandomValues(new Uint8Array(25));
  const chars = [...bytes].map((b) => alphabet.charAt(b % 32)).join("");
  return chars.match(/.{5}/g)?.join("-") ?? chars;
}

function normalizeRecovery(key: string): string {
  return key.toUpperCase().replace(/[\s-]/g, "");
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let state: MockState | null = null;

function vaultFromSample(sample: SampleVault, id: string, now: number): MockVault {
  return {
    info: {
      id,
      name: sample.name,
      path: vaultPath(id, false),
      createdAt: now - sample.createdDaysAgo * DAY,
      updatedAt: now - DAY,
      hasRecoveryKey: sample.hasRecoveryKey,
    },
    password: DEMO_PASSWORD,
    recoveryKey: sample.hasRecoveryKey ? DEMO_RECOVERY_KEY : null,
    data: { items: sample.items, folders: sample.folders, generatorHistory: sample.generatorHistory },
  };
}

function initState(): MockState {
  const now = Date.now();
  const params = new URLSearchParams(window.location.search);
  const empty = params.get("mock") === "empty";
  const vaults = new Map<string, MockVault>();
  if (!empty) {
    const priv = vaultFromSample(privateVault(now), "6f1c2a9e-3b7d-4c51-9a0e-1d2f3a4b5c6d", now);
    const work = vaultFromSample(workVault(now), "a83e7b10-55c2-4f6e-b1d9-7c8e9f0a1b2c", now);
    vaults.set(priv.info.id, priv);
    vaults.set(work.info.id, work);
  }
  const lang = params.get("lang") === "en" ? "en" : "de";
  return {
    vaults,
    unlockedId: null,
    settings: {
      theme: "system",
      language: lang,
      autoLockMinutes: 15,
      lockOnSystemLock: true,
      clipboardClearSeconds: 30,
      minimizeToTray: true,
      startInTray: false,
      browserIntegration: true,
      lastVaultId: empty ? null : "6f1c2a9e-3b7d-4c51-9a0e-1d2f3a4b5c6d",
      showIcons: false,
    },
    appInfo: {
      version: "2.0.0",
      dataDir: dataDir(false),
      portable: false,
      platform: "windows",
      extensionId: EXTENSION_ID,
    },
    browsers: [
      { id: "chrome", name: "Google Chrome", detected: true, registered: true },
      { id: "edge", name: "Microsoft Edge", detected: true, registered: false },
      { id: "brave", name: "Brave", detected: false, registered: false },
      { id: "chromium", name: "Chromium", detected: false, registered: false },
      { id: "vivaldi", name: "Vivaldi", detected: false, registered: false },
    ],
    clients: empty
      ? []
      : [{ id: "c-7f3a", name: "Chrome – DESKTOP-4F2K9", createdAt: now - 32 * DAY, lastSeenAt: now - 2 * 3_600_000 }],
    pairings: new Map(),
    lastActivity: now,
  };
}

function getState(): MockState {
  if (!state) {
    state = initState();
    startAutoLockTimer();
    installDevHelpers();
  }
  return state;
}

function openVault(): MockVault {
  const s = getState();
  const vault = s.unlockedId ? s.vaults.get(s.unlockedId) : undefined;
  if (!vault) fail("locked");
  return vault;
}

function touch(vault: MockVault): void {
  vault.info.updatedAt = Date.now();
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

type Listener = (payload: unknown) => void;
const listeners = new Map<string, Set<Listener>>();

export function mockListen(event: string, handler: Listener): () => void {
  getState();
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(handler);
  return () => {
    listeners.get(event)?.delete(handler);
  };
}

function emit(event: string, payload: unknown): void {
  listeners.get(event)?.forEach((handler) => {
    try {
      handler(payload);
    } catch (err) {
      console.error(`mock listener for ${event} failed`, err);
    }
  });
}

function lock(reason: "manual" | "timeout" | "system"): void {
  const s = getState();
  if (!s.unlockedId) return;
  s.unlockedId = null;
  emit("vault://locked", { reason });
}

function startAutoLockTimer(): void {
  window.setInterval(() => {
    const s = state;
    if (!s?.unlockedId || s.settings.autoLockMinutes <= 0) return;
    if (Date.now() - s.lastActivity > s.settings.autoLockMinutes * 60_000) lock("timeout");
  }, 5_000);
}

function requestPairing(clientName = "Microsoft Edge – DESKTOP-4F2K9"): PairingRequest {
  const s = getState();
  const code = String(crypto.getRandomValues(new Uint32Array(1))[0] ?? 0).padStart(6, "0").slice(-6);
  const request: PairingRequest = { requestId: uuid(), clientName, code };
  s.pairings.set(request.requestId, request);
  emit("bridge://pairing-request", request);
  // Like the real bridge, a request expires after 120 s.
  window.setTimeout(() => s.pairings.delete(request.requestId), 120_000);
  return request;
}

export interface MockDevHelpers {
  triggerPairing: (clientName?: string) => PairingRequest;
  simulateTimeoutLock: () => void;
  simulateExternalChange: () => void;
  simulateUnlockRequest: () => void;
}

declare global {
  interface Window {
    __keysteadMock?: MockDevHelpers;
  }
}

function installDevHelpers(): void {
  window.__keysteadMock = {
    triggerPairing: (clientName?: string) => requestPairing(clientName),
    simulateTimeoutLock: () => lock("timeout"),
    simulateExternalChange: () => emit("vault://changed", {}),
    simulateUnlockRequest: () => emit("bridge://unlock-request", {}),
  };
}

/** Used by the settings page in demo mode ("Kopplungsanfrage simulieren"). */
export function mockTriggerPairing(): void {
  window.setTimeout(() => requestPairing(), 400);
}

// ---------------------------------------------------------------------------
// Domain logic
// ---------------------------------------------------------------------------

function normalizeItem(input: VaultItem): VaultItem {
  const item = clone(input);
  if (item.type !== "login") item.login = null;
  else
    item.login ??= { username: "", password: "", uris: [], totp: "", passwordRevisedAt: null };
  if (item.type !== "card") item.card = null;
  else item.card ??= { cardholderName: "", brand: "", number: "", expMonth: "", expYear: "", code: "" };
  if (item.type !== "identity") item.identity = null;
  else
    item.identity ??= {
      title: "", firstName: "", lastName: "", email: "", phone: "", company: "", address1: "", address2: "",
      postalCode: "", city: "", state: "", country: "", username: "",
    };
  if (item.type !== "login") item.passwordHistory = [];
  return item;
}

function saveItem(input: VaultItem): VaultItem {
  const vault = openVault();
  if (!input || typeof input !== "object") fail("invalid_input:item");
  if (!["login", "card", "identity", "note"].includes(input.type)) fail("invalid_input:unknown item type");
  if (!input.name?.trim()) fail("invalid_input:name_required");
  if (input.name.trim().length > 200) fail("invalid_input:name_too_long");
  const now = Date.now();
  const item = normalizeItem(input);
  item.name = item.name.trim();
  // Like the core: an unknown folder id is dropped instead of failing.
  if (item.folderId && !vault.data.folders.some((f) => f.id === item.folderId)) item.folderId = null;
  const idx = item.id ? vault.data.items.findIndex((i) => i.id === item.id) : -1;
  const existing = idx >= 0 ? vault.data.items[idx] : undefined;
  if (!existing) {
    item.id = uuid();
    item.createdAt = now;
    item.passwordHistory = [];
    item.deletedAt = null;
    if (item.login) item.login.passwordRevisedAt = null;
  } else {
    item.createdAt = existing.createdAt;
    item.passwordHistory = existing.passwordHistory;
    item.deletedAt = existing.deletedAt;
    const oldPw = existing.login?.password ?? "";
    if (item.login) {
      item.login.passwordRevisedAt = existing.login?.passwordRevisedAt ?? null;
      if (oldPw && oldPw !== item.login.password) {
        item.passwordHistory = [{ password: oldPw, replacedAt: now }, ...existing.passwordHistory].slice(0, 10);
        item.login.passwordRevisedAt = now;
      }
    }
  }
  item.updatedAt = now;
  if (existing) vault.data.items[idx] = item;
  else vault.data.items.push(item);
  touch(vault);
  return clone(item);
}

function findItem(id: string): VaultItem {
  const item = openVault().data.items.find((i) => i.id === id);
  if (!item) fail("not_found");
  return item;
}

function healthReport(data: VaultData): HealthReport {
  const logins = data.items.filter((i) => i.type === "login" && i.deletedAt === null && i.login);
  const withPassword = logins.filter((i) => (i.login?.password ?? "") !== "");
  const weak = withPassword.filter((i) => estimateStrength(i.login?.password ?? "").score < 2).map((i) => i.id);
  const byPassword = new Map<string, string[]>();
  for (const item of withPassword) {
    const pw = item.login?.password ?? "";
    byPassword.set(pw, [...(byPassword.get(pw) ?? []), item.id]);
  }
  const reused = [...byPassword.values()].filter((ids) => ids.length > 1);
  const yearAgo = Date.now() - 365 * DAY;
  const old = withPassword.filter((i) => (i.login?.passwordRevisedAt ?? i.createdAt) < yearAgo).map((i) => i.id);
  const missingTotpCount = withPassword.filter((i) => !(i.login?.totp ?? "").trim()).length;
  const affected = new Set([...weak, ...reused.flat(), ...old]);
  const score = withPassword.length === 0 ? 100 : Math.round(100 * (1 - affected.size / withPassword.length));
  return { totalLogins: logins.length, weak, reused, old, missingTotpCount, score };
}

function browserStatus(): BrowserStatus {
  const s = getState();
  return {
    serverRunning: s.settings.browserIntegration,
    extensionId: EXTENSION_ID,
    browsers: clone(s.browsers),
    clients: clone(s.clients),
  };
}

function parseBrowserIds(args: Record<string, unknown>): BrowserId[] {
  const value = args.browsers;
  if (!Array.isArray(value)) fail("invalid_input:browsers");
  const known: BrowserId[] = ["chrome", "edge", "brave", "chromium", "vivaldi"];
  return value.map((id) => {
    if (typeof id !== "string" || !known.includes(id as BrowserId)) fail(`invalid_input:unknown browser ${String(id)}`);
    return id as BrowserId;
  });
}

function validateSettings(input: unknown): Settings {
  if (!input || typeof input !== "object") fail("invalid_input:settings");
  const s = input as Settings;
  if (!["system", "light", "dark"].includes(s.theme)) fail("invalid_input:theme");
  if (!["de", "en"].includes(s.language)) fail("invalid_input:language");
  if (!Number.isInteger(s.autoLockMinutes) || s.autoLockMinutes < 0) fail("invalid_input:autoLockMinutes");
  if (!Number.isInteger(s.clipboardClearSeconds) || s.clipboardClearSeconds < 0) fail("invalid_input:clipboardClearSeconds");
  return clone(s);
}

let lastCopied: string | null = null;

async function copyText(text: string, sensitive: boolean): Promise<void> {
  const s = getState();
  lastCopied = text;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Clipboard access can be denied in automated browsers; the demo ignores that.
  }
  if (sensitive && s.settings.clipboardClearSeconds > 0) {
    window.setTimeout(() => {
      if (lastCopied !== text) return;
      lastCopied = null;
      navigator.clipboard.writeText("").catch(() => undefined);
    }, s.settings.clipboardClearSeconds * 1000);
  }
}

function importItems(vault: MockVault, items: VaultItem[], folders: Folder[] = []): number {
  const folderMap = new Map<string, string>();
  for (const folder of folders) {
    const id = uuid();
    folderMap.set(folder.id, id);
    vault.data.folders.push({ id, name: folder.name });
  }
  for (const item of items) {
    const copy = normalizeItem(item);
    copy.id = uuid();
    copy.folderId = copy.folderId ? folderMap.get(copy.folderId) ?? null : null;
    vault.data.items.push(copy);
  }
  touch(vault);
  return items.length;
}

// ---------------------------------------------------------------------------
// Command dispatcher
// ---------------------------------------------------------------------------

async function dispatch(command: string, args: Record<string, unknown>): Promise<unknown> {
  const s = getState();
  switch (command) {
    case "app_info":
      return clone(s.appInfo);

    case "list_vaults":
      return [...s.vaults.values()]
        .map((v) => clone(v.info))
        .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));

    case "session_state": {
      const vault = s.unlockedId ? s.vaults.get(s.unlockedId) : undefined;
      const result: SessionState = { unlocked: Boolean(vault), vault: vault ? clone(vault.info) : null };
      return result;
    }

    case "create_vault": {
      const name = str(args, "name").trim();
      const password = str(args, "masterPassword");
      if (!name) fail("invalid_input:name_required");
      if (!password) fail("invalid_input:password_empty");
      await sleep(450);
      const id = uuid();
      const now = Date.now();
      const vault: MockVault = {
        info: { id, name, path: vaultPath(id, s.appInfo.portable), createdAt: now, updatedAt: now, hasRecoveryKey: false },
        password,
        recoveryKey: null,
        data: { items: [], folders: [], generatorHistory: [] },
      };
      s.vaults.set(id, vault);
      s.unlockedId = id;
      s.lastActivity = now;
      return clone(vault.info);
    }

    case "unlock_vault": {
      const vault = s.vaults.get(str(args, "vaultId"));
      const password = str(args, "masterPassword");
      await sleep(350);
      if (!vault) fail("not_found");
      if (vault.password !== password) fail("wrong_password");
      s.unlockedId = vault.info.id;
      s.lastActivity = Date.now();
      return clone(vault.info);
    }

    case "unlock_with_recovery": {
      const vault = s.vaults.get(str(args, "vaultId"));
      const key = str(args, "recoveryKey");
      const newPassword = str(args, "newMasterPassword");
      await sleep(500);
      if (!vault) fail("not_found");
      if (!/^[0-9A-Z]{25}$/.test(normalizeRecovery(key))) fail("invalid_input:recovery_key_format");
      if (!vault.recoveryKey) fail("not_found");
      if (normalizeRecovery(vault.recoveryKey) !== normalizeRecovery(key)) fail("wrong_password");
      if (!newPassword) fail("invalid_input:password_empty");
      vault.password = newPassword;
      touch(vault);
      s.unlockedId = vault.info.id;
      s.lastActivity = Date.now();
      return clone(vault.info);
    }

    case "lock_vault":
      lock("manual");
      return null;

    case "touch_activity":
      s.lastActivity = Date.now();
      return null;

    case "list_items":
      return clone(openVault().data.items);

    case "list_folders":
      return clone(openVault().data.folders).sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));

    case "save_item":
      return saveItem(args.item as VaultItem);

    case "trash_item": {
      findItem(str(args, "id")).deletedAt = Date.now();
      touch(openVault());
      return null;
    }

    case "restore_item": {
      findItem(str(args, "id")).deletedAt = null;
      touch(openVault());
      return null;
    }

    case "delete_item": {
      const vault = openVault();
      const id = str(args, "id");
      findItem(id);
      vault.data.items = vault.data.items.filter((i) => i.id !== id);
      touch(vault);
      return null;
    }

    case "empty_trash": {
      const vault = openVault();
      const before = vault.data.items.length;
      vault.data.items = vault.data.items.filter((i) => i.deletedAt === null);
      touch(vault);
      return before - vault.data.items.length;
    }

    case "save_folder": {
      const vault = openVault();
      const folder = args.folder as Folder | undefined;
      const name = folder?.name?.trim() ?? "";
      if (!folder || !name) fail("invalid_input:name_required");
      const existing = folder.id ? vault.data.folders.find((f) => f.id === folder.id) : undefined;
      if (existing) {
        existing.name = name;
        touch(vault);
        return clone(existing);
      }
      const created: Folder = { id: uuid(), name };
      vault.data.folders.push(created);
      touch(vault);
      return clone(created);
    }

    case "delete_folder": {
      const vault = openVault();
      const id = str(args, "id");
      if (!vault.data.folders.some((f) => f.id === id)) fail("not_found");
      vault.data.folders = vault.data.folders.filter((f) => f.id !== id);
      vault.data.items.forEach((item) => {
        if (item.folderId === id) item.folderId = null;
      });
      touch(vault);
      return null;
    }

    case "generate_password": {
      const options = { ...DEFAULT_GENERATOR_OPTIONS, ...(args.options as Partial<GeneratorOptions>) };
      let password: string;
      try {
        password = generate(options);
      } catch (err) {
        fail(err instanceof Error && err.message.startsWith("invalid_input:") ? err.message : "invalid_input:options");
      }
      const vault = s.unlockedId ? s.vaults.get(s.unlockedId) : undefined;
      if (bool(args, "remember") && vault) {
        const entry: GeneratedPassword = { password, createdAt: Date.now() };
        vault.data.generatorHistory = [entry, ...vault.data.generatorHistory].slice(0, 50);
      }
      return password;
    }

    case "generator_history":
      return clone(openVault().data.generatorHistory);

    case "clear_generator_history":
      openVault().data.generatorHistory = [];
      return null;

    case "password_strength":
      return estimateStrength(str(args, "password"));

    case "totp_code":
      try {
        return await totpNow(str(args, "seed"));
      } catch {
        fail("invalid_input:totp_secret");
      }

    case "copy_text":
      await copyText(str(args, "text"), bool(args, "sensitive"));
      return null;

    case "health_report":
      return healthReport(openVault().data);

    case "change_master_password": {
      const vault = openVault();
      const current = str(args, "current");
      const next = str(args, "newPassword");
      await sleep(400);
      if (current !== vault.password) fail("wrong_password");
      if (!next) fail("invalid_input:password_empty");
      vault.password = next;
      touch(vault);
      return null;
    }

    case "create_recovery_key": {
      const vault = openVault();
      await sleep(300);
      vault.recoveryKey = makeRecoveryKey();
      vault.info.hasRecoveryKey = true;
      touch(vault);
      return vault.recoveryKey;
    }

    case "remove_recovery_key": {
      const vault = openVault();
      vault.recoveryKey = null;
      vault.info.hasRecoveryKey = false;
      touch(vault);
      return null;
    }

    case "rename_vault": {
      const vault = openVault();
      const name = str(args, "name").trim();
      if (!name) fail("invalid_input:name_required");
      vault.info.name = name;
      touch(vault);
      return clone(vault.info);
    }

    case "delete_vault": {
      const id = str(args, "vaultId");
      const vault = s.vaults.get(id);
      await sleep(350);
      if (!vault) fail("not_found");
      if (vault.password !== str(args, "masterPassword")) fail("wrong_password");
      s.vaults.delete(id);
      if (s.unlockedId === id) s.unlockedId = null;
      if (s.settings.lastVaultId === id) s.settings.lastVaultId = null;
      return null;
    }

    case "legacy_scan": {
      const result: LegacyVaultInfo[] = [
        { name: "Max (VaultX 1.x)", path: "C:\\Users\\Demo\\AppData\\Local\\VaultX\\vault_max.json" },
      ];
      return result;
    }

    case "import_data": {
      const vault = openVault();
      const format = str(args, "format") as ImportFormat;
      const path = str(args, "path");
      const password = optStr(args, "password");
      if (!path) fail("invalid_input:path_required");
      await sleep(500);
      const now = Date.now();
      let report: ImportReport;
      switch (format) {
        case "legacy":
          if (password !== DEMO_PASSWORD) fail("wrong_password");
          report = {
            imported: importItems(vault, legacyImportItems(now)),
            skipped: 0,
            warnings: ["Die alte Zwei-Faktor-Entsperrung (TotpSecret) wurde nicht übernommen."],
          };
          break;
        case "keystead":
          if (password !== DEMO_PASSWORD) fail("wrong_password");
          report = { imported: importItems(vault, legacyImportItems(now).slice(0, 2)), skipped: 0, warnings: [] };
          break;
        case "csv":
        case "bitwarden_json":
          report = {
            imported: importItems(vault, legacyImportItems(now).slice(1)),
            skipped: 1,
            warnings: ["Zeile 7: kein Name und keine URL – übersprungen."],
          };
          break;
        default:
          fail(`unsupported:${String(format)}`);
      }
      emit("vault://changed", {});
      return report;
    }

    case "export_data": {
      const vault = openVault();
      const format = str(args, "format") as ExportFormat;
      const password = optStr(args, "password");
      str(args, "path");
      await sleep(400);
      if (str(args, "masterPassword") !== vault.password) fail("wrong_password");
      if (!["keystead", "csv", "bitwarden_json"].includes(format)) fail(`unsupported:${String(format)}`);
      if (format === "keystead" && !password) fail("invalid_input:password_required");
      return null;
    }

    case "get_settings":
      return clone(s.settings);

    case "save_settings":
      s.settings = validateSettings(args.settings);
      return clone(s.settings);

    case "browser_status":
      return browserStatus();

    case "register_browsers": {
      const ids = parseBrowserIds(args);
      s.browsers.forEach((b) => {
        if (ids.includes(b.id)) b.registered = true;
      });
      await sleep(200);
      return browserStatus();
    }

    case "unregister_browsers": {
      const ids = parseBrowserIds(args);
      s.browsers.forEach((b) => {
        if (ids.includes(b.id)) b.registered = false;
      });
      await sleep(200);
      return browserStatus();
    }

    case "revoke_client": {
      const id = str(args, "clientId");
      if (!s.clients.some((c) => c.id === id)) fail("not_found");
      s.clients = s.clients.filter((c) => c.id !== id);
      return browserStatus();
    }

    case "respond_pairing": {
      const request = s.pairings.get(str(args, "requestId"));
      if (!request) fail("not_found");
      s.pairings.delete(request.requestId);
      if (bool(args, "approve")) {
        const now = Date.now();
        s.clients.push({ id: uuid(), name: request.clientName, createdAt: now, lastSeenAt: now });
      }
      return null;
    }

    case "open_terminal":
    case "open_data_dir":
      return null;

    case "set_portable_mode": {
      const enabled = bool(args, "enabled");
      await sleep(300);
      s.appInfo.portable = enabled;
      s.appInfo.dataDir = dataDir(enabled);
      s.vaults.forEach((v) => {
        v.info.path = vaultPath(v.info.id, enabled);
      });
      return clone(s.appInfo);
    }

    default:
      fail(`unsupported:unknown command ${command}`);
  }
}

export async function mockInvoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
  // Simulate IPC latency so loading states are exercised.
  await sleep(20 + Math.random() * 60);
  return (await dispatch(command, args)) as T;
}

export async function mockPickOpenFile(filters?: { name: string; extensions: string[] }[]): Promise<string | null> {
  await sleep(150);
  const ext = filters?.[0]?.extensions[0] ?? "csv";
  const names: Record<string, string> = {
    json: "vault_export.json",
    csv: "passwords.csv",
    keystead: "Keystead-Export.keystead",
  };
  return `C:\\Users\\Demo\\Downloads\\${names[ext] ?? `import.${ext}`}`;
}

export async function mockPickSaveFile(defaultPath?: string): Promise<string | null> {
  await sleep(150);
  return `C:\\Users\\Demo\\Documents\\${defaultPath ?? "Keystead-Export"}`;
}
