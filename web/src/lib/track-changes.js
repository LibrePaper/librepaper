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
// offsets, decides them, keeps them in step with untracked edits
// (untrackedSplice) and converts the records of the earlier model
// (migrateChanges).
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
 * @typedef {Change & { stale: boolean, hasCursor: boolean }} ResolvedChange
 * @typedef {{ id: string, at: number, fragments: Fragment[] }} Gap
 * @typedef {{ id: string, at: number, fragments: ResolvedFragment[], stale: boolean }} ResolvedGap
 * @typedef {{ id: string, change: string, text: string, rank: number, at: number, stale: boolean, who: any, author: any, run: any, when: any }} OrphanFragment
 * @typedef {{ changes: Change[], gaps: Gap[] }} FileState
 * @typedef {Change & { deleted: string }} Row
 * @typedef {Change & { state: "kept" | "set" | "removed", emptied?: boolean }} PlannedChange
 * @typedef {Gap & { state: "kept" | "set" | "removed" }} PlannedGap
 * @typedef {{ changes: PlannedChange[], gaps: PlannedGap[], dropped: string[] }} PlannedState
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
 * nothing; the spans and gaps still follow the text. `dropped` lists the ids
 * of fragment records joined into a neighbour, whose records go.
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
  // Fragments joined into a neighbour along the way; their records go.
  const dropped = [];
  let adj = 0;
  for (const edit of edits) {
    const ef = edit.from + adj;
    const et = edit.to + adj;
    const insert = edit.insert;
    if (ef === et && insert.length === 0) continue;
    const step = planOne(cur, changes, gaps, ef, et, insert, me, run, now, mintId, untracked, dropped);
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
    dropped,
  };
}

function hasSpan(c) {
  return c.to > c.from;
}

