// The phone's fingerprint unlock on the UI side ("Fingerprint unlock (phone)"
// in docs/ARCHITECTURE.md): the status (`device_unlock_status`), setting it
// up and removing it for a vault (Settings → Sicherheit) and unlocking with
// it (unlock screen). The prompt itself is Android's; the page only passes
// its texts and never sees a key. Only on the phone platform – the desktop
// has none of these commands; there `useDeviceUnlock()` is null.

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { ApiError, api, events, subscribeEffect } from "../lib/api";
import type { DeviceUnlockPrompt, DeviceUnlockStatus, VaultInfo } from "../lib/types";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";

export interface DeviceUnlockApi {
  /** null until the first answer. */
  status: DeviceUnlockStatus | null;
  /** Reads the status again (the backend tidies up setups that no longer fit). */
  refresh: () => Promise<void>;
  /** The vault opens with the fingerprint: the feature is on, the vault set up, the sensor usable. */
  readyFor: (vaultId: string | undefined) => boolean;
  /** The vault's setup was removed since the app started (it no longer opened the vault). */
  wasReset: (vaultId: string | undefined) => boolean;
  /** Sets it up for the open vault (the system prompt); toasts the outcome, resolves to whether it worked. */
  enable: (vaultName: string) => Promise<boolean>;
  /** Removes it for a vault (the setting goes off with the last one); `quiet`: no toast. */
  disable: (vaultId: string, quiet?: boolean) => Promise<boolean>;
  /** Unlocks with the fingerprint; rejects with the backend's `ApiError` (`unsupported:device_unlock_*`). */
  unlock: (vault: VaultInfo) => Promise<VaultInfo>;
}

const DeviceUnlockContext = createContext<DeviceUnlockApi | null>(null);

/** The fingerprint unlock, or null outside the phone platform. */
export function useDeviceUnlock(): DeviceUnlockApi | null {
  return useContext(DeviceUnlockContext);
}

/** `unsupported:<detail>` of the device-unlock commands. */
export function deviceUnlockError(err: unknown): string | null {
  return err instanceof ApiError && err.code === "unsupported" && err.detail.startsWith("device_unlock_") ? err.detail : null;
}

// After the user locks the vault by hand the unlock screen must not open the
// fingerprint prompt on its own (it would pop up right after "Sperren"). The
// lock shows the unlock screen first and then reloads the page
// (`discardPageData`), so the note lives in sessionStorage and names the page
// load that wrote it: the unlock screen of that same page leaves it for the
// one after the reload, which consumes it. Opening a vault forgets it (the
// mock backend never reloads).
const NO_AUTO_PROMPT_KEY = "keystead.noAutoPrompt";
/** This page load (module scope: evaluated once per document). */
const PAGE_LOAD = `${Date.now()}-${Math.random()}`;

/** The user locked the vault: no automatic fingerprint prompt on the next unlock screen. */
export function skipNextAutoPrompt(): void {
  try {
    window.sessionStorage.setItem(NO_AUTO_PROMPT_KEY, PAGE_LOAD);
  } catch {
    /* storage unavailable: the prompt opens, the user can cancel it */
  }
}

/** Whether the unlock screen may open the prompt on its own (see `skipNextAutoPrompt`). */
export function autoPromptAllowed(): boolean {
  try {
    return window.sessionStorage.getItem(NO_AUTO_PROMPT_KEY) === null;
  } catch {
    return true;
  }
}

/**
 * The unlock screen has read the note (an effect: React may run a state
 * initialiser twice). Kept while the page that wrote it still shows: its
 * reload is coming, and the unlock screen after it reads the note again.
 */
export function clearAutoPromptNote(): void {
  try {
    const note = window.sessionStorage.getItem(NO_AUTO_PROMPT_KEY);
    if (note !== null && note !== PAGE_LOAD) window.sessionStorage.removeItem(NO_AUTO_PROMPT_KEY);
  } catch {
    /* nothing to clear */
  }
}

/** A vault was opened: a note from an earlier lock no longer applies. */
export function forgetAutoPromptNote(): void {
  try {
    window.sessionStorage.removeItem(NO_AUTO_PROMPT_KEY);
  } catch {
    /* nothing to forget */
  }
}

/** `enabled`: the phone platform. */
export function DeviceUnlockProvider({ enabled, children }: { enabled: boolean; children: ReactNode }) {
  const { t, errorText } = useT();
  const toast = useToast();
  const [status, setStatus] = useState<DeviceUnlockStatus | null>(null);

  const refresh = useCallback(async () => {
    if (!enabled) return;
    try {
      setStatus(await api.deviceUnlockStatus());
    } catch {
      /* keep the previous status */
    }
  }, [enabled]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A sync can bring a new master password (the setup is reset then), a lock
  // with reason `keyChanged` likewise: read the status again.
  useEffect(() => {
    if (!enabled) return;
    return subscribeEffect(
      events.onSyncStatus((s) => {
        if (!s.syncing) void refresh();
      }),
    );
  }, [enabled, refresh]);
  useEffect(() => {
    if (!enabled) return;
    return subscribeEffect(events.onVaultLocked(() => void refresh()));
  }, [enabled, refresh]);

  const readyFor = useCallback(
    (vaultId: string | undefined) =>
      Boolean(vaultId && status?.enabled && status.available && status.vaults.includes(vaultId)),
    [status],
  );
  const wasReset = useCallback(
    (vaultId: string | undefined) => Boolean(vaultId && status?.invalidated.includes(vaultId) && !status.vaults.includes(vaultId)),
    [status],
  );

  const enable = useCallback(
    async (vaultName: string) => {
      const prompt: DeviceUnlockPrompt = {
        title: t("deviceUnlock.setupPromptTitle"),
        reason: t("deviceUnlock.setupPromptReason", { name: vaultName }),
        cancel: t("common.cancel"),
      };
      try {
        setStatus(await api.enableDeviceUnlock(prompt));
        toast.success(t("deviceUnlock.enabled", { name: vaultName }));
        return true;
      } catch (err) {
        // Closing the prompt is the user's answer, no error.
        if (deviceUnlockError(err) !== "device_unlock_cancelled") toast.error(errorText(err));
        void refresh();
        return false;
      }
    },
    [errorText, refresh, t, toast],
  );

  const disable = useCallback(
    async (vaultId: string, quiet = false) => {
      try {
        setStatus(await api.disableDeviceUnlock(vaultId));
        if (!quiet) toast.info(t("deviceUnlock.disabled"));
        return true;
      } catch (err) {
        toast.error(errorText(err));
        return false;
      }
    },
    [errorText, t, toast],
  );

  const unlock = useCallback(
    async (vault: VaultInfo) => {
      const prompt: DeviceUnlockPrompt = {
        title: t("deviceUnlock.promptTitle"),
        reason: t("deviceUnlock.promptReason", { name: vault.name }),
        cancel: t("deviceUnlock.promptCancel"),
      };
      try {
        return await api.unlockWithDevice(vault.id, prompt);
      } catch (err) {
        // The setup may have been reset (or the vault be gone).
        void refresh();
        throw err;
      }
    },
    [refresh, t],
  );

  const value = useMemo<DeviceUnlockApi | null>(
    () => (enabled ? { status, refresh, readyFor, wasReset, enable, disable, unlock } : null),
    [enabled, status, refresh, readyFor, wasReset, enable, disable, unlock],
  );
  return <DeviceUnlockContext.Provider value={value}>{children}</DeviceUnlockContext.Provider>;
}
