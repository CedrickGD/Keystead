// A tiny DOM for the `node --test` suite of lib/forms.js (CI runs the tests
// without installing anything, so there is no jsdom). It implements just what
// forms.js uses: elements with attributes, a small HTML parser (incl.
// declarative shadow roots: <template shadowrootmode="open|closed">), simple
// selectors (tag, #id, .class, [attr], [attr="value"], comma lists), tree
// order, open/closed shadow roots, labels, the input value setter and fake
// layout (every rendered element is 200×30 px; `data-rect="x,y,w,h"`
// overrides it; `hidden` and inline display/visibility/opacity hide it).
//
//   const { document, window } = createDocument("<form>…</form>", { title, path });
//   installGlobals();   // HTMLInputElement, Event, … (needed before loading forms.js)

const VOID = new Set(["input", "br", "img", "meta", "link", "hr", "area", "base", "col", "embed", "source", "track", "wbr"]);
const INPUT_TYPES = new Set([
  "text", "password", "email", "search", "tel", "url", "number", "submit", "button", "image", "hidden",
  "checkbox", "radio", "date", "file", "reset", "range", "color", "month", "week", "time", "datetime-local",
]);

let creationCounter = 0;

class Node {
  constructor(doc) {
    this.ownerDocument = doc;
    this.parentNode = null;
    this.childNodes = [];
  }

  get parentElement() {
    return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null;
  }

  get firstChild() {
    return this.childNodes[0] ?? null;
  }

  get children() {
    return this.childNodes.filter((n) => n.nodeType === 1);
  }

  get textContent() {
    return this.childNodes.map((n) => n.textContent).join("");
  }

  set textContent(value) {
    this.childNodes = [];
    if (value) this.appendChild(new Text(this.ownerDocument, String(value)));
  }

  appendChild(child) {
    if (child.parentNode) child.parentNode.removeChild(child);
    child.parentNode = this;
    this.childNodes.push(child);
    return child;
  }

  append(...children) {
    for (const child of children) this.appendChild(typeof child === "string" ? new Text(this.ownerDocument, child) : child);
  }

  removeChild(child) {
    this.childNodes = this.childNodes.filter((n) => n !== child);
    child.parentNode = null;
    return child;
  }

  remove() {
    this.parentNode?.removeChild(this);
  }

  /** The root of this node's tree: a Document, a ShadowRoot or a detached top node. */
  getRootNode() {
    let node = this;
    while (node.parentNode) node = node.parentNode;
    return node;
  }

  get isConnected() {
    let root = this.getRootNode();
    while (root.nodeType === 11 && root.host) root = root.host.getRootNode();
    return root.nodeType === 9;
  }

  contains(other) {
    for (let n = other; n; n = n.parentNode) if (n === this) return true;
    return false;
  }

  /** Tree order within one tree (shadow roots are separate trees, like in browsers). */
  compareDocumentPosition(other) {
    if (other === this) return 0;
    if (this.getRootNode() !== other.getRootNode()) return 1 | 32; // DISCONNECTED | IMPLEMENTATION_SPECIFIC
    const order = [];
    const walk = (n) => {
      order.push(n);
      n.childNodes.forEach(walk);
    };
    walk(this.getRootNode());
    return order.indexOf(other) > order.indexOf(this) ? 4 : 2;
  }

  /** Elements of this tree (not crossing into shadow roots), in tree order. */
  descendants() {
    const out = [];
    const walk = (n) => {
      for (const c of n.childNodes) {
        if (c.nodeType === 1) out.push(c);
        walk(c);
      }
    };
    walk(this);
    return out;
  }

  querySelectorAll(selector) {
    const match = compile(selector);
    return this.descendants().filter(match);
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  getElementById(id) {
    return this.descendants().find((e) => e.id === id) ?? null;
  }
}

class Text extends Node {
  constructor(doc, data) {
    super(doc);
    this.nodeType = 3;
    this.data = data;
  }

  get textContent() {
    return this.data;
  }
}

class ShadowRoot extends Node {
  constructor(host, mode) {
    super(host.ownerDocument);
    this.nodeType = 11;
    this.host = host;
    this.mode = mode;
  }
}

class Element extends Node {
  constructor(doc, localName) {
    super(doc);
    this.nodeType = 1;
    this.localName = localName;
    this.attributes = new Map();
    this.events = [];
    this.order = creationCounter++;
    this._shadow = null;
  }

