// Locking wipes the vault in the backend, but the items, generator history
// and health report the UI fetched stay in this renderer's JS heap (and the
// IPC buffers) until garbage collection. Reloading the page drops that heap.
// Hygiene, not a guarantee: it makes the data unreachable, it does not
// overwrite it.

import { IN_TAURI, pendingCalls } from "./api";

const NOTICES_KEY = "keystead.carriedNotices";
const POLL_MS = 50;
const MAX_WAIT_MS = 3000;

/** A message to show again after the reload (never one with vault data). */
export interface CarriedNotice {
  kind: "success" | "error" | "info";
  message: string;
}

let reloading = false;

/**
 * Reloads the page to drop the vault data the UI held. Waits (at most 3 s)
 * for commands still in flight so their outcome is not lost; `notices` is
 * read right before the reload and shown again afterwards. Returns whether
 * a reload is coming (also if one was already scheduled).
 *
 * Outside Tauri (mock backend) this does nothing and returns false: the mock
 * keeps its demo data in this same page and would only start over.
 */
export function discardPageData(notices: () => CarriedNotice[]): boolean {
  if (!IN_TAURI) return false;
  if (reloading) return true;
  reloading = true;
  const deadline = Date.now() + MAX_WAIT_MS;
  let quietPolls = 0;
  const poll = () => {
    quietPolls = pendingCalls() === 0 ? quietPolls + 1 : 0;
    if (quietPolls < 2 && Date.now() < deadline) {
      window.setTimeout(poll, POLL_MS);
      return;
    }
    try {
      window.sessionStorage.setItem(NOTICES_KEY, JSON.stringify(notices()));
    } catch {
      /* storage unavailable: the notices are lost, the reload matters more */
    }
    window.location.reload();
  };
  window.setTimeout(poll, POLL_MS);
  return true;
}

function isNotice(value: unknown): value is CarriedNotice {
  if (!value || typeof value !== "object") return false;
  const { kind, message } = value as Record<string, unknown>;
  return (kind === "success" || kind === "error" || kind === "info") && typeof message === "string";
}

/** The notices saved by `discardPageData` (once). */
export function takeCarriedNotices(): CarriedNotice[] {
  try {
    const raw = window.sessionStorage.getItem(NOTICES_KEY);
    window.sessionStorage.removeItem(NOTICES_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isNotice) : [];
  } catch {
    return [];
  }
}
