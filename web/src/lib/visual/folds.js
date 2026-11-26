import { Facet, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import { m } from "../paraglide/messages.js";
import { revealingSelection } from "../visual-markup/markup.ts";
import { mergeFolds } from "./merge-folds.js";

/** @typedef {import("./merge-folds.js").Fold} Fold */
export { mergeFolds };

// The finder and the include opener come in through a facet, because the state
// field that draws the folds is shared by every format and cannot be built per
// call. The first configuration wins.
const finderFacet = Facet.define({ combine: (values) => values[0] ?? null });

// The taken ranges of a layer that takes none, shared so that its identity is stable.
const NONE = [];

/// Dispatched when a finder's syntax arrives, so the folds are found again for
/// the same text. A finder that was not ready returned nothing, and nothing was kept.
export const refreshFolds = StateEffect.define();

/// Dispatched with true to show the source of every fold, or false to fold them
/// again. The flag is kept in the field and is not persisted.
/** @type {import("@codemirror/state").StateEffectType<boolean>} */
export const showAllFolds = StateEffect.define();

// The show-all flag on its own, read by the fold layer and by any layer that
// draws folds itself (the paper title block). It depends on nothing, so reading
// it never cycles, and as one shared field it survives a reconfiguration.
const showAllField = StateField.define({
  create: () => false,
  update: (open, tr) => tr.effects.reduce((flag, effect) => (effect.is(showAllFolds) ? effect.value : flag), open),
});

// Finding the folds parses the whole document, about 1.7 ms per keystroke on a
// 1,700-line file in Chrome and several times that in the desktop app's WebKit.
// So an edit does not find them again: the folds in place are mapped through
// the edit, and refreshFolds, sent after a pause in typing, finds them again.
const REFIND_DELAY = 400;

// A block replacement has to come from a state field, not a view plugin. The
// folds are found when refreshFolds arrives and mapped through each edit until
// then; a selection change, or a new set of taken ranges, only redraws.
const foldField = StateField.define({
  create: (state) => foldState(state, findIn(state), foldsAllOpen(state)),
  update(value, tr) {
    const refresh = tr.effects.some((effect) => effect.is(refreshFolds));
    const toggled = tr.effects.some((effect) => effect.is(showAllFolds));
    const open = foldsAllOpen(tr.state);
    const ranges = takenOf(tr.state);
    if (!tr.docChanged && !tr.selection && !refresh && !toggled && ranges === value.taken) return value;
    const folds = refresh ? findIn(tr.state) : tr.docChanged ? mapFolds(tr.changes, value.folds) : value.folds;
    const next = foldState(tr.state, folds, open);
    // A move that leaves the same folds drawn keeps the old set, so nothing is redrawn.
    if (!tr.docChanged && !refresh && !toggled && ranges === value.taken && sameDrawn(next.drawn, value.drawn)) return value;
    return next;
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
});

/// The fold layer for one format. `findFolds(text)` returns the folds of a
/// document; `openInclude(name)` opens an include, and null leaves out its
/// Open button. `taken(state)` returns the ranges another layer draws itself,
/// such as a title block with its own pill; no fold is drawn inside them. Null,
/// the default, takes no ranges.
export function foldLayer(findFolds, { openInclude = null, taken = null } = {}) {
  return [finderFacet.of({ findFolds, openInclude, taken }), showAllField, foldField, refindAfterPause];
}

// The ranges another layer draws, for a state.
function takenOf(state) {
  const { taken } = state.facet(finderFacet);
  return taken ? taken(state) : NONE;
}

// The merged folds of a state's text, from the finder in the facet.
function findIn(state) {
  const text = state.doc.toString();
  return mergeFolds(text, state.facet(finderFacet).findFolds(text));
}

// The folds that survive an edit, moved to where their text now is. A fold the
// edit touched is left out rather than mapped: its text may now read differently
// to its finder, and the refresh after the pause finds it again. Typing does not
// touch a drawn fold, since the caret there reveals it, so this matters for
// remote or programmatic changes. A fold whose whole text was deleted maps to
// nothing and is dropped.
function mapFolds(changes, folds) {
  const mapped = [];
  for (const fold of folds) {
    if (changes.touchesRange(fold.from, fold.to)) continue;
    const from = changes.mapPos(fold.from, 1);
    const to = changes.mapPos(fold.to, -1);
    if (to > from) mapped.push({ ...fold, from, to });
  }
  return mapped;
}

// Sends refreshFolds once typing pauses, so the folds are found again after the
// edits that were mapped through. Dispatching from update() would re-enter the
// view, so the dispatch waits for a timer.
const refindAfterPause = ViewPlugin.fromClass(
  class {
    constructor(view) {
      this.view = view;
      this.timer = undefined;
      this.destroyed = false;
    }

    update(update) {
      if (!update.docChanged) return;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => {
        this.timer = undefined;
        if (!this.destroyed) this.view.dispatch({ effects: refreshFolds.of(null) });
      }, REFIND_DELAY);
    }

    destroy() {
      this.destroyed = true;
      clearTimeout(this.timer);
    }
  },
);

// A fold is drawn unless the selection touches it; a touched fold shows its
// source as ordinary lines, the whole fold at once. A parked caret (the layer
// just turned on) reveals nothing, so a file opens with its preamble folded.
// A fold that lies inside a range another layer draws is not drawn either.
// With `open` every fold shows its source, so none is drawn.
function foldState(state, folds, open) {
  const { openInclude } = state.facet(finderFacet);
  const taken = takenOf(state);
  const revealed = revealingSelection(state);
  const drawn = open
    ? []
    : folds.filter(
        (fold) =>
          !revealed.some((range) => range.from <= fold.to && range.to >= fold.from) &&
          !taken.some((range) => range.from <= fold.from && fold.to <= range.to),
      );
  const decorations = Decoration.set(
    drawn.map((fold) =>
      Decoration.replace({ widget: new FoldWidget(fold, openInclude), block: true }).range(fold.from, fold.to),
    ),
  );
  return { folds, drawn, decorations, taken, open };
}

/// Shows the source of every fold, or folds them again: the editor command.
/// Returns true, as a command does.
export function toggleAllFolds(view) {
  view.dispatch({ effects: showAllFolds.of(!foldsAllOpen(view.state)) });
  return true;
}

/// Whether every fold's source is shown; false where the fold layer is absent.
export function foldsAllOpen(state) {
  return state.field(showAllField, false) ?? false;
}

/// Whether the state carries the fold layer, the visual layer of a format that folds.
export function hasFoldLayer(state) {
  return state.field(foldField, false) !== undefined;
}

function sameDrawn(a, b) {
  return a.length === b.length && a.every((fold, i) => fold === b[i]);
}

function kindLabel(kind) {
  switch (kind) {
    case "preamble":
      return m.folds_preamble();
    case "include":
      return m.folds_include();
    case "comment":
      return m.folds_comment();
    default:
      return m.folds_definitions();
  }
}

/// The pill that stands in for folded source: the element a fold is drawn as.
/// A click, or a keyboard activation of its label, moves the caret to the
/// pill's place in the document, which reveals the source. `fold` gives
/// `kind`, and optionally `detail` and `target`; `opener` is the include
/// opener, or null.
export function foldPill(view, fold, opener = null) {
  const target = fold.target ?? "";
  const includeOpener = fold.kind === "include" && target && opener ? opener : null;
  const gap = document.createElement("div");
  gap.className = "cm-lp-pill-gap";
  const pill = document.createElement("div");
  pill.className = `cm-lp-pill cm-lp-fold cm-lp-fold-${fold.kind}`;
  gap.append(pill);

  const reveal = () => {
    view.dispatch({ selection: { anchor: view.posAtDOM(gap) } });
    view.focus();
  };

  const label = document.createElement("button");
  label.type = "button";
  label.className = "cm-lp-fold-label";
  label.textContent = kindLabel(fold.kind);
  label.setAttribute("data-tip", m.common_show_source());
  label.setAttribute("aria-label", m.common_show_source());
  // Keyboard activation sends a click with detail 0; a mouse click was already
  // handled on mousedown.
  label.addEventListener("click", (event) => {
    if (event.detail === 0) reveal();
  });
  pill.append(label);

  if (fold.detail) {
    const detail = document.createElement("span");
    detail.className = "cm-lp-fold-detail";
    detail.textContent = fold.detail;
    pill.append(detail);
  }

  if (includeOpener) {
    const open = document.createElement("button");
    open.type = "button";
    open.className = "cm-lp-fold-open";
    open.textContent = m.common_open();
    // The Open button acts on its own, so it stops the mousedown that would reveal the fold.
    open.addEventListener("mousedown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      includeOpener(target);
    });
    open.addEventListener("click", (event) => {
      if (event.detail === 0) includeOpener(target);
    });
    pill.append(open);
  }

  pill.addEventListener("mousedown", (event) => {
    event.preventDefault();
    reveal();
  });
  return gap;
}

// A pill that stands in for folded source. A click, a press of the label's
// keyboard activation, moves the caret into the fold, which reveals its source.
class FoldWidget extends WidgetType {
  constructor(fold, opener) {
    super();
    this.kind = fold.kind;
    this.detail = fold.detail ?? "";
    this.target = fold.target ?? "";
    this.opener = fold.kind === "include" && this.target && opener ? opener : null;
  }

  eq(other) {
    return (
      other instanceof FoldWidget &&
      other.kind === this.kind &&
      other.detail === this.detail &&
      other.target === this.target &&
      other.opener === this.opener
    );
  }

  toDOM(view) {
    return foldPill(view, { kind: this.kind, detail: this.detail, target: this.target }, this.opener);
  }

  ignoreEvent() {
    return true;
  }
}
