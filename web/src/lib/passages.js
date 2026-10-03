// Where a passage went.
//
// A comment records a source range and the frontier it was made on -- the
// exact position in the editing history, not a label (§8.2 dropped
// `annotations.checkpoint_id`; the anchor now carries `source_sequence` and
// `frontier` instead, room-v2.md). While the passage is still in the document
// there is nothing to say. When it is gone, the card used to read "Needs
// re-anchoring", which told the person who wrote the comment nothing they did
// not already know. What they want is where it went, and the two records
// together answer that: the passage was in the text at the comment's own
// frontier and is not in the text now, so there is a label between the
// two where it stopped being found.
//
// Finding it is an ordered walk: edits can remove a passage and later
// reintroduce it, so bisection cannot identify its first loss. Each label
// response is cached for other comments that ask about the same moment.
//
// This is deliberately not the word-level diff. What replaced the passage is
// `history.js`'s `wordDiff` and needs the diff crate in the browser; what is
// here needs nothing beyond the labels, and it is the half of
// the question a reviewer actually asks.
//
// The source file is followed by its stable ID through historical labels;
// a rename does not make the passage disappear.

import { anchorOne, flatten } from "./anchor.js";
import * as history from "./history.js";
import { MOMENT } from "./moment.js";

// Passage lookups can outlive a single comment card in a long-lived reader.
// Cached label responses can be reused across comments, avoiding refetching
// the same historical source for every comment.
const CACHE_LIMIT = 64;

// A label SHA can be shared by documents and a reader can arrive with a
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

// One label fetch per document/sha, shared by every comment that asks
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

/// The text of one file at one moment, as it was written. A historical
/// comment asks by stable file ID, so a rename since then is harmless. A path
/// is accepted for callers already holding a name from that moment.
///
/// The moment is either a label or a `frontier:` position in the editing
/// history -- a comment made on the live draft carries the second, because
/// nothing writes a source archive for a comment (§7.3). The server answers
/// both through the same route and the same shape (`history.rs`'s
/// `projection_wire`), so there is one fetch here, not two.
export async function sourceTextAt(slug, sha, file, headers = {}) {
  const key = cacheKey(slug, sha, headers);
  if (!points.has(key)) {
    const pending = history.read(slug, sha, headers);
    remember(points, key, pending);
    // A failed request must not poison this label forever. The identity
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

/// How a comment is read against an old label: the passage it is about,
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
    frontier: anchor.frontier || "",
    created: comment.created || "",
    file_id: target.file_id,
    path: paths?.get?.(target.file_id) || "",
    selector: { exact: target.exact, prefix: target.prefix || "", suffix: target.suffix || "" },
  };
}

/// The first label at which a comment's passage was no longer found.
///
/// `labels` is the manifest, oldest first. A comment always carries its
/// own frontier now, never a label (§7.3: nothing writes a label for a
/// comment), so the search starts at that exact position rather than at a
/// manifest entry. Labels may show the passage disappearing and returning,
/// so inspect them in order and return the first loss. Returns that entry, or null
/// when there is nothing to say: no history to look in, or a passage that
/// turns out still to be there.
export async function wentAtMany(slug, tracedList, labels, headers = {}, atSource = sourceTextAt) {
  const answers = Array(tracedList.length).fill(null);
  if (!labels?.length) return answers;

  // Validate each comment against its own frontier before comparing it with
  // the shared label timeline. State is proportional to comments, while
  // historical source text is read one label at a time below.
  const groups = new Map();
  for (const [index, traced] of tracedList.entries()) {
    if (!traced?.frontier) continue;
    let ownText;
    try {
      ownText = await atSource(slug, MOMENT + traced.frontier, { file_id: traced.file_id }, headers);
    } catch {
      continue;
    }
    if (typeof ownText !== "string" || !holds(ownText, traced.selector)) continue;
    const created = Date.parse(traced.created);
    if (!Number.isFinite(created)) continue;
    const start = labels.findIndex((point) => (Date.parse(point.at) || 0) >= created);
    if (start < 0) continue;
    const key = traced.file_id;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push({ index, traced, start });
  }

  if (!groups.size) return answers;
  const newest = labels.at(-1);
  const active = new Map();
  for (const [fileId, candidates] of groups) {
    let newestText;
    try {
      newestText = await atSource(slug, newest.sha, { file_id: fileId }, headers);
    } catch {
      continue;
    }
    if (typeof newestText !== "string") continue;
    const stillPresent = new Set(candidates
      .filter(({ traced }) => holds(newestText, traced.selector))
      .map(({ index }) => index));
    const remaining = candidates.filter(({ index }) => !stillPresent.has(index));
    if (remaining.length) active.set(fileId, remaining);
  }

  // All comments for a label are evaluated before advancing to the next one.
  // This shares each bounded source response across its consumers even when
  // comments have different creation times; no per-history text map grows.
  for (let at = 0; at < labels.length && active.size; at += 1) {
    const point = labels[at];
    for (const [fileId, candidates] of active) {
      const eligible = candidates.filter(({ start }) => start <= at);
      if (!eligible.length) continue;
      // The newest label was already read for each active group. Every
      // remaining candidate is known absent there, so resolve any survivors
      // at that point without retaining or fetching its text again.
      if (at === labels.length - 1) {
        for (const { index } of eligible) answers[index] = point;
        active.delete(fileId);
        continue;
      }
      let text;
      try {
        text = await atSource(slug, point.sha, { file_id: fileId }, headers);
      } catch {
        text = null;
      }
      if (typeof text !== "string") {
        const unresolved = new Set(eligible.map(({ index }) => index));
        const remaining = candidates.filter(({ index }) => !unresolved.has(index));
        if (remaining.length) active.set(fileId, remaining);
        else active.delete(fileId);
        continue;
      }
      const found = new Set();
      for (const candidate of eligible) {
        if (!holds(text, candidate.traced.selector)) {
          answers[candidate.index] = point;
          found.add(candidate.index);
        }
      }
      if (found.size) {
        const remaining = candidates.filter(({ index }) => !found.has(index));
        if (remaining.length) active.set(fileId, remaining);
        else active.delete(fileId);
      }
    }
  }
  return answers;
}

export async function wentAt(slug, traced, labels, headers = {}, atSource = sourceTextAt) {
  return (await wentAtMany(slug, [traced], labels, headers, atSource))[0] ?? null;
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
