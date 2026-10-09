import { useEffect, useId, useState, type FormEvent } from "react";
import {
  ArrowLeft,
  Check,
  ChevronRight,
  FileUp,
  FolderInput,
  HardDrive,
  KeyRound,
  LifeBuoy,
  Plug,
  ShieldCheck,
  TriangleAlert,
} from "lucide-react";
import { api, pickOpenFile } from "../lib/api";
import type { ImportReport, LegacyVaultInfo, VaultInfo } from "../lib/types";
import { useT } from "../i18n";
import { useApp } from "../state/app";
import { Logo } from "../components/Logo";
import { Button, Field, PasswordInput } from "../components/Controls";
import { RecoveryKeyReveal } from "../components/RecoveryKey";
import {
  MasterPasswordFields,
  masterPasswordProblem,
  useStrength,
  type MasterPasswordState,
} from "../components/MasterPasswordFields";

type Mode = "choose" | "create" | "import";

export function WelcomeScreen() {
  const { t } = useT();
  const { vaults, showUnlock } = useApp();
  const [mode, setMode] = useState<Mode>("choose");
  const firstRun = vaults.length === 0;

  if (mode !== "choose") {
    return <CreateVaultWizard withImport={mode === "import"} onBack={() => setMode("choose")} />;
  }

  return (
    <div className="auth-screen">
      <div className="welcome">
        {!firstRun && (
          <button type="button" className="btn btn-ghost btn-sm welcome-back" onClick={showUnlock}>
            <ArrowLeft />
            {t("welcome.backToUnlock")}
          </button>
        )}
        <div className="welcome-hero">
          <Logo size={64} title="VaultX" />
          <h1>{firstRun ? t("welcome.title") : t("welcome.titleAnother")}</h1>
          <p>{t("welcome.subtitle")}</p>
        </div>

        <div className="welcome-choices">
          <button type="button" className="choice-card" onClick={() => setMode("create")}>
            <span className="choice-icon">
              <ShieldCheck />
            </span>
            <span className="choice-text">
              <span className="choice-title">{t("welcome.createTitle")}</span>
              <span className="choice-desc">{t("welcome.createDesc")}</span>
            </span>
            <ChevronRight className="choice-chevron" />
          </button>
          <button type="button" className="choice-card" onClick={() => setMode("import")}>
            <span className="choice-icon muted-icon">
              <FolderInput />
            </span>
            <span className="choice-text">
              <span className="choice-title">{t("welcome.importTitle")}</span>
              <span className="choice-desc">{t("welcome.importDesc")}</span>
            </span>
            <ChevronRight className="choice-chevron" />
          </button>
        </div>

        <ul className="welcome-features">
          <li>
            <HardDrive />
            <span>
              <strong>{t("welcome.featLocal")}</strong>
              {t("welcome.featLocalDesc")}
            </span>
          </li>
          <li>
            <KeyRound />
            <span>
              <strong>{t("welcome.featCrypto")}</strong>
              {t("welcome.featCryptoDesc")}
            </span>
          </li>
          <li>
            <Plug />
            <span>
              <strong>{t("welcome.featBrowser")}</strong>
              {t("welcome.featBrowserDesc")}
            </span>
          </li>
        </ul>
      </div>
    </div>
  );
}

type Step = "vault" | "import" | "recovery";

function Stepper({ steps, current }: { steps: { id: Step; label: string }[]; current: Step }) {
  const currentIdx = steps.findIndex((s) => s.id === current);
  return (
    <ol className="stepper">
      {steps.map((step, idx) => (
        <li key={step.id} className={idx < currentIdx ? "done" : idx === currentIdx ? "current" : ""}>
          <span className="stepper-dot">{idx < currentIdx ? <Check /> : idx + 1}</span>
          <span className="stepper-label">{step.label}</span>
        </li>
      ))}
    </ol>
  );
}

