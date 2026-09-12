// Redlines: turning the history panel's semantic hunks into paint
// instructions for the frame, and working out whose name -- and whose
// colour -- belongs on them.
//
// Pure functions only, so `web/tests/unit/redlines.mjs` can check both without a
// browser: the hunk shapes come from `history.hunks` (see `Reader.svelte`'s
// `computeHistoryChanges`, which stamps a rendered-text `position` onto each
// one), and the checkpoint shape is the manifest's `{sha, at, by, why,
// label}`, oldest first.
import { normalizeAuthorship } from "./provenance.js";

// Who made the change: explicit content-authorship evidence on every
// checkpoint strictly after the baseline, up to and including the compare
// point. Event actors (`by`) and uncaptured live edits do not qualify. One
// name when they all agree, "several people" when they do not, and "" when
// an interval is unknown or has a pruned ancestry edge.
export function attribution(checkpoints = [], baselineSha, targetSha = null) {
  // The uncaptured live interval has no provenance proof.
  if (!targetSha) return "";
  const baselineIndex = checkpoints.findIndex((point) => point.sha === baselineSha);
  if (baselineIndex < 0) return "";
  const endIndex = targetSha
    ? checkpoints.findIndex((point) => point.sha === targetSha)
    : checkpoints.length - 1;
  if (endIndex < baselineIndex) return "";
  const interval = checkpoints.slice(baselineIndex + 1, endIndex + 1);
  const authors = new Set();
  let previous = baselineSha;
  for (const point of interval) {
    if (point.ancestry_gap !== false || point.original_parent !== previous) return "";
    const evidence = normalizeAuthorship(point.authorship);
    if (evidence.kind === "unknown") return "";
    if (evidence.kind === "multiple") authors.add("several people");
    else authors.add(evidence.name);
    previous = point.sha;
  }
  if (authors.has("several people")) return "several people";
  if (authors.size === 0) return "";
  if (authors.size === 1) return [...authors][0];
  return "several people";
}

// A colour per author, keyed by the order they first appear in the manifest
// (oldest first) -- the same rule `History.svelte`'s `authors` derivation
// uses for the dot beside each row (`seen.size % 5`, five being more people
// than a document usually has, a sixth sharing the first's colour). Kept in
// sync by convention, not by sharing code: `History.svelte` reads the
// checkpoints reactively as `$state`, which this module does not touch.
export function authorIndex(checkpoints = []) {
  const seen = new Map();
  for (const point of checkpoints) {
    if (point?.by && !seen.has(point.by)) seen.set(point.by, seen.size % 5);
  }
  return seen;
}

// One item per hunk, in the shape the frame's `redlines()` painter expects:
// `{start, end, kind:"insert", who}` for text now present, `{at, kind:
// "delete", text, who}` for text no longer there. A hunk that both removed
// and added text (`kind: "replace"`) becomes both an insert and a delete
// item sharing one offset -- what was there struck through immediately
// before what replaced it, the way the passage reads.
//
// `who` on the hunk itself -- stamped by `attributeChain`, below -- wins
// when present; `fallback` (ordinarily `attribution()`'s range-level name)
// covers every hunk `attributeChain` had nothing to say about, and every
// caller that never ran it at all.
export function itemsFor(hunks = [], fallback = "") {
  const items = [];
  for (const hunk of hunks) {
    const start = Number(hunk.position) || 0;
    const who = hunk.who ?? fallback;
    if (hunk.kind === "insert" || hunk.kind === "replace") {
      const insert = hunk.insert || "";
      if (insert) items.push({ start, end: start + insert.length, kind: "insert", who });
    }
    if (hunk.kind === "delete" || hunk.kind === "replace") {
      const deleted = hunk.old || "";
      if (deleted) items.push({ at: start, kind: "delete", text: deleted, who });
    }
  }
  return items;
}

// Maps one span (`{start, end}`, half-open, in the coordinates the edits'
// `at` is measured in) forward across a step's edits, the way `history.js`'s
// internal `newOffset` maps a single point. A part of the span an edit
// deletes is dropped rather than carried forward -- that part of the text
// does not survive the step, so whoever wrote it stops being who the
// surviving text is attributed to. `edits` -- and `hunks`, which have the
// same `at`/`delete`/`insert` shape -- are assumed sorted and non-
// overlapping, exactly as `history.hunks`'s own contract promises.
function mapSpanForward(span, edits) {
  let shift = 0;
  let start = span.start;
  const end = span.end;
  const pieces = [];
  for (const edit of edits) {
    if (start >= end) break;
    const editStart = Number(edit.at) || 0;
    const editDelete = Number(edit.delete) || 0;
    const editEnd = editStart + editDelete;
    const delta = (edit.insert || "").length - editDelete;
    if (editEnd <= start) { shift += delta; continue; }
    if (editStart >= end) break;
    if (editStart > start) pieces.push({ start: start + shift, end: editStart + shift });
    shift += delta;
    start = Math.max(start, editEnd);
  }
  if (start < end) pieces.push({ start: start + shift, end: end + shift });
  return pieces;
}

