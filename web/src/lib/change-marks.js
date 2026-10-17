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
// a widget hung at the gap where it was taken from. A gap can hold deletions
// from several authors: each is its own widget, in the document order the gap
// keeps them, so they read in the order they had.
//
// The marks are mapped through every edit, so typing next to one does not
// make it flicker, and they are replaced whenever the caller sends a fresh
// list. The caller sends one after every document change, local or imported.
import { Decoration, EditorView, WidgetType } from "@codemirror/view";
import { StateEffect, StateField } from "@codemirror/state";

/// The changes and gaps to draw, who is looking, and which change is selected.
///
/// The shape is written out below in a block comment rather than in these
/// ones: `///` is this file's prose, and TypeScript reads only `/** */`.
/// Without it `define()` infers an effect carrying `null`, and every caller
/// is then an error at the point of use rather than here.
/**
 * @typedef {{id: string, who: string, author: string, from: number, to: number}} DrawableChange
 * @typedef {{id: string, at: number, fragments: {change: string, text: string}[]}} DrawableGap
 * @type {import("@codemirror/state").StateEffectType<{
 *   changes?: DrawableChange[],
 *   gaps?: DrawableGap[],
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

/// The classes a change's marks carry: whose change it is, and whether it is
/// the selected one.
function stateClasses(change, me, selected) {
  const mine = change.who === me?.who ? " change-mine" : "";
  const chosen = change.id === selected ? " change-selected" : "";
  return `${mine}${chosen}`;
}

/// Decorations for one file's changes and gaps. A change's span is marked in
/// place. Each fragment of a gap is a widget at the gap's point, one per
/// fragment, in document order: the side keeps the widgets in that order.
function marksFor(state, changes, gaps, me, selected) {
  const marks = [];
  const length = state.doc.length;
  const owners = new Map(changes.map((change) => [change.id, change]));
  for (const change of changes) {
    if (change.from < 0 || change.to > length || change.to < change.from) continue;
    if (change.to > change.from) {
      marks.push(
        Decoration.mark({
          class: `change-added${stateClasses(change, me, selected)}`,
          attributes: { title: change.author || "" },
        }).range(change.from, change.to),
      );
    }
  }
  for (const gap of gaps) {
    if (gap.at < 0 || gap.at > length) continue;
    gap.fragments.forEach((fragment, i) => {
      // A fragment whose change is not in the list is drawn as not mine and
      // not selected.
      const owner = owners.get(fragment.change);
      const classes = owner ? stateClasses(owner, me, selected).trim() : "";
      marks.push(
        Decoration.widget({
          widget: new RemovedText(fragment.text, fragment.change, classes),
          side: i - gap.fragments.length,
        }).range(gap.at),
      );
    });
  }
  // CodeMirror wants them in document order. The marks are collected change by
  // change and gap by gap, so they are sorted here.
  marks.sort((a, b) => a.from - b.from || a.value.startSide - b.value.startSide);
  return Decoration.set(marks);
}

/// The editor extension. `setChangeMarks` carries `{changes, gaps, me, selected}`,
/// where `changes` and `gaps` are `resolveFile` for the file on screen.
export function changeMarks() {
  return StateField.define({
    create: () => Decoration.none,
    update(value, transaction) {
      if (transaction.docChanged) value = value.map(transaction.changes);
      for (const effect of transaction.effects) {
        if (!effect.is(setChangeMarks)) continue;
        const { changes = [], gaps = [], me = null, selected = null } = effect.value || {};
        value = marksFor(transaction.state, changes, gaps, me, selected);
      }
      return value;
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
