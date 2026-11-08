import assert from "node:assert/strict";
import test from "node:test";
import { markdownAdapter } from "../../src/lib/document-index/adapters/markdown.js";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import {
  definition,
  diagnostics,
  references,
} from "../../src/lib/document-index/queries.js";
import { citedKeys } from "../../src/lib/citation-keys.js";
import { quartoLabels } from "../../src/lib/completion/quarto.js";
import { extractOutline } from "../../src/lib/outline.js";
import { skipRanges } from "../../src/lib/spellcheck/index.js";

/** Range of the first `needle` at or after `from`. */
function at(text, needle, from = 0) {
  const start = text.indexOf(needle, from);
  assert.notEqual(start, -1, `missing ${needle}`);
  return [start, start + needle.length];
}

/** The text an item's name covers. */
const named = (text, item) => text.slice(...item.nameRange);

/** Items of one kind, in source order. */
const kindOf = (items, kind) => items.filter((item) => item.kind === kind).sort((a, b) => a.range[0] - b.range[0]);

const quartoDoc = [
  "---",
  'title: "Notes"',
  "bibliography: refs.bib",
  "---",
  "",
  "# Intro {#sec-intro}",
  "",
  "See @fig-plot, [@smith2020], and [see @tbl-data, p. 3]. Inline `@fig-code` is code.",
  "",
  "```{r}",
  "#| label: fig-plot",
  '#| fig-cap: "Scatter of x and y"',
  "plot(1:10)",
  "```",
  "",
  '```{r, label="tbl-data"}',
  '#| tbl-cap: "Data"',
  "head(mtcars)",
  "```",
  "",
  "![A photo](images/photo.png){#fig-photo}",
  "",
  "::: {.callout-note}",
  "A note with $x^2$ inside.",
  ":::",
  "",
  "$$",
  "a = b",
  "$$ {#eq-a}",
  "",
  "```markdown",
  "@fig-z and {#fig-z}",
  "```",
  "",
  "Stop at @fig-missing.",
  "",
  "## Methods",
  "",
  "Text by [@jones2019; @smith2020].",
  "",
].join("\n");

const plainDoc = [
  "# Intro",
  "",
  "Text with [@smith2020] and a [link](#methods).",
  "",
  "```python",
  "# not a heading",
  "@nope",
  "```",
  "",
  "## Methods {#methods}",
  "",
  "Done. $x$ math.",
  "",
].join("\n");

test("Quarto equivalence: headings, cites, skips and labels match the existing scanners", () => {
  const items = markdownAdapter(quartoDoc, "doc.qmd");

  const headings = kindOf(items, "heading").map((item) => ({
    title: item.title,
    level: item.level,
    from: item.range[0],
  }));
  assert.deepEqual(
    headings,
    extractOutline(quartoDoc, "quarto").map((heading) => ({ title: heading.title, level: heading.level, from: heading.from })),
  );

  const cites = kindOf(items, "cite").map((item) => ({ key: item.name, from: item.range[0], to: item.range[1] }));
  assert.deepEqual(cites, citedKeys(quartoDoc, "quarto"));

  const markup = kindOf(items, "markup").map((item) => item.range);
  assert.deepEqual(markup, skipRanges(quartoDoc, "quarto"));

  const labels = new Set(kindOf(items, "label").map((item) => item.name));
  assert.deepEqual(labels, new Set(quartoLabels({ "doc.qmd": quartoDoc }).map((label) => label.id)));
});

test("plain Markdown equivalence: headings, cites and skips match the existing scanners", () => {
  const items = markdownAdapter(plainDoc, "notes.md");

  assert.deepEqual(
    kindOf(items, "heading").map((item) => ({ title: item.title, level: item.level, from: item.range[0] })),
    extractOutline(plainDoc, "markdown").map((heading) => ({ title: heading.title, level: heading.level, from: heading.from })),
  );
  assert.deepEqual(
    kindOf(items, "cite").map((item) => ({ key: item.name, from: item.range[0], to: item.range[1] })),
    citedKeys(plainDoc, "markdown"),
  );
  assert.deepEqual(kindOf(items, "markup").map((item) => item.range), skipRanges(plainDoc, "markdown"));
  assert.deepEqual(kindOf(items, "ref"), []);
});

