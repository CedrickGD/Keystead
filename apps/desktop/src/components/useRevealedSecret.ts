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
  /** `reveal_secret` is running. */
  loading: boolean;
  /** Shows the value, or hides it again. */
  toggle: () => void;
}

/**
 * An eye toggle for one secret of an item (`reveal_secret`). The value lives
 * only in this component's state – never in the item list or another store –
 * and is dropped again after 30 s, when the item or the field changes (also a
 * new `revision`, e.g. the item was saved), when the window loses focus or is
 * hidden, on lock and on unmount.
 */
export function useRevealedSecret(itemId: string, field: SecretField, revision: number): RevealedSecret {
  const { errorText } = useT();
  const toast = useToast();
  const [value, setValue] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // Answers of requests started before a hide are ignored.
  const generation = useRef(0);
  const timer = useRef(0);
  // `{ custom: 1 }` is a new object on every render: compare by value.
  const fieldKey = JSON.stringify(field);
  const fieldRef = useRef(field);
  fieldRef.current = field;

  const hide = useCallback(() => {
    generation.current += 1;
    window.clearTimeout(timer.current);
    setValue(null);
    setLoading(false);
  }, []);

  // A different item, field or version of the item: hide, also on unmount.
  useEffect(() => hide, [itemId, fieldKey, revision, hide]);

  const shown = value !== null;
  useEffect(() => {
    if (!shown) return undefined;
    const onVisibility = () => {
      if (document.visibilityState === "hidden") hide();
    };
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", onVisibility);
    const unsubscribe = subscribeEffect(events.onVaultLocked(hide));
    return () => {
      window.removeEventListener("blur", hide);
      document.removeEventListener("visibilitychange", onVisibility);
      unsubscribe();
    };
  }, [shown, hide]);

  const reveal = useCallback(async () => {
    const mine = ++generation.current;
    setLoading(true);
    try {
      const secret = await api.revealSecret(itemId, fieldRef.current);
      if (mine !== generation.current) return;
      setValue(secret);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(hide, REVEAL_MS);
    } catch (err) {
      if (mine === generation.current) toast.error(errorText(err));
    } finally {
      if (mine === generation.current) setLoading(false);
    }
  }, [itemId, hide, errorText, toast]);

  const toggle = useCallback(() => {
    if (shown) hide();
    else void reveal();
  }, [shown, hide, reveal]);

  return { value, loading, toggle };
}
