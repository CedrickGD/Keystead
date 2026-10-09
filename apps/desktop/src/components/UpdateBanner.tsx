import { useId, useState } from "react";
import { ChevronDown, CircleAlert, CircleArrowUp, Download, ExternalLink, RefreshCw, X } from "lucide-react";
import { ApiError, openExternal } from "../lib/api";
import { useT } from "../i18n";
import { useUpdate } from "../state/update";
import { Button } from "./Controls";

/** Error text of a failed update (network errors read better than the generic `io` text). */
export function useUpdateErrorText(): (err: unknown) => string {
  const { t, errorText } = useT();
  return (err: unknown) =>
    err instanceof ApiError && err.code === "io" ? t("update.errorNetwork", { detail: err.detail }) : errorText(err);
}

function formatMb(bytes: number): string {
  return (bytes / (1024 * 1024)).toFixed(1);
}

/**
 * Slim bar at the top of the main window and the unlock screen: a new
 * version is available ([Jetzt aktualisieren] / portable: [Herunterladen],
 * [Später], "Was ist neu?"), then the download progress and the restart.
 */
export function UpdateBanner() {
  const { t, lang } = useT();
  const updateErrorText = useUpdateErrorText();
  const { info, phase, progress, error, bannerVisible, install, dismiss } = useUpdate();
  const [notesOpen, setNotesOpen] = useState(false);
  const notesId = useId();
  if (!bannerVisible || !info) return null;

  const version = info.version ?? "";

  if (phase === "downloading" || phase === "restarting") {
    const total = progress?.total ?? null;
    const percent = total && progress ? Math.min(100, Math.round((progress.downloaded / total) * 100)) : null;
    const restarting = phase === "restarting";
    return (
      <div className="update-banner busy" role="status" aria-live="polite">
        <div className="update-row">
          <span className="update-icon">
            {restarting ? <RefreshCw className="spin" /> : <Download />}
          </span>
          <div className="update-text">
            <strong>{restarting ? t("update.restarting") : t("update.downloading", { version })}</strong>
            <span className="update-sub">
              {restarting
                ? t("update.restartingHint")
                : progress
                  ? total
                    ? t("update.progress", { done: formatMb(progress.downloaded), total: formatMb(total), percent: percent ?? 0 })
                    : t("update.progressUnknown", { done: formatMb(progress.downloaded) })
                  : t("update.lockHint")}
            </span>
          </div>
        </div>
        <div
          className={`update-progress ${percent === null && !restarting ? "indeterminate" : ""}`}
          role="progressbar"
          aria-label={t("update.progressLabel")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={restarting ? 100 : (percent ?? undefined)}
        >
          <span style={{ width: `${restarting ? 100 : (percent ?? 0)}%` }} />
        </div>
      </div>
    );
  }

  if (phase === "error") {
    return (
      <div className="update-banner error" role="alert">
        <div className="update-row">
          <span className="update-icon">
            <CircleAlert />
          </span>
          <div className="update-text">
            <strong>{t("update.failed")}</strong>
            <span className="update-sub">{updateErrorText(error)}</span>
          </div>
          <div className="update-actions">
            <Button size="sm" variant="primary" onClick={() => void install()}>
              {t("common.retry")}
            </Button>
            <button type="button" className="icon-btn sm" onClick={dismiss} aria-label={t("common.close")} title={t("common.close")}>
              <X />
            </button>
          </div>
        </div>
      </div>
    );
  }

  const date = info.date ? new Date(info.date) : null;
  const dateText =
    date && !Number.isNaN(date.getTime())
      ? new Intl.DateTimeFormat(lang === "de" ? "de-DE" : "en-US", { dateStyle: "medium" }).format(date)
      : null;

  return (
    <div className="update-banner" role="region" aria-label={t("update.regionLabel")}>
      <div className="update-row">
        <span className="update-icon">
          <CircleArrowUp />
        </span>
        <div className="update-text">
          <strong>{t("update.available", { version })}</strong>
          <span className="update-sub">
            {info.canInstall ? t("update.availableHint") : t("update.portableHint")}
            {info.notes ? (
              <>
                {" · "}
                <button
                  type="button"
                  className="link-btn"
                  aria-expanded={notesOpen}
                  aria-controls={notesId}
                  onClick={() => setNotesOpen((open) => !open)}
                >
                  {t("update.whatsNew")}
                  <ChevronDown className={`chev ${notesOpen ? "open" : ""}`} aria-hidden />
                </button>
              </>
            ) : (
              <>
                {" · "}
                <button type="button" className="link-btn" onClick={() => openExternal(info.releaseUrl)}>
                  {t("update.releaseNotes")}
                </button>
              </>
            )}
          </span>
        </div>
        <div className="update-actions">
          {info.canInstall ? (
            <Button size="sm" variant="primary" icon={<Download />} onClick={() => void install()}>
              {t("update.installNow")}
            </Button>
          ) : (
            <Button size="sm" variant="primary" icon={<ExternalLink />} onClick={() => void install()}>
              {t("update.download")}
            </Button>
          )}
          <Button size="sm" variant="ghost" onClick={dismiss}>
            {t("update.later")}
          </Button>
        </div>
      </div>
      {info.notes && notesOpen && (
        <div className="update-notes" id={notesId}>
          {dateText && <div className="update-notes-date">{t("update.releasedOn", { date: dateText })}</div>}
          <div className="update-notes-text selectable">{info.notes}</div>
        </div>
      )}
    </div>
  );
}
