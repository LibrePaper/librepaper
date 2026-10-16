// A pending change, drawn in the text it changed.
//
// A tracked edit is in the document at once, and the document also records it
// as a pending change. This draws the record: what the change added,
// underlined where it sits, and what it took out, struck through but still
// legible, because a reviewer deciding whether to lose a sentence needs to
// read the sentence.
//
// Nothing here writes to the document. The inserted text is real text and is
// marked in place. The removed text is not in the document any more, so it is
// a widget hung at the point it was taken from.
//
// The marks are mapped through every edit, so typing next to one does not
// make it flicker, and they are replaced whenever the caller sends a fresh
// list. The caller sends one after every document change, local or imported.
import { Decoration, EditorView, WidgetType } from "@codemirror/view";
import { StateEffect, StateField } from "@codemirror/state";

/// The changes to draw, who is looking, and which change is selected.
///
/// The shape is written out below in a block comment rather than in these
/// ones: `///` is this file's prose, and TypeScript reads only `/** */`.
/// Without it `define()` infers an effect carrying `null`, and every caller
/// is then an error at the point of use rather than here.
/**
 * @typedef {{id: string, who: string, author: string, from: number, to: number, deleted: string}} DrawableChange
 * @type {import("@codemirror/state").StateEffectType<{
 *   changes?: DrawableChange[],
 *   me?: {who: string},
 *   selected?: string | null,
 * }>}
 */
export const setChangeMarks = StateEffect.define();

/// Text a change took out. Shown rather than hidden: the decision is about
/// these words, so they have to be readable while it is being made.
class RemovedText extends WidgetType {
  // What this is, readable without a DOM to render it into.
  kind = "removed";
  constructor(text, change, className) {
    super();
    this.text = text;
    this.change = change;
    this.className = className;
  }
  eq(other) {
    return other.text === this.text && other.change === this.change && other.className === this.className;
  }
  toDOM() {
    const node = document.createElement("del");
    node.className = `change-removed ${this.className}`.trim();
    node.dataset.change = this.change;
    node.textContent = this.text;
    return node;
  }
  ignoreEvent() {
    return false;
  }
}

/// Decorations for one file's changes.
function marksFor(state, changes, me, selected) {
  const marks = [];
  const length = state.doc.length;
  for (const change of changes) {
    if (change.from < 0 || change.to > length || change.to < change.from) continue;
    const mine = change.who === me?.who ? " change-mine" : "";
    const chosen = change.id === selected ? " change-selected" : "";
    if (change.to > change.from) {
      marks.push(
        Decoration.mark({
          class: `change-added${mine}${chosen}`,
          attributes: { title: change.author || "" },
        }).range(change.from, change.to),
      );
    }
    if (change.deleted) {
      marks.push(
        Decoration.widget({
          widget: new RemovedText(change.deleted, change.id, `${mine}${chosen}`.trim()),
          side: -1,
        }).range(change.from),
      );
    }
  }
  // CodeMirror wants them in document order, and a change list is in the
  // order the document keeps its entries.
  marks.sort((a, b) => a.from - b.from || a.value.startSide - b.value.startSide);
  return Decoration.set(marks);
}

/// The editor extension. `setChangeMarks` carries `{changes, me, selected}`,
/// where `changes` is `resolveChanges` for the file on screen.
export function changeMarks() {
  return StateField.define({
    create: () => Decoration.none,
    update(value, transaction) {
      if (transaction.docChanged) value = value.map(transaction.changes);
      for (const effect of transaction.effects) {
        if (!effect.is(setChangeMarks)) continue;
        const { changes = [], me = null, selected = null } = effect.value || {};
        value = marksFor(transaction.state, changes, me, selected);
      }
      return value;
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
