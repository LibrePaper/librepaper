import { StateField } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";
import { DisclosureWidget, PillWidget, revealingSelection } from "./markup.ts";
import { latexCommands, readArguments } from "../latex-language/args.js";
import { findMetadata } from "./metadata.js";
import { m } from "../paraglide/messages.js";

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

  // The first fold is the disclosure that names the document's class and
  // packages; the other folds say there is more.
  return Decoration.set(
    pieces.map(([pieceFrom, pieceTo], index) =>
      index === 0 ? widgetFor(pieceFrom, pieceTo, summary) : pillFor(pieceFrom, pieceTo, m.preamble_more()),
    ),
  );
}

function widgetFor(from, to, meta) {
  return Decoration.replace({
    widget: new DisclosureWidget(m.preamble_summary_label(), meta, m.preamble_show(), "cm-lp-preamble"),
    block: true,
  }).range(from, to);
}

function pillFor(from, to, text) {
  return Decoration.replace({ widget: new PillWidget(text, m.preamble_show(), "cm-lp-preamble"), block: true }).range(from, to);
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
    const { code, commands } = latexCommands(doc.sliceString(0, to));
    if (!commands.some((command) => command.type === "command" && command.name === "documentclass")) return null;
    return { from: 0, to, summary: summarise(code, commands) };
  }
  return null;
}

function summarise(code, commands) {
  let documentClass = null;
  let packages = 0;
  for (const command of commands) {
    if (command.type !== "command" || (command.name !== "documentclass" && command.name !== "usepackage")) continue;
    const args = readArguments(code, command.to);
    const named = args.find((arg) => arg.kind === "group");
    if (command.name === "documentclass" && documentClass === null) {
      documentClass = named ? named.value : "";
    } else if (command.name === "usepackage" && named) {
      packages += named.value.split(",").filter((name) => name.trim() !== "").length;
    }
  }

  // The metadata beside the "Preamble" label: the class when known, the package
  // count when not zero, and nothing when neither is known.
  if (documentClass !== null && documentClass.trim() !== "") {
    return packages > 0 ? m.preamble_meta_class({ documentClass: documentClass.trim(), count: packages }) : documentClass.trim();
  }
  return packages > 0 ? m.preamble_meta_packages({ count: packages }) : "";
}
