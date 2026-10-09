import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import type { TotpCode } from "../lib/types";
import { formatTotp } from "../lib/utils";
import { useT } from "../i18n";

export interface LiveTotp {
  code: TotpCode | null;
  remaining: number;
  error: string | null;
}

/** Live TOTP code for a seed; fetches a new code when the period rolls over. */
export function useTotp(seed: string): LiveTotp {
  const { errorText } = useT();
  const [code, setCode] = useState<TotpCode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const expiresAt = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;

    const load = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const next = await api.totpCode(seed);
        if (cancelled) return;
        expiresAt.current = Date.now() + next.remaining * 1000;
        setCode(next);
        setError(null);
      } catch (err) {
        if (cancelled) return;
        expiresAt.current = 0;
        setCode(null);
        setError(errorText(err));
      } finally {
        inFlight = false;
      }
    };

    setCode(null);
    setError(null);
    expiresAt.current = 0;
    void load();
    const timer = window.setInterval(() => {
      const current = Date.now();
      setNow(current);
      if (expiresAt.current > 0 && current >= expiresAt.current) void load();
    }, 1000);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [seed, errorText]);

  const remaining = code ? Math.max(0, Math.ceil((expiresAt.current - now) / 1000)) : 0;
  return { code, remaining, error };
}

export function TotpRing({ remaining, period }: { remaining: number; period: number }) {
  const radius = 12;
  const circumference = 2 * Math.PI * radius;
  const fraction = period > 0 ? Math.min(1, remaining / period) : 0;
  return (
    <div className={`totp-ring ${remaining <= 5 ? "expiring" : ""}`} aria-hidden>
      <svg viewBox="0 0 30 30">
        <circle className="track" cx="15" cy="15" r={radius} fill="none" strokeWidth="3" />
        <circle
          className="progress"
          cx="15"
          cy="15"
          r={radius}
          fill="none"
          strokeWidth="3"
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - fraction)}
        />
      </svg>
      <span>{remaining}</span>
    </div>
  );
}

export function TotpValue({ totp }: { totp: LiveTotp }) {
  const { t } = useT();
  if (totp.error) return <span className="field-error">{t("item.totpInvalid")}</span>;
  if (!totp.code) return <span className="totp-code subtle">––– –––</span>;
  return (
    <div className="totp">
      <span className="totp-code selectable" aria-label={t("item.totpCodeAria", { seconds: totp.remaining })}>
        {formatTotp(totp.code.code)}
      </span>
      <TotpRing remaining={totp.remaining} period={totp.code.period} />
    </div>
  );
}
