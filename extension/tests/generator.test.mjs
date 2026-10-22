// Run: node --test extension/tests/*.test.mjs
// Generator options of the extension (lib/generator.js): what is sent to the
// app, how stored options are read, and the checks mirrored from the core.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_SYMBOLS,
  GENERATOR_DEFAULTS,
  WORDLIST_SIZES,
  entropyBits,
  entropyScore,
  generatorIssue,
  lenientOptions,
  requestOptions,
  requiredCharacters,
  sameOptions,
  symbolSetProblem,
} from "../chrome/lib/generator.js";

const options = (patch = {}) => ({ ...GENERATOR_DEFAULTS, ...patch });

test("the defaults are the core's GeneratorOptions::default() and valid", () => {
  assert.deepEqual(GENERATOR_DEFAULTS, {
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
    symbolSet: "!#$%&*+-=?@^_",
    exclude: "",
    avoidAmbiguous: false,
    startWithLetter: false,
    words: 5,
    separator: "-",
    capitalize: true,
    includeNumber: true,
    wordlist: "en",
  });
  assert.ok(Object.isFrozen(GENERATOR_DEFAULTS));
  assert.equal(DEFAULT_SYMBOLS, GENERATOR_DEFAULTS.symbolSet);
  assert.equal(generatorIssue(GENERATOR_DEFAULTS), null);
  assert.equal(generatorIssue(options({ kind: "passphrase" })), null);
});

test("no own options: nothing to send, so the app uses its default preset", () => {
  assert.equal(requestOptions(null), null);
  assert.equal(requestOptions(undefined), null);
  assert.equal(requestOptions("x"), null);
  assert.equal(requestOptions([]), null);
  assert.deepEqual(requestOptions({}), {});
});

test("only known fields with acceptable values are sent", () => {
  assert.deepEqual(requestOptions(GENERATOR_DEFAULTS), { ...GENERATOR_DEFAULTS });
  assert.deepEqual(
    requestOptions({
      kind: "pin",
      length: 4,
      uppercase: "yes",
      minDigits: 2,
      minSymbols: -1,
      symbolSet: "!?a",
      exclude: "x".repeat(257),
      separator: "123456789",
      words: 21,
      wordlist: "de",
      startWithLetter: true,
      extra: 1,
    }),
    { length: 4, minDigits: 2, wordlist: "de", startWithLetter: true },
  );
  // Length 4 is the new minimum (PIN preset); 3 and 129 are not.
  assert.deepEqual(requestOptions({ length: 3 }), {});
  assert.deepEqual(requestOptions({ length: 129 }), {});
  assert.deepEqual(requestOptions({ separator: "12345678", symbolSet: "" }), { separator: "12345678", symbolSet: "" });
});

test("stored options of an older popup keep their values; new fields get defaults", () => {
  const old = {
    kind: "password",
    length: 32,
    uppercase: true,
    lowercase: true,
    digits: true,
    symbols: false,
    minDigits: 1,
    minSymbols: 1,
    avoidAmbiguous: true,
    words: 6,
    separator: "_",
    capitalize: false,
    includeNumber: true,
  };
  const read = lenientOptions(old);
  assert.deepEqual(read, { ...GENERATOR_DEFAULTS, ...old });
  assert.equal(generatorIssue(read), null);
  assert.deepEqual(lenientOptions(null), { ...GENERATOR_DEFAULTS });
  assert.deepEqual(lenientOptions({ length: "20", wordlist: "fr" }), { ...GENERATOR_DEFAULTS });
  assert.ok(sameOptions(lenientOptions({}), GENERATOR_DEFAULTS));
  assert.ok(!sameOptions(read, GENERATOR_DEFAULTS));
});

