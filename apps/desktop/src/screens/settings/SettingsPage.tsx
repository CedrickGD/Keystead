import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import {
  Check,
  CircleCheck,
  Copy,
  Download,
  ExternalLink,
  FileDown,
  FileUp,
  FingerprintPattern,
  FolderOpen,
  FolderSync,
  RefreshCw,
  Server,
  CircleArrowUp,
  CircleAlert,
  Globe,
  Info,
  KeyRound,
  LifeBuoy,
  Monitor,
  Moon,
  Paperclip,
  Plug,
  ShieldCheck,
  SlidersHorizontal,
  Smartphone,
  Sun,
  Upload,
  LockKeyhole,
  TriangleAlert,
  Unlink,
  Wrench,
  Zap,
} from "lucide-react";
import { ApiError, IS_MOCK, api, openExternal } from "../../lib/api";
import type {
  AppInfo,
  BrowserInfo,
  BrowserStatus,
  ItemListEntry,
  Language,
  ThemeSetting,
  UpdateChannel,
  UpdateInfo,
  VaultInfo,
} from "../../lib/types";
import { formatBytes } from "../../lib/utils";
import { useT } from "../../i18n";
import { useApp, useCopy } from "../../state/app";
import { useToast } from "../../components/Toasts";
import { useConfirm } from "../../components/Confirm";
import { Button, Segmented, Select, Switch } from "../../components/Controls";
import { useOpenImport } from "../../components/import/ImportDialog";
import { RELEASES_URL, extensionDownloadUrl, releasesUrl } from "../../lib/links";
import { isPhonePlatform } from "../../lib/phone";
import { clearRecoveryUnconfirmed } from "../../lib/recoveryMarker";
import { Logo } from "../../components/Logo";
import { useUpdateErrorText } from "../../components/UpdateBanner";
import { useUpdate } from "../../state/update";
import { ChangeMasterPasswordDialog, DeleteVaultDialog, ExportDialog, RecoveryKeyDialog } from "./dialogs";
import { WebsiteIconsSetting } from "./WebsiteIconsSetting";
import { useFolderSync } from "../../state/sync";
import { useDeviceUnlock } from "../../state/deviceUnlock";
import { useMirror } from "../../state/mirror";
import {
  PhoneServerDialog,
  RelinkBanner,
  SyncFileList,
  SyncSteps,
  folderLost,
  needsRelink,
  serverNeedsPassword,
  useRelativeTimeTick,
  useSkippedText,
  useSyncErrorText,
} from "../../components/FolderSync";
import { WebDavDialog } from "../../components/WebDavForm";

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
  const { info, settings, updateSettings } = useApp();
  const langId = useId();
  // Tray and website icons do not exist on the phone (no tray, no network).
  const desktop = !isPhonePlatform(info.platform);
  return (
    <SettingsSection id="general" title={t("settings.general")} description={desktop ? t("settings.generalDesc") : t("settings.generalDescPhone")}>
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
      {desktop && (
        <>
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
        </>
      )}
    </SettingsSection>
  );
}

