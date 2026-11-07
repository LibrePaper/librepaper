// The markup the link, heading and list shortcuts write, run against plain
// EditorStates. The builders are pure, so none of these cases needs a view.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { canHeading, canLink, canList, headingEdit, linkEdit, listEdit } from "../../src/lib/editing-shortcuts.js";

/* ------------------------------------------------------------- the helpers */

// A state over `doc`, with the selection from `anchor` to `head`.
const stateOf = (doc, anchor = 0, head = anchor) => EditorState.create({ doc, selection: { anchor, head } });

// Applies a builder's spec and says what the document and the selection are after.
function applied(state, spec) {
  assert.ok(spec, "the builder gave a spec");
  const next = state.update(spec).state;
  const { from, to } = next.selection.main;
  return { doc: next.doc.toString(), anchor: next.selection.main.anchor, head: next.selection.main.head, selected: next.sliceDoc(from, to) };
}

/* -------------------------------------------------------------- the formats */

// Every format a file can be written in has links and lists; LaTeX has headings
// to level five, which is as far as its sectioning commands go.
for (const format of ["latex", "typst", "markdown", "quarto", "html"]) {
  assert.ok(canLink(format), `${format} has links`);
  assert.ok(canList(format), `${format} has lists`);
}
assert.ok(!canLink(""), "a file with no markup format has no link");
assert.ok(!canList(""), "and no list");
assert.ok(canHeading("latex", 5) && !canHeading("latex", 6), "LaTeX stops at \\subparagraph");
assert.ok(canHeading("html", 6) && canHeading("typst", 6) && canHeading("quarto", 6), "the other formats go to level six");
assert.ok(!canHeading("markdown", 0) && !canHeading("markdown", 7), "a heading is level one to six");
assert.equal(linkEdit(stateOf("abc"), ""), null, "a link is unavailable where there is no markup");
assert.equal(headingEdit(stateOf("abc"), "latex", 6), null, "LaTeX has no level six");
assert.equal(listEdit(stateOf("abc"), "", "numbered"), null, "no markup, no list");

/* ----------------------------------------------------------------- the link */

// With a selection, the selected words are the link text and the URL slot is
// the placeholder, selected so that a URL typed over it replaces it.
let link = applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "markdown"));
assert.equal(link.doc, "see [docs](url) now");
assert.equal(link.selected, "url", "the cursor is in the URL slot");

assert.equal(applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "quarto")).doc, "see [docs](url) now", "Quarto writes Markdown links");
assert.equal(applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "latex")).doc, "see \\href{url}{docs} now");
assert.equal(applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "typst")).doc, 'see #link("url")[docs] now');
assert.equal(applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "html")).doc, 'see <a href="url">docs</a> now');
assert.equal(applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "latex")).selected, "url");
assert.equal(applied(stateOf("see docs now", 4, 8), linkEdit(stateOf("see docs now", 4, 8), "html")).selected, "url");

// With no selection, the link is inserted with placeholder text, and the URL
// slot is still the thing selected.
link = applied(stateOf(""), linkEdit(stateOf(""), "markdown"));
assert.equal(link.doc, "[text](url)");
assert.equal(link.selected, "url");
assert.equal(applied(stateOf(""), linkEdit(stateOf(""), "typst")).doc, '#link("url")[text]');

/* --------------------------------------------------------------- headings */

// A line with no heading becomes one, and the caret stays in the text after
// the marker.
let heading = applied(stateOf("Intro\nBody", 0), headingEdit(stateOf("Intro\nBody", 0), "markdown", 2));
assert.equal(heading.doc, "## Intro\nBody");
assert.equal(heading.anchor, 3, "the caret is after the marker, at the start of the text");

// A heading already there is replaced, not nested.
assert.equal(applied(stateOf("## Old", 3), headingEdit(stateOf("## Old", 3), "markdown", 1)).doc, "# Old");
assert.equal(applied(stateOf("# Title"), headingEdit(stateOf("# Title"), "markdown", 3)).doc, "### Title");
assert.equal(applied(stateOf("  Indented"), headingEdit(stateOf("  Indented"), "markdown", 1)).doc, "  # Indented", "the indentation is kept");

// Typst uses equals signs.
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "typst", 2)).doc, "== Intro");
assert.equal(applied(stateOf("= Old"), headingEdit(stateOf("= Old"), "typst", 4)).doc, "==== Old");

// LaTeX levels one to three are the sectioning commands, four and five are the
// paragraph commands, and an existing sectioning command is swapped for the new one.
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "latex", 1)).doc, "\\section{Intro}");
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "latex", 2)).doc, "\\subsection{Intro}");
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "latex", 3)).doc, "\\subsubsection{Intro}");
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "latex", 4)).doc, "\\paragraph{Intro}");
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "latex", 5)).doc, "\\subparagraph{Intro}");
assert.equal(applied(stateOf("\\subsection{Intro}"), headingEdit(stateOf("\\subsection{Intro}"), "latex", 1)).doc, "\\section{Intro}");

