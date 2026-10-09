import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import {
  Check,
  CircleCheck,
  Copy,
  Download,
  ExternalLink,
  FileDown,
  FileUp,
  FolderOpen,
  RefreshCw,
  CircleArrowUp,
  CircleAlert,
  Globe,
  Info,
  KeyRound,
  LifeBuoy,
  Monitor,
  Moon,
  Plug,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
  Upload,
  LockKeyhole,
  Wrench,
  Zap,
} from "lucide-react";
import { IS_MOCK, api } from "../../lib/api";
import type {
  BrowserInfo,
  BrowserStatus,
  Language,
  ThemeSetting,
  UpdateChannel,
  UpdateInfo,
} from "../../lib/types";
import { useT } from "../../i18n";
import { useApp, useCopy } from "../../state/app";
import { useToast } from "../../components/Toasts";
import { useConfirm } from "../../components/Confirm";
import { Button, Segmented, Select, Switch } from "../../components/Controls";
import { useOpenImport } from "../../components/import/ImportDialog";
import { extensionDownloadUrl } from "../../lib/links";
import { Logo } from "../../components/Logo";
import { useUpdateErrorText } from "../../components/UpdateBanner";
import { useUpdate } from "../../state/update";
import { ChangeMasterPasswordDialog, DeleteVaultDialog, ExportDialog, RecoveryKeyDialog } from "./dialogs";
import { WebsiteIconsSetting } from "./WebsiteIconsSetting";

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

function SettingsSection({
  id,
  title,
  description,
  children,
}: {
  id: string;
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section id={`settings-${id}`} className="settings-section" aria-labelledby={`settings-${id}-title`}>
      <div className="settings-section-head">
        <h2 id={`settings-${id}-title`}>{title}</h2>
        {description && <p>{description}</p>}
      </div>
      <div className="card settings-card">{children}</div>
    </section>
  );
}

