// Comments and highlights, drawn in the source.
//
// The preview paints each comment over its passage. The source editor does
// the same for the file on screen: a comment is a subtle underline, a
// highlight a translucent wash in its own colour, and the selected one is
// stronger. Clicking a mark selects its comment; the caret still moves.
//
// Nothing here writes to the document. The marks are decorations, mapped
// through every edit, and replaced whenever the caller sends a fresh list.
import { Decoration, EditorView } from "@codemirror/view";
import { StateEffect, StateField } from "@codemirror/state";

/// The marks to draw in the file on screen. Each entry is
/// `{id, from, to, highlight, color, selected}`, with offsets into that file.
export const setCommentMarks = StateEffect.define();

const COLOR = /^#[0-9a-f]{6}$/i;

/// The decorations for one file's marks. An entry with nothing to cover is
/// dropped, and one that runs past the end of the document is clamped to it.
function marksFor(state, list) {
  const length = state.doc.length;
  const marks = [];
  for (const entry of list) {
    const from = Math.max(0, entry.from);
    const to = Math.min(length, entry.to);
    if (!(from < to)) continue;
    const id = String(entry.id);
    const highlight = Boolean(entry.highlight);
    const color = typeof entry.color === "string" && COLOR.test(entry.color) ? entry.color : null;
    const classes = ["cm-comment-mark"];
    if (highlight) classes.push("cm-comment-mark-highlight");
    if (entry.selected) classes.push("cm-comment-mark-selected");
    marks.push({
      from,
      to,
      decoration: Decoration.mark({
        class: classes.join(" "),
        attributes: {
          "data-comment-id": id,
          ...(color && highlight ? { style: `--comment-mark-color: ${color}` } : {}),
        },
      }),
    });
  }
  marks.sort((a, b) => a.from - b.from || a.to - b.to);
  return Decoration.set(marks.map((mark) => mark.decoration.range(mark.from, mark.to)));
}

/// The editor extension: the marks, the click, and their theme. Build one per component, not per file: a state field
/// is identified by its object, so every file's state must share it.
///
/// `onselect(id)` is called with the comment id when a mark is clicked.
export function commentMarks({ onselect } = {}) {
  const field = StateField.define({
    create: () => Decoration.none,
    update(value, transaction) {
      if (transaction.docChanged) value = value.map(transaction.changes);
      for (const effect of transaction.effects) {
        if (!effect.is(setCommentMarks)) continue;
        value = marksFor(transaction.state, effect.value || []);
      }
      return value;
    },
    provide: (f) => EditorView.decorations.from(f),
  });
  const clicks = EditorView.domEventHandlers({
    click(event) {
      const node = event.target?.closest?.("[data-comment-id]");
      if (!node) return false;
      onselect?.(node.dataset.commentId);
      // Not handled here: the click still places the caret.
      return false;
    },
  });
  return [field, clicks, commentMarkTheme];
}

/// How the marks look (included in `commentMarks`). The colours are the app's own tokens, so light and
/// dark themes follow without a rule of their own.
export const commentMarkTheme = EditorView.baseTheme({
  ".cm-comment-mark": {
    textDecoration: "underline",
    textDecorationColor: "var(--color-brand)",
    textUnderlineOffset: "2px",
    textDecorationThickness: "1px",
  },
  ".cm-comment-mark-highlight": {
    borderRadius: "var(--radius-base)",
    background: "color-mix(in srgb, var(--comment-mark-color, var(--color-brand)) 22%, transparent)",
    textDecoration: "none",
  },
  ".cm-comment-mark-selected": {
    outline: "2px solid var(--color-focus)",
    outlineOffset: "1px",
    borderRadius: "var(--radius-base)",
  },
});