test("headings keep the outline's title and span the whole line", () => {
  const text = "# One\n\ntext\n\n## Two ##\n```\n# not\n```\n";
  const headings = kindOf(markdownAdapter(text, "a.md"), "heading");
  assert.equal(headings.length, 2);
  assert.equal(headings[0].level, 1);
  assert.equal(headings[0].title, "One");
  assert.deepEqual(headings[0].range, [0, 5]);
  assert.equal(headings[1].level, 2);
  assert.equal(headings[1].title, "Two");
  assert.deepEqual(headings[1].range, at(text, "## Two ##"));
});

test("explicit ids are labels in Quarto with their titles; automatic heading ids are not", () => {
  const text = "# Intro {#sec-intro}\n\n![A photo](a.png){#fig-photo}\n\n: Table caption {#tbl-data}\n\n## Plain {#plain}\n\n# Automatic\n";
  const labels = kindOf(markdownAdapter(text, "a.qmd"), "label");
  assert.deepEqual(labels.map((item) => item.name), ["sec-intro", "fig-photo", "tbl-data"]);
  assert.deepEqual(labels.map((item) => item.title), ["Intro", "A photo", "Table caption"]);
  for (const label of labels) assert.equal(named(text, label), label.name);
  assert.equal(text.slice(...labels[1].range), "{#fig-photo}");
  assert.deepEqual(kindOf(markdownAdapter("# Intro\n\n## Methods\n", "a.qmd"), "label"), []);
});

test("plain Markdown takes any explicit id as a label", () => {
  const text = "# A {#intro}\n\n![x](a.png){#fig-x}\n";
  const labels = kindOf(markdownAdapter(text, "notes.md"), "label");
  assert.deepEqual(labels.map((item) => item.name), ["intro", "fig-x"]);
});

test("executable cells: #| label wins over the chunk header, captions become titles", () => {
  const text = [
    "```{r}",
    "#| label: fig-a",
    '#| fig-cap: "First"',
    "plot(1)",
    "```",
    "",
    '```{r, label="tbl-b"}',
    "#| tbl-cap: 'Second'",
    "head(x)",
    "```",
    "",
    '```{r, label="fig-d"}',
    "#| label: fig-c",
    "plot(2)",
    "```",
    "",
    "```{python}",
    "#| label: setup",
    "```",
    "",
  ].join("\n");
  const labels = kindOf(markdownAdapter(text, "a.qmd"), "label");
  assert.deepEqual(labels.map((item) => item.name), ["fig-a", "tbl-b", "fig-c"]);
  assert.deepEqual(labels.map((item) => item.title), ["First", "Second", undefined]);
  assert.equal(text.slice(...labels[0].range), "#| label: fig-a");
  assert.equal(text.slice(...labels[1].range), '```{r, label="tbl-b"}');
  for (const label of labels) assert.equal(named(text, label), label.name);
});

test("refs are found in Quarto prose, not in code, math, front matter or inline code", () => {
  const text = [
    "---",
    'note: "@fig-front"',
    "---",
    "",
    "See @fig-a and [see @tbl-b].",
    "",
    "Inline `@fig-inline` is code.",
    "",
    "Math $@fig-math$ here.",
    "",
    "$$",
    "@fig-display",
    "$$",
    "",
    "```",
    "@fig-code",
    "```",
    "",
  ].join("\n");
  const refs = kindOf(markdownAdapter(text, "a.qmd"), "ref");
  assert.deepEqual(refs.map((item) => item.name), ["fig-a", "tbl-b"]);
  assert.deepEqual(refs.map((item) => item.namespace), ["label", "label"]);
  assert.equal(text.slice(...refs[0].range), "@fig-a");
  assert.equal(named(text, refs[0]), "fig-a");
});

test("plain Markdown emits no refs, and @keys stay citations", () => {
  const text = "See @fig-a and [@smith].\n";
  const items = markdownAdapter(text, "notes.md");
  assert.deepEqual(kindOf(items, "ref"), []);
  assert.deepEqual(kindOf(items, "cite").map((item) => item.name), ["smith"]);
});

