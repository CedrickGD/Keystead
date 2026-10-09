import { createContext, useCallback, useContext } from "react";
import type { AppInfo, Settings, VaultInfo } from "../lib/types";
import { api } from "../lib/api";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";

export interface AppContextValue {
  info: AppInfo;
  setInfo: (info: AppInfo) => void;
  settings: Settings;
  /** Optimistically applies and persists a settings change; reverts and toasts on failure. */
  updateSettings: (patch: Partial<Settings>) => Promise<boolean>;
  vaults: VaultInfo[];
  refreshVaults: () => Promise<VaultInfo[]>;
  /** The unlocked vault (null while locked). */
  vault: VaultInfo | null;
  setVault: (vault: VaultInfo | null) => void;
  /** Switches to the main window after a successful unlock / creation. */
  enterVault: (vault: VaultInfo) => void;
  /** Locks the vault and returns to the unlock screen. */
  lock: () => Promise<void>;
  /** Shows the "create / import vault" flow. */
  showWelcome: () => void;
  /** Returns to the unlock screen (or the welcome screen when there are no vaults). */
  showUnlock: () => void;
}

export const AppContext = createContext<AppContextValue | null>(null);

export function useApp(): AppContextValue {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useApp must be used inside <AppContext.Provider>");
  return ctx;
}

export interface CopyKind {
  /** Already translated, capitalised noun ("Passwort", "Benutzername", …). */
  label: string;
  sensitive: boolean;
}

/** Copies via the backend (`copy_text`) and shows a toast. */
export function useCopy(): (text: string, kind: CopyKind) => Promise<void> {
  const { t, errorText } = useT();
  const toast = useToast();
  const ctx = useContext(AppContext);
  const clearSeconds = ctx?.settings.clipboardClearSeconds ?? 0;
  return useCallback(
    async (text: string, kind: CopyKind) => {
      if (!text) return;
      try {
        await api.copyText(text, kind.sensitive);
        toast.success(
          kind.sensitive && clearSeconds > 0
            ? t("copy.copiedClears", { what: kind.label, seconds: clearSeconds })
            : t("copy.copied", { what: kind.label }),
        );
      } catch (err) {
        toast.error(errorText(err));
      }
    },
    [clearSeconds, errorText, t, toast],
  );
}
