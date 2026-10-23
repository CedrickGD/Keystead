// In-memory mock of the Tauri backend (docs/ARCHITECTURE.md, "Desktop backend
// ↔ frontend"). Used automatically when the UI runs outside of Tauri, e.g.
// `npm run dev` in a normal browser and the automated screenshots.
//
// Two demo vaults ("Privat", "Arbeit"), master password "demo" for both.
// URL parameters: `?mock=empty` starts without any vault (first-run screen).
// `?platform=android` (or `ios`) pretends to be the phone app: `AppInfo`
// says so (`readOnly`, no extension id), only the phone's commands exist
// (`PHONE_COMMANDS`; any other one fails like an unknown command), the UI
// takes its phone layout (narrow the window as well to see it at phone
// size) and the background lock (`lockInBackgroundSeconds`) runs on the
// page's visibility – switch tabs or minimise the browser to leave the app
// (the backend's wait for a pending clipboard clear is not simulated).
// `__keysteadMock.simulateBack()` sends the Android back gesture.
// The phone's cloud-folder sync is simulated (`link_folder` & co.): the
// "PC's folder" holds the two sample vaults; "Cloud-Ordner wählen" links
// "Keystead" and copies them in (with `?mock=empty` the welcome screen shows
// first), `?sync=linked` starts already linked (with `?mock=empty`: before
// the PC's copies arrived – the sync at start brings them). `simulatePcSave()` lets the
// next sync bring a newer revision (a note "Neu vom PC" in the open vault
// or "Privat"), `simulatePcPasswordChange()` one with the new master
// password "demo2" (an open vault locks with reason `keyChanged`),
// `simulateFolderLost()` makes the next sync fail with
// `unsupported:folder_access_lost` (linking again repairs it). Like the real
// app, a linked folder is synced on its own at start and whenever the page
// becomes visible again (`Resumed`), at most every 60 s – switch tabs and
// come back a minute after `simulatePcSave()` to see it.
// The desktop's side, "Kopie im Cloud-Ordner aktuell halten"
// (`mirrorFolder`, `mirror_status`, `mirror://status`), is simulated too:
// the folder dialog picks `C:\Users\Demo\Nextcloud\Keystead`, every change
// of the open vault (and its unlock) "copies" it ~0.4 s later, `?mirror=on`
// starts with the folder set, and `simulateMirrorFolderLost(lost?)` lets
// the copies fail with `unsupported:mirror_folder_missing` (a cloud drive
// that is not there) until it is called with `false`.
// The phone's fingerprint unlock (`device_unlock_status` & co.) is simulated
// as well: the "system prompt" answers after ~0.9 s with success, unless
// `__keysteadMock.setFingerprintResult("cancel" | "fail" | "lockout")` makes
// the next one fail; `simulateNewFingerprint()` invalidates every setup (the
// next fingerprint unlock resets it, like a new enrollment), a synced master
// password change (`simulatePcPasswordChange()`) resets that vault's setup.
// `?fingerprint=on` starts with "Privat" set up, `?fingerprint=none` /
// `?fingerprint=notEnrolled` without a usable sensor.
// `?update=available` announces a fake update ~1 s after start (banner),
// `?update=portable` the same for a portable copy (download link instead of
// install), `?update=none` makes "Nach Updates suchen" find nothing; by
// default only the manual check finds the fake update.
// `window.__keysteadMock` exposes helpers to simulate backend events.
// Like the backend, `list_items` answers items without their secrets
// (demo/redact.ts); `reveal_secret`, `copy_secret_field`, `totp_for_item` and
// `get_item_for_edit` hand them out one at a time. Attachments live in a side
// store without bytes (demo/mockAttachments.ts). The generator works like the
// app's: `generate_password` answers previews, only `remember_generated`
// stores, `generator_history` is redacted (id + time) and `reveal_generated` /
// `copy_generated` hand out one entry; the presets live in the settings
// (demo/generator.ts has the built-in ones and the option checks).

import type {
  AppInfo,
  BrowserId,
  BrowserInfo,
  BrowserStatus,
  DeviceUnlockStatus,
  ExportFormat,
  Folder,
  FolderStatus,
  GeneratedPassword,
  GeneratedPasswordInfo,
  GeneratorOptions,
  GeneratorPreset,
  GeneratorPresets,
  HealthReport,
  ImportFormat,
  ImportReport,
  LegacyVaultInfo,
  LockReason,
  MirrorStatus,
  PairedClient,
  PairingRequest,
  SecretField,
  SessionState,
  Settings,
  SyncFileResult,
  TotpCode,
  UpdateInfo,
  VaultData,
  VaultInfo,
  VaultItem,
} from "./types";
import { DEFAULT_GENERATOR_OPTIONS } from "./types";
import { totpNow } from "./demo/totp";
import { estimateStrength } from "./demo/strength";
import { builtinPresets, generate, normalizeOptions, validatePresetName } from "./demo/generator";
import { GENERATOR_HISTORY_MAX, GENERATOR_PRESETS_MAX } from "./generatorRules";
import { legacyImportItems, privateVault, workVault, type SampleVault } from "./demo/sampleData";
import {
  MOCK_LIST_LIMIT,
  describe,
  mockImportFile,
  mockPreview,
  planMockImport,
  type MockImportFile,
  type MockPlan,
} from "./demo/importFiles";
import { mockClearIcons, mockFetchIcons, mockGetIcons, seedSampleIcons } from "./demo/mockIcons";
import {
  mockAddAttachment,
  mockAttachmentsOf,
  mockDeleteAttachments,
  mockPruneAttachments,
  mockRemoveAttachment,
  mockRevealAttachment,
  mockSaveAttachment,
  seedSampleAttachments,
} from "./demo/mockAttachments";
import { mockBreachCount, mockPwnedReport } from "./demo/pwned";
import { parseSecretField, redactItem, storedSecret } from "./demo/redact";

export { mockPickAttachmentFile } from "./demo/mockAttachments";

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
  sync: MockSync;
  mirror: MockMirror;
  deviceUnlock: MockDeviceUnlock;
}

/** The phone's fingerprint unlock (see the header comment). */
interface MockDeviceUnlock {
  unavailable: DeviceUnlockStatus["unavailableReason"];
  /** Set-up vaults → the master password at setup (stands in for the KDF salt of the sealed KEK). */
  sealed: Map<string, string>;
  /** Setups removed because they no longer opened their vault. */
  invalidated: Set<string>;
  /** Setups whose keystore key is gone (a new fingerprint): reset at their next use. */
  stale: Set<string>;
  /** What the next prompt does. */
  next: MockFingerprintResult;
}

export type MockFingerprintResult = "ok" | "cancel" | "fail" | "lockout";

/** The desktop's cloud-folder mirror (see the header comment). */
interface MockMirror {
  lastMirroredAt: number | null;
  lastError: string | null;
  /** The copies fail: the folder is not there. */
  lost: boolean;
}

/** The phone's linked cloud folder (see the header comment). */
interface MockSync {
  link: { folderName: string; linkedAt: number } | null;
  lastSyncAt: number | null;
  lastError: string | null;
  files: SyncFileResult[];
  syncing: boolean;
  /** The PC's copies in the folder, by vault id. */
  folder: Map<string, { revision: number; password: string; newNotes: number }>;
  /** The revisions of the phone's copies. */
  phone: Map<string, number>;
  /** The next sync finds the folder's permission revoked. */
  lost: boolean;
}

