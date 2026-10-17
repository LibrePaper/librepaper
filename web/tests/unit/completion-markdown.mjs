// Markdown and Quarto completion: heading anchors, labels, and each completion
// context, run against a real EditorState with a fake project accessor.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { markdownCompletion, markdownLabels } from "../../src/lib/completion/markdown.js";

const ids = (text) => markdownLabels({ "doc.md": text }).map((label) => label.id);

// --- Anchor slugs -----------------------------------------------------------
// Punctuation is dropped, spaces become hyphens.
assert.deepEqual(ids("# Hello, World!"), ["hello-world"]);
// Unicode letters are kept.
assert.deepEqual(ids("## Café au lait"), ["café-au-lait"]);
// Trailing closing hashes and an attribute block without an id are not part of the slug.
assert.deepEqual(ids("## Title ##"), ["title"]);
assert.deepEqual(ids("## Plain {.class}"), ["plain"]);
// An explicit {#id} wins over the slug, and "C#" keeps its hash.
assert.deepEqual(ids("### C# and ## Sharp {#sharp}"), ["sharp"]);
// Duplicates get -1, -2 in order.
assert.deepEqual(ids("## Intro\n## Intro\n## Intro ##"), ["intro", "intro-1", "intro-2"]);
// Headings inside fenced code do not count.
assert.deepEqual(ids("```\n# not a heading\n```\n# Real\n~~~\n# also not\n~~~"), ["real"]);
// Neither does a YAML comment in front matter.
assert.deepEqual(ids("---\ntitle: x\n# not\n---\n# Real"), ["real"]);
// Lines that only look like headings are ignored.
assert.deepEqual(ids("#hashtag\n####### seven"), []);

// The id comes from the heading's text, not its markdown.
assert.deepEqual(ids("# [Intro](other.md)"), ["intro"]);
assert.deepEqual(ids("# The **bold** move"), ["the-bold-move"]);
assert.deepEqual(ids("# Using `npm`"), ["using-npm"]);
assert.deepEqual(ids("# snake_case name"), ["snake_case-name"]);
assert.deepEqual(ids("# A &amp; B"), ["a--b"]);
assert.deepEqual(ids("# ![Logo](a.png) and [ref][1]"), ["logo-and-ref"]);
assert.deepEqual(ids("# It&#39;s <em>here</em>"), ["its-here"]);
assert.deepEqual(ids("# Visit <https://a.b>"), ["visit-httpsab"]);
assert.deepEqual(ids("# __init__ file"), ["init-file"]);

// Labels carry the path and the 1-based line, and skip non-Markdown files.
assert.deepEqual(
  markdownLabels({ "a.md": "# Top\n## Two", "b.txt": "# No", "c.qmd": "# Quarto" }),
  [
    { id: "top", kind: "heading", path: "a.md", line: 1 },
    { id: "two", kind: "heading", path: "a.md", line: 2 },
    { id: "quarto", kind: "heading", path: "c.qmd", line: 1 },
  ],
);

// --- Completion contexts ----------------------------------------------------
// The "|" marks the cursor. Returns the source's result for that position.
function complete(doc, { path = "chapters/intro.md", format = "markdown", texts = {}, files = Object.keys(texts) } = {}) {
  const pos = doc.indexOf("|");
  const text = doc.slice(0, pos) + doc.slice(pos + 1);
  const state = EditorState.create({ doc: text });
  const source = markdownCompletion(() => ({ path, format, texts, files }));
  return { result: source(new CompletionContext(state, pos, false)), pos };
}
const labels = (result) => (result ? result.options.map((option) => option.label) : null);

// Image paths: only image-like files, relative to the edited file's folder.
{
  const { result, pos } = complete("![pic](figs/|", {
    files: ["figs/a.png", "figs/b.txt", "chapters/intro.md", "figs/c.jpg"],
  });
  assert.deepEqual(labels(result), ["../figs/a.png", "../figs/c.jpg"]);
  assert.equal(result.from, pos - "figs/".length);
  assert.ok(result.validFor.test("../figs/a"));
}

// Link targets: project files relative to the edited file, plus this file's anchors.
{
  const { result } = complete("# Overview\n\nSee [here](|", {
    files: ["chapters/other.md", "img.png", "chapters/intro.md"],
  });
  assert.deepEqual(labels(result), ["../img.png", "intro.md", "other.md", "#overview"]);
}

// The file list stops matching at "#", so the anchors are asked for again.
{
  const { result } = complete("[x](oth|", { files: ["chapters/other.md"] });
  assert.ok(result.validFor.test("other.md"));
  assert.equal(result.validFor.test("other.md#"), false);
  assert.equal(result.validFor.test("#"), false);
}

// "other.md#" offers that file's headings, written as typed.
{
  const { result, pos } = complete("[x](other.md#|", {
    texts: { "chapters/other.md": "## Alpha\n## Beta" },
  });
  assert.deepEqual(labels(result), ["other.md#alpha", "other.md#beta"]);
  assert.equal(result.from, pos - "other.md#".length);
}

// A relative path in the link resolves from the edited file's folder.
{
  const { result } = complete("[x](../other.md#|", { texts: { "other.md": "# Top" } });
  assert.deepEqual(labels(result), ["../other.md#top"]);
}

// "#" alone offers this file's anchors.
{
  const { result } = complete("# Overview\n[x](#|");
  assert.deepEqual(labels(result), ["#overview"]);
}

// A link to a file the project does not have offers nothing.
assert.equal(complete("[x](missing.md#|").result, null);

// Reference link ids come from this file's definitions, not footnotes.
{
  const { result, pos } = complete("See [a][|\n\n[one]: http://x\n[Two]: http://y\n[^note]: body");
  assert.deepEqual(labels(result), ["one", "Two"]);
  assert.equal(result.from, pos);
}

// Footnote ids come from this file's footnote definitions.
{
  const { result } = complete("Text[^|\n\n[^note]: body\n[^2]: more\n[plain]: http://x");
  assert.deepEqual(labels(result), ["note", "2"]);
}

// No footnote completion at the start of a line, where a definition is typed.
assert.equal(complete("[^|\n\n[^note]: body").result, null);

// Quarto files get the same contexts.
{
  const { result } = complete("![x](|", { format: "quarto", path: "a.qmd", files: ["a.png"] });
  assert.deepEqual(labels(result), ["a.png"]);
}

// Nothing to complete in plain text.
assert.equal(complete("hello|").result, null);

// Inside a fenced code block.
assert.equal(complete("```\n![x](|\n```").result, null);

// Inside an inline code span.
assert.equal(complete("Use `[a](|`").result, null);

// Other formats never complete here.
assert.equal(complete("[a](|", { format: "latex", files: ["a.tex"] }).result, null);
