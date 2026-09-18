// Where a passage went.
//
// A comment records a source range and the checkpoint it was made on. While
// the passage is still in the document there is nothing to say. When it is
// gone, the card used to read "Needs re-anchoring", which told the person who
// wrote the comment nothing they did not already know. What they want is where
// it went, and the two records together answer that: the passage was in the
// text at the comment's own checkpoint and is not in the text now, so there is
// a checkpoint between the two where it stopped being found.
//
// Finding it is a search, not a walk. "Found" only goes one way -- a passage
// that came back would be a passage that was never gone in a way anybody means
// -- so a bisection finds the moment in a handful of checkpoints rather than
// all of them, and each one it looks at is cached for every other comment that
// asks about the same moment.
//
// This is deliberately not the word-level diff. What replaced the passage is
// `history.js`'s `wordDiff` and needs the diff crate in the browser; what is
// here needs nothing beyond the checkpoints, and it is the half of
// the question a reviewer actually asks.
//
// The source file is followed by its stable ID through historical checkpoints;
// a rename does not make the passage disappear.

import { anchorOne, flatten } from "./anchor.js";
import * as history from "./history.js";

// Passage lookups can outlive a single comment card in a long-lived reader.
// A bisection reads the same handful of checkpoints for every comment it
// walks, so the entries are what make an N-comment review cost a history's
// worth of requests rather than N of them.
const CACHE_LIMIT = 64;

// A checkpoint SHA can be shared by documents and a reader can arrive with a
// link key. Partition entries by the supplied authentication context. The
// value is kept only in this private in-memory key and is never sent or
// logged; the server still checks authorization on every miss.
const scopes = new Map();
let nextScope = 1;
let cacheGeneration = 0;
function authScope(headers) {
  const entries = typeof Headers !== "undefined" && headers instanceof Headers
    ? Array.from(headers.entries())
    : Object.entries(headers || {});
  const fingerprint = JSON.stringify(entries
    .filter(([name]) => name.toLowerCase() !== "x-librepaper-client")
    .map(([name, value]) => `${name.toLowerCase()}:${String(value)}`)
    .sort());
  // Cache keys contain an opaque process-local scope, never bearer tokens or
  // link keys. Clearing the passage cache retires these scopes on sign-out or
  // document-link changes.
  if (!scopes.has(fingerprint)) scopes.set(fingerprint, `scope-${nextScope++}`);
  return scopes.get(fingerprint);
}

const cacheKey = (slug, sha, headers) => `${slug}\u0000${sha}\u0000${authScope(headers)}`;

