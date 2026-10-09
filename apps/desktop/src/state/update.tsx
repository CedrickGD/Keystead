// In-app updates on the UI side: what the backend found (`update://available`,
// `pending_update`, the manual check), the install progress and the user's
// "Später" for a version. Rendered by <UpdateBanner> and the About section.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, events, openExternal, subscribeEffect } from "../lib/api";
import type { UpdateInfo, UpdateProgress } from "../lib/types";

export type UpdatePhase = "idle" | "downloading" | "restarting" | "error";

export interface UpdateApi {
  /** The newest available update (null if none is known). */
  info: UpdateInfo | null;
  phase: UpdatePhase;
  progress: UpdateProgress | null;
  /** The install error (`install_update` failed). */
  error: unknown;
  /** The banner is shown (an update the user did not put off, or an install in progress). */
  bannerVisible: boolean;
  /** Checks now; resolves to the result (available or not), rejects on network errors. */
  check: () => Promise<UpdateInfo>;
  /** Installs `info` (installed copies) or opens its release page (portable). */
  install: () => Promise<void>;
  /** "Später": hides the banner for this version (until the app restarts). */
  dismiss: () => void;
  /** Forgets the known update (e.g. the channel changed). */
  reset: () => void;
}

const UpdateContext = createContext<UpdateApi | null>(null);

// "Später" survives the page reload after a lock (sessionStorage lives as long
// as the window), not an app restart.
const DISMISSED_KEY = "keystead.update.dismissed";

function loadDismissed(): string | null {
  try {
    return sessionStorage.getItem(DISMISSED_KEY);
  } catch {
    return null;
  }
}

function saveDismissed(version: string): void {
  try {
    sessionStorage.setItem(DISMISSED_KEY, version);
  } catch {
    /* only a convenience */
  }
}

export function UpdateProvider({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [dismissed, setDismissed] = useState<string | null>(loadDismissed);
  const phaseRef = useRef(phase);
  phaseRef.current = phase;

  const adopt = useCallback((next: UpdateInfo | null) => {
    // Never replace what is being installed.
    if (phaseRef.current === "downloading" || phaseRef.current === "restarting") return;
    setInfo(next?.available ? next : null);
  }, []);

  // A page that reloaded (lock) gets the update the backend already found.
  useEffect(() => {
    let active = true;
    api
      .pendingUpdate()
      .then((pending) => {
        if (active && pending) adopt(pending);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [adopt]);

  useEffect(() => subscribeEffect(events.onUpdateAvailable((next) => adopt(next))), [adopt]);
  useEffect(
    () =>
      subscribeEffect(
        events.onUpdateProgress((next) => {
          // Also after a page reload (e.g. auto-lock) while the backend downloads.
          if (phaseRef.current === "restarting") return;
          if (phaseRef.current !== "downloading") {
            phaseRef.current = "downloading";
            setPhase("downloading");
          }
          setProgress(next);
        }),
      ),
    [],
  );
  useEffect(() => subscribeEffect(events.onUpdateReady(() => setPhase("restarting"))), []);

  const check = useCallback(async () => {
    const result = await api.checkUpdate();
    adopt(result);
    // Asked for explicitly: show the banner again even after "Später".
    if (result.available) setDismissed(null);
    return result;
  }, [adopt]);

  const install = useCallback(async () => {
    if (!info) return;
    if (!info.canInstall) {
      openExternal(info.releaseUrl);
      return;
    }
    if (phaseRef.current === "downloading" || phaseRef.current === "restarting") return;
    setError(null);
    setProgress(null);
    setPhase("downloading");
    phaseRef.current = "downloading";
    try {
      await api.installUpdate();
      // Answers only once the restart is under way.
      setPhase("restarting");
    } catch (err) {
      setError(err);
      setPhase("error");
    }
  }, [info]);

  const dismiss = useCallback(() => {
    if (phaseRef.current === "error") {
      setPhase("idle");
      setError(null);
    }
    if (info?.version) {
      saveDismissed(info.version);
      setDismissed(info.version);
    }
  }, [info]);

  const reset = useCallback(() => {
    if (phaseRef.current === "downloading" || phaseRef.current === "restarting") return;
    setInfo(null);
    setPhase("idle");
    setError(null);
  }, []);

  const bannerVisible = phase !== "idle" || (info !== null && info.version !== dismissed);

  const value = useMemo<UpdateApi>(
    () => ({ info, phase, progress, error, bannerVisible, check, install, dismiss, reset }),
    [info, phase, progress, error, bannerVisible, check, install, dismiss, reset],
  );
  return <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>;
}

export function useUpdate(): UpdateApi {
  const ctx = useContext(UpdateContext);
  if (!ctx) throw new Error("useUpdate must be used inside <UpdateProvider>");
  return ctx;
}
