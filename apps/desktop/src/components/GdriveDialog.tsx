// The Google Drive login (stage 2d, "Google Drive" in docs/ARCHITECTURE.md),
// on both platforms: "Bei Google anmelden" asks the backend for the login
// (`gdrive_login` – the PKCE verifier, the state and the device code stay
// there) and opens Google's page in the browser. On the PC Google sends the
// browser back to the backend's loopback listener; on the phone the user
// enters the code shown here (copied for them) at Google's page – the device
// flow, no redirect into the app. The dialog asks `gdrive_login_ready` every
// second and when the window gets the focus back; once Google allowed it,
// it calls the caller's `onConnect` (`link_gdrive` on the phone,
// `set_sync_gdrive` on the PC). Opened again while a login waits (after the
// phone's background lock, say), it continues that login. The page never
// sees a code of Google's or a token.

import { useEffect, useRef, useState } from "react";
import { Check, Copy, ExternalLink, HardDrive, TriangleAlert } from "lucide-react";
import { api, openExternal } from "../lib/api";
import type { GdriveLoginState } from "../lib/types";
import { useT } from "../i18n";
import { Button } from "./Controls";
import { Modal } from "./Modal";

/** How often the dialog asks whether Google allowed the login. */
const POLL_MS = 1000;

export function GdriveDialog({
  onConnect,
  onClose,
}: {
  /** Links or sets the account Google allowed; resolves to whether it worked (it shows its own errors). */
  onConnect: () => Promise<boolean>;
  onClose: () => void;
}) {
  const { t, errorText } = useT();
  const [phase, setPhase] = useState<"start" | "waiting" | "connecting">("start");
  const [login, setLogin] = useState<GdriveLoginState | null>(null);
  const [opening, setOpening] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The latest callbacks for the polling effect (it must not restart on a re-render).
  const latest = useRef({ onConnect, onClose, errorText });
  latest.current = { onConnect, onClose, errorText };

  // A login that still waits (the dialog was closed, the phone locked): continue it.
  useEffect(() => {
    let gone = false;
    api
      .gdriveLoginReady()
      .then((state) => {
        if (gone) return;
        setLogin(state);
        setPhase("waiting");
      })
      .catch(() => {
        /* none waits (or it ended): start from the beginning */
      });
    return () => {
      gone = true;
    };
  }, []);

  const copyCode = async (code: string) => {
    try {
      await api.copyText(code, false);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  const open = async () => {
    setOpening(true);
    setError(null);
    try {
      const state = await api.gdriveLogin();
      setLogin(state);
      if (state.userCode) await copyCode(state.userCode);
      openExternal(state.url);
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
        const state = await api.gdriveLoginReady();
        if (stopped) return;
        setLogin(state);
        if (!state.ready) return;
        stopped = true;
        setPhase("connecting");
        if (await latest.current.onConnect()) latest.current.onClose();
        else setPhase("start");
      } catch (err) {
        if (!stopped) {
          stopped = true;
          setError(latest.current.errorText(err));
          setLogin(null);
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

  const userCode = login?.userCode ?? null;
  const device = userCode !== null;
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
        <li>{device ? t("gdrive.stepDevice1") : t("gdrive.step1")}</li>
        <li>{device ? t("gdrive.stepDevice2") : t("gdrive.step2")}</li>
      </ol>
      {userCode && phase !== "start" && (
        <div className="gdrive-code">
          <span className="gdrive-code-label">{t("gdrive.code")}</span>
          <code className="gdrive-code-value selectable" aria-live="polite">
            {userCode}
          </code>
          <Button
            size="sm"
            variant="ghost"
            icon={copied ? <Check /> : <Copy />}
            onClick={() => void copyCode(userCode)}
          >
            {copied ? t("gdrive.codeCopied") : t("gdrive.copyCode")}
          </Button>
        </div>
      )}
      {phase !== "start" && (
        <div className="gdrive-wait" role="status" aria-live="polite">
          <span className="spinner" aria-hidden />
          <span>{phase === "connecting" ? t("gdrive.connecting") : t("gdrive.waiting")}</span>
        </div>
      )}
      {phase === "waiting" && login && (
        <div>
          <Button size="sm" variant="ghost" icon={<ExternalLink />} onClick={() => openExternal(login.url)}>
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