/** What the mock's folder dialog picks (a Nextcloud folder). */
const MOCK_MIRROR_FOLDER = "C:\\Users\\Demo\\Nextcloud\\Keystead";

const PRIVATE_VAULT_ID = "6f1c2a9e-3b7d-4c51-9a0e-1d2f3a4b5c6d";
const WORK_VAULT_ID = "a83e7b10-55c2-4f6e-b1d9-7c8e9f0a1b2c";

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
  return portable ? "D:\\Keystead\\Keystead-Data" : "C:\\Users\\Demo\\AppData\\Local\\Keystead";
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
  seedSampleAttachments(id, sample.attachments, now);
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
    const priv = vaultFromSample(privateVault(now), PRIVATE_VAULT_ID, now);
    const work = vaultFromSample(workVault(now), WORK_VAULT_ID, now);
    vaults.set(priv.info.id, priv);
    // Website icons as if the app had loaded them before (sample icons).
    seedSampleIcons(priv.info.id, priv.data.items);
    vaults.set(work.info.id, work);
  }
  const lang = params.get("lang") === "en" ? "en" : "de";
  const platform = params.get("platform");
  const phone = platform === "android" || platform === "ios";
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
      lastVaultId: empty ? null : PRIVATE_VAULT_ID,
      websiteIcons: true,
      updateCheck: true,
      // The app's default is "stable"; the mock starts on "beta" so the fake update flows (?update=…) show.
      updateChannel: "beta",
      lockInBackgroundSeconds: 60,
      deviceUnlock: phone && params.get("fingerprint") === "on" && !empty,
      mirrorFolder: !phone && params.get("mirror") === "on" ? MOCK_MIRROR_FOLDER : null,
      generatorPresets: empty
        ? []
        : [
            {
              id: "5d0c8a3e-1f7b-4c2d-9e6a-7b8c9d0e1f2a",
              name: "Online-Banking",
              options: { ...DEFAULT_GENERATOR_OPTIONS, length: 16, symbolSet: "!?#", startWithLetter: true },
            },
          ],
      generatorDefault: "standard",
    },
    appInfo: {
      version: installedMockVersion(),
      dataDir: dataDir(false),
      portable: false,
      platform: phone ? platform : "windows",
      extensionId: phone ? null : EXTENSION_ID,
      readOnly: phone,
      attachmentsReadOnly: phone,
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
    sync: initSync(phone && params.get("sync") === "linked", empty, now),
    mirror: { lastMirroredAt: null, lastError: null, lost: false },
    deviceUnlock: initDeviceUnlock(phone ? params.get("fingerprint") : null, empty),
  };
}

function initDeviceUnlock(mode: string | null, empty: boolean): MockDeviceUnlock {
  return {
    unavailable: mode === "none" ? "noHardware" : mode === "notEnrolled" ? "notEnrolled" : null,
    sealed: new Map(mode === "on" && !empty ? [[PRIVATE_VAULT_ID, DEMO_PASSWORD]] : []),
    invalidated: new Set(),
    stale: new Set(),
    next: "ok",
  };
}

function initSync(linked: boolean, empty: boolean, now: number): MockSync {
  const folder = new Map([
    [PRIVATE_VAULT_ID, { revision: 42, password: DEMO_PASSWORD, newNotes: 0 }],
    [WORK_VAULT_ID, { revision: 17, password: DEMO_PASSWORD, newNotes: 0 }],
  ]);
  const phone = new Map<string, number>(empty ? [] : [...folder].map(([id, copy]) => [id, copy.revision]));
  const sync: MockSync = {
    link: null,
    lastSyncAt: null,
    lastError: null,
    files: [],
    syncing: false,
    folder,
    phone,
    lost: false,
  };
  if (linked) {
    sync.link = { folderName: "Keystead", linkedAt: now - 3 * DAY };
    sync.lastSyncAt = now - 4 * 60_000;
    // `?mock=empty`: linked before the PC's copies arrived – they are in the
    // folder now, and the sync at start brings them.
    sync.files = empty ? [] : [...folder].map(([id, copy]) => syncResult(id, copy.revision, "upToDate", false));
  }
  return sync;
}

function getState(): MockState {
  if (!state) {
    state = initState();
    startAutoLockTimer();
    if (state.appInfo.platform === "android" || state.appInfo.platform === "ios") {
      startBackgroundLockTimer();
      startAutoSync();
    }
    installDevHelpers();
    scheduleMockUpdateAnnouncement();
  }
  return state;
}

// ---------------------------------------------------------------------------
// Fake updates (see the header comment)
// ---------------------------------------------------------------------------

const MOCK_CURRENT_VERSION = "2.0.0-beta.4";
const MOCK_NEW_VERSION = "2.0.0-beta.5";
const MOCK_INSTALLED_KEY = "keystead.mock.installedVersion";
const MOCK_UPDATE_NOTES = [
  "– Updates direkt in der App: signiert, geprüft und mit einem Klick installiert.",
  "– Die Browser-Erweiterung liegt in einem eigenen Ordner der App und aktualisiert sich mit.",
  "– Behoben: Ein kopiertes Passwort wird beim Sperren zuverlässig aus der Zwischenablage gelöscht.",
].join("\n");

/** The version the mock "runs": a fake update installed in this tab survives the simulated restart. */
function installedMockVersion(): string {
  try {
    return sessionStorage.getItem(MOCK_INSTALLED_KEY) ?? MOCK_CURRENT_VERSION;
  } catch {
    return MOCK_CURRENT_VERSION;
  }
}

function mockUpdateMode(): string {
  return new URLSearchParams(window.location.search).get("update") ?? "";
}

let mockPendingUpdate: UpdateInfo | null = null;
let mockInstalling = false;

/** What the fake update server answers for the current settings. */
function mockUpdateInfo(portable = mockUpdateMode() === "portable"): UpdateInfo {
  const s = getState();
  const current = s.appInfo.version;
  const offered = mockUpdateMode() !== "none" && s.settings.updateChannel === "beta" && current !== MOCK_NEW_VERSION;
  return {
    available: offered,
    currentVersion: current,
    version: s.settings.updateChannel === "beta" ? MOCK_NEW_VERSION : null,
    notes: s.settings.updateChannel === "beta" ? MOCK_UPDATE_NOTES : null,
    date: new Date(Date.now() - 2 * 3_600_000).toISOString(),
    canInstall: offered && !portable,
    releaseUrl: `https://github.com/CedrickGD/Keystead/releases${s.settings.updateChannel === "beta" ? `/tag/v${MOCK_NEW_VERSION}` : ""}`,
  };
}

function announceMockUpdate(portable?: boolean): void {
  const info = mockUpdateInfo(portable);
  if (!info.available) return;
  mockPendingUpdate = info;
  emit("update://available", clone(info));
}

function scheduleMockUpdateAnnouncement(): void {
  const mode = mockUpdateMode();
  if (mode !== "available" && mode !== "portable") return;
  window.setTimeout(() => {
    if (state?.settings.updateCheck) announceMockUpdate();
  }, 1_000);
}

