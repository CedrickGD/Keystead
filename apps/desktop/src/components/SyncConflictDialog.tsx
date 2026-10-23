// The conflict dialog of the two-way cloud-folder sync (docs/BLOCK5-DESIGN.md
// "Stufe 2b", "Sync conflicts" in docs/ARCHITECTURE.md): the vault was
// changed on the PC and on the phone since both last agreed. One side is
// kept, the other stays as a conflict copy on this device; "Später" closes
// the dialog until the next sync event or unlock. Same words on both
// devices – the PC's state and the phone's (lib/syncConflict.mjs maps them
// to the backend's "local" / "remote").

import { CopyCheck, Laptop, Paperclip, Smartphone, TriangleAlert } from "lucide-react";
import type { SyncConflict } from "../lib/types";
import { conflictChoices, conflictSides, type ConflictDevice, type ConflictKeep } from "../lib/syncConflict.mjs";
import { isPhonePlatform } from "../lib/phone";
import { useT, type MessageKey } from "../i18n";
import { useApp } from "../state/app";
import { Modal } from "./Modal";
import { Button } from "./Controls";

const LABELS: Record<ConflictDevice, Record<ConflictKeep, MessageKey>> = {
  pc: { local: "sync.conflict.keepPc", remote: "sync.conflict.takePc" },
  phone: { local: "sync.conflict.keepPhone", remote: "sync.conflict.takePhone" },
};

function SideCard({
  device,
  here,
  revision,
  updatedAt,
}: {
  device: ConflictDevice;
  here: boolean;
  revision: number;
  updatedAt: number;
}) {
  const { t, formatRelative, formatDateTime } = useT();
  return (
    <div className="conflict-side">
      <span className="conflict-side-icon" aria-hidden>
        {device === "pc" ? <Laptop /> : <Smartphone />}
      </span>
      <div className="conflict-side-text">
        <div className="conflict-side-title">
          {device === "pc" ? t("sync.conflict.pcState") : t("sync.conflict.phoneState")}
          {here && <span className="chip">{t("sync.conflict.thisDevice")}</span>}
        </div>
        <div className="conflict-side-meta">
          {t("sync.conflict.changed", { when: formatRelative(updatedAt) })}
          <span className="subtle">
            {" "}
            · {formatDateTime(updatedAt)} · {t("sync.conflict.version", { n: revision })}
          </span>
        </div>
      </div>
    </div>
  );
}

export function SyncConflictDialog({
  conflict,
  busy,
  onResolve,
  onLater,
}: {
  conflict: SyncConflict;
  /** The side being kept right now. */
  busy: ConflictKeep | null;
  onResolve: (keep: ConflictKeep) => void;
  onLater: () => void;
}) {
  const { t, tp } = useT();
  const { info } = useApp();
  const phone = isPhonePlatform(info.platform);
  const sides = conflictSides(info.platform);
  const choices = conflictChoices(info.platform, conflict.passwordChanged);
  const side = (device: ConflictDevice) =>
    sides.local === device
      ? { revision: conflict.localRevision, updatedAt: conflict.localUpdatedAt }
      : { revision: conflict.remoteRevision, updatedAt: conflict.remoteUpdatedAt };
  const lost = conflict.lostAttachments ?? 0;
  const remoteOffered = choices.some((c) => c.keep === "remote");

  return (
    <Modal
      title={t("sync.conflict.title")}
      subtitle={t("sync.conflict.text", { name: conflict.vaultName })}
      icon={<CopyCheck />}
      tone="warning"
      onClose={onLater}
      dismissable={busy === null}
      className="conflict-dialog"
      wide
      footer={
        <>
          <Button variant="ghost" className="conflict-later" onClick={onLater} disabled={busy !== null}>
            {t("sync.conflict.later")}
          </Button>
          {choices.map((choice) => (
            <Button
              key={choice.device}
              variant="primary"
              loading={busy === choice.keep}
              disabled={busy !== null && busy !== choice.keep}
              onClick={() => onResolve(choice.keep)}
            >
              {t(LABELS[choice.device][choice.keep])}
            </Button>
          ))}
        </>
      }
    >
      <div className="conflict-sides">
        {(["pc", "phone"] as const).map((device) => (
          <SideCard key={device} device={device} here={sides.local === device} {...side(device)} />
        ))}
      </div>
      {conflict.passwordChanged && (
        <div className="callout callout-warning" role="note">
          <TriangleAlert />
          <span>{phone ? t("sync.conflict.passwordChangedPhone") : t("sync.conflict.passwordChangedPc")}</span>
        </div>
      )}
      {!phone && lost > 0 && remoteOffered && (
        <div className="callout callout-warning" role="note">
          <Paperclip />
          <span>{tp("sync.conflict.lostAttachments", lost)}</span>
        </div>
      )}
      <p className="conflict-note">{phone ? t("sync.conflict.copyNotePhone") : t("sync.conflict.copyNotePc")}</p>
    </Modal>
  );
}
