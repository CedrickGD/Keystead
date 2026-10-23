// Mirrors crates/keystead-core/src/model.rs and docs/ARCHITECTURE.md.
// Keep both sides in sync.

export type ItemType = "login" | "card" | "identity" | "note";
export type UriMatch = "domain" | "host" | "startsWith" | "exact" | "never";
export type FieldKind = "text" | "hidden" | "boolean";

export interface LoginUri {
  uri: string;
  match: UriMatch;
}

export interface LoginData {
  username: string;
  password: string;
  uris: LoginUri[];
  /** base32 secret or otpauth:// URI, "" = none */
  totp: string;
  passwordRevisedAt: number | null;
}

export interface CardData {
  cardholderName: string;
  brand: string;
  number: string;
  expMonth: string;
  expYear: string;
  code: string;
}

export interface IdentityData {
  title: string;
  firstName: string;
  lastName: string;
  email: string;
  phone: string;
  company: string;
  address1: string;
  address2: string;
  postalCode: string;
  city: string;
  state: string;
  country: string;
  username: string;
}

export interface CustomField {
  name: string;
  value: string;
  kind: FieldKind;
}

export interface PasswordHistoryEntry {
  password: string;
  replacedAt: number;
}

export interface VaultItem {
  /** "" for a new item – the backend assigns the id */
  id: string;
  type: ItemType;
  name: string;
  folderId: string | null;
  favorite: boolean;
  notes: string;
  /** Free labels such as "Bank"; the backend normalises them (see "Tags" in ARCHITECTURE.md). */
  tags: string[];
  login: LoginData | null;
  card: CardData | null;
  identity: IdentityData | null;
  fields: CustomField[];
  passwordHistory: PasswordHistoryEntry[];
  createdAt: number;
  updatedAt: number;
  deletedAt: number | null;
}

// ---------------------------------------------------------------------------
// Redacted items (`list_items`, `save_item`, `set_favorite`), see "Secrets and
// the webview" in docs/ARCHITECTURE.md. Mirrors `ItemListEntry` in
// apps/desktop/src-tauri/src/secrets.rs. A secret reaches the UI only through
// `reveal_secret` (one value) and `get_item_for_edit` (the full `VaultItem`).
// ---------------------------------------------------------------------------

/** A login without its password and TOTP key. */
export interface LoginListData {
  username: string;
  /** Always "": the password stays in the backend. */
  password: "";
  hasPassword: boolean;
  uris: LoginUri[];
  /** Always "": the TOTP key stays in the backend (`totp_for_item` gives the code). */
  totp: "";
  hasTotp: boolean;
  passwordRevisedAt: number | null;
}

/** A card without its number and security code. */
export interface CardListData {
  cardholderName: string;
  brand: string;
  /** "•••• 1234" (the last 4 digits of a number with at least 8), "••••" for a shorter one, "" for none. */
  number: string;
  hasNumber: boolean;
  expMonth: string;
  expYear: string;
  /** Always "". */
  code: "";
  hasCode: boolean;
}

export interface CustomFieldListEntry {
  name: string;
  /** "" for `hidden` fields (`hasValue` says whether there is one). */
  value: string;
  kind: FieldKind;
  hasValue: boolean;
}

/** An old password: only when it was replaced (`reveal_secret` { history: index } gives it). */
export interface PasswordHistoryListEntry {
  replacedAt: number;
}

/**
 * A file attached to an item (see "Attachments" in docs/ARCHITECTURE.md):
 * name, size and date only. The bytes stay in the backend – `save_attachment`
 * writes them to a path of the user's choice, `reveal_attachment` answers a
 * data URL of an image for the in-app preview (kept in component state like a
 * revealed secret).
 */
export interface AttachmentListEntry {
  id: string;
  name: string;
  /** Bytes of the original file. */
  size: number;
  createdAt: number;
}

/** Limits of `keystead_core::attachments` (the backend enforces them; the UI shows them). */
export const ATTACHMENT_MAX_BYTES = 20 * 1024 * 1024;
export const ATTACHMENTS_PER_ITEM_MAX = 10;

