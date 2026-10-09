import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from "react";
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
    <div ref={ref} className="popover gen-popover" role="dialog" aria-label={t("generator.title")} style={placement}>
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
        <Button variant="ghost" size="sm" onClick={closeAndRefocus}>
          {t("common.cancel")}
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={!value}
          onClick={() => {
            onUse(value);
            closeAndRefocus();
          }}
        >
          {t("generator.use")}
        </Button>
      </div>
    </div>
  );
}
