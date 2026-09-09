// Engine selection is a pure function of text, so this check runs it over
// short synthetic sources rather than a real corpus document: what matters
// here is precedence and false-positive avoidance, not real-world documents
// (those are exercised end-to-end in latex-browser.mjs).
import assert from "node:assert/strict";
import { ENGINES, directiveOf, detect, resolveEngine, needsBiber } from "../src/lib/latex/engine.js";

assert.deepEqual(ENGINES, ["pdflatex", "xelatex", "lualatex"]);

// --- directiveOf --------------------------------------------------------

assert.equal(directiveOf("% !TEX program = xelatex\n\\documentclass{article}"), "xelatex");
// Case-insensitivity on both the directive keyword and the engine name.
assert.equal(directiveOf("% !tex PROGRAM = XeLaTeX\n"), "xelatex");
assert.equal(directiveOf("%!TEX TS-program = lualatex\n"), "lualatex");
assert.equal(directiveOf("% !TEX program = pdflatex\n"), "pdflatex");
assert.equal(directiveOf("% !TEX program = latex\n"), "pdflatex");
assert.equal(directiveOf("% !TEX program = luatex\n"), "lualatex");
assert.equal(directiveOf("% !TEX program = xetex\n"), "xelatex");
// Unknown names never resolve to something -- not a silent pdflatex either.
assert.equal(directiveOf("% !TEX program = context\n"), null);
assert.equal(directiveOf("\\documentclass{article}\n% no directive here\n"), null);
// A directive-shaped line past the scanned prefix does not count.
assert.equal(directiveOf(`${"x".repeat(4096)}\n% !TEX program = xelatex\n`), null);

// --- detect --------------------------------------------------------------

assert.equal(detect("\\documentclass{article}\n\\usepackage{fontspec}\n\\begin{document}\\end{document}"), "xelatex");
assert.equal(detect("\\usepackage{unicode-math}\n"), "xelatex");
assert.equal(detect("\\usepackage{polyglossia}\n"), "xelatex");
assert.equal(detect("\\usepackage{xeCJK}\n"), "xelatex");
assert.equal(detect("\\usepackage{xetexko}\n"), "xelatex");
assert.equal(detect("\\setmainfont{Latin Modern Roman}\n"), "xelatex");
assert.equal(detect("\\usepackage{luacode}\n"), "lualatex");
assert.equal(detect("\\usepackage{luatexja}\n"), "lualatex");
assert.equal(detect("\\directlua{tex.print(1)}\n"), "lualatex");
assert.equal(detect("\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\\end{document}"), null);

// A commented-out package requirement must not switch the engine: an author
// who tried fontspec and commented it back out should still get pdflatex.
assert.equal(detect("% \\usepackage{fontspec}\n\\documentclass{article}\n"), null);
assert.equal(detect("\\usepackage{amsmath} % not \\usepackage{fontspec}\n"), null);
// An escaped percent is not a comment marker.
assert.equal(detect("\\newcommand{\\pct}{\\%}\n\\usepackage{fontspec}\n"), "xelatex");

// Only the preamble is scanned: a package loaded after \begin{document} in a
// stray \usepackage (unusual, but not our problem to detect) is out of scope,
// and text that merely mentions a package name in prose does not count.
assert.equal(detect("\\begin{document}\n\\usepackage{fontspec}\n\\end{document}"), null);

// --- resolveEngine ---------------------------------------------------------

function treeWith(text) {
  return { main: "main.tex", texts: { "main.tex": text } };
}

// 1. Explicit setting wins over everything, including a directive.
assert.equal(
  resolveEngine(treeWith("% !TEX program = xelatex\n"), { engine: "lualatex" }),
  "lualatex",
);
// 2. No explicit engine (or "auto"): the directive wins over detection.
assert.equal(
  resolveEngine(treeWith("% !TEX program = pdflatex\n\\usepackage{fontspec}\n"), { engine: "auto" }),
  "pdflatex",
);
assert.equal(
  resolveEngine(treeWith("% !TEX program = pdflatex\n\\usepackage{fontspec}\n"), {}),
  "pdflatex",
);
// 3. No setting, no directive: detection.
assert.equal(resolveEngine(treeWith("\\usepackage{luatexja}\n"), { engine: "auto" }), "lualatex");
// 4. Nothing matches: pdflatex.
assert.equal(resolveEngine(treeWith("\\documentclass{article}\n"), { engine: "auto" }), "pdflatex");
assert.equal(resolveEngine(treeWith(""), null), "pdflatex");

// --- needsBiber ------------------------------------------------------------

assert.equal(needsBiber("\\usepackage{biblatex}\n"), true);
assert.equal(needsBiber("\\usepackage[style=authoryear]{biblatex}\n"), true);
// backend=bibtex is the one biblatex option that opts back out of Biber.
assert.equal(needsBiber("\\usepackage[backend=bibtex]{biblatex}\n"), false);
assert.equal(needsBiber("\\usepackage[style=authoryear,backend=bibtex]{biblatex}\n"), false);
assert.equal(needsBiber("\\usepackage{natbib}\n"), false);
assert.equal(needsBiber("\\bibliographystyle{plain}\n\\bibliography{refs}\n"), false);
// A commented-out biblatex load must not trigger the hint either.
assert.equal(needsBiber("% \\usepackage{biblatex}\n"), false);

console.log("latex-engine: ok");
