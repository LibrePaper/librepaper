// The typesetter, driven with a stand-in document: which spans it takes,
// what it asks KaTeX for, and how it fetches KaTeX once and only when needed.
import assert from "node:assert/strict";
import {
  applyNumbering,
  createKatexCache,
  createMathTypesetter,
  displaySources,
  numberDocument,
  prepareQuartoMath,
  typeset,
  typesetDetached,
  untypeset,
} from "../../src/lib/math.js";
import { numberDisplays, rewriteDisplay } from "../../src/lib/math-numbering.js";

function span(style, tex, rendered = false) {
  const attributes = {};
  const node = {
    dataset: { mathStyle: style },
    textContent: tex,
    firstElementChild: rendered ? { className: "katex" } : null,
    attributes,
    setAttribute(name, value) { attributes[name] = value; },
    getAttribute(name) { return attributes[name] ?? null; },
    appendChild(child) { this.firstElementChild = child; },
  };
  return node;
}

function fakeDocument(spans) {
  const head = { children: [], appendChild(node) { this.children.push(node); return node; } };
  const body = { querySelectorAll: (selector) => selector === "span[data-math-style]" ? spans : [] };
  return { head, body, createElement: (tag) => span(tag, "") };
}

// Quarto's inert MathJax input is stripped of its delimiters and handed to
// the same safe KaTeX path as browser-rendered Markdown equations.
{
  const quarto = [
    { dataset: {}, textContent: "\\(x^2\\)", firstElementChild: null, classList: { contains: (name) => name === "inline" } },
    { dataset: {}, textContent: "\n\\[ SE = \\frac{s}{\\sqrt{n}}. \\]\n", firstElementChild: null, classList: { contains: (name) => name === "display" } },
  ];
  const root = { querySelectorAll: (selector) => selector.startsWith("span.math") ? quarto : [] };
  assert.equal(prepareQuartoMath(root), 2);
  assert.equal(quarto[0].dataset.mathStyle, "inline");
  assert.equal(quarto[0].textContent, "x^2");
  assert.equal(quarto[1].dataset.mathStyle, "display");
  assert.equal(quarto[1].textContent, "SE = \\frac{s}{\\sqrt{n}}.");
}

// Only the renderer's spans, and only the ones still holding TeX.
{
  const spans = [span("inline", "x^2"), span("display", "\\sum", true), span("inline", "y")];
  const found = untypeset(fakeDocument(spans).body);
  assert.deepEqual(found.map((s) => s.textContent), ["x^2", "y"]);
}

// Display math is told it is display math, and a bad formula is not fatal.
{
  const calls = [];
  const katex = { render: (tex, node, options) => calls.push({ tex, node, options }) };
  const spans = [span("inline", "a &lt; b"), span("display", "\\frac{1}{2}")];
  typeset(katex, spans);
  assert.equal(calls.length, 2);
  assert.equal(calls[0].node, spans[0]);
  assert.equal(calls[0].options.displayMode, false);
  assert.equal(calls[1].options.displayMode, true);
  assert.equal(calls[1].options.throwOnError, false);
  assert.equal(calls[1].options.output, "html");
}

// A document without math fetches nothing. One with math fetches KaTeX from
// its own origin, once, and typesets when it arrives; the next repaint is
// typeset on the spot.
{
  const window = {};
  const document = fakeDocument([]);
  const typesetMath = createMathTypesetter({ base: "/assets/katex-1.0.0/", document, window });
  assert.equal(typesetMath(), false);
  assert.equal(document.head.children.length, 0, "nothing loaded for a document without math");

  const spans = [span("inline", "x")];
  document.body.querySelectorAll = (selector) => selector === "span[data-math-style]" ? spans : [];
  assert.equal(typesetMath(), false, "not yet typeset: KaTeX has to arrive first");
  const [link, script] = document.head.children;
  assert.equal(link.rel, "stylesheet");
  assert.equal(link.href, "/assets/katex-1.0.0/katex.min.css");
  assert.equal(script.src, "/assets/katex-1.0.0/katex.min.js");
  assert.equal(typesetMath(), false, "a second call while loading");
  assert.equal(document.head.children.length, 2, "asked for once");

  const calls = [];
  window.katex = { render: (tex, node) => { calls.push(tex); node.firstElementChild = { className: "katex" }; } };
  script.onload();
  await new Promise((done) => setTimeout(done, 0));
  assert.deepEqual(calls, ["x"], "typeset once the script arrived");

  spans.push(span("display", "y"));
  assert.equal(typesetMath(), true, "typeset before returning now that KaTeX is here");
  assert.deepEqual(calls, ["x", "y"], "the span already rendered was left alone");
}

