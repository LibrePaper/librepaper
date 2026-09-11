// Pure helpers for suggestions (track changes), kept apart from the
// components that use them so the card's diff, the modal's prefill rule and
// the accept/reject state transitions are checkable without a browser.

/// What the compose modal starts the proposal textarea with: the source
/// slice when the passage was placed there, the rendered words otherwise.
export function prefillFor(pending) {
  return pending?.source?.exact ?? pending?.exact ?? "";
}

/// Turns `history.wordDiff`'s UTF-16 edits `{at, delete, insert}` -- sorted
/// and non-overlapping, the way `librepaper-text` produces them -- into runs a
/// card can render without re-deriving offsets: the untouched stretches of
/// `oldText`, the deleted stretches, and the inserted text, in reading order.
/// A suggestion with no proposal comes back as a single "del" run over the
/// whole quotation, since `wordDiff` against an empty string does exactly
/// that -- nothing here special-cases it.
export function runsFor(oldText, edits = []) {
  const text = oldText || "";
  const runs = [];
  let cursor = 0;
  for (const edit of edits) {
    const at = Number(edit.at) || 0;
    const deleted = Number(edit.delete) || 0;
    const insert = typeof edit.insert === "string" ? edit.insert : "";
    if (at > cursor) runs.push({ kind: "same", text: text.slice(cursor, at) });
    if (deleted) runs.push({ kind: "del", text: text.slice(at, at + deleted) });
    if (insert) runs.push({ kind: "ins", text: insert });
    cursor = at + deleted;
  }
  if (cursor < text.length) runs.push({ kind: "same", text: text.slice(cursor) });
  return runs;
}

// How many characters at the end of `a` match the start of `b`, and the
// reverse -- the same tie-break the server runs in `Room::accept_suggestion`
// when a quotation occurs more than once, so a browser preview of a stale
// accept lands on the same place the server would have chosen.
function commonSuffixLength(a, b) {
  let n = 0;
  while (n < a.length && n < b.length && a[a.length - 1 - n] === b[b.length - 1 - n]) n++;
  return n;
}
function commonPrefixLength(a, b) {
  let n = 0;
  while (n < a.length && n < b.length && a[n] === b[n]) n++;
  return n;
}

/// Where a source anchor's quotation sits in `text`, when it is anywhere:
/// the one occurrence when there is only one, otherwise the occurrence whose
/// surrounding text best matches the anchor's prefix and suffix, ties broken
/// by distance from the anchor's position. `null` when the quotation is not
/// in `text` at all.
export function locateInText(text, source) {
  const needle = source?.exact ?? "";
  if (!needle || !text) return null;
  const occurrences = [];
  for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + 1)) {
    occurrences.push(at);
  }
  if (occurrences.length === 0) return null;
  if (occurrences.length === 1) {
    const at = occurrences[0];
    return { start: at, end: at + needle.length };
  }
  const prefix = source?.prefix ?? "";
  const suffix = source?.suffix ?? "";
  const position = Number.isInteger(source?.position) ? source.position : null;
  const scored = occurrences.map((at) => ({
    at,
    score: commonSuffixLength(text.slice(0, at), prefix) + commonPrefixLength(text.slice(at + needle.length), suffix),
    distance: position == null ? 0 : Math.abs(at - position),
  }));
  scored.sort((a, b) => b.score - a.score || a.distance - b.distance);
  const at = scored[0].at;
  return { start: at, end: at + needle.length };
}

/// `text` with the proposal applied at the source anchor, for the preview
/// the merge editor opens on a stale accept. `text` unchanged when the
/// anchor cannot be found in it -- the best this browser can do; the editor
/// still sees the live document on the other side and can apply the change
/// by hand.
export function applyProposal(text, source, proposed) {
  const at = locateInText(text || "", source);
  if (!at) return text || "";
  return text.slice(0, at.start) + (proposed ?? "") + text.slice(at.end);
}

/// The optimistic transition when an editor clicks Accept or Reject: busy,
/// not resolved, so the buttons disable without the card looking settled.
export function beginDeciding(comment, action) {
  comment.deciding = action;
  return comment;
}

/// What an `accept` or `reject` broadcast settles a suggestion to.
export function applyDecision(comment, event, outcome) {
  comment.resolved = true;
  comment.outcome = outcome;
  comment.resolved_in = event.resolved_in;
  comment.resolved_at = event.resolved_at;
  delete comment.deciding;
  return comment;
}

/// An `error` naming this comment clears the busy state without resolving
/// it -- the decision did not happen, whatever the reason.
export function clearDeciding(comment) {
  delete comment.deciding;
  return comment;
}

// Build the frame-facing part of a pending suggestion after its source anchor
// has been resolved and its passage has been projected. This intentionally
// returns data, not HTML: the frame paints the deletion and creates an inert
// text-node insertion. An overlap or an unlocatable anchor is a review-card
// state, never an approximate inline placement.
export function suggestionDisplay(suggestion, anchor = null, {
  deleted = [],
  inserted = [],
  overlap = false,
} = {}) {
  const id = suggestion?.id == null ? "" : String(suggestion.id);
  const source = suggestion?.source || null;
  const exact = source?.exact || suggestion?.exact || "";
  const proposed = typeof suggestion?.proposed === "string" ? suggestion.proposed : "";
  if (!id || !anchor || !Number.isInteger(anchor.start) || !Number.isInteger(anchor.end)
    || anchor.start < 0 || anchor.end < anchor.start || overlap || suggestion?.overlap || suggestion?.unlocatable) {
    return {
      status: "review",
      id,
      exact,
      proposed,
      reason: overlap || suggestion?.overlap ? "overlap" : "unlocatable",
    };
  }
  const deletedTokens = Array.isArray(deleted) ? deleted : [];
  const insertedTokens = Array.isArray(inserted) ? inserted : [];
  const kind = deletedTokens.length && insertedTokens.length ? "replace"
    : deletedTokens.length ? "delete" : "insert";
  return {
    status: "inline",
    id,
    start: anchor.start,
    end: anchor.end,
    kind,
    deleted: deletedTokens,
    inserted: insertedTokens,
    proposed,
    // Explicitly tells the painter that inserted text is synthetic and must
    // not be treated as a target-DOM span.
    syntheticInsertion: true,
  };
}

export const suggestionRedline = suggestionDisplay;
