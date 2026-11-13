import { Facet, StateField } from "@codemirror/state";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";
import { m } from "../paraglide/messages.js";
import { revealingSelection } from "../visual-markup/markup.ts";
import { mergeFolds } from "./merge-folds.js";

/** @typedef {import("./merge-folds.js").Fold} Fold */
export { mergeFolds };

// The finder and the include opener come in through a facet, because the state
// field that draws the folds is shared by every format and cannot be built per
// call. The first configuration wins.
const finderFacet = Facet.define({ combine: (values) => values[0] ?? null });

// A block replacement has to come from a state field, not a view plugin. The
// folds are found only when the text changes; a selection change only redraws.
const foldField = StateField.define({
  create: (state) => foldState(state, findIn(state)),
  update(value, tr) {
    if (!tr.docChanged && !tr.selection) return value;
    return foldState(tr.state, tr.docChanged ? findIn(tr.state) : value.folds);
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
});

/// The fold layer for one format. `findFolds(text)` returns the folds of a
/// document; `openInclude(name)` opens an include, and null leaves out its
/// Open button.
export function foldLayer(findFolds, { openInclude = null } = {}) {
  return [finderFacet.of({ findFolds, openInclude }), foldField];
}

/// The folds in force for a state, drawn or revealed. Other layers skip
/// anything that overlaps them.
export function foldsOf(state) {
  return state.field(foldField, false)?.folds ?? [];
}

// The merged folds of a state's text, from the finder in the facet.
function findIn(state) {
  const text = state.doc.toString();
  return mergeFolds(text, state.facet(finderFacet).findFolds(text));
}

// A fold is drawn unless the selection touches it; a touched fold shows its
// source as ordinary lines, the whole fold at once. A parked caret (the layer
// just turned on) reveals nothing, so a file opens with its preamble folded.
function foldState(state, folds) {
  const { openInclude } = state.facet(finderFacet);
  const ranges = revealingSelection(state);
  const drawn = folds.filter((fold) => !ranges.some((range) => range.from <= fold.to && range.to >= fold.from));
  const decorations = Decoration.set(
    drawn.map((fold) =>
      Decoration.replace({ widget: new FoldWidget(fold, openInclude), block: true }).range(fold.from, fold.to),
    ),
  );
  return { folds, decorations };
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
    const gap = document.createElement("div");
    gap.className = "cm-lp-pill-gap";
    const pill = document.createElement("div");
    pill.className = `cm-lp-pill cm-lp-fold cm-lp-fold-${this.kind}`;
    gap.append(pill);

    const reveal = () => {
      view.dispatch({ selection: { anchor: view.posAtDOM(gap) } });
      view.focus();
    };

    const label = document.createElement("button");
    label.type = "button";
    label.className = "cm-lp-fold-label";
    label.textContent = kindLabel(this.kind);
    label.setAttribute("data-tip", m.common_show_source());
    label.setAttribute("aria-label", m.common_show_source());
    // Keyboard activation sends a click with detail 0; a mouse click was already
    // handled on mousedown.
    label.addEventListener("click", (event) => {
      if (event.detail === 0) reveal();
    });
    pill.append(label);

    if (this.detail) {
      const detail = document.createElement("span");
      detail.className = "cm-lp-fold-detail";
      detail.textContent = this.detail;
      pill.append(detail);
    }

    if (this.opener) {
      const open = document.createElement("button");
      open.type = "button";
      open.className = "cm-lp-fold-open";
      open.textContent = m.common_open();
      // The Open button acts on its own, so it stops the mousedown that would reveal the fold.
      open.addEventListener("mousedown", (event) => {
        event.preventDefault();
        event.stopPropagation();
        this.opener(this.target);
      });
      open.addEventListener("click", (event) => {
        if (event.detail === 0) this.opener(this.target);
      });
      pill.append(open);
    }

    pill.addEventListener("mousedown", (event) => {
      event.preventDefault();
      reveal();
    });
    return gap;
  }

  ignoreEvent() {
    return true;
  }
}
