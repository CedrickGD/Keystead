import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { ArrowLeft, LifeBuoy, Lock, Plus, Vault } from "lucide-react";
import { ApiError, api } from "../lib/api";
import { useT } from "../i18n";
import { useApp } from "../state/app";
import { useToast } from "../components/Toasts";
import { Logo } from "../components/Logo";
import { Button, Field, PasswordInput, Select } from "../components/Controls";
import {
  MasterPasswordFields,
  masterPasswordProblem,
  useStrength,
  type MasterPasswordState,
} from "../components/MasterPasswordFields";

/** Formats user input as XXXXX-XXXXX-XXXXX-XXXXX-XXXXX (Crockford base32). */
function formatRecoveryInput(raw: string): string {
  const clean = raw.toUpperCase().replace(/[^0-9A-Z]/g, "").slice(0, 25);
  return clean.match(/.{1,5}/g)?.join("-") ?? "";
}

export function UnlockScreen({ focusSignal }: { focusSignal: number }) {
  const { t, errorText } = useT();
  const { vaults, settings, enterVault, showWelcome, info } = useApp();
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

  useEffect(() => {
    if (mode === "password") inputRef.current?.focus();
  }, [focusSignal, mode, vaultId]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!vault || !password || busy) return;
    setBusy(true);
    setError(null);
    try {
      const info = await api.unlockVault(vault.id, password);
      setPassword("");
      enterVault(info);
    } catch (err) {
      setError(err instanceof ApiError && err.code === "wrong_password" ? t("unlock.wrongPassword") : errorText(err));
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
              <Logo size={56} title="VaultX" />
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
                  placeholder={t("unlock.placeholder")}
                  autoFocus
                />
              </div>
            </Field>

            <Button type="submit" variant="primary" size="lg" block loading={busy} disabled={!password} icon={<Lock />}>
              {t("unlock.button")}
            </Button>

            <div className="unlock-links">
              <button type="button" className="btn-link" onClick={() => setMode("recovery")}>
                {t("unlock.forgot")}
              </button>
              <button type="button" className="btn-link" onClick={showWelcome}>
                <Plus size={14} style={{ verticalAlign: "-2px", marginRight: 4 }} />
                {t("unlock.createNew")}
              </button>
            </div>
          </form>
        ) : (
          vault && <RecoveryUnlock vaultId={vault.id} vaultName={vault.name} hasKey={vault.hasRecoveryKey} onBack={() => setMode("password")} />
        )}
      </div>
      <div className="auth-footer">
        VaultX {info.version} · {t("unlock.footer")}
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
      setError(err instanceof ApiError && err.code === "wrong_password" ? t("recovery.wrongKey") : errorText(err));
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
