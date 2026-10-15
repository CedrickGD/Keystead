import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useT } from "../i18n";
import { takeCarriedNotices, type CarriedNotice } from "../lib/discard";

export type ToastKind = "success" | "error" | "info";

export interface ToastOptions {
  message: string;
  kind?: ToastKind;
  action?: { label: string; onClick: () => void };
  /** Milliseconds; defaults to 3.5 s (6 s with an action, 6 s for errors). */
  duration?: number;
  /**
   * Shown again if the page reloads to discard vault data while this toast
   * is visible (see lib/discard.ts). Never set it for messages that contain
   * vault data such as item names.
   */
  carry?: boolean;
}

interface ToastEntry extends ToastOptions {
  id: number;
}

interface ToastApi {
  show: (options: ToastOptions) => number;
  dismiss: (id: number) => void;
  success: (message: string) => number;
  error: (message: string) => number;
  info: (message: string) => number;
  /** The visible toasts marked `carry`. */
  carried: () => CarriedNotice[];
}

const ToastContext = createContext<ToastApi | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const { t } = useT();
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const nextId = useRef(1);
  const timers = useRef(new Map<number, number>());
  const carriedRef = useRef(new Map<number, CarriedNotice>());

  const dismiss = useCallback((id: number) => {
    setToasts((list) => list.filter((toast) => toast.id !== id));
    const timer = timers.current.get(id);
    if (timer) window.clearTimeout(timer);
    timers.current.delete(id);
    carriedRef.current.delete(id);
  }, []);

  const show = useCallback(
    (options: ToastOptions) => {
      const id = nextId.current++;
      const duration = options.duration ?? (options.action || options.kind === "error" ? 6000 : 3500);
      // Keep at most three toasts; replace an identical message instead of stacking it.
      setToasts((list) => [...list.filter((toast) => toast.message !== options.message).slice(-2), { ...options, id }]);
      timers.current.set(id, window.setTimeout(() => dismiss(id), duration));
      if (options.carry) carriedRef.current.set(id, { kind: options.kind ?? "success", message: options.message });
      return id;
    },
    [dismiss],
  );

  // Messages that were visible when the page reloaded after a lock.
  useEffect(() => {
    for (const notice of takeCarriedNotices()) show(notice);
  }, [show]);

  const api = useMemo<ToastApi>(
    () => ({
      show,
      dismiss,
      success: (message) => show({ message, kind: "success" }),
      error: (message) => show({ message, kind: "error" }),
      info: (message) => show({ message, kind: "info" }),
      carried: () => [...carriedRef.current.values()],
    }),
    [show, dismiss],
  );

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div className="toast-region" role="status" aria-live="polite">
        {toasts.map((toast) => {
          const kind = toast.kind ?? "success";
          const Icon = kind === "error" ? CircleAlert : kind === "info" ? Info : CircleCheck;
          return (
            <div key={toast.id} className={`toast ${kind}`}>
              <Icon aria-hidden />
              <span className="toast-message">{toast.message}</span>
              {toast.action && (
                <button
                  type="button"
                  className="toast-action"
                  onClick={() => {
                    toast.action?.onClick();
                    dismiss(toast.id);
                  }}
                >
                  {toast.action.label}
                </button>
              )}
              <button type="button" className="icon-btn" onClick={() => dismiss(toast.id)} aria-label={t("common.close")}>
                <X size={15} />
              </button>
            </div>
          );
        })}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastApi {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used inside <ToastProvider>");
  return ctx;
}
