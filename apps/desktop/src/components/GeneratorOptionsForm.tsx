import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ChevronRight, CircleAlert, RotateCcw } from "lucide-react";
import { DEFAULT_SYMBOLS, type GeneratorOptions, type Wordlist } from "../lib/types";
import { clamp } from "../lib/utils";
import {
  CHARSET_INPUT_MAX_CHARS,
  MIN_KEYS,
  PASSPHRASE_MAX_WORDS,
  PASSPHRASE_MIN_WORDS,
  PASSWORD_MAX_LEN,
  PASSWORD_MIN_LEN,
  SEPARATOR_MAX_CHARS,
  entropyOf,
  entropyScore,
  passphraseIssue,
  passwordPlan,
  requiredCharacters,
  symbolSetProblem,
  type CharClass,
} from "../lib/generatorRules";
import { useT, type MessageKey } from "../i18n";
import { Checkbox, Field, Segmented } from "./Controls";

/**
 * Whether Enter on `el` should generate (page and popover): yes on text and
 * number inputs and outside any control; no where Enter has a meaning of its
 * own – buttons, links, selects, checkboxes, radios, menu items.
 */
export function enterGenerates(el: Element | null): boolean {
  if (!el || el === document.body) return true;
  if (el instanceof HTMLInputElement) {
    return !["checkbox", "radio", "button", "submit", "reset", "file", "image", "color"].includes(el.type);
  }
  if (
    el instanceof HTMLButtonElement ||
    el instanceof HTMLSelectElement ||
    el instanceof HTMLTextAreaElement ||
    el instanceof HTMLAnchorElement
  ) {
    return false;
  }
  const role = el.getAttribute("role");
  return !role || !["button", "radio", "menuitem", "menuitemradio", "switch", "checkbox", "option", "tab", "link"].includes(role);
}

/**
 * A number input that keeps what is typed as a draft (typing "1" on the way
 * to "16" is not clamped at once) and commits it – clamped – on blur and on
 * Enter (Enter then generates on the page and in the popover).
 */
function NumberInput({
  id,
  value,
  min,
  max,
  label,
  onChange,
  disabled,
  invalid,
}: {
  id?: string;
  value: number;
  min: number;
  max: number;
  label: string;
  onChange: (value: number) => void;
  disabled?: boolean;
  invalid?: boolean;
}) {
  const [draft, setDraft] = useState(String(value));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setDraft(String(value));
  }, [value]);
  const commit = () => {
    const parsed = Number(draft);
    const n = draft.trim() === "" || !Number.isFinite(parsed) ? value : clamp(Math.round(parsed), min, max);
    setDraft(String(n));
    if (n !== value) onChange(n);
  };
  return (
    <input
      id={id}
      type="number"
      inputMode="numeric"
      className={`input gen-number ${invalid ? "invalid" : ""}`}
      min={min}
      max={max}
      value={draft}
      disabled={disabled}
      aria-label={label}
      aria-invalid={invalid || undefined}
      onFocus={() => (focused.current = true)}
      onChange={(e) => {
        setDraft(e.target.value);
        const n = Number(e.target.value);
        if (e.target.value.trim() !== "" && Number.isInteger(n) && n >= min && n <= max) onChange(n);
      }}
      onBlur={() => {
        focused.current = false;
        commit();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
      }}
    />
  );
}

function RangeRow({
  label,
  value,
  min,
  max,
  onChange,
  error,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  error?: string | null;
}) {
  const id = useId();
  const shown = clamp(value, min, max);
  const pct = ((shown - min) / (max - min)) * 100;
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
          value={shown}
          style={{ ["--pct" as string]: `${pct}%` }}
          onChange={(e) => onChange(Number(e.target.value))}
        />
        <NumberInput value={value} min={min} max={max} label={label} onChange={onChange} invalid={Boolean(error)} />
      </div>
      <Message error={error} />
    </div>
  );
}

function Message({ error, hint }: { error?: string | null; hint?: ReactNode }) {
  if (error) {
    return (
      <div className="field-error" role="alert">
        <CircleAlert aria-hidden />
        <span>{error}</span>
      </div>
    );
  }
  return hint ? <div className="field-hint">{hint}</div> : null;
}

const CLASS_LABEL_KEYS: Record<CharClass, MessageKey> = {
  uppercase: "generator.uppercase",
  lowercase: "generator.lowercase",
  digits: "generator.digits",
  symbols: "generator.symbols",
};

const MIN_LABEL_KEYS: Record<CharClass, MessageKey> = {
  uppercase: "generator.minUppercase",
  lowercase: "generator.minLowercase",
  digits: "generator.minDigits",
  symbols: "generator.minSymbols",
};

