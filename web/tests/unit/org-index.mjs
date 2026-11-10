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
  assert.equal(intro.attrs.target, "section");
  assert.equal(intro.attrs.implicit, undefined);

  const target = labels.find((item) => item.name === "eq-one");
  assert.equal(span(PAPER, target.nameRange), "eq-one");
  assert.equal(span(PAPER, target.range), "<<eq-one>>");
  assert.equal(target.attrs.target, "other");

  const table = labels.find((item) => item.name === "tbl-results");
  assert.equal(span(PAPER, table.nameRange), "tbl-results");
  assert.equal(table.title, "Results table");
  assert.equal(table.attrs.target, "table");
  assert.ok(span(PAPER, table.range).startsWith("| a | b |"));

  const figure = labels.find((item) => item.name === "fig-plot");
  assert.equal(span(PAPER, figure.nameRange), "fig-plot");
  assert.equal(figure.title, "A plot");
  assert.equal(figure.attrs.target, "figure");

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

const TARGETS = [
  "* Results",
  "* TODO Discussion",
  "",
  "#+NAME: listing-one",
  "#+begin_src python",
  "print(1)",
  "#+end_src",
  "",
  "#+NAME: eq-named",
  "\\begin{equation}",
  "a = b \\label{eq-inside}",
  "\\end{equation}",
  "",
  "A <<<radio>>> and <<spot>>.",
  "",
  "#+NAME: note-words",
  "Just words.",
  "",
].join("\n");

test("org labels carry the kind of target they name", () => {
  const labels = ofKind(indexFile("targets.org", TARGETS).items, "label");
  const named = (name) => labels.find((item) => item.name === name);

  const results = named("Results");
  assert.equal(results.attrs.target, "section");
  assert.equal(results.attrs.implicit, "true");
  assert.equal(results.attrs.form, "heading");

  assert.equal(named("Discussion").attrs.target, "section");
  assert.equal(named("listing-one").attrs.target, "listing");
  assert.equal(named("eq-named").attrs.target, "equation");
  assert.equal(named("note-words").attrs.target, "other");
  assert.equal(named("radio").attrs.target, "other");
  assert.equal(named("spot").attrs.target, "other");

  const inside = named("eq-inside");
  assert.equal(inside.attrs.target, "equation");
  assert.equal(span(TARGETS, inside.nameRange), "eq-inside");
  assert.equal(span(TARGETS, inside.range), "\\label{eq-inside}");
});

test("a #+NAME on a math environment does not repeat its \\label", () => {
  const labels = ofKind(indexFile("eq.org", "#+NAME: e1\n\\begin{equation}\n\\label{e1}\n\\end{equation}\n").items, "label");
  assert.deepEqual(labels.map((item) => [item.name, item.attrs.target]), [["e1", "equation"]]);
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
    "x",
    "missing",
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
  assert.deepEqual(refs.map((item) => item.namespace), [
    "label",
    "label",
    "label",
    "label",
    "label",
    "label",
    "footnote",
    "footnote",
  ]);

  const labels = ofKind(index.items, "label");
  const names = labels.filter((item) => item.namespace !== "footnote").map((item) => item.name);
  for (const name of ["intro", "Methods", "tbl-results", "eq-one"]) {
    assert.ok(names.includes(name), `${name} is a label`);
  }
  const footnotes = labels.filter((item) => item.namespace === "footnote").map((item) => item.name);
  assert.deepEqual(footnotes, ["x", "y"]);
  assert.ok(!footnotes.includes("hidden"));

  const project = createProjectIndex();
  project.update(index);
  assert.deepEqual(
    diagnostics(project, "paper.org").map((diagnostic) => `${diagnostic.code}:${diagnostic.name}`),
    ["unknown-label:nope", "unknown-footnote:missing"],
  );

  // `*Methods` (the second ref) goes to the headline "* TODO Methods".
  const headline = definition(project, "paper.org", refs[1].nameRange[0]);
  assert.deepEqual(headline.map((location) => location.from), [LINKS.indexOf("* TODO Methods")]);
});

test("org footnote definitions and references live in the footnote namespace", () => {
  const text = "Text[fn:a] and [fn:b:inline].\n\n[fn:a] Def A.\n";
  const { items } = indexFile("notes.org", text);
  const definitions = ofKind(items, "label").filter((item) => item.namespace === "footnote");
  assert.deepEqual(definitions.map((item) => item.name), ["b", "a"]);
  assert.deepEqual(definitions.map((item) => item.attrs), [{ target: "footnote" }, { target: "footnote" }]);
  assert.equal(span(text, definitions[1].nameRange), "a");

  const refs = ofKind(items, "ref");
  assert.deepEqual(refs.map((item) => [item.namespace, item.name]), [["footnote", "a"]]);
  assert.equal(span(text, refs[0].nameRange), "a");
  assert.equal(refs[0].attrs, undefined);
});

test("org links to URLs and files are link items", () => {
  const text = [
    "[[https://example.com]] [[file:notes.org]] [[./other.org]] [[../up.org]]",
    "[[/abs.org]] [[~/home.org]] [[id:abc-123]] [[mailto:a@b.c]] [[notes.org::#intro]]",
    "[[file:plot.png]] https://example.org/plain <mailto:x@y.z>",
    "",
  ].join("\n");
  const index = indexFile("links.org", text);
  assert.deepEqual(ofKind(index.items, "ref"), []);
  assert.equal(ofKind(index.items, "asset").length, 1);

  const links = ofKind(index.items, "link");
  assert.deepEqual(
    links.map((item) => [item.name, item.attrs.external ? "external" : "file"]),
    [
      ["https://example.com", "external"],
      ["notes.org", "file"],
      ["./other.org", "file"],
      ["../up.org", "file"],
      ["/abs.org", "file"],
      ["~/home.org", "file"],
      ["mailto:a@b.c", "external"],
      ["notes.org", "file"],
      ["https://example.org/plain", "external"],
      ["mailto:x@y.z", "external"],
    ],
  );
  assert.equal(links[7].attrs.fragment, "#intro");
  assert.equal(links[7].attrs.file, "true");
  assert.equal(span(text, links[7].nameRange), "notes.org");
  assert.equal(span(text, links[1].nameRange), "notes.org");
});

test("org metadata keywords and the abstract become metadata items", () => {
  const text = [
    "#+TITLE: A *bold* title",
    "#+SUBTITLE: Sub",
    "#+AUTHOR: Ada Lovelace",
    "#+DATE: 2026-11-10",
    "#+LANGUAGE: fr",
    "#+begin_abstract",
    "First [[https://example.com][paragraph]] of the abstract.",
    "",
    "Second paragraph.",
    "#+end_abstract",
    "",
    "* Body",
  ].join("\n");
  const metadata = ofKind(indexFile("meta.org", text).items, "metadata");
  assert.deepEqual(
    metadata.map((item) => [item.name, item.title]),
    [
      ["title", "A bold title"],
      ["subtitle", "Sub"],
      ["author", "Ada Lovelace"],
      ["date", "2026-11-10"],
      ["language", "fr"],
      ["abstract", "First paragraph of the abstract. Second paragraph."],
    ],
  );
  assert.equal(span(text, metadata[2].nameRange), "Ada Lovelace");
});
