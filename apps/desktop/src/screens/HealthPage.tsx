import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  CalendarClock,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  RefreshCw,
  Repeat,
  ShieldAlert,
  ShieldCheck,
  ShieldQuestion,
  Smartphone,
} from "lucide-react";
import { api } from "../lib/api";
import type { HealthReport, ItemListEntry } from "../lib/types";
import { itemSubtitle, localPrefs } from "../lib/utils";
import { useT } from "../i18n";
import { useToast } from "../components/Toasts";
import { Avatar } from "../components/Avatar";
import { iconHost } from "../lib/icons";
import { Button, Checkbox } from "../components/Controls";
import { Modal } from "../components/Modal";
import { useBreaches, usePwnedErrorText } from "../state/breaches";

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
  sub,
}: {
  ids: string[];
  byId: Map<string, ItemListEntry>;
  onOpen: (id: string) => void;
  limit?: number;
  /** Replaces the usual subtitle (username / site). */
  sub?: (item: ItemListEntry) => string;
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
                <span className="health-item-sub truncate">{sub ? sub(item) : itemSubtitle(item)}</span>
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

// "Nicht mehr fragen" of the confirmation below: localStorage only
// (`keystead.pwnedConfirmed`), no backend setting.
const PWNED_CONFIRMED_KEY = "pwnedConfirmed";

/** A leaked item of the last check, as the card lists it. */
interface LeakedRow {
  item: ItemListEntry;
  count: number;
}

/**
 * Before a vault-wide check: what leaves the device. "Nicht mehr fragen"
 * skips this dialog from then on (remembered in localStorage only).
 */
function PwnedConfirmDialog({ onClose, onConfirm }: { onClose: () => void; onConfirm: (dontAsk: boolean) => void }) {
  const { t } = useT();
  const [dontAsk, setDontAsk] = useState(false);
  return (
    <Modal
      title={t("pwned.confirmTitle")}
      icon={<ShieldQuestion />}
      onClose={onClose}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => onConfirm(dontAsk)} data-autofocus>
            {t("pwned.confirmRun")}
          </Button>
        </>
      }
    >
      <p>{t("pwned.confirmText")}</p>
      <p>{t("pwned.confirmText2")}</p>
      <Checkbox checked={dontAsk} onChange={setDontAsk}>
        {t("pwned.dontAsk")}
      </Checkbox>
    </Modal>
  );
}

/**
 * "Geleakte Passwörter": the one card that needs a button press – the check
 * sends the SHA-1 prefixes of the vault's passwords to Have I Been Pwned
 * (`check_pwned_vault`, see "Leaked passwords" in docs/ARCHITECTURE.md), so
 * it never runs on its own. The result lives in the breaches context (page
 * memory); `rows` are the leaked items of the last check that are unchanged
 * since, in the report's order (count desc).
 */
