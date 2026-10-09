// Small stroke icons (24×24 grid) and the VaultX mark, built as DOM nodes.

const SVG_NS = "http://www.w3.org/2000/svg";

const ICONS = {
  lock: [
    ["rect", { x: 5, y: 11, width: 14, height: 10, rx: 2 }],
    ["path", { d: "M8 11V7.5a4 4 0 0 1 8 0V11" }],
  ],
  external: [["path", { d: "M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" }]],
  user: [
    ["circle", { cx: 12, cy: 8, r: 4 }],
    ["path", { d: "M4.5 20.5a7.5 7.5 0 0 1 15 0" }],
  ],
  key: [
    ["circle", { cx: 7.5, cy: 15.5, r: 4 }],
    ["path", { d: "M10.4 12.6 20 3M16.5 6.5l3 3M14 9l2 2" }],
  ],
  clock: [
    ["circle", { cx: 12, cy: 12, r: 8.5 }],
    ["path", { d: "M12 7.5V12l3 2" }],
  ],
  copy: [
    ["rect", { x: 9, y: 9, width: 11, height: 11, rx: 2 }],
    ["path", { d: "M5.5 15H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v.5" }],
  ],
  search: [
    ["circle", { cx: 11, cy: 11, r: 6.5 }],
    ["path", { d: "m20 20-4.2-4.2" }],
  ],
  refresh: [["path", { d: "M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4h-4" }]],
  eye: [
    ["path", { d: "M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" }],
    ["circle", { cx: 12, cy: 12, r: 3 }],
  ],
  eyeOff: [
    ["path", { d: "m3 3 18 18M10.6 5.6c.5-.1.9-.1 1.4-.1 6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.9 3.7M6.6 6.6C4 8.3 2.5 12 2.5 12S6 18.5 12 18.5c1.7 0 3.2-.5 4.5-1.2M9.9 9.9a3 3 0 0 0 4.2 4.2" }],
  ],
  plus: [["path", { d: "M12 5v14M5 12h14" }]],
  check: [["path", { d: "m5 12.5 4.5 4.5L19 7.5" }]],
  close: [["path", { d: "M6 6l12 12M18 6 6 18" }]],
  back: [["path", { d: "M19 12H5M11 18l-6-6 6-6" }]],
  plug: [["path", { d: "M9 3v5M15 3v5M6.5 8h11v3a5.5 5.5 0 0 1-11 0V8zM12 16.5V21" }]],
  alert: [["path", { d: "M12 3.5 21.5 20h-19L12 3.5zM12 10v4.5M12 17.2v.1" }]],
  globe: [
    ["circle", { cx: 12, cy: 12, r: 8.5 }],
    ["path", { d: "M3.5 12h17M12 3.5c2.3 2.4 3.5 5.2 3.5 8.5s-1.2 6.1-3.5 8.5c-2.3-2.4-3.5-5.2-3.5-8.5s1.2-6.1 3.5-8.5z" }],
  ],
  shield: [["path", { d: "M12 3.5 19 6v5.5c0 4.4-2.9 7.9-7 9.5-4.1-1.6-7-5.1-7-9.5V6l7-2.5z" }]],
  wand: [["path", { d: "m4 20 10-10M14 4v3M12.5 5.5h3M19 9v3M17.5 10.5h3M18 3.5l.01.01" }]],
};

function svgElement(tag, attrs) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, String(value));
  return node;
}

/** A stroke icon by name (see ICONS). */
export function icon(name, className = "ico") {
  const svg = svgElement("svg", {
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    "stroke-width": 1.8,
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
    class: className,
  });
  for (const [tag, attrs] of ICONS[name] || []) svg.append(svgElement(tag, attrs));
  return svg;
}

/** The VaultX mark (shield with keyhole on a rounded tile), as in the desktop app. */
export function logo(size = 32) {
  const svg = svgElement("svg", { viewBox: "0 0 32 32", width: size, height: size, class: "logo", "aria-hidden": "true" });
  svg.append(
    svgElement("rect", { class: "logo-tile", width: 32, height: 32, rx: 8 }),
    svgElement("path", { class: "logo-shield", d: "M16 5.75 24 8.6v6.35c0 5.1-3.4 9.2-8 10.95-4.6-1.75-8-5.85-8-10.95V8.6z" }),
    svgElement("circle", { class: "logo-hole", cx: 16, cy: 14, r: 2.4 }),
    svgElement("path", { class: "logo-hole", d: "M14.85 15.3h2.3l.65 4.7h-3.6z" }),
  );
  return svg;
}
