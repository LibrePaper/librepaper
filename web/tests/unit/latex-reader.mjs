// Behavioural checks for the reader's LaTeX status text -- the pure mapping
// from a `Status`/`Provenance` to what a reader sees, kept in
// web/src/lib/latex/status-text.js so it can be checked without a Svelte
// runtime and without `latex.js` itself.
import assert from "node:assert/strict";
import { backendChip, provenanceSentence } from "../../src/lib/latex/status-text.js";

const idle = { phase: "idle", backend: null };

// backendChip

assert.equal(backendChip(null), "");
assert.equal(backendChip(idle), "");
assert.equal(backendChip({ ...idle, phase: "compiling", backend: "browser" }), "browser");
assert.equal(backendChip({ ...idle, phase: "browser-biber", backend: "browser" }), "browser Biber");
// LaTeX is built in the browser and nowhere else. A stale `local` backend --
// from a cached result, or a status shape that outlived the companion path --
// names no backend rather than claiming a local build happened.
assert.equal(backendChip({ ...idle, phase: "compiling", backend: "local" }), "");

// provenanceSentence

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
