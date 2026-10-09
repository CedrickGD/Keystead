import type { CardData, IdentityData, ItemType, VaultItem } from "./types";

// ---------------------------------------------------------------------------
// Avatars
// ---------------------------------------------------------------------------

const AVATAR_HUES = [214, 262, 330, 20, 145, 188, 282, 350, 38, 168, 236, 4];

/** Stable hue (degrees) for a name, used for letter avatars. */
export function avatarHue(name: string): number {
  let hash = 0;
  for (const ch of name.trim().toLowerCase()) {
    hash = (hash * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  }
  return AVATAR_HUES[hash % AVATAR_HUES.length] ?? 214;
}

/** First visible letter or digit of a name, upper-cased ("?" if none). */
export function avatarLetter(name: string): string {
  const match = name.trim().match(/[\p{L}\p{N}]/u);
  return match ? match[0].toLocaleUpperCase() : "?";
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

export function emptyIdentity(): IdentityData {
  return {
    title: "",
    firstName: "",
    lastName: "",
    email: "",
    phone: "",
    company: "",
    address1: "",
    address2: "",
    postalCode: "",
    city: "",
    state: "",
    country: "",
    username: "",
  };
}

export function emptyCard(): CardData {
  return { cardholderName: "", brand: "", number: "", expMonth: "", expYear: "", code: "" };
}

export function newItem(type: ItemType, folderId: string | null): VaultItem {
  return {
    id: "",
    type,
    name: "",
    folderId,
    favorite: false,
    notes: "",
    login: type === "login" ? { username: "", password: "", uris: [{ uri: "", match: "domain" }], totp: "", passwordRevisedAt: null } : null,
    card: type === "card" ? emptyCard() : null,
    identity: type === "identity" ? emptyIdentity() : null,
    fields: [],
    passwordHistory: [],
    createdAt: 0,
    updatedAt: 0,
    deletedAt: null,
  };
}

export function cloneItem(item: VaultItem): VaultItem {
  return JSON.parse(JSON.stringify(item)) as VaultItem;
}

export function itemsEqual(a: VaultItem, b: VaultItem): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function identityFullName(identity: IdentityData | null): string {
  if (!identity) return "";
  return [identity.firstName, identity.lastName].filter((s) => s.trim()).join(" ");
}

export function lastDigits(number: string, count = 4): string {
  const digits = number.replace(/\D/g, "");
  return digits.slice(-count);
}

/** Secret-free one-line description shown under the item name in lists. */
export function itemSubtitle(item: VaultItem): string {
  switch (item.type) {
    case "login": {
      const login = item.login;
      if (!login) return "";
      if (login.username) return login.username;
      const host = hostOf(login.uris[0]?.uri ?? "");
      return host ?? "";
    }
    case "card": {
      const card = item.card;
      if (!card) return "";
      const digits = lastDigits(card.number);
      return [card.brand, digits ? `•••• ${digits}` : ""].filter(Boolean).join(" ");
    }
    case "identity":
      return identityFullName(item.identity) || item.identity?.email || "";
    case "note":
      return "";
  }
}

export function hostOf(uri: string): string | null {
  const value = uri.trim();
  if (!value) return null;
  try {
    const url = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `https://${value}`);
    return url.hostname.replace(/^www\./, "") || null;
  } catch {
    return null;
  }
}

/** Search terms: lower-cased, whitespace separated, empty terms removed. */
export function searchTerms(query: string): string[] {
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter((t) => t.length > 0);
}

/**
 * Mirrors `UnlockedVault::search` in keystead-core: matches name, username,
 * URIs, notes and card brand; every term must match somewhere.
 */
export function matchesSearch(item: VaultItem, terms: string[]): boolean {
  if (terms.length === 0) return true;
  const haystack = [
    item.name,
    item.login?.username ?? "",
    ...(item.login?.uris.map((u) => u.uri) ?? []),
    item.notes,
    item.card?.brand ?? "",
    item.identity ? identityFullName(item.identity) : "",
    item.identity?.email ?? "",
  ]
    .join("\n")
    .toLowerCase();
  return terms.every((term) => haystack.includes(term));
}

export interface TextSegment {
  text: string;
  match: boolean;
}

/** Splits `text` into matching / non-matching segments for highlighting. */
export function highlightSegments(text: string, terms: string[]): TextSegment[] {
  if (!text || terms.length === 0) return [{ text, match: false }];
  const lower = text.toLowerCase();
  const marks = new Array<boolean>(text.length).fill(false);
  for (const term of terms) {
    let from = 0;
    for (;;) {
      const idx = lower.indexOf(term, from);
      if (idx < 0) break;
      for (let i = idx; i < idx + term.length && i < marks.length; i++) marks[i] = true;
      from = idx + Math.max(term.length, 1);
    }
  }
  const segments: TextSegment[] = [];
  let current: TextSegment | null = null;
  for (let i = 0; i < text.length; i++) {
    const match = marks[i] ?? false;
    if (!current || current.match !== match) {
      current = { text: "", match };
      segments.push(current);
    }
    current.text += text.charAt(i);
  }
  return segments;
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

export function detectCardBrand(number: string): string {
  const n = number.replace(/\D/g, "");
  if (!n) return "";
  if (/^4/.test(n)) return "Visa";
  const prefix4 = n.length >= 4 ? Number(n.slice(0, 4)) : 0;
  if (/^5[1-5]/.test(n) || (prefix4 >= 2221 && prefix4 <= 2720)) return "Mastercard";
  if (/^3[47]/.test(n)) return "American Express";
  if (/^(6011|65|64[4-9])/.test(n)) return "Discover";
  if (/^3(0[0-5]|[68])/.test(n)) return "Diners Club";
  if (/^35(2[89]|[3-8])/.test(n)) return "JCB";
  if (/^(5018|5020|5038|5893|6304|6759|676[1-3])/.test(n)) return "Maestro";
  if (/^62/.test(n)) return "UnionPay";
  return "";
}

export const CARD_BRANDS = [
  "Visa",
  "Mastercard",
  "American Express",
  "Maestro",
  "Discover",
  "Diners Club",
  "JCB",
  "UnionPay",
];

/** Groups digits for display (Amex 4-6-5, others blocks of 4). */
export function formatCardNumber(number: string): string {
  const n = number.replace(/\s+/g, "");
  if (!/^\d+$/.test(n)) return number;
  if (/^3[47]/.test(n) && n.length === 15) return `${n.slice(0, 4)} ${n.slice(4, 10)} ${n.slice(10)}`;
  return n.replace(/(.{4})/g, "$1 ").trim();
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

/** Formats a 6-8 digit TOTP code in two halves ("123 456"). */
export function formatTotp(code: string): string {
  if (code.length < 6) return code;
  const half = Math.ceil(code.length / 2);
  return `${code.slice(0, half)} ${code.slice(half)}`;
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function classNames(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

/** Safe localStorage access – storage can be unavailable (private mode, tests). */
export const localPrefs = {
  get(key: string): string | null {
    try {
      return window.localStorage.getItem(`keystead.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string): void {
    try {
      window.localStorage.setItem(`keystead.${key}`, value);
    } catch {
      /* ignore */
    }
  },
};

export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
}
