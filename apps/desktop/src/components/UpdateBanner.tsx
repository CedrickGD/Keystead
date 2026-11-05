import { useEffect, useId, useState } from "react";
import { ChevronDown, CircleAlert, CircleArrowUp, Download, ExternalLink, RefreshCw, X } from "lucide-react";
import { ApiError, api, openExternal } from "../lib/api";
import type { PhoneUpdate } from "../lib/types";
import { useT } from "../i18n";
import { useUpdate } from "../state/update";
import { Button } from "./Controls";

/** Error text of a failed update (network errors read better than the generic `io` text). */
export function useUpdateErrorText(): (err: unknown) => string {
  const { t, errorText } = useT();
  return (err: unknown) =>
    err instanceof ApiError && err.code === "io" ? t("update.errorNetwork", { detail: err.detail }) : errorText(err);
}

function formatMb(bytes: number, locale: string): string {
  return new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(bytes / (1024 * 1024));
}

/**
 * Slim bar at the top of the main window and the unlock screen: a new
 * version is available ([Jetzt aktualisieren] / portable: [Herunterladen],
 * [Später], "Was ist neu?"), then the download progress and the restart.
 */
export function UpdateBanner() {
  const { t, locale } = useT();
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
                    ? t("update.progress", {
                        done: formatMb(progress.downloaded, locale),
                        total: formatMb(total, locale),
                        percent: percent ?? 0,
                      })
                    : t("update.progressUnknown", { done: formatMb(progress.downloaded, locale) })
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
      ? new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(date)
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

/** The phone's check this start (asked once; the banner shows on every screen). */
let phoneCheck: Promise<PhoneUpdate> | null = null;
const PHONE_DISMISSED_KEY = "keystead.phoneUpdateDismissed";

function dismissedVersion(): string | null {
  try {
    return localStorage.getItem(PHONE_DISMISSED_KEY);
  } catch {
    return null;
  }
}

/**
 * The phone: a newer version on the update channel – from the welcome screen
 * on, so no vault is needed. "Herunterladen" opens its APK in the browser;
 * Android's installer updates the app over the installed one (the vaults
 * stay). Asked once per start, a few seconds in; "Später" hides that version.
 */
export function PhoneUpdateBanner() {
  const { t } = useT();
  const [update, setUpdate] = useState<PhoneUpdate | null>(null);
  const [dismissed, setDismissed] = useState(dismissedVersion);
  const [notesOpen, setNotesOpen] = useState(false);
  const notesId = useId();

  useEffect(() => {
    let gone = false;
    const timer = window.setTimeout(() => {
      phoneCheck ??= api.phoneCheckUpdate(false);
      phoneCheck
        .then((u) => {
          if (!gone) setUpdate(u);
        })
        .catch(() => {
          phoneCheck = null; // offline: try again at the next screen
        });
    }, 3000);
    return () => {
      gone = true;
      window.clearTimeout(timer);
    };
  }, []);

  if (!update?.available || !update.apkUrl || !update.version || dismissed === update.version) return null;
  const { apkUrl, version, notes } = update;
  const later = () => {
    try {
      localStorage.setItem(PHONE_DISMISSED_KEY, version);
    } catch {
      /* only for this start */
    }
    setDismissed(version);
  };

  return (
    <div className="update-banner" role="region" aria-label={t("update.regionLabel")}>
      <div className="update-row">
        <span className="update-icon">
          <CircleArrowUp />
        </span>
        <div className="update-text">
          <strong>{t("update.available", { version })}</strong>
          <span className="update-sub">
            {t("update.phoneHint")}
            {notes && (
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
            )}
          </span>
        </div>
        <div className="update-actions">
          <Button size="sm" variant="primary" icon={<Download />} onClick={() => openExternal(apkUrl)}>
            {t("update.download")}
          </Button>
          <Button size="sm" variant="ghost" onClick={later}>
            {t("update.later")}
          </Button>
        </div>
      </div>
      {notes && notesOpen && (
        <div className="update-notes" id={notesId}>
          <div className="update-notes-text selectable">{notes}</div>
        </div>
      )}
    </div>
  );
}
