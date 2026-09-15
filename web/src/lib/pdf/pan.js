// The hand tool, and nothing else.
//
// This was half of `toolbar.js`, which drew the PDF controls inside the frame
// with the operating system's own button colours. The controls are the
// preview header's now -- one row of controls for every format, in the
// application's colours -- and what stayed behind is the only part of them
// that has to run on this side: dragging the page needs the document that is
// being dragged.
import { GrabToPan } from "./vendor/grab_to_pan.js";

/// Grab-to-pan over the whole frame, switched by the header's cursor tools.
export function createPan() {
  const hand = new GrabToPan({ element: document.documentElement });
  return {
    // Panning and selecting are the same gesture, so a selection made with
    // the text tool is dropped rather than left highlighted under a cursor
    // that no longer extends it.
    grab() {
      hand.activate();
      getSelection()?.removeAllRanges();
    },
    select() {
      hand.deactivate();
    },
  };
}
