// Bold and italic toggles for each source format, run against plain
// EditorStates. The toggle is pure, so none of these cases needs a view.
import assert from "node:assert/strict";
import { EditorSelection, EditorState } from "@codemirror/state";
import { MARKUP, emphasisCommand, inLatexMath, toggleEmphasis } from "../../src/lib/emphasis.js";

/* ------------------------------------------------------------- the helpers */

// A state with one range per [from, to] pair.
function stateOf(doc, pairs, extra = []) {
  return EditorState.create({
    doc,
    selection: EditorSelection.create(pairs.map(([from, to]) => EditorSelection.range(from, to))),
    extensions: [EditorState.allowMultipleSelections.of(true), ...extra],
  });
}

// Applies the toggle and reads back the document and every selection range.
function toggle(doc, pairs, kind, format) {
  const state = stateOf(doc, pairs);
  const spec = toggleEmphasis(state, kind, format);
  assert.ok(spec, `no transaction for ${format} ${kind}`);
  const next = state.update(spec).state;
  return {
    doc: next.doc.toString(),
    ranges: next.selection.ranges.map((range) => [range.from, range.to]),
  };
}

/* ----------------------------------------------------- wrapping a selection */

// "word" is selected in "a word b" in every case, so the inner word always
// starts after the opening marker.
const WRAPPED = [
  ["latex", "bold", "a \\textbf{word} b", [10, 14]],
  ["latex", "italic", "a \\emph{word} b", [8, 12]],
  ["typst", "bold", "a *word* b", [3, 7]],
  ["typst", "italic", "a _word_ b", [3, 7]],
  ["markdown", "bold", "a **word** b", [4, 8]],
  ["markdown", "italic", "a *word* b", [3, 7]],
  ["quarto", "bold", "a **word** b", [4, 8]],
  ["quarto", "italic", "a *word* b", [3, 7]],
  ["html", "bold", "a <strong>word</strong> b", [10, 14]],
  ["html", "italic", "a <em>word</em> b", [6, 10]],
];

for (const [format, kind, doc, range] of WRAPPED) {
  assert.deepEqual(toggle("a word b", [[2, 6]], kind, format), { doc, ranges: [range] }, `${format} ${kind}`);
}

assert.deepEqual(MARKUP.markdown.bold, ["**", "**"]);
assert.deepEqual(MARKUP.latex.mathItalic, ["\\mathit{", "}"]);

/* ------------------------------------------------------------ an empty cursor */

// An empty cursor gets the pair with the cursor between the markers.
assert.deepEqual(toggle("", [[0, 0]], "bold", "markdown"), { doc: "****", ranges: [[2, 2]] });
assert.deepEqual(toggle("", [[0, 0]], "italic", "typst"), { doc: "__", ranges: [[1, 1]] });

// Running it again on the empty pair removes both markers.
assert.deepEqual(toggle("****", [[2, 2]], "bold", "markdown"), { doc: "", ranges: [[0, 0]] });
assert.deepEqual(toggle("__", [[1, 1]], "italic", "typst"), { doc: "", ranges: [[0, 0]] });

/* ------------------------------------------------------------------ unwrap */

// The markers are selected with the text.
assert.deepEqual(toggle("**word**", [[0, 8]], "bold", "markdown"), { doc: "word", ranges: [[0, 4]] });
assert.deepEqual(toggle("_word_", [[0, 6]], "italic", "typst"), { doc: "word", ranges: [[0, 4]] });
assert.deepEqual(toggle("\\textbf{word}", [[0, 13]], "bold", "latex"), { doc: "word", ranges: [[0, 4]] });

// Only the inner text is selected, and the markers sit just outside it.
assert.deepEqual(toggle("**word**", [[2, 6]], "bold", "markdown"), { doc: "word", ranges: [[0, 4]] });
assert.deepEqual(toggle("<em>word</em>", [[4, 8]], "italic", "html"), { doc: "word", ranges: [[0, 4]] });

/* ------------------------------------------------------- several selections */

// Each range is wrapped on its own, and the second one is not shifted by the
// first one's markers.
assert.deepEqual(
  toggle("a b", [[0, 1], [2, 3]], "bold", "typst"),
  { doc: "*a* *b*", ranges: [[1, 2], [5, 6]] },
);

/* ------------------------------------------------------------- LaTeX math */

// The math markup is used inside $...$, \[...\] and a math environment.
assert.deepEqual(toggle("$x$", [[1, 2]], "bold", "latex"), { doc: "$\\mathbf{x}$", ranges: [[9, 10]] });
assert.deepEqual(toggle("$x$", [[1, 2]], "italic", "latex"), { doc: "$\\mathit{x}$", ranges: [[9, 10]] });
assert.deepEqual(toggle("\\[ x \\]", [[3, 4]], "bold", "latex"), { doc: "\\[ \\mathbf{x} \\]", ranges: [[11, 12]] });
assert.deepEqual(
  toggle("\\begin{align} x \\end{align}", [[14, 15]], "bold", "latex"),
  { doc: "\\begin{align} \\mathbf{x} \\end{align}", ranges: [[22, 23]] },
);

// Outside math, the text markup is used. An escaped dollar does not open math,
// and a comment does not either.
assert.deepEqual(toggle("\\$x", [[2, 3]], "bold", "latex"), { doc: "\\$\\textbf{x}", ranges: [[10, 11]] });
assert.deepEqual(toggle("\\$x", [[2, 3]], "italic", "latex"), { doc: "\\$\\emph{x}", ranges: [[8, 9]] });
assert.deepEqual(toggle("% $\nx", [[4, 5]], "bold", "latex"), { doc: "% $\n\\textbf{x}", ranges: [[12, 13]] });

assert.equal(inLatexMath("$x$", 1), true);
assert.equal(inLatexMath("$x$", 3), false);
assert.equal(inLatexMath("\\$x", 2), false);
assert.equal(inLatexMath("% $\nx", 4), false);

/* -------------------------------------------------------------- markdown stars */

// Italic on the inner text of bold adds one star each side.
assert.deepEqual(toggle("**x**", [[2, 3]], "italic", "markdown"), { doc: "***x***", ranges: [[3, 4]] });

// Bold on the inner text of bold-italic removes two stars each side.
assert.deepEqual(toggle("***x***", [[3, 4]], "bold", "markdown"), { doc: "*x*", ranges: [[1, 2]] });

// Italic on the inner text of italic removes one star each side.
assert.deepEqual(toggle("*x*", [[1, 2]], "italic", "markdown"), { doc: "x", ranges: [[0, 1]] });

/* -------------------------------------------------------------- unknown input */

const plain = stateOf("word", [[0, 4]]);
assert.equal(toggleEmphasis(plain, "bold", "word"), null, "unknown format");
assert.equal(toggleEmphasis(plain, "underline", "markdown"), null, "unknown kind");

/* -------------------------------------------------------------- the command */

const view = {
  state: plain,
  dispatched: null,
  dispatch(spec) {
    this.dispatched = spec;
  },
};
assert.equal(emphasisCommand("bold", () => "markdown")(view), true);
assert.equal(view.state.update(view.dispatched).state.doc.toString(), "**word**");
assert.equal(emphasisCommand("bold", () => "word")(view), false, "unknown format");

const locked = {
  state: stateOf("word", [[0, 4]], [EditorState.readOnly.of(true)]),
  dispatch() {
    throw new Error("a read-only editor must not dispatch");
  },
};
assert.equal(emphasisCommand("bold", () => "markdown")(locked), false);
