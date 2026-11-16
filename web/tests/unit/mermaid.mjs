// Mermaid diagrams in the preview, driven with a small stand-in document: the
// loader, which diagrams are found, captions, the cache, and the renderer.
import assert from "node:assert/strict";
import {
  createDiagramCache,
  createDiagramRenderer,
  createMermaidLoader,
  diagramSource,
  figureCaption,
  findDiagrams,
} from "../../src/lib/mermaid.js";

// A tiny element tree: enough for tags, classes, text, children and the
// selectors the module uses (a tag with an optional class, or a list of them).
function matches(node, selector) {
  const m = /^([a-z]+)?(?:\.([\w-]+))?$/.exec(selector.trim());
  if (!m) return false;
  if (m[1] && node.localName !== m[1]) return false;
  if (m[2] && !node.className.split(/\s+/).includes(m[2])) return false;
  return true;
}

function descendants(node) {
  return node.children.flatMap((child) => [child, ...descendants(child)]);
}

function fakeDocument() {
  const strays = {};
  const doc = {
    strays,
    createElement: (tag) => element(tag, {}, doc),
    getElementById: (id) => strays[id] ?? null,
  };
  doc.body = element("body", {}, doc);
  return doc;
}

function element(tag, { cls = "", text = "", children = [] } = {}, ownerDocument) {
  const node = {
    tagName: tag.toUpperCase(),
    localName: tag,
    className: cls,
    textContent: text,
    innerHTML: "",
    ownerDocument,
    parentElement: null,
    isConnected: true,
    children: [],
    attributes: {},
    setAttribute(name, value) {
      this.attributes[name] = String(value);
      if (name === "class") this.className = String(value);
    },
    getAttribute(name) { return this.attributes[name] ?? null; },
    appendChild(child) {
      child.parentElement = this;
      this.children.push(child);
      return child;
    },
    querySelectorAll(selector) {
      return descendants(this).filter((n) => selector.split(",").some((s) => matches(n, s)));
    },
    querySelector(selector) {
      return this.querySelectorAll(selector)[0] ?? null;
    },
    replaceWith(next) {
      const parent = this.parentElement;
      parent.children[parent.children.indexOf(this)] = next;
      next.parentElement = parent;
      this.parentElement = null;
      this.isConnected = false;
    },
    remove() {
      this.removed = true;
      this.isConnected = false;
    },
  };
  for (const child of children) node.appendChild(child);
  return node;
}

// The loader requests its script once, resolves window.mermaid, and retries
// after a failure.
{
  const head = { children: [], appendChild(node) { this.children.push(node); return node; } };
  const window = {};
  const document = { head, createElement: (tag) => ({ tagName: tag }) };
  const load = createMermaidLoader({ base: "/assets/mermaid-11.0.0/", document, window });
  const first = load();
  assert.equal(load(), first, "the same promise while loading");
  assert.equal(head.children.length, 1, "asked for once");
  const [script] = head.children;
  assert.equal(script.src, "/assets/mermaid-11.0.0/mermaid.min.js");
  const mermaid = { name: "fake" };
  window.mermaid = mermaid;
  script.onload();
  assert.equal(await first, mermaid);
  assert.equal(await load(), mermaid, "resolved at once once loaded");
  assert.equal(head.children.length, 1, "no second script");
}

{
  const head = { children: [], appendChild(node) { this.children.push(node); return node; } };
  const window = {};
  const document = { head, createElement: (tag) => ({ tagName: tag }) };
  const load = createMermaidLoader({ base: "/m/", document, window });
  const failed = load();
  head.children[0].onerror();
  await assert.rejects(failed, /did not load/);
  load();
  assert.equal(head.children.length, 2, "the script asked for again after a failure");
}

// Both shapes are found in document order, a stray code listing is not, and
// a diagram met twice is listed once.
{
  const doc = fakeDocument();
  const pre1 = element("pre", { cls: "mermaid mermaid-js", text: "graph TD" });
  const pre2 = element("pre", {
    children: [element("code", { cls: "language-mermaid", text: "graph LR" })],
  });
  const pre3 = element("pre", {
    children: [element("code", { cls: "language-r", text: "x <- 1" })],
  });
  const div = element("div", {
    children: [element("code", { cls: "language-mermaid", text: "stray" })],
  });
  const pre4 = element("pre", {
    cls: "mermaid",
    children: [element("code", { cls: "language-mermaid", text: "twice" })],
  });
  const pre5 = element("pre", {
    children: [element("code", { cls: "language-mermaid", text: "sequenceDiagram" })],
  });
  for (const node of [pre1, pre2, pre3, div, pre4, pre5]) doc.body.appendChild(node);
  assert.deepEqual(findDiagrams(doc.body), [pre1, pre2, pre4, pre5]);
}

