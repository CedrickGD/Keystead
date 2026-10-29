// The Google Drive login (stage 2d, "Google Drive" in docs/ARCHITECTURE.md),
// on both platforms: "Bei Google anmelden" asks the backend for Google's
// consent page (`gdrive_login` – the PKCE verifier and the state stay there)
// and opens it in the browser; Google sends the user back to the app (PC: a
// loopback listener of the backend, phone: the app's own scheme). The dialog
// asks `gdrive_login_ready` every second and when the window gets the focus
// back; once the redirect came it calls the caller's `onConnect`
// (`link_gdrive` on the phone, `set_sync_gdrive` on the PC). The page never
// sees the code or a token.

import { useEffect, useRef, useState } from "react";
import { ExternalLink, HardDrive, TriangleAlert } from "lucide-react";
import { api, openExternal } from "../lib/api";
import { useT } from "../i18n";
import { Button } from "./Controls";
import { Modal } from "./Modal";

/** How often the dialog asks whether Google sent the user back. */
const POLL_MS = 1000;

export function GdriveDialog({
  onConnect,
  onClose,
}: {
  /** Links or sets the account whose login came back; resolves to whether it worked (it shows its own errors). */
  onConnect: () => Promise<boolean>;
  onClose: () => void;
}) {
  const { t, errorText } = useT();
  const [phase, setPhase] = useState<"start" | "waiting" | "connecting">("start");
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The latest callbacks for the polling effect (it must not restart on a re-render).
  const latest = useRef({ onConnect, onClose, errorText });
  latest.current = { onConnect, onClose, errorText };

  const open = async () => {
    setOpening(true);
    setError(null);
    try {
      const url = await api.gdriveLogin();
      openExternal(url);
      setPhase("waiting");
    } catch (err) {
      setError(errorText(err));
    } finally {
      setOpening(false);
    }
  };

  useEffect(() => {
    if (phase !== "waiting") return;
    let stopped = false;
    let asking = false;
    const check = async () => {
      if (stopped || asking) return;
      asking = true;
      try {
        if (!(await api.gdriveLoginReady()) || stopped) return;
        stopped = true;
        setPhase("connecting");
        if (await latest.current.onConnect()) latest.current.onClose();
        else setPhase("start");
      } catch (err) {
        if (!stopped) {
          stopped = true;
          setError(latest.current.errorText(err));
          setPhase("start");
        }
      } finally {
        asking = false;
      }
    };
    const timer = window.setInterval(() => void check(), POLL_MS);
    const onFocus = () => void check();
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [phase]);

  return (
    <Modal
      title={t("gdrive.title")}
      subtitle={t("gdrive.subtitle")}
      icon={<HardDrive />}
      onClose={onClose}
      dismissable={phase !== "connecting"}
      onSubmit={() => void (phase === "start" ? open() : undefined)}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={phase === "connecting"}>
            {t("common.cancel")}
          </Button>
          {phase === "start" && (
            <Button type="submit" variant="primary" icon={<ExternalLink />} loading={opening}>
              {t("gdrive.signIn")}
            </Button>
          )}
        </>
      }
    >
      <ol className="sync-steps">
        <li>{t("gdrive.step1")}</li>
        <li>{t("gdrive.step2")}</li>
      </ol>
      {phase !== "start" && (
        <div className="gdrive-wait" role="status" aria-live="polite">
          <span className="spinner" aria-hidden />
          <span>{phase === "connecting" ? t("gdrive.connecting") : t("gdrive.waiting")}</span>
        </div>
      )}
      {phase === "waiting" && (
        <div>
          <Button size="sm" variant="ghost" icon={<ExternalLink />} loading={opening} onClick={() => void open()}>
            {t("gdrive.openAgain")}
          </Button>
        </div>
      )}
      {error && (
        <div className="callout callout-danger" role="alert">
          <TriangleAlert />
          <span>{error}</span>
        </div>
      )}
    </Modal>
  );
}
