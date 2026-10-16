// Tracked changes live in the shared document, in the `changes` root map.
// Each entry records one contiguous region: an original span of text
// (`deleted`) was replaced by the text now between two Loro cursors. The text
// itself is ordinary document text. This module plans how an edit changes the
// records (pure, shared with the Rust port through the fixtures), writes the
// records, resolves them to offsets and decides them.
//
// Offsets are UTF-16 everywhere, the same as CodeMirror and loro-crdt in JS.

import { Cursor } from "loro-crdt";

export const CHANGES = "changes";

/**
 * @typedef {{ who: string, author: string }} Me
 * @typedef {{ from: number, to: number, insert: string }} Edit
 * @typedef {{ id: string, who: string, author: string, run: string | null, at: number, from: number, to: number, deleted: string }} Change
 * @typedef {Change & { state: "kept" | "set" | "removed" }} PlannedChange
 */

// ---------------------------------------------------------------------------
// The planner

function mapStart(p, ef, et, len) {
  if (p < ef) return p;
  if (p >= et) return p + len - (et - ef);
  return ef + len;
}

function mapEnd(p, ef, et, len) {
  if (p <= ef) return p;
  if (p > et) return p + len - (et - ef);
  return ef;
}

function mapPoint(p, ef, et, len) {
  if (p < ef) return p;
  if (p >= et) return p + len - (et - ef);
  return ef;
}

// The runs of `[ef, et)` that lie outside every change's inserted span.
function originalRuns(changes, ef, et) {
  const runs = [];
  let pos = ef;
  for (const c of changes) {
    if (c.from >= et) break;
    if (c.to <= pos) continue;
    if (c.from > pos) runs.push({ from: pos, to: c.from });
    pos = Math.max(pos, c.to);
  }
  if (pos < et) runs.push({ from: pos, to: et });
  return runs;
}

function byPosition(a, b) {
  return a.from - b.from || a.to - b.to;
}

// Plans one edit given in the coordinates of `cur`. Returns the changes after
// the edit and the ones the edit removed (with their values before it).
function planOne(cur, list, edit, me, run, now, mintId) {
  const ef = edit.from;
  const et = edit.to;
  const len = edit.insert.length;
  const delta = len - (et - ef);
  const runs = originalRuns(list, ef, et);
  const trigger = runs.length > 0 || len > 0;
  const affected = trigger
    ? list.filter((c) => c.who === me.who && c.from <= et && c.to >= ef)
    : [];
  const out = [];
  const gone = [];

  if (trigger) {
    const pieces = [];
    for (const c of affected) pieces.push({ pos: c.from, kind: 0, text: c.deleted });
    for (const r of runs) {
      let start = r.from;
      for (const c of affected) {
        if (c.from === c.to && c.from > start && c.from < r.to) {
          pieces.push({ pos: start, kind: 1, text: cur.slice(start, c.from) });
          start = c.from;
        }
      }
      pieces.push({ pos: start, kind: 1, text: cur.slice(start, r.to) });
    }
    pieces.sort((a, b) => a.pos - b.pos || a.kind - b.kind);
    const first = affected[0];
    let from = ef;
    let to = ef + len;
    for (const c of affected) {
      from = Math.min(from, c.from);
      if (c.to > et) to = Math.max(to, c.to + delta);
    }
    out.push({
      id: first ? first.id : mintId(),
      who: me.who,
      author: me.author,
      run: run ?? (first ? first.run : null),
      at: now,
      from,
      to,
      deleted: pieces.map((p) => p.text).join(""),
      state: "set",
    });
  }

  for (const c of list) {
    if (affected.includes(c)) {
      if (c !== affected[0]) gone.push(c);
      continue;
    }
    if (trigger && len > 0 && c.from < ef && ef < c.to) {
      out.push({ ...c, to: ef, state: "set" });
      if (c.to > et) {
        out.push({
          id: mintId(),
          who: c.who,
          author: c.author,
          run: c.run,
          at: c.at,
          from: ef + len,
          to: c.to + delta,
          deleted: "",
          state: "set",
        });
      }
      continue;
    }
    let from;
    let to;
    if (c.from === c.to) {
      from = mapPoint(c.from, ef, et, len);
      to = from;
    } else {
      from = mapStart(c.from, ef, et, len);
      to = mapEnd(c.to, ef, et, len);
      if (to < from) from = to;
    }
    const overlaps =
      c.from === c.to ? et > ef && ef <= c.from && c.from < et : c.from < et && c.to > ef;
    out.push({ ...c, from, to, state: overlaps ? "set" : c.state });
  }

  const kept = [];
  for (const c of out) {
    if (c.from === c.to && c.deleted === "") gone.push(c);
    else kept.push(c);
  }
  kept.sort(byPosition);
  return { list: kept, gone };
}

