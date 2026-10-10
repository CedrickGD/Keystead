// Typed wrappers around the Tauri commands and events defined in
// docs/ARCHITECTURE.md. Outside of Tauri (plain browser, `npm run dev`,
// automated screenshots) every call is routed to the in-memory mock backend
// in ./mock.ts instead.

import type {
  AppInfo,
  BrowserId,
  BrowserStatus,
  ExportFormat,
  Folder,
  GeneratedPassword,
  GeneratorOptions,
  HealthReport,
  ConflictMode,
  ImportAnalysis,
  ImportFormat,
  ImportReport,
  ItemListEntry,
  LegacyVaultInfo,
  LockReason,
  MasterPasswordChanged,
  PairingRequest,
  SecretField,
  SessionState,
  Settings,
  Strength,
  TotpCode,
  UpdateInfo,
  UpdateProgress,
  VaultInfo,
  VaultItem,
} from "./types";

/** True when running inside the Tauri webview. */
export const IN_TAURI: boolean = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** True when the in-memory demo backend is used instead of Tauri. */
export const IS_MOCK = !IN_TAURI;

/**
 * Error thrown by every API call. `code` is the stable backend error code
 * (`wrong_password`, `locked`, `not_found`, `conflict`, `invalid_input`,
 * `io`, `corrupt`, `unsupported`, or `unknown`), `detail` the text after the
 * first colon, if any.
 */
export class ApiError extends Error {
  readonly code: string;
  readonly detail: string;

  constructor(raw: string) {
    super(raw);
    this.name = "ApiError";
    const idx = raw.indexOf(":");
    this.code = idx >= 0 ? raw.slice(0, idx) : raw;
    this.detail = idx >= 0 ? raw.slice(idx + 1).trim() : "";
  }
}

function toApiError(err: unknown): ApiError {
  if (err instanceof ApiError) return err;
  if (typeof err === "string") return new ApiError(err);
  if (err instanceof Error) return new ApiError(`unknown:${err.message}`);
  try {
    return new ApiError(`unknown:${JSON.stringify(err)}`);
  } catch {
    return new ApiError("unknown");
  }
}

type Args = Record<string, unknown>;

let callsInFlight = 0;

/** Number of commands that have not answered yet. */
export function pendingCalls(): number {
  return callsInFlight;
}

// The vault this page works on: the one it shows, or the one it has just
// created or unlocked itself (the setup wizard imports into a new vault before
// showing it). Sent as `pageVaultId` with every command that changes or
// exports the open vault: if the browser extension has switched to another
// vault and this page has not reloaded yet, the backend answers `locked`
// instead of applying an edit meant for the old vault to the new one.
let pageVaultId: string | null = null;

/** Sets the vault this page works on (see `pageVaultId`). */
export function setPageVault(id: string): void {
  pageVaultId = id;
}

/** The vault this page works on (see `pageVaultId`), if any. */
export function getPageVault(): string | null {
  return pageVaultId;
}

/** A vault this page opened itself: it works on that one from now on. */
async function opened(info: Promise<VaultInfo>): Promise<VaultInfo> {
  const vault = await info;
  pageVaultId = vault.id;
  return vault;
}

async function call<T>(command: string, args?: Args): Promise<T> {
  callsInFlight += 1;
  try {
    if (IN_TAURI) {
      const { invoke } = await import("@tauri-apps/api/core");
      return await invoke<T>(command, args);
    }
    const mock = await import("./mock");
    return await mock.mockInvoke<T>(command, args ?? {});
  } catch (err) {
    throw toApiError(err);
  } finally {
    callsInFlight -= 1;
  }
}

// ---------------------------------------------------------------------------
// Commands (names and argument shapes exactly as in docs/ARCHITECTURE.md)
// ---------------------------------------------------------------------------

