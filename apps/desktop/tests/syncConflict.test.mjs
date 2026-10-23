// node --test apps/desktop/tests/*.test.mjs – the sync conflict dialog's buttons.

import { test } from "node:test";
import assert from "node:assert/strict";
import { attachmentWarnings, conflictChoices, conflictSides } from "../src/lib/syncConflict.mjs";

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

test("attachment warnings name the state that has them and the choice that drops them", () => {
  const counts = { lostAttachments: 2, remoteOnlyAttachments: 3 };
  // PC: this side is the PC.
  assert.deepEqual(attachmentWarnings("windows", counts, conflictChoices("windows", false)), {
    pcOnly: 2,
    phoneOnly: 3,
  });
  // Phone: this side is the phone, the folder's copy the PC's.
  assert.deepEqual(attachmentWarnings("android", counts, conflictChoices("android", false)), {
    pcOnly: 3,
    phoneOnly: 2,
  });
  // A choice that is not offered warns about nothing.
  assert.deepEqual(attachmentWarnings("android", counts, conflictChoices("android", true)), {
    pcOnly: 0,
    phoneOnly: 2,
  });
  assert.deepEqual(attachmentWarnings("windows", counts, conflictChoices("windows", true)), {
    pcOnly: 2,
    phoneOnly: 0,
  });
  // Unknown counts: nothing.
  assert.deepEqual(attachmentWarnings("android", {}, conflictChoices("android", false)), {
    pcOnly: 0,
    phoneOnly: 0,
  });
});
