// Owns the replay caches for decorations painted by the document frame.
// A rebuilt frame invalidates these caches; ordinary reactive updates cross
// the frame only when their payload has actually changed.
export function createFrameOverlays({ ready, send }) {
  let selected = null;
  let regions = null;
  let highlights = null;
  let redlines = null;

  function deliver(payload, previous, remember) {
    if (!ready()) return false;
    const encoded = JSON.stringify(payload);
    if (encoded === previous()) return false;
    remember(encoded);
    // The former in-component cache sent the parsed JSON value. Preserve that
    // wire shape as well as using the encoding for equality (notably, omit
    // optional properties whose value is undefined).
    send(JSON.parse(encoded));
    return true;
  }

  function selection(id) {
    if (!ready() || id === selected) return false;
    selected = id;
    send({ type: "select", id });
    return true;
  }

  function annotations(comments) {
    const nextRegions = comments.filter((comment) => comment.region).map((comment) => ({
      id: comment.id,
      point: Boolean(comment.point),
      digest: comment.region.image_digest,
      index: comment.region.image_index,
      x: comment.region.x,
      y: comment.region.y,
      w: comment.region.w,
      h: comment.region.h,
      motivation: comment.motivation,
      resolved: Boolean(comment.resolved),
    }));
    const nextHighlights = comments
      .filter((comment) => !comment.orphaned && comment.start != null)
      .map((comment) => ({
        id: comment.id,
        point: Boolean(comment.point),
        start: comment.start,
        end: comment.end,
        motivation: comment.motivation,
        resolved: Boolean(comment.resolved),
        proposed: comment.proposed ?? "",
        outcome: comment.outcome || "",
        color: comment.color || undefined,
      }));
    const paintedRegions = deliver(
      { type: "regions", regions: nextRegions },
      () => regions,
      (value) => (regions = value),
    );
    const paintedHighlights = deliver(
      { type: "highlight", ranges: nextHighlights },
      () => highlights,
      (value) => (highlights = value),
    );
    return paintedRegions || paintedHighlights;
  }

  function history(message) {
    return deliver(message, () => redlines, (value) => (redlines = value));
  }

  return {
    selection,
    annotations,
    history,
    resetAnnotations() {
      regions = null;
      highlights = null;
    },
    reset() {
      selected = null;
      regions = null;
      highlights = null;
      redlines = null;
    },
  };
}
