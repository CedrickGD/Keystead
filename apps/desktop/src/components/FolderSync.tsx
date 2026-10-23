// Building blocks of the phone's cloud-folder sync UI ("Cloud-folder sync
// (phone)" in docs/ARCHITECTURE.md): the way a vault reaches the phone, the
// status line of the unlock screen and the per-vault results. The state and
// actions come from `useFolderSync()` (state/sync.tsx).

import { useCallback, useEffect, useState } from "react";
import { CloudOff, FolderSync as FolderSyncIcon, RefreshCw, TriangleAlert } from "lucide-react";
import { ApiError } from "../lib/api";
import type { FolderStatus, SyncFileResult } from "../lib/types";
import { useT, type MessageKey } from "../i18n";
import { useFolderSync } from "../state/sync";

/** The text of an error code the sync recorded (`lastError`, a file's `error`). */
export function useSyncErrorText(): (code: string) => string {
  const { errorText } = useT();
  return useCallback((code: string) => errorText(new ApiError(code)), [errorText]);
}

const OUTCOME_KEYS: Record<SyncFileResult["outcome"], MessageKey> = {
  imported: "sync.outcome.imported",
  updated: "sync.outcome.updated",
  upToDate: "sync.outcome.upToDate",
  pcOlder: "sync.outcome.pcOlder",
  otherVault: "sync.outcome.otherVault",
  misnamed: "sync.outcome.misnamed",
  unreadable: "sync.outcome.unreadable",
  tooLarge: "sync.outcome.tooLarge",
  failed: "sync.outcome.failed",
  missing: "sync.outcome.missing",
  rejected: "sync.outcome.rejected",
  pushed: "sync.outcome.pushed",
  conflict: "sync.outcome.conflict",
  notWritable: "sync.outcome.notWritable",
  localOnly: "sync.outcome.localOnly",
};

/** Re-renders every 30 s, so a relative time ("vor 2 Minuten") stays current. */
export function useRelativeTimeTick(active: boolean): void {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setTick((n) => n + 1), 30_000);
    return () => window.clearInterval(timer);
  }, [active]);
}

/** Whether the phone has a usable copy of this file's vault. */
export function isUsable(file: SyncFileResult): boolean {
  return ["imported", "updated", "upToDate", "pcOlder", "missing"].includes(file.outcome);
}

/** The status's lost-folder error (the user has to pick the folder again). */
export function folderLost(status: FolderStatus | null): boolean {
  return status?.lastError === "unsupported:folder_access_lost";
}

/** The steps that bring a vault from the PC to the phone. */
export function SyncSteps() {
  const { t } = useT();
  return (
    <ol className="sync-steps">
      <li>{t("sync.stepPc")}</li>
      <li>{t("sync.stepCloudApp")}</li>
      <li>{t("sync.stepPhone")}</li>
    </ol>
  );
}

/** What the last sync did with each vault file of the folder. */
export function SyncFileList({ files }: { files: SyncFileResult[] }) {
  const { t, tp } = useT();
  if (files.length === 0) return null;
  return (
    <ul className="sync-files">
      {files.map((file) => {
        const extras = [
          file.passwordChanged ? t("sync.passwordChanged") : null,
          file.attachmentsCopied > 0 ? tp("sync.attachmentsCopied", file.attachmentsCopied) : null,
          file.attachmentsFailed > 0 ? tp("sync.attachmentsFailed", file.attachmentsFailed) : null,
        ].filter((x): x is string => x !== null);
        return (
          <li key={file.fileName} className={`sync-file ${isUsable(file) ? "" : "skipped"}`}>
            <span className="sync-file-name truncate">{file.vaultName ?? file.fileName}</span>
            <span className="sync-file-outcome">
              {t(OUTCOME_KEYS[file.outcome])}
              {extras.length > 0 && ` · ${extras.join(" · ")}`}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * The unlock screen's line: last sync and "Jetzt synchronisieren", a lost
 * folder, or a way to link one. `vaultId`: the selected vault (its "PC's
 * copy is older" note).
 */
export function SyncStatusLine({ vaultId }: { vaultId: string | undefined }) {
  const { t, formatRelative } = useT();
  const folderSync = useFolderSync();
  const errorFor = useSyncErrorText();
  useRelativeTimeTick(Boolean(folderSync?.status?.lastSyncAt));
  if (!folderSync?.status) return null;
  const { status, busy, link, sync } = folderSync;

  if (!status.linked) {
    return (
      <div className="sync-line">
        <CloudOff aria-hidden />
        <span className="sync-line-text">{t("sync.notLinked")}</span>
        <button type="button" className="btn-link" onClick={() => void link()} disabled={busy !== null}>
          {t("sync.connect")}
        </button>
      </div>
    );
  }
  if (folderLost(status)) {
    return (
      <div className="sync-line warning" role="status">
        <TriangleAlert aria-hidden />
        <span className="sync-line-text">{t("sync.accessLost")}</span>
        <button type="button" className="btn-link" onClick={() => void link()} disabled={busy !== null}>
          {t("sync.repick")}
        </button>
      </div>
    );
  }
  const syncing = status.syncing || busy === "sync";
  const selected = status.files.find((f) => f.vaultId === vaultId);
  return (
    <div className="sync-line-wrap">
      <div className={`sync-line ${status.lastError ? "warning" : ""}`} role="status">
        {status.lastError ? <TriangleAlert aria-hidden /> : <FolderSyncIcon aria-hidden />}
        <span className="sync-line-text">
          {syncing
            ? t("sync.syncing")
            : status.lastError
              ? errorFor(status.lastError)
              : status.lastSyncAt
                ? t("sync.lastSync", { when: formatRelative(status.lastSyncAt) })
                : t("sync.neverSynced")}
        </span>
        <button
          type="button"
          className="icon-btn"
          onClick={() => void sync()}
          disabled={syncing || busy !== null}
          aria-label={t("sync.syncNow")}
          title={t("sync.syncNow")}
        >
          {syncing ? <span className="spinner" aria-hidden /> : <RefreshCw />}
        </button>
      </div>
      {selected?.outcome === "pcOlder" && <p className="sync-line-note">{t("sync.pcOlderHint")}</p>}
    </div>
  );
}
