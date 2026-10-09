import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import type { GeneratorOptions } from "../lib/types";
import { useEscapeLayer, useOutsideClick } from "../lib/layers";
import { useT } from "../i18n";
import { Button } from "./Controls";
import { GeneratorOptionsForm, loadGeneratorOptions, saveGeneratorOptions } from "./GeneratorOptionsForm";
import { PasswordText } from "./PasswordText";
import { StrengthMeter } from "./StrengthMeter";

/** Compact generator shown next to a password field. */
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
  const ref = useRef<HTMLDivElement>(null);
  const [options, setOptions] = useState<GeneratorOptions>(loadGeneratorOptions);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEscapeLayer(onClose);
  useOutsideClick([ref, anchorRef], onClose);

  const generate = useCallback(
    async (opts: GeneratorOptions) => {
      try {
        setValue(await api.generatePassword(opts, false));
        setError(null);
      } catch (err) {
        setError(errorText(err));
      }
    },
    [errorText],
  );

  useEffect(() => {
    saveGeneratorOptions(options);
    const timer = window.setTimeout(() => void generate(options), 80);
    return () => window.clearTimeout(timer);
  }, [options, generate]);

  return (
    <div ref={ref} className="popover gen-popover" role="dialog" aria-label={t("generator.title")}>
      <div className="gen-popover-value">
        <div className="gen-popover-text selectable">{value ? <PasswordText value={value} /> : " "}</div>
        <button
          type="button"
          className="icon-btn sm"
          onClick={() => void generate(options)}
          aria-label={t("generator.regenerate")}
          title={t("generator.regenerate")}
        >
          <RefreshCw />
        </button>
      </div>
      {error ? <div className="field-error">{error}</div> : <StrengthMeter password={value} />}
      <GeneratorOptionsForm compact options={options} onChange={setOptions} />
      <div className="gen-popover-footer">
        <Button variant="ghost" size="sm" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={!value}
          onClick={() => {
            onUse(value);
            onClose();
          }}
        >
          {t("generator.use")}
        </Button>
      </div>
    </div>
  );
}