test("the extension test ignores case and nothing else makes a file Quarto", () => {
  assert.equal(kindOf(markdownAdapter("See @fig-a.", "DOC.QMD"), "ref").length, 1);
  assert.equal(kindOf(markdownAdapter("See @fig-a.", "doc.md"), "ref").length, 0);
});

test("includes are Quarto-only; assets skip URLs, anchors and data URIs", () => {
  const text = [
    "{{< include chapters/one.qmd >}}",
    "",
    '![a](img/a.png "Title") ![b](https://x.org/b.png) ![c](#frag)',
    "![d](img/d.png#x) ![e](data:image/png;base64,AAAA) ![f](<img/with space.png>)",
    "",
  ].join("\n");
  const quartoItems = markdownAdapter(text, "a.qmd");
  const includes = kindOf(quartoItems, "include");
  assert.deepEqual(includes.map((item) => item.name), ["chapters/one.qmd"]);
  assert.equal(named(text, includes[0]), "chapters/one.qmd");
  const assets = kindOf(quartoItems, "asset");
  assert.deepEqual(assets.map((item) => item.name), ["img/a.png", "img/d.png", "img/with space.png"]);
  assert.equal(named(text, assets[0]), "img/a.png");
  assert.equal(named(text, assets[2]), "img/with space.png");
  assert.deepEqual(kindOf(markdownAdapter(text, "a.md"), "include"), []);
});

test("bibliography files come from the front matter, scalar, inline or block", () => {
  const scalar = "---\nbibliography: refs.bib\n---\n";
  const block = '---\nbibliography:\n  - "a.bib"\n  - b.bib\n---\n';
  const inline = "---\nbibliography: [x.bib, y.bib]\n---\n";
  const scalarItems = kindOf(markdownAdapter(scalar, "a.qmd"), "bibliography");
  assert.deepEqual(scalarItems.map((item) => item.name), ["refs.bib"]);
  assert.equal(named(scalar, scalarItems[0]), "refs.bib");

  const blockItems = kindOf(markdownAdapter(block, "a.qmd"), "bibliography");
  assert.deepEqual(blockItems.map((item) => item.name), ["a.bib", "b.bib"]);
  assert.equal(named(block, blockItems[0]), "a.bib");
  assert.equal(named(block, blockItems[1]), "b.bib");

  assert.deepEqual(kindOf(markdownAdapter(inline, "a.md"), "bibliography").map((item) => item.name), ["x.bib", "y.bib"]);
});

test("fenced code gives a code item for the whole fence and a block between the fence lines", () => {
  const text = "---\na: 1\n---\n\n```r\nx <- 1\n```\n";
  const items = markdownAdapter(text, "a.qmd");
  const code = kindOf(items, "code");
  assert.equal(code.length, 2);
  assert.equal(text.slice(...code[0].range), "---\na: 1\n---");
  assert.equal(text.slice(...code[1].range), "```r\nx <- 1\n```");
  const blocks = kindOf(items, "block");
  const front = blocks.find((item) => item.name === "front-matter");
  assert.equal(text.slice(...front.range), "\na: 1\n");
  const fence = blocks.find((item) => item.name === "code");
  assert.equal(text.slice(...fence.range), "\nx <- 1\n");
  assert.equal(fence.attrs.lang, "r");
});

test("an unclosed fence runs to the end of the text", () => {
  const text = "Intro\n\n```\nopen\n";
  const code = kindOf(markdownAdapter(text, "a.md"), "code");
  assert.equal(code.length, 1);
  assert.equal(code[0].range[1], text.length);
});

test("nested fenced divs are blocks, with the class of each", () => {
  const text = "::: outer\n:::: inner\nx\n::::\n:::\n";
  const divs = kindOf(markdownAdapter(text, "a.qmd"), "block").filter((item) => item.name === "div");
  assert.deepEqual(divs.map((item) => item.attrs.class), ["outer", "inner"]);
  const [outer, inner] = divs;
  assert.equal(outer.range[0] < inner.range[0] && inner.range[1] < outer.range[1], true);
  assert.equal(text.slice(...inner.range), "\nx\n");
});

test("math spans are whole, escaped dollars and code are not math", () => {
  const text = "Cost \\$5 and $x^2$ and\n$$\na+b\n$$\n`$y$`\n";
  const math = kindOf(markdownAdapter(text, "a.md"), "math");
  assert.deepEqual(math.map((item) => text.slice(...item.range)), ["$x^2$", "$$\na+b\n$$"]);
});

