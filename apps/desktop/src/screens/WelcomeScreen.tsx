import { useEffect, useId, useState, type FormEvent, type ReactNode } from "react";
import {
  ArrowLeft,
  BookOpen,
  Check,
  ChevronRight,
  FolderInput,
  FolderOpen,
  HardDrive,
  KeyRound,
  LifeBuoy,
  Plug,
  Puzzle,
  TriangleAlert,
} from "lucide-react";
import { api } from "../lib/api";
import type { VaultInfo } from "../lib/types";
import { useT } from "../i18n";
import { useApp } from "../state/app";
import { Logo } from "../components/Logo";
import { Button, Field } from "../components/Controls";
import { RecoveryKeyReveal } from "../components/RecoveryKey";
import { ImportFlow } from "../components/import/ImportFlow";
import { useNoDropTargetMode } from "../components/import/FileDrop";
import { startOnSettings } from "../lib/startView";
import { clearRecoveryUnconfirmed, markRecoveryUnconfirmed } from "../lib/recoveryMarker";
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
          <Logo size={64} title="Keystead" />
          <h1>{firstRun ? t("welcome.title") : t("welcome.titleAnother")}</h1>
          <p>{t("welcome.subtitle")}</p>
        </div>

        <div className="welcome-choices">
          <button type="button" className="choice-card" onClick={() => setMode("create")}>
            <span className="choice-icon">
              <Logo variant="glyph" />
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
  // The vault exists from here on: a dropped file without a target is about
  // finishing the setup, not about creating or unlocking a vault.
  useNoDropTargetMode(created ? "finishSetup" : null);

  const steps: { id: Step; label: string }[] = [
    { id: "vault", label: t("wizard.stepVault") },
    ...(withImport ? [{ id: "import" as Step, label: t("wizard.stepImport") }] : []),
    { id: "recovery", label: t("wizard.stepRecovery") },
  ];

  // "Anleitung" on the last step: Settings → Browser-Integration once the vault opens.
  const [extensionGuide, setExtensionGuide] = useState(false);

  const finish = () => {
    if (!created) return;
    if (extensionGuide) startOnSettings("browser");
    enterVault(created);
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
        {step === "import" && <ImportStep onDone={() => setStep("recovery")} />}
        {step === "recovery" && (
          <RecoveryStep
            onDone={finish}
            extension={
              <ExtensionCard guideQueued={extensionGuide} onToggleGuide={() => setExtensionGuide((queued) => !queued)} />
            }
          />
        )}
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

/** The setup wizard's import step: the import dialog's flow inline (skippable). */
function ImportStep({ onDone }: { onDone: () => void }) {
  return (
    <ImportFlow variant="wizard" onFinish={onDone}>
      {(layout) => (
        <form
          className="wizard-body"
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            layout.onSubmit();
          }}
        >
          <div className="wizard-heading">
            <h1 className="auth-title">{layout.title}</h1>
            {layout.subtitle && <p className="muted">{layout.subtitle}</p>}
          </div>
          {layout.body}
          <div className="wizard-actions">{layout.footer}</div>
        </form>
      )}
    </ImportFlow>
  );
}

/**
 * Optional pointer to the browser extension on the wizard's last step: the
 * folder the app keeps it in and the full guide (Settings →
 * Browser-Integration), which opens once the wizard is finished.
 */
function ExtensionCard({ guideQueued, onToggleGuide }: { guideQueued: boolean; onToggleGuide: () => void }) {
  const { t, errorText } = useT();
  const [dir, setDir] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .browserStatus()
      .then((status) => {
        if (!cancelled) setDir(status.extensionDir);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  const openFolder = async () => {
    setError(null);
    try {
      await api.openExtensionDir();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <section className="wizard-ext" aria-labelledby="wizard-ext-title">
      <span className="wizard-ext-icon" aria-hidden>
        <Puzzle />
      </span>
      <div className="wizard-ext-body">
        <div className="wizard-ext-title" id="wizard-ext-title">
          {t("wizard.extTitle")}
          <span className="chip">{t("wizard.extOptional")}</span>
        </div>
        <div className="wizard-ext-text">{t("wizard.extText")}</div>
        {dir && (
          <code className="wizard-ext-path selectable" title={dir}>
            {dir}
          </code>
        )}
        <div className="wizard-ext-actions">
          <Button size="sm" variant="secondary" icon={<FolderOpen />} onClick={() => void openFolder()}>
            {t("browser.openFolder")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            icon={guideQueued ? <Check /> : <BookOpen />}
            aria-pressed={guideQueued}
            onClick={onToggleGuide}
          >
            {guideQueued ? t("wizard.extGuideQueued") : t("wizard.extGuide")}
          </Button>
        </div>
        {guideQueued && <div className="wizard-ext-hint">{t("wizard.extGuideHint")}</div>}
        {error && <div className="field-error">{error}</div>}
      </div>
    </section>
  );
}

function RecoveryStep({ onDone, extension }: { onDone: () => void; extension: ReactNode }) {
  const { t, errorText } = useT();
  const [key, setKey] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const created = await api.createRecoveryKey();
      markRecoveryUnconfirmed();
      setKey(created);
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
        <Button
          variant="primary"
          size="lg"
          block
          disabled={!confirmed}
          onClick={() => {
            clearRecoveryUnconfirmed();
            onDone();
          }}
        >
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
      {extension}
    </div>
  );
}
