import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { HelpCircle, Trash2, TriangleAlert } from "lucide-react";
import { Modal } from "./Modal";
import { useT } from "../i18n";

export interface ConfirmOptions {
  title: string;
  message?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: "accent" | "danger" | "warning";
  icon?: ReactNode;
}

type ConfirmFn = (options: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmFn | null>(null);

/** Provides `useConfirm()`: promise-based, styled replacement for window.confirm. */
export function ConfirmProvider({ children }: { children: ReactNode }) {
  const { t } = useT();
  const [current, setCurrent] = useState<ConfirmOptions | null>(null);
  const resolver = useRef<((value: boolean) => void) | null>(null);

  const confirm = useCallback<ConfirmFn>((options) => {
    resolver.current?.(false);
    setCurrent(options);
    return new Promise<boolean>((resolve) => {
      resolver.current = resolve;
    });
  }, []);

  const finish = (value: boolean) => {
    resolver.current?.(value);
    resolver.current = null;
    setCurrent(null);
  };

  const tone = current?.tone ?? "accent";
  const defaultIcon = tone === "danger" ? <Trash2 /> : tone === "warning" ? <TriangleAlert /> : <HelpCircle />;

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      {current && (
        <Modal
          title={current.title}
          icon={current.icon ?? defaultIcon}
          tone={tone}
          onClose={() => finish(false)}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => finish(false)}>
                {current.cancelLabel ?? t("common.cancel")}
              </button>
              <button
                type="button"
                className={`btn ${tone === "danger" ? "btn-danger" : "btn-primary"}`}
                onClick={() => finish(true)}
                data-autofocus
              >
                {current.confirmLabel ?? t("common.confirm")}
              </button>
            </>
          }
        >
          {current.message && <div>{current.message}</div>}
        </Modal>
      )}
    </ConfirmContext.Provider>
  );
}

export function useConfirm(): ConfirmFn {
  const ctx = useContext(ConfirmContext);
  if (!ctx) throw new Error("useConfirm must be used inside <ConfirmProvider>");
  return ctx;
}