// The source is the code listing's text, or else the pre's own, trimmed at
// the end only.
{
  const listed = element("pre", {
    children: [element("code", { cls: "language-mermaid", text: "  graph TD\n  A-->B\n\n" })],
  });
  assert.equal(diagramSource(listed), "  graph TD\n  A-->B");
  const bare = element("pre", { cls: "mermaid", text: "pie\n  \"x\" : 1\n" });
  assert.equal(diagramSource(bare), "pie\n  \"x\" : 1");
}

// The caption is the fig-cap option, quotes removed, or empty.
{
  assert.equal(figureCaption('graph TD\n%%| fig-cap: "Flow of data"\nA-->B'), "Flow of data");
  assert.equal(figureCaption("%%| fig-cap: 'Single quoted'"), "Single quoted");
  assert.equal(figureCaption("%%| fig-cap: Plain words  "), "Plain words");
  assert.equal(figureCaption("%%| label: fig-x\ngraph TD"), "");
  assert.equal(figureCaption("graph TD"), "");
}

// The cache keeps the most recently used entries and drops the oldest.
{
  const cache = createDiagramCache(2);
  cache.set("a", "<svg>a</svg>");
  cache.set("b", "<svg>b</svg>");
  assert.equal(cache.get("a"), "<svg>a</svg>", "a read makes an entry recent");
  cache.set("c", "<svg>c</svg>");
  assert.equal(cache.get("b"), undefined, "the oldest entry was evicted");
  assert.equal(cache.get("a"), "<svg>a</svg>");
  assert.equal(cache.get("c"), "<svg>c</svg>");
}

// A cached diagram is swapped in at once, with its caption as text.
{
  const doc = fakeDocument();
  const source = "graph TD\n%%| fig-cap: \"Flow\"\nA-->B";
  const pre = element("pre", { children: [element("code", { cls: "language-mermaid", text: `${source}\n` })] });
  doc.body.appendChild(pre);
  const cache = createDiagramCache();
  cache.set(`default\u0000${source}`, "<svg>one</svg>");
  const renderer = createDiagramRenderer({
    document: doc,
    window: {},
    load: () => Promise.reject(new Error("not needed")),
    cache,
    theme: () => "default",
  });
  assert.equal(renderer.applyCached(doc.body), 0);
  const [figure] = doc.body.children;
  assert.equal(figure.localName, "figure");
  assert.equal(figure.getAttribute("class"), "lp-mermaid");
  assert.equal(figure.getAttribute("data-lp-mermaid"), "");
  assert.equal(figure.innerHTML, "<svg>one</svg>");
  assert.equal(figure.ownerDocument, doc);
  const figcaption = figure.children[0];
  assert.equal(figcaption.localName, "figcaption");
  assert.equal(figcaption.textContent, "Flow");
  assert.equal(pre.isConnected, false, "the listing was replaced");
}

