// The phone's cloud-folder sync on the UI side ("Cloud-folder sync (phone)"
// in docs/ARCHITECTURE.md): the status (`folder_status`, `sync://status`)
// and the actions with their toasts – a cloud folder (`link_folder`) or,
// since stage 2c, a WebDAV server (`link_webdav`, "Server sync (WebDAV)");
// once per app start a hint when
// an edit could not reach the folder because it is linked read-only
// (outcome `notWritable`). Only on the phone platform – the
// desktop has none of these commands; there `useFolderSync()` is null.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ApiError, api, events, subscribeEffect } from "../lib/api";
import type { FolderStatus, WebDavConfig } from "../lib/types";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";
import { useConfirm } from "../components/Confirm";

export type SyncAction = "link" | "server" | "sync" | "unlink";

export interface FolderSyncApi {
  /** null until the first answer. */
  status: FolderStatus | null;
  /** The action the user started and that has not answered yet. */
  busy: SyncAction | null;
  /** Opens the system folder picker and links the choice (then syncs). */
  link: () => Promise<void>;
  /** Links a folder on a WebDAV server (then syncs); resolves to whether it worked. */
  linkServer: (config: WebDavConfig) => Promise<boolean>;
  /** Syncs now. */
  sync: () => Promise<void>;
  /** Asks, then forgets the folder (the vault copies stay). */
  unlink: () => Promise<void>;
}

const FolderSyncContext = createContext<FolderSyncApi | null>(null);

/** The cloud-folder sync, or null outside the phone platform. */
export function useFolderSync(): FolderSyncApi | null {
  return useContext(FolderSyncContext);
}

/** Vault files a sync added or replaced. */
function changedVaults(status: FolderStatus): number {
  return status.files.filter((f) => f.outcome === "imported" || f.outcome === "updated").length;
}

/** Vault files the phone can use (everything that is not skipped or failed). */
function usableVaults(status: FolderStatus): number {
  const usable = ["imported", "updated", "upToDate", "pcOlder", "pushed", "conflict", "notWritable", "localOnly"];
  return status.files.filter((f) => usable.includes(f.outcome)).length;
}

/** A change of the phone was not written: the folder is linked read-only. */
function blockedByReadOnlyLink(status: FolderStatus): boolean {
  return status.linked && !status.writable && status.files.some((f) => f.outcome === "notWritable");
}

/**
 * `enabled`: the phone platform. `onStatus` runs for every new status (the
 * app re-reads its vault list: a vault may have arrived).
 */
export function FolderSyncProvider({
  enabled,
  onStatus,
  children,
}: {
  enabled: boolean;
  onStatus: (status: FolderStatus) => void;
  children: ReactNode;
}) {
  const { t, tp, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const [status, setStatus] = useState<FolderStatus | null>(null);
  const [busy, setBusy] = useState<SyncAction | null>(null);
  // One subscription for the provider's life; `onStatus` may change meanwhile.
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;
  // The read-only hint: once per app start (the banner in the settings stays).
  const relinkHinted = useRef(false);
  const relinkRef = useRef<() => void>(() => undefined);
  const hintRelink = useCallback(
    (s: FolderStatus) => {
      if (relinkHinted.current || s.syncing || !blockedByReadOnlyLink(s)) return;
      relinkHinted.current = true;
      toast.show({
        kind: "info",
        message: t("sync.relink.toast"),
        duration: 10_000,
        action: { label: t("sync.relink.toastAction"), onClick: () => relinkRef.current() },
      });
    },
    [t, toast],
  );
  const hintRelinkRef = useRef(hintRelink);
  hintRelinkRef.current = hintRelink;

  // The first status is fetched once the listener is registered: the sync at
  // start runs while the page boots, and its only event could otherwise fall
  // between the boot's vault list and the subscription (the welcome screen
  // would stay although a vault arrived). This way the result is in the answer
  // or in an event; the answer also goes to `onStatus`, unless an event came
  // first (it is at least as new).
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let heard = false;
    const subscription = events.onSyncStatus((s) => {
      heard = true;
      setStatus(s);
      onStatusRef.current(s);
      hintRelinkRef.current(s);
    });
    void subscription
      .catch(() => undefined)
      .then(() => (disposed ? null : api.folderStatus()))
      .then((s) => {
        if (!s || disposed || heard) return;
        setStatus(s);
        onStatusRef.current(s);
      })
      .catch(() => undefined);
    const unsubscribe = subscribeEffect(subscription);
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [enabled]);

  const link = useCallback(async () => {
    setBusy("link");
    try {
      const linked = await api.linkFolder();
      if (!linked) return; // picker closed without a choice
      setStatus(linked);
      onStatus(linked);
      const folder = linked.folderName ?? "";
      if (linked.lastError) toast.error(errorText(new ApiError(linked.lastError)));
      else if (usableVaults(linked) === 0) toast.info(t("sync.linkedEmpty", { folder }));
      else toast.success(tp("sync.linkedVaults", usableVaults(linked), { folder }));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(null);
    }
  }, [errorText, onStatus, t, toast, tp]);

  const linkServer = useCallback(
    async (config: WebDavConfig) => {
      setBusy("server");
      try {
        const linked = await api.linkWebdav(config);
        setStatus(linked);
        onStatus(linked);
        const folder = linked.folderName ?? "";
        if (linked.lastError) toast.error(errorText(new ApiError(linked.lastError)));
        else if (usableVaults(linked) === 0) toast.info(t("sync.linkedEmpty", { folder }));
        else toast.success(tp("sync.linkedVaults", usableVaults(linked), { folder }));
        return true;
      } catch (err) {
        toast.error(errorText(err));
        return false;
      } finally {
        setBusy(null);
      }
    },
    [errorText, onStatus, t, toast, tp],
  );

  const sync = useCallback(async () => {
    setBusy("sync");
    try {
      const synced = await api.syncNow();
      setStatus(synced);
      onStatus(synced);
      const changed = changedVaults(synced);
      toast.success(changed > 0 ? tp("sync.doneChanged", changed) : t("sync.doneUpToDate"));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(null);
    }
  }, [errorText, onStatus, t, toast, tp]);

  const unlink = useCallback(async () => {
    const ok = await confirm({
      title: t("sync.unlinkConfirmTitle"),
      message: t("sync.unlinkConfirmText"),
      confirmLabel: t("sync.unlink"),
      tone: "warning",
    });
    if (!ok) return;
    setBusy("unlink");
    try {
      const unlinked = await api.unlinkFolder();
      setStatus(unlinked);
      onStatus(unlinked);
      toast.info(t("sync.unlinked"));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(null);
    }
  }, [confirm, errorText, onStatus, t, toast]);

  relinkRef.current = () => void link();

  const value = useMemo<FolderSyncApi | null>(
    () => (enabled ? { status, busy, link, linkServer, sync, unlink } : null),
    [enabled, status, busy, link, linkServer, sync, unlink],
  );
  return <FolderSyncContext.Provider value={value}>{children}</FolderSyncContext.Provider>;
}