export const api = {
  appInfo: () => call<AppInfo>("app_info"),
  listVaults: () => call<VaultInfo[]>("list_vaults"),
  sessionState: () => call<SessionState>("session_state"),
  createVault: (name: string, masterPassword: string) =>
    opened(call<VaultInfo>("create_vault", { name, masterPassword })),
  unlockVault: (vaultId: string, masterPassword: string) =>
    opened(call<VaultInfo>("unlock_vault", { vaultId, masterPassword })),
  unlockWithRecovery: (vaultId: string, recoveryKey: string, newMasterPassword: string) =>
    opened(call<VaultInfo>("unlock_with_recovery", { vaultId, recoveryKey, newMasterPassword })),
  lockVault: () => call<null>("lock_vault"),
  touchActivity: () => call<null>("touch_activity"),

  /** All items incl. trash, without their secrets (see `ItemListEntry`). */
  listItems: () => call<ItemListEntry[]>("list_items"),
  listFolders: () => call<Folder[]>("list_folders"),
  /** Saves a full item (from `getItemForEdit` or a new one); answers the redacted entry. */
  saveItem: (item: VaultItem) => call<ItemListEntry>("save_item", { item, pageVaultId }),
  setFavorite: (itemId: string, favorite: boolean) =>
    call<ItemListEntry>("set_favorite", { itemId, favorite, pageVaultId }),
  /** One secret, for an eye toggle; keep it in component state only. */
  revealSecret: (itemId: string, field: SecretField) => call<string>("reveal_secret", { itemId, field, pageVaultId }),
  /** Copies a secret as sensitive in the backend; the value never reaches the page. */
  copySecretField: (itemId: string, field: SecretField) =>
    call<null>("copy_secret_field", { itemId, field, pageVaultId }),
  /** The current TOTP code of a login (never its key). */
  totpForItem: (itemId: string) => call<TotpCode>("totp_for_item", { itemId, pageVaultId }),
  /** The full item, only for the editor (drop it after save / cancel). */
  getItemForEdit: (itemId: string) => call<VaultItem>("get_item_for_edit", { itemId, pageVaultId }),
  trashItem: (id: string) => call<null>("trash_item", { id, pageVaultId }),
  restoreItem: (id: string) => call<null>("restore_item", { id, pageVaultId }),
  deleteItem: (id: string) => call<null>("delete_item", { id, pageVaultId }),
  emptyTrash: () => call<number>("empty_trash", { pageVaultId }),
  saveFolder: (folder: Folder) => call<Folder>("save_folder", { folder, pageVaultId }),
  deleteFolder: (id: string) => call<null>("delete_folder", { id, pageVaultId }),

  generatePassword: (options: GeneratorOptions, remember: boolean) =>
    call<string>("generate_password", { options, remember, pageVaultId }),
  generatorHistory: () => call<GeneratedPassword[]>("generator_history"),
  clearGeneratorHistory: () => call<null>("clear_generator_history", { pageVaultId }),
  passwordStrength: (password: string) => call<Strength>("password_strength", { password }),
  totpCode: (seed: string) => call<TotpCode>("totp_code", { seed }),
  copyText: (text: string, sensitive: boolean) => call<null>("copy_text", { text, sensitive }),
  healthReport: () => call<HealthReport>("health_report"),

  /** Also replaces an existing recovery key: show `newRecoveryKey` once. */
  changeMasterPassword: (current: string, newPassword: string) =>
    call<MasterPasswordChanged>("change_master_password", { current, newPassword, pageVaultId }),
  createRecoveryKey: () => call<string>("create_recovery_key", { pageVaultId }),
  removeRecoveryKey: () => call<null>("remove_recovery_key", { pageVaultId }),
  renameVault: (name: string) => call<VaultInfo>("rename_vault", { name, pageVaultId }),
  deleteVault: (vaultId: string, masterPassword: string) =>
    call<null>("delete_vault", { vaultId, masterPassword }),

  legacyScan: () => call<LegacyVaultInfo[]>("legacy_scan"),
  importData: (format: ImportFormat, path: string, password: string | null) =>
    call<ImportReport>("import_data", { format, path, password, pageVaultId }),
  exportData: (format: ExportFormat, path: string, password: string | null, masterPassword: string) =>
    call<null>("export_data", { format, path, password, masterPassword, pageVaultId }),
  /**
   * Recognises an import file by its content and compares it with the open
   * vault. `preview: null` = the file is encrypted: ask for its password and
   * call again. The plan waits in the backend (15 min) for `commitImport`.
   */
  analyzeImport: (path: string, password: string | null) =>
    call<ImportAnalysis>("analyze_import", { path, password, pageVaultId }),
  /** Imports an analysed file (`not_found` once the plan expired, was replaced or the vault was locked). */
  commitImport: (importId: string, conflictMode: ConflictMode) =>
    call<ImportReport>("commit_import", { importId, conflictMode, pageVaultId }),
  cancelImport: (importId: string) => call<null>("cancel_import", { importId }),
  /** Shows the file the last export wrote in the file manager. */
  showExport: () => call<null>("show_export"),

  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),

  /** Stored website icons of the page's vault: { [host]: "data:image/png;base64,…" } (see lib/icons.ts). */
  getIcons: () => call<Record<string, string>>("get_icons", { pageVaultId }),
  /** Deletes the stored website icons; returns how many. */
  clearIcons: () => call<number>("clear_icons", { pageVaultId }),

  browserStatus: () => call<BrowserStatus>("browser_status"),
  registerBrowsers: (browsers: BrowserId[]) => call<BrowserStatus>("register_browsers", { browsers }),
  unregisterBrowsers: (browsers: BrowserId[]) => call<BrowserStatus>("unregister_browsers", { browsers }),
  revokeClient: (clientId: string) => call<BrowserStatus>("revoke_client", { clientId }),
  respondPairing: (requestId: string, approve: boolean) =>
    call<null>("respond_pairing", { requestId, approve }),

  openTerminal: () => call<null>("open_terminal"),
  openDataDir: () => call<null>("open_data_dir"),
  /** Opens the folder the app keeps the browser extension in. */
  openExtensionDir: () => call<null>("open_extension_dir"),
  setPortableMode: (enabled: boolean) => call<AppInfo>("set_portable_mode", { enabled }),

  /** Checks the configured update channel now (network errors → `io:<detail>`). */
  checkUpdate: () => call<UpdateInfo>("check_update"),
  /** The update the last (background) check found, without a network request. */
  pendingUpdate: () => call<UpdateInfo | null>("pending_update"),
  /**
   * Downloads, verifies and installs the update, locks the vault and restarts
   * the app. Progress via `update://progress` / `update://ready`; only
   * answers on failure (or once the restart is under way).
   */
  installUpdate: () => call<null>("install_update"),
};

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export type Unlisten = () => void;

