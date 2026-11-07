import assert from "node:assert/strict";
import test from "node:test";
import { citedKeys, typstLabels } from "../../src/lib/citation-keys.js";
import { typstAdapter } from "../../src/lib/document-index/adapters/typst.js";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { definition, diagnostics } from "../../src/lib/document-index/queries.js";
import { extractOutline } from "../../src/lib/outline.js";
import { skipRanges } from "../../src/lib/spellcheck/index.js";

/** Range of the first `needle` at or after `from`. */
function at(text, needle, from = 0) {
  const start = text.indexOf(needle, from);
  assert.notEqual(start, -1, `missing ${needle}`);
  return [start, start + needle.length];
}

/** Items of one kind, sorted by start. */
function kindOf(items, kind) {
  return items.filter((item) => item.kind === kind).sort((a, b) => a.range[0] - b.range[0]);
}

const docA = [
  '#set page(paper: "a4")',
  '#let version = "1.2"',
  "#let greet(name) = [Hello #name]",
  "",
  "= Introduction <sec:intro>",
  "",
  "Some text with @smith2020 and @fig:a and @sec:intro[Section].",
  "// A comment with @fake and <fake-label>",
  "Raw `@rawref <rawlabel>` and a block:",
  "```typ",
  "= Not a heading <no>",
  "@nope <nolabel>",
  "```",
  "",
  "#figure(",
  '  image("plot.png", width: 80%),',
  "  caption: [A plot of _things_],",
  ") <fig:a>",
  "",
  '#include "chapters/methods.typ"',
  '#bibliography("refs.bib")',
  "#cite(<smith2020>) is cited.",
  "",
  "= Methods",
  "#block[",
  "  Some content",
  "  [nested]",
  "]",
  "",
].join("\n");

const docB = [
  '#import "template.typ": *',
  "= Results <sec:results>",
  "As shown in @fig:a and @tbl:b, @smith2020 agrees.",
  "",
  "#figure(",
  "  rect(width: 2cm),",
  "  caption: [Shapes],",
  ") <fig:a>",
  "",
  '#bibliography(("refs.bib", "extra.bib"))',
  '#include "appendix.typ"',
  "Email me@example.org and see https://example.org/@user.",
  "$ sum_(i) x_i $",
  "",
].join("\n");

/** The unique reference names, cites and labels of one source, for equivalence checks. */
function namesOf(items) {
  return {
    either: new Set(kindOf(items, "ref").filter((r) => r.namespace === "either").map((r) => r.name)),
    cites: new Set(kindOf(items, "cite").map((c) => c.name)),
    labels: new Set(kindOf(items, "label").map((l) => l.name)),
  };
}

for (const [label, text] of [["A", docA], ["B", docB]]) {
  test(`typst adapter reproduces the existing scanners on document ${label}`, () => {
    const items = typstAdapter(text, "main.typ");

    const headings = kindOf(items, "heading")
      .map((item) => ({ title: item.title, level: item.level, from: item.range[0] }))
      .sort((a, b) => a.from - b.from);
    assert.deepEqual(headings, extractOutline(text, "typst").map(({ title, level, from }) => ({ title, level, from })));

    const markup = kindOf(items, "markup").map((item) => item.range);
    assert.deepEqual(markup, skipRanges(text, "typst"));

    assert.deepEqual(namesOf(items).labels, new Set(typstLabels({ "main.typ": text })));

    // citedKeys reads every `@name` as a possible key. Here an either ref is a
    // label or a key, so with no labels given, the adapter's either refs and
    // #cite(<key>) must name the same keys. citedKeys does not read the
    // #cite(label("key")) form or #ref, and no fixture here uses them, and no
    // `@` sits in math, which citedKeys does not mask.
    const names = namesOf(items);
    const adapterKeys = new Set([...names.either, ...names.cites]);
    const citedSet = new Set(citedKeys(text, "typst").map((cite) => cite.key));
    assert.deepEqual(adapterKeys, citedSet);
  });
}

test("headings run to the end of their line and keep the outline's title", () => {
  const text = "= Intro <sec:intro>\n== Sub\n";
  const [first, second] = kindOf(typstAdapter(text, "a.typ"), "heading");
  assert.deepEqual(first.range, [0, text.indexOf("\n")]);
  assert.equal(first.level, 1);
  assert.equal(first.title, "Intro <sec:intro>");
  assert.deepEqual(second.range, [text.indexOf("== Sub"), text.length - 1]);
  assert.equal(second.level, 2);
  assert.equal(second.title, "Sub");
});

