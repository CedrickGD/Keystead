import { useCallback, useEffect, useId, useRef, useState, type FormEvent } from "react";
import { ArrowLeft, FingerprintPattern, LifeBuoy, Lock, Plus, Vault } from "lucide-react";
import { ApiError, api } from "../lib/api";
import { useT } from "../i18n";
import { useApp } from "../state/app";
import { useToast } from "../components/Toasts";
import { useConfirm } from "../components/Confirm";
import { SyncStatusLine } from "../components/FolderSync";
import { isPhonePlatform } from "../lib/phone";
import { useFolderSync } from "../state/sync";
import {
  autoPromptAllowed,
  clearAutoPromptNote,
  deviceUnlockError,
  forgetAutoPromptNote,
  useDeviceUnlock,
} from "../state/deviceUnlock";
import { Logo } from "../components/Logo";
import { Button, Field, PasswordInput, Select } from "../components/Controls";
import {
  MasterPasswordFields,
  masterPasswordProblem,
  useStrength,
  type MasterPasswordState,
} from "../components/MasterPasswordFields";

/**
 * Phone: the synced copy of the vault did not open (forged or damaged) and
 * there was no earlier one – the backend removed it.
 */
function copyRejected(err: unknown): boolean {
  return err instanceof ApiError && err.code === "unsupported" && err.detail === "sync_copy_rejected";
}

/** Formats user input as XXXXX-XXXXX-XXXXX-XXXXX-XXXXX (Crockford base32). */
function formatRecoveryInput(raw: string): string {
  const clean = raw.toUpperCase().replace(/[^0-9A-Z]/g, "").slice(0, 25);
  return clean.match(/.{1,5}/g)?.join("-") ?? "";
}

