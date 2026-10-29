import assert from "node:assert/strict";
import { typstFacts } from "../../src/lib/visual-markup/typst-facts.js";

// Headings are numbered only while a numbering is set. A label after a heading binds to it.
{
  const numbered = '#set heading(numbering: "1.1")\n= Intro <intro>\n== Background <bg>\n';
  const facts = typstFacts({ "main.typ": numbered }, "main.typ", "main.typ", []);
  assert.deepEqual(facts.labels.get("intro"), { number: "1", kind: "section" });
  assert.deepEqual(facts.labels.get("bg"), { number: "1.1", kind: "section" });

  const plain = "= Intro <intro>\n== Background <bg>\n";
  const unnumbered = typstFacts({ "main.typ": plain }, "main.typ", "main.typ", []);
  assert.equal(unnumbered.labels.get("intro")?.number ?? null, null);
  assert.equal(unnumbered.labels.get("bg")?.number ?? null, null);
}

// Figures are numbered by kind: an image and a table count separately. A label
// right after a figure binds to that figure.
{
  const paper = [
    '#figure(image("a.png"), caption: [A plot]) <fig-a>',
    '#figure(table(columns: 2, [a], [b]), caption: [Values]) <tab-b>',
    '#figure(image("c.png"), caption: [Second plot]) <fig-c>',
  ].join("\n");
  const facts = typstFacts({ "main.typ": paper }, "main.typ", "main.typ", []);
  assert.deepEqual(facts.labels.get("fig-a"), { number: "1", kind: "figure" });
  assert.deepEqual(facts.labels.get("tab-b"), { number: "1", kind: "table" });
  assert.deepEqual(facts.labels.get("fig-c"), { number: "2", kind: "figure" });
}

// Display equations are numbered while a math equation numbering is set. Inline
// math takes no number.
{
  const paper = [
    '#set math.equation(numbering: "(1)")',
    "$ a^2 + b^2 = c^2 $ <eq-pyth>",
    "Inline $x$ stays unnumbered.",
    "$ y = 2 $ <eq-two>",
  ].join("\n");
  const facts = typstFacts({ "main.typ": paper }, "main.typ", "main.typ", []);
  assert.deepEqual(facts.labels.get("eq-pyth"), { number: "1", kind: "equation" });
  assert.deepEqual(facts.labels.get("eq-two"), { number: "2", kind: "equation" });

  const unnumbered = typstFacts({ "main.typ": "$ y = 2 $ <eq-two>" }, "main.typ", "main.typ", []);
  assert.equal(unnumbered.labels.get("eq-two")?.number ?? null, null);
}

// An included file is walked where the include stands, so its heading comes
// before the main file's heading that follows the include.
{
  const main = '#set heading(numbering: "1.")\n#include "ch.typ"\n= After <after>\n';
  const chapter = "= Chapter <ch>\n";
  const facts = typstFacts({ "main.typ": main, "ch.typ": chapter }, "main.typ", "main.typ", []);
  assert.deepEqual(facts.labels.get("ch"), { number: "1", kind: "section" });
  assert.deepEqual(facts.labels.get("after"), { number: "2", kind: "section" });
}

// Citations are numbered by first use, across @key and #cite(<key>). A reference
// to a label of this document is not a citation.
{
  const entries = [
    { key: "knuth", authors: ["Donald E. Knuth"], year: "1984", title: "Literate programming" },
    { key: "lamport", authors: ["Leslie Lamport"], year: "1994", title: "LaTeX: A Document Preparation System" },
    { key: "dijkstra", authors: ["Edsger W. Dijkstra"], year: "1968", title: "Go to statement considered harmful" },
  ];
  const paper = [
    "= Intro <intro>",
    "Start with @knuth, then #cite(<lamport>) and @intro.",
    "Later @dijkstra and @knuth again.",
  ].join("\n");
  const facts = typstFacts({ "main.typ": paper }, "main.typ", "main.typ", entries);
  assert.equal(facts.citations.get("knuth").number, 1);
  assert.equal(facts.citations.get("lamport").number, 2);
  assert.equal(facts.citations.get("dijkstra").number, 3);
  assert.equal(facts.citations.has("intro"), false);
  assert.deepEqual(facts.citations.get("knuth").authors, ["Knuth"]);
  assert.equal(facts.citations.get("knuth").year, "1984");
}

// The citation style is numeric by default, and author-year for the APA style.
{
  const style = (paper) => typstFacts({ "main.typ": paper }, "main.typ", "main.typ", []).citeStyle;
  assert.equal(style("Text @knuth."), "numeric");
  assert.equal(style('Text @knuth.\n#bibliography("refs.bib")'), "numeric");
  assert.equal(style('#bibliography("refs.bib", style: "ieee")'), "numeric");
  assert.equal(style('#bibliography("refs.bib", style: "apa")'), "author-year");
}
