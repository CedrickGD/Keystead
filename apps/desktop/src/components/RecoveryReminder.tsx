import { useEffect, useState } from "react";
import { LifeBuoy, TriangleAlert } from "lucide-react";
import { useT } from "../i18n";
import { useApp } from "../state/app";
import { clearRecoveryUnconfirmed, useRecoveryUnconfirmed } from "../lib/recoveryMarker";
import { RecoveryKeyDialog } from "../screens/settings/dialogs";
import { Button } from "./Controls";

/**
 * Persistent warning above the main window: a new recovery key of this vault
 * was shown (a master-password change replaced the old one, or one was
 * created) but the dialog closed – a lock or vault switch reloads the page –
 * before the user confirmed storing it. The old key no longer works and
 * nobody knows the new one, so offer to create a fresh one right away.
 * See `lib/recoveryMarker.ts`.
 */
export function RecoveryReminder() {
  const { t } = useT();
  const { vault, setVault } = useApp();
  const pending = useRecoveryUnconfirmed(vault?.id);
  const [dialog, setDialog] = useState(false);
  const relevant = Boolean(vault && pending && vault.hasRecoveryKey);

  // The vault has no recovery key (any more): nothing to confirm.
  useEffect(() => {
    if (vault && pending && !vault.hasRecoveryKey) clearRecoveryUnconfirmed(vault.id);
  }, [vault, pending]);

  if (!vault || (!relevant && !dialog)) return null;
  return (
    <>
      {relevant && (
        <div className="update-banner warning" role="alert">
          <div className="update-row">
            <span className="update-icon">
              <TriangleAlert />
            </span>
            <div className="update-text">
              <strong>{t("recovery.unconfirmedTitle")}</strong>
              <span className="update-sub">{t("recovery.unconfirmedText")}</span>
            </div>
            <div className="update-actions">
              <Button size="sm" variant="primary" icon={<LifeBuoy />} onClick={() => setDialog(true)}>
                {t("recovery.unconfirmedAction")}
              </Button>
            </div>
          </div>
        </div>
      )}
      {dialog && (
        <RecoveryKeyDialog
          replacing
          onClose={() => setDialog(false)}
          onCreated={() => setVault({ ...vault, hasRecoveryKey: true })}
        />
      )}
    </>
  );
}
