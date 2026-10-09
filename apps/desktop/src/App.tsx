import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, ServerCrash } from "lucide-react";
import { api, events, subscribeEffect } from "./lib/api";
import type { AppInfo, PairingRequest, Settings, ThemeSetting, VaultInfo } from "./lib/types";
import { localPrefs } from "./lib/utils";
import { I18nProvider, useT } from "./i18n";
import { ToastProvider, useToast } from "./components/Toasts";
import { ConfirmProvider } from "./components/Confirm";
import { Button } from "./components/Controls";
import { Logo } from "./components/Logo";
import { AppContext, type AppContextValue } from "./state/app";
import { WelcomeScreen } from "./screens/WelcomeScreen";
import { UnlockScreen } from "./screens/UnlockScreen";
import { MainScreen } from "./screens/main/MainScreen";
import { PairingModal } from "./screens/PairingModal";

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
          <AppRoot settings={settings} setSettings={setSettings} />
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
  const [pairings, setPairings] = useState<PairingRequest[]>([]);
  const [unlockFocus, setUnlockFocus] = useState(0);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const vaultsRef = useRef<VaultInfo[]>([]);
  vaultsRef.current = boot?.vaults ?? [];
  // Boot must run once – not again when the language (and thus errorText) changes.
  const errorTextRef = useRef(errorText);
  errorTextRef.current = errorText;

  const load = useCallback(async () => {
    setScreen("loading");
    setBootError(null);
    try {
      const [info, loadedSettings, vaults, session] = await Promise.all([
        api.appInfo(),
        api.getSettings(),
        api.listVaults(),
        api.sessionState(),
      ]);
      setSettings(loadedSettings);
      setBoot({ info, vaults });
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
          if (reason === "timeout") toast.info(t("lock.timeoutToast", { minutes }));
          if (reason === "system") toast.info(t("lock.systemToast"));
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
      setVault(info);
      setScreen("main");
      void refreshVaults().catch(() => undefined);
      if (settingsRef.current && settingsRef.current.lastVaultId !== info.id) {
        void updateSettings({ lastVaultId: info.id });
      }
    },
    [refreshVaults, updateSettings],
  );

  // The browser extension unlocked the vault: leave the unlock screen.
  useEffect(() => subscribeEffect(events.onVaultUnlocked((info) => enterVault(info))), [enterVault]);

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
      {screen === "welcome" && <WelcomeScreen />}
      {screen === "unlock" && <UnlockScreen focusSignal={unlockFocus} />}
      {screen === "main" && vault && <MainScreen key={vault.id} />}
      {currentPairing && (
        <PairingModal
          key={currentPairing.requestId}
          request={currentPairing}
          onDone={() => setPairings((list) => list.filter((r) => r.requestId !== currentPairing.requestId))}
        />
      )}
    </AppContext.Provider>
  );
}