function CreateVaultWizard({ withImport, onBack }: { withImport: boolean; onBack: () => void }) {
  const { t } = useT();
  const { vaults, enterVault } = useApp();
  const [step, setStep] = useState<Step>("vault");
  const [created, setCreated] = useState<VaultInfo | null>(null);

  const steps: { id: Step; label: string }[] = [
    { id: "vault", label: t("wizard.stepVault") },
    ...(withImport ? [{ id: "import" as Step, label: t("wizard.stepImport") }] : []),
    { id: "recovery", label: t("wizard.stepRecovery") },
  ];

  const finish = () => {
    if (created) enterVault(created);
  };

  return (
    <div className="auth-screen">
      <div className="auth-card wizard">
        <div className="wizard-top">
          {step === "vault" ? (
            <button type="button" className="icon-btn" onClick={onBack} aria-label={t("common.back")} title={t("common.back")}>
              <ArrowLeft />
            </button>
          ) : (
            <span style={{ width: 32 }} />
          )}
          <Stepper steps={steps} current={step} />
          <span style={{ width: 32 }} />
        </div>

        {step === "vault" && (
          <CreateVaultStep
            withImport={withImport}
            defaultName={vaults.length === 0 ? t("welcome.defaultVaultName") : ""}
            onCreated={(info) => {
              setCreated(info);
              setStep(withImport ? "import" : "recovery");
            }}
          />
        )}
        {step === "import" && <LegacyImportStep onDone={() => setStep("recovery")} />}
        {step === "recovery" && <RecoveryStep onDone={finish} />}
      </div>
    </div>
  );
}

function CreateVaultStep({
  withImport,
  defaultName,
  onCreated,
}: {
  withImport: boolean;
  defaultName: string;
  onCreated: (info: VaultInfo) => void;
}) {
  const { t, errorText } = useT();
  const nameId = useId();
  const [name, setName] = useState(defaultName);
  const [master, setMaster] = useState<MasterPasswordState>({ password: "", confirm: "" });
  const [showErrors, setShowErrors] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const strength = useStrength(master.password);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setShowErrors(true);
    setError(null);
    if (!name.trim()) return;
    setBusy(true);
    try {
      // Evaluate the final value (the live meter is debounced).
      const finalStrength = master.password ? await api.passwordStrength(master.password) : null;
      if (masterPasswordProblem(master, finalStrength)) return;
      const info = await api.createVault(name.trim(), master.password);
      onCreated(info);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="wizard-body" onSubmit={submit} noValidate>
      <div className="wizard-heading">
        <h1 className="auth-title">{withImport ? t("wizard.createForImportTitle") : t("wizard.createTitle")}</h1>
        <p className="muted">{withImport ? t("wizard.createForImportDesc") : t("wizard.createDesc")}</p>
      </div>

      <Field label={t("wizard.vaultName")} htmlFor={nameId} error={showErrors && !name.trim() ? t("wizard.nameRequired") : null}>
        <input
          id={nameId}
          className="input lg"
          value={name}
          placeholder={t("wizard.vaultNamePlaceholder")}
          onChange={(e) => setName(e.target.value)}
          maxLength={80}
          autoFocus
        />
      </Field>

      <MasterPasswordFields
        value={master}
        onChange={setMaster}
        showErrors={showErrors}
        strength={strength}
        labels={{ password: t("master.label") }}
      />

      <div className="callout callout-warning">
        <TriangleAlert />
        <span>
          <strong>{t("wizard.noRecoveryTitle")}</strong> {t("wizard.noRecoveryText")}
        </span>
      </div>

      {error && <div className="field-error">{error}</div>}

      <Button type="submit" variant="primary" size="lg" block loading={busy}>
        {t("wizard.createButton")}
      </Button>
    </form>
  );
}