/** An item as the item list holds it: `VaultItem` without its secrets. */
export interface ItemListEntry {
  id: string;
  type: ItemType;
  name: string;
  folderId: string | null;
  favorite: boolean;
  notes: string;
  /** Not secret: as stored. */
  tags: string[];
  login: LoginListData | null;
  card: CardListData | null;
  identity: IdentityData | null;
  fields: CustomFieldListEntry[];
  /** Newest first, like `VaultItem.passwordHistory`. */
  passwordHistory: PasswordHistoryListEntry[];
  /** Oldest first. Not part of `VaultItem`: attachments act on the saved item, never through the editor. */
  attachments: AttachmentListEntry[];
  createdAt: number;
  updatedAt: number;
  deletedAt: number | null;
}

/**
 * One secret of an item (`reveal_secret`, `copy_secret_field`): "totp" is the
 * stored key, "totpCode" the current code; `custom` / `history` index
 * `fields` / `passwordHistory`.
 */
export type SecretField =
  | "password"
  | "totp"
  | "totpCode"
  | "cardNumber"
  | "cardCode"
  | { custom: number }
  | { history: number };

export interface Folder {
  id: string;
  name: string;
}

/** A generator history entry inside the vault (only the mock backend holds these). */
export interface GeneratedPassword {
  /** UUID v4; entries of older vaults get one when the vault is opened. */
  id: string;
  password: string;
  createdAt: number;
}

/**
 * A generator history entry as the page sees it (`generator_history`):
 * never the password – `reveal_generated` / `copy_generated` address it by id.
 */
export interface GeneratedPasswordInfo {
  id: string;
  createdAt: number;
}

/** Decrypted vault payload (only used by the in-memory mock backend). */
export interface VaultData {
  items: VaultItem[];
  folders: Folder[];
  generatorHistory: GeneratedPassword[];
  /** Website icons by host (`lib/icons.ts`); the UI reads them via `get_icons`. */
  icons?: Record<string, IconEntry>;
}

/** A stored website icon (inside the encrypted vault). */
export interface IconEntry {
  /** Base64 of a 64×64 PNG; null until a fetch succeeded. */
  png: string | null;
  /** Unix ms of the last fetch attempt. */
  fetchedAt: number;
  /** Unix ms of the last failed attempt; null after a success. */
  failedAt: number | null;
}

export interface VaultInfo {
  id: string;
  name: string;
  path: string;
  createdAt: number;
  updatedAt: number;
  hasRecoveryKey: boolean;
}

export interface ItemSummary {
  id: string;
  type: ItemType;
  name: string;
  subtitle: string;
  uri: string;
  favorite: boolean;
  hasTotp: boolean;
  folderId: string | null;
}

export interface AppInfo {
  version: string;
  dataDir: string;
  portable: boolean;
  platform: "windows" | "linux" | "macos" | "android" | "ios";
  /** The browser extension's id; `null` on the phone (no browser integration there). */
  extensionId: string | null;
  /** The vault cannot be changed from this app: the UI hides every editing control (`false` on both platforms since stage 2b). */
  readOnly: boolean;
  /** Attachments cannot be added, saved or removed (the phone: previews only). */
  attachmentsReadOnly: boolean;
}

export interface SessionState {
  unlocked: boolean;
  vault: VaultInfo | null;
}

/**
 * `change_master_password`: the vault key is rotated, so an existing recovery
 * key is replaced – the new one (show it once) or `null` without recovery key.
 */
export interface MasterPasswordChanged {
  newRecoveryKey: string | null;
}

export type GeneratorKind = "password" | "passphrase";

/** Passphrase word list: "en" = EFF large list, "de" = German list (dys2p, see the core's wordlist/README.md). */
export type Wordlist = "en" | "de";

