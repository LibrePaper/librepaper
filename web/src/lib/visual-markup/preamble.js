import { StateField } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";
import { PillWidget, revealingSelection } from "./markup.ts";
import { findMetadata } from "./metadata.js";

// The preamble is looked for in the first lines only; a document whose
// \begin{document} is further down is left unfolded.
const SEARCH_LINES = 2000;

/// Folds the LaTeX preamble into one line that names its class and packages.
/// The source returns while the selection touches the preamble, so it can be
/// edited.
export function preambleFold() {
  return [preambleField];
}

// A block replacement has to come from a state field, not a view plugin.
const preambleField = StateField.define({
  create: (state) => preambleDecorations(state),
  update(decorations, tr) {
    if (!tr.docChanged && !tr.selection) return decorations;
    return preambleDecorations(tr.state);
  },
  provide: (field) => EditorView.decorations.from(field),
});

function preambleDecorations(state) {
  const preamble = findPreamble(state.doc);
  if (!preamble) return Decoration.none;

  const { from, to, summary } = preamble;
  // Atomic ranges are left out on purpose: a caret moved into the preamble
  // reveals it.
  const touching = revealingSelection(state).filter((range) => range.from <= to && range.to >= from);
  if (touching.length === 0) return foldAll(from, to, summary);

  // A caret on a metadata line (\title, \author and the like) reveals that line
  // alone. Any other selection in the preamble reveals all of it.
  const lines = metadataLines(state.doc, to);
  const onLine = (range, line) => range.from >= line.from && range.to <= line.to;
  if (!touching.every((range) => lines.some((line) => onLine(range, line)))) return Decoration.none;

  const revealed = mergeLines(lines.filter((line) => touching.some((range) => onLine(range, line))));
  return foldAround(revealed, from, to, summary);
}

function foldAll(from, to, summary) {
  return Decoration.set([widgetFor(from, to, summary)]);
}

// The folds that cover the preamble except for the revealed lines. Each fold
// stops at the end of the line before a revealed line and starts at the start
// of the line after it, so no fold covers the caret's line or its line break.
function foldAround(revealed, from, to, summary) {
  const pieces = [];
  let start = from;
  for (const line of revealed) {
    if (line.from - 1 >= start) pieces.push([start, line.from - 1]);
    start = line.to + 1;
  }
  if (start <= to) pieces.push([start, to]);

  // The summary names the document's class and packages; the other folds say so.
  return Decoration.set(
    pieces.map(([pieceFrom, pieceTo], index) =>
      widgetFor(pieceFrom, pieceTo, index === 0 ? summary : "More preamble"),
    ),
  );
}

function widgetFor(from, to, text) {
  return Decoration.replace({ widget: new PillWidget(text, "Show the preamble", "cm-lp-preamble"), block: true }).range(from, to);
}

// The lines that hold a metadata command in the preamble, widened to whole lines.
function metadataLines(doc, to) {
  return findMetadata(doc.sliceString(0, to)).map((command) => ({
    from: doc.lineAt(command.from).from,
    to: doc.lineAt(command.to).to,
  }));
}

// Overlapping lines become one, sorted by position.
function mergeLines(lines) {
  const merged = [];
  for (const line of [...lines].sort((a, b) => a.from - b.from)) {
    const last = merged.at(-1);
    if (last && line.from <= last.to) last.to = Math.max(last.to, line.to);
    else merged.push({ ...line });
  }
  return merged;
}

// The preamble runs from the start of the document to the line before
// \begin{document}. Null when there is no preamble to fold.
function findPreamble(doc) {
  const last = Math.min(doc.lines, SEARCH_LINES);
  for (let number = 1; number <= last; number++) {
    const line = doc.line(number);
    const at = line.text.indexOf("\\begin{document}");
    // A commented-out \begin{document} does not end the preamble.
    if (at === -1 || line.text.slice(0, at).includes("%")) continue;

    if (number === 1) return null;
    const to = doc.line(number - 1).to;
    const text = withoutComments(doc.sliceString(0, to));
    if (!/\\documentclass\b/.test(text)) return null;
    return { from: 0, to, summary: summarise(text) };
  }
  return null;
}

function summarise(text) {
  const documentClass = /\\documentclass\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}/.exec(text);
  let packages = 0;
  for (const match of text.matchAll(/\\usepackage\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}/g)) {
    packages += match[1].split(",").filter((name) => name.trim() !== "").length;
  }

  const parts = [];
  if (documentClass && documentClass[1].trim() !== "") parts.push(documentClass[1].trim());
  parts.push(`${packages} ${packages === 1 ? "package" : "packages"}`);
  return `Preamble: ${parts.join(", ")}`;
}

// Drops the text of each % comment, so a commented-out package is not counted.
function withoutComments(text) {
  return text.replace(/(^|[^\\])%.*$/gm, "$1");
}
