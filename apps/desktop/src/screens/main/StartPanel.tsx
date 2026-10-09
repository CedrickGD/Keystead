import { useEffect, useState, type ReactNode } from "react";
import { ChevronRight, CircleCheck, FileUp, KeyRound, LifeBuoy, MousePointerClick, Plug, WandSparkles } from "lucide-react";
import { api } from "../../lib/api";
import { useT, type MessageKey } from "../../i18n";
import { useApp } from "../../state/app";
import { Logo } from "../../components/Logo";
import { useOpenImport } from "../../components/import/ImportDialog";

function ActionTile({
  icon,
  title,
  desc,
  kbd,
  primary,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  desc: string;
  kbd?: string;
  primary?: boolean;
  onClick: () => void;
}) {
  return (
    <button type="button" className={`start-tile ${primary ? "primary" : ""}`} onClick={onClick}>
      <span className="start-tile-icon">{icon}</span>
      <span className="start-tile-text">
        <span className="start-tile-title">
          {title}
          {kbd && <span className="kbd">{kbd}</span>}
        </span>
        <span className="start-tile-desc">{desc}</span>
      </span>
    </button>
  );
}

function SetupRow({
  icon,
  title,
  desc,
  done,
  doneLabel,
  actionLabel,
  onAction,
}: {
  icon: ReactNode;
  title: string;
  desc: string;
  done: boolean;
  doneLabel: string;
  actionLabel: string;
  onAction: () => void;
}) {
  return (
    <li className={`setup-row ${done ? "done" : ""}`}>
      <span className="setup-icon">{done ? <CircleCheck /> : icon}</span>
      <span className="setup-text">
        <span className="setup-title">{title}</span>
        <span className="setup-desc">{done ? doneLabel : desc}</span>
      </span>
      {!done && (
        <button type="button" className="btn btn-secondary btn-sm" onClick={onAction}>
          {actionLabel}
          <ChevronRight />
        </button>
      )}
    </li>
  );
}

/**
 * Shown in the detail pane when nothing is selected: the most common tasks one
 * click away, the remaining setup steps, and the keyboard shortcuts.
 */
export function StartPanel({
  empty,
  onNewLogin,
  onGenerator,
  onSettings,
}: {
  /** The vault has no items yet. */
  empty: boolean;
  onNewLogin: () => void;
  onGenerator: () => void;
  onSettings: (section: string) => void;
}) {
  const { t } = useT();
  const { vault, settings } = useApp();
  const openImport = useOpenImport();
  const [browserPaired, setBrowserPaired] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .browserStatus()
      .then((status) => {
        if (!cancelled) setBrowserPaired(status.clients.length > 0);
      })
      .catch(() => {
        if (!cancelled) setBrowserPaired(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const shortcuts: [string, MessageKey][] = [
    ["Ctrl F", "shortcuts.search"],
    ["Ctrl N", "shortcuts.new"],
    ["Ctrl ⇧ C", "shortcuts.copyPassword"],
    ["Ctrl B", "shortcuts.copyUsername"],
    ["Ctrl L", "shortcuts.lock"],
  ];

  const browserDone = settings.browserIntegration && browserPaired === true;
  const recoveryDone = vault?.hasRecoveryKey ?? false;
  const setupOpen = !browserDone || !recoveryDone;

  return (
    <div className="start-panel">
      <div className="start-head">
        <div className={`start-head-icon ${empty ? "brand" : ""}`} aria-hidden>
          {empty ? <Logo variant="glyph" size={30} /> : <MousePointerClick />}
        </div>
        <h2>{empty ? t("start.emptyTitle") : t("detail.noneTitle")}</h2>
        <p>{empty ? t("start.emptyHint") : t("detail.noneHint")}</p>
      </div>

      <div className="start-tiles">
        <ActionTile
          primary={empty}
          icon={<KeyRound />}
          title={t("start.newLogin")}
          desc={t("start.newLoginDesc")}
          kbd="Ctrl N"
          onClick={onNewLogin}
        />
        <ActionTile
          icon={<FileUp />}
          title={t("start.import")}
          desc={t("start.importDesc")}
          onClick={() => (openImport ? openImport() : onSettings("data"))}
        />
        <ActionTile icon={<WandSparkles />} title={t("start.generate")} desc={t("start.generateDesc")} onClick={onGenerator} />
      </div>

      {setupOpen && browserPaired !== null && (
        <section className="start-setup" aria-labelledby="start-setup-title">
          <h3 id="start-setup-title">{t("start.setupTitle")}</h3>
          <ul>
            <SetupRow
              icon={<Plug />}
              title={t("start.browserTitle")}
              desc={t("start.browserDesc")}
              done={browserDone}
              doneLabel={t("start.browserDone")}
              actionLabel={t("start.browserAction")}
              onAction={() => onSettings("browser")}
            />
            <SetupRow
              icon={<LifeBuoy />}
              title={t("start.recoveryTitle")}
              desc={t("start.recoveryDesc")}
              done={recoveryDone}
              doneLabel={t("start.recoveryDone")}
              actionLabel={t("start.recoveryAction")}
              onAction={() => onSettings("security")}
            />
          </ul>
        </section>
      )}

      {!empty && (
        <section className="shortcuts" aria-labelledby="start-shortcuts-title">
          <h3 id="start-shortcuts-title">{t("shortcuts.title")}</h3>
          <dl className="shortcut-list">
            {shortcuts.map(([keys, label]) => (
              <div key={label}>
                <dt>
                  <span className="kbd">{keys}</span>
                </dt>
                <dd>{t(label)}</dd>
              </div>
            ))}
          </dl>
        </section>
      )}
    </div>
  );
}