/** `keystead_core::generator::GeneratorOptions` (see docs/ARCHITECTURE.md, "Other core modules"). */
export interface GeneratorOptions {
  kind: GeneratorKind;
  /** Password length, `PASSWORD_MIN_LEN`..`PASSWORD_MAX_LEN`. */
  length: number;
  uppercase: boolean;
  lowercase: boolean;
  digits: boolean;
  symbols: boolean;
  /** Minimum counts per class, each 0..length; only count while their class is on; sum ≤ length. */
  minUppercase: number;
  minLowercase: number;
  minDigits: number;
  minSymbols: number;
  /** The symbols to draw from: printable ASCII, no letter, digit or space; not empty while `symbols` is on. */
  symbolSet: string;
  /** Characters removed from every class. */
  exclude: string;
  /** Exclude I l 1 O 0 (on top of `exclude`). */
  avoidAmbiguous: boolean;
  /** The first character is a letter. */
  startWithLetter: boolean;
  /** Passphrase word count, 3..20. */
  words: number;
  separator: string;
  capitalize: boolean;
  includeNumber: boolean;
  wordlist: Wordlist;
}

/** The core's `DEFAULT_SYMBOLS`. */
export const DEFAULT_SYMBOLS = "!#$%&*+-=?@^_";

export const DEFAULT_GENERATOR_OPTIONS: GeneratorOptions = {
  kind: "password",
  length: 20,
  uppercase: true,
  lowercase: true,
  digits: true,
  symbols: true,
  minUppercase: 1,
  minLowercase: 1,
  minDigits: 1,
  minSymbols: 1,
  symbolSet: DEFAULT_SYMBOLS,
  exclude: "",
  avoidAmbiguous: false,
  startWithLetter: false,
  words: 5,
  separator: "-",
  capitalize: true,
  includeNumber: true,
  wordlist: "en",
};

/** Ids of the built-in generator presets, in display order (the core's `BUILTIN_PRESET_IDS`). */
export const BUILTIN_PRESET_IDS = ["standard", "alnum", "pin", "long", "passphrase"] as const;
export type BuiltinPresetId = (typeof BUILTIN_PRESET_IDS)[number];

/** Named generator options: built-in (names translated by id) or the user's (`settings.generatorPresets`). */
export interface GeneratorPreset {
  id: string;
  name: string;
  options: GeneratorOptions;
}

/** Answer of `generator_presets` and of the preset commands. */
export interface GeneratorPresets {
  /** The built-in presets in display order; `name` is an English fallback – translate by id. */
  builtin: GeneratorPreset[];
  /** The user's presets, in their order. */
  user: GeneratorPreset[];
  /** Id of the default preset (built-in or user). */
  defaultId: string;
  /** Its options: what the editor's generator and the browser extension use. */
  defaultOptions: GeneratorOptions;
}

/** Answer of `save_generator_preset`. */
export interface SavedGeneratorPreset {
  id: string;
  presets: GeneratorPresets;
}

export interface Strength {
  /** 0..4 */
  score: number;
  crackTime: string;
  warning: string;
  suggestions: string[];
}

export interface TotpCode {
  code: string;
  period: number;
  remaining: number;
}

export interface HealthReport {
  totalLogins: number;
  weak: string[];
  reused: string[][];
  old: string[];
  missingTotpCount: number;
  /** 0..100 */
  score: number;
}

/** One row of `PwnedReport.leaked`. */
export interface LeakedItem {
  itemId: string;
  /** How many known data breaches hold the item's password. */
  count: number;
}

/**
 * Result of `check_pwned_vault` (see "Leaked-password check" in
 * docs/ARCHITECTURE.md): only on the user's button press, never stored –
 * the UI keeps it in page memory (`state/breaches.tsx`).
 */
export interface PwnedReport {
  /** When the check ran (Unix ms). */
  checkedAt: number;
  /** Distinct passwords of the non-trashed logins the check covered (empty ones skipped). */
  checked: number;
  /** Items whose password appears in known breaches, by count desc then item id; an item at most once. */
  leaked: LeakedItem[];
  /** Distinct passwords whose request failed after others had succeeded: not known either way. */
  failed: number;
}

