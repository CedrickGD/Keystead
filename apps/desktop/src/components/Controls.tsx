import {
  forwardRef,
  useId,
  useState,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
} from "react";
import { ChevronDown, CircleAlert, Eye, EyeOff } from "lucide-react";
import { useT } from "../i18n";
import { classNames } from "../lib/utils";

// ---------------------------------------------------------------------------
// Button with loading state
// ---------------------------------------------------------------------------

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "danger-ghost" | "danger-outline";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: "sm" | "md" | "lg";
  loading?: boolean;
  block?: boolean;
  icon?: ReactNode;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", loading, block, icon, className, children, disabled, type = "button", ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      className={classNames(
        "btn",
        `btn-${variant}`,
        size === "sm" && "btn-sm",
        size === "lg" && "btn-lg",
        block && "btn-block",
        className,
      )}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      {loading ? <span className="spinner" aria-hidden /> : icon}
      {children}
    </button>
  );
});

// ---------------------------------------------------------------------------
// Field wrapper
// ---------------------------------------------------------------------------

export function Field({
  label,
  hint,
  error,
  htmlFor,
  children,
  className,
  aside,
}: {
  label?: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
  className?: string;
  aside?: ReactNode;
}) {
  return (
    <div className={classNames("field", className)}>
      {label && (
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
          <label className="field-label" htmlFor={htmlFor}>
            {label}
          </label>
          {aside}
        </div>
      )}
      {children}
      {error ? (
        <div className="field-error" role="alert">
          <CircleAlert aria-hidden />
          <span>{error}</span>
        </div>
      ) : (
        hint && <div className="field-hint">{hint}</div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Password input with show/hide toggle and optional extra actions
// ---------------------------------------------------------------------------

export interface PasswordInputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "onChange" | "value" | "size" | "type"> {
  value: string;
  onChange: (value: string) => void;
  size?: "md" | "lg";
  invalid?: boolean;
  mono?: boolean;
  /** Additional icon buttons rendered before the eye toggle. */
  actions?: ReactNode;
  actionCount?: number;
  /** Start revealed (e.g. for freshly generated values). */
  defaultVisible?: boolean;
}

export const PasswordInput = forwardRef<HTMLInputElement, PasswordInputProps>(function PasswordInput(
  { value, onChange, size = "md", invalid, mono = true, actions, actionCount = 0, defaultVisible = false, className, ...rest },
  ref,
) {
  const { t } = useT();
  const [visible, setVisible] = useState(defaultVisible);
  return (
    <div className="input-wrap" style={{ ["--actions" as string]: actionCount + 1 }}>
      <input
        ref={ref}
        type={visible ? "text" : "password"}
        className={classNames("input", size === "lg" && "lg", mono && visible && "mono", invalid && "invalid", className)}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        aria-invalid={invalid || undefined}
        {...rest}
      />
      <div className="input-actions">
        {actions}
        <button
          type="button"
          className="icon-btn"
          onClick={() => setVisible((v) => !v)}
          aria-label={visible ? t("common.hide") : t("common.show")}
          title={visible ? t("common.hide") : t("common.show")}
          aria-pressed={visible}
        >
          {visible ? <EyeOff /> : <Eye />}
        </button>
      </div>
    </div>
  );
});

// ---------------------------------------------------------------------------
// Select (native, styled)
// ---------------------------------------------------------------------------

export interface SelectOption<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
}

export function Select<T extends string>({
  value,
  onChange,
  options,
  size = "md",
  id,
  disabled,
  ariaLabel,
  className,
}: {
  value: T;
  onChange: (value: T) => void;
  options: SelectOption<T>[];
  size?: "sm" | "md";
  id?: string;
  disabled?: boolean;
  ariaLabel?: string;
  className?: string;
}) {
  return (
    <div className={classNames("select-wrap", size === "sm" && "sm", className)}>
      <select
        id={id}
        className="input"
        value={value}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(e) => onChange(e.target.value as T)}
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value} disabled={opt.disabled}>
            {opt.label}
          </option>
        ))}
      </select>
      <ChevronDown aria-hidden />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Switch, checkbox, segmented control
// ---------------------------------------------------------------------------

export function Switch({
  checked,
  onChange,
  label,
  disabled,
  id,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      className="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

export function Checkbox({
  checked,
  onChange,
  children,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  children: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="checkbox">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{children}</span>
    </label>
  );
}

export function Segmented<T extends string>({
  value,
  onChange,
  options,
  ariaLabel,
  block,
}: {
  value: T;
  onChange: (value: T) => void;
  options: { value: T; label: string; icon?: ReactNode }[];
  ariaLabel: string;
  block?: boolean;
}) {
  const name = useId();
  return (
    <div className={classNames("segmented", block && "block")} role="radiogroup" aria-label={ariaLabel}>
      {options.map((opt, idx) => (
        <button
          key={opt.value}
          type="button"
          role="radio"
          aria-checked={opt.value === value}
          tabIndex={opt.value === value ? 0 : -1}
          data-group={name}
          onClick={() => onChange(opt.value)}
          onKeyDown={(e) => {
            if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
            e.preventDefault();
            const next = options[(idx + (e.key === "ArrowRight" ? 1 : options.length - 1)) % options.length];
            if (next) {
              onChange(next.value);
              const buttons = e.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button");
              buttons?.[options.indexOf(next)]?.focus();
            }
          }}
        >
          {opt.icon}
          {opt.label}
        </button>
      ))}
    </div>
  );
}