/** Messages for the options as the core would check them (one problem at a time, like `generate_password`). */
function useProblems(options: GeneratorOptions) {
  const { t } = useT();
  const problems: Partial<
    Record<"length" | "classes" | "minimums" | "symbolSet" | "exclude" | "startWithLetter" | "words" | "separator", string>
  > = {};
  if (options.kind === "passphrase") {
    const issue = passphraseIssue(options);
    if (issue === "words") problems.words = t("error.input.words");
    if (issue === "separator") problems.separator = t("error.input.separator");
    return problems;
  }
  const result = passwordPlan(options);
  if (result.ok) return problems;
  switch (result.issue) {
    case "length":
      problems.length = t("error.input.length");
      break;
    case "symbol_set": {
      const problem = symbolSetProblem(options) ?? "invalid_char";
      problems.symbolSet = t(`generator.symbolSet.${problem}`);
      break;
    }
    case "exclude":
      problems.exclude = t("error.input.exclude");
      break;
    case "no_character_set":
      if (result.emptied) problems.exclude = t(`generator.emptied.${result.emptied}`);
      else problems.classes = t("error.input.no_character_set");
      break;
    case "start_with_letter":
      problems.startWithLetter = t("error.input.start_with_letter");
      break;
    case "minimums_exceed_length":
      problems.minimums = t("generator.minimumsTooMany", { n: requiredCharacters(options), length: options.length });
      break;
    default:
      break;
  }
  return problems;
}

/** The estimated entropy of the options (from the character set or the real word list size). */
export function EntropyHint({ options }: { options: GeneratorOptions }) {
  const { t, locale } = useT();
  const entropy = entropyOf(options);
  if (!entropy) return null;
  const score = entropyScore(entropy.bits);
  const int = new Intl.NumberFormat(locale, { maximumFractionDigits: 0 });
  const one = new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  const detail =
    entropy.pool !== undefined
      ? t("generator.entropyPool", { pool: int.format(entropy.pool) })
      : t("generator.entropyWords", { bits: one.format(entropy.bitsPerWord ?? 0), size: int.format(entropy.listSize ?? 0) });
  return (
    <div className="gen-entropy" data-score={score} title={t("generator.entropyHint")}>
      <span className="gen-entropy-dot" aria-hidden />
      <span className="gen-entropy-bits">{t("generator.entropy", { bits: int.format(entropy.bits) })}</span>
      <span className="gen-entropy-label">{t(`strength.${score}`)}</span>
      <span className="gen-entropy-detail">{detail}</span>
    </div>
  );
}

