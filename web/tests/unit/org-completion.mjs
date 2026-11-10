// Org completion: citation keys, cross-reference targets, file paths, block
// names and keywords, run against a real EditorState with a fake project.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { orgCompletion, orgLabels } from "../../src/lib/completion/org.js";

const BLOCKS = ["SRC", "QUOTE", "EXAMPLE", "VERSE", "CENTER", "COMMENT", "EXPORT"];

// The "|" marks the cursor. Returns the source's result for that position.
function complete(doc, { path = "notes/main.org", format = "org", texts = {}, files = Object.keys(texts), entries } = {}) {
  const pos = doc.indexOf("|");
  const text = doc.slice(0, pos) + doc.slice(pos + 1);
  const state = EditorState.create({ doc: text });
  const source = orgCompletion(() => ({ path, format, texts, files, entries }));
  return { result: source(new CompletionContext(state, pos, false)), pos };
}
const labels = (result) => (result ? result.options.map((option) => option.label) : null);

// Citation keys: offered inside a [cite:...] group from the bibliography entries.
{
  const entries = [
    { key: "doe2020", authors: ["Jane Doe"], year: 2020, title: "Rivers" },
    { key: "smith2019", authors: ["Ann Smith"], year: 2019, title: "Lakes" },
  ];
  const { result, pos } = complete("See [cite:@d|", { entries });
  assert.deepEqual(labels(result), ["doe2020"]);
  assert.equal(result.from, pos - 1);
  // Without entries there is nothing to offer.
  assert.equal(complete("See [cite:@d|").result, null);
}

// Cross-reference targets: labels, names and custom ids of this file first.
{
  const { result, pos } = complete("[[|\n<<intro>>\n* Method\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n#+NAME: fig1", {
    texts: { "other.org": "<<elsewhere>>" },
  });
  assert.deepEqual(labels(result), ["intro", "#methods", "fig1", "elsewhere"]);
  assert.equal(result.from, pos);
  assert.equal(result.options[1].info, "Method");
}

// Labels in code blocks and comments are not targets.
assert.deepEqual(orgLabels({ "a.org": "#+BEGIN_SRC\n<<hidden>>\n#+END_SRC\n# <<also>>\n<<shown>>" }).map((label) => label.label), ["shown"]);

// File links: paths relative to the file's folder.
{
  const { result } = complete("[[file:|", { files: ["notes/a.org", "notes/img.png", "b.org"] });
  assert.deepEqual(labels(result), ["../b.org", "a.org", "img.png"]);
}

// "./" links: paths starting with "./" unless they climb out of the folder.
{
  const { result } = complete("[[./|", { files: ["notes/a.org", "notes/img.png", "b.org"] });
  assert.deepEqual(labels(result), ["../b.org", "./a.org", "./img.png"]);
}

// Includes and bibliographies: quoted paths in the header.
{
  const { result } = complete('#+INCLUDE: "|', { files: ["notes/a.org", "b.org"] });
  assert.deepEqual(labels(result), ["../b.org", "a.org"]);
}
{
  const { result } = complete("#+BIBLIOGRAPHY: |", { files: ["notes/refs.bib", "refs2.bib", "a.org"] });
  assert.deepEqual(labels(result), ["../refs2.bib", "refs.bib"]);
}

// Block names after "#+BEGIN_" at the start of a line.
{
  const { result, pos } = complete("#+BEGIN_|");
  assert.deepEqual(labels(result), BLOCKS);
  assert.equal(result.from, pos);
}

// Keywords after "#+" at the start of a line.
{
  const { result, pos } = complete("#+TI|");
  assert.ok(labels(result).includes("TITLE:"));
  assert.equal(result.from, pos - 2);
}

// Nothing inside a source block, nor in a Markdown file.
assert.equal(complete("#+BEGIN_SRC org\n[[file:|\n#+END_SRC").result, null);
assert.equal(complete("#+BEGIN_|", { path: "a.md", format: "markdown" }).result, null);
assert.equal(complete("[cite:@d|", { path: "a.md", format: "markdown", entries: [{ key: "doe" }] }).result, null);
assert.equal(complete("[[file:|", { path: "a.md", format: "markdown", files: ["a.png"] }).result, null);
