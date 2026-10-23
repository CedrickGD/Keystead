// Desktop: a vault in the cloud folder this PC does not have yet – e.g. one
// made on the phone (`folder_offers`, `mirror://offers`, state/mirror.tsx).
// The PC never takes it on its own: "Übernehmen" (`adopt_folder_vault`)
// puts it into the vault list, "Nicht übernehmen" (`dismiss_folder_vault`)
// stops asking; closing the dialog asks again after the next start. The
// same offers are listed in Settings → Handy.

import { useState } from "react";
import { FolderDown } from "lucide-react";
import type { FolderOffer } from "../lib/types";
import { useT } from "../i18n";
import { useMirror } from "../state/mirror";
import { Modal } from "./Modal";
import { Button } from "./Controls";

/** Shows the first offer not put off in this session. */
export function FolderOfferHost() {
  const mirror = useMirror();
  // Closed with the X / Escape: not asked again until the next start.
  const [snoozed, setSnoozed] = useState<string[]>([]);
  const offer = mirror?.offers.find((o) => !snoozed.includes(o.vaultId));
  if (!mirror || !offer) return null;
  return (
    <FolderOfferDialog
      key={offer.vaultId}
      offer={offer}
      busy={mirror.offerBusy === offer.vaultId}
      onAdopt={() => void mirror.adopt(offer)}
      onDismiss={() => void mirror.dismiss(offer)}
      onClose={() => setSnoozed((list) => [...list, offer.vaultId])}
    />
  );
}

export function FolderOfferDialog({
  offer,
  busy,
  onAdopt,
  onDismiss,
  onClose,
}: {
  offer: FolderOffer;
  busy: boolean;
  onAdopt: () => void;
  onDismiss: () => void;
  onClose: () => void;
}) {
  const { t, formatRelative, formatDateTime } = useT();
  return (
    <Modal
      title={t("sync.offer.title")}
      icon={<FolderDown />}
      onClose={onClose}
      dismissable={!busy}
      footer={
        <>
          <Button variant="secondary" onClick={onDismiss} disabled={busy}>
            {t("sync.offer.dismiss")}
          </Button>
          <Button variant="primary" onClick={onAdopt} loading={busy}>
            {t("sync.offer.adopt")}
          </Button>
        </>
      }
    >
      <div className="offer-vault">
        <strong className="offer-vault-name">{offer.name}</strong>
        <span className="subtle">
          {t("sync.offer.changed", { when: formatRelative(offer.updatedAt) })} · {formatDateTime(offer.updatedAt)}
        </span>
      </div>
      <p>{t("sync.offer.text")}</p>
      <p className="subtle">{t("sync.offer.dismissNote")}</p>
    </Modal>
  );
}
