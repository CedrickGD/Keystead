import { useEffect, useRef, type ReactNode, type RefObject } from "react";
import { useEscapeLayer, useOutsideClick } from "../lib/layers";

/**
 * Dropdown menu anchored to its (relatively positioned) parent. Arrow keys
 * move between items, Esc / outside click close it.
 */
export function Menu({
  onClose,
  anchorRef,
  align = "left",
  children,
  label,
  width,
}: {
  onClose: () => void;
  anchorRef: RefObject<HTMLElement | null>;
  align?: "left" | "right";
  children: ReactNode;
  label: string;
  width?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEscapeLayer(() => {
    onClose();
    anchorRef.current?.focus();
  });
  useOutsideClick([ref, anchorRef], onClose);

  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("[role=menuitem], [role=menuitemradio]")?.focus();
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End", "Tab"].includes(e.key)) return;
    if (e.key === "Tab") {
      onClose();
      return;
    }
    e.preventDefault();
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem], [role=menuitemradio]") ?? []);
    if (items.length === 0) return;
    const idx = items.indexOf(document.activeElement as HTMLElement);
    let next = 0;
    if (e.key === "ArrowDown") next = idx < 0 ? 0 : (idx + 1) % items.length;
    if (e.key === "ArrowUp") next = idx < 0 ? items.length - 1 : (idx - 1 + items.length) % items.length;
    if (e.key === "End") next = items.length - 1;
    items[next]?.focus();
  };

  return (
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      onKeyDown={onKeyDown}
      style={{
        top: "calc(100% + 6px)",
        [align === "left" ? "left" : "right"]: 0,
        width,
      }}
    >
      {children}
    </div>
  );
}

export function MenuItem({
  icon,
  children,
  onSelect,
  danger,
  checked,
  hint,
}: {
  icon?: ReactNode;
  children: ReactNode;
  onSelect: () => void;
  danger?: boolean;
  /** Renders as a radio item with a check mark when true. */
  checked?: boolean;
  hint?: ReactNode;
}) {
  const radio = checked !== undefined;
  return (
    <button
      type="button"
      role={radio ? "menuitemradio" : "menuitem"}
      aria-checked={radio ? checked : undefined}
      className={`menu-item ${danger ? "danger" : ""}`}
      tabIndex={-1}
      onClick={onSelect}
    >
      {icon}
      <span style={{ flex: 1 }}>{children}</span>
      {hint}
      {radio && checked && (
        <svg className="check" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <path d="M20 6 9 17l-5-5" />
        </svg>
      )}
    </button>
  );
}