export function SettingRow({
  title,
  description,
  children,
  htmlFor,
  stacked,
}: {
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  htmlFor?: string;
  stacked?: boolean;
}) {
  return (
    <div className={`setting-row ${stacked ? "stacked" : ""}`}>
      <div className="setting-text">
        {htmlFor ? (
          <label className="setting-title" htmlFor={htmlFor}>
            {title}
          </label>
        ) : (
          <div className="setting-title">{title}</div>
        )}
        {description && <div className="setting-desc">{description}</div>}
      </div>
      {children && <div className="setting-control">{children}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

function GeneralSection() {
  const { t } = useT();
  const { settings, updateSettings } = useApp();
  const langId = useId();
  return (
    <SettingsSection id="general" title={t("settings.general")} description={t("settings.generalDesc")}>
      <SettingRow title={t("settings.theme")} description={t("settings.themeDesc")}>
        <Segmented<ThemeSetting>
          ariaLabel={t("settings.theme")}
          value={settings.theme}
          onChange={(theme) => void updateSettings({ theme })}
          options={[
            { value: "system", label: t("settings.themeSystem"), icon: <Monitor /> },
            { value: "light", label: t("settings.themeLight"), icon: <Sun /> },
            { value: "dark", label: t("settings.themeDark"), icon: <Moon /> },
          ]}
        />
      </SettingRow>
      <SettingRow title={t("settings.language")} description={t("settings.languageDesc")} htmlFor={langId}>
        <Select<Language>
          id={langId}
          value={settings.language}
          onChange={(language) => void updateSettings({ language })}
          options={[
            { value: "de", label: "Deutsch" },
            { value: "en", label: "English" },
          ]}
          className="setting-select"
        />
      </SettingRow>
      <SettingRow title={t("settings.minimizeToTray")} description={t("settings.minimizeToTrayDesc")}>
        <Switch
          checked={settings.minimizeToTray}
          onChange={(minimizeToTray) => void updateSettings({ minimizeToTray })}
          label={t("settings.minimizeToTray")}
        />
      </SettingRow>
      <SettingRow title={t("settings.startInTray")} description={t("settings.startInTrayDesc")}>
        <Switch checked={settings.startInTray} onChange={(startInTray) => void updateSettings({ startInTray })} label={t("settings.startInTray")} />
      </SettingRow>
      <WebsiteIconsSetting />
    </SettingsSection>
  );
}

function SecuritySection() {
  const { t, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const { settings, updateSettings, vault, setVault } = useApp();
  const autoLockId = useId();
  const clipboardId = useId();
  const [dialog, setDialog] = useState<"password" | "recovery" | null>(null);
  const hasKey = vault?.hasRecoveryKey ?? false;

  const autoLockOptions = [1, 5, 15, 30, 60, 240, 0];
  if (!autoLockOptions.includes(settings.autoLockMinutes)) autoLockOptions.unshift(settings.autoLockMinutes);
  const clipboardOptions = [10, 20, 30, 60, 120, 0];
  if (!clipboardOptions.includes(settings.clipboardClearSeconds)) clipboardOptions.unshift(settings.clipboardClearSeconds);

  const minutesLabel = (m: number) =>
    m === 0 ? t("settings.never") : m >= 60 && m % 60 === 0 ? t("settings.hours", { n: m / 60 }) : t("settings.minutes", { n: m });

  const removeKey = async () => {
    const ok = await confirm({
      title: t("recovery.removeTitle"),
      message: t("recovery.removeText"),
      confirmLabel: t("recovery.remove"),
      tone: "danger",
    });
    if (!ok || !vault) return;
    try {
      await api.removeRecoveryKey();
      setVault({ ...vault, hasRecoveryKey: false });
      toast.success(t("recovery.removed"));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  return (
    <SettingsSection id="security" title={t("settings.security")} description={t("settings.securityDesc")}>
      <SettingRow title={t("settings.autoLock")} description={t("settings.autoLockDesc")} htmlFor={autoLockId}>
        <Select
          id={autoLockId}
          value={String(settings.autoLockMinutes)}
          onChange={(v) => void updateSettings({ autoLockMinutes: Number(v) })}
          options={autoLockOptions.map((m) => ({ value: String(m), label: minutesLabel(m) }))}
          className="setting-select"
        />
      </SettingRow>
      <SettingRow title={t("settings.lockOnSystemLock")} description={t("settings.lockOnSystemLockDesc")}>
        <Switch
          checked={settings.lockOnSystemLock}
          onChange={(lockOnSystemLock) => void updateSettings({ lockOnSystemLock })}
          label={t("settings.lockOnSystemLock")}
        />
      </SettingRow>
      <SettingRow title={t("settings.clipboard")} description={t("settings.clipboardDesc")} htmlFor={clipboardId}>
        <Select
          id={clipboardId}
          value={String(settings.clipboardClearSeconds)}
          onChange={(v) => void updateSettings({ clipboardClearSeconds: Number(v) })}
          options={clipboardOptions.map((s) => ({
            value: String(s),
            label: s === 0 ? t("settings.never") : t("settings.seconds", { n: s }),
          }))}
          className="setting-select"
        />
      </SettingRow>
      <SettingRow title={t("settings.masterPassword")} description={t("settings.masterPasswordDesc")}>
        <Button variant="secondary" icon={<KeyRound />} onClick={() => setDialog("password")}>
          {t("settings.changeMaster")}
        </Button>
      </SettingRow>
      <SettingRow
        title={
          <span className="setting-title-with-chip">
            {t("settings.recoveryKey")}
            <span className={`chip ${hasKey ? "success" : "warning"}`}>
              {hasKey ? t("settings.recoveryActive") : t("settings.recoveryMissing")}
            </span>
          </span>
        }
        description={hasKey ? t("settings.recoveryDescActive") : t("settings.recoveryDescMissing")}
      >
        <div className="setting-buttons">
          {hasKey && (
            <Button variant="danger-ghost" onClick={() => void removeKey()}>
              {t("recovery.remove")}
            </Button>
          )}
          <Button variant={hasKey ? "secondary" : "primary"} icon={<LifeBuoy />} onClick={() => setDialog("recovery")}>
            {hasKey ? t("recovery.replace") : t("recovery.create")}
          </Button>
        </div>
      </SettingRow>

      {dialog === "password" && <ChangeMasterPasswordDialog onClose={() => setDialog(null)} />}
      {dialog === "recovery" && (
        <RecoveryKeyDialog
          replacing={hasKey}
          onClose={() => setDialog(null)}
          onCreated={() => vault && setVault({ ...vault, hasRecoveryKey: true })}
        />
      )}
    </SettingsSection>
  );
}

function GuideStep({ n, done, title, children }: { n: number; done: boolean; title: string; children: ReactNode }) {
  const { t } = useT();
  return (
    <li className={`guide-step ${done ? "done" : ""}`}>
      <span className="guide-num" aria-label={done ? t("browser.stepDone", { n }) : undefined}>
        {done ? <Check /> : n}
      </span>
      <div className="guide-body">
        <strong>{title}</strong>
        {children}
      </div>
    </li>
  );
}

function BrowserRow({ browser, busy, onToggle }: { browser: BrowserInfo; busy: boolean; onToggle: () => void }) {
  const { t } = useT();
  return (
    <div className="browser-row">
      <span className="browser-icon">
        <Globe />
      </span>
      <div className="browser-text">
        <div className="browser-name">{browser.name}</div>
        <div className="browser-status">
          {browser.registered ? (
            <span className="status-text ok">
              <CircleCheck />
              {t("browser.registered")}
            </span>
          ) : (
            <span className="subtle">{t("browser.notRegistered")}</span>
          )}
        </div>
      </div>
      <Button variant={browser.registered ? "ghost" : "primary"} size="sm" loading={busy} onClick={onToggle}>
        {browser.registered ? t("browser.disconnect") : t("browser.connect")}
      </Button>
    </div>
  );
}

function BrowserSection() {
  const { t, errorText, formatDate, formatRelative } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const copy = useCopy();
  const { settings, updateSettings, info } = useApp();
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.browserStatus());
    } catch (err) {
      toast.error(errorText(err));
    }
  }, [errorText, toast]);

  useEffect(() => {
    void refresh();
    // Pairings are approved elsewhere (modal) – keep the list current.
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const toggleBrowser = async (browser: BrowserInfo) => {
    setBusy(browser.id);
    try {
      setStatus(browser.registered ? await api.unregisterBrowsers([browser.id]) : await api.registerBrowsers([browser.id]));
      toast.success(browser.registered ? t("browser.disconnected", { name: browser.name }) : t("browser.connected", { name: browser.name }));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const revoke = async (clientId: string, name: string) => {
    const ok = await confirm({
      title: t("browser.revokeTitle"),
      message: t("browser.revokeText", { name }),
      confirmLabel: t("browser.revoke"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      setStatus(await api.revokeClient(clientId));
      toast.success(t("browser.revoked", { name }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const toggleIntegration = async (enabled: boolean) => {
    if (await updateSettings({ browserIntegration: enabled })) void refresh();
  };

  const openExtensionFolder = async () => {
    try {
      await api.openExtensionDir();
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const detected = status?.browsers.filter((b) => b.detected) ?? [];
  const missing = status?.browsers.filter((b) => !b.detected) ?? [];
  const extensionId = status?.extensionId || info.extensionId;
  const running = status?.serverRunning ?? false;
  const anyRegistered = detected.some((b) => b.registered);
  const clients = status?.clients ?? [];

  return (
    <SettingsSection id="browser" title={t("settings.browser")} description={t("settings.browserDesc")}>
      <SettingRow
        title={t("browser.enable")}
        description={
          <span className="bridge-status">
            <span className={`status-dot ${status ? (running ? "on" : "off") : ""}`} aria-hidden />
            {status ? (running ? t("browser.serverRunning") : t("browser.serverStopped")) : t("common.loading")}
          </span>
        }
      >
        <Switch checked={settings.browserIntegration} onChange={(v) => void toggleIntegration(v)} label={t("browser.enable")} />
      </SettingRow>

      <div className={`setting-row stacked ${settings.browserIntegration ? "" : "dimmed"}`}>
        <ol className="guide-steps">
          <GuideStep n={1} done={anyRegistered} title={t("browser.step1Title")}>
            <p>{t("browser.step1Text")}</p>
            <div className="browser-list">
              {status && detected.length === 0 && <div className="browser-empty subtle">{t("browser.noneDetected")}</div>}
              {detected.map((b) => (
                <BrowserRow key={b.id} browser={b} busy={busy === b.id} onToggle={() => void toggleBrowser(b)} />
              ))}
              {missing.length > 0 && (
                <div className="browser-missing">{t("browser.notInstalled", { names: missing.map((b) => b.name).join(", ") })}</div>
              )}
            </div>
          </GuideStep>

          <GuideStep n={2} done={clients.length > 0} title={t("browser.step2Title")}>
            <p>
              {t("browser.step2TextA")} <code>chrome://extensions</code> {t("browser.step2TextB")} <code>edge://extensions</code>
              {t("browser.step2TextC")}
            </p>
            <div className="ext-folder">
              <code className="ext-folder-path selectable" aria-label={t("browser.folderLabel")}>
                {status?.extensionDir ?? t("common.loading")}
              </code>
              <div className="ext-folder-actions">
                <Button size="sm" variant="secondary" icon={<FolderOpen />} onClick={() => void openExtensionFolder()}>
                  {t("browser.openFolder")}
                </Button>
                <Button
                  size="sm"
                  variant="secondary"
                  icon={<Copy />}
                  disabled={!status?.extensionDir}
                  onClick={() => {
                    if (status?.extensionDir) void copy(status.extensionDir, { label: t("browser.folderLabel"), sensitive: false });
                  }}
                >
                  {t("browser.copyPath")}
                </Button>
                {/* Just in case the folder does not work: the same extension from the release. */}
                <a
                  className="ext-download"
                  href={extensionDownloadUrl(info.version)}
                  target="_blank"
                  rel="noopener noreferrer"
                  title={t("browser.downloadTitle")}
                >
                  <Download aria-hidden />
                  {t("browser.download")}
                  <ExternalLink className="ext-download-external" aria-hidden />
                </a>
              </div>
              <p className="subtle">{t("browser.step2Folder")}</p>
            </div>
            <div className="ext-id">
              <span className="ext-id-label">{t("browser.extensionId")}</span>
              <code className="selectable">{extensionId}</code>
              <button
                type="button"
                className="icon-btn sm"
                onClick={() => void copy(extensionId, { label: t("browser.extensionId"), sensitive: false })}
                aria-label={t("common.copyNamed", { what: t("browser.extensionId") })}
                title={t("common.copy")}
              >
                <Copy />
              </button>
            </div>
          </GuideStep>

          <GuideStep n={3} done={clients.length > 0} title={t("browser.step3Title")}>
            <p>{t("browser.step3Text")}</p>
            {clients.length > 0 && (
              <ul className="client-list" aria-label={t("browser.clients")}>
                {clients.map((client) => (
                  <li key={client.id}>
                    <span className="browser-icon small">
                      <Plug />
                    </span>
                    <div className="client-text">
                      <div className="client-name">{client.name}</div>
                      <div className="client-meta">
                        {t("browser.pairedOn", { date: formatDate(client.createdAt) })} ·{" "}
                        {t("browser.lastSeen", { when: formatRelative(client.lastSeenAt) })}
                      </div>
                    </div>
                    <Button variant="danger-ghost" size="sm" onClick={() => void revoke(client.id, client.name)}>
                      {t("browser.revoke")}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            {IS_MOCK && (
              <div className="demo-tools">
                <span className="chip accent">{t("settings.demo")}</span>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<Zap />}
                  onClick={() => {
                    void import("../../lib/mock").then((m) => m.mockTriggerPairing());
                  }}
                >
                  {t("browser.simulatePairing")}
                </Button>
              </div>
            )}
          </GuideStep>
        </ol>
      </div>
    </SettingsSection>
  );
}

function ImportExportSection() {
  const { t } = useT();
  const { vault } = useApp();
  const openImport = useOpenImport();
  const [exportOpen, setExportOpen] = useState(false);

  return (
    <SettingsSection id="data" title={t("settings.data")} description={t("settings.dataDesc")}>
      <SettingRow
        title={
          <>
            <Upload size={15} className="inline-icon" />
            {t("import.title")}
          </>
        }
        description={
          <>
            {t("import.desc")} {t("import.dropTip")}
          </>
        }
      >
        <Button variant="secondary" icon={<FileUp />} onClick={() => openImport?.()} disabled={!openImport}>
          {t("import.open")}
        </Button>
      </SettingRow>

      <SettingRow
        title={
          <>
            <Download size={15} className="inline-icon" />
            {t("export.title")}
          </>
        }
        description={t("export.desc")}
      >
        <Button variant="secondary" icon={<FileDown />} onClick={() => setExportOpen(true)}>
          {t("export.open")}
        </Button>
      </SettingRow>

      {exportOpen && vault && <ExportDialog vaultName={vault.name} onClose={() => setExportOpen(false)} />}
    </SettingsSection>
  );
}

function AdvancedSection() {
  const { t, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const { info, setInfo, showUnlock } = useApp();
  const [portableBusy, setPortableBusy] = useState(false);

  const togglePortable = async (enabled: boolean) => {
    const ok = await confirm({
      title: enabled ? t("data.portableOnTitle") : t("data.portableOffTitle"),
      message: enabled ? t("data.portableOnText") : t("data.portableOffText"),
      confirmLabel: enabled ? t("data.portableOn") : t("data.portableOff"),
    });
    if (!ok) return;
    setPortableBusy(true);
    try {
      setInfo(await api.setPortableMode(enabled));
      toast.show({ kind: "success", message: enabled ? t("data.portableEnabled") : t("data.portableDisabled"), carry: true });
    } catch (err) {
      toast.show({ kind: "error", message: errorText(err), carry: true });
    } finally {
      setPortableBusy(false);
      // The backend closes the vault to move its file – also when the move
      // fails afterwards. Never stay on the item view of a closed vault.
      try {
        const session = await api.sessionState();
        if (!session.unlocked) showUnlock();
      } catch {
        /* the `vault://locked` event navigates as well */
      }
    }
  };

  const openFolder = async () => {
    try {
      await api.openDataDir();
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  return (
    <SettingsSection id="advanced" title={t("settings.advanced")} description={t("settings.advancedDesc")}>
      <SettingRow title={t("data.folder")} description={<code className="path selectable">{info.dataDir}</code>}>
        <Button variant="secondary" icon={<FolderOpen />} onClick={() => void openFolder()}>
          {t("data.openFolder")}
        </Button>
      </SettingRow>
      <SettingRow title={t("data.portable")} description={t("data.portableDesc")}>
        <Switch checked={info.portable} onChange={(v) => void togglePortable(v)} label={t("data.portable")} disabled={portableBusy} />
      </SettingRow>
      <SettingRow title={t("terminal.title")} description={t("terminal.desc")}>
        <Button
          variant="secondary"
          icon={<ExternalLink />}
          onClick={() => {
            api.openTerminal().catch((err: unknown) => toast.error(errorText(err)));
          }}
        >
          {t("terminal.open")}
        </Button>
      </SettingRow>
    </SettingsSection>
  );
}

function VaultSection() {
  const { t, errorText } = useT();
  const toast = useToast();
  const { vault, setVault, refreshVaults, showUnlock } = useApp();
  const nameId = useId();
  const [name, setName] = useState(vault?.name ?? "");
  const [busy, setBusy] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const changed = name.trim() !== "" && name.trim() !== vault?.name;

  const rename = async () => {
    if (!changed) return;
    setBusy(true);
    try {
      const info = await api.renameVault(name.trim());
      setVault(info);
      setName(info.name);
      void refreshVaults().catch(() => undefined);
      toast.success(t("danger.renamed", { name: info.name }));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsSection id="vault" title={t("settings.vault")} description={t("settings.vaultDesc", { name: vault?.name ?? "" })}>
      <SettingRow title={t("danger.rename")} description={t("danger.renameDesc")} htmlFor={nameId}>
        <form
          className="rename-form"
          onSubmit={(e) => {
            e.preventDefault();
            void rename();
          }}
        >
          <input id={nameId} className="input" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} />
          <Button type="submit" variant="secondary" disabled={!changed} loading={busy}>
            {t("danger.renameButton")}
          </Button>
        </form>
      </SettingRow>
      <SettingRow title={t("danger.delete")} description={t("danger.deleteDesc")}>
        <Button variant="danger-outline" onClick={() => setDeleteOpen(true)}>
          {t("danger.deleteOpen")}
        </Button>
      </SettingRow>
      {deleteOpen && vault && (
        <DeleteVaultDialog
          vault={vault}
          onClose={() => setDeleteOpen(false)}
          onDeleted={() => {
            setDeleteOpen(false);
            toast.success(t("danger.deleted", { name: vault.name }));
            showUnlock();
          }}
        />
      )}
    </SettingsSection>
  );
}

type CheckResult = { kind: "uptodate" | "none"; info: UpdateInfo } | { kind: "available"; info: UpdateInfo } | { kind: "error"; text: string };

function UpdateCheckResult({ result }: { result: CheckResult }) {
  const { t } = useT();
  const update = useUpdate();
  if (result.kind === "error") {
    return (
      <div className="update-check-result error" role="alert">
        <CircleAlert />
        <span>{result.text}</span>
      </div>
    );
  }
  if (result.kind === "available") {
    const busy = update.phase === "downloading" || update.phase === "restarting";
    return (
      <div className="update-check-result available" role="status">
        <CircleArrowUp />
        <div>
          <div>{t("update.foundAvailable", { version: result.info.version ?? "" })}</div>
          {result.info.canInstall ? (
            <Button size="sm" variant="primary" icon={<Download />} loading={busy} onClick={() => void update.install()}>
              {t("update.installNow")}
            </Button>
          ) : (
            <Button size="sm" variant="primary" icon={<ExternalLink />} onClick={() => void update.install()}>
              {t("update.download")}
            </Button>
          )}
        </div>
      </div>
    );
  }
  return (
    <div className="update-check-result ok" role="status">
      <CircleCheck />
      <span>
        {result.kind === "uptodate" ? t("update.upToDate", { version: result.info.currentVersion }) : t("update.noRelease")}
      </span>
    </div>
  );
}

function AboutSection() {
  const { t } = useT();
  const { info, settings, updateSettings } = useApp();
  const update = useUpdate();
  const updateErrorText = useUpdateErrorText();
  const channelId = useId();
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<CheckResult | null>(null);
  const platform = info.platform === "windows" ? "Windows" : info.platform === "macos" ? "macOS" : "Linux";

  const check = async () => {
    setChecking(true);
    setResult(null);
    try {
      const found = await update.check();
      setResult({ kind: found.available ? "available" : found.version ? "uptodate" : "none", info: found });
    } catch (err) {
      setResult({ kind: "error", text: updateErrorText(err) });
    } finally {
      setChecking(false);
    }
  };

  const changeChannel = async (updateChannel: UpdateChannel) => {
    if (updateChannel === settings.updateChannel) return;
    // The backend checks the new channel in the background.
    if (await updateSettings({ updateChannel })) {
      update.reset();
      setResult(null);
    }
  };

  return (
    <SettingsSection id="about" title={t("settings.about")}>
      <div className="about">
        <Logo size={44} />
        <div>
          <div className="about-name">
            Keystead <span className="about-version">{info.version}</span>
            {IS_MOCK && <span className="chip accent">{t("settings.demo")}</span>}
          </div>
          <p className="muted">{t("about.tagline")}</p>
          <p className="subtle about-meta">
            {platform} · {info.portable ? t("about.portable") : t("about.installed")}
          </p>
        </div>
      </div>
      <SettingRow
        title={t("update.checkTitle")}
        description={result ? <UpdateCheckResult result={result} /> : t("update.checkDesc", { version: info.version })}
      >
        <Button variant="secondary" icon={<RefreshCw />} loading={checking} onClick={() => void check()}>
          {t("update.checkNow")}
        </Button>
      </SettingRow>
      <SettingRow title={t("update.auto")} description={t("update.autoDesc")}>
        <Switch
          checked={settings.updateCheck}
          onChange={(updateCheck) => void updateSettings({ updateCheck })}
          label={t("update.auto")}
        />
      </SettingRow>
      <SettingRow title={t("update.channel")} description={t("update.channelDesc")} htmlFor={channelId}>
        <Select<UpdateChannel>
          id={channelId}
          value={settings.updateChannel}
          onChange={(channel) => void changeChannel(channel)}
          options={[
            { value: "beta", label: t("update.channelBeta") },
            { value: "stable", label: t("update.channelStable") },
          ]}
          className="setting-select"
        />
      </SettingRow>
    </SettingsSection>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/** Old section ids (deep links from elsewhere) → current ones. */
const SECTION_ALIASES: Record<string, string> = { appearance: "general", terminal: "advanced", danger: "vault" };

export function SettingsPage({ initialSection }: { initialSection?: string }) {
  const { t } = useT();
  const scrollRef = useRef<HTMLElement>(null);
  /** Section picked in the list: keeps it highlighted until the user scrolls by hand. */
  const pinned = useRef<string | null>(null);
  const initial = initialSection ? SECTION_ALIASES[initialSection] ?? initialSection : undefined;
  const [active, setActive] = useState(initial ?? "general");

  const sections: { id: string; label: string; icon: ReactNode }[] = [
    { id: "general", label: t("settings.general"), icon: <SlidersHorizontal /> },
    { id: "security", label: t("settings.security"), icon: <ShieldCheck /> },
    { id: "browser", label: t("settings.browserShort"), icon: <Plug /> },
    { id: "data", label: t("settings.data"), icon: <Upload /> },
    { id: "advanced", label: t("settings.advanced"), icon: <Wrench /> },
    { id: "vault", label: t("settings.vault"), icon: <LockKeyhole /> },
    { id: "about", label: t("settings.aboutShort"), icon: <Info /> },
  ];

  const scrollTo = (id: string, smooth: boolean) => {
    const root = scrollRef.current;
    const el = document.getElementById(`settings-${id}`);
    if (!root || !el) return;
    // Leave room for the sticky section bar (narrow windows).
    const toc = root.querySelector<HTMLElement>(".settings-toc");
    const offset = toc && getComputedStyle(toc).flexDirection === "row" ? toc.offsetHeight + 16 : 24;
    const top = el.getBoundingClientRect().top - root.getBoundingClientRect().top + root.scrollTop - offset;
    root.scrollTo({ top, behavior: smooth ? "smooth" : "auto" });
  };

  useEffect(() => {
    if (initial) {
      scrollTo(initial, false);
      setActive(initial);
      pinned.current = initial;
    }
  }, [initial]);

  // Highlight the section currently at the top of the scroll area.
  useEffect(() => {
    const root = scrollRef.current;
    if (!root) return;
    const unpin = () => {
      pinned.current = null;
    };
    const onScroll = () => {
      if (pinned.current) return;
      const top = root.getBoundingClientRect().top;
      let current = "general";
      for (const el of root.querySelectorAll<HTMLElement>(".settings-section")) {
        if (el.getBoundingClientRect().top - top <= 160) current = el.id.replace("settings-", "");
      }
      if (root.scrollTop + root.clientHeight >= root.scrollHeight - 4) current = "about";
      setActive(current);
    };
    root.addEventListener("scroll", onScroll, { passive: true });
    root.addEventListener("wheel", unpin, { passive: true });
    root.addEventListener("pointerdown", unpin);
    root.addEventListener("keydown", unpin);
    return () => {
      root.removeEventListener("scroll", onScroll);
      root.removeEventListener("wheel", unpin);
      root.removeEventListener("pointerdown", unpin);
      root.removeEventListener("keydown", unpin);
    };
  }, []);

  // Keep the active tab visible in the horizontal section bar.
  useEffect(() => {
    const tab = scrollRef.current?.querySelector<HTMLElement>(`.settings-toc-item[data-id="${active}"]`);
    const bar = tab?.parentElement;
    if (!tab || !bar || bar.scrollWidth <= bar.clientWidth) return;
    const left = tab.offsetLeft - bar.offsetLeft;
    if (left < bar.scrollLeft || left + tab.offsetWidth > bar.scrollLeft + bar.clientWidth) {
      bar.scrollTo({ left: Math.max(0, left - 24), behavior: "smooth" });
    }
  }, [active]);

  return (
    <main className="page settings-page" ref={scrollRef} aria-labelledby="settings-title">
      <div className="settings-layout">
        <header className="page-header settings-header">
          <h1 id="settings-title">{t("settings.title")}</h1>
          <p>{t("settings.subtitle")}</p>
        </header>
        <nav className="settings-toc" aria-label={t("settings.sections")}>
          {sections.map((s) => (
            <button
              key={s.id}
              type="button"
              data-id={s.id}
              className={`settings-toc-item ${active === s.id ? "active" : ""}`}
              aria-current={active === s.id ? "true" : undefined}
              onClick={(e) => {
                e.stopPropagation();
                scrollTo(s.id, true);
                setActive(s.id);
                pinned.current = s.id;
              }}
              onPointerDown={(e) => e.stopPropagation()}
            >
              {s.icon}
              {s.label}
            </button>
          ))}
        </nav>
        <div className="settings-content">
          <GeneralSection />
          <SecuritySection />
          <BrowserSection />
          <ImportExportSection />
          <AdvancedSection />
          <VaultSection />
          <AboutSection />
        </div>
      </div>
    </main>
  );
}
