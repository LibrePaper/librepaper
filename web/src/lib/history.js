// The timeline: what the document used to say, and when.
//
// The server writes a manifest at every checkpoint and serves it whole. This
// module is the reader's side of it: fetching the manifest and one
// checkpoint's files, naming a checkpoint, the word-level diff between two
// versions, and the order the manifest is read in.
//
// The reading of it -- which day a checkpoint fell on, which month the panel
// is showing -- lives next door in `history-calendar.js`, because it draws
// the activity rows beside the checkpoints.

import { SHELL_HEADERS } from "./api.js";
import * as renderers from "./renderers.js";

const asked = (headers) => ({ ...SHELL_HEADERS, ...headers });

/// The history manifest and the independently reported durability boundaries.
/// The endpoint keeps this metadata beside `checkpoints` so old consumers can
/// continue treating the response as a list through `load` below.
export async function loadWithStatus(slug, headers = {}) {
  const points = new Map();
  let cursor = null;
  let durability = null;
  do {
    const response = await fetch(`/api/documents/${slug}/history${cursor === null ? "" : `?after=${encodeURIComponent(cursor)}`}`, {
      headers: asked(headers),
      cache: "no-store",
    });
    if (!response.ok) throw new Error("this document's history is not readable");
    const payload = await response.json();
    if (cursor === null) durability = payload.durability || null;
    for (const point of payload.checkpoints || []) points.set(point.sha, point);
    const next = payload.next_cursor ?? null;
    if (next !== null && (!Number.isSafeInteger(next) || next <= 0 || (cursor !== null && next >= cursor))) {
      throw new Error("The history server returned an invalid page cursor.");
    }
    cursor = next;
  } while (cursor !== null);
  return {
    // Controllers and provenance read oldest first. The paged endpoint reads
    // newest first; normalize once at the boundary, including timestamp ties.
    checkpoints: [...points.values()].sort(checkpointOrder),
    durability: durability && typeof durability === "object" ? durability : null,
  };
}

/// Every checkpoint of a document, oldest first, as the manifest holds them.
/// Kept as a list-returning adapter for callers that predate durability
/// metadata and for small integrations that only need the timeline rows.
export async function load(slug, headers = {}) {
  return (await loadWithStatus(slug, headers)).checkpoints;
}

/// What the document said at one checkpoint: which file was the document, the
/// tree, and the text of every file in it. The figures are named by digest and
/// fetched from where figures always come from, so this is texts alone.
export async function checkpoint(slug, sha, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history/${sha}`, {
    headers: asked(headers),
    // The tree and bodies are immutable, but this response also contains
    // mutable labels and is access-controlled by the current request.
    cache: "no-store",
  });
  if (!response.ok) throw new Error("that checkpoint is not readable");
  return await response.json();
}

/// Names a checkpoint, or takes its name away with an empty one. Editors only,
/// which the server decides and this does not pretend to know.
export async function label(slug, sha, text, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history/${sha}`, {
    method: "PATCH",
    headers: { ...asked(headers), "content-type": "application/json" },
    body: JSON.stringify({ label: text }),
  });
  if (!response.ok) {
    const said = await response.json().catch(() => ({}));
    throw new Error(said.error || "that checkpoint could not be named");
  }
  return await response.json();
}

/// The shortest name for a checkpoint that is still a name: seven characters,
/// which is what git prints and the history API accepts.
export const shortSha = (sha) => (sha || "").slice(0, 7);

/// The shared word-level diff, with a test seam for callers that already have
/// an engine or for checks that do not load a browser worker. The engine's
/// offsets are UTF-16 code units, which are also JavaScript string offsets.
export async function wordDiff(oldText, newText, format = "markdown", services = {}) {
  const compute = services.diff || renderers.diff;
  const edits = await compute(oldText || "", newText || "", format);
  return Array.isArray(edits) ? edits : [];
}

// Keep the shorter name convenient for panels that treat this as the history
// operation, while retaining the explicit name for code that distinguishes it
// from a visual or source diff.
export const diff = wordDiff;

