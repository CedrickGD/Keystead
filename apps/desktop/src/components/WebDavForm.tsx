// The WebDAV server form of the server sync (stage 2c, "Server sync
// (WebDAV)" in docs/ARCHITECTURE.md), on both platforms: address, user,
// app password and folder; "Verbindung testen" (`webdav_test`), confirming
// a certificate the system does not trust (its SHA-256 becomes the
// config's `certSha256`), taking a URL the server suggests, "Ordner
// anlegen" (`webdav_create_folder`) and finally "Verbinden" – the caller's
// `onConnect` (`link_webdav` on the phone, `set_sync_server` on the PC).
// The password lives in this dialog's state only and goes to the backend
// with each call; the page never stores it.

import { useId, useState } from "react";
import { CircleCheck, Folder, FolderOpen, FolderPlus, FolderUp, PlugZap, Server, ShieldAlert, TriangleAlert } from "lucide-react";
import { ApiError, api } from "../lib/api";
import type { CertInfo, SyncServer, WebDavConfig, WebDavTest } from "../lib/types";
import { useT } from "../i18n";
import { Button, Field, PasswordInput } from "./Controls";
import { Modal } from "./Modal";

/** The folder the form suggests (the backend's default as well). */
export const DEFAULT_SERVER_FOLDER = "Keystead";

type Busy = "test" | "create" | "browse" | "connect" | null;

/** `ab12…` → `AB:12:…`, the way servers and browsers show a fingerprint. */
function fingerprint(hex: string): string {
  return (hex.toUpperCase().match(/.{1,2}/g) ?? []).join(":");
}

