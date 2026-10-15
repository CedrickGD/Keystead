// Run: node --test extension/tests/*.test.mjs
// Save / update prompts are bound to the vault they were computed for
// (lib/pending.js, used by background.js).

import { test } from "node:test";
import assert from "node:assert/strict";
import { openVaultId, pendingVaultState } from "../chrome/lib/pending.js";

const arbeit = { state: "unlocked", vaultId: "vault-arbeit", vaultName: "Arbeit" };
const privat = { state: "unlocked", vaultId: "vault-privat", vaultName: "Privat" };
const locked = { state: "locked", vaultId: null, vaultName: null };

test("a prompt captured under one vault is stale once another vault is open", () => {
  // Captured while "Arbeit" was open …
  const pending = { id: "p1", kind: "save", vaultId: openVaultId(arbeit) };
  assert.equal(pending.vaultId, "vault-arbeit");
  assert.equal(pendingVaultState(pending, arbeit), "ok");
  // … the user switches to "Privat" in the popup: neither shown (cs:pending)
  // nor saved (cs:save-decision) any more.
  assert.equal(pendingVaultState(pending, privat), "stale");
  // An update prompt names an item of "Arbeit": same.
  assert.equal(pendingVaultState({ ...pending, kind: "update", itemId: "x" }, privat), "stale");
});

test("locked or unknown state keeps the prompt for the request to decide", () => {
  const pending = { id: "p1", kind: "save", vaultId: "vault-arbeit" };
  assert.equal(pendingVaultState(pending, locked), "unknown");
  assert.equal(pendingVaultState(pending, null), "unknown");
  assert.equal(pendingVaultState(pending, { state: "unlocked", vaultId: null }), "unknown");
  assert.equal(pendingVaultState(pending, { state: "app_unavailable" }), "unknown");
});

test("prompts of apps without vault ids are not bound", () => {
  for (const vaultId of [null, undefined, "", 7]) {
    assert.equal(pendingVaultState({ id: "p", vaultId }, privat), "ok", String(vaultId));
  }
  assert.equal(pendingVaultState(null, privat), "ok");
});

test("open vault id only while unlocked", () => {
  assert.equal(openVaultId(privat), "vault-privat");
  assert.equal(openVaultId(locked), null);
  assert.equal(openVaultId({ state: "locked", vaultId: "vault-privat" }), null);
  assert.equal(openVaultId({ state: "unlocked", vaultId: "" }), null);
  assert.equal(openVaultId(null), null);
});
