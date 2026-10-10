import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import type { SecretField } from "../lib/types";
import { useT } from "../i18n";
import { useCopy, useCopySecret } from "../state/app";

/** What to copy: a value the page has, or a secret of an item the backend copies itself. */
export type CopySource = { value: string; sensitive: boolean } | { secret: { itemId: string; field: SecretField } };

/** Icon button that copies a value and briefly turns into a check mark. */
export function CopyButton({
  label,
  disabled,
  className,
  ...source
}: CopySource & {
  /** Translated name of the value ("Passwort"), used for the tooltip and toast. */
  label: string;
  disabled?: boolean;
  className?: string;
}) {
  const { t } = useT();
  const copy = useCopy();
  const copySecret = useCopySecret();
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
        const copied =
          "secret" in source
            ? copySecret(source.secret.itemId, source.secret.field, label)
            : copy(source.value, { label, sensitive: source.sensitive });
        void copied.then((ok) => {
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
