// The typesetter, driven with a stand-in document: which spans it takes,
// what it asks KaTeX for, and how it fetches KaTeX once and only when needed.
import assert from "node:assert/strict";
import { createMathTypesetter, typeset, untypeset } from "../../src/lib/math.js";

function span(style, tex, rendered = false) {
  return {
    dataset: { mathStyle: style },
    textContent: tex,
    firstElementChild: rendered ? { className: "katex" } : null,
  };
}

function fakeDocument(spans) {
  const head = { children: [], appendChild(node) { this.children.push(node); return node; } };
  const body = { querySelectorAll: (selector) => (selector === "span[data-math-style]" ? spans : []) };
  return { head, body, createElement: (tag) => ({ tag }) };
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
  document.body.querySelectorAll = () => spans;
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

console.log("math: spans, KaTeX options, one lazy load, retry after failure passed");
