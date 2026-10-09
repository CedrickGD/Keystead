import { Copy, Check } from "lucide-react";
import { useState } from "react";
import { useT } from "../i18n";
import { useCopy } from "../state/app";
import { Checkbox } from "./Controls";

/** Shows a freshly created recovery key once, with copy + "I wrote it down". */
export function RecoveryKeyReveal({
  value,
  confirmed,
  onConfirmedChange,
}: {
  value: string;
  confirmed: boolean;
  onConfirmedChange: (confirmed: boolean) => void;
}) {
  const { t } = useT();
  const copy = useCopy();
  const [copied, setCopied] = useState(false);
  return (
    <div className="recovery-reveal">
      <div className="recovery-key">
        <code className="selectable" aria-label={t("recovery.keyAria")}>
          {value}
        </code>
        <button
          type="button"
          className="btn btn-secondary btn-sm"
          onClick={() => {
            void copy(value, { label: t("recovery.key"), sensitive: true });
            setCopied(true);
          }}
        >
          {copied ? <Check /> : <Copy />}
          {copied ? t("common.copied") : t("common.copy")}
        </button>
      </div>
      <div className="callout callout-warning">
        <span>{t("recovery.storeHint")}</span>
      </div>
      <Checkbox checked={confirmed} onChange={onConfirmedChange}>
        {t("recovery.confirmStored")}
      </Checkbox>
    </div>
  );
}
