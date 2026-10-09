// Tiny "layer" stack for Escape handling: the most recently opened layer
// (modal, popover, menu) receives Escape; nothing beneath it does. Global
// shortcuts check `hasModalLayer()` so they stay inactive while a dialog is open.

import { useEffect, useRef, type RefObject } from "react";

interface Layer {
  id: number;
  onEscape: () => void;
  modal: boolean;
}

const layers: Layer[] = [];
let nextId = 1;
let installed = false;

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

export function hasAnyLayer(): boolean {
  return layers.length > 0;
}

export function isTopLayer(id: number): boolean {
  return layers[layers.length - 1]?.id === id;
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