function LegacyImportStep({ onDone }: { onDone: () => void }) {
  const { t, tp, errorText } = useT();
  const pwId = useId();
  const [found, setFound] = useState<LegacyVaultInfo[] | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .legacyScan()
      .then((list) => {
        if (cancelled) return;
        setFound(list);
        if (list[0]) setPath(list[0].path);
      })
      .catch(() => {
        if (!cancelled) setFound([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const pickFile = async () => {
    try {
      const picked = await pickOpenFile({
        title: t("import.pickLegacy"),
        filters: [{ name: "VaultX 1.x", extensions: ["json"] }],
      });
      if (!picked) return;
      setFound((list) => {
        const current = list ?? [];
        if (current.some((v) => v.path === picked)) return current;
        const fileName = picked.split(/[\\/]/).pop() ?? picked;
        return [...current, { name: fileName, path: picked }];
      });
      setPath(picked);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!path || !password) return;
    setBusy(true);
    setError(null);
    try {
      setReport(await api.importData("legacy", path, password));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (report) {
    return (
      <div className="wizard-body">
        <div className="wizard-heading">
          <div className="success-badge">
            <Check />
          </div>
          <h1 className="auth-title">{t("import.doneTitle")}</h1>
          <p className="muted">
            {tp("import.imported", report.imported)}
            {report.skipped > 0 && ` · ${tp("import.skipped", report.skipped)}`}
          </p>
        </div>
        {report.warnings.length > 0 && (
          <div className="callout callout-warning">
            <TriangleAlert />
            <ul className="plain-list">
              {report.warnings.map((w, i) => (
                <li key={i}>{w}</li>
              ))}
            </ul>
          </div>
        )}
        <Button variant="primary" size="lg" block onClick={onDone} autoFocus>
          {t("common.continue")}
        </Button>
      </div>
    );
  }

  return (
    <form className="wizard-body" onSubmit={submit} noValidate>
      <div className="wizard-heading">
        <h1 className="auth-title">{t("import.legacyTitle")}</h1>
        <p className="muted">{t("import.legacyDesc")}</p>
      </div>

      <div className="field">
        <span className="field-label">{t("import.source")}</span>
        {found === null ? (
          <div className="legacy-list-loading">
            <span className="spinner" /> {t("import.scanning")}
          </div>
        ) : (
          <div className="legacy-list" role="radiogroup" aria-label={t("import.source")}>
            {found.length === 0 && <div className="legacy-empty">{t("import.noneFound")}</div>}
            {found.map((v) => (
              <label key={v.path} className={`legacy-option ${path === v.path ? "selected" : ""}`}>
                <input type="radio" name="legacy" checked={path === v.path} onChange={() => setPath(v.path)} />
                <span className="legacy-option-text">
                  <span className="legacy-option-name">{v.name}</span>
                  <span className="legacy-option-path" title={v.path}>
                    {v.path}
                  </span>
                </span>
              </label>
            ))}
            <button type="button" className="legacy-pick" onClick={() => void pickFile()}>
              <FileUp />
              {t("import.pickOther")}
            </button>
          </div>
        )}
      </div>

      <Field label={t("import.legacyPassword")} htmlFor={pwId} hint={t("import.legacyPasswordHint")}>
        <PasswordInput id={pwId} value={password} onChange={setPassword} size="lg" />
      </Field>

      {error && <div className="field-error">{error}</div>}

      <div className="wizard-actions">
        <Button variant="ghost" onClick={onDone}>
          {t("common.skip")}
        </Button>
        <Button type="submit" variant="primary" size="lg" loading={busy} disabled={!path || !password}>
          {t("import.importButton")}
        </Button>
      </div>
    </form>
  );
}

function RecoveryStep({ onDone }: { onDone: () => void }) {
  const { t, errorText } = useT();
  const [key, setKey] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      setKey(await api.createRecoveryKey());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (key) {
    return (
      <div className="wizard-body">
        <div className="wizard-heading">
          <h1 className="auth-title">{t("recovery.yourKey")}</h1>
          <p className="muted">{t("recovery.yourKeyDesc")}</p>
        </div>
        <RecoveryKeyReveal value={key} confirmed={confirmed} onConfirmedChange={setConfirmed} />
        <Button variant="primary" size="lg" block disabled={!confirmed} onClick={onDone}>
          {t("wizard.finish")}
        </Button>
      </div>
    );
  }

  return (
    <div className="wizard-body">
      <div className="wizard-heading">
        <div className="success-badge">
          <Check />
        </div>
        <h1 className="auth-title">{t("wizard.createdTitle")}</h1>
        <p className="muted">{t("wizard.createdDesc")}</p>
      </div>
      <div className="recovery-offer">
        <span className="choice-icon">
          <LifeBuoy />
        </span>
        <div>
          <div className="recovery-offer-title">{t("recovery.offerTitle")}</div>
          <p className="muted">{t("recovery.offerText")}</p>
        </div>
      </div>
      {error && <div className="field-error">{error}</div>}
      <div className="wizard-actions">
        <Button variant="ghost" onClick={onDone}>
          {t("recovery.later")}
        </Button>
        <Button variant="primary" size="lg" loading={busy} onClick={() => void create()} autoFocus>
          {t("recovery.create")}
        </Button>
      </div>
    </div>
  );
}
