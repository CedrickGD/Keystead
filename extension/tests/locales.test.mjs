// Run: node --test extension/tests/*.test.mjs
// The extension's message files (_locales/de, _locales/en): every UI string
// exists in both languages with the same placeholders.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const load = (lang) => JSON.parse(readFileSync(new URL(`../chrome/_locales/${lang}/messages.json`, import.meta.url), "utf8"));
const de = load("de");
const en = load("en");

/** The $NAME$ placeholders a message uses (a doubled $$ is a literal dollar sign). */
function used(message) {
  return [...message.replace(/\$\$/g, "").matchAll(/\$([A-Za-z0-9_@]+)\$/g)].map((m) => m[1].toLowerCase()).sort();
}

test("German and English have the same keys", () => {
  assert.deepEqual(Object.keys(en).sort(), Object.keys(de).sort());
});

test("keys are unique ignoring case (Chrome looks them up case-insensitively)", () => {
  const lower = Object.keys(de).map((key) => key.toLowerCase());
  assert.equal(new Set(lower).size, lower.length);
});

test("every placeholder is declared, and both languages declare the same ones", () => {
  for (const [lang, messages] of [
    ["de", de],
    ["en", en],
  ]) {
    for (const [key, entry] of Object.entries(messages)) {
      assert.equal(typeof entry.message, "string", `${lang}.${key}`);
      const declared = Object.keys(entry.placeholders ?? {}).sort();
      assert.deepEqual(used(entry.message), [...new Set(used(entry.message))], `${lang}.${key} repeats a placeholder`);
      assert.deepEqual(used(entry.message), declared, `${lang}.${key}`);
      // A lone "$" that starts no placeholder must be written as "$$".
      assert.doesNotMatch(entry.message.replace(/\$\$/g, "").replace(/\$[A-Za-z0-9_@]+\$/g, ""), /\$(?!\d)/, `${lang}.${key}`);
    }
  }
  for (const key of Object.keys(de)) {
    assert.deepEqual(Object.keys(en[key].placeholders ?? {}).sort(), Object.keys(de[key].placeholders ?? {}).sort(), key);
  }
});
