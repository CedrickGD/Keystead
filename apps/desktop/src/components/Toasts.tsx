import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";
import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useT } from "../i18n";

export type ToastKind = "success" | "error" | "info";

export interface ToastOptions {
  message: string;
  kind?: ToastKind;
  action?: { label: string; onClick: () => void };
  /** Milliseconds; defaults to 3.5 s (6 s with an action, 6 s for errors). */
  duration?: number;
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
}

const ToastContext = createContext<ToastApi | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const { t } = useT();
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const nextId = useRef(1);
  const timers = useRef(new Map<number, number>());

  const dismiss = useCallback((id: number) => {
    setToasts((list) => list.filter((toast) => toast.id !== id));
    const timer = timers.current.get(id);
    if (timer) window.clearTimeout(timer);
    timers.current.delete(id);
  }, []);

  const show = useCallback(
    (options: ToastOptions) => {
      const id = nextId.current++;
      const duration = options.duration ?? (options.action || options.kind === "error" ? 6000 : 3500);
      // Keep at most three toasts; replace an identical message instead of stacking it.
      setToasts((list) => [...list.filter((toast) => toast.message !== options.message).slice(-2), { ...options, id }]);
      timers.current.set(id, window.setTimeout(() => dismiss(id), duration));
      return id;
    },
    [dismiss],
  );

  const api = useMemo<ToastApi>(
    () => ({
      show,
      dismiss,
      success: (message) => show({ message, kind: "success" }),
      error: (message) => show({ message, kind: "error" }),
      info: (message) => show({ message, kind: "info" }),
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
