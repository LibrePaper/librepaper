// Tracked changes live in the shared document, in the `changes` root map. Each
// record is a map container, so two peers writing different keys of one record
// both keep their write. A change record is one author's edit of one region:
// the text it inserted (a span, between two Loro cursors, or none). A fragment
// record is one piece of deleted text, anchored by a cursor on the character
// after it and ordered among the fragments at its point by its rank. Gaps are
// not stored: the fragments that resolve to one offset form the gap there, in
// rank order. So a rejected change puts its text back where it was, whatever
// order the decisions come in.
//
// This module plans how an edit changes the records (pure, shared with the
// Rust port through the fixtures), writes the records, resolves them to
// offsets, decides them and keeps them in step with untracked edits
// (untrackedSplice). Records of the earlier model are read in place and
// rewritten as containers the first time something is written to them.
//
// Offsets are UTF-16 everywhere, the same as CodeMirror and loro-crdt in JS.

import { Cursor, LoroMap } from "loro-crdt";

export const CHANGES = "changes";

// How long a record rests before prune drops it when empty, or re-anchors its
// stale cursors, so that its author's other session has time to sync.
const SETTLE_MS = 60_000;

// A gap is not stored. It is the fragments that resolve to one offset, and its
// id only names it within one plan.
/**
 * @typedef {{ who: string, author: string }} Me
 * @typedef {{ from: number, to: number, insert: string }} Edit
 * @typedef {{ id?: string, change: string, text: string, rank?: number }} Fragment
 * @typedef {{ id: string, change: string, text: string, rank: number, stale: boolean }} ResolvedFragment
 * @typedef {{ id: string, who: string, author: string, run: string | null, at: number, from: number, to: number }} Change
 * @typedef {Change & { stale: boolean, hasCursor: boolean, legacy?: boolean }} ResolvedChange
 * @typedef {{ id: string, at: number, fragments: Fragment[] }} Gap
 * @typedef {{ id: string, at: number, fragments: ResolvedFragment[], stale: boolean }} ResolvedGap
 * @typedef {{ id: string, change: string, text: string, rank: number, at: number, stale: boolean, who: any, author: any, run: any, when: any }} OrphanFragment
 * @typedef {{ changes: Change[], gaps: Gap[] }} FileState
 * @typedef {Change & { deleted: string }} Row
 * @typedef {Change & { state: "kept" | "set" | "removed", emptied?: boolean }} PlannedChange
 * @typedef {Gap & { state: "kept" | "set" | "removed" }} PlannedGap
 * @typedef {{ changes: PlannedChange[], gaps: PlannedGap[] }} PlannedState
 */

// ---------------------------------------------------------------------------
// Planner (pure)

/**
 * Plans a batch of edits against the records of one file.
 *
 * `edits` are in the coordinates of `before`, ascending and non-overlapping,
 * and are planned in order with a running adjustment. Positions in the result
 * are in the coordinates of the text after the batch. Removed items keep their
 * last values. With `untracked` the edits make no change of mine and record
 * nothing; the spans and gaps still follow the text. Adjacent pieces of one
 * change join into one fragment only while neither is stored yet.
 *
 * @param {string} before
 * @param {FileState} state
 * @param {Edit[]} edits
 * @param {Me} me
 * @param {{ run?: string | null, now: number, mintId: () => string, untracked?: boolean }} opts
 * @returns {PlannedState}
 */
export function planEdits(before, state, edits, me, { run = null, now, mintId, untracked = false }) {
  let cur = before;
  let changes = state.changes.map((c) => ({ ...c, state: "kept" }));
  let gaps = state.gaps.map((g) => ({ ...g, fragments: g.fragments.map((f) => ({ ...f })), state: "kept" }));
  const originalChanges = new Set(state.changes.map((c) => c.id));
  const originalGaps = new Set(state.gaps.map((g) => g.id));
  const removedChanges = new Map();
  const removedGaps = new Map();
  let adj = 0;
  for (const edit of edits) {
    const ef = edit.from + adj;
    const et = edit.to + adj;
    const insert = edit.insert;
    if (ef === et && insert.length === 0) continue;
    const step = planOne(cur, changes, gaps, ef, et, insert, me, run, now, mintId, untracked);
    for (const c of changes) if (!step.changes.some((x) => x.id === c.id) && originalChanges.has(c.id)) removedChanges.set(c.id, { ...c, state: "removed" });
    for (const g of gaps) if (!step.gaps.some((x) => x.id === g.id) && originalGaps.has(g.id)) removedGaps.set(g.id, { ...g, state: "removed" });
    changes = step.changes;
    gaps = step.gaps;
    cur = cur.slice(0, ef) + insert + cur.slice(et);
    adj += insert.length - (et - ef);
  }
  for (const c of changes) if (!originalChanges.has(c.id) && c.state === "kept") c.state = "set";
  for (const g of gaps) if (!originalGaps.has(g.id) && g.state === "kept") g.state = "set";
  return {
    changes: [...changes, ...removedChanges.values()],
    gaps: [...gaps, ...removedGaps.values()],
  };
}

function hasSpan(c) {
  return c.to > c.from;
}