/** Fake download (~9 MB, a few seconds), then "restart" (reload with the new version). */
async function installMockUpdate(): Promise<null> {
  const info = mockUpdateInfo();
  if (!info.available) fail("not_found");
  if (!info.canInstall) fail("unsupported:update_portable");
  if (mockInstalling) fail("invalid_input:update_in_progress");
  mockInstalling = true;
  const total = 9_437_184;
  const params = new URLSearchParams(window.location.search);
  // `?updateSpeed=slow` keeps the progress on screen (screenshots).
  const step = params.get("updateSpeed") === "slow" ? 90_000 : 480_000;
  for (let done = 0; done < total; ) {
    await sleep(120);
    done = Math.min(total, done + step);
    emit("update://progress", { downloaded: done, total });
  }
  emit("update://ready", { version: info.version });
  // Like the backend: the vault is closed without an event (the banner shows the restart).
  getState().unlockedId = null;
  if (params.get("updateSpeed") !== "slow") {
    window.setTimeout(() => {
      try {
        sessionStorage.setItem(MOCK_INSTALLED_KEY, info.version ?? MOCK_NEW_VERSION);
      } catch {
        /* the restart then shows the old version again */
      }
      window.location.reload();
    }, 2_500);
  }
  return null;
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

function lock(reason: LockReason): void {
  const s = getState();
  if (!s.unlockedId) return;
  s.unlockedId = null;
  pendingImport = null;
  importTicket += 1;
  emit("vault://locked", { reason });
}

function startAutoLockTimer(): void {
  window.setInterval(() => {
    const s = state;
    if (!s?.unlockedId || s.settings.autoLockMinutes <= 0) return;
    if (Date.now() - s.lastActivity > s.settings.autoLockMinutes * 60_000) lock("timeout");
  }, 5_000);
}

/**
 * Phone mock: the page's visibility stands in for the activity's
 * onPause/onResume – hidden emits `app://suspended` and starts the clock;
 * `lockInBackgroundSeconds` later (or on return, if that is later) the vault
 * locks with reason `background`; null = never.
 */
function startBackgroundLockTimer(): void {
  let since: number | null = null;
  const check = () => {
    const seconds = state?.settings.lockInBackgroundSeconds ?? null;
    if (since === null || seconds === null) return;
    if (Date.now() - since >= seconds * 1000) {
      since = null;
      lock("background");
    }
  };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      if (since === null) since = Date.now();
      emit("app://suspended", {});
    } else {
      check();
      since = null;
    }
  });
  window.setInterval(check, 1_000);
}

/** When the last sync of any kind ended (the automatic one is throttled by it). */
let lastMockSyncEnd = 0;
/** Like `AUTO_SYNC_INTERVAL` in `mobile::sync`. */
const AUTO_SYNC_INTERVAL_MS = 60_000;

/**
 * Phone mock: like `mobile::sync::start` / `auto_sync` – a sync at start and
 * whenever the page becomes visible again (`Resumed`), only with a linked
 * folder, at most every 60 s and never while one runs; its errors only go
 * into the status (no toast – nobody asked for it).
 */
function startAutoSync(): void {
  const run = () => {
    const sync = state?.sync;
    if (!sync?.link || sync.syncing || Date.now() - lastMockSyncEnd < AUTO_SYNC_INTERVAL_MS) return;
    void mockSyncFolder().catch(() => undefined);
  };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") run();
  });
  // At start, while the page boots (as on the phone, its first event may come
  // before the page listens).
  window.setTimeout(run, 0);
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
  /**
   * The Android back gesture (`keystead:back`, as MainActivity sends it):
   * true = the page used it, false = the app would move to the background.
   */
  simulateBack: () => boolean;
  simulateTimeoutLock: () => void;
  /** The phone's lock after `lockInBackgroundSeconds` in the background (`vault://locked {background}`). */
  simulateBackgroundLock: () => void;
  simulateExternalChange: () => void;
  simulateUnlockRequest: () => void;
  /** Cloud-folder sync: the PC saved the open vault (or "Privat"); the next sync brings it. */
  simulatePcSave: () => void;
  /** Cloud-folder sync: the PC changed that vault's master password to "demo2". */
  simulatePcPasswordChange: () => void;
  /** Cloud-folder sync: the next sync finds the folder's permission revoked. */
  simulateFolderLost: () => void;
  /** Desktop mirror: the copies fail as if the cloud folder were gone (`false`: it is back). */
  simulateMirrorFolderLost: (lost?: boolean) => void;
  /** Fingerprint unlock: what the next "system prompt" does (default "ok"). */
  setFingerprintResult: (result: MockFingerprintResult) => void;
  /** Fingerprint unlock: a new fingerprint was enrolled – every setup is reset at its next use. */
  simulateNewFingerprint: () => void;
  /** Announces the fake update (`update://available`); `portable` → download link instead of install. */
  simulateUpdate: (portable?: boolean) => void;
  /** Drags a file over the window (the drop overlay stays until `dropFile` / `cancelDrag`). Default: a Chrome CSV export. */
  dragFile: (path?: string | string[]) => void;
  /** Drops a file onto the window (opens the import dialog while unlocked). */
  dropFile: (path?: string | string[]) => void;
  cancelDrag: () => void;
}

declare global {
  interface Window {
    __keysteadMock?: MockDevHelpers;
  }
}

