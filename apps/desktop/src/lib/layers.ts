// Tiny "layer" stack for Escape handling: the most recently opened layer
// (modal, popover, menu) receives Escape; nothing beneath it does. Global
// shortcuts check `hasModalLayer()` so they stay inactive while a dialog is open.
//
// The phone's back gesture works like Escape: the Android shell dispatches
// the cancelable DOM event `keystead:back` on `window` (MainActivity.kt) and
// moves the app to the background unless a listener calls
// `preventDefault()`. The top layer gets it first (closes the dialog, menu
// or drawer), then the back handlers (`useBackHandler`, newest first) –
// e.g. the main screen: detail → list → all items. Nothing left: Android's.

import { useEffect, useRef, type RefObject } from "react";

interface Layer {
  id: number;
  onEscape: () => void;
  modal: boolean;
}

const layers: Layer[] = [];
let nextId = 1;
let installed = false;

/** Answers whether it handled the back gesture. */
type BackHandler = () => boolean;
const backHandlers: { id: number; handler: BackHandler }[] = [];
let backInstalled = false;

/** The event the Android shell dispatches for the back gesture. */
export const BACK_EVENT = "keystead:back";

function installBack(): void {
  if (backInstalled) return;
  backInstalled = true;
  window.addEventListener(BACK_EVENT, (e) => {
    const top = layers[layers.length - 1];
    if (top) {
      // Consumed even when the layer stays open (a dialog that cannot be dismissed).
      e.preventDefault();
      top.onEscape();
      return;
    }
    for (let i = backHandlers.length - 1; i >= 0; i--) {
      if (backHandlers[i]?.handler()) {
        e.preventDefault();
        return;
      }
    }
  });
}

function install(): void {
  if (installed) return;
  installed = true;
  window.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Escape") return;
      const top = layers[layers.length - 1];
      if (!top) return;
      e.preventDefault();
      e.stopPropagation();
      top.onEscape();
    },
    true,
  );
}

export function pushLayer(onEscape: () => void, modal = false): () => void {
  install();
  installBack();
  const layer: Layer = { id: nextId++, onEscape, modal };
  layers.push(layer);
  return () => {
    const idx = layers.findIndex((l) => l.id === layer.id);
    if (idx >= 0) layers.splice(idx, 1);
  };
}

export function hasModalLayer(): boolean {
  return layers.some((l) => l.modal);
}

/** Registers an Escape handler while `active` is true. */
export function useEscapeLayer(onEscape: () => void, active = true, modal = false): void {
  const handler = useRef(onEscape);
  handler.current = onEscape;
  useEffect(() => {
    if (!active) return;
    return pushLayer(() => handler.current(), modal);
  }, [active, modal]);
}

/**
 * Registers a handler for the back gesture (`keystead:back`) while `active`
 * is true; it runs when no layer is open and returns whether it did
 * something (otherwise the gesture goes on to older handlers, then Android).
 */
export function useBackHandler(handler: BackHandler, active = true): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!active) return;
    installBack();
    const entry = { id: nextId++, handler: () => ref.current() };
    backHandlers.push(entry);
    return () => {
      const idx = backHandlers.findIndex((h) => h.id === entry.id);
      if (idx >= 0) backHandlers.splice(idx, 1);
    };
  }, [active]);
}

/** Calls `onOutside` for pointer presses outside every given element. */
export function useOutsideClick(
  refs: RefObject<HTMLElement | null>[],
  onOutside: () => void,
  active = true,
): void {
  const handler = useRef(onOutside);
  handler.current = onOutside;
  const refsRef = useRef(refs);
  refsRef.current = refs;
  useEffect(() => {
    if (!active) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node | null;
      if (!target) return;
      if (refsRef.current.some((r) => r.current?.contains(target))) return;
      handler.current();
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [active]);
}
