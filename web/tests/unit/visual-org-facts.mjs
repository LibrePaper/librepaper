import assert from "node:assert/strict";
import { orgFacts } from "../../src/lib/visual-markup/org-facts.js";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";

await loadOrgSyntaxForTests();

const facts = (text, extra = {}) => orgFacts({ "main.org": text, ...extra }, "main.org", "main.org");
const numbers = (f) => [...f.headingNumbers.values()];

// Headlines are numbered by default, to any depth, keyed by the Headline offset.
{
  const text = ["* One", "** Two", "*** Three", "* Four", ""].join("\n");
  const f = facts(text);
  assert.deepEqual(numbers(f), ["1", "1.1", "1.1.1", "2"]);
  assert.equal(f.headingNumbers.get(0), "1");
  assert.equal(f.headingNumbers.get(text.indexOf("** Two")), "1.1");
  assert.equal(f.citeStyle, "author-year");
}

// num:nil turns numbering off, num:2 limits the depth.
{
  assert.deepEqual(numbers(facts(["#+OPTIONS: num:nil", "* One", "** Two", ""].join("\n"))), []);
  assert.deepEqual(numbers(facts(["#+OPTIONS: toc:nil num:2", "* One", "** Two", "*** Three", "* Four", ""].join("\n"))), ["1", "1.1", "2"]);
}

// UNNUMBERED and noexport headlines are skipped and not counted, with their subtrees.
{
  const text = [
    "* One",
    "* Plain",
    ":PROPERTIES:",
    ":UNNUMBERED: t",
    ":END:",
    "** Child",
    "* Hidden :noexport:",
    "** Sub",
    "* Two",
    "",
  ].join("\n");
  assert.deepEqual(numbers(facts(text)), ["1", "2"]);
}

// Headline, custom id and target labels.
{
  const text = [
    "* Intro",
    ":PROPERTIES:",
    ":CUSTOM_ID: intro",
    ":END:",
    "** TODO [#A] Methods :tag:",
    "A word <<anchor>> in a paragraph.",
    "",
  ].join("\n");
  const f = facts(text);
  assert.deepEqual(f.labels.get("*Intro"), { kind: "section", number: "1" });
  assert.deepEqual(f.labels.get("#intro"), { kind: "section", number: "1", title: "Intro" });
  assert.deepEqual(f.labels.get("*Methods"), { kind: "section", number: "1.1" });
  assert.deepEqual(f.labels.get("anchor"), { kind: "target", number: "1.1" });
}

// Tables and figures are numbered only with a caption; a name labels them either way.
{
  const text = [
    "#+NAME: bare",
    "| a |",
    "",
    "#+NAME: tab",
    "#+CAPTION: Results",
    "| b |",
    "",
    "#+NAME: fig",
    "#+CAPTION: A chart",
    "[[file:chart.png]]",
    "",
    "[[file:plain.png]]",
    "",
    "#+CAPTION: Second table",
    "| c |",
    "",
  ].join("\n");
  const f = facts(text);
  assert.deepEqual(f.labels.get("bare"), { kind: "table" });
  assert.deepEqual(f.labels.get("tab"), { kind: "table", number: "1", title: "Results" });
  assert.deepEqual(f.labels.get("fig"), { kind: "figure", number: "1", title: "A chart" });
}

// Equations: equation and align count, the starred forms do not.
{
  const text = [
    "\\begin{equation*}",
    "a",
    "\\end{equation*}",
    "",
    "#+NAME: one",
    "\\begin{equation}",
    "b = 1",
    "\\end{equation}",
    "",
    "#+NAME: two",
    "\\begin{align}",
    "c = 2",
    "\\end{align}",
    "",
  ].join("\n");
  const f = facts(text);
  assert.deepEqual(f.labels.get("one"), { kind: "equation", number: "1" });
  assert.deepEqual(f.labels.get("two"), { kind: "equation", number: "2" });
}

// Listings are numbered when captioned.
{
  const text = ["#+NAME: code", "#+CAPTION: A script", "#+begin_src python", "x = 1", "#+end_src", ""].join("\n");
  assert.deepEqual(facts(text).labels.get("code"), { kind: "listing", number: "1", title: "A script" });
}

// Citations are numbered by first appearance, across an include; a citation in code is not one.
{
  const main = ["* One", "See [cite:@b].", "", "#+INCLUDE: \"sub.org\"", "", "[cite:@c;@a]", "#+begin_src org", "[cite:@x]", "#+end_src", ""].join("\n");
  const sub = ["* Sub", "[cite:@a]", ""].join("\n");
  const entries = ["a", "b", "c", "x"].map((key) => ({ key, authors: ["Doe, J."], year: "2020", title: key }));
  const f = orgFacts({ "main.org": main, "sub.org": sub }, "main.org", "main.org", entries);
  const order = (key) => f.citations.get(key).number;
  assert.equal(order("b"), 1);
  assert.equal(order("a"), 2);
  assert.equal(order("c"), 3);
  assert.equal(order("x"), 0);
  // The included headline continues the numbering.
  assert.deepEqual(numbers(f), ["1"]);
  assert.equal(orgFacts({ "main.org": main, "sub.org": sub }, "main.org", "sub.org").headingNumbers.get(0), "2");
}

// The citation style comes from #+CITE_EXPORT:.
{
  const style = (value) => facts(`#+CITE_EXPORT: ${value}\n`).citeStyle;
  assert.equal(style("csl apa.csl"), "author-year");
  assert.equal(style("csl ieee.csl"), "numeric");
  assert.equal(style("basic numeric"), "numeric");
  assert.equal(style("natbib"), "author-year");
  assert.equal(style("biblatex"), "author-year");
  assert.equal(facts("* One\n").citeStyle, "author-year");
}