function planOne(cur, changes, gaps, ef, et, insert, me, run, now, mintId, untracked, dropped) {
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
  // Adjacent fragments of one change join, and the record of the one that
  // joins goes.
  const pushFragment = (list, change, text, id) => {
    const last = list[list.length - 1];
    if (last && last.change === change) {
      last.text += text;
      if (id) dropped.push(id);
    } else list.push({ change, text, ...(id ? { id } : {}) });
  };
  for (let p = ef; p <= et; p++) {
    const target = splitAtEnd && p === et ? after : merged;
    // Untracked edits can leave two gaps on one point; every one of them is taken.
    for (const g of swallowedGaps) if (g.at === p) for (const f of g.fragments) pushFragment(target, mine && owned.has(f.change) ? mine.id : f.change, f.text, f.id);
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
    else if (owned.has(c.id)) {
      // My own span, deleted whole by me: the change goes unless fragments
      // of it remain elsewhere.
      outChanges.push({ ...c, from: ef + len, to: ef + len, state: "set" });
    } else {
      // Another author's whole span went from this view. The record stays,
      // untouched: its author may be extending it at this moment, and a stale
      // cursor resolves to the right place anyway. Prune drops it later if it
      // is still empty.
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
      for (const g of there) for (const f of g.fragments) pushFragment(list, f.change, f.text, f.id);
      for (const f of merged) pushFragment(list, f.change, f.text, f.id);
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
      for (const f of g.fragments) pushFragment(list, owned.has(f.change) ? mine.id : f.change, f.text, f.id);
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
// fragments; each writes its own records.

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
  return changes.map(({ stale, hasCursor, ...c }) => ({ ...c, deleted: deletedOf(gaps, c.id) }));
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
    const file = value instanceof LoroMap ? value.get("file") : undefined;
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
// moved the other both keep their write.
function writeChange(doc, text, fileId, c) {
  let start = null;
  let end = null;
  if (c.to > c.from) {
    start = text.getCursor(c.from, -1);
    end = text.getCursor(lastCharStart(text.toString(), c.to), 1);
    if (!start || !end) return false;
  }
  const map = doc.getMap(CHANGES);
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
    if (!have || have.from !== c.from) record.set("start", start.encode());
    if (!have || have.to !== c.to) record.set("end", end.encode());
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
function writeFragment(doc, id, fileId, f, startBytes, at, rank, owner) {
  const map = doc.getMap(CHANGES);
  let record = map.get(id);
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
  if (startBytes) moveCursor(doc, record, "start", startBytes, at);
  if (record.get("rank") !== rank) record.set("rank", rank);
}

// Writes the fragments of one gap at `at`: each its own record, with rank.
// Returns false when no cursor can be made.
function writeGap(doc, text, fileId, at, fragments, owners = new Map()) {
  const start = text.getCursor(at, -1);
  if (!start) return false;
  const map = doc.getMap(CHANGES);
  const ranks = rankGap(fragments);
  const bytes = start.encode();
  fragments.forEach((f, i) => {
    const id = f.id ?? crypto.randomUUID();
    f.id = id;
    writeFragment(doc, id, fileId, f, bytes, at, ranks[i], owners.get(f.change));
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
  for (const id of plan.dropped ?? []) if (!live.has(id)) map.delete(id);
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
  migrateChanges(doc);
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
  migrateChanges(doc);
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
  migrateChanges(doc);
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
 * Converts records of the earlier model in place. The editor calls this when
 * it opens a document and before anything is planned or written, so an old
 * pending change is never read as plain text. Idempotent, and the same on
 * every peer: fragment ids derive from the old record's id. Does not commit.
 * Returns the number of records converted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @returns {number}
 */
export function migrateChanges(doc) {
  const map = doc.getMap(CHANGES);
  const byFile = new Map();
  for (const [id, entry] of [...map.entries()]) {
    const v = /** @type {any} */ (entry);
    if (!v || v instanceof LoroMap || v.kind !== undefined || typeof v.file !== "string" || typeof v.deleted !== "string") continue;
    const end = v.end ?? null;
    if (!(v.start instanceof Uint8Array) || !(end === null || end instanceof Uint8Array)) continue;
    if (!byFile.has(v.file)) byFile.set(v.file, new Map());
    byFile.get(v.file).set(id, v);
  }
  let count = 0;
  for (const [file, records] of byFile) {
    for (const [id, v] of records) {
      const record = map.setContainer(id, new LoroMap());
      record.set("kind", "change");
      record.set("file", file);
      record.set("who", v.who);
      record.set("author", v.author);
      record.set("run", v.run ?? null);
      record.set("at", v.at);
      record.set("start", v.start);
      record.set("end", v.end ?? null);
      count += 1;
    }
    const visited = new Set();
    // The fragments of one old record: its deleted text split at each point,
    // with the pointed record's fragments in each split.
    const expand = (id) => {
      const v = records.get(id);
      if (!v || visited.has(id)) return [];
      visited.add(id);
      const points = (v.points ?? []).filter((p) => records.has(p.id));
      // A point that another point of this record lists sits inside that one.
      const own = points.filter((p) => !points.some((k) => k.id !== p.id && (records.get(k.id).points ?? []).some((q) => q.id === p.id)));
      own.sort((a, b) => a.at - b.at);
      const out = [];
      let cursor = 0;
      for (const p of own) {
        const piece = v.deleted.slice(cursor, p.at);
        if (piece !== "") out.push({ change: id, text: piece });
        out.push(...expand(p.id));
        cursor = p.at;
      }
      const rest = v.deleted.slice(cursor);
      if (rest !== "") out.push({ change: id, text: rest });
      return out;
    };
    const pointedAt = new Set();
    for (const [id, v] of records) for (const p of v.points ?? []) if (p.id !== id && records.has(p.id)) pointedAt.add(p.id);
    for (const [id, v] of records) {
      if (pointedAt.has(id)) continue;
      expand(id).forEach((f, index) => {
        const owner = records.get(f.change);
        const fragment = map.setContainer(`${id}:${index}`, new LoroMap());
        fragment.set("kind", "fragment");
        fragment.set("file", file);
        fragment.set("change", f.change);
        fragment.set("text", f.text);
        fragment.set("start", v.start);
        fragment.set("rank", index);
        fragment.set("who", owner.who);
        fragment.set("author", owner.author);
        fragment.set("run", owner.run ?? null);
        fragment.set("at", owner.at);
      });
    }
  }
  return count;
}

/**
 * Drops orphaned records, and empty ones that have rested, re-parents fragments
 * whose change is gone, and re-anchors stale cursors that have rested too.
 * Converts the records of the earlier model first. Does not commit. Returns the
 * number of records deleted; converted and re-parented records are not counted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @returns {number}
 */
export function prune(doc) {
  migrateChanges(doc);
  const map = doc.getMap(CHANGES);
  let count = 0;
  const files = new Set();
  for (const [, v] of map.entries()) { const file = v instanceof LoroMap ? v.get("file") : undefined; if (typeof file === "string") files.add(file); }
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
    // An empty record is dropped, and a stale cursor healed onto a live
    // character, only once the record has rested for a while: its author may
    // still be extending it in a session that has not synced yet, and a
    // write here would race that one.
    const settled = (v) => { const at = v.get("at"); return typeof at !== "number" || at < Date.now() - SETTLE_MS; };
    for (const [id, v] of [...map.entries()]) {
      if (!(v instanceof LoroMap) || v.get("file") !== file) continue;
      if (!liveIds.has(id) && settled(v)) { map.delete(id); count += 1; }
    }
    for (const g of gaps) if (g.stale && g.fragments.every((f) => settled(map.get(f.id)))) writeGap(doc, text, file, g.at, g.fragments);
    for (const c of changes) if (c.stale && c.to > c.from && settled(map.get(c.id))) writeChange(doc, text, file, c);
  }
  return count;
}
