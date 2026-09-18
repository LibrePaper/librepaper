// How a moment in a document's past is spelled, when it is not a checkpoint.
//
// There are two kinds and one string holds either. A checkpoint id names a
// version somebody asked for. A `frontier:` anchor names a position in the
// editing history -- every state the document has ever been in, including the
// ones nobody saved -- and it is what the activity timeline hands out and what
// a comment made on the live draft is anchored to.
//
// This lives on its own, away from the panel that reads them, because the
// server writes these strings and three unrelated readers have to recognise
// one: the history panel, a comment's passage trace, and a stale suggestion.

export const MOMENT = "frontier:";
export const isMoment = (selected) => String(selected || "").startsWith(MOMENT);
export const anchorOf = (selected) => (isMoment(selected) ? String(selected).slice(MOMENT.length) : "");
