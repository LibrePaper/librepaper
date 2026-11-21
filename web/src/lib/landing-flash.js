// The short highlight drawn over the line a jump from the preview landed on:
// Show in source, or a double-click in the preview. It is line decoration only;
// it never changes the text, the selection or the caret. The fade is a CSS
// animation that reduced-motion users do not get (see styles/librepaper.css).
// The editor removes the decoration after LANDING_FLASH_MS (Editor.svelte).
import { StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";

export const LANDING_FLASH_MS = 1000;

/** Set a flash on the line holding an offset, or clear it with null. */
/** @type {import("@codemirror/state").StateEffectType<number | null>} */
export const flashLanding = StateEffect.define();

/** The extension: add it once to each editor state. */
export const landingFlash = StateField.define({
  create: () => Decoration.none,
  update(decorations, transaction) {
    let next = decorations.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (!effect.is(flashLanding)) continue;
      next = effect.value === null
        ? Decoration.none
        : Decoration.set([
            Decoration.line({ class: "cm-landing-flash" }).range(transaction.state.doc.lineAt(effect.value).from),
          ]);
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});