// A load that fails is forgotten, so the next document with math tries again.
{
  const window = {};
  const document = fakeDocument([span("inline", "x")]);
  const typesetMath = createMathTypesetter({ base: "/assets/katex-1.0.0/", document, window });
  typesetMath();
  document.head.children[1].onerror();
  await new Promise((done) => setTimeout(done, 0));
  typesetMath();
  assert.equal(document.head.children.length, 3, "the script asked for again after a failure, the stylesheet not");
}

// Equation numbering: stand-in document with spans, links and ids.
function numbered(spans, links = [], taken = {}) {
  const body = {
    querySelectorAll: (selector) =>
      selector === "span[data-math-style]" ? spans : selector === "a[href]" ? links : [],
  };
  const document = { body, getElementById: (id) => taken[id] ?? null, createElement: (tag) => span(tag, "") };
  const rendered = [];
  const katex = {
    render: (tex, node) => {
      rendered.push(tex);
      node.firstElementChild = { className: "katex" };
    },
  };
  const run = () => typeset(katex, untypeset(body), numberDocument(body, document));
  return { run, rendered, document };
}

{
  const eq = span("display", "\\begin{equation}\nSE = x\n\\label{eq:se}\n\\end{equation}");
  const eq2 = span("display", "\\begin{equation}\ny\n\\end{equation}");
  const star = span("display", "\\begin{equation*}\nz\n\\end{equation*}");
  const align = span("display", "\\begin{align}\nx &= 1 \\label{eq:x}\\\\\ny &= 2 \\nonumber\\\\\nw &= 3\n\\end{align}");
  const multline = span("display", "\\begin{multline}\na \\\\ b\n\\end{multline}");
  const tagged = span("display", "\\begin{equation}\nq \\tag{A}\n\\end{equation}");
  const ref = span("inline", "\\ref{eq:se}");
  const eqref = span("inline", "\\eqref{eq:x}");
  const unknown = span("inline", "\\ref{nope}");
  const link = { attributes: { href: "eq:se" }, textContent: "eq:se", getAttribute(n) { return this.attributes[n]; }, setAttribute(n, v) { this.attributes[n] = v; } };
  const spans = [ref, eq, eq2, star, align, multline, tagged, eqref, unknown];
  const t = numbered(spans, [link]);
  t.run();
  assert.equal(t.rendered.length, 9 - 2, "the two linked references are not sent to KaTeX");
  assert.ok(t.rendered[0].includes("SE = x") && t.rendered[0].includes("\\tag{1}"));
  assert.ok(!t.rendered.join("").includes("\\label"));
  assert.ok(t.rendered[1].includes("\\tag{2}"), "second equation is number 2");
  assert.ok(!t.rendered[2].includes("\\tag"), "equation* is not numbered");
  assert.ok(t.rendered[3].startsWith("\\begin{align*}"));
  assert.equal([...t.rendered[3].matchAll(/\\tag\{(\d+)\}/g)].map((m) => m[1]).join(), "3,4", "nonumber row skipped");
  assert.ok(!t.rendered[3].includes("nonumber"));
  assert.ok(t.rendered[4].startsWith("\\begin{gather*}") && t.rendered[4].includes("\\tag{5}"), "multline rewritten");
  assert.ok(t.rendered[5].includes("\\tag{A}") && !t.rendered[5].includes("\\tag{6}"), "own tag kept, not counted");
  assert.equal(ref.firstElementChild.textContent, "1");
  assert.equal(ref.firstElementChild.attributes.href, "#eq:se");
  assert.equal(eqref.firstElementChild.textContent, "(3)");
  assert.equal(t.rendered[6], "\\ref{nope}", "unknown key left for KaTeX");
  assert.equal(eq.attributes.id, "eq:se");
  assert.equal(align.attributes.id, "eq:x");
  assert.equal(link.attributes.href, "#eq:se");
  assert.equal(link.textContent, "1");

  // A second pass renumbers nothing and renders nothing again.
  const before = t.rendered.length;
  t.run();
  assert.equal(t.rendered.length, before);
  assert.equal(ref.firstElementChild.textContent, "1");
  const again = numberDocument({ querySelectorAll: (q) => q === "span[data-math-style]" ? spans : [] }, t.document);
  assert.equal(again.labels.get("eq:x"), 3);
  assert.equal(again.labels.get("eq:se"), 1);
}

