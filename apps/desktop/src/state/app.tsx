import { createContext, useCallback, useContext } from "react";
import type { AppInfo, SecretField, Settings, VaultInfo } from "../lib/types";
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

/** Runs a copy in the backend and shows a toast; resolves to whether it worked. */
function useCopyWithToast(): (copy: () => Promise<unknown>, kind: CopyKind) => Promise<boolean> {
  const { t, errorText } = useT();
  const toast = useToast();
  const ctx = useContext(AppContext);
  const clearSeconds = ctx?.settings.clipboardClearSeconds ?? 0;
  return useCallback(
    async (copy: () => Promise<unknown>, kind: CopyKind) => {
      try {
        await copy();
        toast.success(
          kind.sensitive && clearSeconds > 0
            ? t("copy.copiedClears", { what: kind.label, seconds: clearSeconds })
            : t("copy.copied", { what: kind.label }),
        );
        return true;
      } catch (err) {
        toast.error(errorText(err));
        return false;
      }
    },
    [clearSeconds, errorText, t, toast],
  );
}

/** Copies via the backend (`copy_text`) and shows a toast; resolves to whether it worked. */
export function useCopy(): (text: string, kind: CopyKind) => Promise<boolean> {
  const copyWithToast = useCopyWithToast();
  return useCallback(
    async (text: string, kind: CopyKind) => (text ? copyWithToast(() => api.copyText(text, kind.sensitive), kind) : false),
    [copyWithToast],
  );
}

/**
 * Copies a secret of an item as sensitive (`copy_secret_field`): the backend
 * copies it, the value never reaches the page. `label` names it in the toast.
 */
export function useCopySecret(): (itemId: string, field: SecretField, label: string) => Promise<boolean> {
  const copyWithToast = useCopyWithToast();
  return useCallback(
    (itemId: string, field: SecretField, label: string) =>
      copyWithToast(() => api.copySecretField(itemId, field), { label, sensitive: true }),
    [copyWithToast],
  );
}
