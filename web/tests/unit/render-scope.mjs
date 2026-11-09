import assert from "node:assert/strict";
import { renderScope } from "../../src/lib/reader/render-scope.js";

// A render pays for the accepted document, not the whole directory. These
// cases pin the two halves of that: everything a compiler can reach is kept,
// and files the open document never names are not handed over.

const thesis = {
  main: "thesis.tex",
  texts: {
    "thesis.tex": String.raw`\documentclass{article}
\input{chapters/one}
\bibliography{refs}`,
    "chapters/one.tex": String.raw`\includegraphics[width=\linewidth]{figures/plot}`,
    "refs.bib": "@article{one, title={One}}",
    "other.tex": String.raw`\documentclass{article}\input{chapters/two}`,
    "chapters/two.tex": "unrelated",
  },
  digests: {
    "figures/plot.pdf": "a".repeat(64),
    "other/figure.png": "b".repeat(64),
    "thesis.pdf": "c".repeat(64),
  },
};

const thesisScope = renderScope(thesis);
assert.deepEqual(
  Object.keys(thesisScope.texts).sort(),
  ["chapters/one.tex", "refs.bib", "thesis.tex"],
  "a LaTeX render keeps its inputs and bibliography",
);
assert.deepEqual(
  Object.keys(thesisScope.digests),
  ["figures/plot.pdf"],
  "a LaTeX render keeps only the figures its inputs name",
);
assert.deepEqual(
  Object.keys(thesis.texts).sort(),
  ["chapters/one.tex", "chapters/two.tex", "other.tex", "refs.bib", "thesis.tex"],
  "the caller's tree is not changed",
);

const markdown = {
  main: "README.md",
  texts: {
    "README.md": "# Results\n\n![Plot](figures/plot.png)\n",
    "notes/unrelated.md": "A different page.",
    "refs.bib": "@article{one}",
  },
  digests: {
    "figures/plot.png": "d".repeat(64),
    "notes/unrelated.png": "e".repeat(64),
  },
};
const markdownScope = renderScope(markdown);
assert.deepEqual(
  Object.keys(markdownScope.texts).sort(),
  ["README.md", "refs.bib"],
  "a Markdown render keeps its page and the bibliography",
);
assert.deepEqual(
  Object.keys(markdownScope.digests),
  ["figures/plot.png"],
  "a Markdown render keeps the figures its page names",
);

const html = {
  main: "index.html",
  texts: {
    "index.html": '<iframe src="nested.html"></iframe><img src="logo.png">',
    "nested.html": "<p>nested</p>",
    "other.md": "not part of this page",
  },
  digests: {
    "logo.png": "f".repeat(64),
    "other.png": "0".repeat(64),
  },
};
const htmlScope = renderScope(html);
assert.deepEqual(
  Object.keys(htmlScope.texts).sort(),
  ["index.html", "nested.html"],
  "an authored HTML page keeps the frames it can show",
);
assert.deepEqual(Object.keys(htmlScope.digests), ["logo.png"], "only the page's figure is kept");

const empty = { main: "", texts: { "a.md": "a" }, digests: {} };
assert.equal(renderScope(empty), empty, "a tree with no main file is left alone");