function planOne(cur, changes, gaps, ef, et, insert, me, run, now, mintId, untracked) {
  const len = insert.length;
  const delta = len - (et - ef);
  // Own changes this edit touches: a span overlapping or adjacent, or a
  // fragment in a gap inside the closed range.
  const swallowedGaps = gaps.filter((g) => g.at >= ef && g.at <= et);
  const owned = new Set();
  // A span of mine joins when it touches the edit. A change of mine with no
  // span joins when one of its fragments sits in a swallowed gap. A span of
  // mine elsewhere stays its own change: one change holds one span, and a
  // span never widens over text that is not its own.
  for (const c of changes) {
    if (c.who !== me.who) continue;
    if (hasSpan(c) && c.from <= et && c.to >= ef) owned.add(c.id);
  }
  for (const g of swallowedGaps) for (const f of g.fragments) {
    const c = changes.find((x) => x.id === f.change);
    if (c && c.who === me.who && !hasSpan(c)) owned.add(c.id);
  }
  // Pure deletion of text inside inserted spans only, by nobody of mine: the
  // spans shrink and nothing is recorded, unless original text is involved.
  const spanAt = (p) => changes.find((c) => hasSpan(c) && c.from <= p && p < c.to);
  const originalRuns = [];
  {
    let p = ef;
    while (p < et) {
      const s = spanAt(p);
      if (s) { p = Math.min(et, s.to); continue; }
      let q = p;
      while (q < et && !spanAt(q)) q++;
      originalRuns.push({ from: p, to: q });
      p = q;
    }
  }
  // Is there anything to record? Original text deleted, or text inserted.
  // An untracked edit records nothing and makes no change of mine; it still
  // moves, shrinks and splits spans and merges the gaps it collapses, so the
  // records keep describing the text.
  const records = !untracked && (originalRuns.length > 0 || len > 0);
  if (untracked) owned.clear();
  let mine = null;
  if (records) {
    const first = [...owned].map((id) => changes.find((c) => c.id === id)).sort((a, b) => a.from - b.from)[0];
    mine = first
      ? { ...first, run: run ?? first.run, at: now, state: "set" }
      : { id: mintId(), who: me.who, author: me.author, run, at: now, from: ef, to: ef, state: "set" };
  }
  // Build the merged gap: walk ef..et, gaps at p then the original char at p.
  // With text inserted, what sat at `et` was after the deleted text and goes
  // after the new span; everything else goes before it. When a span or a
  // fragment of mine sits at `et` it joins the new change, and then the whole
  // of what sat there goes before the span: my deleted text belongs with it.
  const ownAtEnd = changes.some((c) => c.who === me.who && hasSpan(c) && c.from === et && owned.has(c.id))
    || swallowedGaps.some((g) => g.at === et && g.fragments.some((f) => owned.has(f.change)));
  const splitAtEnd = len > 0 && !ownAtEnd;
  const merged = [];
  const after = [];
  // Adjacent pieces of one change join into one fragment only while neither is
  // stored yet. A stored fragment is never rewritten or dropped here: its
  // author's other session may be extending it, or a reviewer deciding its
  // change, at this very moment, and either write would be lost. A stored
  // fragment keeps its id and rank through the walk, so writing the gap back
  // changes nothing about it unless the order asks for it.
  const pushFragment = (list, change, text, id, rank) => {
    const last = list[list.length - 1];
    if (last && last.change === change && !last.id && !id) last.text += text;
    else list.push({ change, text, ...(id ? { id } : {}), ...(typeof rank === "number" ? { rank } : {}) });
  };
  for (let p = ef; p <= et; p++) {
    const target = splitAtEnd && p === et ? after : merged;
    // Untracked edits can leave two gaps on one point; every one of them is taken.
    for (const g of swallowedGaps) if (g.at === p) for (const f of g.fragments) pushFragment(target, mine && owned.has(f.change) ? mine.id : f.change, f.text, f.id, f.rank);
    if (p < et && !spanAt(p) && records) pushFragment(merged, mine.id, cur[p]);
  }
  // Spans.
  const outChanges = [];
  const newSpans = [];
  for (const c of changes) {
    if (mine && owned.has(c.id)) continue;
    if (!hasSpan(c)) {
      // A pure-deletion change: its place follows its fragments. Keep it unless
      // all its fragments vanished (handled below).
      outChanges.push(c);
      continue;
    }
    // A span whose boundary sits exactly where text is inserted is written
    // again: a cursor left on a deleted character there could drift across
    // the new text, and the write re-pins it only if it did.
    if (c.to <= ef) { outChanges.push(len > 0 && c.to === ef ? { ...c, state: "set" } : c); continue; }
    if (c.from >= et) { outChanges.push({ ...c, from: c.from + delta, to: c.to + delta, ...(len > 0 && c.from === et ? { state: "set" } : {}) }); continue; }
    // Foreign span overlapping the edit.
    if (c.from < ef && c.to > et && len > 0) {
      // Split around the inserted text.
      outChanges.push({ ...c, to: ef, state: "set" });
      newSpans.push({ ...c, id: mintId(), from: ef + len, to: c.to + delta, state: "set" });
      continue;
    }
    const from = c.from < ef ? c.from : ef + len;
    const to = c.to > et ? c.to + delta : (c.from < ef ? ef : ef + len);
    if (to > from) outChanges.push({ ...c, from, to, state: "set" });
    else {
      // The whole span went from this view. The record stays, untouched: its
      // author, in this or another session, may be extending it at this
      // moment, and a stale cursor resolves to the right place anyway. Prune
      // drops it once it has rested empty for a while.
      outChanges.push({ ...c, from: ef + len, to: ef + len, emptied: true });
    }
  }
  outChanges.push(...newSpans);
  if (mine) {
    // The merged span: my overlapping spans plus the inserted text.
    let from = ef;
    let to = ef + len;
    for (const id of owned) {
      const c = changes.find((x) => x.id === id);
      if (!hasSpan(c)) continue;
      if (c.from < from) from = c.from;
      if (c.to > et) to = Math.max(to, c.to + delta);
    }
    mine.from = from;
    mine.to = to;
    outChanges.push(mine);
  }
  // Gaps.
  const outGaps = [];
  for (const g of gaps) {
    if (g.at < ef) { outGaps.push(g); continue; }
    if (g.at > et) { outGaps.push({ ...g, at: g.at + delta }); continue; }
  }
  const ids = swallowedGaps.map((g) => g.id);
  if (merged.length > 0) {
    // One gap per position: gaps already at the span's start, which held text
    // replaced by its earlier part, take the new fragments after their own.
    const pos = mine ? mine.from : ef;
    const there = outGaps.filter((g) => g.at === pos);
    if (there.length > 0) {
      const list = [];
      for (const g of there) for (const f of g.fragments) pushFragment(list, f.change, f.text, f.id, f.rank);
      for (const f of merged) pushFragment(list, f.change, f.text, f.id, f.rank);
      const rest = outGaps.filter((g) => g.at !== pos);
      rest.push({ ...there[0], fragments: list, state: "set" });
      outGaps.length = 0;
      outGaps.push(...rest);
    } else outGaps.push({ id: ids.shift() ?? mintId(), at: pos, fragments: merged, state: "set" });
  }
  if (after.length > 0) {
    outGaps.push({ id: ids.shift() ?? mintId(), at: ef + len, fragments: after, state: "set" });
  }
  // The changes that merged into mine are gone; their fragments anywhere
  // else now belong to mine.
  if (mine) {
    for (let i = 0; i < outGaps.length; i++) {
      const g = outGaps[i];
      if (!g.fragments.some((f) => owned.has(f.change) && f.change !== mine.id)) continue;
      const list = [];
      for (const f of g.fragments) pushFragment(list, owned.has(f.change) ? mine.id : f.change, f.text, f.id, f.rank);
      outGaps[i] = { ...g, fragments: list, state: "set" };
    }
  }
  // A change with no span and no fragment is gone.
  const referenced = new Set(outGaps.flatMap((g) => g.fragments.map((f) => f.change)));
  const live = outChanges.filter((c) => hasSpan(c) || referenced.has(c.id) || c.emptied);
  live.sort((a, b) => a.from - b.from || a.to - b.to);
  outGaps.sort((a, b) => a.at - b.at);
  // Pure-deletion changes take their place from their first fragment.
  for (const c of live) if (!hasSpan(c)) {
    const g = outGaps.find((x) => x.fragments.some((f) => f.change === c.id));
    if (g && c.from !== g.at) { c.from = g.at; c.to = g.at; }
  }
  return { changes: live, gaps: outGaps };
}

