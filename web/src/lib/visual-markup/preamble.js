import { StateField } from "@codemirror/state";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";

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
  const revealed = state.selection.ranges.some((range) => range.from <= to && range.to >= from);
  if (revealed) return Decoration.none;

  return Decoration.set([
    Decoration.replace({ widget: new PreambleWidget(summary), block: true }).range(from, to),
  ]);
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

class PreambleWidget extends WidgetType {
  constructor(summary) {
    super();
    this.summary = summary;
  }

  eq(other) {
    return other instanceof PreambleWidget && other.summary === this.summary;
  }

  toDOM() {
    const element = document.createElement("div");
    element.className = "cm-lp-preamble";
    element.title = "Show the preamble";
    element.textContent = this.summary;
    return element;
  }

  // A click places the caret at the widget, which reveals the source.
  ignoreEvent() {
    return false;
  }
}