function PwnedCard({
  items,
  rows,
  byId,
  onOpen,
  limit = 4,
}: {
  items: ItemListEntry[];
  rows: LeakedRow[];
  byId: Map<string, ItemListEntry>;
  onOpen: (id: string) => void;
  limit?: number;
}) {
  const { t, tp, locale, formatDateTime } = useT();
  const { report, checking, error, check } = useBreaches();
  const pwnedErrorText = usePwnedErrorText();
  const [confirming, setConfirming] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const numberFmt = useMemo(() => new Intl.NumberFormat(locale), [locale]);

  const start = () => {
    if (localPrefs.get(PWNED_CONFIRMED_KEY) === "1") void check(items);
    else setConfirming(true);
  };
  const confirmed = (dontAsk: boolean) => {
    setConfirming(false);
    if (dontAsk) localPrefs.set(PWNED_CONFIRMED_KEY, "1");
    void check(items);
  };
  const countOf = new Map(rows.map((row) => [row.item.id, row.count]));
  const foundText = (item: ItemListEntry) => {
    const count = countOf.get(item.id) ?? 0;
    return tp("pwned.rowFound", count, { n: numberFmt.format(count) });
  };

  const leaked = rows.length;
  // `error` is cleared when a check starts, so it only shows between runs.
  const failed = error !== null;
  const known = report !== null && !failed;
  const tone = leaked || failed ? "danger" : known ? "ok" : "pending";
  const icon = failed ? <CircleAlert /> : leaked ? <ShieldAlert /> : known ? <CircleCheck /> : <ShieldQuestion />;
  const description = checking
    ? t("pwned.checking")
    : failed
      ? pwnedErrorText(error)
      : !report
        ? t("pwned.desc")
        : report.checked === 0
          ? t("pwned.nothingToCheck")
          : leaked
            ? t("pwned.leakedDesc")
            : tp("pwned.ok", report.checked);

  return (
    <section className={`card health-card ${tone}`}>
      <div className="health-card-head">
        <span className="health-icon">{icon}</span>
        <div className="health-card-title">
          <h2>{t("pwned.title")}</h2>
          <p role={failed ? "alert" : undefined}>{description}</p>
        </div>
        {checking ? <span className="spinner" aria-hidden /> : known && <span className="health-count">{leaked}</span>}
      </div>
      {leaked > 0 && (
        <>
          <ItemRows ids={rows.map((row) => row.item.id)} byId={byId} onOpen={onOpen} limit={expanded ? undefined : limit} sub={foundText} />
          {leaked > limit && (
            <button type="button" className="health-more" onClick={() => setExpanded((e) => !e)}>
              {expanded ? t("health.showLess") : t("health.showAll", { count: leaked })}
            </button>
          )}
        </>
      )}
      <div className="health-card-foot">
        {report !== null && report.failed > 0 && !checking && <p className="health-note">{tp("pwned.failed", report.failed)}</p>}
        <div className="health-card-actions">
          <Button variant={report ? "secondary" : "primary"} size="sm" icon={<ShieldCheck />} loading={checking} onClick={start}>
            {report || failed ? t("pwned.recheck") : t("pwned.check")}
          </Button>
          {report && !checking && (
            <span className="health-checked-at">{t("pwned.checkedAt", { when: formatDateTime(report.checkedAt) })}</span>
          )}
        </div>
      </div>
      {confirming && <PwnedConfirmDialog onClose={() => setConfirming(false)} onConfirm={confirmed} />}
    </section>
  );
}

/** The report comes from the backend (`health_report`); `items` (no secrets) only name the affected items. */
export function HealthPage({ items, onOpenItem }: { items: ItemListEntry[]; onOpenItem: (id: string) => void }) {
  const { t, tp, errorText } = useT();
  const toast = useToast();
  const breaches = useBreaches();
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
  // The leaked items of the last Have I Been Pwned check that still exist
  // outside the trash and are unchanged since (see `state/breaches.tsx`).
  const leakedRows = useMemo(() => {
    const rows: LeakedRow[] = [];
    for (const { itemId } of breaches.report?.leaked ?? []) {
      const item = byId.get(itemId);
      const count = item ? breaches.breachCount(item) : null;
      if (item && item.deletedAt === null && count !== null) rows.push({ item, count });
    }
    return rows;
  }, [breaches.report, breaches.breachCount, byId]);
  // The report only carries a count; list the logins without a TOTP seed ourselves.
  const without2fa = useMemo(
    () =>
      items
        .filter((i) => i.type === "login" && i.deletedAt === null && i.login?.hasPassword && !i.login.hasTotp)
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
              {breaches.report && (
                <span className={`chip ${leakedRows.length ? "danger" : "success"}`}>{tp("pwned.chipLeaked", leakedRows.length)}</span>
              )}
              <span className={`chip ${report.weak.length ? "danger" : "success"}`}>{tp("health.chipWeak", report.weak.length)}</span>
              <span className={`chip ${reusedCount ? "warning" : "success"}`}>{tp("health.chipReused", reusedCount)}</span>
              <span className={`chip ${report.old.length ? "warning" : "success"}`}>{tp("health.chipOld", report.old.length)}</span>
            </div>
          </div>
        </section>

        <div className="health-grid">
          <PwnedCard items={items} rows={leakedRows} byId={byId} onOpen={onOpenItem} />

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
