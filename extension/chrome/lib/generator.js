// Generator options of the extension, mirrored from keystead-core's
// generator.rs (docs/ARCHITECTURE.md, "Generator"). Pure: also loaded by
// extension/tests (node --test).
//
// The app generates; the extension only decides what to send. The popup keeps
// *own* options in chrome.storage.local "generatorOptions" once the user chose
// to customise them there ("Anpassen") – or, from an update, the options the
// popup of an extension up to 2.0.0 stored on every change. Stored options are
// always read field by field over the defaults (storedOptions), by the popup
// and the service worker alike. Without own options a request carries no
// `options` and the app uses its default generator preset (the desktop
// generator page's "Als Standard verwenden"). The popup checks own options
// with generatorIssue() before it stores or sends them – the bridge only
// answers `invalid_request` for options the generator rejects – and with
// olderAppIssue() against what an older app would ignore.

export const PASSWORD_MIN_LEN = 4;
export const PASSWORD_MAX_LEN = 128;
export const PASSPHRASE_MIN_WORDS = 3;
export const PASSPHRASE_MAX_WORDS = 20;
export const SEPARATOR_MAX_CHARS = 8;
/** Longest accepted `symbolSet` / `exclude` input (characters, before duplicates are removed). */
export const CHARSET_INPUT_MAX_CHARS = 256;
/** Longest generated password the app keeps in its history (`remember_generated`). */
export const GENERATED_MAX_CHARS = 1024;
/** The core's default symbol set (no quotes, backslash or space). */
export const DEFAULT_SYMBOLS = "!#$%&*+-=?@^_";
/** Real sizes of the app's embedded word lists (the core's tests pin them). */
export const WORDLIST_SIZES = Object.freeze({ en: 7776, de: 7724 });
/**
 * The app's generator interface this extension is written for: `status`
 * reports it as `generatorVersion` (the bridge's GENERATOR_VERSION). Apps
 * before the generator rework (up to 2.0.0) report none – version 1: they
 * ignore the newer option fields and reject lengths below 5.
 */
export const GENERATOR_VERSION = 2;
/** Shortest password an app before GENERATOR_VERSION 2 generates. */
const OLDER_APP_MIN_LEN = 5;

const UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER = "abcdefghijklmnopqrstuvwxyz";
const DIGITS = "0123456789";
/** Characters excluded by `avoidAmbiguous`. */
const AMBIGUOUS = "Il1O0";

/** The core's `GeneratorOptions::default()` – the start of own options in the popup. */
export const GENERATOR_DEFAULTS = Object.freeze({
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
});

export const CHAR_CLASSES = Object.freeze(["uppercase", "lowercase", "digits", "symbols"]);
export const MIN_KEYS = Object.freeze({
  uppercase: "minUppercase",
  lowercase: "minLowercase",
  digits: "minDigits",
  symbols: "minSymbols",
});

/** Code points, like Rust's `chars()`. */
function chars(text) {
  return Array.from(text);
}

const isBool = (v) => typeof v === "boolean";
const intIn = (min, max) => (v) => Number.isInteger(v) && v >= min && v <= max;
const shortString = (max) => (v) => typeof v === "string" && chars(v).length <= max;

/**
 * What a field may hold to be sent to the app: the right type and a value
 * the generator accepts on its own (combinations – minimums against the
 * length, an empty symbol set – are generatorIssue's business).
 */
const FIELD_RULES = Object.freeze({
  kind: (v) => v === "password" || v === "passphrase",
  length: intIn(PASSWORD_MIN_LEN, PASSWORD_MAX_LEN),
  uppercase: isBool,
  lowercase: isBool,
  digits: isBool,
  symbols: isBool,
  minUppercase: intIn(0, PASSWORD_MAX_LEN),
  minLowercase: intIn(0, PASSWORD_MAX_LEN),
  minDigits: intIn(0, PASSWORD_MAX_LEN),
  minSymbols: intIn(0, PASSWORD_MAX_LEN),
  symbolSet: (v) => shortString(CHARSET_INPUT_MAX_CHARS)(v) && chars(v).every(isAsciiSymbol),
  exclude: shortString(CHARSET_INPUT_MAX_CHARS),
  avoidAmbiguous: isBool,
  startWithLetter: isBool,
  words: intIn(PASSPHRASE_MIN_WORDS, PASSPHRASE_MAX_WORDS),
  separator: shortString(SEPARATOR_MAX_CHARS),
  capitalize: isBool,
  includeNumber: isBool,
  wordlist: (v) => v === "en" || v === "de",
});