export function WebDavDialog({
  initial,
  title,
  connectLabel,
  passwordNote,
  onConnect,
  onClose,
}: {
  /** The stored server (change it, or enter its password again); null for a new one. */
  initial: SyncServer | null;
  title: string;
  connectLabel: string;
  /** Where the password is kept on this device. */
  passwordNote: string;
  /** Links or sets the server; resolves to whether it worked (it shows its own errors). */
  onConnect: (config: WebDavConfig) => Promise<boolean>;
  onClose: () => void;
}) {
  const { t, errorText, formatDate } = useT();
  const ids = { url: useId(), user: useId(), password: useId(), folder: useId() };
  const [url, setUrl] = useState(initial?.url ?? "");
  const [user, setUser] = useState(initial?.user ?? "");
  const [password, setPassword] = useState("");
  const [folder, setFolder] = useState(initial?.folder ?? DEFAULT_SERVER_FOLDER);
  // A confirmed certificate belongs to the address it was confirmed for.
  const [cert, setCert] = useState<{ url: string; sha256: string } | null>(
    initial?.certSha256 ? { url: initial.url, sha256: initial.certSha256 } : null,
  );
  const [test, setTest] = useState<WebDavTest | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  const [showErrors, setShowErrors] = useState(false);
  const [browsedPath, setBrowsedPath] = useState<string | null>(null);
  const [browsedDirs, setBrowsedDirs] = useState<string[] | null>(null);

  const normalizedFolder = (): string => {
    const trimmed = folder.trim();
    if (trimmed === "/") return "/";
    return trimmed || DEFAULT_SERVER_FOLDER;
  };

  const config = (certSha256 = cert?.url.trim() === url.trim() ? cert.sha256 : null): WebDavConfig => ({
    url: url.trim(),
    user: user.trim(),
    password,
    folder: normalizedFolder(),
    certSha256,
  });
  const missing = {
    url: !url.trim(),
    user: !user.trim(),
    password: !password,
  };
  const complete = !missing.url && !missing.user && !missing.password;

  // Any change of the inputs makes the last test stale.
  const changed = <T,>(set: (value: T) => void) => (value: T) => {
    set(value);
    setTest(null);
    setError(null);
  };

  const browseFolder = async (targetFolder: string) => {
    setShowErrors(true);
    if (!complete) return;
    setBusy("browse");
    setError(null);
    const clean = targetFolder.replace(/^\/+|\/+$/g, "");
    try {
      const dirs = await api.webdavListFolders({
        ...config(),
        folder: clean,
      });
      setBrowsedPath(clean);
      setBrowsedDirs(dirs);
      setFolder(clean || "/");
      setTest(null);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const run = async (kind: Exclude<Busy, null>, call: () => Promise<WebDavTest>) => {
    setShowErrors(true);
    if (!complete) return;
    setBusy(kind);
    setError(null);
    try {
      setTest(await call());
    } catch (err) {
      setTest(null);
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const runTest = (certSha256?: string | null) => run("test", () => api.webdavTest(config(certSha256)));

  const trust = (info: CertInfo) => {
    setCert({ url: url.trim(), sha256: info.sha256 });
    void runTest(info.sha256);
  };

  const takeSuggested = (suggested: string) => {
    setUrl(suggested);
    setTest(null);
    void run("test", () => api.webdavTest({ ...config(), url: suggested }));
  };

  const connect = async () => {
    setBusy("connect");
    setError(null);
    try {
      if (await onConnect(config())) {
        setPassword("");
        onClose();
      }
    } finally {
      setBusy(null);
    }
  };

  const ready = test?.ok === true;
  const untrusted = test?.certificate && !ready ? test.certificate : null;

  return (
    <Modal
      title={title}
      subtitle={t("webdav.subtitle")}
      icon={<Server />}
      onClose={onClose}
      dismissable={busy === null}
      wide
      onSubmit={() => void (ready ? connect() : runTest())}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy !== null}>
            {t("common.cancel")}
          </Button>
          {ready ? (
            <Button type="submit" variant="primary" icon={<PlugZap />} loading={busy === "connect"}>
              {connectLabel}
            </Button>
          ) : (
            <Button type="submit" variant="primary" loading={busy === "test"} disabled={busy !== null && busy !== "test"}>
              {t("webdav.test")}
            </Button>
          )}
        </>
      }
    >
      <Field
        label={t("webdav.url")}
        htmlFor={ids.url}
        hint={t("webdav.urlHint")}
        error={showErrors && missing.url ? t("webdav.required") : null}
      >
        <input
          id={ids.url}
          className="input"
          type="url"
          inputMode="url"
          value={url}
          onChange={(e) => changed(setUrl)(e.target.value)}
          placeholder="https://cloud.example.de"
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </Field>
      <Field label={t("webdav.user")} htmlFor={ids.user} error={showErrors && missing.user ? t("webdav.required") : null}>
        <input
          id={ids.user}
          className="input"
          value={user}
          onChange={(e) => changed(setUser)(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </Field>
      <Field
        label={t("webdav.password")}
        htmlFor={ids.password}
        hint={`${t("webdav.passwordHint")} ${passwordNote}`}
        error={showErrors && missing.password ? t("webdav.required") : null}
      >
        <PasswordInput id={ids.password} value={password} onChange={changed(setPassword)} mono={false} />
      </Field>
      <Field label={t("webdav.folder")} htmlFor={ids.folder} hint={t("webdav.folderHint")}>
        <div className="rename-form">
          <input
            id={ids.folder}
            className="input"
            value={folder}
            onChange={(e) => changed(setFolder)(e.target.value)}
            placeholder={DEFAULT_SERVER_FOLDER}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
          <Button
            variant="secondary"
            icon={<FolderOpen />}
            loading={busy === "browse"}
            disabled={!complete || (busy !== null && busy !== "browse")}
            onClick={() => void browseFolder(browsedPath ?? "")}
          >
            {t("webdav.browse")}
          </Button>
        </div>
      </Field>

      {browsedDirs !== null && (
        <div
          style={{
            maxHeight: 180,
            overflowY: "auto",
            border: "1px solid var(--border-subtle, rgba(128,128,128,0.25))",
            borderRadius: 8,
            padding: 6,
            display: "flex",
            flexDirection: "column",
            gap: 4,
          }}
        >
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "2px 6px" }}>
            <code className="mono">{browsedPath ? `/${browsedPath}` : t("webdav.browseRoot")}</code>
            {browsedPath && (
              <Button
                size="sm"
                variant="ghost"
                icon={<FolderUp />}
                disabled={busy !== null}
                onClick={() => {
                  const idx = browsedPath.lastIndexOf("/");
                  void browseFolder(idx < 0 ? "" : browsedPath.slice(0, idx));
                }}
              >
                {t("webdav.browseUp")}
              </Button>
            )}
          </div>
          {browsedDirs.map((dir) => {
            const sub = browsedPath ? `${browsedPath}/${dir}` : dir;
            return (
              <Button
                key={sub}
                size="sm"
                variant="ghost"
                icon={<Folder />}
                disabled={busy !== null}
                onClick={() => void browseFolder(sub)}
              >
                {dir}
              </Button>
            );
          })}
          {browsedDirs.length === 0 && (
            <span className="subtle" style={{ padding: "4px 6px" }}>
              {t("webdav.browseEmpty")}
            </span>
          )}
        </div>
      )}

      {error && (
        <div className="callout callout-danger" role="alert">
          <TriangleAlert />
          <span>{error}</span>
        </div>
      )}
      {ready && (
        <div className={`callout ${test.writable ? "callout-success" : "callout-warning"}`} role="status">
          <CircleCheck />
          <span>{test.writable ? t("webdav.ok") : t("webdav.okReadOnly")}</span>
        </div>
      )}
      {untrusted && (
        <div className="callout callout-warning webdav-cert" role="alert">
          <ShieldAlert />
          <div className="webdav-cert-body">
            <strong>{t("webdav.certTitle")}</strong>
            <span>{t("webdav.certText")}</span>
            <dl className="webdav-cert-facts">
              <dt>{t("webdav.certSubject")}</dt>
              <dd>{untrusted.subject || "–"}</dd>
              <dt>{t("webdav.certIssuer")}</dt>
              <dd>{untrusted.issuer || "–"}</dd>
              {untrusted.notBefore !== null && untrusted.notAfter !== null && (
                <>
                  <dt>{t("webdav.certValid")}</dt>
                  <dd>
                    {t("webdav.certValidRange", { from: formatDate(untrusted.notBefore), to: formatDate(untrusted.notAfter) })}
                  </dd>
                </>
              )}
              <dt>{t("webdav.certFingerprint")}</dt>
              <dd className="mono selectable webdav-fingerprint">{fingerprint(untrusted.sha256)}</dd>
            </dl>
            <div>
              <Button size="sm" variant="secondary" loading={busy === "test"} onClick={() => trust(untrusted)}>
                {t("webdav.certTrust")}
              </Button>
            </div>
          </div>
        </div>
      )}
      {test && !ready && !untrusted && test.folderMissing && (
        <div className="callout callout-info" role="status">
          <FolderPlus />
          <div className="webdav-cert-body">
            <span>{t("webdav.folderMissing", { folder: config().folder })}</span>
            <div>
              <Button
                size="sm"
                variant="secondary"
                loading={busy === "create"}
                disabled={busy !== null && busy !== "create"}
                onClick={() => void run("create", () => api.webdavCreateFolder(config()))}
              >
                {t("webdav.createFolder")}
              </Button>
            </div>
          </div>
        </div>
      )}
      {test && !ready && !untrusted && !test.folderMissing && test.code && (
        <div className="callout callout-danger" role="alert">
          <TriangleAlert />
          <span>{errorText(new ApiError(test.code))}</span>
        </div>
      )}
      {test && !ready && test.suggestedUrl && test.suggestedUrl !== url.trim() && (
        <div className="callout callout-info" role="status">
          <Server />
          <div className="webdav-cert-body">
            <span>{t("webdav.suggested", { url: test.suggestedUrl })}</span>
            <div>
              <Button size="sm" variant="secondary" onClick={() => takeSuggested(test.suggestedUrl ?? "")}>
                {t("webdav.useSuggested")}
              </Button>
            </div>
          </div>
        </div>
      )}
    </Modal>
  );
}
