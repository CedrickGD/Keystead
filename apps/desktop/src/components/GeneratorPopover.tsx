import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type RefObject } from "react";
import { Check, Copy, RefreshCw, WandSparkles } from "lucide-react";
import { api } from "../lib/api";
import { DEFAULT_GENERATOR_OPTIONS, type GeneratorOptions, type GeneratorPreset, type GeneratorPresets } from "../lib/types";
import { generatorIssue, normalizedOptions, sameOptions } from "../lib/generatorRules";
import { useEscapeLayer, useOutsideClick } from "../lib/layers";
import { useT } from "../i18n";
import { useCopy } from "../state/app";
import { Button } from "./Controls";
import { GeneratorOptionsForm, enterGenerates } from "./GeneratorOptionsForm";
import { PresetSelect, findPreset } from "./GeneratorPresets";
import { PasswordText } from "./PasswordText";
import { StrengthMeter } from "./StrengthMeter";
import { useToast } from "./Toasts";

/**
 * Compact generator next to a password field. It starts with the default
 * preset (`generator_presets`) and generates only on a click (Enter / Ctrl+G
 * inside it); the preview is not stored. "Verwenden" puts it into the field
 * and – like copying it – reports it as used (`remember_generated`), so it
 * enters the generator history.
 */
export function GeneratorPopover({
  onUse,
  onClose,
  anchorRef,
}: {
  onUse: (password: string) => void;
  onClose: () => void;
  anchorRef: RefObject<HTMLElement | null>;
}) {
  const { t, errorText } = useT();
  const toast = useToast();
  const copy = useCopy();
  const ref = useRef<HTMLDivElement>(null);
  const generateRef = useRef<HTMLButtonElement>(null);
  const refreshRef = useRef<HTMLButtonElement>(null);
  /** Move the focus to "Neu generieren" once the first preview is shown. */
  const focusRefresh = useRef(false);
  const [presets, setPresets] = useState<GeneratorPresets | null>(null);
  const [presetId, setPresetId] = useState<string | null>(null);
  const [options, setOptions] = useState<GeneratorOptions>({ ...DEFAULT_GENERATOR_OPTIONS });
  const [ready, setReady] = useState(false);
  const [preview, setPreview] = useState<{ value: string; options: GeneratorOptions } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const requestId = useRef(0);
  const copiedTimer = useRef(0);
  useEffect(() => () => window.clearTimeout(copiedTimer.current), []);

  const [placement, setPlacement] = useState<CSSProperties>({});

  // Closing via Esc or the buttons returns focus to the toggle button.
  const closeAndRefocus = () => {
    onClose();
    anchorRef.current?.focus();
  };
  useEscapeLayer(closeAndRefocus);
  useOutsideClick([ref, anchorRef], onClose);

  // Open upwards when there is not enough room below the field.
  useLayoutEffect(() => {
    const el = ref.current;
    const anchor = el?.parentElement;
    if (!el || !anchor) return;
    let rect = anchor.getBoundingClientRect();
    const height = el.offsetHeight;
    // In small windows, first scroll the field up (just below the sticky header).
    const scroller = anchor.closest<HTMLElement>(".detail-pane");
    if (scroller && window.innerHeight - rect.bottom - 12 < height) {
      const headerHeight = scroller.querySelector<HTMLElement>(".detail-header")?.offsetHeight ?? 0;
      scroller.scrollTop += rect.top - scroller.getBoundingClientRect().top - headerHeight - 8;
      rect = anchor.getBoundingClientRect();
    }
    const below = window.innerHeight - rect.bottom - 12;
    const above = rect.top - 12;
    if (below >= height || below >= above) {
      setPlacement({ top: "calc(100% + 6px)", maxHeight: Math.max(240, below) });
    } else {
      setPlacement({ bottom: "calc(100% + 6px)", top: "auto", maxHeight: Math.max(240, above) });
    }
  }, []);

  // The default preset – the editor has no options of its own.
  const loaded = useRef(false);
  useEffect(() => {
    if (loaded.current) return;
    loaded.current = true;
    api
      .generatorPresets()
      .then((next) => {
        setPresets(next);
        setPresetId(next.defaultId);
        setOptions({ ...next.defaultOptions });
      })
      .catch((err) => setError(errorText(err)))
      .finally(() => setReady(true));
  }, [errorText]);

  useEffect(() => {
    if (ready) generateRef.current?.focus({ preventScroll: true });
  }, [ready]);

  const current = findPreset(presets, presetId);
  // The picker names the preset only while the options are still its own.
  const shownPresetId = current && sameOptions(normalizedOptions(options), current.options) ? current.id : null;
  const issue = generatorIssue(options);
  const stale = preview !== null && !sameOptions(preview.options, options);

  const generate = async () => {
    if (!ready || issue) return;
    const id = ++requestId.current;
    const opts = options;
    const first = preview === null;
    try {
      const value = await api.generatePreview(opts);
      if (id !== requestId.current) return;
      setPreview({ value, options: opts });
      setError(null);
      setCopied(false);
      // The "Generieren" button gives way to the preview: keep the focus inside.
      if (first) focusRefresh.current = true;
    } catch (err) {
      if (id === requestId.current) setError(errorText(err));
    }
  };
  const generateLatest = useRef(generate);
  generateLatest.current = generate;

  useEffect(() => {
    if (!preview || !focusRefresh.current) return;
    focusRefresh.current = false;
    refreshRef.current?.focus({ preventScroll: true });
  }, [preview]);

  const remember = (value: string) => {
    api.rememberGenerated(value).catch((err) => toast.error(errorText(err)));
  };

  const copyPreview = async () => {
    if (!preview) return;
    const value = preview.value;
    const label = preview.options.kind === "passphrase" ? t("generator.passphrase") : t("field.password");
    if (!(await copy(value, { label, sensitive: true }))) return;
    setCopied(true);
    window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => setCopied(false), 1500);
    remember(value);
  };

  const use = () => {
    if (!preview) return;
    onUse(preview.value);
    remember(preview.value);
    closeAndRefocus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.altKey) return;
    const mod = e.ctrlKey || e.metaKey;
    const shortcut = mod && !e.shiftKey && e.key.toLowerCase() === "g";
    // Enter in an input generates (and must not submit the editor's form).
    const enter = e.key === "Enter" && !mod && !e.shiftKey && e.target instanceof HTMLInputElement && enterGenerates(e.target);
    if (!shortcut && !enter) return;
    e.preventDefault();
    e.stopPropagation();
    window.setTimeout(() => void generateLatest.current(), 0);
  };

  const selectPreset = (preset: GeneratorPreset) => {
    setPresetId(preset.id);
    setOptions({ ...preset.options });
  };

  let hint: string | null = null;
  if (issue) hint = t("generator.invalid");
  else if (stale) hint = t("generator.stale");
  else if (!preview) hint = t("generator.popoverHint");

  return (
    <div
      ref={ref}
      className="popover gen-popover"
      role="dialog"
      aria-label={t("generator.title")}
      style={placement}
      onKeyDown={onKeyDown}
    >
      <PresetSelect size="sm" presets={presets} value={shownPresetId} onSelect={selectPreset} />
      <div className="gen-popover-value">
        {preview ? (
          <>
            <div className="gen-popover-text selectable">
              <PasswordText value={preview.value} />
            </div>
            <button
              ref={refreshRef}
              type="button"
              className="icon-btn sm"
              onClick={() => void generate()}
              disabled={Boolean(issue)}
              aria-label={t("generator.regenerate")}
              title={t("generator.regenerate")}
            >
              <RefreshCw />
            </button>
            <button
              type="button"
              className={`icon-btn sm ${copied ? "success" : ""}`}
              onClick={() => void copyPreview()}
              aria-label={t("common.copyNamed", { what: t("field.password") })}
              title={t("common.copyNamed", { what: t("field.password") })}
            >
              {copied ? <Check /> : <Copy />}
            </button>
          </>
        ) : (
          <Button
            ref={generateRef}
            variant="primary"
            size="sm"
            block
            onClick={() => void generate()}
            disabled={!ready || Boolean(issue)}
            icon={<WandSparkles />}
          >
            {t("generator.generateShort")}
          </Button>
        )}
      </div>
      {error ? (
        <div className="field-error" role="alert">
          {error}
        </div>
      ) : (
        preview && <StrengthMeter password={preview.value} />
      )}
      {hint && <div className="field-hint">{hint}</div>}
      <GeneratorOptionsForm compact options={options} onChange={setOptions} />
      <div className="gen-popover-footer">
        <Button variant="ghost" size="sm" onClick={closeAndRefocus}>
          {t("common.cancel")}
        </Button>
        <Button variant="primary" size="sm" disabled={!preview} onClick={use}>
          {t("generator.use")}
        </Button>
      </div>
    </div>
  );
}
