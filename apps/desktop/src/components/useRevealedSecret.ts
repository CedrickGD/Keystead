import { useCallback, useEffect, useRef, useState } from "react";
import { api, events, subscribeEffect } from "../lib/api";
import type { SecretField } from "../lib/types";
import { useT } from "../i18n";
import { useToast } from "./Toasts";

/** A revealed secret is hidden again after this long. */
export const REVEAL_MS = 30_000;

export interface RevealedSecret {
  /** The value while it is shown, otherwise null. */
  value: string | null;
  /** The reveal command is running. */
  loading: boolean;
  /** Shows the value, or hides it again. */
  toggle: () => void;
}

/**
 * The reveal pattern of the detail pane, shared by secrets and attachment
 * previews: `fetch` runs on demand, its answer lives only in this component's
 * state – never in the item list or another store – and is dropped again
 * after 30 s, when `key` changes (another item, field or version of the
 * item), when the window loses focus or is hidden (on the phone also on the
 * backend's `app://suspended`), on lock and on unmount.
 */
export function useRevealedValue(key: string, fetch: () => Promise<string>): RevealedSecret {
  const { errorText } = useT();
  const toast = useToast();
  const [value, setValue] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // Answers of requests started before a hide are ignored.
  const generation = useRef(0);
  const timer = useRef(0);
  // `fetch` is a new closure on every render: always call the latest one.
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;

  const hide = useCallback(() => {
    generation.current += 1;
    window.clearTimeout(timer.current);
    setValue(null);
    setLoading(false);
  }, []);

  // Another key (item, field, version): hide, also on unmount.
  useEffect(() => hide, [key, hide]);

  const shown = value !== null;
  // Also while the reveal is running: its answer must not show up after the
  // window was left in the meantime.
  const active = shown || loading;
  useEffect(() => {
    if (!active) return undefined;
    const onVisibility = () => {
      if (document.visibilityState === "hidden") hide();
    };
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", onVisibility);
    const unsubscribeLocked = subscribeEffect(events.onVaultLocked(hide));
    const unsubscribeSuspended = subscribeEffect(events.onSuspended(hide));
    return () => {
      window.removeEventListener("blur", hide);
      document.removeEventListener("visibilitychange", onVisibility);
      unsubscribeLocked();
      unsubscribeSuspended();
    };
  }, [active, hide]);

  const reveal = useCallback(async () => {
    const mine = ++generation.current;
    setLoading(true);
    try {
      const secret = await fetchRef.current();
      if (mine !== generation.current) return;
      setValue(secret);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(hide, REVEAL_MS);
    } catch (err) {
      if (mine === generation.current) toast.error(errorText(err));
    } finally {
      if (mine === generation.current) setLoading(false);
    }
  }, [hide, errorText, toast]);

  const toggle = useCallback(() => {
    if (shown) hide();
    else void reveal();
  }, [shown, hide, reveal]);

  return { value, loading, toggle };
}

/**
 * An eye toggle for one secret of an item (`reveal_secret`), see
 * `useRevealedValue`: hidden again after 30 s, when the item or the field
 * changes (also a new `revision`, e.g. the item was saved), when the window
 * loses focus or is hidden, on lock and on unmount.
 */
export function useRevealedSecret(itemId: string, field: SecretField, revision: number): RevealedSecret {
  // `{ custom: 1 }` is a new object on every render: compare by value.
  const fieldKey = JSON.stringify(field);
  return useRevealedValue(`${itemId}\n${fieldKey}\n${revision}`, () => api.revealSecret(itemId, field));
}

/**
 * An eye toggle for one generator history entry (`reveal_generated`), see
 * `useRevealedValue`: the history itself reaches the page redacted (ids and
 * times); the password is hidden again after 30 s, when the window loses
 * focus or is hidden, on lock and on unmount (e.g. the history is cleared).
 */
export function useRevealedGenerated(id: string): RevealedSecret {
  return useRevealedValue(`generated\n${id}`, () => api.revealGenerated(id));
}
