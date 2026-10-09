import { useId, useState, type ReactNode } from "react";
import { Braces, Check, FileDown, FileLock2, FileSpreadsheet, KeyRound, LifeBuoy, Trash2, TriangleAlert } from "lucide-react";
import { ApiError, api, pickSaveFile } from "../../lib/api";
import type { ExportFormat, VaultInfo } from "../../lib/types";
import { useT, type MessageKey } from "../../i18n";
import { useToast } from "../../components/Toasts";
import { Modal } from "../../components/Modal";
import { Button, Field, PasswordInput } from "../../components/Controls";
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

const EXPORT_EXT: Record<ExportFormat, string> = { keystead: "keystead", csv: "csv", bitwarden_json: "json" };

const EXPORT_FORMATS: {
  value: ExportFormat;
  icon: ReactNode;
  title: MessageKey;
  desc: MessageKey;
  file: MessageKey;
}[] = [
  { value: "keystead", icon: <FileLock2 />, title: "export.cardKeystead", desc: "export.cardKeysteadDesc", file: "export.fileKeystead" },
  { value: "csv", icon: <FileSpreadsheet />, title: "export.cardCsv", desc: "export.cardCsvDesc", file: "export.fileCsv" },
  { value: "bitwarden_json", icon: <Braces />, title: "export.cardBitwarden", desc: "export.cardBitwardenDesc", file: "export.fileJson" },
];

/** `<vault>-<YYYY-MM-DD>` in local time, safe as a file name. */
function exportBaseName(vaultName: string): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const safeName = vaultName.trim().replace(/[^\p{L}\p{N}_-]+/gu, "-").replace(/^-+|-+$/g, "") || "Tresor";
  return `Keystead-${safeName}-${date}`;
}

/**
 * Export in one step: pick a format card, set an export password (encrypted)
 * or read the warning (unencrypted), confirm with the master password,
 * "Speichern unter …". The toast offers "Im Ordner anzeigen".
 */
export function ExportDialog({ vaultName, onClose }: { vaultName: string; onClose: () => void }) {
  const { t, errorText } = useT();
  const toast = useToast();
  const masterId = useId();
  const exportPwId = useId();
  const exportPw2Id = useId();
  const formatName = useId();
  const [format, setFormat] = useState<ExportFormat>("keystead");
  const [exportPassword, setExportPassword] = useState("");
  const [exportPassword2, setExportPassword2] = useState("");
  const [master, setMaster] = useState("");
  const [busy, setBusy] = useState(false);
  const [showErrors, setShowErrors] = useState(false);
  const [masterError, setMasterError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const encrypted = format === "keystead";
  const spec = EXPORT_FORMATS.find((f) => f.value === format) ?? EXPORT_FORMATS[0];

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
    let path: string | null;
    try {
      path = await pickSaveFile({
        title: t("export.saveAs"),
        defaultPath: `${exportBaseName(vaultName)}.${EXPORT_EXT[format]}`,
        filters: spec ? [{ name: t(spec.file), extensions: [EXPORT_EXT[format]] }] : undefined,
      });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!path) return;
    setBusy(true);
    try {
      await api.exportData(format, path, encrypted ? exportPassword : null, master);
      toast.show({
        kind: "success",
        message: t("export.done", { path: path.split(/[\\/]/).pop() ?? path }),
        duration: 8000,
        action: {
          label: t("export.showInFolder"),
          onClick: () => {
            api.showExport().catch((err: unknown) => toast.error(errorText(err)));
          },
        },
      });
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
      className="export-dialog"
      onSubmit={() => void submit()}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant={encrypted ? "primary" : "danger"} loading={busy} icon={<FileDown />}>
            {t("export.saveAs")}
          </Button>
        </>
      }
    >
      <div className="export-formats" role="radiogroup" aria-label={t("export.format")}>
        {EXPORT_FORMATS.map((option) => (
          <label key={option.value} className={`export-format ${format === option.value ? "selected" : ""}`}>
            <input
              type="radio"
              name={formatName}
              value={option.value}
              checked={format === option.value}
              onChange={() => {
                setFormat(option.value);
                setError(null);
              }}
            />
            <span className="export-format-icon" aria-hidden>
              {option.icon}
            </span>
            <span className="export-format-text">
              <span className="export-format-title">
                {t(option.title)}
                {option.value === "keystead" && <span className="chip success">{t("export.recommended")}</span>}
              </span>
              <span className="export-format-desc">{t(option.desc)}</span>
            </span>
            <Check className="export-format-check" aria-hidden />
          </label>
        ))}
      </div>
      {encrypted ? (
        <div className="export-fields">
          <Field
            label={t("export.password")}
            htmlFor={exportPwId}
            error={showErrors && exportPwProblem === t("export.passwordRequired") ? exportPwProblem : null}
          >
            <PasswordInput id={exportPwId} value={exportPassword} onChange={setExportPassword} />
            <StrengthMeter password={exportPassword} emptyHint={t("export.passwordHint")} />
          </Field>
          <Field
            label={t("export.passwordConfirm")}
            htmlFor={exportPw2Id}
            error={showErrors && exportPwProblem === t("master.mismatch") ? exportPwProblem : null}
          >
            <PasswordInput id={exportPw2Id} value={exportPassword2} onChange={setExportPassword2} />
          </Field>
        </div>
      ) : (
        <div className="callout callout-danger" role="note">
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