/** Printable ASCII that is neither a letter, a digit nor a space. */
export function isAsciiSymbol(c) {
  const code = c.codePointAt(0) ?? 0;
  return c.length === 1 && code >= 0x21 && code <= 0x7e && !/[A-Za-z0-9]/.test(c);
}

/**
 * The `options` of a `generate_password` request: the known fields of `raw`
 * with acceptable values (anything else is left out, so the app takes it
 * from its default preset), or null if `raw` is no object (no own options:
 * the request carries none).
 */
export function requestOptions(raw) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const options = {};
  for (const [key, valid] of Object.entries(FIELD_RULES)) {
    if (valid(raw[key])) options[key] = raw[key];
  }
  return options;
}

/**
 * Stored options read field by field over the defaults: a missing or
 * unacceptable field gets its default, unknown keys are dropped – options an
 * older popup stored (without the newer fields) keep their values.
 */
export function lenientOptions(raw) {
  return { ...GENERATOR_DEFAULTS, ...(requestOptions(raw) ?? {}) };
}

/**
 * The own options in storage ("generatorOptions") as the popup shows them and
 * every request sends them – complete, via lenientOptions – or null if there
 * are none (the app's default preset applies). A partial object an older
 * popup stored must not leave its missing fields to the app's default preset
 * in some requests and to the defaults in others.
 */
export function storedOptions(raw) {
  return raw && typeof raw === "object" && !Array.isArray(raw) ? lenientOptions(raw) : null;
}

/** True if both have the same value in every option field. */
export function sameOptions(a, b) {
  return Object.keys(GENERATOR_DEFAULTS).every((key) => a?.[key] === b?.[key]);
}

const minimum = (o, cls) => Math.max(0, Math.floor(Number(o[MIN_KEYS[cls]]) || 0));

/**
 * Why the symbol set is invalid – over 256 characters, a character that is
 * not an ASCII symbol, empty while symbols are on – or null.
 */
export function symbolSetProblem(o) {
  const list = chars(String(o.symbolSet ?? ""));
  if (list.length > CHARSET_INPUT_MAX_CHARS) return "too_long";
  if (list.some((c) => !isAsciiSymbol(c))) return "invalid_char";
  if (o.symbols && list.length === 0) return "empty";
  return null;
}

/**
 * The character classes a password draws from (the core's
 * `password_plan`, same checks in the same order): `{ ok: true, pool,
 * required }` or `{ ok: false, code, emptied? }` with the generator's
 * `invalid_input` code (`emptied`: the enabled class all of whose
 * characters are excluded).
 */
function passwordPlan(o) {
  if (!FIELD_RULES.length(o.length)) return { ok: false, code: "length" };
  if (symbolSetProblem(o)) return { ok: false, code: "symbol_set" };
  const exclude = String(o.exclude ?? "");
  if (chars(exclude).length > CHARSET_INPUT_MAX_CHARS) return { ok: false, code: "exclude" };
  const excluded = new Set(chars(exclude));
  const charset = (set) => [...new Set(chars(set))].filter((c) => !excluded.has(c) && !(o.avoidAmbiguous && AMBIGUOUS.includes(c)));
  const sets = { uppercase: UPPER, lowercase: LOWER, digits: DIGITS, symbols: String(o.symbolSet ?? "") };
  let pool = 0;
  let required = 0;
  let letters = 0;
  let letterMinimum = false;
  let enabled = 0;
  for (const cls of CHAR_CLASSES) {
    // Minimums of disabled classes are ignored (older clients send them).
    if (!o[cls]) continue;
    const list = charset(sets[cls]);
    if (list.length === 0) return { ok: false, code: "no_character_set", emptied: cls };
    const min = minimum(o, cls);
    if (cls === "uppercase" || cls === "lowercase") {
      letters += list.length;
      letterMinimum ||= min > 0;
    }
    enabled += 1;
    pool += list.length;
    required += min;
  }
  if (enabled === 0) return { ok: false, code: "no_character_set" };
  if (o.startWithLetter && letters === 0) return { ok: false, code: "start_with_letter" };
  if (o.startWithLetter && !letterMinimum) required += 1;
  if (required > o.length) return { ok: false, code: "minimums_exceed_length" };
  return { ok: true, pool, required };
}

/**
 * The first problem the app's generator would report for these options – its
 * `invalid_input` code (`length`, `symbol_set`, `exclude`,
 * `no_character_set`, `start_with_letter`, `minimums_exceed_length`,
 * `words`, `separator`) plus `emptied` for a class excluded completely – or
 * null if they are fine.
 */
