// The generator's option rules, mirrored from keystead-core's generator.rs
// (docs/ARCHITECTURE.md, "Other core modules" and "Generator"). The page uses
// them to check options inline and to show the entropy while the user edits
// – nothing is generated before the user asks. The backend checks again when
// generating (the same `invalid_input:<code>` codes), and the mock backend
// generates with them.

import { DEFAULT_GENERATOR_OPTIONS, type GeneratorKind, type GeneratorOptions, type Wordlist } from "./types";

export const PASSWORD_MIN_LEN = 4;
export const PASSWORD_MAX_LEN = 128;
export const PASSPHRASE_MIN_WORDS = 3;
export const PASSPHRASE_MAX_WORDS = 20;
export const SEPARATOR_MAX_CHARS = 8;
/** Longest accepted `symbolSet` / `exclude` input (characters, before duplicates are removed). */
export const CHARSET_INPUT_MAX_CHARS = 256;
/** Longest preset name (characters, after trimming). */
export const PRESET_NAME_MAX_CHARS = 40;
/** At most this many user presets (`settings::GENERATOR_PRESETS_MAX`). */
export const GENERATOR_PRESETS_MAX = 50;
/** The vault keeps the newest this many history entries. */
export const GENERATOR_HISTORY_MAX = 50;

/**
 * Real sizes of the embedded word lists (`Wordlist::words().len()` in the
 * core; its tests pin 7776 and 7724). The entropy shown per word uses these.
 */
export const WORDLIST_SIZES: Record<Wordlist, number> = { en: 7776, de: 7724 };

export const UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
export const LOWER = "abcdefghijklmnopqrstuvwxyz";
export const DIGITS = "0123456789";
/** Characters excluded by `avoidAmbiguous`. */
export const AMBIGUOUS = "Il1O0";

/** `invalid_input` codes of the generator, in the order the core checks them. */
export type GeneratorIssue =
  | "length"
  | "symbol_set"
  | "exclude"
  | "no_character_set"
  | "start_with_letter"
  | "minimums_exceed_length"
  | "words"
  | "separator";

export type CharClass = "uppercase" | "lowercase" | "digits" | "symbols";
export const CHAR_CLASSES: CharClass[] = ["uppercase", "lowercase", "digits", "symbols"];

export const MIN_KEYS: Record<CharClass, "minUppercase" | "minLowercase" | "minDigits" | "minSymbols"> = {
  uppercase: "minUppercase",
  lowercase: "minLowercase",
  digits: "minDigits",
  symbols: "minSymbols",
};

/** The validated character classes of a password (the core's `PasswordPlan`). */
export interface PasswordPlan {
  classes: { cls: CharClass; chars: string[]; min: number }[];
  /** Union of the enabled classes. */
  all: string[];
  /** Union of the enabled letter classes (only with `startWithLetter`). */
  letters: string[];
  /** `startWithLetter` without a letter minimum: one extra letter is drawn. */
  extraLetter: boolean;
  /** The symbol set without duplicates. */
  symbolSet: string;
  /** The exclude input without duplicates. */
  exclude: string;
}

export type PlanResult =
  | { ok: true; plan: PasswordPlan }
  | {
      ok: false;
      issue: GeneratorIssue;
      /** `no_character_set` because everything of this enabled class is excluded. */
      emptied?: CharClass;
    };

/** Code points, like Rust's `chars()`. */
function chars(s: string): string[] {
  return Array.from(s);
}

function dedupe(list: string[]): string {
  return [...new Set(list)].join("");
}

/** Why the symbol set is invalid: over 256 characters, a character that is not an ASCII symbol, or empty while symbols is on. */
export function symbolSetProblem(o: GeneratorOptions): "too_long" | "invalid_char" | "empty" | null {
  const list = chars(o.symbolSet);
  if (list.length > CHARSET_INPUT_MAX_CHARS) return "too_long";
  if (list.some((c) => !isAsciiSymbol(c))) return "invalid_char";
  if (o.symbols && list.length === 0) return "empty";
  return null;
}

/** Printable ASCII that is neither a letter, a digit nor a space. */
export function isAsciiSymbol(c: string): boolean {
  const code = c.codePointAt(0) ?? 0;
  return c.length === 1 && code >= 0x21 && code <= 0x7e && !/[A-Za-z0-9]/.test(c);
}

/** The core's `password_plan`: the same checks in the same order. */
export function passwordPlan(o: GeneratorOptions): PlanResult {
  if (!Number.isInteger(o.length) || o.length < PASSWORD_MIN_LEN || o.length > PASSWORD_MAX_LEN) {
    return { ok: false, issue: "length" };
  }
  if (symbolSetProblem(o)) return { ok: false, issue: "symbol_set" };
  const symbolSet = dedupe(chars(o.symbolSet));
  const excludeList = chars(o.exclude);
  if (excludeList.length > CHARSET_INPUT_MAX_CHARS) return { ok: false, issue: "exclude" };
  const exclude = dedupe(excludeList);
  const excluded = new Set(chars(exclude));
  const charset = (set: string) =>
    chars(set).filter((c) => !excluded.has(c) && !(o.avoidAmbiguous && AMBIGUOUS.includes(c)));
  const sets: Record<CharClass, string> = { uppercase: UPPER, lowercase: LOWER, digits: DIGITS, symbols: symbolSet };
  const classes: PasswordPlan["classes"] = [];
  let letters: string[] = [];
  let letterMinimum = false;
  for (const cls of CHAR_CLASSES) {
    // Minimums of disabled classes are ignored (older clients send them).
    if (!o[cls]) continue;
    const list = charset(sets[cls]);
    if (list.length === 0) return { ok: false, issue: "no_character_set", emptied: cls };
    const min = Math.max(0, Math.floor(Number(o[MIN_KEYS[cls]]) || 0));
    if (cls === "uppercase" || cls === "lowercase") {
      letters = letters.concat(list);
      letterMinimum ||= min > 0;
    }
    classes.push({ cls, chars: list, min });
  }
  if (classes.length === 0) return { ok: false, issue: "no_character_set" };
  if (o.startWithLetter && letters.length === 0) return { ok: false, issue: "start_with_letter" };
  const extraLetter = o.startWithLetter && !letterMinimum;
  const required = classes.reduce((sum, c) => sum + c.min, 0) + (extraLetter ? 1 : 0);
  if (required > o.length) return { ok: false, issue: "minimums_exceed_length" };
  return {
    ok: true,
    plan: {
      classes,
      all: classes.flatMap((c) => c.chars),
      letters: o.startWithLetter ? letters : [],
      extraLetter,
      symbolSet,
      exclude,
    },
  };
}