// ---------------------------------------------------------------------------
// Reading
//
// Stored records:
//   change:   { kind: "change", file, who, author, run, at, start, end }
//   fragment: { kind: "fragment", file, change, text, start, rank }
// A fragment is one piece of deleted text, anchored by a cursor on the
// character after it (or the end of the text) and ordered among the fragments
// at the same point by `rank`. Gaps are derived on read: the fragments that
// resolve to one offset, in rank order. No record is ever moved between
// gaps, so two peers writing near one point never overwrite each other's
// fragments; each writes its own records. Records of the earlier model (plain
// values with no `kind`) are read as the change and fragments they stand for
// (`expandLegacy`).

function fileText(doc, fileId) {
  const text = /** @type {any} */ (doc.getMap("files").get(fileId));
  return text && typeof text.toString === "function" && typeof text.getCursor === "function" ? text : null;
}

function cursorPos(doc, bytes) {
  try {
    const result = doc.getCursorPos(Cursor.decode(bytes));
    if (!result || typeof result.offset !== "number") return null;
    return { offset: result.offset, stale: Boolean(result.update) };
  } catch {
    return null;
  }
}

// Whether two encoded cursors sit on the same character (or both at the end)
// on the same side. Encodings differ with the offset they were made at, so
// they are compared by what they are attached to.
function sameCursor(a, b) {
  if (!(a instanceof Uint8Array) || !(b instanceof Uint8Array)) return false;
  try {
    const x = Cursor.decode(a);
    const y = Cursor.decode(b);
    return JSON.stringify([x.pos() ?? null, x.side(), x.containerId()]) === JSON.stringify([y.pos() ?? null, y.side(), y.containerId()]);
  } catch {
    return false;
  }
}

function charWidth(body, pos) {
  const hi = body.charCodeAt(pos);
  const lo = body.charCodeAt(pos + 1);
  return hi >= 0xd800 && hi <= 0xdbff && lo >= 0xdc00 && lo <= 0xdfff ? 2 : 1;
}

function lastCharStart(body, to) {
  const lo = body.charCodeAt(to - 1);
  const hi = to >= 2 ? body.charCodeAt(to - 2) : 0;
  return lo >= 0xdc00 && lo <= 0xdfff && hi >= 0xd800 && hi <= 0xdbff ? to - 2 : to - 1;
}

// Order at one point: by rank, then by id. Ranks are kept in document order
// by every planner that touches the point; two peers that collapse separate
// points onto one at the same moment can leave ranks that interleave, which
// misorders but never loses text.
const byRank = (a, b) => a.rank - b.rank || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

/**
 * Resolves every record of a file to offsets. Gaps are derived here: the
 * fragments that resolve to one offset, in rank order. A change's `stale` flag
 * is set when one of its cursors pointed at a deleted character, and a gap's
 * when one of its fragments did. Records that do not resolve are left out.
 * `orphans` are the fragments whose change is gone.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @returns {{ changes: ResolvedChange[], gaps: ResolvedGap[], orphans: OrphanFragment[] }}
 */
