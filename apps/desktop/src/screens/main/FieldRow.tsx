import { useState, type ReactNode } from "react";
import { ExternalLink, Eye, EyeOff } from "lucide-react";
import { openExternal } from "../../lib/api";
import { useT } from "../../i18n";
import { CopyButton } from "../../components/CopyButton";
import { PasswordText } from "../../components/PasswordText";

const MASK = "••••••••••••";

export interface FieldRowProps {
  /** Omit when the surrounding section already names the value. */
  label?: string;
  value: string;
  /** Rendered instead of `value` (when not masked). */
  display?: ReactNode;
  mono?: boolean;
  secret?: boolean;
  /** Colour-code digits/symbols when revealed. */
  password?: boolean;
  multiline?: boolean;
  /** Label for the copy toast; omit to hide the copy button. */
  copyLabel?: string;
  sensitive?: boolean;
  /** Turns the value into a link that opens in the system browser. */
  href?: string;
  actions?: ReactNode;
  footer?: ReactNode;
}

export function FieldRow({
  label,
  value,
  display,
  mono,
  secret,
  password,
  multiline,
  copyLabel,
  sensitive = false,
  href,
  actions,
  footer,
}: FieldRowProps) {
  const { t } = useT();
  const [revealed, setRevealed] = useState(false);
  const masked = secret && !revealed;

  let content: ReactNode;
  if (masked) content = <span className="mask">{MASK}</span>;
  else if (display !== undefined) content = display;
  else if (password) content = <PasswordText value={value} />;
  else if (href)
    content = (
      <button type="button" className="field-link truncate" onClick={() => openExternal(href)} title={value}>
        {value}
      </button>
    );
  else content = value;

  return (
    <div className={`fieldrow ${label ? "" : "no-label"}`}>
      <div className="fieldrow-main">
        {label && <div className="fieldrow-label">{label}</div>}
        <div className={`fieldrow-value selectable ${mono && !masked ? "mono" : ""} ${multiline ? "multiline" : ""}`}>
          {content}
        </div>
        {footer}
      </div>
      <div className="fieldrow-actions">
        {actions}
        {href && (
          <button
            type="button"
            className="icon-btn"
            onClick={() => openExternal(href)}
            title={t("item.openWebsite")}
            aria-label={t("item.openWebsite")}
          >
            <ExternalLink />
          </button>
        )}
        {secret && (
          <button
            type="button"
            className="icon-btn"
            onClick={() => setRevealed((r) => !r)}
            title={revealed ? t("common.hide") : t("common.show")}
            aria-label={`${revealed ? t("common.hide") : t("common.show")}: ${label ?? copyLabel ?? ""}`}
            aria-pressed={revealed}
          >
            {revealed ? <EyeOff /> : <Eye />}
          </button>
        )}
        {copyLabel && <CopyButton value={value} label={copyLabel} sensitive={sensitive} />}
      </div>
    </div>
  );
}

export function Section({ title, children, aside }: { title?: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="detail-section">
      {title && (
        <div className="section-label">
          <span>{title}</span>
          {aside && <span style={{ marginLeft: "auto" }}>{aside}</span>}
        </div>
      )}
      <div className="card field-card">{children}</div>
    </section>
  );
}
