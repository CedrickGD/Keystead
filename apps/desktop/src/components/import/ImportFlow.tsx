import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import {
  Archive,
  Ban,
  Check,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CopyCheck,
  FileText,
  FileUp,
  Globe,
  Info,
  KeyRound,
  Plus,
  Shield,
} from "lucide-react";
import { ApiError, api, pickOpenFile } from "../../lib/api";
import type {
  ConflictMode,
  ImportAnalysis,
  ImportConflict,
  ImportFormat,
  ImportMatch,
  ImportReport,
  LegacyVaultInfo,
} from "../../lib/types";
import { useT, type MessageKey } from "../../i18n";
import { Button, Field, PasswordInput } from "../Controls";
import { Logo } from "../Logo";
import { baseName, useFileDropTarget } from "./FileDrop";

type Step = "choose" | "password" | "preview" | "result";

/** What the surrounding container (modal or wizard card) renders. */
export interface ImportFlowLayout {
  step: Step;
  title: string;
  subtitle?: string;
  icon: ReactNode;
  body: ReactNode;
  footer: ReactNode;
  /** Enter in the password field. */
  onSubmit: () => void;
  /** False while the import is being written (the dialog must stay open). */
  dismissable: boolean;
}

/**
 * A file to analyse right away (dropped onto the window); `seq` changes for
 * every new request, `others` = further files dropped with it (ignored).
 */
export interface ImportRequest {
  path: string;
  seq: number;
  others?: number;
}

/** Long lists in the preview are cut off after this many rows. */
const LIST_LIMIT = 40;

const FORMAT_LABEL: Record<ImportFormat, MessageKey> = {
  csv: "importFlow.format.csv",
  bitwarden_json: "importFlow.format.bitwarden_json",
  keystead: "importFlow.format.keystead",
  legacy: "importFlow.format.legacy",
};

/**
 * Import with preview: choose or drop a file (format detected automatically)
 * → its password if it is encrypted → preview (new / already there /
 * different password / invalid) with the decision about conflicts → result.
 * Nothing is written before the user confirms the preview.
 */