export function resolveFile(doc, fileId) {
  const text = fileText(doc, fileId);
  if (!text) return { changes: [], gaps: [], orphans: [] };
  const body = text.toString();
  const changes = [];
  const fragments = [];
  // Records of the earlier model are plain values; they are read as the
  // change and fragments they stand for, and rewritten in the new form only
  // when something has to be written to them (`convertLegacy`).
  const legacy = legacyRecords(doc, fileId);
  for (const { change: c, fragments: fs } of expandLegacy(legacy)) {
    for (const f of fs) {
      const pos = cursorPos(doc, f.start);
      if (!pos) continue;
      fragments.push({ ...f, at: pos.offset, stale: pos.stale });
    }
    let from = 0;
    let to = 0;
    let stale = false;
    if (c.end instanceof Uint8Array) {
      const s0 = cursorPos(doc, c.start);
      const e0 = cursorPos(doc, c.end);
      if (!s0 || !e0) continue;
      from = s0.offset;
      stale = s0.stale || e0.stale;
      to = Math.max(from, e0.stale ? e0.offset : e0.offset + charWidth(body, e0.offset));
    }
    changes.push({ id: c.id, who: c.who, author: c.author, run: c.run ?? null, at: c.at, from, to, stale, hasCursor: true, legacy: true });
  }
  for (const [id, entry] of doc.getMap(CHANGES).entries()) {
    if (!(entry instanceof LoroMap)) continue;
    const value = /** @type {any} */ (entry);
    if (value.get("file") !== fileId) continue;
    if (value.get("kind") === "fragment") {
      // Records are map containers: anchor, rank and text are separate keys,
      // so a peer re-anchoring a fragment and its author extending it at the
      // same time both keep their write, and two peers moving the two ends of
      // one span each keep theirs.
      const start = value.get("start");
      const change = value.get("change");
      const deleted = value.get("text");
      const rank = value.get("rank");
      if (!(start instanceof Uint8Array) || typeof change !== "string" || typeof deleted !== "string" || deleted === "") continue;
      const pos = cursorPos(doc, start);
      if (!pos) continue;
      fragments.push({ id, change, text: deleted, rank: typeof rank === "number" ? rank : 0, at: pos.offset, stale: pos.stale, who: value.get("who"), author: value.get("author"), run: value.get("run") ?? null, when: value.get("at") });
    } else {
      if (value.get("kind") !== "change") continue;
      const startBytes = value.get("start");
      const endBytes = value.get("end");
      let from = 0;
      let to = 0;
      let stale = false;
      if (startBytes instanceof Uint8Array) {
        const s = cursorPos(doc, startBytes);
        if (!s) continue;
        from = s.offset;
        to = from;
        stale = s.stale;
        if (endBytes instanceof Uint8Array) {
          const e = cursorPos(doc, endBytes);
          if (!e) continue;
          if (e.stale) stale = true;
          to = e.stale ? e.offset : e.offset + charWidth(body, e.offset);
          if (to < from) to = from;
        }
      }
      changes.push({ id, who: value.get("who"), author: value.get("author"), run: value.get("run") ?? null, at: value.get("at"), from, to, stale, hasCursor: startBytes instanceof Uint8Array });
    }
  }
  // Fragments of a change that no longer exists were accepted by someone
  // else while they were written; they are inert and prune drops them.
  const known = new Set(changes.map((c) => c.id));
  const live = fragments.filter((f) => known.has(f.change));
  // A fragment never sits inside a span in one peer's sequence of edits, but
  // a span extended leftward by its author while another peer anchored a
  // fragment on its first character leaves one there after they sync. The
  // deleted text preceded the span's then first character, so it belongs
  // before the span.
  for (const f of live) {
    const inside = changes.find((c) => c.to > c.from && c.from < f.at && f.at < c.to);
    if (inside) f.at = inside.from;
  }
  const gaps = [];
  for (const f of live.sort((x, y) => x.at - y.at || byRank(x, y))) {
    const last = gaps[gaps.length - 1];
    if (last && last.at === f.at) {
      last.fragments.push({ id: f.id, change: f.change, text: f.text, rank: f.rank, stale: f.stale });
      if (f.stale) last.stale = true;
    } else gaps.push({ id: `gap@${f.at}`, at: f.at, fragments: [{ id: f.id, change: f.change, text: f.text, rank: f.rank, stale: f.stale }], stale: f.stale });
  }
  const out = [];
  for (const c of changes) {
    const g = gaps.find((x) => x.fragments.some((f) => f.change === c.id));
    if (c.to === c.from) {
      if (!g) continue;
      c.from = g.at;
      c.to = g.at;
    }
    out.push(c);
  }
  out.sort((a, b) => a.from - b.from || a.to - b.to || (a.id < b.id ? -1 : 1));
  return { changes: out, gaps, orphans: fragments.filter((f) => !known.has(f.change)) };
}

// The plain-value records of the earlier model in one file, by id.
function legacyRecords(doc, fileId) {
  /** @type {Map<string, any>} */
  const out = new Map();
  for (const [id, entry] of doc.getMap(CHANGES).entries()) {
    const value = /** @type {any} */ (entry);
    if (!value || value instanceof LoroMap || value.file !== fileId) continue;
    if (typeof value.deleted !== "string" || !(value.start instanceof Uint8Array)) continue;
    out.set(id, value);
  }
  return out;
}

// What the earlier model's records stand for: one change per record, and the
// deleted texts as fragments at the anchor of the record that holds them
// outermost, in the order the records' `points` give. Fragment ids derive
// from that outer record and the index, so every peer names them alike.
/**
 * @param {Map<string, any>} records
 * @returns {{ change: any, fragments: any[] }[]}
 */
