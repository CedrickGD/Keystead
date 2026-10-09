import { createContext, useContext, useEffect, useMemo, useRef, useState, type MutableRefObject, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { FileDown, LockKeyhole } from "lucide-react";
import { events, subscribeEffect } from "../../lib/api";
import { useT } from "../../i18n";
import { useToast } from "../Toasts";

/**
 * Receives the path of a file dropped onto the window. Returns false if it
 * cannot take a file right now (e.g. an import is being committed).
 */
export type FileDropHandler = (path: string) => boolean | void;

interface FileDropApi {
  register: (handler: MutableRefObject<FileDropHandler>) => () => void;
}

const FileDropContext = createContext<FileDropApi | null>(null);

/** The file name of a path (Windows or Unix separators). */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/**
 * Listens for files dragged onto the window (Tauri webview drag & drop) and
 * hands a dropped file to the most recently registered target
 * ({@link useFileDropTarget}): the import dialog if it is open, else the main
 * screen (which opens it) or the setup wizard's import step. While a file is
 * dragged over the window a full-window overlay explains what will happen –
 * without a target (vault locked, welcome screen) that importing needs an
 * unlocked vault first.
 */
export function FileDropProvider({ noTarget, children }: { noTarget: "unlock" | "create"; children: ReactNode }) {
  const { t } = useT();
  const toast = useToast();
  const targets = useRef<{ id: number; handler: MutableRefObject<FileDropHandler> }[]>([]);
  const nextId = useRef(1);
  const [dragging, setDragging] = useState<{ withTarget: boolean } | null>(null);

  const api = useMemo<FileDropApi>(
    () => ({
      register: (handler) => {
        const id = nextId.current++;
        targets.current.push({ id, handler });
        return () => {
          targets.current = targets.current.filter((target) => target.id !== id);
        };
      },
    }),
    [],
  );

  const noTargetText = noTarget === "create" ? t("drop.noVaultTitle") : t("drop.lockedTitle");
  const drop = useRef<(paths: string[]) => void>(() => undefined);
  drop.current = (paths: string[]) => {
    const first = paths[0];
    if (!first) return;
    const target = targets.current[targets.current.length - 1];
    if (!target) {
      toast.info(noTargetText);
      return;
    }
    if (paths.length > 1) toast.info(t("drop.onlyOne", { name: baseName(first) }));
    if (target.handler.current(first) === false) toast.info(t("drop.busy"));
  };

  useEffect(
    () =>
      subscribeEffect(
        events.onFileDrag((event) => {
          if (event.type === "enter") {
            // Dragged text or links carry no paths: no overlay.
            if (event.paths.length > 0) setDragging({ withTarget: targets.current.length > 0 });
          } else if (event.type === "leave") {
            setDragging(null);
          } else if (event.type === "drop") {
            setDragging(null);
            drop.current(event.paths);
          }
        }),
      ),
    [],
  );

  return (
    <FileDropContext.Provider value={api}>
      {children}
      {dragging && <DropOverlay withTarget={dragging.withTarget} noTarget={noTarget} />}
    </FileDropContext.Provider>
  );
}

function DropOverlay({ withTarget, noTarget }: { withTarget: boolean; noTarget: "unlock" | "create" }) {
  const { t } = useT();
  return createPortal(
    <div className={`drop-overlay ${withTarget ? "" : "blocked"}`} aria-live="polite">
      <div className="drop-frame">
        <div className="drop-card">
          <span className="drop-icon" aria-hidden>
            {withTarget ? <FileDown /> : <LockKeyhole />}
          </span>
          {withTarget ? (
            <>
              <div className="drop-title">{t("drop.title")}</div>
              <div className="drop-formats">{t("drop.formats")}</div>
              <div className="drop-hint">{t("importFlow.dropHint")}</div>
            </>
          ) : (
            <>
              <div className="drop-title">{noTarget === "create" ? t("drop.noVaultTitle") : t("drop.lockedTitle")}</div>
              <div className="drop-hint">{noTarget === "create" ? t("drop.noVaultHint") : t("drop.lockedHint")}</div>
            </>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}

/**
 * Makes the calling component the receiver of dropped files while it is
 * mounted (and `enabled`). The newest receiver wins.
 */
export function useFileDropTarget(handler: FileDropHandler, enabled = true): void {
  const ctx = useContext(FileDropContext);
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!ctx || !enabled) return;
    return ctx.register(ref);
  }, [ctx, enabled]);
}
