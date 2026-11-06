// The phone's SMB form ("Mit SMB-Freigabe verbinden"): a folder on a Windows
// share or a NAS – one address line as a file app or Windows shows it
// (`192.168.1.10/Daten/Keystead`, `\\nas\Daten\Keystead`, `smb://…`; read by
// lib/smbAddress.mjs), user and password; a domain only under "Erweitert".
// "Verbindung testen" checks share, login, folder and write access
// (`smb_test`; nothing is stored), "Ordner anlegen" makes a missing folder
// (`smb_create_folder`), "Verbinden" links it (`link_smb`, the password is
// sealed on the device). The password lives in this dialog's state only. See
// "SMB shares (phone)" in docs/ARCHITECTURE.md.

import { useId, useMemo, useState } from "react";
import { ChevronRight, CircleCheck, FolderPlus, HardDrive, PlugZap, TriangleAlert } from "lucide-react";
import { ApiError, api } from "../lib/api";
import { formatSmbAddress, isDriveAddress, parseSmbAddress } from "../lib/smbAddress.mjs";
import type { SmbConfig, SmbTest, SyncSmb } from "../lib/types";
import { useT } from "../i18n";
import { Button, Field, PasswordInput } from "./Controls";
import { Modal } from "./Modal";

const PLAIN_INPUT = { autoComplete: "off", autoCapitalize: "off", autoCorrect: "off", spellCheck: false } as const;

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
  const ids = { address: useId(), user: useId(), password: useId(), domain: useId() };
  const [address, setAddress] = useState(initial ? formatSmbAddress(initial) : "");
  const [user, setUser] = useState(initial?.user ?? "");
  const [password, setPassword] = useState("");
  const [domain, setDomain] = useState(initial?.domain ?? "");
  const [advanced, setAdvanced] = useState(Boolean(initial?.domain));
  const [test, setTest] = useState<SmbTest | null>(null);
  const [busy, setBusy] = useState<"test" | "folder" | "connect" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const parsed = useMemo(() => parseSmbAddress(address), [address]);
  const config: SmbConfig | null = parsed && {
    server: parsed.server,
    share: parsed.share,
    folder: parsed.folder,
    user,
    password,
    domain,
  };

  const changed = <T,>(apply: (value: T) => void) => (value: T) => {
    apply(value);
    setTest(null);
    setError(null);
  };
  const changeAddress = changed((value: string) => {
    setAddress(value);
    // `smb://ich@nas/…`: the user goes to its own field (if that is empty).
    const named = parseSmbAddress(value)?.user;
    if (named && user.trim() === "") setUser(named);
  });

  const run = async (what: "test" | "folder") => {
    if (!config) return;
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
    if (!config) return;
    setBusy("connect");
    try {
      if (await onConnect(config)) onClose();
    } finally {
      setBusy(null);
    }
  };

  // Under the address: what Keystead read from it, so a typo shows at once;
  // a PC drive (`Z:\…`) gets told what to write instead.
  const addressHint = parsed
    ? parsed.folder
      ? t("smb.addressParsed", { server: parsed.server, share: parsed.share, folder: parsed.folder })
      : t("smb.addressParsedRoot", { server: parsed.server, share: parsed.share })
    : isDriveAddress(address)
      ? t("smb.addressDrive", { drive: address.trim().slice(0, 2).toUpperCase() })
      : address.trim() !== ""
        ? t("smb.addressIncomplete")
        : t("smb.addressHint");

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
            <Button type="submit" variant="primary" loading={busy === "test"} disabled={!config}>
              {t("smb.test")}
            </Button>
          )}
        </>
      }
    >
      <Field label={t("smb.address")} htmlFor={ids.address} hint={addressHint}>
        <input
          id={ids.address}
          className="input mono"
          value={address}
          onChange={(e) => changeAddress(e.target.value)}
          placeholder={t("smb.addressPlaceholder")}
          inputMode="url"
          autoFocus
          {...PLAIN_INPUT}
        />
      </Field>
      <Field label={t("smb.user")} htmlFor={ids.user} hint={t("smb.userHint")}>
        <input
          id={ids.user}
          className="input"
          value={user}
          onChange={(e) => changed(setUser)(e.target.value)}
          {...PLAIN_INPUT}
        />
      </Field>
      <Field label={t("smb.password")} htmlFor={ids.password} hint={t("smb.passwordHint")}>
        <PasswordInput id={ids.password} value={password} onChange={changed(setPassword)} />
      </Field>
      <button type="button" className="collapse-toggle" aria-expanded={advanced} onClick={() => setAdvanced((open) => !open)}>
        <ChevronRight className="collapse-chevron" />
        <span>{t("smb.advanced")}</span>
      </button>
      {advanced && (
        <Field label={t("smb.domain")} htmlFor={ids.domain} hint={t("smb.domainHint")}>
          <input
            id={ids.domain}
            className="input"
            value={domain}
            onChange={(e) => changed(setDomain)(e.target.value)}
            {...PLAIN_INPUT}
          />
        </Field>
      )}
      {test?.ok && (
        <div className="callout callout-success" role="status">
          <CircleCheck />
          <span>{test.writable ? t("smb.testOk") : t("smb.testReadOnly")}</span>
        </div>
      )}
      {test?.folderMissing && (
        <div className="callout callout-warning" role="status">
          <TriangleAlert />
          <span>{t("smb.folderMissing", { folder: parsed?.folder || "/" })}</span>
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
