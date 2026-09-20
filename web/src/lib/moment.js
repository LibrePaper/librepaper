// How a moment in a document's past is spelled, when it is not a label.
//
// There are two kinds and one string holds either. A label id names a
// version somebody asked for. A `frontier:` anchor names a position in the
// editing history -- every state the document has ever been in, including the
// ones nobody saved -- and it is what a comment made on the live draft is
// anchored to, because naming the state the room is actually in is free and
// writing a source archive for every remark is not.
//
// This lives on its own because the server writes these strings and two
// unrelated readers have to recognise one: a comment's passage trace, and a
// stale suggestion opened against the draft it was made on.

export const MOMENT = "frontier:";
export const isMoment = (selected) => String(selected || "").startsWith(MOMENT);
export const anchorOf = (selected) => (isMoment(selected) ? String(selected).slice(MOMENT.length) : "");