function installDevHelpers(): void {
  window.__keysteadMock = {
    triggerPairing: (clientName?: string) => requestPairing(clientName),
    simulateBack: () => {
      const event = new Event("keystead:back", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    },
    simulateTimeoutLock: () => lock("timeout"),
    simulateBackgroundLock: () => lock("background"),
    simulateExternalChange: () => emit("vault://changed", {}),
    simulateUnlockRequest: () => emit("bridge://unlock-request", {}),
    simulatePcSave: () => mockPcSave(false),
    simulatePcPasswordChange: () => mockPcSave(true),
    simulateFolderLost: () => {
      getState().sync.lost = true;
    },
    simulateMirrorFolderLost: (lost = true) => {
      getState().mirror.lost = lost;
      scheduleMirrorPass();
    },
    setFingerprintResult: (result: MockFingerprintResult) => {
      getState().deviceUnlock.next = result;
    },
    simulateNewFingerprint: () => {
      const du = getState().deviceUnlock;
      du.stale = new Set(du.sealed.keys());
    },
    simulateUpdate: (portable?: boolean) => announceMockUpdate(portable),
    dragFile: (path?: string | string[]) => emit("mock://drag-drop", { type: "enter", paths: dropPaths(path) }),
    dropFile: (path?: string | string[]) => emit("mock://drag-drop", { type: "drop", paths: dropPaths(path) }),
    cancelDrag: () => emit("mock://drag-drop", { type: "leave" }),
  };
}

/** Used by the settings page in demo mode ("Kopplungsanfrage simulieren"). */
export function mockTriggerPairing(): void {
  window.setTimeout(() => requestPairing(), 400);
}

// ---------------------------------------------------------------------------
// Domain logic
// ---------------------------------------------------------------------------

// Tags, like `vault::normalize_tags` in keystead-core.
const TAG_MAX_CHARS = 50;
const TAGS_MAX = 20;

/** Tags are compared case-insensitively ("Bank" and "bank" are one tag). */
function tagEq(a: string, b: string): boolean {
  return a.toLowerCase() === b.toLowerCase();
}

function hasTag(tags: string[], tag: string): boolean {
  return tags.some((t) => tagEq(t, tag));
}

/** `tag_required` / `tag_too_long` for a single tag, trimmed. */
function validateTag(tag: string): string {
  const trimmed = tag.trim();
  if (!trimmed) fail("invalid_input:tag_required");
  if ([...trimmed].length > TAG_MAX_CHARS) fail("invalid_input:tag_too_long");
  return trimmed;
}

/**
 * Tags as the user typed them: trimmed, empty ones dropped, duplicates
 * removed case-insensitively (the first spelling stays), order kept;
 * `tag_too_long` / `tags_too_many` like the core.
 */
function normalizeTags(input: unknown): string[] {
  const out: string[] = [];
  for (const raw of Array.isArray(input) ? input : []) {
    if (typeof raw !== "string" || !raw.trim()) continue;
    const tag = validateTag(raw);
    if (hasTag(out, tag)) continue;
    if (out.length === TAGS_MAX) fail("invalid_input:tags_too_many");
    out.push(tag);
  }
  return out;
}

/** `normalizeTags` for imported items: nothing is refused, too long is shortened, too many are dropped. */
function normalizeTagsLossy(input: unknown): string[] {
  const out: string[] = [];
  for (const raw of Array.isArray(input) ? input : []) {
    if (typeof raw !== "string") continue;
    const tag = [...raw.trim()].slice(0, TAG_MAX_CHARS).join("").trim();
    if (!tag || hasTag(out, tag)) continue;
    if (out.length === TAGS_MAX) break;
    out.push(tag);
  }
  return out;
}

/**
 * `rename_tag` (`to` a string) / `delete_tag` (`to` null) over every item
 * incl. trash, like `retag_item` in the core: an item that already carries
 * `to` in another spelling keeps that one and only loses `from`; otherwise
 * `from` is replaced in place. Returns the number of changed items.
 */
function retagItems(items: VaultItem[], from: string, to: string | null): number {
  let changed = 0;
  for (const item of items) {
    if (!hasTag(item.tags, from)) continue;
    const merge = to !== null && item.tags.some((t) => !tagEq(t, from) && tagEq(t, to));
    const tags: string[] = [];
    let replaced = false;
    for (const tag of item.tags) {
      if (!tagEq(tag, from)) tags.push(tag);
      else if (to !== null && !merge && !replaced) {
        tags.push(to);
        replaced = true;
      }
    }
    if (tags.length === item.tags.length && tags.every((t, i) => t === item.tags[i])) continue;
    item.tags = tags;
    changed += 1;
  }
  return changed;
}

function normalizeItem(input: VaultItem): VaultItem {
  const item = clone(input);
  if (!Array.isArray(item.tags)) item.tags = [];
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
  item.tags = normalizeTags(item.tags);
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

/** `ItemListEntry` of an item: redacted, with its attachments (name, size, date). */
function entryOf(vault: MockVault, item: VaultItem) {
  return redactItem(item, mockAttachmentsOf(vault.info.id, item.id));
}

/** The open vault if it is the page's one (`pageVaultId`), otherwise `locked`. */
function pageVault(args: Record<string, unknown>): MockVault {
  const vault = openVault();
  if (optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
  return vault;
}

/** The item `itemId` of the page's vault (`pageVaultId`; another open vault is `locked`). */
function pageItem(args: Record<string, unknown>): VaultItem {
  const vault = pageVault(args);
  const item = vault.data.items.find((i) => i.id === str(args, "itemId"));
  if (!item) fail("not_found");
  return item;
}

function fieldArg(args: Record<string, unknown>): SecretField {
  const field = parseSecretField(args.field);
  if (!field) fail("invalid_input:field");
  return field;
}

/** `totp_for_item`: the current code only. */
async function secretTotp(item: VaultItem): Promise<TotpCode> {
  const seed = item.login?.totp.trim() ?? "";
  if (!seed) fail("not_found");
  try {
    return await totpNow(seed);
  } catch {
    fail("invalid_input:totp_secret");
  }
}

/** One secret of an item (`reveal_secret`, `copy_secret_field`). */
async function secretValue(item: VaultItem, field: SecretField): Promise<string> {
  if (field === "totpCode") return (await secretTotp(item)).code;
  const value = storedSecret(item, field);
  if (value === null) fail("not_found");
  return value;
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
    extensionDir: `${s.appInfo.dataDir}\\browser-extension`,
    extensionVersion: "2.0.0.4",
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

/** `save_settings`: checked like the backend; the generator presets and the default keep their stored values. */
function validateSettings(input: unknown, stored: Settings): Settings {
  if (!input || typeof input !== "object") fail("invalid_input:settings");
  const s = { ...(input as Settings), generatorPresets: stored.generatorPresets, generatorDefault: stored.generatorDefault };
  if (!["system", "light", "dark"].includes(s.theme)) fail("invalid_input:theme");
  if (!["de", "en"].includes(s.language)) fail("invalid_input:language");
  if (!Number.isInteger(s.autoLockMinutes) || s.autoLockMinutes < 0) fail("invalid_input:autoLockMinutes");
  if (!Number.isInteger(s.clipboardClearSeconds) || s.clipboardClearSeconds < 0) fail("invalid_input:clipboardClearSeconds");
  if (typeof s.updateCheck !== "boolean") fail("invalid_input:settings");
  if (!["beta", "stable"].includes(s.updateChannel)) fail("invalid_input:settings");
  if (s.lockInBackgroundSeconds !== null && (!Number.isInteger(s.lockInBackgroundSeconds) || s.lockInBackgroundSeconds < 0)) {
    fail("invalid_input:lockInBackgroundSeconds");
  }
  if (typeof s.deviceUnlock !== "boolean") fail("invalid_input:settings");
  if (s.mirrorFolder !== null && typeof s.mirrorFolder !== "string") fail("invalid_input:settings");
  return clone({ ...s, mirrorFolder: s.mirrorFolder?.trim() ? s.mirrorFolder : null });
}

// ---------------------------------------------------------------------------
// Cloud-folder mirror (desktop, see the header comment)
// ---------------------------------------------------------------------------

/** Like `mirror::check_folder`: an absolute dialog path outside the data directory. */
function checkMirrorFolder(folder: string): void {
  const parts = folder.split(/[\\/]/);
  if (!/^[A-Za-z]:[\\/]/.test(folder) || parts.includes("..") || parts.includes(".")) fail("invalid_input:mirror_folder");
  const data = dataDir(getState().appInfo.portable).toLowerCase();
  const lower = folder.toLowerCase().replace(/[\\/]+$/, "");
  if (lower === data || lower.startsWith(`${data}\\`) || parts[parts.length - 1]?.toLowerCase() === "vaults") {
    fail("invalid_input:mirror_folder_data_dir");
  }
}

function mirrorStatus(): MirrorStatus {
  const s = getState();
  return { folder: s.settings.mirrorFolder, lastMirroredAt: s.mirror.lastMirroredAt, lastError: s.mirror.lastError };
}

let mirrorTimer: number | undefined;

/** A pass of the mirror for the open vault, a moment after a change (like the worker thread). */
function scheduleMirrorPass(): void {
  window.clearTimeout(mirrorTimer);
  mirrorTimer = window.setTimeout(() => {
    const s = getState();
    if (!s.settings.mirrorFolder || !s.unlockedId || s.appInfo.readOnly) return;
    const before = JSON.stringify(mirrorStatus());
    if (s.mirror.lost) {
      s.mirror.lastError = "unsupported:mirror_folder_missing";
    } else {
      s.mirror.lastError = null;
      s.mirror.lastMirroredAt = Date.now();
    }
    if (JSON.stringify(mirrorStatus()) !== before) emit("mirror://status", mirrorStatus());
  }, 400);
}

/** Commands after which the real app copies the open vault into the mirror folder. */
const MIRROR_TRIGGERS = new Set([
  "create_vault",
  "unlock_vault",
  "unlock_with_recovery",
  "save_item",
  "set_favorite",
  "trash_item",
  "restore_item",
  "delete_item",
  "empty_trash",
  "save_folder",
  "delete_folder",
  "rename_tag",
  "delete_tag",
  "generate_password",
  "remember_generated",
  "clear_generator_history",
  "change_master_password",
  "create_recovery_key",
  "remove_recovery_key",
  "rename_vault",
  "import_data",
  "commit_import",
  "add_attachment",
  "remove_attachment",
  "clear_icons",
]);

// Generator (see the header comment and demo/generator.ts).

const BUILTIN_IDS = new Set(builtinPresets().map((p) => p.id));

/** Runs a generator check; its `invalid_input:<code>` becomes the command's error. */
function generatorCall<T>(run: () => T): T {
  try {
    return run();
  } catch (err) {
    if (err instanceof Error && err.message.startsWith("invalid_input:")) fail(err.message);
    throw err;
  }
}

/** `GeneratorOptions` from JSON like serde with defaults: missing fields get theirs, a wrong type is `invalid_input:options`. */
function parseOptions(value: unknown): GeneratorOptions {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail("invalid_input:options");
  const input = value as Record<string, unknown>;
  const out: GeneratorOptions = { ...DEFAULT_GENERATOR_OPTIONS };
  const target = out as unknown as Record<string, unknown>;
  for (const [key, fallback] of Object.entries(DEFAULT_GENERATOR_OPTIONS)) {
    if (!(key in input)) continue;
    const v = input[key];
    const ok =
      key === "kind"
        ? v === "password" || v === "passphrase"
        : key === "wordlist"
          ? v === "en" || v === "de"
          : typeof fallback === "number"
            ? typeof v === "number" && Number.isInteger(v) && v >= 0
            : typeof v === typeof fallback;
    if (!ok) fail("invalid_input:options");
    target[key] = v;
  }
  return out;
}

function addGenerated(vault: MockVault, password: string): void {
  const entry: GeneratedPassword = { id: uuid(), password, createdAt: Date.now() };
  vault.data.generatorHistory = [entry, ...vault.data.generatorHistory].slice(0, GENERATOR_HISTORY_MAX);
  touch(vault);
}

/** The history entry `id` of the page's vault (`locked` / `not_found`). */
function generatedEntry(args: Record<string, unknown>): GeneratedPassword {
  const vault = pageVault(args);
  const entry = vault.data.generatorHistory.find((g) => g.id === str(args, "id"));
  if (!entry) fail("not_found");
  return entry;
}

function presetOptions(id: string, user: GeneratorPreset[]): GeneratorOptions | undefined {
  return (builtinPresets().find((p) => p.id === id) ?? user.find((p) => p.id === id))?.options;
}

function generatorPresets(): GeneratorPresets {
  const s = getState();
  const user = s.settings.generatorPresets;
  const defaultId = presetOptions(s.settings.generatorDefault, user) ? s.settings.generatorDefault : "standard";
  return clone({
    builtin: builtinPresets(),
    user,
    defaultId,
    defaultOptions: presetOptions(defaultId, user) ?? { ...DEFAULT_GENERATOR_OPTIONS },
  });
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
    copy.tags = normalizeTagsLossy(copy.tags);
    copy.folderId = copy.folderId ? folderMap.get(copy.folderId) ?? null : null;
    vault.data.items.push(copy);
  }
  touch(vault);
  return items.length;
}

// Import with preview (`analyze_import` → `commit_import`), see demo/importFiles.ts.

interface MockPendingImport {
  id: string;
  vaultId: string;
  expires: number;
  file: MockImportFile;
  plan: MockPlan;
}

let pendingImport: MockPendingImport | null = null;
let importSeq = 0;
/** Like the backend's slot ticket: only the newest analysis may store its plan. */
let importTicket = 0;

async function analyzeImport(args: Record<string, unknown>): Promise<unknown> {
  const s = getState();
  const path = str(args, "path");
  const password = optStr(args, "password") || null;
  if (!path.trim()) fail("invalid_input:path_required");
  pendingImport = null;
  const ticket = ++importTicket;
  const vault = openVault();
  if (optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
  await sleep(450);
  const file = mockImportFile(path, Date.now());
  if ("error" in file) fail(file.error);
  const fileName = path.split(/[\\/]/).pop() ?? path;
  if (file.needsPassword && !password) {
    return { importId: null, fileName, format: file.format, needsPassword: true, preview: null };
  }
  if (file.needsPassword) {
    await sleep(500);
    if (password !== DEMO_PASSWORD) fail("wrong_password");
  }
  if (s.unlockedId !== vault.info.id) fail("locked");
  // Overtaken by a newer analysis (another file dropped meanwhile).
  if (ticket !== importTicket) fail("not_found");
  const plan = planMockImport(vault.data.items, file.items);
  importSeq += 1;
  pendingImport = { id: `import-${importSeq}`, vaultId: vault.info.id, expires: Date.now() + 15 * 60_000, file, plan };
  return {
    importId: pendingImport.id,
    fileName,
    format: file.format,
    needsPassword: file.needsPassword,
    preview: clone(mockPreview(plan, file)),
  };
}

async function commitImport(args: Record<string, unknown>): Promise<ImportReport> {
  const id = str(args, "importId");
  const mode = args.conflictMode;
  if (mode !== "skip" && mode !== "update" && mode !== "keepBoth") fail("invalid_input:conflictMode");
  const pending = pendingImport && pendingImport.id === id && pendingImport.expires > Date.now() ? pendingImport : null;
  pendingImport = null;
  if (!pending) fail("not_found");
  const vault = openVault();
  if (optStr(args, "pageVaultId") !== vault.info.id || pending.vaultId !== vault.info.id) fail("locked");
  await sleep(400);
  // Classified again: the vault may have changed since the preview.
  const now = planMockImport(vault.data.items, [...pending.plan.newItems, ...pending.plan.conflicts.map((c) => c.item)]);
  const duplicates = [...pending.plan.duplicates, ...now.duplicates];
  const report: ImportReport = {
    imported: 0,
    updated: 0,
    skipped: pending.file.invalid,
    duplicates: duplicates.slice(0, MOCK_LIST_LIMIT),
    duplicateCount: duplicates.length,
    conflictsSkipped: [],
    warnings: pending.file.warnings.slice(0, MOCK_LIST_LIMIT),
    warningCount: pending.file.warnings.length,
  };
  report.imported += importItems(vault, now.newItems);
  for (const { conflict, item } of now.conflicts) {
    const existing = vault.data.items.find((i) => i.id === conflict.existingId);
    if (mode === "keepBoth") {
      report.imported += importItems(vault, [item]);
    } else if (mode === "update" && existing?.login && item.login) {
      saveItem({ ...existing, login: { ...existing.login, password: item.login.password } });
      report.updated += 1;
    } else {
      report.conflictsSkipped.push(describe(item, existing ?? null));
    }
  }
  if (report.imported + report.updated > 0) touch(vault);
  emit("vault://changed", {});
  return report;
}

/** Paths for the simulated drag & drop (default: a Chrome password export). */
function dropPaths(path?: string | string[]): string[] {
  if (Array.isArray(path)) return path;
  return [path ?? "C:\\Users\\Demo\\Downloads\\Chrome-Passwörter.csv"];
}

// ---------------------------------------------------------------------------
// Cloud-folder sync (phone, see the header comment)
// ---------------------------------------------------------------------------

function sampleFor(id: string, now: number): SampleVault {
  return id === WORK_VAULT_ID ? workVault(now) : privateVault(now);
}

function syncResult(id: string, revision: number, outcome: SyncFileResult["outcome"], passwordChanged: boolean): SyncFileResult {
  const name = getStateOrNull()?.vaults.get(id)?.info.name ?? sampleFor(id, Date.now()).name;
  return {
    fileName: `${id}.keystead`,
    vaultId: id,
    vaultName: name,
    revision,
    outcome,
    error: null,
    attachmentsCopied: 0,
    attachmentsFailed: 0,
    passwordChanged,
  };
}

function folderStatus(): FolderStatus {
  const sync = getState().sync;
  return {
    linked: sync.link !== null,
    writable: sync.link !== null,
    syncing: sync.syncing,
    folderName: sync.link?.folderName ?? null,
    linkedAt: sync.link?.linkedAt ?? null,
    lastSyncAt: sync.link ? sync.lastSyncAt : null,
    lastError: sync.link ? sync.lastError : null,
    files: sync.link ? clone(sync.files) : [],
  };
}

function emitSyncStatus(): void {
  emit("sync://status", folderStatus());
}

/** The PC saved a vault (and maybe changed its master password): the folder's copy gets newer. */
function mockPcSave(newPassword: boolean): void {
  const s = getState();
  const id = s.unlockedId && s.sync.folder.has(s.unlockedId) ? s.unlockedId : PRIVATE_VAULT_ID;
  const copy = s.sync.folder.get(id);
  if (!copy) return;
  copy.revision += 1;
  copy.newNotes += 1;
  if (newPassword) copy.password = "demo2";
}

/** One sync of the linked folder: copy-in only, like `folder_sync::sync_folder`. */
async function mockSyncFolder(): Promise<FolderStatus> {
  const s = getState();
  const sync = s.sync;
  if (!sync.link) fail("unsupported:folder_not_linked");
  sync.syncing = true;
  emitSyncStatus();
  await sleep(900);
  sync.syncing = false;
  lastMockSyncEnd = Date.now();
  if (sync.lost) {
    sync.lastError = "unsupported:folder_access_lost";
    emitSyncStatus();
    fail(sync.lastError);
  }
  const now = Date.now();
  const previous = sync.files;
  const files: SyncFileResult[] = [];
  let lockedForKey = false;
  let changedOpen = false;
  for (const [id, copy] of sync.folder) {
    const phoneRevision = sync.phone.get(id);
    let outcome: SyncFileResult["outcome"];
    let passwordChanged = false;
    let attachments = 0;
    if (phoneRevision === undefined || !s.vaults.has(id)) {
      const vault = vaultFromSample(sampleFor(id, now), id, now);
      vault.password = copy.password;
      s.vaults.set(id, vault);
      if (id === PRIVATE_VAULT_ID) seedSampleIcons(id, vault.data.items);
      attachments = mockAttachmentsOfVault(id);
      outcome = "imported";
    } else if (copy.revision > phoneRevision) {
      const vault = s.vaults.get(id);
      if (vault) {
        passwordChanged = vault.password !== copy.password;
        vault.password = copy.password;
        // Like `device_unlock::forget_if_stale`: the sealed KEK opens nothing any more.
        if (passwordChanged) mockInvalidateDeviceUnlock(id);
        for (let n = 0; n < copy.newNotes; n += 1) {
          vault.data.items.push({
            id: uuid(),
            type: "note",
            name: "Neu vom PC",
            folderId: null,
            favorite: false,
            notes: "Diese Notiz kam mit der Synchronisierung aus dem Cloud-Ordner.",
            tags: [],
            login: null,
            card: null,
            identity: null,
            fields: [],
            passwordHistory: [],
            createdAt: now,
            updatedAt: now,
            deletedAt: null,
          });
        }
        copy.newNotes = 0;
        touch(vault);
        if (s.unlockedId === id) {
          if (passwordChanged) lockedForKey = true;
          else changedOpen = true;
        }
      }
      outcome = "updated";
    } else if (copy.revision < phoneRevision) {
      outcome = "pcOlder";
    } else {
      outcome = "upToDate";
      passwordChanged = previous.some((f) => f.vaultId === id && f.revision === copy.revision && f.passwordChanged);
    }
    if (outcome !== "pcOlder") sync.phone.set(id, copy.revision);
    files.push({ ...syncResult(id, copy.revision, outcome, passwordChanged), attachmentsCopied: attachments });
  }
  sync.files = files;
  sync.lastSyncAt = now;
  sync.lastError = null;
  if (lockedForKey) lock("keyChanged");
  else if (changedOpen) emit("vault://changed", {});
  emitSyncStatus();
  return folderStatus();
}

/** Attachments the sample vault comes with (copied in with it). */
function mockAttachmentsOfVault(id: string): number {
  const vault = getState().vaults.get(id);
  if (!vault) return 0;
  return vault.data.items.reduce((sum, item) => sum + mockAttachmentsOf(id, item.id).length, 0);
}

function getStateOrNull(): MockState | null {
  return state;
}

// ---------------------------------------------------------------------------
// Fingerprint unlock (phone, see the header comment)
// ---------------------------------------------------------------------------

/** Like `device_unlock::status`: off → no setup kept; setups that no longer fit are reset. */
function deviceUnlockStatus(): DeviceUnlockStatus {
  const s = getState();
  const du = s.deviceUnlock;
  if (!s.settings.deviceUnlock) du.sealed.clear();
  for (const [id, password] of [...du.sealed]) {
    const vault = s.vaults.get(id);
    if (!vault) du.sealed.delete(id);
    else if (vault.password !== password) mockInvalidateDeviceUnlock(id);
  }
  return {
    available: du.unavailable === null,
    unavailableReason: du.unavailable,
    enabled: s.settings.deviceUnlock,
    vaults: [...du.sealed.keys()].sort(),
    invalidated: [...du.invalidated].sort(),
  };
}

/** The setup of `id` no longer opens its vault: removed and remembered as reset. */
function mockInvalidateDeviceUnlock(id: string): void {
  const du = getState().deviceUnlock;
  if (!du.sealed.delete(id)) return;
  du.stale.delete(id);
  du.invalidated.add(id);
}

/** The system's fingerprint prompt: the user "touches the sensor" after ~0.9 s (see `setFingerprintResult`). */
async function mockFingerprintPrompt(): Promise<void> {
  await sleep(900);
  const du = getState().deviceUnlock;
  const result = du.next;
  du.next = "ok";
  if (result === "cancel") fail("unsupported:device_unlock_cancelled");
  if (result === "fail") fail("unsupported:device_unlock_failed");
  if (result === "lockout") fail("unsupported:device_unlock_lockout");
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

    case "list_items": {
      // Without secrets, like the backend (see demo/redact.ts).
      const vault = openVault();
      return vault.data.items.map((item) => entryOf(vault, item));
    }

    case "list_folders":
      return clone(openVault().data.folders).sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));

    case "save_item":
      return entryOf(openVault(), saveItem(args.item as VaultItem));

    case "set_favorite": {
      const favorite = bool(args, "favorite");
      return entryOf(openVault(), saveItem({ ...pageItem(args), favorite }));
    }

    case "add_attachment": {
      const vault = pageVault(args);
      const itemId = str(args, "itemId");
      // The backend reads the file first; the demo only has its path.
      await sleep(300);
      mockAddAttachment(vault.info.id, vault.data.items, itemId, str(args, "path"), s.appInfo.dataDir);
      touch(vault);
      s.lastActivity = Date.now();
      return entryOf(vault, findItem(itemId));
    }

    case "remove_attachment": {
      const vault = pageVault(args);
      const itemId = mockRemoveAttachment(vault.info.id, str(args, "attachmentId"));
      touch(vault);
      s.lastActivity = Date.now();
      return entryOf(vault, findItem(itemId));
    }

    case "save_attachment": {
      const vault = pageVault(args);
      await sleep(300);
      mockSaveAttachment(vault.info.id, str(args, "attachmentId"), str(args, "path"), s.appInfo.dataDir);
      s.lastActivity = Date.now();
      return null;
    }

    case "reveal_attachment": {
      const vault = pageVault(args);
      await sleep(250);
      s.lastActivity = Date.now();
      return mockRevealAttachment(vault.info.id, str(args, "attachmentId"));
    }

    case "reveal_secret": {
      const field = fieldArg(args);
      return secretValue(pageItem(args), field);
    }

    case "copy_secret_field": {
      const field = fieldArg(args);
      let value = await secretValue(pageItem(args), field);
      if (field === "cardNumber") value = value.replace(/\s+/g, "");
      if (!value) fail("not_found");
      await copyText(value, true);
      return null;
    }

    case "totp_for_item":
      return secretTotp(pageItem(args));

    case "get_item_for_edit":
      return clone(pageItem(args));

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
      // Like the core on save: attachments of items that no longer exist go too.
      mockPruneAttachments(vault.info.id, vault.data.items);
      touch(vault);
      return null;
    }

    case "empty_trash": {
      const vault = openVault();
      const before = vault.data.items.length;
      vault.data.items = vault.data.items.filter((i) => i.deletedAt === null);
      mockPruneAttachments(vault.info.id, vault.data.items);
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

    case "rename_tag": {
      const vault = openVault();
      if (optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
      const to = validateTag(str(args, "to"));
      const from = str(args, "from").trim();
      // Like the core: no change, no new revision (and no event either way).
      const changed = from ? retagItems(vault.data.items, from, to) : 0;
      if (changed > 0) touch(vault);
      return changed;
    }

    case "delete_tag": {
      const vault = openVault();
      if (optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
      const tag = str(args, "tag").trim();
      const changed = tag ? retagItems(vault.data.items, tag, null) : 0;
      if (changed > 0) touch(vault);
      return changed;
    }

    case "generate_password": {
      const options = parseOptions(args.options);
      const password = generatorCall(() => generate(options));
      // `remember: true` is kept for compatibility (the page sends false): into the page's vault only.
      const vault = s.unlockedId ? s.vaults.get(s.unlockedId) : undefined;
      if (args.remember === true && vault && optStr(args, "pageVaultId") === vault.info.id) {
        addGenerated(vault, password);
      }
      s.lastActivity = Date.now();
      return password;
    }

    case "remember_generated": {
      const password = str(args, "password");
      if (!password) fail("invalid_input:password_empty");
      if (Array.from(password).length > 1024) fail("invalid_input:password_too_long");
      const vault = pageVault(args);
      // The same password as the newest entry is one use (copied twice, or copied and taken).
      if (vault.data.generatorHistory[0]?.password !== password) addGenerated(vault, password);
      s.lastActivity = Date.now();
      return null;
    }

    case "generator_history":
      // Redacted like the backend: never a password.
      return openVault().data.generatorHistory.map(({ id, createdAt }): GeneratedPasswordInfo => ({ id, createdAt }));

    case "reveal_generated": {
      const entry = generatedEntry(args);
      await sleep(120);
      s.lastActivity = Date.now();
      return entry.password;
    }

    case "copy_generated": {
      const entry = generatedEntry(args);
      await copyText(entry.password, true);
      s.lastActivity = Date.now();
      return null;
    }

    case "clear_generator_history":
      pageVault(args).data.generatorHistory = [];
      return null;

    case "generator_presets":
      return generatorPresets();

    case "save_generator_preset": {
      const id = optStr(args, "id");
      const options = parseOptions(args.options);
      const user = s.settings.generatorPresets;
      if (id !== null && BUILTIN_IDS.has(id)) fail("invalid_input:preset_id");
      if (id !== null && !user.some((p) => p.id === id)) fail("not_found");
      if (id === null && user.length >= GENERATOR_PRESETS_MAX) fail("invalid_input:presets_too_many");
      const name = generatorCall(() => validatePresetName(str(args, "name")));
      const normalized = generatorCall(() => normalizeOptions(options));
      const presetId = id ?? uuid();
      const lower = name.toLowerCase();
      if (user.some((p) => p.id !== presetId && p.name.toLowerCase() === lower)) fail("invalid_input:preset_name_taken");
      const preset: GeneratorPreset = { id: presetId, name, options: normalized };
      s.settings.generatorPresets = id === null ? [...user, preset] : user.map((p) => (p.id === id ? preset : p));
      return { id: presetId, presets: generatorPresets() };
    }

    case "delete_generator_preset": {
      const id = str(args, "id");
      if (BUILTIN_IDS.has(id)) fail("invalid_input:preset_id");
      if (!s.settings.generatorPresets.some((p) => p.id === id)) fail("not_found");
      s.settings.generatorPresets = s.settings.generatorPresets.filter((p) => p.id !== id);
      // A deleted default falls back to "standard" (`Settings::normalized`).
      if (s.settings.generatorDefault === id) s.settings.generatorDefault = "standard";
      return generatorPresets();
    }

    case "set_default_generator_preset": {
      const id = str(args, "id");
      if (!BUILTIN_IDS.has(id) && !s.settings.generatorPresets.some((p) => p.id === id)) fail("not_found");
      s.settings.generatorDefault = id;
      return generatorPresets();
    }

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

    // Leaked passwords (see "Leaked-password check" in docs/ARCHITECTURE.md): the
    // real app asks Have I Been Pwned (~150 ms per request); the demo answers
    // from a table after about a second. Both count as user activity.
    case "check_pwned_password": {
      s.lastActivity = Date.now();
      const password = str(args, "password");
      await sleep(900);
      return mockBreachCount(password);
    }

    case "check_pwned_vault": {
      s.lastActivity = Date.now();
      // Like the backend: the candidates are taken first, the requests run
      // without the lock, and a vault locked or switched meanwhile discards
      // the result (`locked`).
      const report = mockPwnedReport(pageVault(args).data);
      await sleep(1_000);
      pageVault(args);
      return report;
    }

    case "change_master_password": {
      const vault = openVault();
      const current = str(args, "current");
      const next = str(args, "newPassword");
      await sleep(400);
      if (current !== vault.password) fail("wrong_password");
      if (!next) fail("invalid_input:password_empty");
      vault.password = next;
      // Like the backend: the vault key is rotated, so an existing recovery
      // key is replaced by a new one.
      if (vault.recoveryKey) vault.recoveryKey = makeRecoveryKey();
      touch(vault);
      return { newRecoveryKey: vault.recoveryKey };
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
      mockDeleteAttachments(id);
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
      // The older one-step import (the UI uses analyze_import / commit_import).
      let report: Pick<ImportReport, "imported" | "skipped" | "warnings">;
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
      return {
        updated: 0,
        duplicates: [],
        duplicateCount: 0,
        conflictsSkipped: [],
        warningCount: report.warnings.length,
        ...report,
      } satisfies ImportReport;
    }

    case "analyze_import":
      return analyzeImport(args);

    case "commit_import":
      return commitImport(args);

    case "cancel_import":
      if (pendingImport?.id === str(args, "importId")) pendingImport = null;
      return null;

    case "show_export":
      return null;

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

    case "get_icons": {
      const vault = openVault();
      if (optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
      return mockGetIcons(vault.info.id, vault.data.items);
    }

    case "clear_icons": {
      const vault = openVault();
      if (optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
      return mockClearIcons(vault.info.id);
    }

    case "get_settings":
      return clone(s.settings);

    case "save_settings": {
      // Only the device-unlock commands change `deviceUnlock` (like `from_the_page`);
      // `validateSettings` keeps the generator presets.
      const next = { ...validateSettings(args.settings, s.settings), deviceUnlock: s.settings.deviceUnlock };
      const folderChanged = next.mirrorFolder !== s.settings.mirrorFolder;
      // The phone ignores the field (no mirror there).
      if (folderChanged && next.mirrorFolder && !s.appInfo.readOnly) checkMirrorFolder(next.mirrorFolder);
      s.settings = next;
      if (folderChanged && !s.appInfo.readOnly) {
        s.mirror.lastMirroredAt = null;
        s.mirror.lastError = null;
        emit("mirror://status", mirrorStatus());
        scheduleMirrorPass();
      }
      return clone(s.settings);
    }

    case "mirror_status":
      return mirrorStatus();

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
    case "open_extension_dir":
      return null;

    case "check_update": {
      await sleep(600);
      const info = mockUpdateInfo();
      mockPendingUpdate = info.available ? info : null;
      return clone(info);
    }

    case "pending_update":
      return mockPendingUpdate ? clone(mockPendingUpdate) : null;

    case "install_update":
      return installMockUpdate();

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

    case "folder_status":
      return folderStatus();

    case "link_folder": {
      // The system picker; the user picks the PC's folder in the cloud app.
      await sleep(700);
      s.sync.link = { folderName: "Keystead", linkedAt: Date.now() };
      s.sync.lost = false;
      s.sync.lastError = null;
      s.lastActivity = Date.now();
      try {
        await mockSyncFolder();
      } catch {
        /* recorded in lastError */
      }
      return folderStatus();
    }

    case "sync_now":
      s.lastActivity = Date.now();
      return mockSyncFolder();

    case "unlink_folder":
      s.lastActivity = Date.now();
      s.sync.link = null;
      s.sync.files = [];
      s.sync.lastSyncAt = null;
      s.sync.lastError = null;
      emitSyncStatus();
      return folderStatus();

    case "device_unlock_status":
      return deviceUnlockStatus();

    case "enable_device_unlock": {
      const vault = s.unlockedId ? s.vaults.get(s.unlockedId) : undefined;
      if (!vault || optStr(args, "pageVaultId") !== vault.info.id) fail("locked");
      const du = s.deviceUnlock;
      if (du.unavailable === "notEnrolled") fail("unsupported:device_unlock_not_enrolled");
      if (du.unavailable) fail("unsupported:device_unlock_unavailable");
      s.lastActivity = Date.now();
      await mockFingerprintPrompt();
      du.sealed.set(vault.info.id, vault.password);
      du.invalidated.delete(vault.info.id);
      du.stale.delete(vault.info.id);
      s.settings.deviceUnlock = true;
      return deviceUnlockStatus();
    }

    case "disable_device_unlock": {
      const du = s.deviceUnlock;
      const id = optStr(args, "vaultId");
      if (id) {
        du.sealed.delete(id);
        du.invalidated.delete(id);
      } else {
        du.sealed.clear();
        du.invalidated.clear();
      }
      if (du.sealed.size === 0) s.settings.deviceUnlock = false;
      s.lastActivity = Date.now();
      return deviceUnlockStatus();
    }

    case "unlock_with_device": {
      const id = str(args, "vaultId");
      const du = s.deviceUnlock;
      if (!s.settings.deviceUnlock || !du.sealed.has(id)) fail("unsupported:device_unlock_not_set");
      const vault = s.vaults.get(id);
      if (!vault) {
        du.sealed.delete(id);
        fail("not_found");
      }
      // A new master password: no prompt for a KEK that cannot open the vault.
      if (du.sealed.get(id) !== vault.password) {
        mockInvalidateDeviceUnlock(id);
        fail("unsupported:device_unlock_invalid");
      }
      await mockFingerprintPrompt();
      if (du.stale.has(id)) {
        mockInvalidateDeviceUnlock(id);
        fail("unsupported:device_unlock_invalid");
      }
      s.unlockedId = id;
      s.lastActivity = Date.now();
      return clone(vault.info);
    }

    default:
      fail(`unsupported:unknown command ${command}`);
  }
}

/**
 * The commands the phone app registers (`gui.rs`, "On the phone" in
 * docs/ARCHITECTURE.md) – with `?platform=android` every other one fails,
 * so a phone screen that still reaches a desktop command shows up here.
 */
const PHONE_COMMANDS = new Set([
  "app_info",
  "list_vaults",
  "session_state",
  "unlock_vault",
  "lock_vault",
  "touch_activity",
  "list_items",
  "list_folders",
  "reveal_secret",
  "copy_secret_field",
  "totp_for_item",
  "reveal_attachment",
  "password_strength",
  "copy_text",
  "health_report",
  "get_icons",
  "get_settings",
  "save_settings",
  "folder_status",
  "link_folder",
  "sync_now",
  "unlink_folder",
  "device_unlock_status",
  "enable_device_unlock",
  "disable_device_unlock",
  "unlock_with_device",
]);

export async function mockInvoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
  // Simulate IPC latency so loading states are exercised.
  await sleep(20 + Math.random() * 60);
  if (getState().appInfo.readOnly && !PHONE_COMMANDS.has(command)) {
    // Tauri rejects a command its handler does not know with a plain message.
    console.warn(`[mock] the phone app has no command ${command}`);
    fail(`Command ${command} not found`);
  }
  const result = (await dispatch(command, args)) as T;
  // The phone has no icon fetcher (no network); it shows stored icons only.
  if (ICON_FETCH_TRIGGERS.has(command) && !getState().appInfo.readOnly) scheduleIconFetch();
  if (MIRROR_TRIGGERS.has(command)) scheduleMirrorPass();
  return result;
}

/** Commands after which the real app may load website icons. */
const ICON_FETCH_TRIGGERS = new Set([
  "unlock_vault",
  "unlock_with_recovery",
  "create_vault",
  "save_item",
  "set_favorite",
  "restore_item",
  "save_settings",
  "commit_import",
  "import_data",
  "clear_icons",
]);

/** The background icon fetch: sample icons for new hosts after ~1.5 s. */
function scheduleIconFetch(): void {
  window.setTimeout(() => {
    const s = getState();
    const vault = s.unlockedId ? s.vaults.get(s.unlockedId) : undefined;
    if (!vault || !s.settings.websiteIcons) return;
    if (mockFetchIcons(vault.info.id, vault.data.items)) emit("vault://icons", {});
  }, 1_500);
}

export async function mockPickOpenFile(filters?: { name: string; extensions: string[] }[]): Promise<string | null> {
  await sleep(150);
  const ext = filters?.[0]?.extensions[0] ?? "csv";
  const names: Record<string, string> = {
    json: "bitwarden_export.json",
    csv: "Chrome-Passwörter.csv",
    keystead: "Keystead-Export.keystead",
  };
  return `C:\\Users\\Demo\\Downloads\\${names[ext] ?? `import.${ext}`}`;
}

export async function mockPickFolder(): Promise<string | null> {
  await sleep(150);
  return MOCK_MIRROR_FOLDER;
}

export async function mockPickSaveFile(defaultPath?: string): Promise<string | null> {
  await sleep(150);
  return `C:\\Users\\Demo\\Documents\\${defaultPath ?? "Keystead-Export"}`;
}
