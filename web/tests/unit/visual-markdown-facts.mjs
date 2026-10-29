import assert from "node:assert/strict";
import { markdownFacts } from "../../src/lib/visual-markup/markdown-facts.js";

// Figures, tables and equations are numbered in document order, across an
// included file. A code cell takes its number from its label line.
{
  const main = [
    "---",
    "title: Paper",
    "---",
    "",
    "![A chart](chart.png){#fig-a}",
    "",
    "```{r}",
    "#| label: fig-cell",
    "#| fig-cap: \"A cell plot\"",
    "plot(1:10)",
    "```",
    "",
    "{{< include sub.qmd >}}",
    "",
    "| a | b |",
    "|---|---|",
    "| 1 | 2 |",
    "",
    ": Results {#tbl-x}",
    "",
    "$$ x = 1 $$ {#eq-one}",
    "",
  ].join("\n");
  const sub = [
    "![Sub chart](sub.png){#fig-b}",
    "",
    "| c |",
    "|---|",
    "| 3 |",
    "",
    ": Sub table {#tbl-s}",
    "",
    "$$ y = 2 $$ {#eq-two}",
    "",
  ].join("\n");
  const facts = markdownFacts({ "main.qmd": main, "sub.qmd": sub }, "main.qmd", "main.qmd", []);
  assert.deepEqual(facts.labels.get("fig-a"), { number: "1", kind: "figure" });
  assert.deepEqual(facts.labels.get("fig-cell"), { number: "2", kind: "figure" });
  assert.deepEqual(facts.labels.get("fig-b"), { number: "3", kind: "figure" });
  assert.deepEqual(facts.labels.get("tbl-s"), { number: "1", kind: "table" });
  assert.deepEqual(facts.labels.get("tbl-x"), { number: "2", kind: "table" });
  assert.deepEqual(facts.labels.get("eq-two"), { number: "1", kind: "equation" });
  assert.deepEqual(facts.labels.get("eq-one"), { number: "2", kind: "equation" });
}

// Sections get no number unless the YAML sets number-sections: true. The
// label still exists, with an empty number, so the widget shows the heading.
{
  const plain = ["## Intro {#sec-intro}", "", "Text.", ""].join("\n");
  const facts = markdownFacts({ "main.qmd": plain }, "main.qmd", "main.qmd", []);
  assert.deepEqual(facts.labels.get("sec-intro"), { number: "", kind: "section" });
}
{
  const numbered = [
    "---",
    "number-sections: true",
    "---",
    "",
    "# Methods {#sec-methods}",
    "",
    "## Data {#sec-data}",
    "",
    "# Results {#sec-results}",
    "",
  ].join("\n");
  const facts = markdownFacts({ "main.qmd": numbered }, "main.qmd", "main.qmd", []);
  assert.deepEqual(facts.labels.get("sec-methods"), { number: "1", kind: "section" });
  assert.deepEqual(facts.labels.get("sec-data"), { number: "1.1", kind: "section" });
  assert.deepEqual(facts.labels.get("sec-results"), { number: "2", kind: "section" });
}

// Theorem-like divs count with one counter per prefix: thm and lem each start at 1.
{
  const paper = [
    "::: {#thm-a}",
    "Text.",
    ":::",
    "",
    "::: {#lem-b}",
    "Text.",
    ":::",
    "",
    "::: {#thm-c}",
    "Text.",
    ":::",
    "",
  ].join("\n");
  const facts = markdownFacts({ "main.qmd": paper }, "main.qmd", "main.qmd", []);
  assert.deepEqual(facts.labels.get("thm-a"), { number: "1", kind: "theorem" });
  assert.deepEqual(facts.labels.get("lem-b"), { number: "1", kind: "theorem" });
  assert.deepEqual(facts.labels.get("thm-c"), { number: "2", kind: "theorem" });
}

// Citations are numbered by first use, across bracketed and bare citations.
// Email addresses and cross-references are not citations.
{
  const entries = [
    { key: "knuth", authors: ["Donald E. Knuth"], year: "1984", title: "Literate programming" },
    { key: "lamport", authors: ["Leslie Lamport"], year: "1994", title: "LaTeX: A Document Preparation System" },
    { key: "dijkstra", authors: ["Edsger W. Dijkstra"], year: "1968", title: "Go to statement considered harmful" },
  ];
  const paper = [
    "---",
    "csl: ieee.csl",
    "---",
    "",
    "See [@knuth] and [see @lamport, p. 3; @knuth].",
    "",
    "Then @dijkstra wrote, and @fig-x is a cross-reference.",
    "",
    "Mail jane@example.com about it.",
    "",
  ].join("\n");
  const facts = markdownFacts({ "main.qmd": paper }, "main.qmd", "main.qmd", entries);
  assert.equal(facts.citations.get("knuth").number, 1);
  assert.equal(facts.citations.get("lamport").number, 2);
  assert.equal(facts.citations.get("dijkstra").number, 3);
  assert.deepEqual(facts.citations.get("knuth").authors, ["Knuth"]);
  assert.equal(facts.citations.get("knuth").year, "1984");
  assert.equal(facts.citations.get("knuth").title, "Literate programming");
  assert.equal(facts.citations.has("fig-x"), false);
  assert.equal(facts.citations.has("example"), false);
}

// A csl: ieee.csl setting makes the citations numeric; without one they are author-year.
{
  const style = (paper) => markdownFacts({ "main.qmd": paper }, "main.qmd", "main.qmd", []).citeStyle;
  assert.equal(style("---\ncsl: ieee.csl\n---\n\nText [@a].\n"), "numeric");
  assert.equal(style("---\ntitle: Plain\n---\n\nText [@a].\n"), "author-year");
}