/** An item of an import file that is not added as a new item, and the vault item it matches (never a password). */
export interface ImportMatch {
  incomingName: string;
  /** Login username; for other types the list subtitle (card "•••• 1234", identity name/e-mail, "" for notes). */
  username: string;
  /** Login host without www. ("" without URI and for other types). */
  site: string;
  itemType: ItemType;
  /** "" = the file itself contains this item twice (`existingName` = the earlier entry's name). */
  existingId: string;
  existingName: string;
}

/** A login whose site and username exist with a different password ("password") or 2FA key ("totp"). */
export interface ImportConflict extends ImportMatch {
  conflictId: string;
  reason: "password" | "totp";
}

/** Secret-free summary of an analysed import file (`analyze_import`). `duplicates` and `warnings` hold at most 100 entries; the counts are complete. `conflicts` is complete. */
export interface ImportPreview {
  newCount: number;
  duplicates: ImportMatch[];
  duplicateCount: number;
  conflicts: ImportConflict[];
  /** Rows/entries of the file that could not be read (see `warnings`). */
  invalid: number;
  /** English, technical. */
  warnings: string[];
  warningCount: number;
}

/** Answer of `analyze_import`. `preview` null (and `importId` null) = ask for the file's password and analyse again. */
export interface ImportAnalysis {
  importId: string | null;
  fileName: string;
  format: ImportFormat;
  /** Encrypted format (VaultX 1.x vault, Keystead export). */
  needsPassword: boolean;
  preview: ImportPreview | null;
}

/** What `commit_import` does with conflicts. */
export type ConflictMode = "skip" | "update" | "keepBoth";

export interface ImportReport {
  imported: number;
  /** Existing logins whose password was taken over from the file (`update`). */
  updated: number;
  /** Invalid rows/entries of the file. */
  skipped: number;
  /** Not imported: already in the vault (or twice in the file) – at most 100; `duplicateCount` counts all. */
  duplicates: ImportMatch[];
  duplicateCount: number;
  /** Not imported: conflicts that were skipped. */
  conflictsSkipped: ImportMatch[];
  /** At most 100; `warningCount` counts all. */
  warnings: string[];
  warningCount: number;
}

export interface LegacyVaultInfo {
  name: string;
  path: string;
}

export type ImportFormat = "legacy" | "csv" | "bitwarden_json" | "keystead";
export type ExportFormat = "keystead" | "csv" | "bitwarden_json";

export type ThemeSetting = "system" | "light" | "dark";
export type Language = "de" | "en";

export interface Settings {
  theme: ThemeSetting;
  language: Language;
  autoLockMinutes: number;
  lockOnSystemLock: boolean;
  clipboardClearSeconds: number;
  minimizeToTray: boolean;
  startInTray: boolean;
  browserIntegration: boolean;
  lastVaultId: string | null;
  /** Load missing website icons in the background, directly from the websites. Default true; off = no icon requests (stored icons stay visible). */
  websiteIcons: boolean;
  /** Look for app updates in the background (at start and every 6 h). Default true. */
  updateCheck: boolean;
  /** "stable" = stable releases only (default), "beta" = test versions and stable releases. The mock starts on "beta" so the update demos work. */
  updateChannel: UpdateChannel;
  /** Mobile: lock the open vault this many seconds after the app went to the background. null = never, 0 = immediately; default 60. The desktop ignores it. */
  lockInBackgroundSeconds: number | null;
  /**
   * Mobile: unlock with the fingerprint (opt-in). Default false. Only the device-unlock commands change it
   * (`save_settings` keeps the stored value). The desktop ignores it.
   */
  deviceUnlock: boolean;
  /**
   * Desktop: "Kopie im Cloud-Ordner aktuell halten" – the folder (absolute path, from the folder
   * dialog) in which the app keeps an encrypted copy of every vault opened in it, for the phone.
   * null = off (default). The phone ignores it.
   */
  mirrorFolder: string | null;
  /**
   * The user's generator presets and the default preset's id. Changed only by
   * the preset commands (`save_generator_preset` …); `save_settings` keeps the
   * stored values, so read them from `generator_presets`, not from here.
   */
  generatorPresets: GeneratorPreset[];
  generatorDefault: string;
}

