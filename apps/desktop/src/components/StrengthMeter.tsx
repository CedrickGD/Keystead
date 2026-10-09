import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { Strength } from "../lib/types";
import { useT, type MessageKey } from "../i18n";

/** Debounced strength estimate from the backend (`password_strength`). */
export function useStrength(password: string, delay = 180): Strength | null {
  const [strength, setStrength] = useState<Strength | null>(null);
  useEffect(() => {
    if (!password) {
      setStrength(null);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      api
        .passwordStrength(password)
        .then((result) => {
          if (!cancelled) setStrength(result);
        })
        .catch(() => {
          if (!cancelled) setStrength(null);
        });
    }, delay);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [password, delay]);
  return password ? strength : null;
}

const LABELS: MessageKey[] = ["strength.0", "strength.1", "strength.2", "strength.3", "strength.4"];

export function StrengthMeter({
  password,
  strength: given,
  emptyHint,
}: {
  password: string;
  strength?: Strength | null;
  /** Shown instead of the (empty) meter while nothing is typed. */
  emptyHint?: string;
}) {
  const { t } = useT();
  const fetched = useStrength(given === undefined ? password : "");
  const strength = given === undefined ? fetched : given;
  const score = password && strength ? Math.max(0, Math.min(4, strength.score)) : -1;
  const label = score >= 0 ? t(LABELS[score] ?? "strength.0") : "";
  // One bar for "very weak", all four for "very strong".
  const filled = score < 0 ? 0 : Math.max(1, score);
  if (!password && emptyHint) {
    return (
      <div className="strength">
        <span className="field-hint">{emptyHint}</span>
      </div>
    );
  }
  return (
    <div className="strength" data-score={score >= 0 ? score : undefined}>
      <div className="strength-bars" aria-hidden style={score < 0 ? { visibility: "hidden" } : undefined}>
        {[1, 2, 3, 4].map((i) => (
          <div key={i} className={`strength-bar ${i <= filled ? "on" : ""}`} />
        ))}
      </div>
      <span className="strength-label" aria-live="polite">
        {label || " "}
      </span>
    </div>
  );
}