export function generatorIssue(o) {
  if (!o || typeof o !== "object") return { code: "options" };
  if (o.kind === "passphrase") {
    if (!FIELD_RULES.words(o.words)) return { code: "words" };
    if (!FIELD_RULES.separator(o.separator)) return { code: "separator" };
    return null;
  }
  const plan = passwordPlan(o);
  if (plan.ok) return null;
  return plan.emptied ? { code: plan.code, emptied: plan.emptied } : { code: plan.code };
}

/**
 * `{ code: "app_outdated", field }` if an app with this `generatorVersion` (from
 * `status`; 1 for apps that report none) would not generate what the options
 * `o` (request options, possibly partial: a missing field is the app's own)
 * ask for (`field`: the first option it would not honour), else null – also
 * while the version is unknown (null). Apps before
 * GENERATOR_VERSION 2 silently ignore `symbolSet`, `exclude`,
 * `startWithLetter`, `wordlist` and minimums of upper- and lower-case
 * letters (they always put in one of each), and reject lengths below 5. What
 * they do produce for the remaining options still meets them (a minimum of 0
 * or 1 letters, the default symbols, the English list).
 */
export function olderAppIssue(o, generatorVersion) {
  if (!Number.isInteger(generatorVersion) || generatorVersion >= GENERATOR_VERSION) return null;
  if (!o || typeof o !== "object") return null;
  const outdated = (field) => ({ code: "app_outdated", field });
  if (o.kind === "passphrase") return o.wordlist !== undefined && o.wordlist !== "en" ? outdated("wordlist") : null;
  if (o.length !== undefined && o.length < OLDER_APP_MIN_LEN) return outdated("length");
  const enabled = (cls) => o[cls] !== false;
  if (enabled("uppercase") && (o.minUppercase ?? 1) > 1) return outdated("minUppercase");
  if (enabled("lowercase") && (o.minLowercase ?? 1) > 1) return outdated("minLowercase");
  const sameSet = (a, b) => {
    const x = new Set(chars(a));
    const y = new Set(chars(b));
    return x.size === y.size && [...x].every((c) => y.has(c));
  };
  if (enabled("symbols") && o.symbolSet !== undefined && !sameSet(String(o.symbolSet), DEFAULT_SYMBOLS)) return outdated("symbolSet");
  if (o.exclude !== undefined) {
    // Only characters the password could contain matter.
    const sets = { uppercase: UPPER, lowercase: LOWER, digits: DIGITS, symbols: DEFAULT_SYMBOLS };
    const pool = CHAR_CLASSES.filter(enabled).map((cls) => sets[cls]).join("");
    if (chars(String(o.exclude)).some((c) => pool.includes(c) && !(o.avoidAmbiguous && AMBIGUOUS.includes(c)))) return outdated("exclude");
  }
  if (o.startWithLetter === true) return outdated("startWithLetter");
  return null;
}

/** Sum of the minimums of the enabled classes (plus the extra first letter of `startWithLetter`). */
export function requiredCharacters(o) {
  let sum = 0;
  let letterMinimum = false;
  for (const cls of CHAR_CLASSES) {
    if (!o[cls]) continue;
    const min = minimum(o, cls);
    sum += min;
    if ((cls === "uppercase" || cls === "lowercase") && min > 0) letterMinimum = true;
  }
  return sum + (o.startWithLetter && (o.uppercase || o.lowercase) && !letterMinimum ? 1 : 0);
}

/**
 * Estimated entropy in bits, or null while the options are invalid: a
 * password `length × log2(characters to draw from)`, a passphrase like the
 * core's `passphrase_entropy_bits` – `words × log2(real list size)`, plus
 * `log2(10 × words)` for the appended digit.
 */
export function entropyBits(o) {
  if (generatorIssue(o)) return null;
  if (o.kind === "passphrase") {
    const size = WORDLIST_SIZES[o.wordlist] ?? WORDLIST_SIZES.en;
    return o.words * Math.log2(size) + (o.includeNumber ? Math.log2(10 * o.words) : 0);
  }
  return o.length * Math.log2(passwordPlan(o).pool);
}

/** Strength step 0 (very weak) … 4 (very strong) for an entropy in bits (as the desktop generator page). */
export function entropyScore(bits) {
  if (bits < 28) return 0;
  if (bits < 36) return 1;
  if (bits < 60) return 2;
  if (bits < 80) return 3;
  return 4;
}
