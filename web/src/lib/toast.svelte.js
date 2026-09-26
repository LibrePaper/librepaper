// What the application has to say, said in one place.
//
// A toast is for something that happened out of band and has nowhere else to
// appear: work refused while nobody was looking at the control that started
// it, or a failure whose cause is not the thing on screen. Anything a panel
// or a button already reports belongs there instead. A message said in two
// places is a message the reader learns to skip in both, and the panel is the
// better of the two: it stays, it sits beside what it is about, and it can
// carry a button that retries.
//
// The store is Zag's, so a toast is announced to a screen reader, pauses on
// hover, and stacks with the others rather than replacing them.
import { createToaster } from "@skeletonlabs/skeleton-svelte";

export const toaster = createToaster({
  placement: "bottom-end",
  overlap: true,
  gap: 12,
});

const TYPES = { problem: "error", done: "success", note: "info" };

/// Say one thing, once.
///
/// `kind` is "problem" (something was refused or failed), "done" (something
/// finished with no other sign of having finished) or "note".
///
/// A problem stays until it is dismissed: a message about work that was
/// refused should not vanish while the reader is still looking at what they
/// typed. Everything else goes on its own.
///
/// `id` names the *event*, not the sentence. A line said again under an id
/// that is already up is refreshed in place rather than stacked under its
/// twin, so the two phrasings of one failure -- the sentence written here and
/// whatever the server called it -- are told to the reader once. This matters
/// most for problems, which never expire: without an id a retry loop builds a
/// column of near-identical red cards. Falling back to the text only dedups
/// messages that are constants, which the interesting ones are not.
/** @param {string} text
 *  @param {{ kind?: "problem" | "done" | "note", id?: string }} [options] */
export function say(text, { kind = "note", id } = {}) {
  if (typeof text !== "string" || !text.trim()) return null;
  return toaster.create({
    type: TYPES[kind] || "info",
    description: text,
    id: id || `say:${text}`,
    ...(kind === "problem" ? { duration: Number.POSITIVE_INFINITY } : {}),
  });
}