/// Turns UTF-16 edits into displayable hunks. Context is measured in words and
/// is taken from the old and new text independently, so an insertion has old
/// context and a deletion still has the words that survived around it.
export function hunks(oldText, newText, edits = [], context = 6) {
  return edits.map((edit) => {
    const at = Number(edit.at) || 0;
    const deleted = Number(edit.delete) || 0;
    const oldEnd = at + deleted;
    const insert = typeof edit.insert === "string" ? edit.insert : "";
    const oldRange = windowAround(oldText || "", at, oldEnd, context);
    const newAt = newOffset(edits, edit);
    const newRange = windowAround(newText || "", newAt, newAt + insert.length, context);
    return {
      at,
      delete: deleted,
      insert,
      old: (oldText || "").slice(at, oldEnd),
      before: oldRange.before,
      after: oldRange.after,
      current: newRange.value,
      currentBefore: newRange.before,
      currentAfter: newRange.after,
      kind: deleted && insert ? "replace" : deleted ? "delete" : "insert",
    };
  });
}

// The new-text offset of an edit is its old offset plus all prior insertions
// and deletions. Edits from librepaper-text are sorted and non-overlapping.
function newOffset(edits, edit) {
  let offset = Number(edit.at) || 0;
  for (const prior of edits) {
    if (prior === edit) break;
    offset += (prior.insert || "").length - (Number(prior.delete) || 0);
  }
  return offset;
}

function wordStarts(text) {
  const starts = [];
  const pattern = /\S+/g;
  for (let match; (match = pattern.exec(text));) starts.push([match.index, pattern.lastIndex]);
  return starts;
}

function windowAround(text, start, end, context) {
  const words = wordStarts(text);
  const left = words.filter(([, right]) => right <= start).slice(-context);
  const right = words.filter(([at]) => at >= end).slice(0, context);
  const from = left.length ? left[0][0] : 0;
  const to = right.length ? right[right.length - 1][1] : text.length;
  return {
    value: text.slice(start, end),
    before: text.slice(from, start),
    after: text.slice(end, to),
  };
}

/// How much a checkpoint changed the document's size relative to its parent,
/// for the density bar beside its row. `null` when there is nothing to draw:
/// no parent, an unlisted parent, or either end missing its `size`. The width
/// is scaled by the logarithm of the change, not the change itself, because a
/// five-byte fix and a five-kilobyte paste are both worth a bar and neither
/// should swallow the other -- the log keeps a small edit visible and a large
/// one from running off the sidebar. `grew` says which theme colour the bar
/// takes; the width is already rounded to a whole pixel, and the label reads
/// the way a diff stat does.
const SIZE_BAR_MIN = 2;
const SIZE_BAR_MAX = 48;
// Where the log scale saturates: a checkpoint that changed the document by
// this many bytes or more draws the widest bar. Fifty kilobytes is a lot of
// prose to add or remove in one sitting.
const SIZE_BAR_SATURATION = Math.log2(50_000);
export function sizeDelta(point, checkpoints) {
  if (!point?.parent || typeof point.size !== "number") return null;
  const parent = (checkpoints || []).find((candidate) => candidate.sha === point.parent);
  if (!parent || typeof parent.size !== "number") return null;
  const delta = point.size - parent.size;
  if (!delta) return null;
  const magnitude = Math.log2(Math.abs(delta) + 1);
  const width = Math.round(
    Math.max(SIZE_BAR_MIN, Math.min(SIZE_BAR_MAX, (magnitude / SIZE_BAR_SATURATION) * SIZE_BAR_MAX)),
  );
  return {
    grew: delta > 0,
    width,
    title: `${delta > 0 ? "+" : "−"}${Math.abs(delta)} byte${Math.abs(delta) === 1 ? "" : "s"}`,
  };
}

/// The order the manifest is read in, oldest first. The server writes a
/// sequence number and a timestamp, and two checkpoints can share a
/// timestamp, so the sequence decides first and the digest breaks the last
/// tie -- a history whose order depends on the order it arrived in is a
/// history that is wrong about when things happened.
export function checkpointOrder(left, right) {
  if (left.seq > 0 && right.seq > 0 && left.seq !== right.seq) return left.seq - right.seq;
  return (Date.parse(left.at) || 0) - (Date.parse(right.at) || 0) || String(left.sha).localeCompare(String(right.sha));
}

