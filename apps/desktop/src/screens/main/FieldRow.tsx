import type { ReactNode } from "react";
import { ExternalLink, Eye, EyeOff } from "lucide-react";
import { openExternal } from "../../lib/api";
import type { SecretField } from "../../lib/types";
import { useT } from "../../i18n";
import { CopyButton } from "../../components/CopyButton";
import { PasswordText } from "../../components/PasswordText";
import { useRevealedSecret } from "../../components/useRevealedSecret";

const MASK = "••••••••••••";

function FieldRowLayout({
  label,
  value,
  mono,
  multiline,
  actions,
  footer,
}: {
  label?: string;
  value: ReactNode;
  mono?: boolean;
  multiline?: boolean;
  actions: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <div className={`fieldrow ${label ? "" : "no-label"}`}>
      <div className="fieldrow-main">
        {label && <div className="fieldrow-label">{label}</div>}
        <div className={`fieldrow-value selectable ${mono ? "mono" : ""} ${multiline ? "multiline" : ""}`}>{value}</div>
        {footer}
      </div>
      <div className="fieldrow-actions">{actions}</div>
    </div>
  );
}

export interface FieldRowProps {
  /** Omit when the surrounding section already names the value. */
  label?: string;
  value: string;
  /** Rendered instead of `value`. */
  display?: ReactNode;
  mono?: boolean;
  multiline?: boolean;
  /** Label for the copy toast; omit to hide the copy button. */
  copyLabel?: string;
  sensitive?: boolean;
  /** Turns the value into a link that opens in the system browser. */
  href?: string;
  actions?: ReactNode;
  footer?: ReactNode;
}

/** A value the item list holds (no secret: those use `SecretFieldRow`). */
export function FieldRow({
  label,
  value,
  display,
  mono,
  multiline,
  copyLabel,
  sensitive = false,
  href,
  actions,
  footer,
}: FieldRowProps) {
  const { t } = useT();

  let content: ReactNode;
  if (display !== undefined) content = display;
  else if (href)
    content = (
      <button type="button" className="field-link truncate" onClick={() => openExternal(href)} title={value}>
        {value}
      </button>
    );
  else content = value;

  return (
    <FieldRowLayout
      label={label}
      value={content}
      mono={mono}
      multiline={multiline}
      footer={footer}
      actions={
        <>
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
          {copyLabel && <CopyButton value={value} label={copyLabel} sensitive={sensitive} />}
        </>
      }
    />
  );
}

export interface SecretFieldRowProps {
  itemId: string;
  field: SecretField;
  /** The item's `updatedAt`: a revealed value is hidden again when the item changes. */
  revision: number;
  label: string;
  /** Name of the value in the copy toast (default: `label`). */
  copyLabel?: string;
  /** Colour-code digits/symbols when revealed. */
  password?: boolean;
  /** Monospace when revealed. */
  mono?: boolean;
  /** Renders the revealed value (e.g. grouped card digits). */
  format?: (value: string) => ReactNode;
}

/**
 * A secret of an item: masked until the eye toggle fetches it
 * (`reveal_secret`, hidden again after 30 s, see `useRevealedSecret`); the
 * copy button copies in the backend (`copy_secret_field`).
 */
export function SecretFieldRow({ itemId, field, revision, label, copyLabel, password, mono, format }: SecretFieldRowProps) {
  const { t } = useT();
  const secret = useRevealedSecret(itemId, field, revision);
  const shown = secret.value !== null;

  let content: ReactNode;
  if (secret.value === null) content = <span className="mask">{MASK}</span>;
  else if (format) content = format(secret.value);
  else if (password) content = <PasswordText value={secret.value} />;
  else content = secret.value;

  const toggleLabel = shown ? t("common.hide") : t("common.show");
  return (
    <FieldRowLayout
      label={label}
      value={content}
      mono={mono && shown}
      actions={
        <>
          <button
            type="button"
            className="icon-btn"
            onClick={secret.toggle}
            aria-busy={secret.loading}
            title={toggleLabel}
            aria-label={`${toggleLabel}: ${label}`}
            aria-pressed={shown}
          >
            {shown ? <EyeOff /> : <Eye />}
          </button>
          <CopyButton secret={{ itemId, field }} label={copyLabel ?? label} />
        </>
      }
    />
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