test("end to end: a two-file Quarto project resolves, diagnoses and lists references", () => {
  const a = [
    "---",
    "bibliography: refs.bib",
    "---",
    "",
    "# Intro {#sec-intro}",
    "",
    "See @fig-plot and @fig-gone [@smith2020].",
    "",
    "{{< include b.qmd >}}",
    "",
    "{{< include nope.qmd >}}",
    "",
  ].join("\n");
  const b = ['```{r}', "#| label: fig-plot", '#| fig-cap: "Plot"', "plot(1)", "```", ""].join("\n");

  const project = createProjectIndex();
  project.setFiles(["a.qmd", "b.qmd"]);
  project.setBibliography(["smith2020"]);
  project.update({ path: "a.qmd", format: "quarto", items: markdownAdapter(a, "a.qmd"), length: a.length });
  project.update({ path: "b.qmd", format: "quarto", items: markdownAdapter(b, "b.qmd"), length: b.length });

  const [refStart] = at(a, "@fig-plot");
  const target = definition(project, "a.qmd", refStart + 2);
  assert.deepEqual(target.map((location) => location.path), ["b.qmd"]);
  assert.equal(b.slice(target[0].from, target[0].to), "fig-plot");

  const [labelStart] = at(b, "fig-plot");
  const uses = references(project, "b.qmd", labelStart + 1);
  assert.equal(uses.some((location) => location.path === "a.qmd"), true);

  const codes = diagnostics(project, "a.qmd").map((diagnostic) => `${diagnostic.code}:${diagnostic.name}`);
  assert.deepEqual(codes.sort(), ["missing-file:nope.qmd", "unknown-label:fig-gone"]);
  assert.deepEqual(diagnostics(project, "b.qmd"), []);
});

test("the tree keeps @keys in code, math and addresses from being citations", () => {
  const text = [
    "Real [@smith] and @jones.",
    "",
    "Inline `@code` and mail a@b.org and $x @inmath y$ here.",
    "",
    "```r",
    "@block",
    "```",
    "",
    "    @indented",
    "",
    "$$",
    "@display",
    "$$",
    "",
  ].join("\n");
  for (const path of ["a.md", "a.qmd"]) {
    const cites = kindOf(markdownAdapter(text, path), "cite");
    assert.deepEqual(cites.map((item) => item.name), ["smith", "jones"]);
    assert.deepEqual(cites.map((item) => named(text, item)), ["smith", "jones"]);
  }
  assert.deepEqual(kindOf(markdownAdapter(text, "a.qmd"), "ref"), []);
});

test("inline code and indented code are code items; a bracketed citation with emphasis still cites", () => {
  const text = "Use `x` here.\n\n    indented\n\n[see *this* @a, $m$ @b]\n";
  const items = markdownAdapter(text, "a.md");
  assert.deepEqual(kindOf(items, "code").map((item) => text.slice(...item.range)), ["`x`", "indented"]);
  assert.deepEqual(kindOf(items, "cite").map((item) => item.name), ["a", "b"]);
  assert.deepEqual(kindOf(items, "math").map((item) => text.slice(...item.range)), ["$m$"]);
});

test("Windows line breaks and front matter keep every offset", () => {
  const text = "---\r\nt: 1\r\n---\r\n\r\nSee [@k] and $x$.\r\n";
  const items = markdownAdapter(text, "a.md");
  const [cite] = kindOf(items, "cite");
  assert.equal(named(text, cite), "k");
  assert.deepEqual(kindOf(items, "math").map((item) => text.slice(...item.range)), ["$x$"]);
});

test("a 300 KB document indexes in under two seconds", () => {
  const paragraph = "Some *prose* with [@key1; @key2], $a_1 + b$ math and `code`, see @fig-a.\n\n::: note\ntext {#sec-x}\n:::\n\n";
  const text = paragraph.repeat(Math.ceil(300000 / paragraph.length));
  const start = performance.now();
  const items = markdownAdapter(text, "big.qmd");
  const elapsed = performance.now() - start;
  assert.ok(items.length > 1000);
  assert.ok(elapsed < 2000, `took ${elapsed} ms`);
});