// HTML wraps the line in the heading element, and replaces one already there.
assert.equal(applied(stateOf("Intro"), headingEdit(stateOf("Intro"), "html", 3)).doc, "<h3>Intro</h3>");
assert.equal(applied(stateOf("<h1>Intro</h1>"), headingEdit(stateOf("<h1>Intro</h1>"), "html", 2)).doc, "<h2>Intro</h2>");

// The caret stays before a closing tag it was in front of.
assert.equal(applied(stateOf("Intro", 5), headingEdit(stateOf("Intro", 5), "html", 3)).anchor, 9, "the caret is inside <h3>, before </h3>");

// Every selected line is a heading.
assert.equal(applied(stateOf("a\nb", 0, 3), headingEdit(stateOf("a\nb", 0, 3), "markdown", 1)).doc, "# a\n# b");

/* ------------------------------------------------------------------ lists */

// Numbered and bulleted lists, on every selected line.
const two = (doc) => stateOf(doc, 0, doc.length);
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "markdown", "numbered")).doc, "1. a\n2. b");
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "markdown", "bulleted")).doc, "- a\n- b");
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "quarto", "numbered")).doc, "1. a\n2. b", "Quarto writes Markdown lists");
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "typst", "numbered")).doc, "+ a\n+ b", "Typst numbers with plus");
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "typst", "bulleted")).doc, "- a\n- b");

// Applying the list a second time takes it off; a different kind changes it.
assert.equal(applied(two("1. a\n2. b"), listEdit(two("1. a\n2. b"), "markdown", "numbered")).doc, "a\nb", "the same kind again takes it off");
assert.equal(applied(two("1. a\n2. b"), listEdit(two("1. a\n2. b"), "markdown", "bulleted")).doc, "- a\n- b", "a different kind changes it");
assert.equal(applied(two("- a\n- b"), listEdit(two("- a\n- b"), "typst", "bulleted")).doc, "a\nb", "Typst takes its own bullets off");

// Blank lines are not items, and the numbers count items only.
assert.equal(applied(two("a\n\nb"), listEdit(two("a\n\nb"), "markdown", "numbered")).doc, "1. a\n\n2. b");

// The caret follows its text past the marker.
const caret = applied(stateOf("a"), listEdit(stateOf("a"), "markdown", "numbered"));
assert.equal(caret.doc, "1. a");
assert.equal(caret.anchor, 3);

// LaTeX wraps the items in an environment, with an \item on each.
assert.equal(
  applied(two("a\nb"), listEdit(two("a\nb"), "latex", "numbered")).doc,
  "\\begin{enumerate}\n\\item a\n\\item b\n\\end{enumerate}",
);
assert.equal(
  applied(two("a\nb"), listEdit(two("a\nb"), "latex", "bulleted")).doc,
  "\\begin{itemize}\n\\item a\n\\item b\n\\end{itemize}",
);

// Applied to items already inside an environment: the environment is taken off
// on the same kind, and renamed on a different one.
const enumerated = "\\begin{enumerate}\n\\item a\n\\item b\n\\end{enumerate}";
assert.equal(applied(stateOf(enumerated, 18, 33), listEdit(stateOf(enumerated, 18, 33), "latex", "numbered")).doc, "a\nb", "same kind takes the list off");
assert.equal(
  applied(stateOf(enumerated, 18, 33), listEdit(stateOf(enumerated, 18, 33), "latex", "bulleted")).doc,
  "\\begin{itemize}\n\\item a\n\\item b\n\\end{itemize}",
  "a different kind renames the environment",
);

// HTML wraps each item in a list item and the list in <ol> or <ul>.
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "html", "numbered")).doc, "<ol>\n<li>a</li>\n<li>b</li>\n</ol>");
assert.equal(applied(two("a\nb"), listEdit(two("a\nb"), "html", "bulleted")).doc, "<ul>\n<li>a</li>\n<li>b</li>\n</ul>");
const ordered = "<ol>\n<li>a</li>\n<li>b</li>\n</ol>";
assert.equal(applied(stateOf(ordered, 5, 26), listEdit(stateOf(ordered, 5, 26), "html", "numbered")).doc, "a\nb", "same kind takes the HTML list off");
assert.equal(
  applied(stateOf(ordered, 5, 26), listEdit(stateOf(ordered, 5, 26), "html", "bulleted")).doc,
  "<ul>\n<li>a</li>\n<li>b</li>\n</ul>",
  "a different kind renames the HTML list",
);

console.log("editing-shortcuts: links, headings and lists in each format's own markup, toggled and carried by the caret");