// An id already taken elsewhere is left alone.
{
  const eq = span("display", "\\begin{equation}a\\label{k}\\end{equation}");
  numbered([eq], [], { k: {} }).run();
  assert.equal(eq.attributes.id, undefined);
}

// A claim with no number leaves the formula untagged, and its labels still go.
{
  const equation = rewriteDisplay("\\begin{equation}\ny \\label{eq:none}\n\\end{equation}", () => null);
  assert.ok(!equation.tex.includes("\\tag"), "no \\tag without a number");
  assert.ok(!equation.tex.includes("\\label"));
  assert.deepEqual(equation.keys, ["eq:none"]);

  const aligned = rewriteDisplay("\\begin{align}\na &= 1 \\label{r1}\\\\\nb &= 2\n\\end{align}", () => null);
  assert.ok(aligned.tex.startsWith("\\begin{align*}"));
  assert.ok(!aligned.tex.includes("\\tag") && !aligned.tex.includes("\\label"));

  const withNumber = rewriteDisplay("\\begin{equation}\ny \\label{eq:some}\n\\end{equation}", () => 4);
  assert.ok(withNumber.tex.includes("\\tag{4}"));
}

// applyNumbering takes numbers computed for a root's own spans and applies them.
{
  const source = "\\begin{equation}\ny \\label{eq:y}\n\\end{equation}";
  const eq = span("display", source);
  const link = { attributes: { href: "eq:y" }, textContent: "eq:y", getAttribute(n) { return this.attributes[n]; }, setAttribute(n, v) { this.attributes[n] = v; } };
  const body = {
    querySelectorAll: (selector) =>
      selector === "span[data-math-style]" ? [eq] : selector === "a[href]" ? [link] : [],
  };
  const document = { body, getElementById: () => null };
  assert.deepEqual(displaySources(body), [source]);

  const numbering = numberDisplays(displaySources(body));
  const applied = applyNumbering(body, document, numbering);
  assert.equal(applied.tex.get(eq), numbering.tex[0]);
  assert.ok(applied.tex.get(eq).includes("\\tag{1}"));
  assert.equal(applied.labels.get("eq:y"), 1);
  assert.equal(applied.document, document);
  assert.equal(eq.attributes.id, "eq:y");
  assert.equal(link.attributes.href, "#eq:y");
  assert.equal(link.textContent, "1");

  // The numbers given are the ones used, whatever this root alone would count to.
  const given = new Map([["eq:y", 7]]);
  const fixed = applyNumbering(body, document, { tex: ["T"], keys: [["eq:y"]], labels: given });
  assert.equal(fixed.tex.get(eq), "T");
  assert.equal(fixed.labels, given);

  // Without ids, as the paper blocks ask, no span takes the key as its id.
  const bare = span("display", source);
  const root = { querySelectorAll: (selector) => (selector === "span[data-math-style]" ? [bare] : []) };
  applyNumbering(root, document, numbering, { ids: false });
  assert.equal(bare.attributes.id, undefined);
}