export function UnlockScreen({ focusSignal }: { focusSignal: number }) {
  const { t, errorText } = useT();
  const { vaults, settings, enterVault, showWelcome, showUnlock, info, phoneLayout } = useApp();
  const initial = vaults.find((v) => v.id === settings.lastVaultId)?.id ?? vaults[0]?.id ?? "";
  const [vaultId, setVaultId] = useState(initial);
  const [mode, setMode] = useState<"password" | "recovery">("password");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [shake, setShake] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const pwId = useId();
  const vaultSelectId = useId();
  const vault = vaults.find((v) => v.id === vaultId) ?? vaults[0];
  // Phone: the cloud-folder sync (null on the desktop).
  const folderSync = useFolderSync();
  const phone = isPhonePlatform(info.platform);
  const passwordChangedOnPc = Boolean(
    folderSync?.status?.files.some((f) => f.vaultId === vault?.id && f.passwordChanged),
  );
  // Phone: the fingerprint unlock (null on the desktop).
  const deviceUnlock = useDeviceUnlock();
  const confirm = useConfirm();
  const fingerprintReady = deviceUnlock?.readyFor(vault?.id) ?? false;
  const fingerprintReset = deviceUnlock?.wasReset(vault?.id) ?? false;
  const [fingerprintBusy, setFingerprintBusy] = useState(false);
  // The prompt opens on its own once per unlock screen – not after the user
  // locked the vault by hand.
  const [mayAutoPrompt] = useState(autoPromptAllowed);
  useEffect(clearAutoPromptNote, []);
  const autoPrompted = useRef(false);

  const unlockWithFingerprint = useCallback(async () => {
    if (!vault || !deviceUnlock || busy || fingerprintBusy) return;
    setFingerprintBusy(true);
    setError(null);
    try {
      const info = await deviceUnlock.unlock(vault);
      setPassword("");
      forgetAutoPromptNote();
      enterVault(info);
    } catch (err) {
      // Closing the prompt is an answer, not an error: the password field is right
      // here. A reset setup is explained by the note below the buttons.
      const detail = deviceUnlockError(err);
      if (detail !== "device_unlock_cancelled" && detail !== "device_unlock_invalid") setError(errorText(err));
      if (copyRejected(err)) showUnlock();
    } finally {
      setFingerprintBusy(false);
    }
  }, [busy, deviceUnlock, enterVault, errorText, fingerprintBusy, showUnlock, vault]);

  // The automatic prompt waits until the app is visible (a vault locked in the
  // background shows this screen before the user is back).
  useEffect(() => {
    if (!fingerprintReady || !mayAutoPrompt || autoPrompted.current || mode !== "password") return;
    const open = () => {
      if (autoPrompted.current || document.visibilityState !== "visible") return;
      autoPrompted.current = true;
      void unlockWithFingerprint();
    };
    open();
    document.addEventListener("visibilitychange", open);
    return () => document.removeEventListener("visibilitychange", open);
  }, [fingerprintReady, mayAutoPrompt, mode, unlockWithFingerprint]);

  // Phone: no focus on its own – it would pop the on-screen keyboard over
  // the form before the user even looked at it.
  useEffect(() => {
    if (mode === "password" && !phoneLayout) inputRef.current?.focus();
  }, [focusSignal, mode, vaultId, phoneLayout]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!vault || busy) return;
    if (!password) {
      setError(t("master.required"));
      inputRef.current?.focus();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const info = await api.unlockVault(vault.id, password);
      setPassword("");
      forgetAutoPromptNote();
      // Its fingerprint setup was reset (it no longer opened the vault): offer it again.
      if (deviceUnlock && fingerprintReset && deviceUnlock.status?.available) {
        const again = await confirm({
          title: t("deviceUnlock.offerTitle"),
          message: t("deviceUnlock.offerText", { name: vault.name }),
          confirmLabel: t("deviceUnlock.offerConfirm"),
          cancelLabel: t("deviceUnlock.offerLater"),
        });
        // "Nicht jetzt" forgets the note (Settings → Sicherheit sets it up any time).
        if (again) await deviceUnlock.enable(vault.name);
        else await deviceUnlock.disable(vault.id, true);
      }
      enterVault(info);
    } catch (err) {
      const wrong = err instanceof ApiError && err.code === "wrong_password";
      // The synced copy carries a master password set on the PC since.
      setError(wrong ? (passwordChangedOnPc ? t("unlock.passwordChangedOnPc") : t("unlock.wrongPassword")) : errorText(err));
      // The phone removed that vault's copy: the list (or the welcome screen) follows.
      if (copyRejected(err)) showUnlock();
      setShake((n) => n + 1);
      requestAnimationFrame(() => inputRef.current?.select());
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="auth-screen">
      <div className="auth-card unlock-card">
        {mode === "password" ? (
          <form onSubmit={submit} noValidate className="unlock-form">
            <div className="unlock-brand">
              <Logo size={56} title="Keystead" />
              <h1 className="auth-title">{t("unlock.title")}</h1>
              <p className="muted">{t("unlock.subtitle")}</p>
            </div>

            <Field label={t("unlock.vault")} htmlFor={vaultSelectId}>
              {vaults.length > 1 ? (
                <Select
                  id={vaultSelectId}
                  value={vaultId}
                  onChange={(id) => {
                    setVaultId(id);
                    setError(null);
                  }}
                  options={vaults.map((v) => ({ value: v.id, label: v.name }))}
                  className="lg"
                />
              ) : (
                <div className="vault-static" id={vaultSelectId}>
                  <Vault />
                  <span className="truncate">{vault?.name}</span>
                </div>
              )}
            </Field>

            <Field label={t("unlock.masterPassword")} htmlFor={pwId} error={error}>
              <div key={shake} className={shake > 0 ? "shake" : undefined}>
                <PasswordInput
                  id={pwId}
                  ref={inputRef}
                  value={password}
                  onChange={(v) => {
                    setPassword(v);
                    if (error) setError(null);
                  }}
                  size="lg"
                  mono={false}
                  invalid={Boolean(error)}
                  autoFocus={!phoneLayout}
                />
              </div>
            </Field>

            <Button type="submit" variant="primary" size="lg" block loading={busy} icon={<Lock />} disabled={fingerprintBusy}>
              {t("unlock.button")}
            </Button>

            {/* Phone: the fingerprint, once set up for this vault (Settings → Sicherheit). */}
            {fingerprintReady && (
              <Button
                variant="secondary"
                size="lg"
                block
                loading={fingerprintBusy}
                disabled={busy}
                icon={<FingerprintPattern />}
                onClick={() => void unlockWithFingerprint()}
              >
                {t("deviceUnlock.button")}
              </Button>
            )}
            {fingerprintReset && (
              <div className="callout callout-info">
                <FingerprintPattern />
                <span>{t("deviceUnlock.resetHint")}</span>
              </div>
            )}

            {/* "Passwort vergessen?" sets a new master password: PC-only (like the recovery key). */}
            {!info.readOnly && (
              <div className="unlock-links">
                {!phone && (
                  <button type="button" className="btn-link" onClick={() => setMode("recovery")}>
                    {t("unlock.forgot")}
                  </button>
                )}
                <button type="button" className="btn-link" onClick={showWelcome}>
                  <Plus size={14} style={{ verticalAlign: "-2px", marginRight: 4 }} />
                  {t("unlock.createNew")}
                </button>
              </div>
            )}
            {/* Phone: where the vaults come from (the cloud-folder sync). */}
            {folderSync && <SyncStatusLine vaultId={vault?.id} />}
          </form>
        ) : (
          vault && <RecoveryUnlock vaultId={vault.id} vaultName={vault.name} hasKey={vault.hasRecoveryKey} onBack={() => setMode("password")} />
        )}
      </div>
      <div className="auth-footer">
        Keystead {info.version} · {t("unlock.footer")}
      </div>
    </div>
  );
}

