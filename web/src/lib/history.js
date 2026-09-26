// The timeline: what the document used to say, and when.
//
// The server writes a manifest at every label and serves it whole. This
// module is the reader's side of it: fetching the manifest and one
// label's files, naming a label, the word-level diff between two
// versions, and the order the manifest is read in.
//
// The reading of it -- which day a label fell on, which month the panel
// is showing -- lives next door in `history-calendar.js`.

import { SHELL_HEADERS } from "./api.js";
import * as renderers from "./renderers.js";

const asked = (headers) => ({ ...SHELL_HEADERS, ...headers });

/// A `document_labels` row (`handle_history`'s `label_wire`, room-v2.md's
/// timeline) turned into what the rest of this panel reads. The wire names
/// are the storage row's own -- `sequence`, `reason` -- and the panel and the
/// calendar coarsening both read `seq` and `why`, the names this codebase
/// used before checkpoints and labels were the same table; normalizing once
/// here means neither of those has to know the wire changed under it.
/// `changed`, the paths a version touched, no longer exists on the wire at
/// all (§8.2: the manifest is a moment, not a diff), so nothing here invents
/// one -- a caller that reads `point.changed` sees `undefined`, exactly as it
/// would for a label this server never annotated with one.
function fromWire(row) {
  return {
    sha: row.sha,
    seq: row.sequence,
    at: row.at,
    by: row.by,
    label: row.label,
    why: row.reason,
    tree_sha: row.tree_sha,
    frontier: row.frontier,
    archive_status: row.archive_status,
  };
}

/// The history manifest, oldest first.
export async function loadWithStatus(slug, headers = {}) {
  const points = new Map();
  let cursor = null;
  do {
    const response = await fetch(`/api/documents/${slug}/history${cursor === null ? "" : `?after=${encodeURIComponent(cursor)}`}`, {
      headers: asked(headers),
      cache: "no-store",
    });
    if (!response.ok) throw new Error("this document's history is not readable");
    const payload = await response.json();
    for (const row of payload.labels || []) points.set(row.sha, fromWire(row));
    const next = payload.next_cursor ?? null;
    if (next !== null && (!Number.isSafeInteger(next) || next <= 0 || (cursor !== null && next >= cursor))) {
      throw new Error("The history server returned an invalid page cursor.");
    }
    cursor = next;
  } while (cursor !== null);
  return {
    // Controllers and provenance read oldest first. The paged endpoint reads
    // newest first; normalize once at the boundary, including timestamp ties.
    labels: [...points.values()].sort(labelOrder),
  };
}

/// Every label of a document, oldest first, as the manifest holds them.
/// Kept as a list-returning adapter for small integrations that only need
/// the timeline rows.
export async function load(slug, headers = {}) {
  return (await loadWithStatus(slug, headers)).labels;
}

/// What the document said at one label: which file was the document, the
/// tree, and the text of every file in it. The figures are named by digest and
/// fetched from where figures always come from, so this is texts alone.
///
/// A file entry on the wire is `librepaper_document_core::projection::Entry`,
/// whose digest field is spelled `digest`. The rest of this codebase's own
/// trees -- what `session.tree()` hands the comparison as the live side --
/// spell an asset's digest `sha` (see `Reader.svelte`'s own tree builder).
/// `history-source.svelte.js` compares both sides with one `entryOf`, so
/// without this the label side of every asset compared silently read
/// `undefined` where the live side read a real digest.
export async function read(slug, sha, headers = {}) {
  // `sha` is a label UUID, but it can also be a comment's own
  // `frontier:<base64>` moment (`passages.js`'s `wentAt`), whose base64
  // alphabet includes characters a path segment cannot carry unescaped.
  const response = await fetch(`/api/documents/${slug}/history/${encodeURIComponent(sha)}`, {
    headers: asked(headers),
    // The tree and bodies are immutable, but this response also contains
    // mutable labels and is access-controlled by the current request.
    cache: "no-store",
  });
  if (!response.ok) throw new Error("that label is not readable");
  const payload = await response.json();
  const files = payload.files && typeof payload.files === "object"
    ? Object.fromEntries(Object.entries(payload.files).map(([path, entry]) => [path, { ...entry, sha: entry.digest }]))
    : payload.files;
  return { ...payload, files };
}

/// Asks for a version's plain-source archive (room-v2.md "Labels" and
/// §8.5): the first call with `archive=1` requests one if none is pending or
/// ready yet, and every call -- this one included -- reports where that
/// request stands. There is no separate poll endpoint; a caller wanting the
/// wait to end asks this same question again until `archive_status` reads
/// `"ready"` or `"failed"`.
export async function requestArchive(slug, sha, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history/${sha}?archive=1`, {
    headers: asked(headers),
    cache: "no-store",
  });
  if (!response.ok) throw new Error("that version's archive could not be prepared");
  return fromWire(await response.json());
}

/// Where the bytes are, once `archive_status` reads `"ready"`. A 404 until
/// then, the same as any object this server has not produced yet.
export const archiveUrl = (slug, sha) => `/api/documents/${slug}/history/${sha}/archive`;

/// Names a label, or takes its name away with an empty one. Editors only,
/// which the server decides and this does not pretend to know.
export async function label(slug, sha, text, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history/${sha}`, {
    method: "PATCH",
    headers: { ...asked(headers), "content-type": "application/json" },
    body: JSON.stringify({ label: text }),
  });
  if (!response.ok) {
    const said = await response.json().catch(() => ({}));
    throw new Error(said.error || "that label could not be named");
  }
  return await response.json();
}

/// The shortest name for a label that is still a name: seven characters,
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

/// The order the manifest is read in, oldest first. The server writes a
/// sequence number and a timestamp, and two labels can share a
/// timestamp, so the sequence decides first and the digest breaks the last
/// tie -- a history whose order depends on the order it arrived in is a
/// history that is wrong about when things happened.
export function labelOrder(left, right) {
  if (left.seq > 0 && right.seq > 0 && left.seq !== right.seq) return left.seq - right.seq;
  return (Date.parse(left.at) || 0) - (Date.parse(right.at) || 0) || String(left.sha).localeCompare(String(right.sha));
}
