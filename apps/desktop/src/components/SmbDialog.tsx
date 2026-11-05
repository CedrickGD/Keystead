// The phone's SMB form ("Mit SMB-Freigabe verbinden"): a folder on a Windows
// share or a NAS – server, share, folder, user, password (and a domain, if
// the network has one). "Verbindung testen" checks share, login, folder and
// write access (`smb_test`; nothing is stored), "Ordner anlegen" makes a
// missing folder (`smb_create_folder`), "Verbinden" links it (`link_smb`,
// the password is sealed on the device). The password lives in this
// dialog's state only. See "SMB shares (phone)" in docs/ARCHITECTURE.md.

import { useId, useState } from "react";
import { CircleCheck, FolderPlus, HardDrive, PlugZap, TriangleAlert } from "lucide-react";
import { ApiError, api } from "../lib/api";
import type { SmbConfig, SmbTest, SyncSmb } from "../lib/types";
import { useT } from "../i18n";
import { Button, Field, PasswordInput } from "./Controls";
import { Modal } from "./Modal";

export function SmbDialog({
  initial,
  onConnect,
  onClose,
}: {
  /** The linked share (to change it), or null for a new one. */
  initial: SyncSmb | null;
  /** Links the share; resolves to whether it worked (it shows its own errors). */
  onConnect: (config: SmbConfig) => Promise<boolean>;
  onClose: () => void;
}) {
  const { t, errorText } = useT();
  const ids = { server: useId(), share: useId(), folder: useId(), user: useId(), password: useId(), domain: useId() };
  const [config, setConfig] = useState<SmbConfig>({
    server: initial?.server ?? "",
    share: initial?.share ?? "",
    folder: initial?.folder ?? "Keystead",
    user: initial?.user ?? "",
    password: "",
    domain: initial?.domain ?? "",
  });
  const [test, setTest] = useState<SmbTest | null>(null);
  const [busy, setBusy] = useState<"test" | "folder" | "connect" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const set = (key: keyof SmbConfig) => (value: string) => {
    setConfig((c) => ({ ...c, [key]: value }));
    setTest(null);
    setError(null);
  };
  const complete = config.server.trim() !== "" && config.share.trim() !== "";

  const run = async (what: "test" | "folder") => {
    setBusy(what);
    setError(null);
    try {
      setTest(what === "test" ? await api.smbTest(config) : await api.smbCreateFolder(config));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const connect = async () => {
    setBusy("connect");
    try {
      if (await onConnect(config)) onClose();
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal
      title={initial ? t("smb.changeTitle") : t("smb.title")}
      subtitle={t("smb.subtitle")}
      icon={<HardDrive />}
      onClose={onClose}
      dismissable={busy === null}
      onSubmit={() => void (test?.ok ? connect() : run("test"))}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy !== null}>
            {t("common.cancel")}
          </Button>
          {test?.ok ? (
            <Button type="submit" variant="primary" icon={<PlugZap />} loading={busy === "connect"}>
              {t("smb.connect")}
            </Button>
          ) : (
            <Button type="submit" variant="primary" loading={busy === "test"} disabled={!complete}>
              {t("smb.test")}
            </Button>
          )}
        </>
      }
    >
      <Field label={t("smb.server")} htmlFor={ids.server} hint={t("smb.serverHint")}>
        <input
          id={ids.server}
          className="input mono"
          value={config.server}
          onChange={(e) => set("server")(e.target.value)}
          placeholder="192.168.1.10"
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          inputMode="url"
          autoFocus
        />
      </Field>
      <Field label={t("smb.share")} htmlFor={ids.share} hint={t("smb.shareHint")}>
        <input
          id={ids.share}
          className="input"
          value={config.share}
          onChange={(e) => set("share")(e.target.value)}
          placeholder={t("smb.sharePlaceholder")}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </Field>
      <Field label={t("smb.folder")} htmlFor={ids.folder} hint={t("smb.folderHint")}>
        <input
          id={ids.folder}
          className="input"
          value={config.folder}
          onChange={(e) => set("folder")(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </Field>
      <Field label={t("smb.user")} htmlFor={ids.user}>
        <input
          id={ids.user}
          className="input"
          value={config.user}
          onChange={(e) => set("user")(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </Field>
      <Field label={t("smb.password")} htmlFor={ids.password} hint={t("smb.passwordHint")}>
        <PasswordInput id={ids.password} value={config.password} onChange={set("password")} />
      </Field>
      <Field label={t("smb.domain")} htmlFor={ids.domain} hint={t("smb.domainHint")}>
        <input
          id={ids.domain}
          className="input"
          value={config.domain}
          onChange={(e) => set("domain")(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </Field>
      {test?.ok && (
        <div className="callout callout-success" role="status">
          <CircleCheck />
          <span>{test.writable ? t("smb.testOk") : t("smb.testReadOnly")}</span>
        </div>
      )}
      {test?.folderMissing && (
        <div className="callout callout-warning" role="status">
          <TriangleAlert />
          <span>{t("smb.folderMissing", { folder: config.folder.trim() || "/" })}</span>
          <Button size="sm" variant="secondary" icon={<FolderPlus />} loading={busy === "folder"} onClick={() => void run("folder")}>
            {t("smb.createFolder")}
          </Button>
        </div>
      )}
      {test?.code && (
        <div className="callout callout-danger" role="alert">
          <TriangleAlert />
          <span>{errorText(new ApiError(test.code))}</span>
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