/**
 * Plans a batch of edits against the pending changes of one file.
 *
 * `edits` are in the coordinates of `before`, ascending and non-overlapping.
 * They are planned in order with a running adjustment, the way the CodeMirror
 * sync plugin writes them. Positions in the result are in the coordinates of
 * the text after the batch. Removed entries keep their last values.
 *
 * @param {string} before
 * @param {Change[]} changes resolved changes of the file, sorted by `from`
 * @param {Edit[]} edits
 * @param {Me} me
 * @param {{ run?: string | null, now: number, mintId: () => string }} opts
 * @returns {PlannedChange[]}
 */
export function planEdits(before, changes, edits, me, { run = null, now, mintId }) {
  let cur = before;
  let list = changes.map((c) => ({ ...c, state: "kept" }));
  const original = new Set(changes.map((c) => c.id));
  const removed = new Map();
  let adj = 0;
  for (const edit of edits) {
    const shifted = { from: edit.from + adj, to: edit.to + adj, insert: edit.insert };
    const step = planOne(cur, list, shifted, me, run, now, mintId);
    list = step.list;
    for (const c of step.gone) {
      if (original.has(c.id)) removed.set(c.id, { ...c, state: "removed" });
    }
    cur = cur.slice(0, shifted.from) + shifted.insert + cur.slice(shifted.to);
    adj += shifted.insert.length - (shifted.to - shifted.from);
  }
  return [...list, ...removed.values()];
}

// ---------------------------------------------------------------------------
// Reading the records

function fileText(doc, fileId) {
  const text = doc.getMap("files").get(fileId);
  return text && typeof text.toString === "function" && typeof text.getCursor === "function" ? text : null;
}

function cursorPos(doc, bytes) {
  try {
    const result = doc.getCursorPos(Cursor.decode(bytes));
    if (!result || typeof result.offset !== "number") return null;
    return { offset: result.offset, update: result.update ?? null };
  } catch {
    return null;
  }
}

// Resolves one stored value to UTF-16 offsets, or null when it is orphaned.
// `healed` holds the replacement bytes of a cursor whose character was deleted.
function resolveEntry(doc, id, value) {
  if (!value || typeof value.file !== "string" || !(value.start instanceof Uint8Array)) return null;
  const text = fileText(doc, value.file);
  if (!text) return null;
  const start = cursorPos(doc, value.start);
  if (!start) return null;
  const healed = {};
  if (start.update) healed.start = start.update.encode();
  const from = start.offset;
  let to = from;
  if (value.end) {
    const end = cursorPos(doc, value.end);
    if (!end) return null;
    if (end.update) healed.end = end.update.encode();
    to = end.update ? end.offset : end.offset + 1;
  }
  if (to < from) to = from;
  return {
    id,
    file: value.file,
    who: value.who,
    author: value.author,
    run: value.run ?? null,
    at: value.at,
    from,
    to,
    deleted: value.deleted ?? "",
    healed,
  };
}

function resolveAll(doc, fileId) {
  const contents = new Map();
  const found = [];
  for (const [id, value] of doc.getMap(CHANGES).entries()) {
    if (fileId !== undefined && value?.file !== fileId) continue;
    if (value && !contents.has(value.file)) {
      const text = fileText(doc, value.file);
      contents.set(value.file, text ? text.toString() : null);
    }
    const resolved = resolveEntry(doc, id, value);
    if (!resolved) continue;
    if (resolved.from === resolved.to && resolved.deleted === "") continue;
    found.push({ ...resolved, content: contents.get(value.file) });
  }
  found.sort(byPosition);
  return found;
}

/**
 * The pending changes of one file as offsets into its current text. Orphaned
 * and empty entries are left out.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 */
export function resolveChanges(doc, fileId) {
  return resolveAll(doc, fileId).map(({ content, healed, ...change }) => change);
}

