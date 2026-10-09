// Mirrors crates/vaultx-core/src/model.rs and docs/ARCHITECTURE.md.
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

export interface Folder {
  id: string;
  name: string;
}

export interface GeneratedPassword {
  password: string;
  createdAt: number;
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

export interface ImportReport {
  imported: number;
  skipped: number;
  warnings: string[];
}

export interface LegacyVaultInfo {
  name: string;
  path: string;
}

export type ImportFormat = "legacy" | "csv" | "bitwarden_json" | "vaultx";
export type ExportFormat = "vaultx" | "csv" | "bitwarden_json";

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
  showIcons: boolean;
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
