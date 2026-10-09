import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useT } from "../i18n";
import { useCopy } from "../state/app";

/** Icon button that copies a value and briefly turns into a check mark. */
export function CopyButton({
  value,
  label,
  sensitive,
  disabled,
  className,
}: {
  value: string;
  /** Translated name of the value ("Passwort"), used for the tooltip and toast. */
  label: string;
  sensitive: boolean;
  disabled?: boolean;
  className?: string;
}) {
  const { t } = useT();
  const copy = useCopy();
  const [done, setDone] = useState(false);
  const timer = useRef(0);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const name = t("common.copyNamed", { what: label });
  return (
    <button
      type="button"
      className={`icon-btn ${done ? "success" : ""} ${className ?? ""}`}
      disabled={disabled}
      onClick={() => {
        void copy(value, { label, sensitive }).then((ok) => {
          if (!ok) return;
          setDone(true);
          window.clearTimeout(timer.current);
          timer.current = window.setTimeout(() => setDone(false), 1500);
        });
      }}
      title={name}
      aria-label={name}
    >
      {done ? <Check /> : <Copy />}
    </button>
  );
}