/** The core's `check_passphrase`. */
export function passphraseIssue(o: GeneratorOptions): GeneratorIssue | null {
  if (!Number.isInteger(o.words) || o.words < PASSPHRASE_MIN_WORDS || o.words > PASSPHRASE_MAX_WORDS) return "words";
  if (chars(o.separator).length > SEPARATOR_MAX_CHARS) return "separator";
  return null;
}

/** The first problem `generate_password` would report for these options, or null. */
export function generatorIssue(o: GeneratorOptions): GeneratorIssue | null {
  if (o.kind === "passphrase") return passphraseIssue(o);
  const result = passwordPlan(o);
  return result.ok ? null : result.issue;
}

/** Sum of the minimums of the enabled classes (plus the extra letter of `startWithLetter`). */
export function requiredCharacters(o: GeneratorOptions): number {
  let sum = 0;
  let letterMinimum = false;
  for (const cls of CHAR_CLASSES) {
    if (!o[cls]) continue;
    const min = Math.max(0, Math.floor(Number(o[MIN_KEYS[cls]]) || 0));
    sum += min;
    if ((cls === "uppercase" || cls === "lowercase") && min > 0) letterMinimum = true;
  }
  return sum + (o.startWithLetter && (o.uppercase || o.lowercase) && !letterMinimum ? 1 : 0);
}

export interface Entropy {
  /** Estimated entropy in bits (an upper bound: minimums and a first letter take a little away). */
  bits: number;
  /** Password: the number of characters drawn from. */
  pool?: number;
  /** Passphrase: the real word list size and the bits per word. */
  listSize?: number;
  bitsPerWord?: number;
}

/**
 * Entropy of the options, or null while they are invalid: a password
 * `length × log2(characters to draw from)`, a passphrase like the core's
 * `passphrase_entropy_bits` – `words × log2(list size)`, plus
 * `log2(10 × words)` for the appended digit.
 */
export function entropyOf(o: GeneratorOptions): Entropy | null {
  if (o.kind === "passphrase") {
    if (passphraseIssue(o)) return null;
    const listSize = WORDLIST_SIZES[o.wordlist] ?? WORDLIST_SIZES.en;
    const bitsPerWord = Math.log2(listSize);
    const bits = o.words * bitsPerWord + (o.includeNumber ? Math.log2(10 * o.words) : 0);
    return { bits, listSize, bitsPerWord };
  }
  const result = passwordPlan(o);
  if (!result.ok) return null;
  const pool = result.plan.all.length;
  return { bits: o.length * Math.log2(pool), pool };
}

/** Strength label (`strength.0` … `strength.4`) for an entropy in bits. */
export function entropyScore(bits: number): 0 | 1 | 2 | 3 | 4 {
  if (bits < 28) return 0;
  if (bits < 36) return 1;
  if (bits < 60) return 2;
  if (bits < 80) return 3;
  return 4;
}

const OPTION_KEYS = Object.keys(DEFAULT_GENERATOR_OPTIONS) as (keyof GeneratorOptions)[];

/** True if both have the same value in every option field. */
export function sameOptions(a: GeneratorOptions, b: GeneratorOptions): boolean {
  return OPTION_KEYS.every((key) => a[key] === b[key]);
}

/**
 * Options read field by field, like the core's `from_json_lenient`: a missing
 * field or one of the wrong type gets its default, unknown keys are dropped.
 * Not validated (see `generatorIssue`). For options the page kept in
 * localStorage.
 */
export function lenientOptions(raw: unknown): GeneratorOptions {
  const out: GeneratorOptions = { ...DEFAULT_GENERATOR_OPTIONS };
  if (!raw || typeof raw !== "object") return out;
  const input = raw as Record<string, unknown>;
  const target = out as unknown as Record<string, unknown>;
  for (const key of OPTION_KEYS) {
    const value = input[key];
    const fallback = DEFAULT_GENERATOR_OPTIONS[key];
    if (key === "kind") {
      if (value === "password" || value === "passphrase") out.kind = value as GeneratorKind;
    } else if (key === "wordlist") {
      if (value === "en" || value === "de") out.wordlist = value;
    } else if (typeof fallback === "number") {
      if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff) target[key] = value;
    } else if (typeof value === typeof fallback) {
      target[key] = value;
    }
  }
  return out;
}

/**
 * The options as the backend stores them in a preset (`normalized`): for a
 * password the symbol set and the excluded characters without duplicates.
 */
export function normalizedOptions(o: GeneratorOptions): GeneratorOptions {
  if (o.kind !== "password") return { ...o };
  return { ...o, symbolSet: dedupe(chars(o.symbolSet)), exclude: dedupe(chars(o.exclude)) };
}
