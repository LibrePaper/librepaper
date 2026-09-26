// A proposed change, drawn in the text it would change.
//
// A proposal is a branch, and reviewing one is reading the difference between
// where it forked and where it has reached (SPEC-loro.md §3.3). This turns that
// difference into something to look at: what it would add, underlined; what it
// would take out, struck through but still legible, because a reviewer deciding
// whether to lose a sentence needs to read the sentence.
//
// The text is not changed. Nothing here writes to the document -- the proposal
// reaches it only when the server resolves it (§5.1a), and until then this is a
// drawing over text that still says what it said.
//
// ## Two bases
//
// A reviewer reads someone else's proposal against the document: the hunks are
// offsets into the proposal's base, which is the text on screen, and a deletion
// covers words that are still there to strike through.
//
// An author writing with track changes on is reading their own branch, because
// that is what the editor is bound to while one is open. Their insertions are
// already in that text and their deletions are already out of it, so the same
// hunks would strike through words that are gone and show every insertion
// twice. `draft` carries the other basis -- `draftMarksOf` in `proposals.js` --
// and the two are drawn side by side here.
//
// ## Offsets
//
// A hunk's `start` is into the proposal's base, and the editor is showing the
// document as it is now. Those are the same text while the base is current, and
// they are not once somebody edits underneath. Rather than draw a suggestion
// somewhere approximate, a proposal whose base has moved is reported as stale
// and drawn not at all: a reviewer agreeing to a change in the wrong place is
// worse than a reviewer being told to look again.
import { Decoration, EditorView, WidgetType } from "@codemirror/view";
import { StateEffect, StateField } from "@codemirror/state";

/// The proposals to draw, and which hunk is being looked at.
///
/// The shape is written out below in a block comment rather than in these
/// ones: `///` is this file's prose, and TypeScript reads only `/** */`.
/// Without it `define()` infers an effect carrying `null`, and every caller
/// is then an error at the point of use rather than here.
/**
 * @typedef {{index: number, start: number, deleted: number, inserted: string, file: string | null}} Hunk
 * @typedef {{id: string, author?: string, hunks: Hunk[]}} DrawableProposal
 * @typedef {{kind: "ins" | "del", file: string | null, at: number, text?: string, length?: number, hunk: number}} DraftMark
 * @type {import("@codemirror/state").StateEffectType<{
 *   proposals?: DrawableProposal[],
 *   showing?: string,
 *   selected?: {proposal: string, hunk: number} | null,
 *   draft?: DraftMark[],
 * }>}
 */
export const setProposalMarks = StateEffect.define();

/// Text a proposal would remove. Shown rather than hidden: the decision is
/// about these words, so they have to be readable while it is being made.
class RemovedText extends WidgetType {
  // What this is, readable without a DOM to render it into.
  kind = "removed";
  constructor(text, proposal, hunk, className = "") {
    super();
    this.text = text;
    this.proposal = proposal;
    this.hunk = hunk;
    this.className = className;
  }
  eq(other) {
    return other.text === this.text && other.hunk === this.hunk && other.className === this.className;
  }
  toDOM() {
    const node = document.createElement("del");
    node.className = `proposal-removed ${this.className}`.trim();
    node.dataset.proposal = this.proposal;
    node.dataset.hunk = String(this.hunk);
    node.textContent = this.text;
    return node;
  }
  ignoreEvent() {
    return false;
  }
}

/// Where a hunk lands in the document, or `null` when it does not.
///
/// The check is the whole of what keeps this honest: the text the hunk expects
/// to replace has to still be there. `deleted` counts UTF-16 units, which is
/// what CodeMirror counts in, so the comparison is between like and like.
function place(document_, hunk) {
  const from = hunk.start;
  const to = from + hunk.deleted;
  if (from < 0 || to > document_.length) return null;
  if (hunk.before !== undefined && document_.sliceString(from, to) !== hunk.before) return null;
  if (hunk.prefix && document_.sliceString(Math.max(0, from - hunk.prefix.length), from) !== hunk.prefix) return null;
  if (hunk.suffix && document_.sliceString(to, to + hunk.suffix.length) !== hunk.suffix) return null;
  return { from, to };
}

