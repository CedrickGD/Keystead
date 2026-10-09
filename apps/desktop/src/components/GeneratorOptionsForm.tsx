import { useEffect, useId, useRef, useState } from "react";
import { DEFAULT_GENERATOR_OPTIONS, type GeneratorOptions } from "../lib/types";
import { clamp, localPrefs } from "../lib/utils";
import { useT } from "../i18n";
import { Checkbox, Segmented } from "./Controls";

const STORAGE_KEY = "generator.options";

/** Generator options remembered per device (falls back to the defaults). */
export function loadGeneratorOptions(): GeneratorOptions {
  const raw = localPrefs.get(STORAGE_KEY);
  if (!raw) return { ...DEFAULT_GENERATOR_OPTIONS };
  try {
    const parsed = JSON.parse(raw) as Partial<GeneratorOptions>;
    const merged = { ...DEFAULT_GENERATOR_OPTIONS, ...parsed };
    // Guard against stale or tampered values.
    merged.kind = merged.kind === "passphrase" ? "passphrase" : "password";
    merged.length = clamp(Math.round(Number(merged.length) || 20), 5, 128);
    merged.words = clamp(Math.round(Number(merged.words) || 5), 3, 20);
    merged.minDigits = clamp(Math.round(Number(merged.minDigits) || 0), 0, 9);
    merged.minSymbols = clamp(Math.round(Number(merged.minSymbols) || 0), 0, 9);
    merged.separator = typeof merged.separator === "string" ? merged.separator.slice(0, 3) : "-";
    if (!merged.uppercase && !merged.lowercase && !merged.digits && !merged.symbols) merged.lowercase = true;
    return merged;
  } catch {
    return { ...DEFAULT_GENERATOR_OPTIONS };
  }
}

export function saveGeneratorOptions(options: GeneratorOptions): void {
  localPrefs.set(STORAGE_KEY, JSON.stringify(options));
}

function RangeRow({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  const id = useId();
  // Local draft so typing "1" on the way to "16" does not get clamped immediately.
  const [draft, setDraft] = useState(String(value));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setDraft(String(value));
  }, [value]);
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div className="gen-range">
      <label htmlFor={id} className="field-label">
        {label}
      </label>
      <div className="gen-range-controls">
        <input
          id={id}
          type="range"
          className="range"
          min={min}
          max={max}
          value={value}
          style={{ ["--pct" as string]: `${pct}%` }}
          onChange={(e) => onChange(Number(e.target.value))}
        />
        <input
          type="number"
          className="input gen-number"
          min={min}
          max={max}
          value={draft}
          aria-label={label}
          onFocus={() => (focused.current = true)}
          onChange={(e) => {
            setDraft(e.target.value);
            const n = Number(e.target.value);
            if (Number.isInteger(n) && n >= min && n <= max) onChange(n);
          }}
          onBlur={() => {
            focused.current = false;
            const n = clamp(Math.round(Number(draft) || min), min, max);
            setDraft(String(n));
            if (n !== value) onChange(n);
          }}
        />
      </div>
    </div>
  );
}

function CountInput({ label, value, onChange }: { label: string; value: number; onChange: (v: number) => void }) {
  const id = useId();
  return (
    <div className="gen-count">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        type="number"
        className="input gen-number"
        min={0}
        max={9}
        value={value}
        onChange={(e) => onChange(clamp(Math.round(Number(e.target.value) || 0), 0, 9))}
      />
    </div>
  );
}

/** Shared option controls for the generator page and the inline popover. */
export function GeneratorOptionsForm({
  options,
  onChange,
  compact,
}: {
  options: GeneratorOptions;
  onChange: (options: GeneratorOptions) => void;
  compact?: boolean;
}) {
  const { t } = useT();
  const set = (patch: Partial<GeneratorOptions>) => onChange({ ...options, ...patch });
  const sets = [options.uppercase, options.lowercase, options.digits, options.symbols].filter(Boolean).length;

  const charsetToggle = (key: "uppercase" | "lowercase" | "digits" | "symbols", label: string, hint: string) => (
    <button
      type="button"
      className="gen-toggle"
      aria-pressed={options[key]}
      // Keep at least one character set enabled.
      disabled={options[key] && sets === 1}
      onClick={() => set({ [key]: !options[key] })}
      title={hint}
    >
      <span className="gen-toggle-box" aria-hidden />
      <span className="mono">{label}</span>
    </button>
  );

  return (
    <div className={`gen-options ${compact ? "compact" : ""}`}>
      <Segmented
        block
        ariaLabel={t("generator.kind")}
        value={options.kind}
        onChange={(kind) => set({ kind })}
        options={[
          { value: "password", label: t("generator.password") },
          { value: "passphrase", label: t("generator.passphrase") },
        ]}
      />
      {options.kind === "password" ? (
        <>
          <RangeRow label={t("generator.length")} value={options.length} min={5} max={64} onChange={(length) => set({ length })} />
          <div className="gen-toggles">
            {charsetToggle("uppercase", "A–Z", t("generator.uppercase"))}
            {charsetToggle("lowercase", "a–z", t("generator.lowercase"))}
            {charsetToggle("digits", "0–9", t("generator.digits"))}
            {charsetToggle("symbols", "!@#$%^&*", t("generator.symbols"))}
          </div>
          {!compact && (
            <div className="gen-row">
              <CountInput label={t("generator.minDigits")} value={options.minDigits} onChange={(minDigits) => set({ minDigits })} />
              <CountInput label={t("generator.minSymbols")} value={options.minSymbols} onChange={(minSymbols) => set({ minSymbols })} />
            </div>
          )}
          <Checkbox checked={options.avoidAmbiguous} onChange={(avoidAmbiguous) => set({ avoidAmbiguous })}>
            {t("generator.avoidAmbiguous")}
          </Checkbox>
        </>
      ) : (
        <>
          <RangeRow label={t("generator.words")} value={options.words} min={3} max={20} onChange={(words) => set({ words })} />
          <div className="gen-row">
            <div className="gen-count">
              <label htmlFor="gen-separator">{t("generator.separator")}</label>
              <input
                id="gen-separator"
                className="input gen-number mono"
                value={options.separator}
                maxLength={3}
                onChange={(e) => set({ separator: e.target.value })}
              />
            </div>
          </div>
          <div className="gen-checks">
            <Checkbox checked={options.capitalize} onChange={(capitalize) => set({ capitalize })}>
              {t("generator.capitalize")}
            </Checkbox>
            <Checkbox checked={options.includeNumber} onChange={(includeNumber) => set({ includeNumber })}>
              {t("generator.includeNumber")}
            </Checkbox>
          </div>
        </>
      )}
    </div>
  );
}