function expandLegacy(records) {
  const pointed = new Set();
  for (const v of records.values()) for (const p of v.points ?? []) if (records.has(p?.id)) pointed.add(p.id);
  const out = [];
  const visited = new Set();
  /** @param {string} id @returns {{ change: string, text: string }[]} */
  const expand = (id) => {
    const v = records.get(id);
    if (!v || visited.has(id)) return [];
    visited.add(id);
    const points = (v.points ?? []).filter((/** @type {any} */ p) => p && records.has(p.id) && typeof p.at === "number");
    const inner = new Set();
    for (const p of points) for (const q of records.get(p.id).points ?? []) if (q) inner.add(q.id);
    const kept = points.filter((p) => !inner.has(p.id)).sort((a, b) => a.at - b.at);
    /** @type {{ change: string, text: string }[]} */
    const pieces = [];
    let cursor = 0;
    for (const p of kept) {
      if (p.at > cursor) pieces.push({ change: id, text: v.deleted.slice(cursor, p.at) });
      pieces.push(...expand(p.id));
      cursor = Math.max(cursor, p.at);
    }
    if (cursor < v.deleted.length) pieces.push({ change: id, text: v.deleted.slice(cursor) });
    return pieces;
  };
  for (const [id, v] of records) {
    const change = { id, who: v.who, author: v.author, run: v.run ?? null, at: v.at, start: v.start, end: v.end ?? null };
    if (pointed.has(id)) { out.push({ change, fragments: [] }); continue; }
    const fragments = expand(id).map((f, i) => ({ id: `${id}:${i}`, change: f.change, text: f.text, rank: i, start: v.start, legacy: id }));
    out.push({ change, fragments });
  }
  return out;
}

// Rewrites one outer legacy record, and the records it points at, as
// containers: the change records under their own ids and the fragments under
// their derived ids. Deletes the plain records. Returns false when `id` is
// not a legacy record of the file.
function convertLegacy(doc, fileId, id) {
  const records = legacyRecords(doc, fileId);
  if (!records.has(id)) return false;
  const expanded = expandLegacy(records);
  // The outer record holding this id's fragments, which may be another one.
  const outer = expanded.find((e) => e.change.id === id && e.fragments.length > 0)
    ?? expanded.find((e) => e.fragments.some((f) => f.change === id))
    ?? expanded.find((e) => e.change.id === id);
  const group = [outer, ...expanded.filter((e) => e !== outer && outer.fragments.some((f) => f.change === e.change.id))];
  const map = doc.getMap(CHANGES);
  for (const { change: c } of group) {
    map.delete(c.id);
    const record = map.setContainer(c.id, new LoroMap());
    record.set("kind", "change");
    record.set("file", fileId);
    record.set("who", c.who);
    record.set("author", c.author);
    record.set("run", c.run ?? null);
    record.set("at", c.at);
    record.set("start", c.end ? c.start : null);
    record.set("end", c.end ?? null);
  }
  const owners = new Map(group.map((e) => [e.change.id, e.change]));
  for (const f of outer.fragments) {
    const record = map.setContainer(f.id, new LoroMap());
    record.set("kind", "fragment");
    record.set("file", fileId);
    const o = owners.get(f.change);
    record.set("who", o?.who ?? "");
    record.set("author", o?.author ?? "");
    record.set("run", o?.run ?? null);
    record.set("at", o?.at ?? 0);
    record.set("change", f.change);
    record.set("text", f.text);
    record.set("start", f.start);
    record.set("rank", f.rank);
  }
  return true;
}

// Makes sure the record `id` (a change id, or a fragment id `<legacy>:<i>`)
// is a container, converting its legacy record first if need be.
function materialize(doc, fileId, id) {
  const map = doc.getMap(CHANGES);
  if (map.get(id) instanceof LoroMap) return;
  const colon = id.lastIndexOf(":");
  const legacyId = map.get(id) !== undefined ? id : colon > 0 ? id.slice(0, colon) : id;
  convertLegacy(doc, fileId, legacyId);
}

function deletedOf(gaps, id) {
  let out = "";
  for (const g of gaps) for (const f of g.fragments) if (f.change === id) out += f.text;
  return out;
}

/**
 * The pending changes of one file as offsets into its current text, one row
 * each. `deleted` is the concatenation of the change's fragments in document
 * order.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @returns {Row[]}
 */
export function resolveChanges(doc, fileId) {
  const { changes, gaps } = resolveFile(doc, fileId);
  return changes.map(({ stale, hasCursor, legacy, ...c }) => ({ ...c, deleted: deletedOf(gaps, c.id) }));
}

/**
 * Every pending change in every file, each with the text it inserted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 */
