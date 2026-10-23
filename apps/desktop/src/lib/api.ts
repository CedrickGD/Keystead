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
  FolderConflictCopy,
  FolderOffer,
  FolderStatus,
  GeneratedPasswordInfo,
  GeneratorOptions,
  GeneratorPresets,
  HealthReport,
  ConflictMode,
  DeviceUnlockPrompt,
  DeviceUnlockStatus,
  ImportAnalysis,
  ImportFormat,
  ImportReport,
  ItemListEntry,
  LegacyVaultInfo,
  LockReason,
  MasterPasswordChanged,
  MirrorStatus,
  PairingRequest,
  PwnedReport,
  SavedGeneratorPreset,
  SecretField,
  SessionState,
  Settings,
  SyncConflict,
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
  /** Renames a tag on every item incl. trash (case-insensitive); answers the number of changed items. No event: refetch `listItems`. */
  renameTag: (from: string, to: string) => call<number>("rename_tag", { from, to, pageVaultId }),
  /** Removes a tag from every item incl. trash (case-insensitive); answers the number of changed items. No event: refetch `listItems`. */
  deleteTag: (tag: string) => call<number>("delete_tag", { tag, pageVaultId }),

  // Attachments (see "Attachments" in docs/ARCHITECTURE.md). The file bytes
  // never reach the page: the backend reads the picked file and writes the
  // saved one itself; only an image preview comes back, as a data URL.
  /** Attaches the file at `path` (from `pickAttachmentFile`) to the item; answers the item's new entry. */
  addAttachment: (itemId: string, path: string) =>
    call<ItemListEntry>("add_attachment", { itemId, path, pageVaultId }),
  /** Writes the decrypted file to `path` (from `pickSaveFile`); `showExport` reveals it afterwards. */
  saveAttachment: (attachmentId: string, path: string) =>
    call<null>("save_attachment", { attachmentId, path, pageVaultId }),
  /** Deletes an attachment; answers the item's new entry. */
  removeAttachment: (attachmentId: string) => call<ItemListEntry>("remove_attachment", { attachmentId, pageVaultId }),
  /**
   * A `data:image/…` URL of an image attachment (PNG, JPEG, GIF, WebP up to
   * 8 MiB; anything else → `unsupported:attachment_preview`). Treat it like a
   * revealed secret: component state only, see `useRevealedAttachment`.
   */
  revealAttachment: (attachmentId: string) => call<string>("reveal_attachment", { attachmentId, pageVaultId }),

  // Generator (see "Secrets and the webview → Generator"). Only on the user's
  // click; a preview is not stored – a password the user uses (copies, takes
  // into an item) is reported with `rememberGenerated`. The history comes
  // back redacted; one entry is revealed or copied by id.
  /** A preview for `options` – nothing is stored. Keep it in component state only. */
  generatePreview: (options: GeneratorOptions) =>
    call<string>("generate_password", { options, remember: false, pageVaultId }),
  /** The user used a preview: into the page's vault's history (the newest entry is not repeated). */
  rememberGenerated: (password: string) => call<null>("remember_generated", { password, pageVaultId }),
  /** The history, newest first – ids and times only, never a password. */
  generatorHistory: () => call<GeneratedPasswordInfo[]>("generator_history"),
  /** One history entry's password, for an eye toggle (`useRevealedValue`). */
  revealGenerated: (id: string) => call<string>("reveal_generated", { id, pageVaultId }),
  /** Copies one history entry as sensitive in the backend; the value never reaches the page. */
  copyGenerated: (id: string) => call<null>("copy_generated", { id, pageVaultId }),
  clearGeneratorHistory: () => call<null>("clear_generator_history", { pageVaultId }),
  /** Built-in and user presets, the default preset and its options. */
  generatorPresets: () => call<GeneratorPresets>("generator_presets"),
  /** `id` null: a new user preset; otherwise replaces name and options of that user preset. */
  saveGeneratorPreset: (id: string | null, name: string, options: GeneratorOptions) =>
    call<SavedGeneratorPreset>("save_generator_preset", { id, name, options }),
  deleteGeneratorPreset: (id: string) => call<GeneratorPresets>("delete_generator_preset", { id }),
  setDefaultGeneratorPreset: (id: string) => call<GeneratorPresets>("set_default_generator_preset", { id }),
  passwordStrength: (password: string) => call<Strength>("password_strength", { password }),
  totpCode: (seed: string) => call<TotpCode>("totp_code", { seed }),
  copyText: (text: string, sensitive: boolean) => call<null>("copy_text", { text, sensitive }),
  healthReport: () => call<HealthReport>("health_report"),
  /**
   * Breach count of one password at Have I Been Pwned (0 = not in known
   * breaches), only on the user's button press: the backend hashes it and
   * only the first 5 hex characters of the SHA-1 leave the app (see "Leaked
   * passwords"). Network errors → `io:<detail>`, a proxy setup the app
   * cannot evaluate → `unsupported:pwned_proxy`.
   */
  checkPwnedPassword: (password: string) => call<number>("check_pwned_password", { password }),
  /**
   * Checks every distinct login password of the page's vault the same way
   * (≤ 4 requests at a time); `locked` when another vault is open or the
   * vault was locked / switched while the requests ran. Nothing is cached or
   * written into the vault.
   */
  checkPwnedVault: () => call<PwnedReport>("check_pwned_vault", { pageVaultId }),

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
  /**
   * Desktop: the cloud-folder mirror ("Kopie im Cloud-Ordner aktuell halten", `Settings.mirrorFolder`) –
   * the folder, the last successful copy and the last error. Changes come as `mirror://status`.
   */
  mirrorStatus: () => call<MirrorStatus>("mirror_status"),
  /** Desktop: the vaults in the mirror folder the PC does not have (as the last scan found them). */
  folderOffers: () => call<FolderOffer[]>("folder_offers"),
  /** Desktop: takes an offered vault from the mirror folder (on trial: it opens with its own master password). */
  adoptFolderVault: (vaultId: string) => call<VaultInfo>("adopt_folder_vault", { vaultId }),
  /** Desktop: the vault is no longer offered. */
  dismissFolderVault: (vaultId: string) => call<null>("dismiss_folder_vault", { vaultId }),
  /** Desktop: the phone's conflict copies in the mirror folder (as the last scan found them). */
  folderConflictCopies: () => call<FolderConflictCopy[]>("folder_conflict_copies"),
  /** Desktop: copies one into the data folder (`conflicts/`) and returns its path for the import. */
  restoreFolderConflictCopy: (fileName: string) => call<string>("restore_folder_conflict_copy", { fileName }),
  /** Desktop: the conflict copy is no longer listed (the file in the folder stays). */
  dismissFolderConflictCopy: (fileName: string) => call<null>("dismiss_folder_conflict_copy", { fileName }),

  // Sync conflicts – both platforms ("local" = this side, "remote" = the folder's copy).
  /** The checked conflicts of the page's open vault. */
  syncConflicts: () => call<SyncConflict[]>("sync_conflicts", { pageVaultId }),
  /**
   * Keeps one side of the open vault's conflict – the two writes `conflict`
   * names (`unsupported:sync_conflict_changed` if it is about others by now);
   * the other becomes a conflict copy in `<data>/conflicts/` (the phone also
   * puts it into the cloud folder). `remote` may lock the vault (`keyChanged`).
   */
  resolveSyncConflict: (conflict: SyncConflict, keep: "local" | "remote") =>
    call<VaultInfo>("resolve_sync_conflict", {
      vaultId: conflict.vaultId,
      pageVaultId,
      keep,
      localDigest: conflict.localDigest,
      remoteDigest: conflict.remoteDigest,
    }),

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

  // Cloud-folder sync – phone only (see "Cloud-folder sync (phone)" in
  // ARCHITECTURE.md). The phone reads its vaults from a folder of the user's
  // own cloud; it never writes there.
  /** The linked folder and what the last sync found (no access to the folder). */
  folderStatus: () => call<FolderStatus>("folder_status"),
  /**
   * Opens the system folder picker, keeps read access to the choice, links it
   * and syncs. `null` when the picker was closed without a choice; a failed
   * first sync is in `lastError`.
   */
  linkFolder: () => call<FolderStatus | null>("link_folder"),
  /** Syncs now; rejects with the sync's error (also recorded in `lastError`). */
  syncNow: () => call<FolderStatus>("sync_now"),
  /** Forgets the folder; the vault copies stay on the phone. */
  unlinkFolder: () => call<FolderStatus>("unlink_folder"),

  // Fingerprint unlock – phone only (see "Fingerprint unlock (phone)" in
  // ARCHITECTURE.md). The master KEK is sealed by the Android Keystore; the
  // page only ever sees this status.
  /** Whether the device can use the fingerprint and which vaults open with it (no prompt). */
  deviceUnlockStatus: () => call<DeviceUnlockStatus>("device_unlock_status"),
  /** Sets up fingerprint unlock for the open vault (the system prompt confirms it). */
  enableDeviceUnlock: (prompt: DeviceUnlockPrompt) =>
    call<DeviceUnlockStatus>("enable_device_unlock", { prompt, pageVaultId }),
  /** Removes it for one vault, or for all (`null`); the setting goes off with the last one. */
  disableDeviceUnlock: (vaultId: string | null) => call<DeviceUnlockStatus>("disable_device_unlock", { vaultId }),
  /** Unlocks with the fingerprint (the system prompt); errors `unsupported:device_unlock_*`. */
  unlockWithDevice: (vaultId: string, prompt: DeviceUnlockPrompt) =>
    opened(call<VaultInfo>("unlock_with_device", { vaultId, prompt })),

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
  /** Desktop: the cloud-folder mirror copied a vault, failed, or its folder changed. */
  onMirrorStatus: (handler: (status: MirrorStatus) => void) => subscribe<MirrorStatus>("mirror://status", handler),
  /** Desktop: the vaults the mirror folder offers changed. */
  onMirrorOffers: (handler: (offers: FolderOffer[]) => void) =>
    subscribe<FolderOffer[]>("mirror://offers", handler),
  onMirrorConflictCopies: (handler: (copies: FolderConflictCopy[]) => void) =>
    subscribe<FolderConflictCopy[]>("mirror://conflictCopies", handler),
  /** Both platforms: the open vault's checked sync conflicts changed (`[]`: none). */
  onSyncConflict: (handler: (conflicts: SyncConflict[]) => void) =>
    subscribe<SyncConflict[]>("sync://conflict", handler),
  /** Phone: the cloud-folder sync started or ended, or the folder was (un)linked. */
  onSyncStatus: (handler: (status: FolderStatus) => void) => subscribe<FolderStatus>("sync://status", handler),
  /** Phone: the app left the foreground (Android `onPause`); revealed values are hidden. */
  onSuspended: (handler: () => void) => subscribe<Record<string, never>>("app://suspended", () => handler()),
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

/** Lets the user pick a folder (desktop: the cloud folder for the vault copies). Returns null when cancelled. */
export async function pickFolder(options: { title?: string; defaultPath?: string } = {}): Promise<string | null> {
  if (IN_TAURI) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const result = await open({ multiple: false, directory: true, title: options.title, defaultPath: options.defaultPath });
    return typeof result === "string" ? result : null;
  }
  const mock = await import("./mock");
  return mock.mockPickFolder();
}

/** Lets the user pick a file to attach to an item (any type). Returns null when cancelled. */
export async function pickAttachmentFile(title: string): Promise<string | null> {
  if (IN_TAURI) return pickOpenFile({ title });
  const mock = await import("./mock");
  return mock.mockPickAttachmentFile();
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
