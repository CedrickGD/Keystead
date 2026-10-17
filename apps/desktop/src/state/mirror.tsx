// The desktop's cloud-folder mirror on the UI side ("Kopie im Cloud-Ordner
// aktuell halten", see "Cloud-folder mirror (desktop) and phone link" in
// docs/ARCHITECTURE.md): the status (`mirror_status`, `mirror://status`), a
// toast once per new error, and setting or turning off the folder
// (`Settings.mirrorFolder`, through `updateSettings`). Desktop only – the
// phone has no such command; there `useMirror()` is null.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ApiError, api, events, pickFolder, subscribeEffect } from "../lib/api";
import type { MirrorStatus } from "../lib/types";
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
  const { settings, updateSettings } = useApp();
  const [status, setStatus] = useState<MirrorStatus | null>(null);
  const [busy, setBusy] = useState(false);
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
    () => (enabled ? { status, busy, choose, turnOff } : null),
    [enabled, status, busy, choose, turnOff],
  );
  return <MirrorContext.Provider value={value}>{children}</MirrorContext.Provider>;
}
