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

/** An item as the item list holds it: `VaultItem` without its secrets. */
export interface ItemListEntry {
  id: string;
  type: ItemType;
  name: string;
  folderId: string | null;
  favorite: boolean;
  notes: string;
  login: LoginListData | null;
  card: CardListData | null;
  identity: IdentityData | null;
  fields: CustomFieldListEntry[];
  /** Newest first, like `VaultItem.passwordHistory`. */
  passwordHistory: PasswordHistoryListEntry[];
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

export interface GeneratedPassword {
  password: string;
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
  platform: "windows" | "linux" | "macos";
  extensionId: string;
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

export interface GeneratorOptions {
  kind: GeneratorKind;
  length: number;
  uppercase: boolean;
  lowercase: boolean;
  digits: boolean;
  symbols: boolean;
  minDigits: number;
  minSymbols: number;
  avoidAmbiguous: boolean;
  words: number;
  separator: string;
  capitalize: boolean;
  includeNumber: boolean;
}

export const DEFAULT_GENERATOR_OPTIONS: GeneratorOptions = {
  kind: "password",
  length: 20,
  uppercase: true,
  lowercase: true,
  digits: true,
  symbols: true,
  minDigits: 1,
  minSymbols: 1,
  avoidAmbiguous: false,
  words: 5,
  separator: "-",
  capitalize: true,
  includeNumber: true,
};

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
  /** "beta" = test versions and stable releases (default during the beta phase), "stable" = stable releases only. */
  updateChannel: UpdateChannel;
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

export type LockReason = "manual" | "timeout" | "system";

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