// Maps one point forward the same way, for a deletion's position rather
// than an insertion's span. A point an edit's own deleted range swallows is
// reported as gone (`null`) -- a later step deleted across ground an earlier
// step's deletion sat on, so nothing about that earlier deletion survives to
// attribute.
function mapPointForward(point, edits) {
  let shift = 0;
  for (const edit of edits) {
    const editStart = Number(edit.at) || 0;
    const editDelete = Number(edit.delete) || 0;
    const editEnd = editStart + editDelete;
    if (point < editStart) break;
    if (point < editEnd) return null;
    shift += (edit.insert || "").length - editDelete;
  }
  return point + shift;
}

// The new-text offset of one of a step's own hunks, the same computation
// `history.js`'s private `newOffset` makes: the hunk's old-text offset plus
// every prior hunk in the same step's net length change.
function stepOffset(hunks, hunk) {
  let offset = Number(hunk.at) || 0;
  for (const prior of hunks) {
    if (prior === hunk) break;
    offset += (prior.insert || "").length - (Number(prior.delete) || 0);
  }
  return offset;
}

// Return an author only when the whole span is covered by explicit evidence.
// A largest-overlap winner is not an authorship proof.
function strictCoverage(spans, start, end) {
  if (end <= start) return null;
  const boundaries = new Set([start, end]);
  const relevant = spans.filter((span) => {
    if (span.end <= start || span.start >= end) return false;
    boundaries.add(Math.max(start, span.start));
    boundaries.add(Math.min(end, span.end));
    return true;
  });
  if (!relevant.length) return null;
  const authors = new Set();
  const sorted = [...boundaries].sort((a, b) => a - b);
  for (let index = 0; index < sorted.length - 1; index += 1) {
    const left = sorted[index];
    const right = sorted[index + 1];
    if (right <= left) continue;
    const active = relevant.filter((span) => span.start <= left && span.end >= right);
    if (!active.length) return null;
    for (const span of active) authors.add(span.by);
  }
  if (!authors.size) return null;
  if (authors.has("several people") || authors.size > 1) return "several people";
  return [...authors][0];
}

// Attributes the range-level `hunks` (baseline text against the compare
// point or live text, as `computeChanges` builds them, with `position`
// already stamped) to individual authors, given the chain of diffs between
// each pair of consecutive checkpoints inside the range.
//
// `steps` runs oldest to newest: `[{authorship, hunks}, ...]`, where each
// entry's `hunks` is `history.hunks(oldText, newText, edits)` for that one
// step. The shape already carries `at` (old-text offset), `delete`, `insert`
// and `old` (the removed text), which is everything this function needs to
// walk forward. Insertions are tracked as spans, carried forward through every
// later step -- a span a later step edits is clipped to what survives, and
// the later step's own author claims the new words that replace it, so the
// latest author to touch a passage is the one credited for what is there
// now. Deletions are tracked as points (nothing they removed exists to
// carry forward as a span) and matched to a range-level `delete` hunk by
// final offset and text.
//
// Returns a new array of hunks, each with a `who` added: the attributed
// author when a step accounts for the hunk, `fallback` (ordinarily
// `attribution()`'s range-level name) otherwise -- a hunk no step's diff
// happens to explain, or an empty `steps` list, for instance.
export function attributeChain(steps = [], hunks = [], fallback = "") {
  let insertSpans = [];
  let deleteMarks = [];
  for (const step of steps) {
    // `step.by` is the checkpoint/event actor and is not content evidence.
    const evidence = normalizeAuthorship(step?.authorship || step?.contentAuthorship);
    const by = evidence.kind === "single"
      ? evidence.name
      : evidence.kind === "multiple" ? "several people" : "";
    const stepHunks = Array.isArray(step?.hunks) ? step.hunks : [];
    insertSpans = insertSpans.flatMap((span) =>
      mapSpanForward(span, stepHunks).map((piece) => ({ ...piece, by: span.by })));
    deleteMarks = deleteMarks
      .map((mark) => {
        const at = mapPointForward(mark.at, stepHunks);
        return at === null ? null : { ...mark, at };
      })
      .filter(Boolean);
    for (const hunk of stepHunks) {
      const insert = hunk.insert || "";
      const deleted = hunk.old || "";
      if (!insert && !deleted) continue;
      const at = stepOffset(stepHunks, hunk);
      if (insert) insertSpans.push({ start: at, end: at + insert.length, by });
      if (deleted) deleteMarks.push({ at, text: deleted, by });
    }
  }

  return hunks.map((hunk) => {
    const start = Number(hunk.position) || 0;
    if (hunk.kind === "delete") {
      const text = hunk.old || "";
      const match = deleteMarks.find((mark) => mark.at === start && mark.text === text);
      return { ...hunk, who: match ? match.by : fallback };
    }
    const insert = hunk.insert || "";
    const who = strictCoverage(insertSpans, start, start + insert.length);
    return { ...hunk, who: who ?? fallback };
  });
}
