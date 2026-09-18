// The vocabulary of making an annotation, as pure functions.
//
// One gesture makes one. A *verb* acts on a passage the reader has already
// selected: the selection is the subject, and Comment, Highlight and Suggest
// are what can be done to it. There is nothing else -- every annotation is
// about words somebody chose, which is what lets one bar over a selection be
// the whole of the interface.
//
// There were modes once, armed in advance and waiting for a gesture, for the
// two annotations that had nothing to select: a box drawn on a figure and a
// note at a point between two words. A box over a rendered image names no
// range of any source file, and a point cost a second way to locate and a
// second way to follow a range -- a parallel implementation of the hardest
// part of the system, for a note that reads "something belongs here". Both
// are gone; selecting the words beside the place says the same thing.

/// What the reader can do to a passage they have selected, in the order the
/// bar offers them. `suggest` is an editor's verb and the caller drops it for
/// anyone who cannot edit the source.
export const VERBS = [
  { id: "comment", label: "Comment", title: "Comment on the selected passage" },
  { id: "highlight", label: "Highlight", title: "Highlight, with no comment" },
  { id: "suggest", label: "Suggest", title: "Suggest a replacement for the selected passage" },
];

/// The W3C motivation an annotation is stored under. The server knows nothing
/// about verbs.
export function motivationFor(which) {
  if (which === "highlight") return "highlighting";
  if (which === "suggest") return "editing";
  return "commenting";
}

/// Whether a draft written under `which` needs the two-field composer: a
/// suggestion is a replacement plus an optional note, everything else is one
/// note.
export function isSuggestion(which) {
  return which === "suggest";
}
