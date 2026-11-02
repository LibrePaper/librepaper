// Who wrote each part of the text, drawn in the text.
//
// The spans come from `markSpans` in `authorship.js`, one per run of text by a
// person other than an unknown author. Each is a mark over its characters,
// tinted with that person's colour, with the name and date on hover.
//
// The marks are mapped through every edit, so typing beside one does not make
// it flicker, and they are replaced whenever the caller sends a fresh list.
import { Decoration, EditorView } from "@codemirror/view";
import { StateEffect, StateField } from "@codemirror/state";

/// The spans to draw, or null to clear. `only` is a key from `summarize`: when
/// it is set, spans of every other person are left undrawn.
///
/// The shape is written out below in a block comment rather than in these
/// ones: `///` is this file's prose, and TypeScript reads only `/** */`.
/**
 * @type {import("@codemirror/state").StateEffectType<{
 *   spans: {from: number, to: number, key: string, colour: string, title: string}[],
 *   only?: string | null,
 * } | null>}
 */
export const setAuthorshipMarks = StateEffect.define();

/// Decorations for the spans that are drawn, clamped to the document. A span
/// that is empty after clamping is skipped.
function marksFor(state, { spans, only = null }) {
  const length = state.doc.length;
  const marks = [];
  for (const span of spans) {
    if (only !== null && span.key !== only) continue;
    const from = Math.max(0, Math.min(span.from, length));
    const to = Math.max(0, Math.min(span.to, length));
    if (to <= from) continue;
    marks.push(
      Decoration.mark({
        class: "cm-authorship",
        attributes: { title: span.title, style: `--author: ${span.colour}` },
      }).range(from, to),
    );
  }
  return Decoration.set(marks, true);
}

/// The editor extension. `setAuthorshipMarks` carries `{spans, only}` for the
/// file on screen, or null to clear the marks.
export function authorshipMarks() {
  return StateField.define({
    create: () => Decoration.none,
    update(value, transaction) {
      if (transaction.docChanged) value = value.map(transaction.changes);
      for (const effect of transaction.effects) {
        if (!effect.is(setAuthorshipMarks)) continue;
        value = effect.value === null ? Decoration.none : marksFor(transaction.state, effect.value);
      }
      return value;
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