test("problems come in the core's order with its codes", () => {
  assert.deepEqual(generatorIssue(options({ length: 3 })), { code: "length" });
  assert.deepEqual(generatorIssue(options({ length: 3, symbolSet: "a" })), { code: "length" });
  assert.deepEqual(generatorIssue(options({ symbolSet: "ab" })), { code: "symbol_set" });
  // The symbol set is checked even while symbols are off.
  assert.deepEqual(generatorIssue(options({ symbols: false, symbolSet: "ä" })), { code: "symbol_set" });
  assert.deepEqual(generatorIssue(options({ symbolSet: "" })), { code: "symbol_set" });
  assert.equal(generatorIssue(options({ symbols: false, symbolSet: "" })), null);
  assert.deepEqual(generatorIssue(options({ exclude: "x".repeat(257) })), { code: "exclude" });
  assert.deepEqual(generatorIssue(options({ exclude: "0123456789" })), { code: "no_character_set", emptied: "digits" });
  assert.deepEqual(generatorIssue(options({ exclude: DEFAULT_SYMBOLS })), { code: "no_character_set", emptied: "symbols" });
  assert.deepEqual(generatorIssue(options({ uppercase: false, lowercase: false, digits: false, symbols: false })), {
    code: "no_character_set",
  });
  assert.deepEqual(generatorIssue(options({ uppercase: false, lowercase: false, startWithLetter: true })), {
    code: "start_with_letter",
  });
  assert.deepEqual(generatorIssue(options({ length: 4, minDigits: 3 })), { code: "minimums_exceed_length" });
  assert.deepEqual(generatorIssue(options({ kind: "passphrase", words: 2 })), { code: "words" });
  assert.deepEqual(generatorIssue(options({ kind: "passphrase", separator: "123456789" })), { code: "separator" });
  assert.deepEqual(generatorIssue(null), { code: "options" });
});

test("minimums of disabled groups are ignored; a first letter needs room", () => {
  // The PIN preset: digits only, length 6, older minimums still set.
  assert.equal(generatorIssue(options({ length: 6, uppercase: false, lowercase: false, symbols: false, minSymbols: 9 })), null);
  // start_with_letter without a letter minimum draws one extra letter.
  const firstLetter = options({ length: 4, minUppercase: 0, minLowercase: 0, minDigits: 2, minSymbols: 1, startWithLetter: true });
  assert.equal(requiredCharacters(firstLetter), 4);
  assert.equal(generatorIssue(firstLetter), null);
  assert.deepEqual(generatorIssue({ ...firstLetter, minDigits: 3 }), { code: "minimums_exceed_length" });
  assert.equal(requiredCharacters(options({ startWithLetter: true })), 4);
});

test("symbol set problems are told apart for the popup's message", () => {
  assert.equal(symbolSetProblem(options()), null);
  assert.equal(symbolSetProblem(options({ symbolSet: "" })), "empty");
  assert.equal(symbolSetProblem(options({ symbolSet: "! " })), "invalid_char");
  assert.equal(symbolSetProblem(options({ symbolSet: "!".repeat(257) })), "too_long");
  assert.equal(symbolSetProblem(options({ symbols: false, symbolSet: "" })), null);
});

test("entropy uses the real pool and word list sizes", () => {
  // 26 + 26 + 10 + 13 symbols = 75 characters.
  assert.ok(Math.abs(entropyBits(GENERATOR_DEFAULTS) - 20 * Math.log2(75)) < 1e-9);
  // avoidAmbiguous and exclude shrink the pool (duplicates count once).
  const smaller = options({ avoidAmbiguous: true, exclude: "xx" });
  assert.ok(Math.abs(entropyBits(smaller) - 20 * Math.log2(75 - 5 - 1)) < 1e-9);
  const phrase = options({ kind: "passphrase", wordlist: "de", includeNumber: false });
  assert.ok(Math.abs(entropyBits(phrase) - 5 * Math.log2(WORDLIST_SIZES.de)) < 1e-9);
  const withNumber = options({ kind: "passphrase" });
  assert.ok(Math.abs(entropyBits(withNumber) - (5 * Math.log2(7776) + Math.log2(50))) < 1e-9);
  assert.equal(entropyBits(options({ length: 3 })), null);
  assert.deepEqual([27, 28, 36, 60, 80].map(entropyScore), [0, 1, 2, 3, 4]);
});
