// The texts of the sync's error codes (`lastError`, a file's `error`), with a
// cloud account's or an SMB share's own words where the server's would
// mislead (stage 2d, "Cloud accounts (Dropbox, Google Drive)", and "SMB
// shares (phone)" in docs/ARCHITECTURE.md). Used by the sync UI
// (components/FolderSync.tsx re-exports it) and the sync state's toasts
// (state/sync.tsx).

import { useCallback } from "react";
import { ApiError } from "./api";
import type { CloudProvider, FolderStatus, SyncFileResult } from "./types";
import { useT } from "../i18n";

/** The provider's name as the texts use it. */
export function providerName(provider: CloudProvider): string {
  return provider === "gdrive" ? "Google Drive" : "Dropbox";
}

/** The cloud account a phone's link goes to (`null`: a folder or a server). */
export function providerOf(status: FolderStatus | null | undefined): CloudProvider | null {
  return status?.kind === "dropbox" || status?.kind === "gdrive" ? status.kind : null;
}

/** Whose words a sync error is told in: a cloud account's or an SMB share's (`null`: the server's). */
export type ErrorSource = CloudProvider | "smb";

/** The words for the errors of this link's sync (`null`: a folder or a WebDAV server). */
export function errorSourceOf(status: FolderStatus | null | undefined): ErrorSource | null {
  return status?.kind === "smb" ? "smb" : providerOf(status);
}

/**
 * The text of a vault file's own error (`SyncFileResult.error`): the sync's texts, except for the
 * cloud folder's bare `not_found` – the cloud app could not open the file; everywhere else that
 * code is a vault item that was not found.
 */
export function useFileErrorText(): (code: string, source?: ErrorSource | null) => string {
  const { t } = useT();
  const syncErrorFor = useSyncErrorText();
  return useCallback(
    (code: string, source?: ErrorSource | null) =>
      code === "not_found" ? t("sync.fileUnavailable") : syncErrorFor(code, source),
    [syncErrorFor, t],
  );
}

/** A vault file the last sync could not read or write (its `error` says why), if any. */
export function failedFile(status: FolderStatus | null | undefined): SyncFileResult | null {
  return status?.files.find((file) => file.outcome === "failed" && file.error) ?? null;
}

/**
 * The text of an error code the sync recorded (`lastError`, a file's `error`). With `source` (a
 * cloud account's or an SMB share's link, or the PC's method) its own words where the server's
 * texts would mislead ("Benutzername und Passwort", "Server-Formular"; for a share in the home
 * network "keine Internetverbindung").
 */
export function useSyncErrorText(): (code: string, source?: ErrorSource | null) => string {
  const { t, errorText } = useT();
  return useCallback(
    (code: string, source?: ErrorSource | null) => {
      if (source === "smb") {
        // The share is in the user's own network: the internet plays no part.
        if (code === "unsupported:sync_offline") return t("sync.smbError.offline");
      } else if (source) {
        const name = providerName(source);
        if (code === "unsupported:sync_auth_failed") return t("sync.cloudError.authFailed", { provider: name });
        if (code === "unsupported:sync_credentials_missing") {
          return t("sync.cloudError.credentialsMissing", { provider: name });
        }
        if (code === "unsupported:sync_server_folder_missing" && source === "gdrive") {
          return t("sync.cloudError.gdriveFolderMissing");
        }
      }
      return errorText(new ApiError(code));
    },
    [errorText, t],
  );
}
