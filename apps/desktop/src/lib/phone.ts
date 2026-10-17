// The phone layout on the React/DOM side: whether it applies (the decision
// itself is in layout.mjs) and the small behaviours it needs. App.tsx sets
// `data-layout="phone"` on <html>; styles/phone.css keys on it.

import { useEffect, useState } from "react";
import { PHONE_MEDIA_QUERY, isPhoneLayout } from "./layout.mjs";

export { isPhoneLayout, isPhonePlatform, phoneBackStep } from "./layout.mjs";

/** `isNarrowViewport(window.innerWidth)`, as a media query (it only reports crossing the breakpoint). */
function narrowViewport(): boolean {
  return typeof window !== "undefined" && (window.matchMedia?.(PHONE_MEDIA_QUERY).matches ?? false);
}

/** Phone layout for this platform (`AppInfo.platform`, unknown before the boot) – follows the viewport width. */
export function usePhoneLayout(platform: string | undefined): boolean {
  const [narrow, setNarrow] = useState(narrowViewport);
  useEffect(() => {
    const media = window.matchMedia?.(PHONE_MEDIA_QUERY);
    if (!media) return;
    const update = () => setNarrow(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  return isPhoneLayout(platform, narrow);
}

/** The phone layout is on (for code outside React's tree of the app, e.g. dialogs). */
export function phoneLayoutActive(): boolean {
  return typeof document !== "undefined" && document.documentElement.dataset.layout === "phone";
}

/**
 * Phone: when the viewport shrinks (the on-screen keyboard opens – the
 * Android shell shrinks the webview by the keyboard's height), the focused
 * field is scrolled back into view.
 */
export function useKeepFocusedFieldVisible(active: boolean): void {
  useEffect(() => {
    if (!active) return;
    let frame = 0;
    const onResize = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const el = document.activeElement;
        if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) {
          el.scrollIntoView({ block: "nearest" });
        }
      });
    };
    window.addEventListener("resize", onResize);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
    };
  }, [active]);
}
