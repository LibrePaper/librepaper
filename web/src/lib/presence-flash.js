// The short highlight drawn over a collaborator's place after a click on their
// avatar. It is line decoration only: it never changes the text, the selection
// or the caret. The colour is the person's own, through the `user-color-`
// class, and the fade is a CSS animation that reduced-motion users do not get
// (see styles/librepaper.css).
import { StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";

export const FLASH_MS = 1400;
// Lines past this are not highlighted; a range that spans a whole chapter
// would paint the screen.
const MAX_LINES = 40;

/** @typedef {{ from: number, to: number, className: string } | null} FlashValue */
/** Set a flash, or clear it with null. */
/** @type {import("@codemirror/state").StateEffectType<{ from: number, to: number, className: string }>} */
export const flashPresence = StateEffect.define();

function flashDecorations(doc, { from, to, className }) {
  const first = doc.lineAt(from).number;
  const last = Math.min(doc.lineAt(to).number, first + MAX_LINES - 1);
  const ranges = [];
  for (let number = first; number <= last; number++) {
    ranges.push(Decoration.line({ class: `peer-jump-flash ${className}`.trim() }).range(doc.line(number).from));
  }
  return Decoration.set(ranges, true);
}

/** The extension: add it once to each editor state. */
export const presenceFlash = StateField.define({
  create: () => Decoration.none,
  update(decorations, transaction) {
    let next = decorations.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (!effect.is(flashPresence)) continue;
      next = effect.value ? flashDecorations(transaction.state.doc, effect.value) : Decoration.none;
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});
