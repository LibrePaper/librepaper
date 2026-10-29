import assert from "node:assert/strict";
import { documentFacts } from "../../src/lib/visual-markup/facts.js";

// Theorem numbering: a shared counter, a counter reset within a section, and a
// starred environment that is unnumbered.
{
  const paper = [
    "\\newtheorem{theorem}{Theorem}",
    "\\newtheorem{lemma}[theorem]{Lemma}",
    "\\newtheorem{corollary}{Corollary}[section]",
    "\\newtheorem*{remark}{Remark}",
    "\\begin{document}",
    "\\section{Results}",
    "\\begin{theorem}\\label{thm:main}",
    "Text.",
    "\\end{theorem}",
    "\\begin{lemma}[Key]\\label{lem:key}",
    "\\end{lemma}",
    "\\begin{corollary}\\label{cor:one}",
    "\\end{corollary}",
    "\\begin{remark}\\label{rem:note}",
    "\\end{remark}",
    "\\end{document}",
  ].join("\n");
  const facts = documentFacts({ "main.tex": paper }, "main.tex", "main.tex", []);
  assert.equal(facts.theorems.get("theorem").title, "Theorem");
  assert.equal(facts.theorems.get("theorem").numbered, true);
  assert.equal(facts.theorems.get("lemma").counter, "theorem");
  assert.equal(facts.theorems.get("remark").numbered, false);
  assert.deepEqual(facts.labels.get("thm:main"), { number: "1", kind: "theorem" });
  assert.deepEqual(facts.labels.get("lem:key"), { number: "2", kind: "theorem" });
  assert.deepEqual(facts.labels.get("cor:one"), { number: "1.1", kind: "theorem" });
  // An unnumbered theorem has no number of its own, so, as in LaTeX, its label takes the section's.
  assert.deepEqual(facts.labels.get("rem:note"), { number: "1", kind: "section" });
  // Environment numbers are keyed by the offset of \begin in the open file.
  assert.equal(facts.environmentNumbers.get(paper.indexOf("\\begin{theorem}")), "1");
  assert.equal(facts.environmentNumbers.get(paper.indexOf("\\begin{lemma}")), "2");
}

// Citations are numbered in order of first citation, across the citation
// families, and take their surnames from the bibliography entries.
{
  const entries = [
    { key: "knuth", authors: ["Donald E. Knuth"], year: "1984", title: "Literate programming" },
    { key: "lamport", authors: ["Leslie Lamport"], year: "1994", title: "LaTeX: A Document Preparation System" },
    { key: "dijkstra", authors: ["Edsger W. Dijkstra"], year: "1968", title: "Go to statement considered harmful" },
  ];
  const paper = "\\citep{knuth} then \\cite{lamport,dijkstra} and \\textcite{knuth}.";
  const facts = documentFacts({ "main.tex": paper }, "main.tex", "main.tex", entries);
  assert.equal(facts.citations.get("knuth").number, 1);
  assert.equal(facts.citations.get("lamport").number, 2);
  assert.equal(facts.citations.get("dijkstra").number, 3);
  assert.deepEqual(facts.citations.get("knuth").authors, ["Knuth"]);
  assert.equal(facts.citations.get("knuth").year, "1984");
  assert.equal(facts.citations.get("knuth").title, "Literate programming");
}

// Acronyms from \newacronym and from the acro package's \DeclareAcronym.
{
  const paper = "\\newacronym{ml}{ML}{machine learning}\n\\DeclareAcronym{nn}{short=NN, long=neural network}\n";
  const facts = documentFacts({ "main.tex": paper }, "main.tex", "main.tex", []);
  assert.deepEqual(facts.acronyms.get("ml"), { short: "ML", long: "machine learning" });
  assert.deepEqual(facts.acronyms.get("nn"), { short: "NN", long: "neural network" });
}

// Text macros from \newcommand with one argument, from \newcommand with an
// optional default, and from a plain \def.
{
  const paper = "\\newcommand{\\foo}[1]{x #1}\n\\newcommand{\\qux}[2][none]{[#1 #2]}\n\\def\\bar{y}\n";
  const facts = documentFacts({ "main.tex": paper }, "main.tex", "main.tex", []);
  assert.deepEqual(facts.textMacros.get("foo"), { args: 1, optional: null, body: "x #1" });
  assert.deepEqual(facts.textMacros.get("qux"), { args: 2, optional: "none", body: "[#1 #2]" });
  assert.deepEqual(facts.textMacros.get("bar"), { args: 0, optional: null, body: "y" });
}

// Math macros for KaTeX: a declared operator becomes \operatorname, and a
// plain \newcommand is passed through as its body.
{
  const paper = "\\DeclareMathOperator{\\argmax}{arg\\,max}\n\\newcommand{\\R}{\\mathbb{R}}\n";
  const facts = documentFacts({ "main.tex": paper }, "main.tex", "main.tex", []);
  assert.equal(facts.mathMacros["\\argmax"], "\\operatorname{arg\\,max}");
  assert.equal(facts.mathMacros["\\R"], "\\mathbb{R}");
}

// The citation style follows the natbib, plainnat and biblatex settings.
{
  const style = (paper) => documentFacts({ "main.tex": paper }, "main.tex", "main.tex", []).citeStyle;
  assert.equal(style("\\usepackage[numbers]{natbib}\n\\bibliographystyle{plain}"), "numeric");
  assert.equal(style("\\usepackage{natbib}\n\\bibliographystyle{plainnat}"), "author-year");
  assert.equal(style("\\usepackage[style=numeric]{biblatex}"), "numeric");
  assert.equal(style("\\bibliographystyle{unsrt}"), "numeric");
}