  get tagName() {
    return this.localName.toUpperCase();
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  hasAttribute(name) {
    return this.attributes.has(name);
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  get id() {
    return this.getAttribute("id") || "";
  }

  get className() {
    return this.getAttribute("class") || "";
  }

  get shadowRoot() {
    return this._shadow && this._shadow.mode === "open" ? this._shadow : null;
  }

  attachShadow({ mode }) {
    this._shadow = new ShadowRoot(this, mode);
    return this._shadow;
  }

  matches(selector) {
    return compile(selector)(this);
  }

  closest(selector) {
    const match = compile(selector);
    for (let n = this; n && n.nodeType === 1; n = n.parentNode) if (match(n)) return n;
    return null;
  }

  /** The parent in the composed tree (for visibility checks). */
  composedParentNode() {
    if (this.parentNode && this.parentNode.nodeType === 11) return this.parentNode.host;
    return this.parentElement;
  }

  checkVisibility() {
    for (let n = this; n; n = n.composedParentNode()) {
      if (n.hasAttribute("hidden")) return false;
      const style = (n.getAttribute("style") || "").replace(/\s+/g, "").toLowerCase();
      if (/display:none|visibility:hidden|opacity:0(;|$)/.test(style)) return false;
    }
    return true;
  }

  getBoundingClientRect() {
    const custom = this.getAttribute("data-rect");
    let [x, y, width, height] = custom ? custom.split(",").map(Number) : [16, 16 + (this.order % 2000) * 40, 200, 30];
    if (!this.isConnected || !this.checkVisibility()) [x, y, width, height] = [0, 0, 0, 0];
    return { left: x, top: y, right: x + width, bottom: y + height, x, y, width, height };
  }

  focus() {
    this.ownerDocument.activeElement = this;
  }

  dispatchEvent(event) {
    this.events.push(event);
    return true;
  }

  get disabled() {
    return this.hasAttribute("disabled");
  }

  get readOnly() {
    return this.hasAttribute("readonly");
  }

  get scrollWidth() {
    return 1280;
  }

  get scrollHeight() {
    return 100_000;
  }
}

class HTMLInputElement extends Element {
  constructor(doc) {
    super(doc, "input");
    this._value = null;
  }

  get type() {
    const type = (this.getAttribute("type") || "text").toLowerCase();
    return INPUT_TYPES.has(type) ? type : "text";
  }

  set type(value) {
    this.setAttribute("type", value);
  }

  get name() {
    return this.getAttribute("name") || "";
  }

  get value() {
    return this._value ?? this.getAttribute("value") ?? "";
  }

  set value(value) {
    this._value = String(value);
  }

  /** Form owner: the nearest <form> ancestor in the same tree (forms do not reach into shadow roots). */
  get form() {
    for (let n = this.parentNode; n && n.nodeType === 1; n = n.parentNode) if (n.localName === "form") return n;
    return null;
  }

  get labels() {
    const out = [];
    const root = this.getRootNode();
    if (this.id) for (const label of root.querySelectorAll("label")) if (label.getAttribute("for") === this.id) out.push(label);
    for (let n = this.parentNode; n && n.nodeType === 1; n = n.parentNode) {
      if (n.localName === "label" && !n.hasAttribute("for")) out.push(n);
    }
    return out;
  }
}

class HTMLTextAreaElement extends Element {
  constructor(doc) {
    super(doc, "textarea");
    this._value = "";
  }

  get value() {
    return this._value;
  }

  set value(value) {
    this._value = String(value);
  }
}

class HTMLButtonElement extends Element {
  constructor(doc) {
    super(doc, "button");
  }

  get type() {
    const type = (this.getAttribute("type") || "submit").toLowerCase();
    return type === "button" || type === "reset" ? type : "submit";
  }

  get value() {
    return this.getAttribute("value") || "";
  }
}

class Document extends Node {
  constructor({ title = "", path = "/" } = {}) {
    super(null);
    this.ownerDocument = null;
    this.nodeType = 9;
    this.title = title;
    this.location = { pathname: path, href: `https://example.test${path}` };
    this.activeElement = null;
    this.defaultView = {
      innerWidth: 1280,
      innerHeight: 900,
      scrollX: 0,
      scrollY: 0,
      getComputedStyle: () => ({ display: "block", visibility: "visible", opacity: "1" }),
    };
  }

  createElement(name) {
    const tag = name.toLowerCase();
    if (tag === "input") return new HTMLInputElement(this);
    if (tag === "textarea") return new HTMLTextAreaElement(this);
    if (tag === "button") return new HTMLButtonElement(this);
    return new Element(this, tag);
  }

  get documentElement() {
    return this.children[0] ?? null;
  }

  get body() {
    return this.documentElement?.children.find((e) => e.localName === "body") ?? null;
  }
}
Object.defineProperty(Document.prototype, "isConnected", { get: () => true });

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

const cache = new Map();

function splitList(selector) {
  const parts = [];
  let depth = 0;
  let quote = null;
  let current = "";
  for (const ch of selector) {
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") quote = ch;
    else if (ch === "[") depth += 1;
    else if (ch === "]") depth -= 1;
    else if (ch === "," && depth === 0) {
      parts.push(current.trim());
      current = "";
      continue;
    }
    current += ch;
  }
  parts.push(current.trim());
  return parts.filter(Boolean);
}

function compileCompound(text) {
  const m = /^([a-z*][\w-]*)?((?:\[[^\]]+\]|#[\w-]+|\.[\w-]+)*)$/i.exec(text);
  if (!m) throw new Error(`mini-dom: unsupported selector "${text}"`);
  const tag = m[1] && m[1] !== "*" ? m[1].toLowerCase() : null;
  const tests = [];
  for (const [, attr, id, cls] of m[2].matchAll(/\[([^\]]+)\]|#([\w-]+)|\.([\w-]+)/g)) {
    if (id) tests.push((e) => e.id === id);
    else if (cls) tests.push((e) => e.className.split(/\s+/).includes(cls));
    else {
      const a = /^([\w-]+)\s*(?:=\s*(?:"([^"]*)"|'([^']*)'|([^\]]*)))?$/.exec(attr.trim());
      if (!a) throw new Error(`mini-dom: unsupported attribute selector "[${attr}]"`);
      const [, name, dq, sq, bare] = a;
      const value = dq ?? sq ?? bare;
      tests.push(value === undefined ? (e) => e.hasAttribute(name) : (e) => e.getAttribute(name) === value);
    }
  }
  return (e) => e.nodeType === 1 && (!tag || e.localName === tag) && tests.every((t) => t(e));
}

function compile(selector) {
  let fn = cache.get(selector);
  if (!fn) {
    const parts = splitList(selector).map(compileCompound);
    fn = (e) => parts.some((p) => p(e));
    cache.set(selector, fn);
  }
  return fn;
}

// ---------------------------------------------------------------------------
// HTML parser (well-formed test markup only)
// ---------------------------------------------------------------------------

function decode(text) {
  return text.replace(/&quot;/g, '"').replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
}

/** Parses `html` and appends the nodes to `parent`. */
export function appendHTML(parent, html) {
  const doc = parent.nodeType === 9 ? parent : parent.ownerDocument;
  const stack = [parent];
  const re = /<!--[\s\S]*?-->|<\/([a-zA-Z][\w-]*)\s*>|<([a-zA-Z][\w-]*)((?:\s+[^\s=>"'/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>"']+))?)*)\s*(\/?)>|([^<]+)/g;
  for (const m of html.matchAll(re)) {
    const top = stack[stack.length - 1];
    if (m[0].startsWith("<!--")) continue;
    if (m[1]) {
      const name = m[1].toLowerCase();
      // Closing a <template shadowrootmode>: back to the host's parent level.
      while (stack.length > 1) {
        const node = stack.pop();
        if (node.localName === name || (name === "template" && node.nodeType === 11)) break;
      }
      continue;
    }
    if (m[2]) {
      const name = m[2].toLowerCase();
      const attrs = new Map();
      for (const a of m[3].matchAll(/([^\s=>"']+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>"']+)))?/g)) {
        attrs.set(a[1].toLowerCase(), decode(a[2] ?? a[3] ?? a[4] ?? ""));
      }
      if (name === "template" && attrs.has("shadowrootmode") && top.nodeType === 1) {
        stack.push(top.attachShadow({ mode: attrs.get("shadowrootmode") }));
        continue;
      }
      const el = doc.createElement(name);
      for (const [k, v] of attrs) el.setAttribute(k, v);
      top.appendChild(el);
      if (!VOID.has(name) && !m[4]) stack.push(el);
      continue;
    }
    if (m[5] && m[5].trim()) top.appendChild(new Text(doc, decode(m[5])));
  }
}

/** A document `<html><head><title/></head><body>…html…</body></html>`. */
export function createDocument(html = "", { title = "Test", path = "/" } = {}) {
  const document = new Document({ title, path });
  const root = document.createElement("html");
  document.appendChild(root);
  root.appendChild(document.createElement("head"));
  const body = document.createElement("body");
  root.appendChild(body);
  appendHTML(body, html);
  return { document, window: document.defaultView };
}

class Event {
  constructor(type, init = {}) {
    this.type = type;
    this.bubbles = !!init.bubbles;
    this.composed = !!init.composed;
    Object.assign(this, init);
  }
}
class InputEvent extends Event {}
class KeyboardEvent extends Event {}

/** Globals forms.js needs when it is loaded (classic script, see loadForms in the test). */
export function installGlobals() {
  Object.assign(globalThis, { HTMLInputElement, HTMLTextAreaElement, Event, InputEvent, KeyboardEvent });
}