test("a label ending a heading line takes the heading title without the label", () => {
  const text = "= Intro <sec:intro>\n";
  const [label] = kindOf(typstAdapter(text, "a.typ"), "label");
  assert.equal(label.title, "Intro");
  assert.deepEqual(label.range, at(text, "<sec:intro>"));
  assert.deepEqual(label.nameRange, at(text, "sec:intro"));
});

test("a label after a figure takes its caption, with nested brackets and whitespace collapsed", () => {
  const text = [
    "#figure(",
    '  image("a.png"),',
    "  caption: [A  plot",
    "     of [things] here],",
    ") <fig:a>",
  ].join("\n");
  const [label] = kindOf(typstAdapter(text, "a.typ"), "label");
  assert.equal(label.name, "fig:a");
  assert.equal(label.title, "A plot of [things] here");
});

test("a label after a figure content block takes the caption of the call", () => {
  const text = "#figure(caption: [Boxes])[\n  content\n] <fig:b>\n";
  const [label] = kindOf(typstAdapter(text, "a.typ"), "label");
  assert.equal(label.name, "fig:b");
  assert.equal(label.title, "Boxes");
});

test("a label with no heading or caption before it has no title", () => {
  const text = "Some paragraph <plain>\n";
  const [label] = kindOf(typstAdapter(text, "a.typ"), "label");
  assert.equal(label.name, "plain");
  assert.equal("title" in label, false);
  assert.deepEqual(label.nameRange, at(text, "plain"));
});

test("an at-reference keeps only the name, without the trailing period or the supplement", () => {
  const text = "See @fig:a[Figure] and @smith.\n";
  const refs = kindOf(typstAdapter(text, "a.typ"), "ref");
  assert.deepEqual(refs.map((r) => r.name), ["fig:a", "smith"]);
  assert.deepEqual(refs.map((r) => r.namespace), ["either", "either"]);
  const [fig, smith] = refs;
  assert.deepEqual(fig.range, at(text, "@fig:a"));
  assert.deepEqual(fig.nameRange, at(text, "fig:a"));
  assert.deepEqual(smith.nameRange, at(text, "smith"));
  assert.deepEqual(smith.range, at(text, "@smith"));
});

test("an at-sign inside a word, an address or a URL is not a reference", () => {
  const text = "Mail me@example.org or see https://example.org/@user.\n";
  assert.deepEqual(kindOf(typstAdapter(text, "a.typ"), "ref"), []);
});

test("refs in math, comments and raw text are not indexed", () => {
  const text = "$@x$ // @y\n`@z` ```\n@w\n```\n";
  assert.deepEqual(kindOf(typstAdapter(text, "a.typ"), "ref"), []);
});

test("#ref(<name>) is a label reference", () => {
  const text = "See #ref(<fig:a>) here.\n";
  const [ref] = kindOf(typstAdapter(text, "a.typ"), "ref");
  assert.equal(ref.namespace, "label");
  assert.equal(ref.name, "fig:a");
  assert.deepEqual(ref.nameRange, at(text, "fig:a"));
  assert.deepEqual(ref.range, at(text, "#ref(<fig:a>)"));
});

test("#cite(<key>) and #cite(label(\"key\")) are citations", () => {
  const text = 'A #cite(<smith2020>) and #cite(label("jones1999")).\n';
  const cites = kindOf(typstAdapter(text, "a.typ"), "cite");
  assert.deepEqual(cites.map((c) => c.name), ["smith2020", "jones1999"]);
  assert.ok(cites.every((c) => c.namespace === "cite"));
  assert.deepEqual(cites[0].nameRange, at(text, "smith2020"));
  assert.deepEqual(cites[0].range, at(text, "#cite(<smith2020>)"));
  assert.deepEqual(cites[1].nameRange, at(text, "jones1999"));
  assert.deepEqual(cites[1].range, at(text, '#cite(label("jones1999"))'));
});

test("include, image and bibliography items carry their paths", () => {
  const text = [
    '#include "chapters/a.typ"',
    '#image("plot.png")',
    'image("b.pdf", width: 1cm)',
    'myimage("not-an-asset.png")',
    '#bibliography(("a.bib", "b.bib"))',
    '#bibliography("refs.bib")',
    "",
  ].join("\n");
  const items = typstAdapter(text, "a.typ");
  const includes = kindOf(items, "include");
  assert.equal(includes.length, 1);
  assert.equal(includes[0].name, "chapters/a.typ");
  assert.deepEqual(includes[0].nameRange, at(text, "chapters/a.typ"));

  const assets = kindOf(items, "asset");
  assert.deepEqual(assets.map((a) => a.name), ["plot.png", "b.pdf"]);
  assert.deepEqual(assets[0].nameRange, at(text, "plot.png"));

  const bibliography = kindOf(items, "bibliography");
  assert.deepEqual(bibliography.map((b) => b.name), ["a.bib", "b.bib", "refs.bib"]);
  assert.deepEqual(bibliography[1].nameRange, at(text, "b.bib"));
});

