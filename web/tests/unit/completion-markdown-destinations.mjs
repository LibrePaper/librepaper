// Link destinations written by completion: file names with spaces, parentheses,
// "#" and "%" are escaped or wrapped. Runs against a real EditorState.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { markdownCompletion } from "../../src/lib/completion/markdown.js";

function complete(doc, { path = "doc.md", texts = {}, files = Object.keys(texts) } = {}) {
  const pos = doc.indexOf("|");
  const text = doc.slice(0, pos) + doc.slice(pos + 1);
  const state = EditorState.create({ doc: text });
  const source = markdownCompletion(() => ({ path, format: "markdown", texts, files }));
  return { result: source(new CompletionContext(state, pos, false)), pos };
}

const option = (result, label) => result.options.find((item) => item.label === label);

// A space or parenthesis wraps the destination in angle brackets. The label stays plain.
{
  const { result } = complete("![x](|", { files: ["figs/plot (2).png"] });
  const plot = option(result, "figs/plot (2).png");
  assert.equal(plot.apply, "<figs/plot (2).png>");
  assert.equal(plot.label, "figs/plot (2).png");
}

// "#" is percent-encoded.
{
  const { result } = complete("![x](|", { files: ["a#b.png"] });
  assert.equal(option(result, "a#b.png").apply, "a%23b.png");
}

// A plain name applies as it is.
{
  const { result } = complete("![x](|", { files: ["plain.png"] });
  assert.equal(option(result, "plain.png").apply, "plain.png");
}

// A "%" that does not start an escape is encoded, and a real escape is kept.
{
  const { result } = complete("[x](|", { files: ["100%.pdf", "50%20off.pdf"] });
  assert.equal(option(result, "100%.pdf").apply, "100%25.pdf");
  assert.equal(option(result, "50%20off.pdf").apply, "50%2520off.pdf");
}

// Link targets get the same forms.
{
  const { result } = complete("[x](|", { files: ["notes (draft)#1.md"] });
  assert.equal(option(result, "notes (draft)#1.md").apply, "<notes (draft)%231.md>");
}

// Inside an open "<", the partial starts after the bracket and the file applies
// with a closing bracket and no second opening one.
{
  const { result, pos } = complete("![a](<fig|", { files: ["figs/x.png"] });
  assert.equal(result.from, pos - "fig".length);
  assert.equal(option(result, "figs/x.png").apply, "figs/x.png>");
  assert.ok(result.validFor.test("my fi"));
}

// A spaced partial inside "<" still completes.
{
  const { result, pos } = complete("![a](<figs/my |", { files: ["figs/my pic.png"] });
  assert.equal(result.from, pos - "figs/my ".length);
  assert.equal(option(result, "figs/my pic.png").apply, "figs/my pic.png>");
}

// An anchor target in an open "<" resolves its file and closes the bracket.
{
  const { result } = complete("[x](<other.md#|", { texts: { "other.md": "## Beta" } });
  assert.equal(option(result, "other.md#beta").apply, "other.md#beta>");
}

// A percent-encoded file name in an anchor target is decoded before lookup.
{
  const { result } = complete("[x](my%20other.md#|", { texts: { "my other.md": "## Alpha" } });
  assert.deepEqual(result.options.map((item) => item.label), ["my%20other.md#alpha"]);
}

// A plain partial still completes after the angle-bracket change.
{
  const { result } = complete("![a](fi|", { files: ["figs/x.png"] });
  assert.ok(result);
  assert.equal(option(result, "figs/x.png").apply, "figs/x.png");
}

// A raw "%" in a name survives the round trip: the applied text decodes back to
// the file, so anchor completion finds it.
{
  const texts = { "50%20off.md": "## Deal" };
  const { result } = complete("[x](50%2520off.md#|", { texts, files: Object.keys(texts) });
  assert.deepEqual(result.options.map((item) => item.label), ["50%2520off.md#deal"]);
}

// A "?" in a name is not a query.
{
  const { result } = complete("[x](|", { files: ["notes?draft.md"] });
  assert.equal(option(result, "notes?draft.md").apply, "notes%3Fdraft.md");
}
