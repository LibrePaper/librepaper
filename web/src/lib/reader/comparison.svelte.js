// The comparison the editor area shows in place of its source: a version of
// the history. Starting one keeps the editor area as it was, so
// that ending it can put that back.
//
// The module owns which comparison is open and the view it saved. It does not
// read or write the Reader: the Reader hands in the function that reads its
// state when a comparison starts, and puts the saved view back itself when
// one ends.

/** @typedef {"" | "history"} ComparisonKind */

/** @returns {{ readonly kind: ComparisonKind, begin: (kind: ComparisonKind, save: () => object) => void, end: () => object | null }} */
export function createComparison() {
  /** @type {ComparisonKind} */
  let kind = $state("");
  let saved = null;
  return {
    get kind() { return kind; },
    // A comparison already open keeps the view from before it started.
    // Moving from one version to another is not a new comparison, and must
    // not save the comparison's own view as the one to return to.
    begin(next, save) {
      if (!kind) saved = save();
      kind = next;
    },
    // The saved view, once. Null when nothing was open.
    end() {
      if (!kind) return null;
      const view = saved;
      kind = "";
      saved = null;
      return view;
    },
  };
}
