import { createContext, useContext } from "react";
import { Modal } from "../Modal";
import { ImportFlow, type ImportRequest } from "./ImportFlow";

/** The import dialog as a modal (main window, settings, start panel, drag & drop). */
export function ImportDialog({ request, onClose }: { request: ImportRequest | null; onClose: () => void }) {
  return (
    <ImportFlow variant="dialog" request={request} onFinish={onClose}>
      {(layout) => (
        <Modal
          title={layout.title}
          subtitle={layout.subtitle}
          icon={layout.icon}
          onClose={onClose}
          dismissable={layout.dismissable}
          onSubmit={layout.onSubmit}
          className={`import-dialog step-${layout.step}`}
          footer={layout.footer}
        >
          {layout.body}
        </Modal>
      )}
    </ImportFlow>
  );
}

/**
 * Opens the import dialog of the main window, optionally with a file to
 * analyse right away (`others`: further files dropped with it, ignored).
 */
export type OpenImport = (path?: string, others?: number) => void;

export const ImportLauncherContext = createContext<OpenImport | null>(null);

/** `openImport()` of the main window (null outside of it). */
export function useOpenImport(): OpenImport | null {
  return useContext(ImportLauncherContext);
}
