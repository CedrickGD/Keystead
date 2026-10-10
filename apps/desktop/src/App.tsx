import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, ServerCrash } from "lucide-react";
import { api, events, setPageVault, subscribeEffect } from "./lib/api";
import { discardPageData } from "./lib/discard";
import type { AppInfo, PairingRequest, Settings, ThemeSetting, VaultInfo } from "./lib/types";
import { localPrefs } from "./lib/utils";
import { I18nProvider, useT } from "./i18n";
import { ToastProvider, useToast } from "./components/Toasts";
import { ConfirmProvider } from "./components/Confirm";
import { Button } from "./components/Controls";
import { Logo } from "./components/Logo";
import { AppContext, type AppContextValue } from "./state/app";
import { UpdateProvider } from "./state/update";
import { UpdateBanner } from "./components/UpdateBanner";
import { RecoveryReminder } from "./components/RecoveryReminder";
import { WelcomeScreen } from "./screens/WelcomeScreen";
import { UnlockScreen } from "./screens/UnlockScreen";
import { MainScreen } from "./screens/main/MainScreen";
import { IconsProvider } from "./state/icons";
import { PairingModal } from "./screens/PairingModal";
import { FileDropProvider } from "./components/import/FileDrop";

type Screen = "loading" | "error" | "welcome" | "unlock" | "main";

function useApplyTheme(theme: ThemeSetting | undefined) {
  useEffect(() => {
    if (!theme) return;
    localPrefs.set("theme", theme);
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const effective = theme === "system" ? (media.matches ? "dark" : "light") : theme;
      document.documentElement.dataset.theme = effective;
    };
    apply();
    if (theme !== "system") return;
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
}

export function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const lang = settings?.language ?? "de";
  useApplyTheme(settings?.theme);
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  return (
    <I18nProvider lang={lang}>
      <ToastProvider>
        <ConfirmProvider>
          <UpdateProvider>
            <AppRoot settings={settings} setSettings={setSettings} />
          </UpdateProvider>
        </ConfirmProvider>
      </ToastProvider>
    </I18nProvider>
  );
}

interface Boot {
  info: AppInfo;
  vaults: VaultInfo[];
}