/**
 * Every pending change in every file, each with the text it inserted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 */
export function listChanges(doc) {
  return resolveAll(doc).map(({ content, healed, ...change }) => ({
    ...change,
    inserted: content.slice(change.from, change.to),
  }));
}

// ---------------------------------------------------------------------------
// Writing the records

function cursorValue(text, from, to, me, run, at, deleted, file) {
  const start = text.getCursor(from, -1);
  if (!start) return null;
  let end = null;
  if (to > from) {
    end = text.getCursor(to - 1, 1);
    if (!end) return null;
  }
  return {
    file,
    author: me.author,
    who: me.who,
    at,
    run,
    start: start.encode(),
    end: end ? end.encode() : null,
    deleted,
  };
}

/**
 * Records edits that were already written into the file's `LoroText`.
 * `before` is the text before the batch and `preResolved` the file's changes
 * resolved before it. Does not commit. Returns the ids it wrote.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @param {string} before
 * @param {Change[]} preResolved
 * @param {Edit[]} edits
 * @param {Me} me
 * @param {{ run?: string | null, now?: number }} [opts]
 * @returns {string[]}
 */
export function recordEdits(doc, fileId, before, preResolved, edits, me, { run = null, now = Date.now() } = {}) {
  const text = fileText(doc, fileId);
  if (!text) return [];
  const map = doc.getMap(CHANGES);
  const plan = planEdits(before, preResolved, edits, me, {
    run,
    now,
    mintId: () => crypto.randomUUID(),
  });
  const written = [];
  for (const c of plan) {
    if (c.state === "removed") {
      map.delete(c.id);
    } else if (c.state === "set") {
      const value = cursorValue(text, c.from, c.to, c, c.run, c.at, c.deleted, fileId);
      if (!value) continue;
      map.set(c.id, value);
      written.push(c.id);
    }
  }
  return written;
}

/**
 * Replaces `[from, to)` with `insert` in a file and records it as a tracked
 * change. Does not commit. Returns the ids it wrote.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @param {number} from
 * @param {number} to
 * @param {string} insert
 * @param {Me} me
 * @param {{ run?: string | null, now?: number }} [opts]
 * @returns {string[]}
 */
export function trackedSplice(doc, fileId, from, to, insert, me, opts) {
  const text = fileText(doc, fileId);
  if (!text) return [];
  const before = text.toString();
  const pre = resolveChanges(doc, fileId);
  if (to > from) text.delete(from, to - from);
  if (insert) text.insert(from, insert);
  return recordEdits(doc, fileId, before, pre, [{ from, to, insert }], me, opts);
}

// ---------------------------------------------------------------------------
// Decisions

/**
 * Accepts a change: drops the record and keeps the text. Returns false when
 * the id is unknown. An orphaned record is dropped too.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} id
 */
export function accept(doc, id) {
  const map = doc.getMap(CHANGES);
  if (!map.get(id)) return false;
  map.delete(id);
  doc.commit();
  return true;
}

/**
 * Rejects a change: puts the original text back, then drops the record.
 * Returns false when the id is unknown. An orphaned record is only dropped.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} id
 */
export function reject(doc, id) {
  const map = doc.getMap(CHANGES);
  const value = map.get(id);
  if (!value) return false;
  const resolved = resolveEntry(doc, id, value);
  if (resolved) {
    const text = fileText(doc, resolved.file);
    if (resolved.to > resolved.from) text.delete(resolved.from, resolved.to - resolved.from);
    if (resolved.deleted) text.insert(resolved.from, resolved.deleted);
  }
  map.delete(id);
  doc.commit();
  return true;
}

/**
 * Deletes orphaned and empty records and rewrites entries whose cursors
 * pointed at deleted characters. Does not commit. Returns the number deleted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 */
export function prune(doc) {
  const map = doc.getMap(CHANGES);
  let count = 0;
  for (const [id, value] of [...map.entries()]) {
    const resolved = resolveEntry(doc, id, value);
    if (!resolved || (resolved.from === resolved.to && resolved.deleted === "")) {
      map.delete(id);
      count += 1;
    } else if (resolved.healed.start || resolved.healed.end) {
      map.set(id, {
        ...value,
        start: resolved.healed.start ?? value.start,
        end: resolved.healed.end ?? value.end,
      });
    }
  }
  return count;
}
