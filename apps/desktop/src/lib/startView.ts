// A settings section the main window opens with once, e.g. the setup
// wizard's "Anleitung" (browser extension) → Settings → Browser-Integration
// after the new vault is shown.

let pendingSection: string | null = null;

/** The next main window starts on Settings → `section`. */
export function startOnSettings(section: string): void {
  pendingSection = section;
}

/** The section requested by `startOnSettings` (not consumed: see `clearStartSection`). */
export function peekStartSection(): string | null {
  return pendingSection;
}

export function clearStartSection(): void {
  pendingSection = null;
}
