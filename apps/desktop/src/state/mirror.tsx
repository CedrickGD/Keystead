// The desktop's cloud-folder mirror on the UI side ("Kopie im Cloud-Ordner
// aktuell halten", see "Cloud-folder mirror (desktop) and phone link" in
// docs/ARCHITECTURE.md): the status (`mirror_status`, `mirror://status`), a
// toast once per new error, and setting or turning off the folder
// (`Settings.mirrorFolder`, through `updateSettings`). Since stage 2b also
// the vaults in the folder this PC does not have yet, e.g. made on the phone
// (`folder_offers`, `mirror://offers`): the PC asks before it takes one
// (`adopt_folder_vault`) and forgets the ones the user declines
// (`dismiss_folder_vault`). Desktop only – the phone has no such command;
// there `useMirror()` is null.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ApiError, api, events, pickFolder, subscribeEffect } from "../lib/api";
import type { FolderOffer, MirrorStatus } from "../lib/types";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";
import { useApp } from "./app";

export interface MirrorApi {
  /** null until the first answer. */
  status: MirrorStatus | null;
  /** The folder dialog or the settings change has not answered yet. */
  busy: boolean;
  /** Opens the folder dialog and sets the choice as the mirror folder. */
  choose: () => Promise<void>;
  /** Turns the mirror off (the copies in the folder stay). */
  turnOff: () => Promise<void>;
  /** Vaults in the folder this PC does not have (newest scan). */
  offers: FolderOffer[];
  /** The offer being taken or declined. */
  offerBusy: string | null;
  /** Takes an offered vault (it then shows in the vault list); resolves to whether it worked. */
  adopt: (offer: FolderOffer) => Promise<boolean>;
  /** Declines an offered vault for good (the file in the folder stays). */
  dismiss: (offer: FolderOffer) => Promise<boolean>;
}

const MirrorContext = createContext<MirrorApi | null>(null);

/** The cloud-folder mirror, or null on the phone. */
export function useMirror(): MirrorApi | null {
  return useContext(MirrorContext);
}

/** `enabled`: the desktop platform. Needs the app context (settings). */
export function MirrorProvider({ enabled, children }: { enabled: boolean; children: ReactNode }) {
  const { t, errorText } = useT();
  const toast = useToast();
  const { settings, updateSettings, refreshVaults } = useApp();
  const [status, setStatus] = useState<MirrorStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [offers, setOffers] = useState<FolderOffer[]>([]);
  const [offerBusy, setOfferBusy] = useState<string | null>(null);
  // The error the user has already been told about (or that was there when
  // the page loaded – a reload after a lock does not repeat the toast).
  const knownError = useRef<string | null | undefined>(undefined);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    api
      .mirrorStatus()
      .then((s) => {
        if (cancelled) return;
        if (knownError.current === undefined) knownError.current = s.lastError;
        setStatus(s);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    return subscribeEffect(
      events.onMirrorStatus((s) => {
        setStatus(s);
        if (s.lastError && s.lastError !== knownError.current) {
          toast.show({ kind: "error", message: t("mirror.failedToast", { error: errorText(new ApiError(s.lastError)) }) });
        }
        knownError.current = s.lastError;
      }),
    );
  }, [enabled, errorText, t, toast]);

  // The offers: the first answer, unless an event came first (it is newer).
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let heard = false;
    const subscription = events.onMirrorOffers((list) => {
      heard = true;
      setOffers(list);
    });
    void subscription
      .catch(() => undefined)
      .then(() => (disposed ? null : api.folderOffers()))
      .then((list) => {
        if (list && !disposed && !heard) setOffers(list);
      })
      .catch(() => undefined);
    const unsubscribe = subscribeEffect(subscription);
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [enabled]);

  const adopt = useCallback(
    async (offer: FolderOffer) => {
      setOfferBusy(offer.vaultId);
      try {
        const info = await api.adoptFolderVault(offer.vaultId);
        setOffers((list) => list.filter((o) => o.vaultId !== offer.vaultId));
        await refreshVaults().catch(() => undefined);
        toast.success(t("sync.offer.adopted", { name: info.name }));
        return true;
      } catch (err) {
        toast.error(errorText(err));
        return false;
      } finally {
        setOfferBusy(null);
      }
    },
    [errorText, refreshVaults, t, toast],
  );

  const dismiss = useCallback(
    async (offer: FolderOffer) => {
      setOfferBusy(offer.vaultId);
      try {
        await api.dismissFolderVault(offer.vaultId);
        setOffers((list) => list.filter((o) => o.vaultId !== offer.vaultId));
        toast.info(t("sync.offer.dismissed", { name: offer.name }));
        return true;
      } catch (err) {
        toast.error(errorText(err));
        return false;
      } finally {
        setOfferBusy(null);
      }
    },
    [errorText, t, toast],
  );

  const choose = useCallback(async () => {
    setBusy(true);
    try {
      const folder = await pickFolder({ title: t("mirror.pickTitle"), defaultPath: settings.mirrorFolder ?? undefined });
      if (!folder || folder === settings.mirrorFolder) return;
      // `updateSettings` shows a refused folder's error itself.
      if (await updateSettings({ mirrorFolder: folder })) toast.success(t("mirror.enabledToast", { folder }));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(false);
    }
  }, [errorText, settings.mirrorFolder, t, toast, updateSettings]);

  const turnOff = useCallback(async () => {
    setBusy(true);
    try {
      if (await updateSettings({ mirrorFolder: null })) toast.info(t("mirror.disabledToast"));
    } finally {
      setBusy(false);
    }
  }, [t, toast, updateSettings]);

  const value = useMemo<MirrorApi | null>(
    () => (enabled ? { status, busy, choose, turnOff, offers, offerBusy, adopt, dismiss } : null),
    [enabled, status, busy, choose, turnOff, offers, offerBusy, adopt, dismiss],
  );
  return <MirrorContext.Provider value={value}>{children}</MirrorContext.Provider>;
}
