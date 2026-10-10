// "A recovery key is waiting to be confirmed": set when a new recovery key is
// shown (created, or replaced by a master-password change – the old one stops
// working at once), cleared once the user confirmed storing it, or when the
// key is removed. A lock or a vault switch reloads the page (discard.ts) and
// would drop the dialog with the only copy of the key; after the next unlock
// the main window then warns and offers to create a fresh key.
//
// Only the vault id is stored – never the key. localStorage, so it also
// survives a restart of the app (quit or crash while the key was shown).

import { useEffect, useLayoutEffect, useState } from "react";
import { getPageVault } from "./api";

const STORAGE_KEY = "keystead.recoveryUnconfirmed";
const CHANGE_EVENT = "keystead:recovery-unconfirmed";

function read(): string[] {
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string" && id !== "") : [];
  } catch {
    return [];
  }
}

function write(ids: string[]): void {
  try {
    if (ids.length) window.localStorage.setItem(STORAGE_KEY, JSON.stringify(ids));
    else window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* storage unavailable: no reminder after a reload */
  }
  window.dispatchEvent(new Event(CHANGE_EVENT));
}

/** A new recovery key of `vaultId` (default: the page's vault) is shown and not yet confirmed as stored. */
export function markRecoveryUnconfirmed(vaultId: string | null = getPageVault()): void {
  if (!vaultId) return;
  const ids = read();
  if (!ids.includes(vaultId)) write([...ids, vaultId]);
}

/** The user confirmed storing the key, or the vault has none any more. */
export function clearRecoveryUnconfirmed(vaultId: string | null = getPageVault()): void {
  if (!vaultId) return;
  const ids = read();
  if (ids.includes(vaultId)) write(ids.filter((id) => id !== vaultId));
}

export function isRecoveryUnconfirmed(vaultId: string | null | undefined): boolean {
  return Boolean(vaultId) && read().includes(vaultId as string);
}

/**
 * Vaults whose new key this page shows right now (a mounted
 * `RecoveryKeyReveal`, counted per instance). Page memory only: a reload
 * drops the dialog and this with it, and the reminder appears.
 */
const keyOnScreen = new Map<string, number>();

/**
 * While the calling component is mounted, this page shows the new recovery
 * key of `vaultId` (default: the page's vault): no reminder meanwhile – the
 * key is right there and the user is about to confirm it.
 */
export function useRecoveryKeyOnScreen(vaultId: string | null = getPageVault()): void {
  // Layout effect: registered before the frame with the key is painted, so
  // the reminder never flashes up behind the dialog.
  useLayoutEffect(() => {
    if (!vaultId) return;
    keyOnScreen.set(vaultId, (keyOnScreen.get(vaultId) ?? 0) + 1);
    window.dispatchEvent(new Event(CHANGE_EVENT));
    return () => {
      const left = (keyOnScreen.get(vaultId) ?? 1) - 1;
      if (left > 0) keyOnScreen.set(vaultId, left);
      else keyOnScreen.delete(vaultId);
      window.dispatchEvent(new Event(CHANGE_EVENT));
    };
  }, [vaultId]);
}

/**
 * `isRecoveryUnconfirmed`, kept up to date (this page and other windows) –
 * but false while this page itself shows that vault's new key.
 */
export function useRecoveryUnconfirmed(vaultId: string | null | undefined): boolean {
  const [pending, setPending] = useState(() => isRecoveryUnconfirmed(vaultId) && !keyOnScreen.has(vaultId as string));
  useEffect(() => {
    const update = () => setPending(isRecoveryUnconfirmed(vaultId) && !keyOnScreen.has(vaultId as string));
    update();
    window.addEventListener(CHANGE_EVENT, update);
    window.addEventListener("storage", update);
    return () => {
      window.removeEventListener(CHANGE_EVENT, update);
      window.removeEventListener("storage", update);
    };
  }, [vaultId]);
  return pending;
}
