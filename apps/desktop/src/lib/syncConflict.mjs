// The sync conflict dialog's pure decisions (components/SyncConflictDialog.tsx)
// – plain JavaScript without imports, so the node tests
// (apps/desktop/tests/syncConflict.test.mjs) run them as they are; the types
// live in syncConflict.d.mts.
//
// The backend speaks of "local" (this device's copy) and "remote" (the
// cloud folder's copy). The dialog speaks of the PC and the phone, the same
// words on both devices ("Stand vom PC übernehmen" / "Stand vom Handy
// behalten"): on the phone the PC's state is the folder's copy, on the PC it
// is its own.

import { isPhonePlatform } from "./layout.mjs";

/**
 * Which device each side of a conflict is on this platform:
 * `{ local, remote }`, each "pc" or "phone".
 */
export function conflictSides(platform) {
  return isPhonePlatform(platform) ? { local: "phone", remote: "pc" } : { local: "pc", remote: "phone" };
}

/**
 * The buttons of the conflict dialog, in order (the PC's state first, then
 * the phone's): `{ device, keep }` with `keep` the value for
 * `resolve_sync_conflict`. `passwordChanged`: the folder's copy has another
 * master password – the backend refuses to keep this device's copy
 * (`unsupported:sync_conflict_password`), so that button is left out.
 */
export function conflictChoices(platform, passwordChanged) {
  const sides = conflictSides(platform);
  const keepFor = (device) => (sides.local === device ? "local" : "remote");
  return ["pc", "phone"]
    .map((device) => ({ device, keep: keepFor(device) }))
    .filter((choice) => !(passwordChanged && choice.keep === "local"));
}
