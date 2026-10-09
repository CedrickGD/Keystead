import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { CalendarClock, ChevronRight, CircleCheck, RefreshCw, Repeat, ShieldAlert, Smartphone } from "lucide-react";
import { api } from "../lib/api";
import type { HealthReport, VaultItem } from "../lib/types";
import { itemSubtitle } from "../lib/utils";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";
import { Avatar } from "../components/Avatar";
import { iconHost } from "../lib/icons";
import { Button } from "../components/Controls";

function ScoreGauge({ score }: { score: number }) {
  const { t } = useT();
  const radius = 52;
  const circumference = 2 * Math.PI * radius;
  const clamped = Math.max(0, Math.min(100, score));
  const level = clamped >= 80 ? "good" : clamped >= 50 ? "fair" : "poor";
  return (
    <div className={`gauge ${level}`} role="img" aria-label={t("health.scoreAria", { score: clamped })}>
      <svg viewBox="0 0 120 120">
        <circle className="gauge-track" cx="60" cy="60" r={radius} />
        <circle
          className="gauge-value"
          cx="60"
          cy="60"
          r={radius}
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - clamped / 100)}
        />
      </svg>
      <div className="gauge-label">
        <span className="gauge-score">{clamped}</span>
        <span className="gauge-max">/ 100</span>
      </div>
    </div>
  );
}

function ItemRows({
  ids,
  byId,
  onOpen,
  limit,
}: {
  ids: string[];
  byId: Map<string, VaultItem>;
  onOpen: (id: string) => void;
  limit?: number;
}) {
  const shown = limit ? ids.slice(0, limit) : ids;
  return (
    <ul className="health-items">
      {shown.map((id) => {
        const item = byId.get(id);
        if (!item) return null;
        return (
          <li key={id}>
            <button type="button" className="health-item" onClick={() => onOpen(id)}>
              <Avatar name={item.name} type={item.type} size="sm" site={iconHost(item)} />
              <span className="health-item-text">
                <span className="health-item-name truncate">{item.name}</span>
                <span className="health-item-sub truncate">{itemSubtitle(item)}</span>
              </span>
              <ChevronRight className="health-item-chevron" />
            </button>
          </li>
        );
      })}
    </ul>
  );
}

function HealthCard({
  icon,
  tone,
  title,
  count,
  description,
  okText,
  children,
  total,
  limit = 4,
}: {
  icon: ReactNode;
  tone: "danger" | "warning" | "info";
  title: string;
  count: number;
  description: string;
  okText: string;
  children: (expanded: boolean) => ReactNode;
  /** Number of entries; a "show all" toggle appears when it exceeds `limit`. */
  total: number;
  limit?: number;
}) {
  const { t } = useT();
  const [expanded, setExpanded] = useState(false);
  const ok = count === 0;
  return (
    <section className={`card health-card ${ok ? "ok" : tone}`}>
      <div className="health-card-head">
        <span className="health-icon">{ok ? <CircleCheck /> : icon}</span>
        <div className="health-card-title">
          <h2>{title}</h2>
          <p>{ok ? okText : description}</p>
        </div>
        <span className="health-count">{count}</span>
      </div>
      {!ok && (
        <>
          {children(expanded)}
          {total > limit && (
            <button type="button" className="health-more" onClick={() => setExpanded((e) => !e)}>
              {expanded ? t("health.showLess") : t("health.showAll", { count: total })}
            </button>
          )}
        </>
      )}
    </section>
  );
}