export type UpdateChannel = "beta" | "stable";

/** Result of `check_update`, payload of `update://available`. */
export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  /** The newest version on the channel (null if none could be determined). */
  version: string | null;
  /** Release notes (Markdown/plain text) of that version. */
  notes: string | null;
  /** Release date (RFC 3339). */
  date: string | null;
  /** False for the portable build (and Linux/macOS except an AppImage): offer `releaseUrl` instead. */
  canInstall: boolean;
  /** Release page in the system browser. */
  releaseUrl: string;
}

/** Payload of `update://progress`. */
export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

export type BrowserId = "chrome" | "edge" | "brave" | "chromium" | "vivaldi";

export interface BrowserInfo {
  id: BrowserId;
  name: string;
  detected: boolean;
  registered: boolean;
}

export interface PairedClient {
  id: string;
  name: string;
  createdAt: number;
  lastSeenAt: number;
}

export interface BrowserStatus {
  serverRunning: boolean;
  extensionId: string;
  /** The folder the app keeps the browser extension in (load it unpacked from there). */
  extensionDir: string;
  /** Version of the extension the app delivers. */
  extensionVersion: string;
  browsers: BrowserInfo[];
  clients: PairedClient[];
}

export interface PairingRequest {
  requestId: string;
  clientName: string;
  code: string;
}

/** Payload of `vault://locked`; `background` only on the phone (`lockInBackgroundSeconds`). */
export type LockReason = "manual" | "timeout" | "system" | "background" | "keyChanged";

/**
 * What the phone's last cloud-folder sync did with one vault file of the
 * folder ("Cloud-folder sync (phone)" in ARCHITECTURE.md).
 */
export type SyncOutcome =
  | "imported"
  | "updated"
  | "upToDate"
  | "pcOlder"
  | "otherVault"
  | "misnamed"
  | "unreadable"
  | "tooLarge"
  | "failed"
  | "missing"
  | "rejected"
  /** Stage 2b: the phone's copy was written to the folder. */
  | "pushed"
  /** Both the phone's and the folder's copy changed: nothing written (`sync_conflicts`). */
  | "conflict"
  /** The phone's copy is newer or new, but the link is read-only (pick the folder again). */
  | "notWritable"
  /** No folder linked: the vault is only on the phone. */
  | "localOnly";

export interface SyncFileResult {
  fileName: string;
  vaultId: string | null;
  vaultName: string | null;
  /** The folder copy's revision (after a push: the pushed one; for a vault the folder lacks: the phone's). */
  revision: number | null;
  outcome: SyncOutcome;
  /** The error code behind `unreadable` / `failed`. */
  error: string | null;
  attachmentsCopied: number;
  attachmentsFailed: number;
  /** The master password was changed on the PC (the old one no longer opens this copy; for a `conflict`: the folder's copy has another one). */
  passwordChanged: boolean;
}

/** The phone's linked cloud folder (`folder_status`, `sync://status`). No secrets. */
export interface FolderStatus {
  linked: boolean;
  /** The app may write the linked folder (false for a read-only link or none). */
  writable: boolean;
  syncing: boolean;
  folderName: string | null;
  linkedAt: number | null;
  /** The last sync that could read the folder (ms). */
  lastSyncAt: number | null;
  /** Error code of the last failed sync, e.g. `unsupported:folder_access_lost`. */
  lastError: string | null;
  files: SyncFileResult[];
}

/**
 * A conflict of the two-way sync (`sync_conflicts`, `sync://conflict`, both
 * platforms): this side's copy ("local") and the folder's ("remote") both
 * changed, or nothing proves that one descends from the other. No secrets.
 * `lostAttachments`: how many attachments only this side's copy has (keeping
 * the folder's drops them); `remoteOnlyAttachments`: only the folder's copy
 * has (keeping this side's drops them) – omitted when 0 or unknown.
 * `localDigest` / `remoteDigest` name the two writes; `resolve_sync_conflict`
 * gets them back and refuses (`unsupported:sync_conflict_changed`) if the
 * conflict is about other writes by then.
 */
