// The vocabulary of making an annotation, as pure functions.
//
// Two kinds of gesture make one, and keeping them apart is the whole point.
// A *verb* acts on a passage the reader has already selected: the selection
// is the subject, and Comment, Highlight and Suggest are what can be done to
// it. A *mode* is armed first and waits for a gesture, because what it
// annotates cannot be selected -- a point between two words, or a box drawn
// on a figure. Only those two are modes; everything else lives on the bar
// that appears over a selection.

/// What the reader can do to a passage they have selected, in the order the
/// bar offers them. `suggest` is an editor's verb and the caller drops it for
/// anyone who cannot edit the source.
export const VERBS = [
  { id: "comment", label: "Comment", title: "Comment on the selected passage" },
  { id: "highlight", label: "Highlight", title: "Highlight, with no comment" },
  { id: "suggest", label: "Suggest", title: "Suggest a replacement for the selected passage" },
];

/// The two annotations that have nothing to select, and so have to be armed.
/// `region` is offered only by a document that has a figure to draw on.
export const MODES = [
  { id: "point", icon: "text-cursor", label: "Note at a point", title: "Then click a place in the document" },
  { id: "region", icon: "box", label: "Box on a figure", title: "Then drag a box on a figure" },
];

/// The W3C motivation an annotation is stored under. A point note and a box
/// are comments; they differ in what they are anchored to, not in what they
/// are. The server knows nothing about verbs or modes.
export function motivationFor(which) {
  if (which === "highlight") return "highlighting";
  if (which === "suggest") return "editing";
  return "commenting";
}

/// Arming a mode from a click on it: choosing the one already armed puts it
/// away again. A mode that is never disarmed by the control that armed it is
/// a trap, and both of these change what a click or a drag in the document
/// does -- `region` stops text being selectable at all.
export function nextMode(current, chosen) {
  return current === chosen ? "" : chosen;
}

/// Whether a draft written under `which` needs the two-field composer: a
/// suggestion is a replacement plus an optional note, everything else is one
/// note.
export function isSuggestion(which) {
  return which === "suggest";
}

/// What the bar offers over what has been selected. A point in the text and a
/// box on a figure have no words: nothing to paint a highlight over, and
/// nothing to propose a replacement for, so both are Comment alone.
export function verbsFor(pending) {
  if (!pending) return [];
  return pending.point || pending.region ? VERBS.filter((verb) => verb.id === "comment") : VERBS;
}
