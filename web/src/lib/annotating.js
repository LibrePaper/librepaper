// The vocabulary of making an annotation, as pure functions.
//
// One gesture makes one. A *verb* acts on a passage the reader has already
// selected: the selection is the subject, and Comment and Highlight are what
// can be done to it. There is nothing else -- every annotation is about words
// somebody chose, which is what lets one bar over a selection be the whole of
// the interface.
//
// There were modes once, armed in advance and waiting for a gesture, for the
// two annotations that had nothing to select: a box drawn on a figure and a
// note at a point between two words. A box over a rendered image names no
// range of any source file, and a point cost a second way to locate and a
// second way to follow a range -- a parallel implementation of the hardest
// part of the system, for a note that reads "something belongs here". Both
// are gone; selecting the words beside the place says the same thing.
//
// A third verb went the same way. Suggesting a replacement from the bar was a
// second composer, a second motivation and a second review queue hung off a
// gesture that is otherwise "say something about these words"; proposing text
// is the agent's job, and the Changes panel reviews what it proposes.

/// What the reader can do to a passage they have selected, in the order the
/// bar offers them. `icon` names a Lucide glyph in `Icon.svelte`: the bar is
/// two icons over the words, so the label is what the button is called rather
/// than what it says.
export const VERBS = [
  { id: "comment", label: "Comment", icon: "comment", title: "Comment on the selected passage" },
  { id: "highlight", label: "Highlight", icon: "highlight", title: "Highlight, with no comment" },
];

/// The W3C motivation an annotation is stored under. The server knows nothing
/// about verbs.
export function motivationFor(which) {
  if (which === "highlight") return "highlighting";
  return "commenting";
}