export interface SyncConflict {
  vaultId: string;
  vaultName: string;
  localRevision: number;
  remoteRevision: number;
  localUpdatedAt: number;
  remoteUpdatedAt: number;
  /** The folder's copy has another master password: this side's state cannot be kept. */
  passwordChanged: boolean;
  lostAttachments?: number;
  remoteOnlyAttachments?: number;
  localDigest: string;
  remoteDigest: string;
}

/**
 * Something an unlock did with a synced copy that the user has to know about
 * (`sync://notice`, both platforms; shown as a toast). No secrets.
 * `keptAside`: this device's previous copy (another master password, edits the
 * new copy does not descend from) was kept as a conflict copy – on the phone
 * also in the cloud folder. `keptOwn`: the folder's copy needs another master
 * password than the one typed; this device's own state was opened and the
 * folder's copy becomes a conflict.
 */
export interface SyncNotice {
  kind: "keptAside" | "keptOwn";
  vaultId: string;
  vaultName: string;
}

/** The desktop's cloud-folder mirror (`mirror_status`, `mirror://status`). No secrets. */
export interface MirrorStatus {
  /** `Settings.mirrorFolder`; null = off. */
  folder: string | null;
  /** When a pass last reached this folder (ms; since the start or since the folder was set). */
  lastMirroredAt: number | null;
  /** When a pass last took the folder's copy of a vault (the phone's edits; ms, likewise). */
  lastPulledAt: number | null;
  /** Error code of the last attempt (e.g. `unsupported:mirror_folder_missing`), null after a success. */
  lastError: string | null;
}

/**
 * Desktop: a vault in the mirror folder the PC does not have, e.g. made on
 * the phone (`folder_offers`, `mirror://offers`). Never imported on its
 * own: `adopt_folder_vault` takes it, `dismiss_folder_vault` hides it.
 * From the plain header – no secrets.
 */
export interface FolderOffer {
  vaultId: string;
  name: string;
  revision: number;
  updatedAt: number;
}

/**
 * Desktop: a conflict copy the phone wrote into the mirror folder
 * (`<id>.keystead.conflict-<YYYYMMDD-HHMMSS>-phone`, the state it did not
 * keep when it resolved a conflict) – `folder_conflict_copies`,
 * `mirror://conflictCopies`. `restore_folder_conflict_copy` copies it into
 * the data folder for the import, `dismiss_folder_conflict_copy` hides it.
 * From the plain header – no secrets.
 */
export interface FolderConflictCopy {
  fileName: string;
  vaultId: string;
  vaultName: string;
  revision: number;
  updatedAt: number;
}

/**
 * The phone's fingerprint unlock (`device_unlock_status`, "Fingerprint unlock
 * (phone)" in ARCHITECTURE.md). No secrets.
 */
export interface DeviceUnlockStatus {
  /** A strong biometric (fingerprint) can be used on this device right now. */
  available: boolean;
  /** Why not; null when available. */
  unavailableReason: "noHardware" | "notEnrolled" | "other" | null;
  /** `Settings.deviceUnlock` – changed only by the device-unlock commands. */
  enabled: boolean;
  /** The vaults that open with the fingerprint (ids). */
  vaults: string[];
  /**
   * Vaults whose setup was removed since the app started because it no longer
   * opened them (a new fingerprint, a master password changed on the PC, …):
   * the UI explains it and offers to set it up again.
   */
  invalidated: string[];
}

/** The texts of the system's fingerprint prompt (translated by the UI). */
export interface DeviceUnlockPrompt {
  title: string;
  /** The line under the title. */
  reason: string;
  /** The button that closes the prompt. */
  cancel: string;
}

/** Stable error codes returned by the backend (see ARCHITECTURE.md). */
export type ErrorCode =
  | "wrong_password"
  | "locked"
  | "not_found"
  | "conflict"
  | `invalid_input:${string}`
  | `io:${string}`
  | `corrupt:${string}`
  | `unsupported:${string}`;
