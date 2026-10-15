import { useEffect, useId, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { useT } from "../i18n";
import { useEscapeLayer } from "../lib/layers";

// Open modals, newest last – only the top-most one traps Tab.
const openModals: string[] = [];

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface ModalProps {
  title: ReactNode;
  subtitle?: ReactNode;
  icon?: ReactNode;
  tone?: "accent" | "danger" | "warning";
  onClose: () => void;
  children?: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  /** When false, Esc / backdrop clicks / the X button do not close the dialog. */
  dismissable?: boolean;
  /** Wraps the content in a <form>; Enter in an input submits. */
  onSubmit?: () => void;
  className?: string;
}

export function Modal({
  title,
  subtitle,
  icon,
  tone = "accent",
  onClose,
  children,
  footer,
  wide,
  dismissable = true,
  onSubmit,
  className,
}: ModalProps) {
  const { t } = useT();
  const id = useId();
  const dialogRef = useRef<HTMLDivElement>(null);

  // Esc closes (when allowed); registering as a modal layer also disables
  // the global keyboard shortcuts of the main window while open.
  useEscapeLayer(
    () => {
      if (dismissable) onClose();
    },
    true,
    true,
  );

  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;
    openModals.push(id);

    const dialog = dialogRef.current;
    if (dialog) {
      const preferred = dialog.querySelector<HTMLElement>("[data-autofocus]");
      const firstInput = dialog.querySelector<HTMLElement>(
        "input:not([disabled]):not([type=checkbox]), textarea:not([disabled])",
      );
      const fallback = dialog.querySelector<HTMLElement>(".modal-footer button:not([disabled])");
      (preferred ?? firstInput ?? fallback ?? dialog).focus();
    }

    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || openModals[openModals.length - 1] !== id || !dialogRef.current) return;
      const focusables = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        (el) => el.offsetParent !== null,
      );
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (!first || !last) return;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey, true);

    return () => {
      window.removeEventListener("keydown", onKey, true);
      const idx = openModals.indexOf(id);
      if (idx >= 0) openModals.splice(idx, 1);
      if (previouslyFocused && document.contains(previouslyFocused)) previouslyFocused.focus();
    };
  }, [id]);

  const titleId = `${id}-title`;
  const content = (
    <>
      <div className="modal-header">
        {icon && <div className={`modal-icon ${tone === "accent" ? "" : tone}`}>{icon}</div>}
        <div style={{ minWidth: 0, flex: 1, paddingTop: icon ? 2 : 0 }}>
          <h2 className="modal-title" id={titleId}>
            {title}
          </h2>
          {subtitle && <p className="modal-subtitle">{subtitle}</p>}
        </div>
        {dismissable && (
          <button type="button" className="icon-btn sm modal-close" onClick={onClose} aria-label={t("common.close")}>
            <X />
          </button>
        )}
      </div>
      {children && <div className="modal-body">{children}</div>}
      {footer && <div className="modal-footer">{footer}</div>}
    </>
  );

  return createPortal(
    <div
      className="modal-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && dismissable) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={`modal ${wide ? "wide" : ""} ${className ?? ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        {onSubmit ? (
          <form
            className="modal-form"
            noValidate
            onSubmit={(e) => {
              e.preventDefault();
              onSubmit();
            }}
          >
            {content}
          </form>
        ) : (
          content
        )}
      </div>
    </div>,
    document.body,
  );
}
