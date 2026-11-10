import assert from "node:assert/strict";
import test from "node:test";
import { indexFile } from "../../src/lib/document-index/adapters/index.js";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { definition, diagnostics } from "../../src/lib/document-index/queries.js";

/** The items of one kind, in source order. */
const ofKind = (items, kind) => items.filter((item) => item.kind === kind);

/** The source text a range covers. */
const span = (text, [from, to]) => text.slice(from, to);

const PAPER = [
  "#+TITLE: Paper",
  "* Introduction",
  ":PROPERTIES:",
  ":CUSTOM_ID: intro",
  ":END:",
  "See <<eq-one>> and [cite:@doe2020;@roe2021].",
  "#+NAME: tbl-results",
  "#+CAPTION: Results table",
  "| a | b |",
  "|---+---|",
  "| 1 | 2 |",
  "",
  "#+NAME: fig-plot",
  "#+CAPTION: A plot",
  "[[file:plot.png]]",
  "",
  "#+BIBLIOGRAPHY: refs.bib",
  '#+INCLUDE: "sections/a.org"',
  "",
  "#+begin_src org",
  "[cite:@hidden]",
  "<<not-a-label>>",
  "#+end_src",
  "",
].join("\n");

test("org index finds headings, labels, figures, tables, citations, bibliographies and includes", () => {
  const index = indexFile("paper.org", PAPER);
  assert.equal(index.format, "org");
  const { items } = index;

  const [heading] = ofKind(items, "heading");
  assert.equal(heading.title, "Introduction");
  assert.equal(heading.level, 1);

  const labels = ofKind(items, "label");
  const intro = labels.find((item) => item.name === "intro");
  assert.equal(span(PAPER, intro.nameRange), "intro");
  assert.equal(intro.title, "Introduction");

  const target = labels.find((item) => item.name === "eq-one");
  assert.equal(span(PAPER, target.nameRange), "eq-one");
  assert.equal(span(PAPER, target.range), "<<eq-one>>");

  const table = labels.find((item) => item.name === "tbl-results");
  assert.equal(span(PAPER, table.nameRange), "tbl-results");
  assert.equal(table.title, "Results table");
  assert.ok(span(PAPER, table.range).startsWith("| a | b |"));

  const figure = labels.find((item) => item.name === "fig-plot");
  assert.equal(span(PAPER, figure.nameRange), "fig-plot");
  assert.equal(figure.title, "A plot");

  const [asset] = ofKind(items, "asset");
  assert.equal(asset.name, "plot.png");
  assert.equal(span(PAPER, asset.nameRange), "plot.png");

  const cites = ofKind(items, "cite");
  assert.deepEqual(cites.map((item) => item.name), ["doe2020", "roe2021"]);
  assert.deepEqual(cites.map((item) => span(PAPER, item.nameRange)), ["doe2020", "roe2021"]);

  const [bibliography] = ofKind(items, "bibliography");
  assert.equal(bibliography.name, "refs.bib");
  assert.equal(span(PAPER, bibliography.nameRange), "refs.bib");

  const [include] = ofKind(items, "include");
  assert.equal(include.name, "sections/a.org");
  assert.equal(span(PAPER, include.nameRange), "sections/a.org");
});

test("org index reports nothing from source blocks", () => {
  const index = indexFile("paper.org", PAPER);
  assert.ok(!index.items.some((item) => item.name === "hidden"));
  assert.ok(!index.items.some((item) => item.name === "not-a-label"));
});

const LINKS = [
  "* Introduction",
  ":PROPERTIES:",
  ":CUSTOM_ID: intro",
  ":END:",
  "* TODO Methods",
  "See [[#intro]], [[*Methods]] and [[*TODO Methods]].",
  "Table [[tbl-results]], figure [[eq-one]] and broken [[#nope]].",
  "Notes[fn:x] and [fn:y:inline text] and [fn::anon] and [fn:missing].",
  "",
  "#+NAME: tbl-results",
  "| a |",
  "",
  "See <<eq-one>>.",
  "",
  "[fn:x] The first note.",
  "",
  "#+begin_src org",
  "[[#hidden]] [fn:hidden]",
  "#+end_src",
  "",
].join("\n");

test("org internal links become refs that resolve to labels, and broken ones are reported", () => {
  const index = indexFile("paper.org", LINKS);
  const refs = ofKind(index.items, "ref");
  assert.deepEqual(refs.map((item) => item.name), [
    "intro",
    "Methods",
    "Methods",
    "tbl-results",
    "eq-one",
    "nope",
    "fn:x",
    "fn:missing",
  ]);
  assert.deepEqual(refs.map((item) => span(LINKS, item.nameRange)), [
    "intro",
    "Methods",
    "Methods",
    "tbl-results",
    "eq-one",
    "nope",
    "x",
    "missing",
  ]);
  assert.ok(refs.every((item) => item.namespace === "label"));

  const labels = ofKind(index.items, "label").map((item) => item.name);
  for (const name of ["intro", "Methods", "tbl-results", "eq-one", "fn:x", "fn:y"]) {
    assert.ok(labels.includes(name), `${name} is a label`);
  }
  assert.ok(!labels.includes("fn:hidden"));

  const project = createProjectIndex();
  project.update(index);
  assert.deepEqual(
    diagnostics(project, "paper.org").map((diagnostic) => `${diagnostic.code}:${diagnostic.name}`),
    ["unknown-label:nope", "unknown-label:fn:missing"],
  );

  // `*Methods` (the second ref) goes to the headline "* TODO Methods".
  const headline = definition(project, "paper.org", refs[1].nameRange[0]);
  assert.deepEqual(headline.map((location) => location.from), [LINKS.indexOf("* TODO Methods")]);
});

test("org links to URLs, other files and other schemes are not refs", () => {
  const text = [
    "[[https://example.com]] [[file:notes.org]] [[./other.org]] [[../up.org]]",
    "[[/abs.org]] [[~/home.org]] [[id:abc-123]] [[mailto:a@b.c]] [[notes.org::#intro]]",
    "[[file:plot.png]]",
    "",
  ].join("\n");
  const index = indexFile("links.org", text);
  assert.deepEqual(ofKind(index.items, "ref"), []);
  assert.equal(ofKind(index.items, "asset").length, 1);
});