export function listChanges(doc) {
  const out = [];
  const seen = new Set();
  for (const [, value] of doc.getMap(CHANGES).entries()) {
    // Legacy records are plain values and name their file too.
    const file = value instanceof LoroMap ? value.get("file") : /** @type {any} */ (value)?.file;
    if (typeof file !== "string" || seen.has(file)) continue;
    seen.add(file);
    const text = fileText(doc, file);
    if (!text) continue;
    const body = text.toString();
    for (const c of resolveChanges(doc, file)) out.push({ ...c, file, inserted: body.slice(c.from, c.to) });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Writing

// Writes a change record: a new container, or only the keys of an existing
// one that changed. A cursor is rewritten only when it sits on another
// character now, so a peer that moved one end of the span and its author who
// moved the other both keep their write. With `heal`, a stale cursor is
// written again too, which settles it onto a live character.
function writeChange(doc, text, fileId, c, heal = false) {
  let start = null;
  let end = null;
  if (c.to > c.from) {
    start = text.getCursor(c.from, -1);
    end = text.getCursor(lastCharStart(text.toString(), c.to), 1);
    if (!start || !end) return false;
  }
  const map = doc.getMap(CHANGES);
  materialize(doc, fileId, c.id);
  let record = map.get(c.id);
  if (!(record instanceof LoroMap)) {
    record = map.setContainer(c.id, new LoroMap());
    record.set("kind", "change");
    record.set("file", fileId);
  }
  const put = (key, value) => { if (record.get(key) !== value) record.set(key, value); };
  put("who", c.who);
  put("author", c.author);
  put("run", c.run ?? null);
  put("at", c.at);
  // A cursor is rewritten only when its side of the span would resolve to
  // another offset than the stored one does: a stale cursor on a deleted
  // character already resolves to the right place (a stale end to the
  // exclusive end itself), so a span that shrank needs no write at all, and
  // the peer that shrank it and its author who extended its other end at the
  // same moment both keep their write. Nothing is written over a cursor with
  // null: a span emptied in this view may have grown in another.
  if (start) {
    const have = resolvedSpan(doc, text, record);
    const staleStart = heal && cursorPos(doc, record.get("start"))?.stale;
    const staleEnd = heal && cursorPos(doc, record.get("end"))?.stale;
    if (!have || have.from !== c.from || staleStart) record.set("start", start.encode());
    if (!have || have.to !== c.to || staleEnd) record.set("end", end.encode());
  } else if (record.get("start") === undefined) {
    record.set("start", null);
    record.set("end", null);
  }
  return true;
}

// The span the stored cursors resolve to now, or null without a span.
function resolvedSpan(doc, text, record) {
  const startBytes = record.get("start");
  const endBytes = record.get("end");
  if (!(startBytes instanceof Uint8Array) || !(endBytes instanceof Uint8Array)) return null;
  const s = cursorPos(doc, startBytes);
  const e = cursorPos(doc, endBytes);
  if (!s || !e) return null;
  const to = e.stale ? e.offset : e.offset + charWidth(text.toString(), e.offset);
  return { from: s.offset, to: Math.max(to, s.offset) };
}

// Sets `key` to `bytes` unless the stored cursor already resolves to `offset`.
function moveCursor(doc, record, key, bytes, offset) {
  const stored = record.get(key);
  if (stored instanceof Uint8Array) {
    if (sameCursor(stored, bytes)) return;
    const pos = cursorPos(doc, stored);
    if (pos && pos.offset === offset) return;
  }
  record.set(key, bytes);
}

// Ranks for the fragments of one gap, in order. Fragments that already carry
// ranks in rising order keep them; the others take values between their
// neighbours, so a peer that adds a fragment beside one of ours does not
// have to move ours.
function rankGap(fragments) {
  const have = fragments.map((f) => (typeof f.rank === "number" ? f.rank : null));
  // Longest rising run of existing ranks keeps its values.
  const n = have.length;
  const best = new Array(n).fill(1);
  const prev = new Array(n).fill(-1);
  for (let i = 0; i < n; i++) {
    if (have[i] === null) { best[i] = 0; continue; }
    for (let j = 0; j < i; j++) if (have[j] !== null && have[j] < have[i] && best[j] + 1 > best[i]) { best[i] = best[j] + 1; prev[i] = j; }
  }
  let end = -1;
  for (let i = 0; i < n; i++) if (best[i] > 0 && (end < 0 || best[i] > best[end])) end = i;
  const keep = new Set();
  for (let i = end; i >= 0; i = prev[i]) keep.add(i);
  const out = new Array(n);
  for (const i of keep) out[i] = have[i];
  for (let i = 0; i < n; i++) {
    if (keep.has(i)) continue;
    let lo = -Infinity;
    for (let j = i - 1; j >= 0; j--) if (out[j] !== undefined) { lo = out[j]; break; }
    let hi = Infinity;
    for (let j = i + 1; j < n; j++) if (keep.has(j)) { hi = out[j]; break; }
    out[i] = lo === -Infinity && hi === Infinity ? 0 : lo === -Infinity ? hi - 1 : hi === Infinity ? lo + 1 : (lo + hi) / 2;
    // Precision gone between two neighbours: push the rest up instead.
    if (out[i] <= lo || out[i] >= hi) { out[i] = lo + 1; for (let j = i + 1; j < n; j++) if (keep.has(j)) keep.delete(j); }
  }
  return out;
}

// Writes one fragment: a new container, or the keys of an existing one that
// changed. `owner` names the change the fragment belongs to, for the author
// fields a fragment carries so that it can be re-parented if its change goes.
// A fragment of a legacy record is converted together with that record, and
// only when something about it changes.
function writeFragment(doc, id, fileId, f, startBytes, at, rank, owner, heal = false) {
  const map = doc.getMap(CHANGES);
  let record = map.get(id);
  if (!(record instanceof LoroMap)) {
    // A fragment of a legacy record (id `<legacy>:<i>`) is rewritten, with
    // the whole record it belongs to, only when something about it changes.
    const colon = id.lastIndexOf(":");
    const legacyId = colon > 0 ? id.slice(0, colon) : null;
    const legacy = legacyId ? legacyRecords(doc, fileId).get(legacyId) : undefined;
    if (legacy) {
      const index = Number(id.slice(colon + 1));
      const was = expandLegacy(legacyRecords(doc, fileId)).find((e) => e.change.id === legacyId)?.fragments[index];
      const same = !heal && was && was.change === f.change && was.text === f.text && was.rank === rank
        && (!startBytes || sameCursor(legacy.start, startBytes) || cursorPos(doc, legacy.start)?.offset === at);
      if (same) return;
      convertLegacy(doc, fileId, legacyId);
      record = map.get(id);
    }
  }
  if (!(record instanceof LoroMap)) {
    record = map.setContainer(id, new LoroMap());
    record.set("kind", "fragment");
    record.set("file", fileId);
    if (owner) {
      record.set("who", owner.who);
      record.set("author", owner.author);
      record.set("run", owner.run ?? null);
      record.set("at", owner.at);
    }
  }
  if (record.get("change") !== f.change) record.set("change", f.change);
  if (record.get("text") !== f.text) record.set("text", f.text);
  if (startBytes) {
    if (heal && cursorPos(doc, record.get("start"))?.stale) record.set("start", startBytes);
    else moveCursor(doc, record, "start", startBytes, at);
  }
  if (record.get("rank") !== rank) record.set("rank", rank);
}

// Writes the fragments of one gap at `at`: each its own record, with rank.
// A fragment with no id gets a fresh one.
// Returns false when no cursor can be made.
function writeGap(doc, text, fileId, at, fragments, owners = new Map(), heal = false) {
  const start = text.getCursor(at, -1);
  if (!start) return false;
  const map = doc.getMap(CHANGES);
  const ranks = rankGap(fragments);
  const bytes = start.encode();
  fragments.forEach((f, i) => {
    if (!f.id) f.id = crypto.randomUUID();
    writeFragment(doc, f.id, fileId, f, bytes, at, ranks[i], owners.get(f.change), heal);
  });
  return true;
}

/**
 * Writes a plan against the file's current text: the gaps and changes marked
 * "set" get fresh cursors, and the records marked "removed" are deleted. Does
 * not commit. Returns the ids of the change records it wrote.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @param {PlannedState} plan
 * @returns {string[]}
 */
export function writePlanned(doc, fileId, plan) {
  const text = fileText(doc, fileId);
  if (!text) return [];
  const map = doc.getMap(CHANGES);
  const written = [];
  const live = new Set();
  for (const g of plan.gaps) if (g.state !== "removed") for (const f of g.fragments) if (f.id) live.add(f.id);
  const owners = new Map(plan.changes.filter((c) => c.state !== "removed").map((c) => /** @type {const} */ ([c.id, c])));
  for (const g of plan.gaps) {
    if (g.state === "removed") { for (const f of g.fragments) if (f.id && !live.has(f.id)) map.delete(f.id); }
    else if (g.state === "set") writeGap(doc, text, fileId, g.at, g.fragments, owners);
  }
  for (const c of plan.changes) {
    if (c.state === "removed") map.delete(c.id);
    else if (c.state === "set" && writeChange(doc, text, fileId, c)) written.push(c.id);
  }
  return written;
}

/**
 * Records edits that were already written into the file's `LoroText`.
 * `before` is the text before the batch and `preResolved` the file's state
 * before it. Does not commit. Returns the ids of the change records it wrote.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @param {string} before
 * @param {FileState} preResolved
 * @param {Edit[]} edits
 * @param {Me} me
 * @param {{ run?: string | null, now?: number }} [opts]
 * @returns {string[]}
 */
export function recordEdits(doc, fileId, before, preResolved, edits, me, { run = null, now = Date.now() } = {}) {
  if (!fileText(doc, fileId)) return [];
  const plan = planEdits(before, preResolved, edits, me, { run, now, mintId: () => crypto.randomUUID() });
  return writePlanned(doc, fileId, plan);
}

/**
 * Replaces `[from, to)` with `insert` in a file and records it as a tracked
 * change. Does not commit. Returns the ids of the change records it wrote.
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
  const pre = resolveFile(doc, fileId);
  if (to > from) text.delete(from, to - from);
  if (insert) text.insert(from, insert);
  return recordEdits(doc, fileId, before, pre, [{ from, to, insert }], me, opts);
}

/**
 * Replaces `[from, to)` with `insert` without tracking, keeping the records in
 * step: pending spans shrink or split, collapsed gaps merge. Does not commit.
 */
export function untrackedSplice(doc, fileId, from, to, insert) {
  const text = fileText(doc, fileId);
  if (!text) return;
  const before = text.toString();
  const pre = resolveFile(doc, fileId);
  if (to > from) text.delete(from, to - from);
  if (insert) text.insert(from, insert);
  if (pre.changes.length === 0 && pre.gaps.length === 0) return;
  const plan = planEdits(before, pre, [{ from, to, insert }], { who: "", author: "" }, { now: Date.now(), mintId: () => crypto.randomUUID(), untracked: true });
  writePlanned(doc, fileId, plan);
}

// ---------------------------------------------------------------------------
// Decisions

/**
 * Accepts a change: its fragments leave every gap, and its record is dropped.
 * The text stays. Returns false when the id is not a change record.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} id
 */
export function accept(doc, id) {
  const map = doc.getMap(CHANGES);
  const plain = /** @type {any} */ (map.get(id));
  if (plain !== undefined && !(plain instanceof LoroMap) && typeof plain.file === "string") convertLegacy(doc, plain.file, id);
  const value = map.get(id);
  if (!(value instanceof LoroMap) || value.get("kind") !== "change") return false;
  for (const [fid, fv] of [...map.entries()]) if (fv instanceof LoroMap && fv.get("kind") === "fragment" && fv.get("change") === id) map.delete(fid);
  map.delete(id);
  doc.commit();
  return true;
}

// Rejects one change without committing. See `reject`.
function rejectOne(doc, id) {
  const map = doc.getMap(CHANGES);
  const plain = /** @type {any} */ (map.get(id));
  if (plain !== undefined && !(plain instanceof LoroMap) && typeof plain.file === "string") convertLegacy(doc, plain.file, id);
  const value = map.get(id);
  if (!(value instanceof LoroMap) || value.get("kind") !== "change") return false;
  const file = /** @type {string} */ (value.get("file"));
  const text = fileText(doc, file);
  if (text) {
    const { changes, gaps } = resolveFile(doc, file);
    const target = changes.find((c) => c.id === id);
    const span = target && target.to > target.from ? { from: target.from, to: target.to } : null;
    const groups = new Map();
    for (const g of gaps) {
      const attached = span && g.at >= span.from && g.at <= span.to;
      if (!attached && !g.fragments.some((f) => f.change === id)) continue;
      const at = attached ? span.from : g.at;
      if (!groups.has(at)) groups.set(at, []);
      groups.get(at).push(g);
    }
    const restore = (at) => {
      const list = groups.get(at).sort((a, b) => a.at - b.at);
      // Gaps collapsing onto one point keep their order: later gaps rank above
      // earlier ones.
      let floor = -Infinity;
      const fragments = [];
      for (const g of list) {
        const ranks = g.fragments.map((f) => f.rank);
        const low = Math.min(...ranks);
        const shift = low <= floor ? floor - low + 1 : 0;
        for (const f of g.fragments) fragments.push({ ...f, rank: f.rank + shift });
        floor = Math.max(...ranks) + shift;
      }
      // Walk the fragments: my text comes back, and each run of others'
      // fragments is anchored on the restored character that follows it (or
      // keeps the anchor after the gap, which the restored text precedes).
      let pos = at;
      let pending = [];
      const flush = (anchor) => {
        if (pending.length === 0) return;
        const start = text.getCursor(anchor, -1);
        if (start) for (const f of pending) writeFragment(doc, f.id, file, f, start.encode(), anchor, f.rank);
        pending = [];
      };
      for (const f of fragments) {
        if (f.change === id) {
          // Spans ending or starting here keep their bounds across the
          // restored text; a cursor left on a deleted character might not.
          const edges = resolveFile(doc, file).changes.filter((c) => c.id !== id && c.to > c.from && (c.to === pos || c.from === pos));
          text.insert(pos, f.text);
          for (const c of edges) writeChange(doc, text, file, c.to === pos ? c : { ...c, from: c.from + f.text.length, to: c.to + f.text.length });
          flush(pos);
          pos += f.text.length;
          map.delete(f.id);
        } else pending.push(f);
      }
      flush(pos);
    };
    const positions = [...groups.keys()].sort((a, b) => b - a);
    for (const at of positions) if (!span || at > span.to) restore(at);
    if (span) text.delete(span.from, span.to - span.from);
    for (const at of positions) if (span && at <= span.to) restore(at);
  }
  for (const [fid, fv] of [...map.entries()]) if (fv instanceof LoroMap && fv.get("kind") === "fragment" && fv.get("change") === id) map.delete(fid);
  map.delete(id);
  return true;
}

/**
 * Rejects a change: puts the original text back, then drops the record.
 * Returns false when the id is not a change record. An orphaned record is only
 * dropped.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} id
 */
export function reject(doc, id) {
  if (!rejectOne(doc, id)) return false;
  doc.commit();
  return true;
}

/**
 * Rejects several changes in one commit, in any order. Returns the number
 * rejected.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string[]} ids
 * @returns {number}
 */
export function rejectMany(doc, ids) {
  let count = 0;
  for (const id of new Set(ids)) if (rejectOne(doc, id)) count += 1;
  if (count > 0) doc.commit();
  return count;
}

/**
 * Drops orphaned records, and empty ones that have stayed empty a while,
 * re-parents fragments whose change is gone, and re-anchors stale cursors that
 * have rested too.
 * Legacy records with nothing left are dropped as well; the others are read in
 * place. Does not commit. Returns the number of records deleted; re-parented
 * records are not counted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @returns {number}
 */
export function prune(doc) {
  const map = doc.getMap(CHANGES);
  let count = 0;
  const files = new Set();
  for (const [, v] of map.entries()) { const file = v instanceof LoroMap ? v.get("file") : /** @type {any} */ (v)?.file; if (typeof file === "string") files.add(file); }
  for (const file of files) {
    const text = fileText(doc, file);
    if (!text) {
      for (const [id, v] of [...map.entries()]) { if (v instanceof LoroMap && v.get("file") === file) { map.delete(id); count += 1; } }
      continue;
    }
    let { changes, gaps, orphans } = resolveFile(doc, file);
    // A fragment whose change was decided while it was written is given a
    // change of its own, so the deletion it records can still be decided.
    if (orphans.length > 0) {
      for (const f of orphans) {
        if (typeof f.who !== "string") continue;
        const id = crypto.randomUUID();
        writeChange(doc, text, file, { id, who: f.who, author: f.author ?? f.who, run: f.run ?? null, at: typeof f.when === "number" ? f.when : Date.now(), from: 0, to: 0 });
        /** @type {LoroMap} */ (map.get(f.id)).set("change", id);
      }
      ({ changes, gaps } = resolveFile(doc, file));
    }
    const liveIds = new Set(changes.map((c) => c.id));
    for (const g of gaps) for (const f of g.fragments) liveIds.add(f.id);
    // An empty record is dropped only once it has rested empty for a while: its
    // author may still be extending it in a session that has not synced yet.
    // The rest starts when a prune first sees it empty (`emptied`), not at its
    // last edit, so an old insertion deleted a moment ago still gets its grace.
    // A record found live again loses the mark.
    const now = Date.now();
    const settled = (v) => { const at = v.get("at"); return typeof at !== "number" || at < now - SETTLE_MS; };
    for (const [id, v] of [...map.entries()]) {
      if (!(v instanceof LoroMap) || v.get("file") !== file) continue;
      const emptiedAt = v.get("emptied");
      if (liveIds.has(id)) { if (emptiedAt !== undefined) v.delete("emptied"); continue; }
      if (typeof emptiedAt !== "number") { v.set("emptied", now); continue; }
      if (emptiedAt < now - SETTLE_MS) { map.delete(id); count += 1; }
    }
    // A legacy record with nothing left to show is dropped too.
    for (const [id, v] of legacyRecords(doc, file)) if (!liveIds.has(id) && !v.points?.length) { map.delete(id); count += 1; }
    const settledRecord = (id) => { const v = map.get(id); return v instanceof LoroMap ? settled(v) : true; };
    for (const g of gaps) if (g.stale && g.fragments.every((f) => settledRecord(f.id))) writeGap(doc, text, file, g.at, g.fragments, new Map(), true);
    for (const c of changes) if (c.stale && c.to > c.from && settledRecord(c.id)) writeChange(doc, text, file, c, true);
  }
  return count;
}