/**
 * Shared option controls for the generator page and the editor's popover.
 * Changing an option never generates anything; problems are shown inline
 * (the same checks as the core, see lib/generatorRules.ts).
 */
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
  const problems = useProblems(options);
  const enabled = [options.uppercase, options.lowercase, options.digits, options.symbols].filter(Boolean).length;
  const symbolSetId = useId();
  const excludeId = useId();
  const separatorId = useId();
  const advancedProblem = Boolean(problems.minimums || problems.symbolSet || problems.exclude || problems.startWithLetter);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const showAdvanced = !compact || advancedOpen || advancedProblem;
  const symbolPreview = Array.from(options.symbolSet).slice(0, 3).join("") || "!#$";
  const required = requiredCharacters(options);

  const classLabel: Record<CharClass, string> = {
    uppercase: "A–Z",
    lowercase: "a–z",
    digits: "0–9",
    symbols: symbolPreview,
  };

  const charsetToggle = (cls: CharClass) => (
    <button
      key={cls}
      type="button"
      className="gen-toggle"
      aria-pressed={options[cls]}
      // Keep at least one character group enabled.
      disabled={options[cls] && enabled === 1}
      onClick={() => set({ [cls]: !options[cls] })}
      title={t(CLASS_LABEL_KEYS[cls])}
      aria-label={t(CLASS_LABEL_KEYS[cls])}
    >
      <span className="gen-toggle-box" aria-hidden />
      <span className="mono" aria-hidden>
        {classLabel[cls]}
      </span>
    </button>
  );

  const minInput = (cls: CharClass) => {
    const key = MIN_KEYS[cls];
    return (
      <NumberInput
        key={cls}
        value={options[key]}
        min={0}
        max={Math.max(0, Math.min(options.length, PASSWORD_MAX_LEN))}
        label={t(MIN_LABEL_KEYS[cls])}
        disabled={!options[cls]}
        invalid={Boolean(problems.minimums) && options[cls] && options[key] > 0}
        onChange={(n) => set({ [key]: n })}
      />
    );
  };

  const advanced = (
    <>
      <div className="gen-subsection">
        <div className="field-label" aria-hidden>
          {t("generator.minimums")}
        </div>
        <div className="gen-grid4">{(["uppercase", "lowercase", "digits", "symbols"] as CharClass[]).map(minInput)}</div>
        <Message
          error={problems.minimums}
          hint={!compact && required > 0 ? t("generator.minimumsHint", { n: required, length: options.length }) : null}
        />
      </div>
      <Field
        label={t("generator.symbolSet")}
        htmlFor={symbolSetId}
        error={problems.symbolSet}
        hint={compact ? undefined : t("generator.symbolSetHint")}
        aside={
          options.symbolSet !== DEFAULT_SYMBOLS && (
            <button type="button" className="link-btn" onClick={() => set({ symbolSet: DEFAULT_SYMBOLS })}>
              <RotateCcw size={13} />
              {t("generator.symbolSetReset")}
            </button>
          )
        }
      >
        <input
          id={symbolSetId}
          className={`input mono ${problems.symbolSet ? "invalid" : ""}`}
          value={options.symbolSet}
          maxLength={CHARSET_INPUT_MAX_CHARS}
          disabled={!options.symbols}
          aria-invalid={Boolean(problems.symbolSet) || undefined}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          onChange={(e) => set({ symbolSet: e.target.value })}
        />
      </Field>
      <Field
        label={t("generator.exclude")}
        htmlFor={excludeId}
        error={problems.exclude}
        hint={compact ? undefined : t("generator.excludeHint")}
      >
        <input
          id={excludeId}
          className={`input mono ${problems.exclude ? "invalid" : ""}`}
          value={options.exclude}
          maxLength={CHARSET_INPUT_MAX_CHARS}
          placeholder={t("generator.excludePlaceholder")}
          aria-invalid={Boolean(problems.exclude) || undefined}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          onChange={(e) => set({ exclude: e.target.value })}
        />
      </Field>
      <div className="gen-checks">
        <Checkbox checked={options.startWithLetter} onChange={(startWithLetter) => set({ startWithLetter })}>
          {t("generator.startWithLetter")}
        </Checkbox>
        <Message error={problems.startWithLetter} />
        <Checkbox checked={options.avoidAmbiguous} onChange={(avoidAmbiguous) => set({ avoidAmbiguous })}>
          {t("generator.avoidAmbiguous")}
        </Checkbox>
      </div>
    </>
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
          <RangeRow
            label={t("generator.length")}
            value={options.length}
            min={PASSWORD_MIN_LEN}
            max={PASSWORD_MAX_LEN}
            onChange={(length) => set({ length })}
            error={problems.length}
          />
          <div className="gen-subsection">
            <div className="field-label" aria-hidden>
              {t("generator.characters")}
            </div>
            <div className="gen-toggles gen-grid4">
              {(["uppercase", "lowercase", "digits", "symbols"] as CharClass[]).map(charsetToggle)}
            </div>
            <Message error={problems.classes} />
          </div>
          {compact && (
            <button
              type="button"
              className="collapse-toggle gen-more"
              aria-expanded={showAdvanced}
              onClick={() => setAdvancedOpen((open) => !open)}
              disabled={advancedProblem}
            >
              <ChevronRight className="collapse-chevron" />
              <span>{t("generator.moreOptions")}</span>
            </button>
          )}
          {showAdvanced && advanced}
        </>
      ) : (
        <>
          <RangeRow
            label={t("generator.words")}
            value={options.words}
            min={PASSPHRASE_MIN_WORDS}
            max={PASSPHRASE_MAX_WORDS}
            onChange={(words) => set({ words })}
            error={problems.words}
          />
          <div className="gen-row">
            <div className="gen-count">
              <label htmlFor={separatorId}>{t("generator.separator")}</label>
              <input
                id={separatorId}
                className={`input gen-number mono ${problems.separator ? "invalid" : ""}`}
                value={options.separator}
                maxLength={SEPARATOR_MAX_CHARS}
                spellCheck={false}
                autoComplete="off"
                aria-invalid={Boolean(problems.separator) || undefined}
                onChange={(e) => set({ separator: e.target.value })}
              />
            </div>
            <div className="gen-count gen-wordlist">
              <span aria-hidden>{t("generator.wordlist")}</span>
              <Segmented<Wordlist>
                ariaLabel={t("generator.wordlist")}
                value={options.wordlist}
                onChange={(wordlist) => set({ wordlist })}
                options={[
                  { value: "en", label: t("generator.wordlist.en") },
                  { value: "de", label: t("generator.wordlist.de") },
                ]}
              />
            </div>
          </div>
          <Message error={problems.separator} />
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
      <EntropyHint options={options} />
    </div>
  );
}