export interface LockedPayload {
  reason: LockReason;
}

async function subscribe<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
  if (IN_TAURI) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<T>(event, (e) => handler(e.payload));
  }
  const mock = await import("./mock");
  return mock.mockListen(event, (payload) => handler(payload as T));
}

export const events = {
  onVaultLocked: (handler: (payload: LockedPayload) => void) =>
    subscribe<LockedPayload>("vault://locked", handler),
  onVaultChanged: (handler: () => void) => subscribe<Record<string, never>>("vault://changed", () => handler()),
  /** New website icons were stored (the background fetcher): re-read them. */
  onIconsChanged: (handler: () => void) => subscribe<Record<string, never>>("vault://icons", () => handler()),
  /** The vault was unlocked outside the UI (browser extension). */
  onVaultUnlocked: (handler: (vault: VaultInfo) => void) => subscribe<VaultInfo>("vault://unlocked", handler),
  onPairingRequest: (handler: (payload: PairingRequest) => void) =>
    subscribe<PairingRequest>("bridge://pairing-request", handler),
  /** A pending pairing request ended without an answer from the UI (cancelled in the browser, timed out). */
  onPairingClosed: (handler: (payload: { requestId: string }) => void) =>
    subscribe<{ requestId: string }>("bridge://pairing-closed", handler),
  onUnlockRequest: (handler: () => void) => subscribe<Record<string, never>>("bridge://unlock-request", () => handler()),
  /** The background check found a version not announced before. */
  onUpdateAvailable: (handler: (info: UpdateInfo) => void) => subscribe<UpdateInfo>("update://available", handler),
  onUpdateProgress: (handler: (progress: UpdateProgress) => void) =>
    subscribe<UpdateProgress>("update://progress", handler),
  /** Downloaded and verified; the app installs it and restarts. */
  onUpdateReady: (handler: (payload: { version: string }) => void) =>
    subscribe<{ version: string }>("update://ready", handler),
  /**
   * Files dragged over / dropped onto the window (Tauri's webview drag & drop
   * events; the mock backend simulates them via `window.__keysteadMock`).
   */
  onFileDrag: async (handler: (event: FileDragEvent) => void): Promise<Unlisten> => {
    if (IN_TAURI) {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      return getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload;
        handler(p.type === "enter" || p.type === "drop" ? { type: p.type, paths: p.paths } : { type: p.type });
      });
    }
    const mock = await import("./mock");
    return mock.mockListen("mock://drag-drop", (payload) => handler(payload as FileDragEvent));
  },
};

/** Drag & drop of files onto the window. */
export type FileDragEvent = { type: "enter" | "drop"; paths: string[] } | { type: "over" | "leave" };

/**
 * Subscribe from a React effect: returns a synchronous cleanup that also
 * works when the async subscription has not resolved yet.
 */
export function subscribeEffect(subscription: Promise<Unlisten>): () => void {
  let disposed = false;
  let unlisten: Unlisten | null = null;
  subscription
    .then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(() => {
      /* event system unavailable – nothing to clean up */
    });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

// ---------------------------------------------------------------------------
// Native file dialogs
// ---------------------------------------------------------------------------

export interface FileFilter {
  name: string;
  extensions: string[];
}

/** Lets the user pick an existing file. Returns null when cancelled. */
export async function pickOpenFile(options: { title?: string; filters?: FileFilter[] } = {}): Promise<string | null> {
  if (IN_TAURI) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const result = await open({ multiple: false, directory: false, title: options.title, filters: options.filters });
    return typeof result === "string" ? result : null;
  }
  const mock = await import("./mock");
  return mock.mockPickOpenFile(options.filters);
}

/** Lets the user choose a target file. Returns null when cancelled. */
export async function pickSaveFile(
  options: { title?: string; defaultPath?: string; filters?: FileFilter[] } = {},
): Promise<string | null> {
  if (IN_TAURI) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const result = await save({ title: options.title, defaultPath: options.defaultPath, filters: options.filters });
    return result ?? null;
  }
  const mock = await import("./mock");
  return mock.mockPickSaveFile(options.defaultPath);
}

/** Opens a website in the system browser. Only http(s) URLs are allowed. */
export function openExternal(url: string): boolean {
  const normalized = normalizeUrl(url);
  if (!normalized) return false;
  window.open(normalized, "_blank", "noopener,noreferrer");
  return true;
}

/** Adds https:// to bare hostnames; returns null for non-web URLs. */
export function normalizeUrl(raw: string): string | null {
  const value = raw.trim();
  if (!value) return null;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `https://${value}`;
  try {
    const parsed = new URL(withScheme);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    return parsed.toString();
  } catch {
    return null;
  }
}