function remember(cache, key, value) {
  cache.set(key, value);
  while (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

// One checkpoint fetch per document/sha, shared by every comment that asks
// about that moment. The key carries an opaque per-authorization scope, and
// `clearPassageCache` retires those scopes when a reader signs out or arrives
// on a different link, so a cached tree can never outlive the permission that
// fetched it.
const points = new Map();

/// Drop historical passage responses when a reader changes link or signs out.
/// In-flight calls may finish, but their identity checks cannot repopulate a
/// map entry that has since been replaced.
export function clearPassageCache() {
  cacheGeneration++;
  points.clear();
  scopes.clear();
}

/// The text of one file of a checkpoint, as it was written. A historical
/// comment asks by stable file ID, so a rename between checkpoints is harmless.
/// A path is accepted for callers already holding a checkpoint-local name.
export async function sourceTextAt(slug, sha, file, headers = {}) {
  const key = cacheKey(slug, sha, headers);
  if (!points.has(key)) {
    const pending = history.checkpoint(slug, sha, headers);
    remember(points, key, pending);
    // A failed request must not poison this checkpoint forever. The identity
    // check preserves a newer retry if eviction or another caller replaced it.
    pending.catch(() => {
      if (points.get(key) === pending) points.delete(key);
    });
  }
  const pending = points.get(key);
  const generation = cacheGeneration;
  const point = await pending;
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  const texts = point.texts || {};
  const path = typeof file === "string" ? file : Object.entries(point.files || {})
    .find(([, entry]) => entry?.kind === "text" && entry.id === file?.file_id)?.[0];
  return Object.prototype.hasOwnProperty.call(texts, path) ? texts[path] : null;
}

/// Whether a quotation is in a text, by the same match that anchors it in the
/// document: exactly, or with whitespace flattened on both sides.
export function holds(text, comment) {
  return typeof text === "string" && Boolean(anchorOne(text, comment, flatten(text)));
}

/// How a comment is read against an old checkpoint: the passage it is about,
/// as the source had it, and the file that passage was in.
///
/// This is the comment's own record, not a guess -- the server wrote it when
/// the comment was made and has not touched it since. `paths` supplies the
/// current name only for comparing the old passage with current source.
export function tracedBy(comment, paths) {
  const anchor = comment?.original_anchor;
  if (anchor?.kind !== "source_text") return null;
  const target = anchor.target;
  return {
    checkpoint: anchor.checkpoint_id || "",
    file_id: target.file_id,
    path: paths?.get?.(target.file_id) || "",
    selector: { exact: target.exact, prefix: target.prefix || "", suffix: target.suffix || "" },
  };
}

/// The first checkpoint at which a comment's passage was no longer found.
///
/// `checkpoints` is the manifest, oldest first. The search starts at the
/// comment's own checkpoint -- or at the oldest the manifest still has, for a
/// comment made before that was recorded -- and ends at the newest, where the
/// caller has already established that the passage is gone. Returns the
/// manifest entry, or null when there is nothing to say: no history to look
/// in, or a passage that turns out still to be there.
export async function wentAt(slug, traced, checkpoints, headers = {}, atSource = sourceTextAt) {
  if (!checkpoints?.length || !traced) return null;
  // A comment from before checkpoints were recorded on one, or one whose
  // checkpoint has since been shed, is read as made on the oldest moment the
  // manifest still has. That is the earliest thing that could be true of it.
  const own = checkpoints.findIndex((point) => point.sha === traced.checkpoint);
  let low = own >= 0 ? own : 0;
  let high = checkpoints.length - 1;
  if (low >= high) return null;

  // Every checkpoint supplies the name this same file had at that moment.
  const selector = traced.selector;
  const read = (sha) => atSource(slug, sha, { file_id: traced.file_id }, headers);

  // The passage has to have been there to have gone. A comment whose own
  // checkpoint does not hold it is one whose quotation this cannot reason
  // about, and saying nothing is better than naming a moment at random.
  const ownText = await read(checkpoints[low].sha);
  if (typeof ownText !== "string" || !holds(ownText, selector)) return null;
  const newestText = await read(checkpoints[high].sha);
  if (typeof newestText !== "string" || holds(newestText, selector)) return null;

  // Invariant: it is in `low` and not in `high`. Each step halves the gap, so
  // the answer costs about five checkpoint reads on a history of thirty.
  while (high - low > 1) {
    const middle = (low + high) >> 1;
    const middleText = await read(checkpoints[middle].sha);
    if (typeof middleText !== "string") return null;
    if (holds(middleText, selector)) low = middle;
    else high = middle;
  }
  return checkpoints[high];
}

/// Finds what replaced a comment's quotation between two versions. The
/// returned text is the quoted old range transformed through the edits, so
/// unchanged words between two replacements remain visible (`blue fox at calm
/// river`, rather than just the two inserted fragments). Null means the
/// quotation could not be anchored or the versions did not replace any part of
/// it. A whitespace-only insertion is retained because it can be the only
/// change inside a quoted source selection.
export async function replacementAt(oldText, newText, selector, diffApi) {
  if (typeof oldText !== "string" || typeof newText !== "string" || !selector?.exact) return null;
  const found = anchorOne(oldText, selector, flatten(oldText));
  if (!found) return null;
  const compute = diffApi || ((before, after) => history.wordDiff(before, after));
  const edits = await compute(oldText, newText);
  const overlapping = edits.filter((edit) => {
    const start = Number(edit.at) || 0;
    const end = start + (Number(edit.delete) || 0);
    // An insertion at the quoted boundary belongs to the replacement only
    // when it is inside the selection, not when it merely follows it.
    return (end > found.start && start < found.end)
      || (end === start && start >= found.start && start < found.end);
  });
  if (!overlapping.length) return null;
  const start = found.start;
  const end = found.end;
  let cursor = start;
  let replacement = "";
  for (const edit of edits) {
    const editStart = Number(edit.at) || 0;
    const editEnd = editStart + (Number(edit.delete) || 0);
    if (editEnd <= start) continue;
    if (editStart >= end) break;
    if (editStart > cursor) replacement += oldText.slice(cursor, Math.min(editStart, end));
    let insert = edit.insert || "";
    // A token hunk can begin just before the quote or end just after it. Drop
    // unchanged outside context when the edit gives us a defensible boundary.
    // When that context also changed, the diff cannot identify which part of
    // the insertion replaces the selected passage.
    if (editStart < start) {
      const outside = oldText.slice(editStart, start);
      if (outside && insert) {
        if (!insert.startsWith(outside)) return null;
        insert = insert.slice(outside.length);
      }
    }
    if (editEnd > end) {
      const outside = oldText.slice(end, editEnd);
      if (outside && insert) {
        if (!insert.endsWith(outside)) return null;
        insert = insert.slice(0, -outside.length);
      }
    }
    if (insert && (editEnd > start || (editStart >= start && editStart < end))) {
      replacement += insert;
    }
    cursor = Math.max(cursor, Math.min(editEnd, end));
  }
  if (cursor < end) replacement += oldText.slice(cursor, end);
  return replacement;
}
