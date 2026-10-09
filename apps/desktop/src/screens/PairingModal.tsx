import { useEffect, useState } from "react";
import { Link2 } from "lucide-react";
import { api } from "../lib/api";
import type { PairingRequest } from "../lib/types";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";
import { Modal } from "../components/Modal";
import { Button } from "../components/Controls";

/** The bridge keeps a pairing request open for 120 s. */
const PAIRING_TIMEOUT_S = 120;

export function PairingModal({ request, onDone }: { request: PairingRequest; onDone: () => void }) {
  const { t, errorText } = useT();
  const toast = useToast();
  const [busy, setBusy] = useState<"approve" | "deny" | null>(null);
  const [left, setLeft] = useState(PAIRING_TIMEOUT_S);

  useEffect(() => {
    const started = Date.now();
    const timer = window.setInterval(() => {
      const remaining = PAIRING_TIMEOUT_S - Math.floor((Date.now() - started) / 1000);
      setLeft(remaining);
      if (remaining <= 0) {
        window.clearInterval(timer);
        toast.info(t("pairing.expired"));
        onDone();
      }
    }, 1000);
    return () => window.clearInterval(timer);
  }, [onDone, t, toast]);

  const respond = async (approve: boolean) => {
    setBusy(approve ? "approve" : "deny");
    try {
      await api.respondPairing(request.requestId, approve);
      if (approve) toast.success(t("pairing.approved", { name: request.clientName }));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(null);
      onDone();
    }
  };

  const code = request.code.replace(/\D/g, "");
  const formatted = code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : request.code;

  return (
    <Modal
      title={t("pairing.title")}
      subtitle={t("pairing.subtitle", { name: request.clientName })}
      icon={<Link2 />}
      onClose={() => void respond(false)}
      dismissable={busy === null}
      footer={
        <>
          <Button variant="secondary" onClick={() => void respond(false)} loading={busy === "deny"} disabled={busy !== null}>
            {t("pairing.deny")}
          </Button>
          <Button variant="primary" onClick={() => void respond(true)} loading={busy === "approve"} disabled={busy !== null} data-autofocus>
            {t("pairing.approve")}
          </Button>
        </>
      }
    >
      <div className="pairing-code" aria-label={t("pairing.codeAria", { code })}>
        {formatted}
      </div>
      <p className="pairing-hint">{t("pairing.hint")}</p>
      <p className="pairing-timer">{t("pairing.expiresIn", { seconds: Math.max(0, left) })}</p>
    </Modal>
  );
}