// A comment after a label does not swallow the \tag; an escaped \% stays.
{
  const commented = rewriteDisplay("\\begin{equation}\nx = 1 \\label{eq:x} % why\n\\end{equation}", () => 3);
  assert.ok(!commented.tex.includes("%"));
  assert.match(commented.tex, /\\tag\{3\}/);
  const percent = rewriteDisplay("\\begin{equation}\n50\\% \\label{eq:p}\n\\end{equation}", () => 1);
  assert.ok(percent.tex.includes("50\\%"));
  const verbatim = rewriteDisplay("\\begin{equation}\n\\verb|50%| \\label{eq:v}\n\\end{equation}", () => 2);
  assert.ok(verbatim.tex.includes("\\verb|50%|"));
}

// The cache renders each (TeX, display) pair once, keeps display and inline
// apart, and drops the least recently used entry past its limit.
{
  let calls = 0;
  const katex = { renderToString: (tex, options) => { calls += 1; return `<${options.displayMode ? "D" : "I"}>${tex}</>`; } };
  const cache = createKatexCache(2);
  assert.equal(cache.html(katex, "a", false), "<I>a</>");
  assert.equal(cache.html(katex, "a", false), "<I>a</>");
  assert.equal(calls, 1, "identical request answered from the cache");
  assert.equal(cache.html(katex, "a", true), "<D>a</>");
  assert.equal(calls, 2, "display and inline cached apart");

  // Recency: "a" inline was just used, so "D a" is the oldest when "b" arrives.
  cache.html(katex, "a", false);
  cache.html(katex, "b", false);
  assert.equal(calls, 3);
  cache.html(katex, "a", false);
  assert.equal(calls, 3, "the recently used entry survives");
  cache.html(katex, "a", true);
  assert.equal(calls, 4, "the oldest entry was evicted");
}

// typeset with a cache sets the cached HTML, and the same data attribute.
{
  let calls = 0;
  const katex = { renderToString: (tex) => { calls += 1; return `<k>${tex}</k>`; }, render: () => { throw new Error("render used with a cache"); } };
  const cache = createKatexCache();
  const spans = [span("inline", "x^2"), span("display", "\\frac{1}{2}")];
  typeset(katex, spans, undefined, cache);
  assert.equal(spans[0].innerHTML, "<k>x^2</k>");
  assert.equal(spans[0].getAttribute("data-equation-source"), "x^2");
  assert.equal(spans[1].innerHTML, "<k>\\frac{1}{2}</k>");
  assert.equal(spans[1].getAttribute("data-equation-source"), "\\frac{1}{2}");
  typeset(katex, [span("inline", "x^2")], undefined, cache);
  assert.equal(calls, 2, "the repeated formula came from the cache");
}

// A detached document is typeset in full, and reports nothing to do once done.
{
  const spans = [span("inline", "x^2"), span("display", "\\frac{1}{2}")];
  const root = { querySelectorAll: (selector) => (selector === "span[data-math-style]" ? spans : []) };
  const document = { getElementById: () => null, createElement: (tag) => span(tag, "") };
  const katex = { renderToString: (tex) => `<k>${tex}</k>` };
  const cache = createKatexCache();
  assert.equal(typesetDetached(katex, root, document, cache), true);
  assert.equal(spans[0].innerHTML, "<k>x^2</k>");
  assert.equal(spans[1].innerHTML, "<k>\\frac{1}{2}</k>");
  spans.forEach((s) => { s.firstElementChild = { className: "katex" }; });
  assert.equal(typesetDetached(katex, root, document, cache), false);
}

console.log("math: spans, KaTeX options, one lazy load, retry after failure, cache and detached typesetting passed");
