import { useId, useState } from "react";
import { FileDown, KeyRound, LifeBuoy, ShieldAlert, Trash2, TriangleAlert } from "lucide-react";
import { ApiError, api, pickSaveFile } from "../../lib/api";
import type { ExportFormat, VaultInfo } from "../../lib/types";
import { useT } from "../../i18n";
import { useToast } from "../../components/Toasts";
import { Modal } from "../../components/Modal";
import { Button, Field, PasswordInput, Select } from "../../components/Controls";
import { RecoveryKeyReveal } from "../../components/RecoveryKey";
import {
  MasterPasswordFields,
  masterPasswordProblem,
  useStrength,
  type MasterPasswordState,
} from "../../components/MasterPasswordFields";
import { StrengthMeter } from "../../components/StrengthMeter";

// ---------------------------------------------------------------------------
// Change master password
// ---------------------------------------------------------------------------

export function ChangeMasterPasswordDialog({ onClose }: { onClose: () => void }) {
  const { t, errorText } = useT();
  const toast = useToast();
  const currentId = useId();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState<MasterPasswordState>({ password: "", confirm: "" });
  const [showErrors, setShowErrors] = useState(false);
  const [busy, setBusy] = useState(false);
  const [currentError, setCurrentError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const strength = useStrength(next.password);

  const submit = async () => {
    setShowErrors(true);
    setError(null);
    setCurrentError(null);
    if (!current) {
      setCurrentError(t("master.required"));
      return;
    }
    setBusy(true);
    try {
      const finalStrength = next.password ? await api.passwordStrength(next.password) : null;
      if (masterPasswordProblem(next, finalStrength)) return;
      if (next.password === current) {
        setError(t("master.sameAsOld"));
        return;
      }
      await api.changeMasterPassword(current, next.password);
      toast.success(t("master.changed"));
      onClose();
    } catch (err) {
      if (err instanceof ApiError && err.code === "wrong_password") setCurrentError(t("master.currentWrong"));
      else setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t("master.changeTitle")}
      subtitle={t("master.changeSubtitle")}
      icon={<KeyRound />}
      onClose={onClose}
      dismissable={!busy}
      onSubmit={() => void submit()}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant="primary" loading={busy}>
            {t("master.changeButton")}
          </Button>
        </>
      }
    >
      <Field label={t("master.current")} htmlFor={currentId} error={currentError}>
        <PasswordInput id={currentId} value={current} onChange={setCurrent} invalid={Boolean(currentError)} mono={false} />
      </Field>
      <MasterPasswordFields value={next} onChange={setNext} showErrors={showErrors} strength={strength} />
      {error && <div className="field-error">{error}</div>}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Recovery key
// ---------------------------------------------------------------------------

export function RecoveryKeyDialog({
  replacing,
  onClose,
  onCreated,
}: {
  replacing: boolean;
  onClose: () => void;
  onCreated: () => void;
}) {
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
      onCreated();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (key) {
    return (
      <Modal
        title={t("recovery.yourKey")}
        subtitle={t("recovery.yourKeyDesc")}
        icon={<LifeBuoy />}
        onClose={onClose}
        dismissable={confirmed}
        wide
        footer={
          <Button variant="primary" onClick={onClose} disabled={!confirmed} data-autofocus>
            {t("common.done")}
          </Button>
        }
      >
        <RecoveryKeyReveal value={key} confirmed={confirmed} onConfirmedChange={setConfirmed} />
      </Modal>
    );
  }

  return (
    <Modal
      title={replacing ? t("recovery.replaceTitle") : t("recovery.createTitle")}
      icon={<LifeBuoy />}
      tone={replacing ? "warning" : "accent"}
      onClose={onClose}
      dismissable={!busy}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => void create()} loading={busy} data-autofocus>
            {replacing ? t("recovery.replaceButton") : t("recovery.create")}
          </Button>
        </>
      }
    >
      <p>{replacing ? t("recovery.replaceText") : t("recovery.offerText")}</p>
      {error && <div className="field-error">{error}</div>}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

const EXPORT_EXT: Record<ExportFormat, string> = { vaultx: "vaultx", csv: "csv", bitwarden_json: "json" };

export function ExportDialog({ vaultName, onClose }: { vaultName: string; onClose: () => void }) {
  const { t, errorText } = useT();
  const toast = useToast();
  const masterId = useId();
  const exportPwId = useId();
  const exportPw2Id = useId();
  const formatId = useId();
  const [format, setFormat] = useState<ExportFormat>("vaultx");
  const [exportPassword, setExportPassword] = useState("");
  const [exportPassword2, setExportPassword2] = useState("");
  const [master, setMaster] = useState("");
  const [busy, setBusy] = useState(false);
  const [showErrors, setShowErrors] = useState(false);
  const [masterError, setMasterError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const encrypted = format === "vaultx";

  const exportPwProblem = encrypted
    ? !exportPassword
      ? t("export.passwordRequired")
      : exportPassword !== exportPassword2
        ? t("master.mismatch")
        : null
    : null;

  const submit = async () => {
    setShowErrors(true);
    setError(null);
    setMasterError(null);
    if (exportPwProblem) return;
    if (!master) {
      setMasterError(t("master.required"));
      return;
    }
    const date = new Date().toISOString().slice(0, 10);
    const safeName = vaultName.replace(/[^\p{L}\p{N}_-]+/gu, "_") || "VaultX";
    let path: string | null;
    try {
      path = await pickSaveFile({
        title: t("export.title"),
        defaultPath: `${safeName}-${date}.${EXPORT_EXT[format]}`,
        filters: [{ name: format.toUpperCase(), extensions: [EXPORT_EXT[format]] }],
      });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!path) return;
    setBusy(true);
    try {
      await api.exportData(format, path, encrypted ? exportPassword : null, master);
      toast.success(t("export.done", { path }));
      onClose();
    } catch (err) {
      if (err instanceof ApiError && err.code === "wrong_password") setMasterError(t("master.currentWrong"));
      else setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t("export.title")}
      subtitle={t("export.subtitle")}
      icon={<FileDown />}
      onClose={onClose}
      dismissable={!busy}
      wide
      onSubmit={() => void submit()}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant={encrypted ? "primary" : "danger"} loading={busy}>
            {t("export.button")}
          </Button>
        </>
      }
    >
      <Field label={t("export.format")} htmlFor={formatId}>
        <Select
          id={formatId}
          value={format}
          onChange={setFormat}
          options={[
            { value: "vaultx", label: t("format.vaultxExport") },
            { value: "csv", label: t("format.csvExport") },
            { value: "bitwarden_json", label: t("format.bitwardenExport") },
          ]}
        />
      </Field>
      {encrypted ? (
        <>
          <div className="callout callout-info">
            <ShieldAlert />
            <span>{t("export.encryptedInfo")}</span>
          </div>
          <Field label={t("export.password")} htmlFor={exportPwId} error={showErrors && exportPwProblem === t("export.passwordRequired") ? exportPwProblem : null}>
            <PasswordInput id={exportPwId} value={exportPassword} onChange={setExportPassword} />
            <StrengthMeter password={exportPassword} />
          </Field>
          <Field
            label={t("export.passwordConfirm")}
            htmlFor={exportPw2Id}
            error={showErrors && exportPwProblem === t("master.mismatch") ? exportPwProblem : null}
          >
            <PasswordInput id={exportPw2Id} value={exportPassword2} onChange={setExportPassword2} />
          </Field>
        </>
      ) : (
        <div className="callout callout-danger">
          <TriangleAlert />
          <span>
            <strong>{t("export.unencryptedTitle")}</strong> {t("export.unencryptedText")}
          </span>
        </div>
      )}
      <Field label={t("export.master")} htmlFor={masterId} error={masterError} hint={t("export.masterHint")}>
        <PasswordInput id={masterId} value={master} onChange={setMaster} mono={false} invalid={Boolean(masterError)} />
      </Field>
      {error && <div className="field-error">{error}</div>}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Delete vault
// ---------------------------------------------------------------------------

export function DeleteVaultDialog({
  vault,
  onClose,
  onDeleted,
}: {
  vault: VaultInfo;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const { t, errorText } = useT();
  const pwId = useId();
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!password) {
      setError(t("master.required"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.deleteVault(vault.id, password);
      onDeleted();
    } catch (err) {
      setError(err instanceof ApiError && err.code === "wrong_password" ? t("master.currentWrong") : errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t("danger.deleteTitle", { name: vault.name })}
      icon={<Trash2 />}
      tone="danger"
      onClose={onClose}
      dismissable={!busy}
      onSubmit={() => void submit()}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant="danger" loading={busy} disabled={!password}>
            {t("danger.deleteButton")}
          </Button>
        </>
      }
    >
      <p>{t("danger.deleteText")}</p>
      <Field label={t("danger.deleteConfirmLabel")} htmlFor={pwId} error={error}>
        <PasswordInput id={pwId} value={password} onChange={setPassword} mono={false} invalid={Boolean(error)} />
      </Field>
    </Modal>
  );
}