export function HealthPage({ items, onOpenItem }: { items: VaultItem[]; onOpenItem: (id: string) => void }) {
  const { t, tp, errorText } = useT();
  const toast = useToast();
  const [report, setReport] = useState<HealthReport | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setReport(await api.healthReport());
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setLoading(false);
    }
  }, [errorText, toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const byId = useMemo(() => new Map(items.map((i) => [i.id, i])), [items]);
  // The report only carries a count; list the logins without a TOTP seed ourselves.
  const without2fa = useMemo(
    () =>
      items
        .filter((i) => i.type === "login" && i.deletedAt === null && i.login?.password && !i.login.totp.trim())
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((i) => i.id),
    [items],
  );

  if (!report) {
    return (
      <main className="page">
        <div className="page-inner wide">
          <header className="page-header">
            <h1>{t("health.title")}</h1>
            <p>{t("health.subtitle")}</p>
          </header>
          <div className="health-loading">
            <span className="spinner" /> {t("health.loading")}
          </div>
        </div>
      </main>
    );
  }

  const reusedCount = report.reused.reduce((sum, group) => sum + group.length, 0);
  const affected = new Set([...report.weak, ...report.reused.flat(), ...report.old]).size;
  const verdict = report.score >= 80 ? t("health.verdictGood") : report.score >= 50 ? t("health.verdictFair") : t("health.verdictPoor");

  return (
    <main className="page" aria-labelledby="health-title">
      <div className="page-inner wide">
        <header className="page-header with-action">
          <div>
            <h1 id="health-title">{t("health.title")}</h1>
            <p>{t("health.subtitle")}</p>
          </div>
          <Button variant="secondary" size="sm" icon={<RefreshCw />} onClick={() => void load()} loading={loading}>
            {t("health.refresh")}
          </Button>
        </header>

        <section className="card health-summary">
          <ScoreGauge score={report.score} />
          <div className="health-summary-text">
            <div className="health-verdict">{verdict}</div>
            <p>
              {report.totalLogins === 0
                ? t("health.noLogins")
                : affected === 0
                  ? t("health.allGood", { total: report.totalLogins })
                  : t("health.summary", { affected, total: report.totalLogins })}
            </p>
            <div className="health-summary-chips">
              <span className={`chip ${report.weak.length ? "danger" : "success"}`}>{tp("health.chipWeak", report.weak.length)}</span>
              <span className={`chip ${reusedCount ? "warning" : "success"}`}>{tp("health.chipReused", reusedCount)}</span>
              <span className={`chip ${report.old.length ? "warning" : "success"}`}>{tp("health.chipOld", report.old.length)}</span>
            </div>
          </div>
        </section>

        <div className="health-grid">
          <HealthCard
            icon={<ShieldAlert />}
            tone="danger"
            title={t("health.weakTitle")}
            count={report.weak.length}
            total={report.weak.length}
            description={t("health.weakDesc")}
            okText={t("health.weakOk")}
          >
            {(expanded) => <ItemRows ids={report.weak} byId={byId} onOpen={onOpenItem} limit={expanded ? undefined : 4} />}
          </HealthCard>

          <HealthCard
            icon={<Repeat />}
            tone="warning"
            title={t("health.reusedTitle")}
            count={reusedCount}
            total={report.reused.length}
            limit={2}
            description={t("health.reusedDesc")}
            okText={t("health.reusedOk")}
          >
            {(expanded) => (
              <div className="health-groups">
                {(expanded ? report.reused : report.reused.slice(0, 2)).map((group, idx) => (
                  <div key={idx} className="health-group">
                    <div className="health-group-label">{t("health.sameGroup", { count: group.length })}</div>
                    <ItemRows ids={group} byId={byId} onOpen={onOpenItem} />
                  </div>
                ))}
              </div>
            )}
          </HealthCard>

          <HealthCard
            icon={<CalendarClock />}
            tone="warning"
            title={t("health.oldTitle")}
            count={report.old.length}
            total={report.old.length}
            description={t("health.oldDesc")}
            okText={t("health.oldOk")}
          >
            {(expanded) => <ItemRows ids={report.old} byId={byId} onOpen={onOpenItem} limit={expanded ? undefined : 4} />}
          </HealthCard>

          <HealthCard
            icon={<Smartphone />}
            tone="info"
            title={t("health.totpTitle")}
            count={without2fa.length}
            total={without2fa.length}
            description={t("health.totpDesc")}
            okText={t("health.totpOk")}
          >
            {(expanded) => <ItemRows ids={without2fa} byId={byId} onOpen={onOpenItem} limit={expanded ? undefined : 4} />}
          </HealthCard>
        </div>
      </div>
    </main>
  );
}