function SecuritySection() {
  const { t, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const { info, settings, updateSettings, vault, setVault } = useApp();
  const autoLockId = useId();
  const backgroundId = useId();
  const clipboardId = useId();
  const [dialog, setDialog] = useState<"password" | "recovery" | null>(null);
  const hasKey = vault?.hasRecoveryKey ?? false;
  // The background lock exists on the phone only (the desktop ignores the
  // setting); the Windows session lock only on the desktop.
  const phone = isPhonePlatform(info.platform);
  // Master password and recovery key: PC-only (the phone edits items only).
  const readOnly = info.readOnly || phone;

  const autoLockOptions = [1, 5, 15, 30, 60, 240, 0];
  if (!autoLockOptions.includes(settings.autoLockMinutes)) autoLockOptions.unshift(settings.autoLockMinutes);
  const clipboardOptions = [10, 20, 30, 60, 120, 0];
  if (!clipboardOptions.includes(settings.clipboardClearSeconds)) clipboardOptions.unshift(settings.clipboardClearSeconds);
  // Seconds; null = never.
  const backgroundOptions: (number | null)[] = [0, 60, 300, null];
  if (!backgroundOptions.includes(settings.lockInBackgroundSeconds)) backgroundOptions.unshift(settings.lockInBackgroundSeconds);

  const minutesLabel = (m: number) =>
    m === 0 ? t("settings.never") : m >= 60 && m % 60 === 0 ? t("settings.hours", { n: m / 60 }) : t("settings.minutes", { n: m });
  const backgroundValue = (s: number | null) => (s === null ? "never" : String(s));
  const backgroundLabel = (s: number | null) =>
    s === null
      ? t("settings.never")
      : s === 0
        ? t("settings.immediately")
        : s % 60 === 0
          ? minutesLabel(s / 60)
          : t("settings.seconds", { n: s });

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
      // No key, nothing left to confirm.
      clearRecoveryUnconfirmed(vault.id);
      setVault({ ...vault, hasRecoveryKey: false });
      toast.success(t("recovery.removed"));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  return (
    <SettingsSection
      id="security"
      title={t("settings.security")}
      description={readOnly ? t("settings.securityDescPhone") : t("settings.securityDesc")}
    >
      <SettingRow title={t("settings.autoLock")} description={t("settings.autoLockDesc")} htmlFor={autoLockId}>
        <Select
          id={autoLockId}
          value={String(settings.autoLockMinutes)}
          onChange={(v) => void updateSettings({ autoLockMinutes: Number(v) })}
          options={autoLockOptions.map((m) => ({ value: String(m), label: minutesLabel(m) }))}
          className="setting-select"
        />
      </SettingRow>
      {phone && (
        <SettingRow title={t("settings.lockInBackground")} description={t("settings.lockInBackgroundDesc")} htmlFor={backgroundId}>
          <Select
            id={backgroundId}
            value={backgroundValue(settings.lockInBackgroundSeconds)}
            onChange={(v) => void updateSettings({ lockInBackgroundSeconds: v === "never" ? null : Number(v) })}
            options={backgroundOptions.map((s) => ({ value: backgroundValue(s), label: backgroundLabel(s) }))}
            className="setting-select"
          />
        </SettingRow>
      )}
      {phone && vault && <FingerprintSetting vault={vault} />}
      {!phone && (
        <SettingRow title={t("settings.lockOnSystemLock")} description={t("settings.lockOnSystemLockDesc")}>
          <Switch
            checked={settings.lockOnSystemLock}
            onChange={(lockOnSystemLock) => void updateSettings({ lockOnSystemLock })}
            label={t("settings.lockOnSystemLock")}
          />
        </SettingRow>
      )}
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
      {!readOnly && (
        <SettingRow title={t("settings.masterPassword")} description={t("settings.masterPasswordDesc")}>
          <Button variant="secondary" icon={<KeyRound />} onClick={() => setDialog("password")}>
            {t("settings.changeMaster")}
          </Button>
        </SettingRow>
      )}
      {!readOnly && (
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
      )}

      {dialog === "password" && <ChangeMasterPasswordDialog hasRecoveryKey={hasKey} onClose={() => setDialog(null)} />}
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

/**
 * Phone: "Mit Fingerabdruck entsperren" for the open vault – sets it up (the
 * system prompt confirms it) or removes it. See state/deviceUnlock.tsx.
 */
function FingerprintSetting({ vault }: { vault: VaultInfo }) {
  const { t } = useT();
  const deviceUnlock = useDeviceUnlock();
  const [busy, setBusy] = useState(false);
  if (!deviceUnlock) return null;
  const { status } = deviceUnlock;
  const setUp = Boolean(status?.enabled && status.vaults.includes(vault.id));
  const unavailable = status && !status.available ? (status.unavailableReason ?? "other") : null;
  const unavailableText = {
    noHardware: t("deviceUnlock.unavailableNoHardware"),
    notEnrolled: t("deviceUnlock.unavailableNotEnrolled"),
    other: t("deviceUnlock.unavailableOther"),
  };
  const change = async (on: boolean) => {
    setBusy(true);
    try {
      if (on) await deviceUnlock.enable(vault.name);
      else await deviceUnlock.disable(vault.id);
    } finally {
      setBusy(false);
    }
  };
  return (
    <SettingRow
      title={
        <span className="setting-title-with-chip">
          <FingerprintPattern size={16} aria-hidden />
          {t("settings.deviceUnlock")}
        </span>
      }
      description={
        <>
          {t("settings.deviceUnlockDesc")}
          {unavailable && (
            <>
              {" "}
              <strong>{unavailableText[unavailable]}</strong>
            </>
          )}
        </>
      }
    >
      <Switch
        checked={setUp}
        // Turning it off always works; on needs a usable sensor.
        disabled={busy || !status || (!setUp && Boolean(unavailable))}
        onChange={(on) => void change(on)}
        label={t("settings.deviceUnlock")}
      />
    </SettingRow>
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
  const extensionId = status?.extensionId || info.extensionId || "";
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

/** `items`: the main screen's list entries (incl. trash) for the attachment totals. */
function VaultSection({ items }: { items: ItemListEntry[] }) {
  const { t, tp, locale, errorText } = useT();
  const toast = useToast();
  const { info, vault, setVault, refreshVaults, showUnlock } = useApp();
  // Renaming changes the vault (both platforms); deleting it is PC-only; the
  // folder button needs a file manager.
  const readOnly = info.readOnly;
  const desktop = !isPhonePlatform(info.platform);
  const nameId = useId();
  const [name, setName] = useState(vault?.name ?? "");
  const [busy, setBusy] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const changed = name.trim() !== "" && name.trim() !== vault?.name;
  // Attachments live beside the vault file, outside every export: count and
  // size over the current entries (no extra command), with the backup hint.
  const attachments = items.flatMap((item) => item.attachments);
  const attachmentBytes = attachments.reduce((sum, a) => sum + a.size, 0);

  const openFolder = async () => {
    try {
      await api.openDataDir();
    } catch (err) {
      toast.error(errorText(err));
    }
  };

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
      {!readOnly && (
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
      )}
      <SettingRow
        title={
          <>
            <Paperclip size={15} className="inline-icon" />
            {t("attachments.title")}
          </>
        }
        description={
          <>
            {attachments.length > 0
              ? tp("attachments.usage", attachments.length, { size: formatBytes(attachmentBytes, locale) })
              : t("attachments.usageNone")}{" "}
            {t("attachments.backupHint")}
          </>
        }
      >
        {desktop && (
          <Button variant="secondary" icon={<FolderOpen />} onClick={() => void openFolder()}>
            {t("data.openFolder")}
          </Button>
        )}
      </SettingRow>
      {!readOnly && desktop && (
        <SettingRow title={t("danger.delete")} description={t("danger.deleteDesc")}>
          <Button variant="danger-outline" onClick={() => setDeleteOpen(true)}>
            {t("danger.deleteOpen")}
          </Button>
        </SettingRow>
      )}
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

/**
 * Phone only: where the vaults are synced with – a cloud folder ("Cloud-folder
 * sync (phone)" in docs/ARCHITECTURE.md) or, since stage 2c, a folder on a
 * WebDAV server ("Server sync (WebDAV)").
 */
function SyncSection() {
  const { t, formatDateTime, formatRelative } = useT();
  const folderSync = useFolderSync();
  const { settings, updateSettings } = useApp();
  const errorFor = useSyncErrorText();
  const skippedText = useSkippedText();
  const [serverForm, setServerForm] = useState<"new" | "edit" | null>(null);
  useRelativeTimeTick(Boolean(folderSync?.status?.lastSyncAt || folderSync?.status?.retryAt));
  if (!folderSync) return null;
  const { status, busy, link, sync, unlink } = folderSync;
  const linked = status?.linked === true;
  const server = status?.kind === "webdav" ? status.server : null;
  const syncing = Boolean(status?.syncing) || busy === "sync";
  const skipped = skippedText(status);
  const dialog = serverForm && (
    <PhoneServerDialog initial={serverForm === "edit" ? server : null} onClose={() => setServerForm(null)} />
  );

  return (
    <SettingsSection id="sync" title={t("settings.sync")} description={t("settings.syncDesc")}>
      {!linked ? (
        <SettingRow title={t("sync.method")} description={<SyncSteps />} stacked>
          <div className="setting-actions">
            <Button
              variant="primary"
              icon={<FolderOpen />}
              loading={busy === "link"}
              disabled={!status || busy !== null}
              onClick={() => void link()}
            >
              {t("sync.pickFolder")}
            </Button>
            <Button
              variant="secondary"
              icon={<Server />}
              loading={busy === "server"}
              disabled={!status || busy !== null}
              onClick={() => setServerForm("new")}
            >
              {t("sync.connectServer")}
            </Button>
          </div>
          <p className="subtle">{t("sync.methodSoon")}</p>
        </SettingRow>
      ) : (
        <>
          {needsRelink(status) && (
            <div className="setting-row stacked">
              <RelinkBanner />
            </div>
          )}
          {server ? (
            <SettingRow
              title={t("sync.server")}
              description={
                <>
                  <code className="path selectable">{server.url}</code>
                  <span className="sync-folder-desc">{t("sync.serverUserFolder", { user: server.user, folder: server.folder })}</span>
                  <span className="sync-folder-desc">{t("sync.serverDesc")}</span>
                </>
              }
              stacked
            >
              <div className="setting-actions">
                <Button
                  variant={serverNeedsPassword(status?.lastError) ? "primary" : "secondary"}
                  icon={serverNeedsPassword(status?.lastError) ? <KeyRound /> : <Server />}
                  loading={busy === "server"}
                  disabled={busy !== null && busy !== "server"}
                  onClick={() => setServerForm("edit")}
                >
                  {serverNeedsPassword(status?.lastError) ? t("sync.reenterPassword") : t("sync.changeServer")}
                </Button>
                <Button
                  variant="ghost"
                  icon={<FolderOpen />}
                  loading={busy === "link"}
                  disabled={busy !== null && busy !== "link"}
                  onClick={() => void link()}
                >
                  {t("sync.switchToFolder")}
                </Button>
              </div>
            </SettingRow>
          ) : (
            <SettingRow
              title={t("settings.syncFolder")}
              description={
                <>
                  <strong className="sync-folder-name">{status?.folderName}</strong>
                  <span className="sync-folder-desc">{t("settings.syncFolderDesc")}</span>
                </>
              }
              stacked
            >
              <div className="setting-actions">
                <Button
                  variant="secondary"
                  icon={<FolderOpen />}
                  loading={busy === "link"}
                  disabled={busy !== null && busy !== "link"}
                  onClick={() => void link()}
                >
                  {folderLost(status) ? t("sync.repick") : t("sync.changeFolder")}
                </Button>
                <Button
                  variant="ghost"
                  icon={<Server />}
                  loading={busy === "server"}
                  disabled={busy !== null && busy !== "server"}
                  onClick={() => setServerForm("new")}
                >
                  {t("sync.switchToServer")}
                </Button>
              </div>
            </SettingRow>
          )}
          <SettingRow
            title={t("settings.syncLast")}
            description={
              <>
                {syncing
                  ? t("sync.syncing")
                  : status?.lastSyncAt
                    ? `${formatRelative(status.lastSyncAt)} · ${formatDateTime(status.lastSyncAt)}`
                    : t("sync.neverSynced")}
                {status?.lastError && !syncing && (
                  <span className="callout callout-warning sync-error" role="status">
                    <TriangleAlert />
                    <span>{errorFor(status.lastError)}</span>
                  </span>
                )}
                {skipped && !syncing && <span className="sync-folder-desc">{skipped}</span>}
              </>
            }
            stacked
          >
            <Button
              variant="secondary"
              icon={<RefreshCw />}
              loading={syncing}
              disabled={busy !== null && busy !== "sync"}
              onClick={() => void sync()}
            >
              {t("sync.syncNow")}
            </Button>
          </SettingRow>
          {server && (
            <SettingRow title={t("sync.wifiOnly")} description={t("sync.wifiOnlyDesc")}>
              <Switch
                checked={settings.syncWifiOnly}
                onChange={(syncWifiOnly) => void updateSettings({ syncWifiOnly })}
                label={t("sync.wifiOnly")}
              />
            </SettingRow>
          )}
          <SettingRow
            title={server ? t("settings.syncVaultsServer") : t("settings.syncVaults")}
            description={
              status && status.files.length > 0 ? <SyncFileList files={status.files} /> : t("settings.syncNoFiles")
            }
            stacked
          />
          <SettingRow title={t("sync.unlink")} description={t("settings.syncUnlinkDesc")} stacked>
            <Button
              variant="danger-outline"
              icon={<Unlink />}
              loading={busy === "unlink"}
              disabled={busy !== null && busy !== "unlink"}
              onClick={() => void unlink()}
            >
              {t("sync.unlink")}
            </Button>
          </SettingRow>
        </>
      )}
      {dialog}
    </SettingsSection>
  );
}

type SyncMethod = "none" | "folder" | "webdav";

/**
 * Desktop: how the PC keeps its encrypted copies for the phone – off, a
 * cloud folder ("Cloud-folder mirror (desktop) and phone link" in
 * docs/ARCHITECTURE.md) or, since stage 2c, a WebDAV server ("Server sync
 * (WebDAV)"); one method at a time.
 */
function DesktopSyncSection() {
  const { t, errorText, formatDateTime, formatRelative } = useT();
  const mirror = useMirror();
  const { settings } = useApp();
  const [serverForm, setServerForm] = useState<"new" | "edit" | null>(null);
  const status = mirror?.status ?? null;
  useRelativeTimeTick(Boolean(status?.lastMirroredAt || status?.lastPulledAt || status?.retryAt));
  if (!mirror) return null;
  const folder = settings.mirrorFolder;
  const server = folder === null ? settings.syncServer : null;
  const method: SyncMethod = folder !== null ? "folder" : server ? "webdav" : "none";

  const pick = (next: SyncMethod) => {
    if (next === method) return;
    // A folder or a server replaces the other one (the backend clears it).
    if (next === "folder") void mirror.choose();
    else if (next === "webdav") setServerForm("new");
    else void (method === "folder" ? mirror.turnOff() : mirror.clearServer());
  };

  const state = (
    <>
      <span className="mirror-state">
        {status?.lastMirroredAt
          ? t("mirror.lastCopied", { when: `${formatRelative(status.lastMirroredAt)} · ${formatDateTime(status.lastMirroredAt)}` })
          : method === "webdav"
            ? t("mirror.noCopyYetServer")
            : t("mirror.noCopyYet")}
      </span>
      {status?.lastPulledAt && (
        <span className="mirror-state">
          {t("mirror.lastPulled", { when: `${formatRelative(status.lastPulledAt)} · ${formatDateTime(status.lastPulledAt)}` })}
        </span>
      )}
      {status?.lastError && (
        <span className="callout callout-warning mirror-error" role="status">
          <TriangleAlert />
          <span>{errorText(new ApiError(status.lastError))}</span>
        </span>
      )}
      {status?.retryAt && status.retryAt > Date.now() && (
        <span className="mirror-state">{t("mirror.retryAt", { when: formatRelative(status.retryAt) })}</span>
      )}
    </>
  );

  return (
    <SettingsSection id="sync" title={t("settings.sync")} description={t("settings.syncDescPc")}>
      <SettingRow title={t("sync.method")} description={t("sync.methodSoon")} stacked>
        <Segmented<SyncMethod>
          value={method}
          onChange={pick}
          ariaLabel={t("sync.method")}
          options={[
            { value: "none", label: t("sync.methodNone") },
            { value: "folder", label: t("sync.methodFolder"), icon: <FolderOpen /> },
            { value: "webdav", label: t("sync.methodServer"), icon: <Server /> },
          ]}
        />
      </SettingRow>
      {method === "folder" && (
        <SettingRow
          title={t("mirror.folder")}
          description={
            <>
              <code className="path selectable">{folder}</code>
              <span className="mirror-state">{t("mirror.desc")}</span>
              {state}
            </>
          }
        >
          <Button variant="secondary" icon={<FolderOpen />} loading={mirror.busy} onClick={() => void mirror.choose()}>
            {t("mirror.changeFolder")}
          </Button>
        </SettingRow>
      )}
      {method === "webdav" && server && (
        <SettingRow
          title={t("sync.server")}
          description={
            <>
              <code className="path selectable">{server.url}</code>
              <span className="mirror-state">{t("sync.serverUserFolder", { user: server.user, folder: server.folder })}</span>
              <span className="mirror-state">{t("mirror.serverDesc")}</span>
              {state}
            </>
          }
          stacked
        >
          <Button
            variant={serverNeedsPassword(status?.lastError) ? "primary" : "secondary"}
            icon={serverNeedsPassword(status?.lastError) ? <KeyRound /> : <Server />}
            loading={mirror.busy}
            onClick={() => setServerForm("edit")}
          >
            {serverNeedsPassword(status?.lastError) ? t("sync.reenterPassword") : t("sync.changeServer")}
          </Button>
        </SettingRow>
      )}
      {serverForm && (
        <WebDavDialog
          initial={serverForm === "edit" ? server : null}
          title={serverForm === "edit" ? t("webdav.changeTitle") : t("webdav.title")}
          connectLabel={t("webdav.connect")}
          passwordNote={status?.passwordKept === false ? t("webdav.passwordMemory") : t("webdav.passwordStoredPc")}
          onConnect={mirror.setServer}
          onClose={() => setServerForm(null)}
        />
      )}
    </SettingsSection>
  );
}

/**
 * Desktop: the way to the phone – the vaults the folder or server offers
 * (made on the phone), the phone's conflict copies and the steps; the
 * method itself is chosen under "Synchronisierung" (`DesktopSyncSection`).
 */
function PhoneSection() {
  const { t, formatDateTime, formatRelative } = useT();
  const mirror = useMirror();
  const openImport = useOpenImport();
  const { settings } = useApp();
  if (!mirror) return null;
  const on = settings.mirrorFolder !== null || settings.syncServer !== null;

  return (
    <SettingsSection id="phone" title={t("settings.phone")} description={t("settings.phoneDesc")}>
      {on && mirror.offers.length > 0 && (
        <SettingRow title={t("sync.offer.listTitle")} description={t("sync.offer.listDesc")} stacked>
          <ul className="offer-list">
            {mirror.offers.map((offer) => (
              <li key={offer.vaultId} className="offer-row">
                <span className="offer-row-text">
                  <strong className="truncate">{offer.name}</strong>
                  <span className="subtle">
                    {t("sync.offer.changed", { when: formatRelative(offer.updatedAt) })} · {formatDateTime(offer.updatedAt)}
                  </span>
                </span>
                <span className="offer-row-actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={mirror.offerBusy !== null}
                    onClick={() => void mirror.dismiss(offer)}
                  >
                    {t("sync.offer.dismiss")}
                  </Button>
                  <Button
                    size="sm"
                    variant="primary"
                    loading={mirror.offerBusy === offer.vaultId}
                    disabled={mirror.offerBusy !== null && mirror.offerBusy !== offer.vaultId}
                    onClick={() => void mirror.adopt(offer)}
                  >
                    {t("sync.offer.adopt")}
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        </SettingRow>
      )}
      {on && mirror.copies.length > 0 && (
        <SettingRow title={t("sync.copies.listTitle")} description={t("sync.copies.listDesc")} stacked>
          <ul className="offer-list">
            {mirror.copies.map((copy) => (
              <li key={copy.fileName} className="offer-row">
                <span className="offer-row-text">
                  <strong className="truncate">{t("sync.copies.vault", { name: copy.vaultName, n: copy.revision })}</strong>
                  <span className="subtle">
                    {t("sync.offer.changed", { when: formatRelative(copy.updatedAt) })} · {formatDateTime(copy.updatedAt)}
                  </span>
                </span>
                <span className="offer-row-actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={mirror.copyBusy !== null}
                    onClick={() => void mirror.dismissCopy(copy)}
                  >
                    {t("sync.copies.dismiss")}
                  </Button>
                  <Button
                    size="sm"
                    variant="primary"
                    loading={mirror.copyBusy === copy.fileName}
                    disabled={!openImport || (mirror.copyBusy !== null && mirror.copyBusy !== copy.fileName)}
                    onClick={() =>
                      void mirror.restoreCopy(copy).then((path) => {
                        if (path) openImport?.(path);
                      })
                    }
                  >
                    {t("sync.copies.restore")}
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        </SettingRow>
      )}
      <div className="setting-row stacked">
        <ol className="guide-steps">
          <GuideStep n={1} done={on} title={t("mirror.step1Title")}>
            <p>{t("mirror.step1Text")}</p>
          </GuideStep>
          <GuideStep n={2} done={false} title={t("mirror.step2Title")}>
            <p>{t("mirror.step2Text")}</p>
          </GuideStep>
          <GuideStep n={3} done={false} title={t("mirror.step3Title")}>
            <p>{t("mirror.step3Text")}</p>
            <div>
              <Button size="sm" variant="secondary" icon={<ExternalLink />} onClick={() => openExternal(releasesUrl())}>
                {t("about.releasesOpen")}
              </Button>
            </div>
          </GuideStep>
        </ol>
        <div className="callout callout-info mirror-privacy">
          <ShieldCheck />
          <span>{t("mirror.privacy")}</span>
        </div>
      </div>
    </SettingsSection>
  );
}

/** The system names in "Über Keystead" (product names, the same in every language). */
const PLATFORM_NAMES: Record<AppInfo["platform"], string> = {
  windows: "Windows",
  macos: "macOS",
  linux: "Linux",
  android: "Android",
  ios: "iOS",
};

function AboutSection() {
  const { t } = useT();
  const { info, settings, updateSettings } = useApp();
  const update = useUpdate();
  const updateErrorText = useUpdateErrorText();
  const copy = useCopy();
  const channelId = useId();
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<CheckResult | null>(null);
  const platform = PLATFORM_NAMES[info.platform] ?? PLATFORM_NAMES.linux;
  // The phone app has no network permission, so no update check: new
  // versions come as an APK from the releases page (installed over the old one).
  const phone = isPhonePlatform(info.platform);

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
            {platform} · {phone ? t("about.phoneData") : info.portable ? t("about.portable") : t("about.installed")}
          </p>
        </div>
      </div>
      {phone ? (
        <SettingRow title={t("about.releasesTitle")} description={t("about.releasesDesc")} stacked>
          <Button variant="secondary" icon={<ExternalLink />} onClick={() => openExternal(releasesUrl())}>
            {t("about.releasesOpen")}
          </Button>
        </SettingRow>
      ) : (
        <>
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
          <SettingRow
            title={t("update.manualTitle")}
            description={
              <>
                {t("update.manualDesc")} <code className="path selectable">{RELEASES_URL}</code>
              </>
            }
          >
            <div className="setting-actions">
              <Button variant="secondary" icon={<ExternalLink />} onClick={() => openExternal(RELEASES_URL)}>
                {t("update.openReleases")}
              </Button>
              <Button
                variant="ghost"
                icon={<Copy />}
                onClick={() => void copy(RELEASES_URL, { label: t("update.releasesLink"), sensitive: false })}
              >
                {t("common.copy")}
              </Button>
            </div>
          </SettingRow>
        </>
      )}
      {/* Attribution of the embedded word lists (EFF: CC BY 4.0 requires it in every copy). */}
      <SettingRow
        title={t("about.wordlists")}
        description={
          <>
            {t("about.wordlistEn")}
            <br />
            {t("about.wordlistDe")}
          </>
        }
      />
    </SettingsSection>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/** Old section ids (deep links from elsewhere) → current ones. */
const SECTION_ALIASES: Record<string, string> = { appearance: "general", terminal: "advanced", danger: "vault" };

/** `items`: the main screen's list entries, for the attachment totals in the vault section. */
export function SettingsPage({ initialSection, items }: { initialSection?: string; items: ItemListEntry[] }) {
  const { t } = useT();
  const { info } = useApp();
  // The phone: no browser integration, import/export or desktop extras.
  const desktop = !isPhonePlatform(info.platform);
  const shown = {
    // The phone's cloud-folder or server sync; the desktop's copies for it.
    sync: true,
    phone: desktop,
    browser: desktop,
    data: desktop && !info.readOnly,
    advanced: desktop,
    vault: desktop || !info.readOnly,
  };
  const scrollRef = useRef<HTMLElement>(null);
  /** Section picked in the list: keeps it highlighted until the user scrolls by hand. */
  const pinned = useRef<string | null>(null);
  const initial = initialSection ? SECTION_ALIASES[initialSection] ?? initialSection : undefined;
  const [active, setActive] = useState(initial ?? "general");

  const sections: { id: string; label: string; icon: ReactNode; hidden?: boolean }[] = [
    { id: "general", label: t("settings.general"), icon: <SlidersHorizontal /> },
    { id: "security", label: t("settings.security"), icon: <ShieldCheck /> },
    { id: "sync", label: t("settings.syncShort"), icon: <FolderSync />, hidden: !shown.sync },
    { id: "phone", label: t("settings.phoneShort"), icon: <Smartphone />, hidden: !shown.phone },
    { id: "browser", label: t("settings.browserShort"), icon: <Plug />, hidden: !shown.browser },
    { id: "data", label: t("settings.data"), icon: <Upload />, hidden: !shown.data },
    { id: "advanced", label: t("settings.advanced"), icon: <Wrench />, hidden: !shown.advanced },
    { id: "vault", label: t("settings.vault"), icon: <LockKeyhole />, hidden: !shown.vault },
    { id: "about", label: t("settings.aboutShort"), icon: <Info /> },
  ].filter((s) => !s.hidden);

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
          {shown.sync && (desktop ? <DesktopSyncSection /> : <SyncSection />)}
          {shown.phone && <PhoneSection />}
          {shown.browser && <BrowserSection />}
          {shown.data && <ImportExportSection />}
          {shown.advanced && <AdvancedSection />}
          {shown.vault && <VaultSection items={items} />}
          <AboutSection />
        </div>
      </div>
    </main>
  );
}