function AppRoot({
  settings,
  setSettings,
}: {
  settings: Settings | null;
  setSettings: (s: Settings | ((prev: Settings | null) => Settings | null)) => void;
}) {
  const { t, errorText } = useT();
  const toast = useToast();
  const [screen, setScreen] = useState<Screen>("loading");
  const [bootError, setBootError] = useState<string | null>(null);
  const [boot, setBoot] = useState<Boot | null>(null);
  const [vault, setVault] = useState<VaultInfo | null>(null);
  const vaultRef = useRef(vault);
  vaultRef.current = vault;
  const screenRef = useRef(screen);
  screenRef.current = screen;
  const [pairings, setPairings] = useState<PairingRequest[]>([]);
  const pairingsRef = useRef(pairings);
  pairingsRef.current = pairings;
  const [unlockFocus, setUnlockFocus] = useState(0);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const vaultsRef = useRef<VaultInfo[]>([]);
  vaultsRef.current = boot?.vaults ?? [];
  // Boot must run once – not again when the language (and thus errorText) changes.
  const errorTextRef = useRef(errorText);
  errorTextRef.current = errorText;
  // Bumped whenever a vault is entered, so a slower boot does not overwrite
  // an unlock that happened meanwhile (e.g. from the browser extension).
  const enterSeq = useRef(0);

  const load = useCallback(async () => {
    setScreen("loading");
    setBootError(null);
    const seq = enterSeq.current;
    try {
      const [info, loadedSettings, vaults, session] = await Promise.all([
        api.appInfo(),
        api.getSettings(),
        api.listVaults(),
        api.sessionState(),
      ]);
      setSettings(loadedSettings);
      setBoot({ info, vaults });
      if (enterSeq.current !== seq) return;
      if (session.unlocked && session.vault) {
        setVault(session.vault);
        setScreen("main");
      } else {
        setScreen(vaults.length === 0 ? "welcome" : "unlock");
      }
    } catch (err) {
      setBootError(errorTextRef.current(err));
      setScreen("error");
    }
  }, [setSettings]);

  useEffect(() => {
    void load();
  }, [load]);

  const refreshVaults = useCallback(async () => {
    const vaults = await api.listVaults();
    setBoot((b) => (b ? { ...b, vaults } : b));
    return vaults;
  }, []);

  const toUnlockOrWelcome = useCallback(async () => {
    setVault(null);
    let vaults = vaultsRef.current;
    try {
      vaults = await refreshVaults();
    } catch {
      /* keep the previous list */
    }
    setScreen(vaults.length === 0 ? "welcome" : "unlock");
  }, [refreshVaults]);

  // Backend events.
  useEffect(
    () =>
      subscribeEffect(
        events.onVaultLocked(({ reason }) => {
          void toUnlockOrWelcome();
          const minutes = settingsRef.current?.autoLockMinutes ?? 0;
          if (reason === "timeout") toast.show({ kind: "info", message: t("lock.timeoutToast", { minutes }), carry: true });
          if (reason === "system") toast.show({ kind: "info", message: t("lock.systemToast"), carry: true });
        }),
      ),
    [t, toast, toUnlockOrWelcome],
  );
  useEffect(
    () =>
      subscribeEffect(
        events.onPairingRequest((request) => {
          setPairings((list) => (list.some((r) => r.requestId === request.requestId) ? list : [...list, request]));
        }),
      ),
    [],
  );
  useEffect(
    () =>
      subscribeEffect(
        events.onPairingClosed(({ requestId }) => {
          if (!pairingsRef.current.some((r) => r.requestId === requestId)) return;
          toast.info(t("pairing.cancelled"));
          setPairings((list) => list.filter((r) => r.requestId !== requestId));
        }),
      ),
    [t, toast],
  );
  useEffect(() => subscribeEffect(events.onUnlockRequest(() => setUnlockFocus((n) => n + 1))), []);

  const updateSettings = useCallback(
    async (patch: Partial<Settings>) => {
      const previous = settingsRef.current;
      if (!previous) return false;
      const next = { ...previous, ...patch };
      setSettings(next);
      try {
        const saved = await api.saveSettings(next);
        setSettings(saved);
        return true;
      } catch (err) {
        setSettings(previous);
        toast.error(errorText(err));
        return false;
      }
    },
    [errorText, setSettings, toast],
  );

  const enterVault = useCallback(
    (info: VaultInfo) => {
      enterSeq.current += 1;
      const shown = screenRef.current === "main" ? vaultRef.current : null;
      if (shown && shown.id !== info.id) {
        // Another vault replaced the open one (the browser extension switched
        // vaults). Leave the old one like on lock: reload the page so its items
        // do not linger in the JS heap; the boot then shows the new vault (the
        // backend already remembered it as the last vault). Until then nothing
        // of either vault is shown. The mock backend has no reload: remount.
        toast.show({ kind: "info", message: t("lock.switchedToast"), carry: true });
        if (discardPageData(toast.carried)) {
          setScreen("loading");
          return;
        }
      }
      setVault(info);
      setScreen("main");
      void refreshVaults().catch(() => undefined);
      if (settingsRef.current && settingsRef.current.lastVaultId !== info.id) {
        void updateSettings({ lastVaultId: info.id });
      }
    },
    [refreshVaults, updateSettings, t, toast],
  );

  // The browser extension unlocked the vault: leave the unlock screen.
  // Subscribe once (via a ref) so no event is lost while re-subscribing.
  const enterVaultRef = useRef(enterVault);
  enterVaultRef.current = enterVault;
  useEffect(() => subscribeEffect(events.onVaultUnlocked((info) => enterVaultRef.current(info))), []);

  // Commands that change the vault carry the id of the vault this page shows
  // (`pageVaultId`). Never reset: after leaving a vault the page reloads, and
  // until then a late command of the old view must not reach a vault the
  // extension opened meanwhile.
  const shownVaultId = screen === "main" ? vault?.id : undefined;
  useLayoutEffect(() => {
    if (shownVaultId) setPageVault(shownVaultId);
  }, [shownVaultId]);

  // Leaving an open vault (lock of any kind, vault deleted, portable switch):
  // reload the page so the decrypted items etc. do not linger in the JS heap.
  const vaultShown = useRef(false);
  useEffect(() => {
    if (screen === "main") vaultShown.current = true;
    else if (vaultShown.current && (screen === "unlock" || screen === "welcome")) discardPageData(toast.carried);
  }, [screen, toast]);

  const lock = useCallback(async () => {
    try {
      await api.lockVault();
    } catch (err) {
      toast.error(errorText(err));
    }
    await toUnlockOrWelcome();
  }, [errorText, toUnlockOrWelcome, toast]);

  const ctx = useMemo<AppContextValue | null>(() => {
    if (!boot || !settings) return null;
    return {
      info: boot.info,
      setInfo: (info) => setBoot((b) => (b ? { ...b, info } : b)),
      settings,
      updateSettings,
      vaults: boot.vaults,
      refreshVaults,
      vault,
      setVault,
      enterVault,
      lock,
      showWelcome: () => setScreen("welcome"),
      showUnlock: () => void toUnlockOrWelcome(),
    };
  }, [boot, settings, updateSettings, refreshVaults, vault, enterVault, lock, toUnlockOrWelcome]);

  if (screen === "error" || (screen !== "loading" && !ctx)) {
    return (
      <div className="auth-screen">
        <div className="auth-card boot-error">
          <div className="empty-icon">
            <ServerCrash />
          </div>
          <h1 className="auth-title">{t("boot.errorTitle")}</h1>
          <p className="muted">{t("boot.errorHint")}</p>
          {bootError && <pre className="boot-error-detail selectable">{bootError}</pre>}
          <Button variant="primary" icon={<RefreshCw />} onClick={() => void load()}>
            {t("common.retry")}
          </Button>
        </div>
      </div>
    );
  }

  if (screen === "loading" || !ctx) {
    return (
      <div className="splash" aria-busy="true">
        <Logo size={56} title="Keystead" />
      </div>
    );
  }

  const currentPairing = pairings[0];

  return (
    <AppContext.Provider value={ctx}>
      {/* Files dragged onto the window: the import dialog (unlocked) or a hint. */}
      <FileDropProvider noTarget={ctx.vaults.length === 0 ? "create" : "unlock"}>
        <div className="app-frame">
          {/* New versions: above the main window and the unlock screen. */}
          {(screen === "main" || screen === "unlock") && <UpdateBanner />}
          {/* A new recovery key that was never confirmed as stored (lost to a lock / reload). */}
          {screen === "main" && vault && <RecoveryReminder key={vault.id} />}
          <div className="app-frame-body">
            {screen === "welcome" && <WelcomeScreen />}
            {screen === "unlock" && <UnlockScreen focusSignal={unlockFocus} />}
            {screen === "main" && vault && (
              <IconsProvider key={vault.id}>
                <MainScreen key={vault.id} />
              </IconsProvider>
            )}
          </div>
        </div>
        {currentPairing && (
          <PairingModal
            key={currentPairing.requestId}
            request={currentPairing}
            onDone={() => setPairings((list) => list.filter((r) => r.requestId !== currentPairing.requestId))}
          />
        )}
      </FileDropProvider>
    </AppContext.Provider>
  );
}