/// Decorations for one file's hunks.
function marksFor(state, proposals, showing, selected, draft) {
  const marks = [];
  for (const proposal of proposals) {
    for (const hunk of proposal.hunks) {
      // A hunk belongs to one file, and this editor is showing one file.
      if (hunk.file && showing && hunk.file !== showing) continue;
      const at = place(state.doc, hunk);
      if (!at) continue;
      const chosen = selected?.proposal === proposal.id && selected?.hunk === hunk.index;
      const mine = `proposal-hunk${chosen ? " proposal-selected" : ""}`;
      if (hunk.deleted > 0) {
        marks.push(
          Decoration.replace({
            widget: new RemovedText(state.doc.sliceString(at.from, at.to), proposal.id, hunk.index),
          }).range(at.from, at.to),
        );
      }
      if (hunk.inserted) {
        marks.push(
          Decoration.widget({
            widget: new AddedText(hunk.inserted, proposal.id, hunk.index, mine),
            side: 1,
          }).range(at.to),
        );
      }
    }
  }
  // The author's own draft, which is the text on screen rather than a claim
  // about text elsewhere: an insertion is really there and is marked where it
  // sits, a deletion is not and is hung at the point it was taken from.
  for (const mark of draft) {
    if (mark.file && showing && mark.file !== showing) continue;
    if (mark.kind === "del") {
      if (mark.at < 0 || mark.at > state.doc.length || !mark.text) continue;
      marks.push(
        Decoration.widget({ widget: new RemovedText(mark.text, "", mark.hunk, "proposal-mine"), side: -1 }).range(mark.at),
      );
      continue;
    }
    const to = mark.at + mark.length;
    if (mark.at < 0 || to > state.doc.length || !mark.length) continue;
    marks.push(Decoration.mark({ class: "proposal-added proposal-mine" }).range(mark.at, to));
  }
  // CodeMirror wants them in document order, and a proposal list is in the
  // order the server sent it.
  marks.sort((a, b) => a.from - b.from || a.value.startSide - b.value.startSide);
  return Decoration.set(marks);
}

/// Text a proposal would add. A widget rather than a mark on real text,
/// because these words are not in the document and must not be edited as
/// though they were.
class AddedText extends WidgetType {
  kind = "added";
  constructor(text, proposal, hunk, className) {
    super();
    this.text = text;
    this.proposal = proposal;
    this.hunk = hunk;
    this.className = className;
  }
  eq(other) {
    return other.text === this.text && other.hunk === this.hunk && other.className === this.className;
  }
  toDOM() {
    const node = document.createElement("ins");
    node.className = `proposal-added ${this.className}`;
    node.dataset.proposal = this.proposal;
    node.dataset.hunk = String(this.hunk);
    node.textContent = this.text;
    return node;
  }
  ignoreEvent() {
    return false;
  }
}

/// The editor extension. `setProposalMarks` carries `{proposals, showing,
/// selected}`, where each proposal is `{id, author, hunks}`.
export function proposalMarks() {
  return StateField.define({
    create: () => Decoration.none,
    update(value, transaction) {
      for (const effect of transaction.effects) {
        if (!effect.is(setProposalMarks)) continue;
        const { proposals = [], showing = "", selected = null, draft = [] } = effect.value || {};
        return marksFor(transaction.state, proposals, showing, selected, draft);
      }
      // Anything else that changed the text moved the offsets these were
      // placed at, so they are redrawn from the caller rather than mapped:
      // a hunk is a claim about the base, and mapping it through an edit
      // would quietly turn it into a claim about something else.
      return transaction.docChanged ? Decoration.none : value;
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
