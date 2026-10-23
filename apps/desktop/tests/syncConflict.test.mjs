// node --test apps/desktop/tests/*.test.mjs – the sync conflict dialog's buttons.

import { test } from "node:test";
import assert from "node:assert/strict";
import { conflictChoices, conflictSides } from "../src/lib/syncConflict.mjs";

test("on the phone the PC's state is the folder's copy", () => {
  assert.deepEqual(conflictSides("android"), { local: "phone", remote: "pc" });
  assert.deepEqual(conflictSides("ios"), { local: "phone", remote: "pc" });
  assert.deepEqual(conflictChoices("android", false), [
    { device: "pc", keep: "remote" },
    { device: "phone", keep: "local" },
  ]);
});

test("on the PC the PC's state is its own copy", () => {
  for (const platform of ["windows", "linux", "macos", undefined]) {
    assert.deepEqual(conflictSides(platform), { local: "pc", remote: "phone" }, String(platform));
    assert.deepEqual(conflictChoices(platform, false), [
      { device: "pc", keep: "local" },
      { device: "phone", keep: "remote" },
    ]);
  }
});

test("another master password in the folder: this device's copy cannot be kept", () => {
  // The phone keeps only "Stand vom PC übernehmen".
  assert.deepEqual(conflictChoices("android", true), [{ device: "pc", keep: "remote" }]);
  // The PC keeps only the folder's copy (the backend refuses keep: "local").
  assert.deepEqual(conflictChoices("windows", true), [{ device: "phone", keep: "remote" }]);
  for (const platform of ["android", "windows"]) {
    assert.ok(conflictChoices(platform, true).every((c) => c.keep !== "local"));
  }
});
