// Conflicts of the two-way cloud-folder sync on the UI side (both
// platforms; "Sync conflicts" in docs/ARCHITECTURE.md): the open vault's
// checked conflicts (`sync_conflicts` at unlock, `sync://conflict` after
// that) and resolving one (`resolve_sync_conflict`). Mounted while a vault
// is open (App.tsx, keyed by the vault): a new unlock asks again. The
// dialog itself is components/SyncConflictDialog.tsx.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError, api, events, subscribeEffect } from "../lib/api";
import type { SyncConflict } from "../lib/types";
import type { ConflictKeep } from "../lib/syncConflict.mjs";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";
import { SyncConflictDialog } from "../components/SyncConflictDialog";
import { useApp } from "./app";

/** The conflict was looked at again (a copy changed meanwhile): the list is fetched anew, no message. */
const CONFLICT_CHANGED = "unsupported:sync_conflict_changed";

export function SyncConflictsProvider({ children }: { children: ReactNode }) {
  const { errorText } = useT();
  const toast = useToast();
  const { setVault, refreshVaults, showUnlock } = useApp();
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  // "Später": hidden until the next event or the next unlock (a new provider).
  const [later, setLater] = useState(false);
  const [busy, setBusy] = useState<ConflictKeep | null>(null);
  // An event that arrives while the first answer is on its way is newer.
  const heard = useRef(false);

  const refetch = useCallback(async () => {
    try {
      const list = await api.syncConflicts();
      setConflicts(list);
    } catch {
      /* locked meanwhile: the unlock screen follows on its own */
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    const subscription = events.onSyncConflict((list) => {
      heard.current = true;
      setConflicts(list);
      setLater(false);
    });
    void subscription
      .catch(() => undefined)
      .then(() => (disposed ? null : api.syncConflicts()))
      .then((list) => {
        if (list && !disposed && !heard.current) setConflicts(list);
      })
      .catch(() => undefined);
    const unsubscribe = subscribeEffect(subscription);
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, []);

  const current = later ? undefined : conflicts[0];

  const resolve = useCallback(
    async (conflict: SyncConflict, keep: ConflictKeep) => {
      setBusy(keep);
      try {
        const info = await api.resolveSyncConflict(conflict.vaultId, keep);
        setConflicts((list) => list.filter((c) => c.vaultId !== conflict.vaultId));
        setVault(info);
        void refreshVaults().catch(() => undefined);
        return true;
      } catch (err) {
        if (err instanceof ApiError && `${err.code}:${err.detail}` === CONFLICT_CHANGED) {
          await refetch();
        } else if (err instanceof ApiError && err.code === "locked") {
          // Another vault or none is open any more.
          showUnlock();
        } else {
          toast.error(errorText(err));
          await refetch();
        }
        return false;
      } finally {
        setBusy(null);
      }
    },
    [errorText, refetch, refreshVaults, setVault, showUnlock, toast],
  );

  return (
    <>
      {children}
      {current && (
        <SyncConflictDialog
          key={`conflict-${current.vaultId}`}
          conflict={current}
          busy={busy}
          onResolve={(keep) => resolve(current, keep)}
          onLater={() => setLater(true)}
        />
      )}
    </>
  );
}
