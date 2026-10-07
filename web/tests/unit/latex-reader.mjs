// Behavioural checks for the reader's LaTeX provenance text -- the pure mapping
// from a `Provenance` to what a reader sees, kept in
// web/src/lib/latex/status-text.js so it can be checked without a Svelte
// runtime and without `latex.js` itself.
import assert from "node:assert/strict";
import { provenanceSentence } from "../../src/lib/latex/status-text.js";

assert.equal(provenanceSentence(null), "");
assert.equal(
  provenanceSentence({ engine: "pdflatex", release: "2026-abc", bibliography: null }),
  "Compiled in the browser with pdfLaTeX (release 2026-abc).",
);
assert.equal(
  provenanceSentence({ engine: "xelatex", release: "2026-abc", bibliography: "browser-biber" }),
  "Compiled in the browser with XeLaTeX (release 2026-abc); bibliography via browser Biber.",
);
assert.equal(
  provenanceSentence({ engine: "lualatex", release: null, bibliography: "bibtex" }),
  "Compiled in the browser with LuaLaTeX; bibliography via BibTeX.",
);

console.log("latex-reader: all checks passed");
