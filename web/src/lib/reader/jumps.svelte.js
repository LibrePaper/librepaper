// Where the editor was before a jump, so that Back can return to it.
//
// A jump moves the caret, often into another file: a comment, a change, a
// diagnostic, an outline heading, a click in the preview. The place it leaves
// is remembered here, and Back takes the newest one back and forgets it. Back
// is not a jump, so the place it leaves is not remembered.
//
// The places are the file id and the offset in it. A file that has since
// gone is not a place anyone can return to, so `back` skips those and drops
// them. The page says which files still exist; this module does not know.

/// The jump stack. `limit` is how many places are kept; the oldest is dropped
/// first.
export function createJumps({ limit = 50 } = {}) {
  let places = $state.raw([]);

  return {
    /// Remembers a place. A place equal to the newest one is not stored twice.
    remember(place) {
      const newest = places.at(-1);
      if (newest && newest.file === place.file && newest.offset === place.offset) return;
      const next = [...places, { file: place.file, offset: place.offset }];
      places = next.length > limit ? next.slice(next.length - limit) : next;
    },

    /// Takes the newest place that `exists` accepts, and removes it with any
    /// newer places that were skipped. Null when there is none left.
    /** @param {(file: string) => boolean} [exists] */
    back(exists = () => true) {
      while (places.length) {
        const place = places.at(-1);
        places = places.slice(0, -1);
        if (exists(place.file)) return place;
      }
      return null;
    },

    /// Whether there is a place to return to, as far as the stack knows. It
    /// does not ask `exists`, so it can say yes when `back` will say null.
    get canGoBack() {
      return places.length > 0;
    },

    /// The newest place, for the label of the Back control.
    get last() {
      return places.at(-1) ?? null;
    },
  };
}

/// Whether a key event is the Back chord: Ctrl+Minus on a Mac, where
/// Option+Left moves by word, and Alt+Left everywhere else.
export function isBackChord(event, apple) {
  if (event.metaKey || event.shiftKey) return false;
  if (apple) return event.ctrlKey && !event.altKey && event.key === "-";
  return event.altKey && !event.ctrlKey && event.key === "ArrowLeft";
}