test("#let names a function or a variable, and needs to start its line", () => {
  const text = [
    "#let greet(name) = [Hello #name]",
    "#let version = \"1.2\"",
    "  #let indented(x) = x",
    "text #let notdefined = 1",
    "",
  ].join("\n");
  const defs = kindOf(typstAdapter(text, "a.typ"), "definition");
  assert.deepEqual(defs.map((d) => d.name), ["greet", "version", "indented"]);
  assert.deepEqual(defs.map((d) => d.attrs.type), ["function", "variable", "function"]);
  assert.deepEqual(defs[0].range, at(text, "#let greet"));
  assert.deepEqual(defs[0].nameRange, at(text, "greet"));
});

test("code and math spans cover the whole construct, and an escaped dollar is not math", () => {
  const text = "A `x` and\n```\nraw\n```\ncosts \\$5 and $x^2$.\n";
  const items = typstAdapter(text, "a.typ");
  assert.deepEqual(kindOf(items, "code").map((c) => c.range), [at(text, "`x`"), at(text, "```\nraw\n```")]);
  assert.deepEqual(kindOf(items, "math").map((m) => m.range), [at(text, "$x^2$")]);
});

test("multi-line bracket and brace blocks opened on a hash line are blocks, single-line ones are not", () => {
  const multi = "#block[\n  one\n]\n";
  const blocks = kindOf(typstAdapter(multi, "a.typ"), "block");
  assert.deepEqual(blocks.map((b) => b.range), [[at(multi, "[")[0] + 1, multi.lastIndexOf("]")]]);

  const figure = "#figure(\n  a,\n)[\n  b\n]\n";
  const figureBlocks = kindOf(typstAdapter(figure, "a.typ"), "block");
  assert.deepEqual(figureBlocks.map((b) => b.range), [[figure.lastIndexOf("[") + 1, figure.lastIndexOf("]")]]);

  const code = "#{\n  let x = 1\n}\n";
  assert.deepEqual(kindOf(typstAdapter(code, "a.typ"), "block").map((b) => b.range), [[2, code.lastIndexOf("}")]]);

  const single = "#block[one line]\n";
  assert.deepEqual(kindOf(typstAdapter(single, "a.typ"), "block"), []);

  const plain = "Text [\n  bracketed\n] here\n";
  assert.deepEqual(kindOf(typstAdapter(plain, "a.typ"), "block"), []);
});

test("a bracket inside a string literal does not close a block", () => {
  const text = '#block[\n  "]"\n]\n';
  const [block] = kindOf(typstAdapter(text, "a.typ"), "block");
  assert.deepEqual(block.range, [text.indexOf("[") + 1, text.lastIndexOf("]")]);
});

test("labels inside comments and raw text are not indexed", () => {
  const text = "// <a>\n`<b>`\n```\n<c>\n```\n<d>\n";
  assert.deepEqual(kindOf(typstAdapter(text, "a.typ"), "label").map((l) => l.name), ["d"]);
});

test("project index resolves typst refs, cites and unknown names end to end", () => {
  const main = "= Intro\n@fig:a and @smith2020 and @nope\n#include \"figs.typ\"\n";
  const figs = '#figure(image("a.png"), caption: [A]) <fig:a>\n';
  const project = createProjectIndex();
  project.setFiles(["main.typ", "figs.typ"]);
  project.setBibliography(["smith2020"]);
  project.update({ path: "main.typ", format: "typst", items: typstAdapter(main, "main.typ"), length: main.length });
  project.update({ path: "figs.typ", format: "typst", items: typstAdapter(figs, "figs.typ"), length: figs.length });

  const [target] = definition(project, "main.typ", at(main, "fig:a")[0] + 1);
  assert.deepEqual(target, { path: "figs.typ", from: at(figs, "fig:a")[0], to: at(figs, "fig:a")[1] });

  assert.equal(project.namespaceOf(kindOf(project.file("main.typ").items, "ref")[1]), "cite");
  assert.deepEqual(definition(project, "main.typ", at(main, "smith2020")[0] + 1), []);

  const found = diagnostics(project, "main.typ");
  assert.deepEqual(found.map((d) => [d.code, d.name]), [["unknown-reference", "nope"]]);
});
