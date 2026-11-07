// Quarto executable cells (```{r}, ```{python}) start with `#|` option lines.
// This highlights them apart from the cell's code, in the editor theme's colors.
import { syntaxTree } from "@codemirror/language";
import { EditorView, Decoration, ViewPlugin } from "@codemirror/view";

const OPTION = /^(\s*)#\|[ \t]*([^\s:][^:]*?)?[ \t]*(?=:|$)/;

/**
 * The `#|` option lines at the top of Quarto cells in [from, to]: `from`/`to`
 * span the line from its `#|`, `keyTo` ends the option name (equal to `to`
 * when the line has no name). Only the contiguous run at the top of a cell counts.
 * @param {import("@codemirror/state").EditorState} state
 * @param {number} [from]
 * @param {number} [to]
 * @returns {Array<{ from: number, to: number, keyTo: number }>}
 */
export function optionRanges(state, from = 0, to = state.doc.length) {
  const ranges = [];
  const { doc } = state;
  syntaxTree(state).iterate({
    from,
    to,
    enter(node) {
      if (node.name !== "FencedCode") return;
      const info = node.node.getChild("CodeInfo");
      const code = node.node.getChild("CodeText");
      if (!info || !code || !doc.sliceString(info.from, info.to).trimStart().startsWith("{")) return false;
      for (let line = doc.lineAt(code.from); line.from < code.to; line = doc.line(line.number + 1)) {
        const match = OPTION.exec(line.text);
        if (!match) break;
        const start = line.from + match[1].length;
        const keyTo = match[2] ? line.from + match[1].length + match[0].slice(match[1].length).trimEnd().length : line.to;
        ranges.push({ from: start, to: line.to, keyTo: Math.min(keyTo, line.to) });
        if (line.number >= doc.lines) break;
      }
      return false;
    },
  });
  return ranges;
}

const optionMark = Decoration.mark({ class: "cm-quarto-option" });
const keyMark = Decoration.mark({ class: "cm-quarto-option-key" });

function decorate(view) {
  const marks = [];
  for (const { from, to } of view.visibleRanges) {
    for (const range of optionRanges(view.state, from, to)) {
      marks.push(optionMark.range(range.from, range.to));
      if (range.keyTo > range.from + 2) marks.push(keyMark.range(range.from + 2, range.keyTo));
    }
  }
  return Decoration.set(marks, true);
}

export const quartoOptions = [
  ViewPlugin.fromClass(class {
    constructor(view) { this.decorations = decorate(view); }
    update(update) {
      if (update.docChanged || update.viewportChanged || syntaxTree(update.state) !== syntaxTree(update.startState)) {
        this.decorations = decorate(update.view);
      }
    }
  }, { decorations: (plugin) => plugin.decorations }),
  EditorView.baseTheme({
    ".cm-quarto-option": { color: "var(--source-highlight-meta)" },
    ".cm-quarto-option-key": { color: "var(--source-highlight-property)" },
  }),
];
