// The leaked-password check of the open vault (see "Leaked passwords" in
// docs/ARCHITECTURE.md): the security report runs `check_pwned_vault` on the
// user's button press – never automatically – and the result lives here, in
// page memory only, so the item view can warn about a leaked item too.
// Mounted per vault page (keyed by the vault id in App.tsx): a lock reloads
// the page and a vault switch remounts it, which drops everything. An item's
// entry expires when its `updatedAt` changes – its password may have been
// replaced since the check.

import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";
import { ApiError, api } from "../lib/api";
import type { ItemListEntry, PwnedReport } from "../lib/types";
import { useT } from "../i18n";
import { useApp } from "./app";

export interface BreachesApi {
  /** The last report of this vault page (null before the first check). */
  report: PwnedReport | null;
  /** `check_pwned_vault` is running. */
  checking: boolean;
  /** Why the last check failed (null after a successful one). */
  error: unknown;
  /**
   * Runs the vault-wide check. `items` are the entries the page shows: the
   * result is pinned to their current `updatedAt`. One run at a time;
   * resolves when done (a failure lands in `error`, `locked` returns to the
   * unlock screen like every other page command).
   */
  check: (items: ItemListEntry[]) => Promise<void>;
  /**
   * The breach count of an item as of the last check, or null when the item
   * was not leaked, is unknown, or was changed since (`updatedAt`).
   */
  breachCount: (item: ItemListEntry) => number | null;
}

interface Entry {
  count: number;
  /** `updatedAt` of the item when the check started. */
  updatedAt: number;
}

const BreachesContext = createContext<BreachesApi | null>(null);

export function BreachesProvider({ children }: { children: ReactNode }) {
  const { showUnlock } = useApp();
  // Via a ref: `check` must not change with every context update.
  const showUnlockRef = useRef(showUnlock);
  showUnlockRef.current = showUnlock;
  const [report, setReport] = useState<PwnedReport | null>(null);
  const [entries, setEntries] = useState<ReadonlyMap<string, Entry>>(() => new Map());
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const running = useRef(false);

  const check = useCallback(async (items: ItemListEntry[]) => {
    if (running.current) return;
    running.current = true;
    // The backend takes its candidates when the command starts: pin the
    // result to the items as they are now.
    const versions = new Map(items.map((item) => [item.id, item.updatedAt]));
    setChecking(true);
    setError(null);
    try {
      const next = await api.checkPwnedVault();
      const known = new Map<string, Entry>();
      for (const { itemId, count } of next.leaked) {
        const updatedAt = versions.get(itemId);
        // An item the page did not list when the check started cannot be
        // matched to a version; the next check picks it up.
        if (updatedAt !== undefined) known.set(itemId, { count, updatedAt });
      }
      setReport(next);
      setEntries(known);
    } catch (err) {
      if (err instanceof ApiError && err.code === "locked") {
        showUnlockRef.current();
        return;
      }
      setError(err);
    } finally {
      running.current = false;
      setChecking(false);
    }
  }, []);

  const breachCount = useCallback(
    (item: ItemListEntry) => {
      const entry = entries.get(item.id);
      return entry && entry.updatedAt === item.updatedAt ? entry.count : null;
    },
    [entries],
  );

  const value = useMemo<BreachesApi>(
    () => ({ report, checking, error, check, breachCount }),
    [report, checking, error, check, breachCount],
  );
  return <BreachesContext.Provider value={value}>{children}</BreachesContext.Provider>;
}

export function useBreaches(): BreachesApi {
  const ctx = useContext(BreachesContext);
  if (!ctx) throw new Error("useBreaches must be used inside <BreachesProvider>");
  return ctx;
}

/**
 * Error text of a failed check: network errors name the service (like
 * `useUpdateErrorText`); `unsupported:pwned_proxy` has its own text in
 * `errorText`.
 */
export function usePwnedErrorText(): (err: unknown) => string {
  const { t, errorText } = useT();
  return useCallback(
    (err: unknown) => (err instanceof ApiError && err.code === "io" ? t("pwned.errorNetwork", { detail: err.detail }) : errorText(err)),
    [t, errorText],
  );
}