// render() draws a valid diagram, caches it and initializes mermaid with the
// current theme; an invalid one is left as it is.
{
  const doc = fakeDocument();
  const good = element("pre", { children: [element("code", { cls: "language-mermaid", text: "graph TD\nA-->B" })] });
  const bad = element("pre", { children: [element("code", { cls: "language-mermaid", text: "graph TD\nBROKEN" })] });
  doc.body.appendChild(good);
  doc.body.appendChild(bad);

  const init = [];
  const rendered = [];
  const mermaid = {
    initialize: (options) => init.push(options),
    parse: async (source) => !source.includes("BROKEN"),
    render: async (id, source) => {
      rendered.push({ id, source });
      return { svg: `<svg data-id="${id}"></svg>` };
    },
  };
  const window = { mermaid };
  const cache = createDiagramCache();
  let theme = "dark";
  const renderer = createDiagramRenderer({
    document: doc,
    window,
    load: async () => mermaid,
    cache,
    theme: () => theme,
  });

  assert.equal(renderer.render(), false, "drawn in the background");
  await new Promise((done) => setTimeout(done, 0));

  assert.deepEqual(init, [{ startOnLoad: false, securityLevel: "strict", theme: "dark" }]);
  assert.deepEqual(rendered, [{ id: "lp-mermaid-0", source: "graph TD\nA-->B" }], "only the valid one drawn");
  assert.equal(good.isConnected, false);
  assert.equal(doc.body.children[0].localName, "figure");
  assert.equal(doc.body.children[0].innerHTML, '<svg data-id="lp-mermaid-0"></svg>');
  assert.equal(doc.body.children[1], bad, "the invalid listing is left alone");
  assert.equal(cache.get("dark\u0000graph TD\nA-->B"), '<svg data-id="lp-mermaid-0"></svg>');

  // The next repaint shows the valid one at once; the invalid one is still undrawn.
  assert.equal(renderer.applyCached(doc.body), 1);
  assert.equal(renderer.render(), false);
  await new Promise((done) => setTimeout(done, 0));
  assert.equal(rendered.length, 1, "the invalid one was parsed again but not rendered");
  assert.equal(init.length, 1, "initialized once for this theme");
}

// Nothing undrawn: no load, and render() reports done.
{
  const doc = fakeDocument();
  let loads = 0;
  const renderer = createDiagramRenderer({
    document: doc,
    window: {},
    load: () => { loads += 1; return Promise.resolve(null); },
    cache: createDiagramCache(),
    theme: () => "default",
  });
  assert.equal(renderer.render(), true);
  assert.equal(loads, 0);
}

// A failed render removes the scratch elements mermaid left behind.
{
  const doc = fakeDocument();
  const pre = element("pre", { children: [element("code", { cls: "language-mermaid", text: "graph TD\nCRASH" })] });
  doc.body.appendChild(pre);
  const stray = { removed: false, remove() { this.removed = true; } };
  doc.strays["dlp-mermaid-0"] = stray;
  const mermaid = {
    initialize: () => {},
    parse: async () => true,
    render: async () => { throw new Error("boom"); },
  };
  const renderer = createDiagramRenderer({
    document: doc,
    window: { mermaid },
    load: async () => mermaid,
    cache: createDiagramCache(),
    theme: () => "default",
  });
  renderer.render();
  await new Promise((done) => setTimeout(done, 0));
  assert.equal(stray.removed, true);
  assert.equal(doc.body.children[0], pre, "the listing stays after a failed render");
}

const settle = () => new Promise((done) => setTimeout(done, 0));

// A render still pending when the same listing is morphed to a newer source:
// the old drawing must not replace the newer listing, and the newer source is
// drawn by a later pass.
{
  const doc = fakeDocument();
  const pre = element("pre", { children: [element("code", { cls: "language-mermaid", text: "graph TD\nOLD" })] });
  doc.body.appendChild(pre);
  const rendered = [];
  const pending = [];
  const mermaid = {
    initialize: () => {},
    parse: async () => true,
    render: (id, source) => new Promise((resolve) => {
      rendered.push(source);
      pending.push(() => resolve({ svg: `<svg>${source}</svg>` }));
    }),
  };
  const cache = createDiagramCache();
  const renderer = createDiagramRenderer({
    document: doc,
    window: { mermaid },
    load: async () => mermaid,
    cache,
    theme: () => "default",
  });
  renderer.render();
  await settle();
  assert.deepEqual(rendered, ["graph TD\nOLD"]);

  // The same <pre> is morphed to a newer source while the old one is pending.
  pre.children[0].textContent = "graph TD\nNEW";
  pending[0]();
  await settle();
  assert.equal(doc.body.children[0], pre, "the newer listing was not replaced");
  assert.equal(pre.isConnected, true);
  assert.equal(cache.get("default\u0000graph TD\nOLD"), "<svg>graph TD\nOLD</svg>", "the old drawing is still cached");

  // The newer source is drawn: by the pass that followed, or by a later render().
  if (rendered.length === 1) renderer.render();
  await settle();
  assert.deepEqual(rendered, ["graph TD\nOLD", "graph TD\nNEW"]);
  pending[1]();
  await settle();
  assert.equal(doc.body.children[0].localName, "figure");
  assert.equal(doc.body.children[0].innerHTML, "<svg>graph TD\nNEW</svg>");
  assert.equal(doc.body.children[0].getAttribute("data-lp-mermaid-source"), "graph TD\nNEW");
}