function RecoveryUnlock({
  vaultId,
  vaultName,
  hasKey,
  onBack,
}: {
  vaultId: string;
  vaultName: string;
  hasKey: boolean;
  onBack: () => void;
}) {
  const { t, errorText } = useT();
  const toast = useToast();
  const { enterVault } = useApp();
  const keyId = useId();
  const [key, setKey] = useState("");
  const [master, setMaster] = useState<MasterPasswordState>({ password: "", confirm: "" });
  const [showErrors, setShowErrors] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const strength = useStrength(master.password);
  const keyComplete = key.replace(/-/g, "").length === 25;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setShowErrors(true);
    setError(null);
    if (!keyComplete) return;
    setBusy(true);
    try {
      const finalStrength = master.password ? await api.passwordStrength(master.password) : null;
      if (masterPasswordProblem(master, finalStrength)) return;
      const info = await api.unlockWithRecovery(vaultId, key, master.password);
      toast.success(t("recovery.unlocked"));
      enterVault(info);
    } catch (err) {
      if (err instanceof ApiError && err.code === "wrong_password") setError(t("recovery.wrongKey"));
      else if (err instanceof ApiError && err.code === "not_found") setError(t("recovery.noKeyForVault"));
      else setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} noValidate className="unlock-form">
      <div className="recovery-head">
        <button type="button" className="icon-btn" onClick={onBack} aria-label={t("common.back")} title={t("common.back")}>
          <ArrowLeft />
        </button>
        <div>
          <h1 className="auth-title">{t("recovery.unlockTitle")}</h1>
          <p className="muted">{t("recovery.unlockFor", { name: vaultName })}</p>
        </div>
      </div>

      {!hasKey ? (
        <>
          <div className="callout callout-warning">
            <LifeBuoy />
            <span>{t("recovery.noKeyForVault")}</span>
          </div>
          <Button variant="secondary" size="lg" block onClick={onBack}>
            {t("recovery.backToPassword")}
          </Button>
        </>
      ) : (
        <>
          <Field
            label={t("recovery.key")}
            htmlFor={keyId}
            error={showErrors && !keyComplete ? t("recovery.keyIncomplete") : null}
          >
            <input
              id={keyId}
              className={`input lg mono recovery-input ${showErrors && !keyComplete ? "invalid" : ""}`}
              value={key}
              placeholder="XXXXX-XXXXX-XXXXX-XXXXX-XXXXX"
              onChange={(e) => setKey(formatRecoveryInput(e.target.value))}
              autoComplete="off"
              spellCheck={false}
              autoFocus
            />
          </Field>
          <MasterPasswordFields value={master} onChange={setMaster} showErrors={showErrors} strength={strength} />
          {error && <div className="field-error">{error}</div>}
          <Button type="submit" variant="primary" size="lg" block loading={busy}>
            {t("recovery.unlockButton")}
          </Button>
        </>
      )}
    </form>
  );
}