export function ImportFlow({
  variant,
  request,
  onFinish,
  children,
}: {
  /** "dialog": modal (main window, settings); "wizard": a step of the setup wizard (skippable). */
  variant: "dialog" | "wizard";
  request?: ImportRequest | null;
  /** Close the dialog / continue the wizard. */
  onFinish: () => void;
  children: (layout: ImportFlowLayout) => ReactNode;
}) {
  const { t, tp, errorText } = useT();
  const passwordId = useId();
  const [step, setStep] = useState<Step>("choose");
  const [file, setFile] = useState<{ path: string; name: string } | null>(null);
  const [analysis, setAnalysis] = useState<ImportAnalysis | null>(null);
  const [password, setPassword] = useState("");
  const [passwordError, setPasswordError] = useState<string | null>(null);
  /** `retry`: checking the same file again may help (I/O, expired preview). */
  const [error, setError] = useState<{ text: string; retry: boolean } | null>(null);
  /** About dropped files that were not taken (shown in the dialog, not as a toast over it). */
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<"analyze" | "commit" | null>(null);
  const [mode, setMode] = useState<ConflictMode>("skip");
  const [report, setReport] = useState<ImportReport | null>(null);
  const [legacy, setLegacy] = useState<LegacyVaultInfo[]>([]);
  const hintId = useId();
  const seq = useRef(0);
  /** The plan waiting in the backend (cancelled when the flow is left). */
  const pendingId = useRef<string | null>(null);
  const busyRef = useRef(busy);
  busyRef.current = busy;
  const analysisRef = useRef(analysis);
  analysisRef.current = analysis;

  const dropPending = useCallback(() => {
    const id = pendingId.current;
    pendingId.current = null;
    if (id) api.cancelImport(id).catch(() => undefined);
  }, []);

  useEffect(
    () => () => {
      // Left while an analysis is still running: its late result is
      // cancelled as soon as it arrives (see `analyze`).
      seq.current += 1;
      dropPending();
    },
    [dropPending],
  );

  useEffect(() => {
    let cancelled = false;
    api
      .legacyScan()
      .then((found) => {
        if (!cancelled) setLegacy(found);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  const importErrorText = useCallback(
    (err: unknown): string => {
      if (err instanceof ApiError) {
        if (err.code === "not_found") return t("importFlow.errNotFound");
        if (err.code === "io") return t("importFlow.errIo", { detail: err.detail });
        if (err.code === "corrupt") return t("importFlow.errCorrupt");
        if (err.code === "unsupported" && err.detail.startsWith("vault format version")) return t("importFlow.errNewer");
      }
      return errorText(err);
    },
    [errorText, t],
  );

  const analyze = useCallback(
    async (path: string, filePassword: string | null, others = 0) => {
      const mine = ++seq.current;
      dropPending();
      const samePasswordStep = filePassword !== null;
      setBusy("analyze");
      setError(null);
      setPasswordError(null);
      setReport(null);
      setFile({ path, name: baseName(path) });
      if (!samePasswordStep) {
        setStep("choose");
        setAnalysis(null);
        setPassword("");
        setNotice(others > 0 ? t("drop.onlyOne", { name: baseName(path) }) : null);
      }
      try {
        const result = await api.analyzeImport(path, filePassword);
        if (mine !== seq.current) {
          if (result.importId) api.cancelImport(result.importId).catch(() => undefined);
          return;
        }
        setAnalysis(result);
        if (!result.preview || !result.importId) {
          setStep("password");
          return;
        }
        pendingId.current = result.importId;
        setMode("skip");
        setStep("preview");
      } catch (err) {
        if (mine !== seq.current) return;
        if (err instanceof ApiError && err.code === "wrong_password" && samePasswordStep) {
          // The password step shows the analysis that asked for the password.
          const legacyFile = analysisRef.current?.format === "legacy";
          setPasswordError(legacyFile ? t("importFlow.legacyWrongPassword") : t("import.wrongPassword"));
          setStep("password");
        } else {
          setError({ text: importErrorText(err), retry: isRetryable(err) });
          setStep("choose");
        }
      } finally {
        if (mine === seq.current) setBusy(null);
      }
    },
    [dropPending, importErrorText, t],
  );

  // A file dropped onto the window before or while the dialog is open (only
  // a new request starts an analysis).
  const analyzeRef = useRef(analyze);
  analyzeRef.current = analyze;
  const requestPath = request?.path;
  const requestSeq = request?.seq;
  const requestOthers = request?.others ?? 0;
  useEffect(() => {
    if (requestPath) void analyzeRef.current(requestPath, null, requestOthers);
  }, [requestPath, requestSeq, requestOthers]);

  useFileDropTarget((path, others) => {
    // While the import is written the file is not taken (it would replace
    // the plan being committed); say so here, not as a toast over the dialog.
    if (busyRef.current === "commit") {
      setNotice(t("drop.busy", { name: baseName(path) }));
      return;
    }
    void analyze(path, null, others);
  });

  const pick = async () => {
    try {
      const picked = await pickOpenFile({
        title: t("import.pickFile"),
        filters: [
          { name: t("importFlow.filterSupported"), extensions: ["csv", "json", "keystead", "tsv", "txt"] },
          { name: t("importFlow.filterAll"), extensions: ["*"] },
        ],
      });
      if (picked) void analyze(picked, null);
    } catch (err) {
      setError({ text: errorText(err), retry: false });
    }
  };

  const submitPassword = () => {
    if (!file || busy) return;
    if (!password) {
      setPasswordError(t("error.input.password_required"));
      return;
    }
    void analyze(file.path, password);
  };

  const commit = async () => {
    const importId = analysis?.importId;
    if (!importId || busy) return;
    setBusy("commit");
    setError(null);
    setNotice(null);
    // The backend drops the plan with every commit attempt.
    pendingId.current = null;
    try {
      setReport(await api.commitImport(importId, mode));
      setStep("result");
      setPassword("");
    } catch (err) {
      const expired = err instanceof ApiError && err.code === "not_found";
      setError({ text: expired ? t("importFlow.errExpired") : importErrorText(err), retry: true });
      setAnalysis(null);
      setStep("choose");
    } finally {
      setBusy(null);
    }
  };

  const backToChoose = () => {
    seq.current += 1;
    dropPending();
    setBusy(null);
    setAnalysis(null);
    setPassword("");
    setPasswordError(null);
    setError(null);
    setNotice(null);
    setStep("choose");
  };

  // A new step replaces the content: focus its main control (the dialog only
  // focuses on mount, and React may keep focus on a reused footer button).
  const firstStep = useRef(true);
  useEffect(() => {
    if (firstStep.current) {
      firstStep.current = false;
      return;
    }
    const frame = requestAnimationFrame(() => {
      document.querySelector<HTMLElement>("[data-import-focus]")?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [step]);

  const finishLabel = variant === "wizard" ? t("common.continue") : t("common.done");
  const leaveLabel = variant === "wizard" ? t("common.skip") : t("common.cancel");
  const preview = analysis?.preview ?? null;
  const noticeBox = notice && (
    <div className="callout callout-info import-notice" role="status">
      <Info />
      <span>{notice}</span>
    </div>
  );

  // ------------------------------------------------------------------ choose

  if (step === "choose") {
    const legacyList = legacy.length > 0 && (
      <section className="import-legacy" aria-label={t("importFlow.foundLegacy")}>
        <div className="import-label">{t("importFlow.foundLegacy")}</div>
        {legacy.map((vault) => (
          <button
            key={vault.path}
            type="button"
            className="import-legacy-option"
            disabled={busy !== null}
            onClick={() => void analyze(vault.path, null)}
          >
            <span className="import-legacy-icon" aria-hidden>
              <Archive />
            </span>
            <span className="import-legacy-text">
              <span className="import-legacy-name">{vault.name}</span>
              <span className="import-legacy-path" title={vault.path}>
                {vault.path}
              </span>
            </span>
            <span className="chip">VaultX 1.x</span>
            <ChevronRight className="import-legacy-chevron" aria-hidden />
          </button>
        ))}
      </section>
    );
    const body = (
      <div className="import-flow">
        {noticeBox}
        {variant === "wizard" && legacyList}
        <button
          type="button"
          className={`import-drop ${busy === "analyze" ? "busy" : ""}`}
          data-import-focus
          onClick={() => void pick()}
          disabled={busy !== null}
          aria-describedby={hintId}
        >
          <span className="import-drop-icon" aria-hidden>
            {busy === "analyze" ? <span className="spinner" /> : <FileUp />}
          </span>
          <span className="import-drop-title">
            {busy === "analyze" && file ? t("importFlow.checking", { name: file.name }) : t("importFlow.dropTitle")}
          </span>
          <span className="import-drop-hint" id={hintId}>
            {t("importFlow.dropHint")}
          </span>
        </button>
        {error && (
          <div className="callout callout-danger" role="alert">
            <CircleAlert />
            <div className="import-error">
              {file && <strong>{file.name}</strong>}
              <span>{error.text}</span>
              {file && error.retry && busy === null && (
                <button type="button" className="btn-link" onClick={() => void analyze(file.path, password || null)}>
                  {t("importFlow.retry")}
                </button>
              )}
            </div>
          </div>
        )}
        {variant === "dialog" && legacyList}
        <section className="import-sources" aria-label={t("importFlow.supported")}>
          <div className="import-label">{t("importFlow.supported")}</div>
          <ul>
            <SourceItem icon={<Globe />} title={t("importFlow.srcBrowser")} desc={t("importFlow.srcBrowserDesc")} />
            <SourceItem icon={<Shield />} title={t("importFlow.srcBitwarden")} desc={t("importFlow.srcBitwardenDesc")} />
            <SourceItem
              icon={<Logo variant="glyph" size={18} />}
              title={t("importFlow.srcKeystead")}
              desc={t("importFlow.srcKeysteadDesc")}
            />
            <SourceItem icon={<Archive />} title={t("importFlow.srcLegacy")} desc={t("importFlow.srcLegacyDesc")} />
          </ul>
        </section>
      </div>
    );
    return children({
      step,
      title: t("importFlow.title"),
      subtitle: t("importFlow.subtitle"),
      icon: <FileUp />,
      body,
      footer: (
        <Button variant={variant === "wizard" ? "ghost" : "secondary"} onClick={onFinish}>
          {leaveLabel}
        </Button>
      ),
      onSubmit: () => undefined,
      dismissable: true,
    });
  }

  // ---------------------------------------------------------------- password

  if (step === "password" && analysis && file) {
    const legacyFile = analysis.format === "legacy";
    const body = (
      <div className="import-flow">
        {noticeBox}
        <FileRow name={analysis.fileName} format={t(FORMAT_LABEL[analysis.format])} />
        <Field
          label={legacyFile ? t("import.legacyPassword") : t("import.filePassword")}
          htmlFor={passwordId}
          error={passwordError}
          hint={legacyFile ? t("import.legacyPasswordHint") : t("importFlow.filePasswordHint")}
        >
          <PasswordInput
            id={passwordId}
            value={password}
            onChange={(value) => {
              setPassword(value);
              setPasswordError(null);
            }}
            invalid={Boolean(passwordError)}
            mono={false}
            size={variant === "wizard" ? "lg" : "md"}
            autoFocus
            data-autofocus
            data-import-focus
          />
        </Field>
      </div>
    );
    return children({
      step,
      title: t("importFlow.passwordTitle"),
      subtitle: t("importFlow.passwordSubtitle", { name: analysis.fileName }),
      icon: <KeyRound />,
      body,
      footer: (
        <>
          <Button variant={variant === "wizard" ? "ghost" : "secondary"} onClick={backToChoose} disabled={busy !== null}>
            {t("common.back")}
          </Button>
          <Button type="submit" variant="primary" loading={busy === "analyze"} size={variant === "wizard" ? "lg" : "md"}>
            {t("common.continue")}
          </Button>
        </>
      ),
      onSubmit: submitPassword,
      dismissable: true,
    });
  }

  // ----------------------------------------------------------------- preview

  if (step === "preview" && analysis && preview) {
    const conflicts = preview.conflicts;
    const toImport = preview.newCount + (mode === "skip" ? 0 : conflicts.length);
    const nothingToDo = preview.newCount === 0 && conflicts.length === 0;
    const body = (
      <div className="import-flow">
        {noticeBox}
        <FileRow name={analysis.fileName} format={t(FORMAT_LABEL[analysis.format])} />
        <div className="import-stats">
          <Stat tone="new" icon={<Plus />} n={preview.newCount} label={t("importFlow.statNew")} hint={t("importFlow.statNewHint")} />
          {/* Only what the file contains; "neu" always (also 0). */}
          {preview.duplicates.length > 0 && (
            <Stat
              tone="same"
              icon={<CopyCheck />}
              n={preview.duplicates.length}
              label={t("importFlow.statDuplicates")}
              hint={t("importFlow.statSkippedHint")}
            />
          )}
          {conflicts.length > 0 && (
            <Stat
              tone="conflict"
              icon={<KeyRound />}
              n={conflicts.length}
              label={t("importFlow.statConflicts")}
              hint={t("importFlow.statConflictsHint")}
            />
          )}
          {preview.invalid > 0 && (
            <Stat
              tone="invalid"
              icon={<Ban />}
              n={preview.invalid}
              label={tp("importFlow.statInvalid", preview.invalid)}
              hint={t("importFlow.statSkippedHint")}
            />
          )}
        </div>

        {nothingToDo && (
          <div className="callout callout-success import-all-present">
            <CircleCheck />
            <div>
              <strong>{preview.duplicates.length > 0 ? t("importFlow.allPresentTitle") : t("importFlow.emptyTitle")}</strong>
              <div>
                {preview.duplicates.length > 0
                  ? tp("importFlow.allPresentText", preview.duplicates.length, { file: analysis.fileName })
                  : t("importFlow.emptyText", { file: analysis.fileName })}
              </div>
            </div>
          </div>
        )}

        {conflicts.length > 0 && (
          <section className="import-conflicts">
            <div className="import-conflicts-head">
              <strong>{tp("importFlow.conflictsList", conflicts.length)}</strong>
              <p>{t("importFlow.conflictsQuestion")}</p>
            </div>
            <ModeChoice value={mode} onChange={setMode} disabled={busy !== null} />
            <details className="import-details inset" open={conflicts.length <= 3}>
              <summary>{t("importFlow.showEntries", { n: conflicts.length })}</summary>
              <MatchList items={conflicts} />
            </details>
          </section>
        )}

        {preview.duplicates.length > 0 && (
          <details className="import-details">
            <summary>{tp("importFlow.duplicatesList", preview.duplicates.length)}</summary>
            <MatchList items={preview.duplicates} />
          </details>
        )}

        {preview.warnings.length > 0 && <Warnings warnings={preview.warnings} />}
        {error && (
          <div className="callout callout-danger" role="alert">
            <CircleAlert />
            <span>{error.text}</span>
          </div>
        )}
      </div>
    );
    return children({
      step,
      title: t("importFlow.previewTitle"),
      subtitle: t("importFlow.previewSubtitle"),
      icon: <FileUp />,
      body,
      footer: nothingToDo ? (
        <>
          <Button variant="ghost" className="footer-start" onClick={backToChoose}>
            {t("importFlow.otherFile")}
          </Button>
          <Button variant="primary" onClick={onFinish} size={variant === "wizard" ? "lg" : "md"} data-autofocus data-import-focus>
            {variant === "wizard" ? t("common.continue") : t("common.close")}
          </Button>
        </>
      ) : (
        <>
          <Button variant="ghost" className="footer-start" onClick={backToChoose} disabled={busy !== null}>
            {t("importFlow.otherFile")}
          </Button>
          {variant === "dialog" && (
            <Button variant="secondary" onClick={onFinish} disabled={busy !== null}>
              {t("common.cancel")}
            </Button>
          )}
          <Button
            variant="primary"
            icon={<Check />}
            loading={busy === "commit"}
            disabled={toImport === 0}
            title={toImport === 0 ? t("importFlow.nothingSelectedHint") : undefined}
            onClick={() => void commit()}
            size={variant === "wizard" ? "lg" : "md"}
            data-import-focus
          >
            {toImport === 0 ? t("importFlow.nothingSelected") : tp("importFlow.importN", toImport)}
          </Button>
        </>
      ),
      onSubmit: () => undefined,
      dismissable: busy !== "commit",
    });
  }

  // ------------------------------------------------------------------ result

  const done = report ?? { imported: 0, updated: 0, skipped: 0, duplicates: [], conflictsSkipped: [], warnings: [] };
  const skippedEntries: { match: ImportMatch; reason: "exists" | "conflict" }[] = [
    ...done.conflictsSkipped.map((match) => ({ match, reason: "conflict" as const })),
    ...done.duplicates.map((match) => ({ match, reason: "exists" as const })),
  ];
  const body = (
    <div className="import-flow">
      {noticeBox}
      <div className="import-result">
        <div className="success-badge">
          <Check />
        </div>
        <div className="import-result-counts">
          <span className="import-count new">{t("importFlow.doneImported", { n: done.imported })}</span>
          <span className="import-count">{t("importFlow.doneUpdated", { n: done.updated })}</span>
          <span className="import-count">{t("importFlow.doneSkipped", { n: skippedEntries.length })}</span>
        </div>
      </div>
      {skippedEntries.length > 0 && (
        <details className="import-details">
          <summary>{t("importFlow.skippedList", { n: skippedEntries.length })}</summary>
          <MatchList
            items={skippedEntries.map((e) => e.match)}
            reasons={skippedEntries.map((e) =>
              e.reason === "conflict" ? t("importFlow.reasonConflict") : t("importFlow.reasonExists"),
            )}
          />
        </details>
      )}
      {done.warnings.length > 0 && <Warnings warnings={done.warnings} />}
    </div>
  );
  return children({
    step: "result",
    title: t("importFlow.doneTitle"),
    icon: <CircleCheck />,
    body,
    footer: (
      // The wizard's single action spans the card, like its other steps.
      <Button
        variant="primary"
        onClick={onFinish}
        size={variant === "wizard" ? "lg" : "md"}
        block={variant === "wizard"}
        data-autofocus
        data-import-focus
      >
        {finishLabel}
      </Button>
    ),
    onSubmit: () => undefined,
    dismissable: true,
  });
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

/** Errors where checking the same file again can help (unlike an unknown format). */
function isRetryable(err: unknown): boolean {
  return !(err instanceof ApiError) || !["unsupported", "corrupt", "invalid_input"].includes(err.code);
}

function SourceItem({ icon, title, desc }: { icon: ReactNode; title: string; desc: string }) {
  return (
    <li>
      <span className="import-source-icon" aria-hidden>
        {icon}
      </span>
      <span>
        <span className="import-source-title">{title}</span>
        <span className="import-source-desc">{desc}</span>
      </span>
    </li>
  );
}

function FileRow({ name, format }: { name: string; format: string }) {
  const { t } = useT();
  return (
    <div className="import-file">
      <span className="import-file-icon" aria-hidden>
        <FileText />
      </span>
      <span className="import-file-name" title={name}>
        {name}
      </span>
      <span className="chip accent">
        <Check />
        {t("importFlow.detected", { format })}
      </span>
    </div>
  );
}

function Stat({
  tone,
  icon,
  n,
  label,
  hint,
}: {
  tone: "new" | "same" | "conflict" | "invalid";
  icon: ReactNode;
  n: number;
  label: string;
  hint: string;
}) {
  return (
    <div className={`import-stat ${tone} ${n === 0 ? "zero" : ""}`}>
      <span className="import-stat-icon" aria-hidden>
        {icon}
      </span>
      <span className="import-stat-n">{n}</span>
      <span className="import-stat-label">{label}</span>
      <span className="import-stat-hint">{hint}</span>
    </div>
  );
}

/** Name, username and site of import entries (never a password). */
function MatchList({ items, reasons }: { items: (ImportMatch | ImportConflict)[]; reasons?: string[] }) {
  const { t } = useT();
  const shown = items.slice(0, LIST_LIMIT);
  return (
    <ul className="import-list">
      {shown.map((item, idx) => {
        const secondary = [item.username || (item.itemType === "login" ? t("importFlow.noUsername") : ""), item.site]
          .filter(Boolean)
          .join(" · ");
        const conflict = "conflictId" in item ? item : null;
        const tag = reasons?.[idx]
          ? reasons[idx]
          : !item.existingId
            ? t("importFlow.inFile")
            : conflict?.reason === "totp"
              ? t("importFlow.otherTotp")
              : item.existingName && item.existingName !== item.incomingName
                ? t("importFlow.savedAs", { name: item.existingName })
                : "";
        return (
          <li key={`${item.existingId}-${idx}`}>
            <span className="import-list-text">
              <span className="import-list-name">{item.incomingName || item.existingName}</span>
              {secondary && <span className="import-list-sub">{secondary}</span>}
            </span>
            {tag && <span className="import-list-tag">{tag}</span>}
          </li>
        );
      })}
      {items.length > shown.length && (
        <li className="import-list-more">{t("importFlow.more", { n: items.length - shown.length })}</li>
      )}
    </ul>
  );
}

function ModeChoice({
  value,
  onChange,
  disabled,
}: {
  value: ConflictMode;
  onChange: (mode: ConflictMode) => void;
  disabled: boolean;
}) {
  const { t } = useT();
  const name = useId();
  const options: { value: ConflictMode; title: MessageKey; desc: MessageKey }[] = [
    { value: "skip", title: "importFlow.modeSkip", desc: "importFlow.modeSkipDesc" },
    { value: "update", title: "importFlow.modeUpdate", desc: "importFlow.modeUpdateDesc" },
    { value: "keepBoth", title: "importFlow.modeKeepBoth", desc: "importFlow.modeKeepBothDesc" },
  ];
  return (
    <div className="import-modes" role="radiogroup" aria-label={t("importFlow.conflictsQuestion")}>
      {options.map((option) => (
        <label key={option.value} className={`import-mode ${value === option.value ? "selected" : ""}`}>
          <input
            type="radio"
            name={name}
            checked={value === option.value}
            disabled={disabled}
            onChange={() => onChange(option.value)}
          />
          <span className="import-mode-text">
            <span className="import-mode-title">
              {t(option.title)}
              {option.value === "skip" && <span className="chip">{t("importFlow.default")}</span>}
            </span>
            <span className="import-mode-desc">{t(option.desc)}</span>
          </span>
        </label>
      ))}
    </div>
  );
}

function Warnings({ warnings }: { warnings: string[] }) {
  const { tp } = useT();
  return (
    <details className="import-details subtle">
      <summary>{tp("importFlow.warnings", warnings.length)}</summary>
      <ul className="import-warnings">
        {warnings.slice(0, LIST_LIMIT).map((warning, idx) => (
          <li key={idx}>{warning}</li>
        ))}
      </ul>
    </details>
  );
}
