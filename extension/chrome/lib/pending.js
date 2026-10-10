// Save / update prompts and the vault they belong to.
//
// A prompt is computed against the vault that is open when the login form
// is submitted ("save" vs. "update", which login is updated). It must only be
// acted on while that vault is still the open one: after a vault switch the
// login would land in the other vault (or the update would name an item that
// vault does not have).
// Pure functions: also loaded by extension/tests (node --test).

/**
 * Whether a pending prompt may still be shown / saved, given a bridge status
 * (`{ state, vaultId }`):
 * - "ok": bound to the open vault, or not bound at all (an app that does not
 *   report vault ids);
 * - "stale": another vault is open now – drop the prompt;
 * - "unknown": no vault is known to be open (locked, app unreachable) – keep
 *   it; the request itself answers `locked`.
 */
export function pendingVaultState(pending, status) {
  const bound = typeof pending?.vaultId === "string" && pending.vaultId ? pending.vaultId : null;
  if (!bound) return "ok";
  if (status?.state !== "unlocked" || typeof status.vaultId !== "string" || !status.vaultId) return "unknown";
  return status.vaultId === bound ? "ok" : "stale";
}

/** The open vault's id from a bridge status (null while locked or unknown). */
export function openVaultId(status) {
  return status?.state === "unlocked" && typeof status.vaultId === "string" && status.vaultId ? status.vaultId : null;
}
