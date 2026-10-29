// The Dropbox login (stage 2d, "Cloud accounts (Dropbox, Google Drive)" in
// docs/ARCHITECTURE.md), on both platforms: "Bei Dropbox anmelden" asks the
// backend for the authorize page (`dropbox_login` – the PKCE verifier stays
// there) and opens it in the browser; Dropbox shows a code after the user
// allowed Keystead's app folder, the user pastes it here, and "Verbinden"
// hands it to the caller's `onConnect` (`link_dropbox` on the phone,
// `set_sync_dropbox` on the PC). Neither the code nor a token is kept by the
// page.

import { useId, useState } from "react";
import { Cloud, ExternalLink, PlugZap, TriangleAlert } from "lucide-react";
import { api, openExternal } from "../lib/api";
import { useT } from "../i18n";
import { Button, Field } from "./Controls";
import { Modal } from "./Modal";

export function DropboxDialog({
  onConnect,
  onClose,
}: {
  /** Links or sets the account with the code; resolves to whether it worked (it shows its own errors). */
  onConnect: (code: string) => Promise<boolean>;
  onClose: () => void;
}) {
  const { t, errorText } = useT();
  const codeId = useId();
  const [opened, setOpened] = useState(false);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState<"open" | "connect" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const open = async () => {
    setBusy("open");
    setError(null);
    try {
      const url = await api.dropboxLogin();
      openExternal(url);
      setOpened(true);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const connect = async () => {
    if (!code.trim()) return;
    setBusy("connect");
    setError(null);
    try {
      if (await onConnect(code.trim())) {
        setCode("");
        onClose();
      }
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal
      title={t("dropbox.title")}
      subtitle={t("dropbox.subtitle")}
      icon={<Cloud />}
      onClose={onClose}
      dismissable={busy === null}
      onSubmit={() => void (opened ? connect() : open())}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy !== null}>
            {t("common.cancel")}
          </Button>
          {opened ? (
            <Button type="submit" variant="primary" icon={<PlugZap />} loading={busy === "connect"} disabled={!code.trim()}>
              {t("dropbox.connect")}
            </Button>
          ) : (
            <Button type="submit" variant="primary" icon={<ExternalLink />} loading={busy === "open"}>
              {t("dropbox.signIn")}
            </Button>
          )}
        </>
      }
    >
      <ol className="sync-steps">
        <li>{t("dropbox.step1")}</li>
        <li>{t("dropbox.step2")}</li>
      </ol>
      {opened && (
        <>
          <Field label={t("dropbox.code")} htmlFor={codeId} hint={t("dropbox.codeHint")}>
            <input
              id={codeId}
              className="input mono"
              value={code}
              onChange={(e) => {
                setCode(e.target.value);
                setError(null);
              }}
              autoComplete="off"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              autoFocus
            />
          </Field>
          <div>
            <Button size="sm" variant="ghost" icon={<ExternalLink />} loading={busy === "open"} onClick={() => void open()}>
              {t("dropbox.openAgain")}
            </Button>
          </div>
        </>
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
