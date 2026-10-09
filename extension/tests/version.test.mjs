// Run: node --test extension/tests/*.test.mjs
// Tests the pure version helpers of the extension's self-update (lib/version.js).

import { test } from "node:test";
import assert from "node:assert/strict";
import { compareVersions, extensionUpdateAction, isNewerVersion, parseVersion } from "../chrome/lib/version.js";

test("parses Chrome extension versions (1–4 integers)", () => {
  assert.deepEqual(parseVersion("2.0.0"), [2, 0, 0]);
  assert.deepEqual(parseVersion("2.0.0.42"), [2, 0, 0, 42]);
  assert.deepEqual(parseVersion(" 7 "), [7]);
  for (const bad of ["", "2.0.0-beta.4", "1.2.3.4.5", "1..2", "a.b", "65536", "-1", null, undefined, 2]) {
    assert.equal(parseVersion(bad), null, String(bad));
  }
});

test("compares numerically, missing parts count as 0", () => {
  assert.equal(compareVersions("2.0.0.10", "2.0.0.9"), 1);
  assert.equal(compareVersions("2.0.0.9", "2.0.0.10"), -1);
  assert.equal(compareVersions("2.0", "2.0.0.0"), 0);
  assert.equal(compareVersions("2.0.0.1", "2.0.0"), 1);
  assert.equal(compareVersions("2.0.0", "2.0.0.1"), -1);
  assert.equal(compareVersions("10.0", "9.9.9.9"), 1);
  assert.equal(compareVersions("2.0.0", "nope"), null);
  assert.equal(isNewerVersion("2.0.0.42", "2.0.0.41"), true);
  assert.equal(isNewerVersion("2.0.0.41", "2.0.0.41"), false);
  assert.equal(isNewerVersion("garbage", "1.0"), false);
});

test("reloads once per delivered version, then shows the notice", () => {
  // The app delivers nothing newer.
  assert.equal(extensionUpdateAction("2.0.0.5", "2.0.0.5", null), "current");
  assert.equal(extensionUpdateAction("2.0.0.4", "2.0.0.5", null), "current");
  assert.equal(extensionUpdateAction(null, "2.0.0.5", null), "current");
  // Newer: reload once …
  assert.equal(extensionUpdateAction("2.0.0.6", "2.0.0.5", null), "reload");
  assert.equal(extensionUpdateAction("2.0.0.6", "2.0.0.5", "2.0.0.4"), "reload");
  // … and if that did not help (loaded from another folder): notice, no loop.
  assert.equal(extensionUpdateAction("2.0.0.6", "2.0.0.5", "2.0.0.6"), "notice");
  // A later version gets its own reload attempt.
  assert.equal(extensionUpdateAction("2.0.0.7", "2.0.0.5", "2.0.0.6"), "reload");
});
