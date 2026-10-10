// Run: node --test extension/tests/*.test.mjs
// Extension settings (lib/settings.js): defaults and what is accepted from storage.

import { test } from "node:test";
import assert from "node:assert/strict";
import { SETTINGS_DEFAULTS, normalizeSettings } from "../chrome/lib/settings.js";

test("2FA auto-copy and password suggestions are on by default", () => {
  assert.deepEqual(normalizeSettings(undefined), { autoCopyTotp: true, suggestPasswords: true });
  assert.deepEqual(normalizeSettings(null), SETTINGS_DEFAULTS);
  assert.ok(Object.isFrozen(SETTINGS_DEFAULTS));
});

test("stored values of the right type win, anything else is ignored", () => {
  assert.deepEqual(normalizeSettings({ autoCopyTotp: false }), { autoCopyTotp: false, suggestPasswords: true });
  assert.deepEqual(normalizeSettings({ autoCopyTotp: "no", suggestPasswords: 0, extra: true }), SETTINGS_DEFAULTS);
  assert.deepEqual(normalizeSettings({ autoCopyTotp: false, suggestPasswords: false }), { autoCopyTotp: false, suggestPasswords: false });
  assert.deepEqual(normalizeSettings("garbage"), SETTINGS_DEFAULTS);
});