// A paper change turns the drawn figures back into listings and draws them in
// the new theme; figures already in the current theme are left alone.
{
  const doc = fakeDocument();
  const pre = element("pre", { children: [element("code", { cls: "language-mermaid", text: "graph TD\nA-->B" })] });
  doc.body.appendChild(pre);
  const init = [];
  const mermaid = {
    initialize: (options) => init.push(options),
    parse: async () => true,
    render: async () => ({ svg: `<svg>${init.at(-1).theme}</svg>` }),
  };
  let theme = "default";
  const renderer = createDiagramRenderer({
    document: doc,
    window: { mermaid },
    load: async () => mermaid,
    cache: createDiagramCache(),
    theme: () => theme,
  });
  renderer.render();
  await settle();
  const [figure] = doc.body.children;
  assert.equal(figure.localName, "figure");
  assert.equal(figure.getAttribute("data-lp-mermaid-source"), "graph TD\nA-->B");
  assert.equal(figure.getAttribute("data-lp-mermaid-theme"), "default");
  assert.equal(figure.innerHTML, "<svg>default</svg>");

  // Same theme: nothing is turned back.
  renderer.retheme();
  await settle();
  assert.equal(doc.body.children[0], figure, "a figure in the current theme is kept");

  theme = "dark";
  assert.equal(renderer.retheme(), false);
  await settle();
  const [redrawn] = doc.body.children;
  assert.notEqual(redrawn, figure, "the figure was replaced");
  assert.equal(figure.isConnected, false);
  assert.equal(redrawn.localName, "figure");
  assert.equal(redrawn.getAttribute("data-lp-mermaid-theme"), "dark");
  assert.equal(redrawn.innerHTML, "<svg>dark</svg>");
  assert.deepEqual(init.map((options) => options.theme), ["default", "dark"]);
}

// A paper change while a drawing is pending: the old-palette drawing is not
// swapped in, and the listing is drawn in the new palette.
{
  const doc = fakeDocument();
  const pre = element("pre", { children: [element("code", { cls: "language-mermaid", text: "graph TD\nA-->B" })] });
  doc.body.appendChild(pre);
  const init = [];
  const pending = [];
  const mermaid = {
    initialize: (options) => init.push(options),
    parse: async () => true,
    render: () => {
      const palette = init.at(-1).theme;
      return new Promise((resolve) => pending.push(() => resolve({ svg: `<svg>${palette}</svg>` })));
    },
  };
  let theme = "default";
  const renderer = createDiagramRenderer({
    document: doc,
    window: { mermaid },
    load: async () => mermaid,
    cache: createDiagramCache(),
    theme: () => theme,
  });
  renderer.render();
  await settle();
  assert.equal(pending.length, 1, "the first drawing is pending");

  theme = "dark";
  renderer.retheme();
  pending[0]();
  await settle();
  assert.equal(doc.body.children[0], pre, "the old-palette drawing was not swapped in");
  assert.equal(pending.length, 2, "a drawing in the new palette started");
  pending[1]();
  await settle();
  const [figure] = doc.body.children;
  assert.equal(figure.localName, "figure");
  assert.equal(figure.getAttribute("data-lp-mermaid-theme"), "dark");
  assert.equal(figure.innerHTML, "<svg>dark</svg>");
}

// A fence changed from mermaid to another language while its drawing is
// pending: the code block is left as it is.
{
  const doc = fakeDocument();
  const code = element("code", { cls: "language-mermaid", text: "graph TD\nA-->B" });
  // In a real page the <pre> holds the code's text too.
  const pre = element("pre", { text: "graph TD\nA-->B", children: [code] });
  doc.body.appendChild(pre);
  const pending = [];
  const mermaid = {
    initialize: () => {},
    parse: async () => true,
    render: () => new Promise((resolve) => pending.push(() => resolve({ svg: "<svg></svg>" }))),
  };
  const renderer = createDiagramRenderer({
    document: doc,
    window: { mermaid },
    load: async () => mermaid,
    cache: createDiagramCache(),
    theme: () => "default",
  });
  renderer.render();
  await settle();
  code.className = "language-text";
  pending[0]();
  await settle();
  assert.equal(doc.body.children[0], pre, "a block that is no longer Mermaid keeps its listing");
}

console.log("mermaid: loader, diagram finding, captions, cache, stale renders and retheme passed");
