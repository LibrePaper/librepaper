// What the application has to say, said in one place.
//
// Before this there were two ways: alert(), which stops the page dead and
// looks like a browser rather than like LibrePaper, and a line of text beside
// the save button that only the editor could use. A toast is neither: it
// appears, it is readable, and it goes.
//
// The store is Zag's, so a toast is announced to a screen reader, pauses on
// hover, and stacks with the others rather than replacing them.
import { createToaster } from "@skeletonlabs/skeleton-svelte";

export const toaster = createToaster({
  placement: "bottom-end",
  overlap: true,
  gap: 12,
});

/// Something went wrong and the reader has to know. Errors stay until they are
/// dismissed: a message about work that was refused should not vanish while
/// the reader is still looking at what they typed.
export const problem = (description) =>
  toaster.create({ type: "error", description, duration: Number.POSITIVE_INFINITY });

/// Something worked. It goes on its own.
export const done = (description) => toaster.create({ type: "success", description });

export const said = (description) => toaster.create({ type: "info", description });
