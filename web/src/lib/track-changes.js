// Tracked changes live in the shared document, in the `changes` root map.
// A change record is one author's edit of one region: the text it inserted
// (a span, between two Loro cursors, or none) and nothing else. A gap record
// is one point in the text where text was deleted. Its cursor sits on the
// character after the gap, and its fragments are the deleted texts that sat
// there, in document order, each owned by a change. There is one gap per
// point, so a rejected change puts its text back where it was, whatever order
// the decisions come in.
//
// This module plans how an edit changes the records (pure, shared with the
// Rust port through the fixtures), writes the records, resolves them to
// offsets and decides them.
//
// Offsets are UTF-16 everywhere, the same as CodeMirror and loro-crdt in JS.

import { Cursor } from "loro-crdt";

export const CHANGES = "changes";

/**
 * @typedef {{ who: string, author: string }} Me
 * @typedef {{ from: number, to: number, insert: string }} Edit
 * @typedef {{ change: string, text: string }} Fragment
 * @typedef {{ id: string, who: string, author: string, run: string | null, at: number, from: number, to: number }} Change
 * @typedef {{ id: string, at: number, fragments: Fragment[] }} Gap
 * @typedef {{ changes: Change[], gaps: Gap[] }} FileState
 * @typedef {Change & { deleted: string }} Row
 * @typedef {Change & { state: "kept" | "set" | "removed" }} PlannedChange
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
 * last values.
 *
 * @param {string} before
 * @param {FileState} state
 * @param {Edit[]} edits
 * @param {Me} me
 * @param {{ run?: string | null, now: number, mintId: () => string }} opts
 * @returns {PlannedState}
 */
export function planEdits(before, state, edits, me, { run = null, now, mintId }) {
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
    const step = planOne(cur, changes, gaps, ef, et, insert, me, run, now, mintId);
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

function planOne(cur, changes, gaps, ef, et, insert, me, run, now, mintId) {
  const len = insert.length;
  const delta = len - (et - ef);
  // Own changes this edit touches: a span overlapping or adjacent, or a
  // fragment in a gap inside the closed range.
  const swallowedGaps = gaps.filter((g) => g.at >= ef && g.at <= et);
  const owned = new Set();
  for (const c of changes) {
    if (c.who !== me.who) continue;
    if (hasSpan(c) && c.from <= et && c.to >= ef) owned.add(c.id);
  }
  for (const g of swallowedGaps) for (const f of g.fragments) {
    const c = changes.find((x) => x.id === f.change);
    if (c && c.who === me.who) owned.add(c.id);
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
  const records = originalRuns.length > 0 || len > 0;
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
  const endGap = swallowedGaps.find((g) => g.at === et);
  const ownAtEnd = changes.some((c) => c.who === me.who && hasSpan(c) && c.from === et && owned.has(c.id))
    || Boolean(endGap && endGap.fragments.some((f) => owned.has(f.change)));
  const splitAtEnd = len > 0 && !ownAtEnd;
  const merged = [];
  const after = [];
  const pushFragment = (list, change, text) => {
    const last = list[list.length - 1];
    if (last && last.change === change) last.text += text;
    else list.push({ change, text });
  };
  for (let p = ef; p <= et; p++) {
    const g = swallowedGaps.find((x) => x.at === p);
    const target = splitAtEnd && p === et ? after : merged;
    if (g) for (const f of g.fragments) pushFragment(target, mine && owned.has(f.change) ? mine.id : f.change, f.text);
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
    if (c.to <= ef) { outChanges.push(c); continue; }
    if (c.from >= et) { outChanges.push({ ...c, from: c.from + delta, to: c.to + delta }); continue; }
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
      // The whole span went. The change stays only if it still owns fragments.
      outChanges.push({ ...c, from: ef + len, to: ef + len, state: "set" });
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
    // One gap per position: a gap already at the span's start, which held
    // text replaced by its earlier part, takes the new fragments after its own.
    const pos = mine ? mine.from : ef;
    const there = outGaps.find((g) => g.at === pos);
    if (there) {
      const list = there.fragments.map((f) => ({ ...f }));
      for (const f of merged) pushFragment(list, f.change, f.text);
      outGaps[outGaps.indexOf(there)] = { ...there, fragments: list, state: "set" };
    } else outGaps.push({ id: ids.shift() ?? mintId(), at: pos, fragments: merged, state: "set" });
  }
  if (after.length > 0) {
    outGaps.push({ id: ids.shift() ?? mintId(), at: ef + len, fragments: after, state: "set" });
  }
  // A change with no span and no fragment is gone.
  const referenced = new Set(outGaps.flatMap((g) => g.fragments.map((f) => f.change)));
  const live = outChanges.filter((c) => hasSpan(c) || referenced.has(c.id));
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
// Reading the records

function fileText(doc, fileId) {
  const text = doc.getMap("files").get(fileId);
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

/**
 * Resolves every record of a file to offsets. A change's `stale` flag is set
 * when one of its cursors pointed at a deleted character, and a gap's when its
 * cursor did. Records that do not resolve are left out.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @returns {FileState & { changes: Array<Change & { stale: boolean, hasCursor: boolean }>, gaps: Array<Gap & { stale: boolean }> }}
 */
export function resolveFile(doc, fileId) {
  const text = fileText(doc, fileId);
  if (!text) return { changes: [], gaps: [] };
  const body = text.toString();
  const changes = [];
  const gaps = [];
  for (const [id, value] of doc.getMap(CHANGES).entries()) {
    if (!value || value.file !== fileId) continue;
    if (value.kind === "gap") {
      if (!(value.start instanceof Uint8Array) || !Array.isArray(value.fragments)) continue;
      const pos = cursorPos(doc, value.start);
      if (!pos) continue;
      const fragments = value.fragments.filter((f) => f && typeof f.change === "string" && typeof f.text === "string" && f.text !== "").map((f) => ({ change: f.change, text: f.text }));
      if (fragments.length === 0) continue;
      gaps.push({ id, at: pos.offset, fragments, stale: pos.stale });
    } else if (value.kind === "change") {
      let from = 0;
      let to = 0;
      let stale = false;
      if (value.start instanceof Uint8Array) {
        const s = cursorPos(doc, value.start);
        if (!s) continue;
        from = s.offset;
        to = from;
        stale = s.stale;
        if (value.end instanceof Uint8Array) {
          const e = cursorPos(doc, value.end);
          if (!e) continue;
          if (e.stale) stale = true;
          to = e.stale ? e.offset : e.offset + charWidth(body, e.offset);
          if (to < from) to = from;
        }
      }
      changes.push({ id, who: value.who, author: value.author, run: value.run ?? null, at: value.at, from, to, stale, hasCursor: value.start instanceof Uint8Array });
    }
  }
  gaps.sort((a, b) => a.at - b.at);
  // A change with no span takes its place from its first fragment; one with
  // neither span nor fragment is empty and left out.
  const live = [];
  for (const c of changes) {
    const g = gaps.find((x) => x.fragments.some((f) => f.change === c.id));
    if (c.to === c.from) {
      if (!g) continue;
      c.from = g.at;
      c.to = g.at;
    }
    live.push(c);
  }
  live.sort((a, b) => a.from - b.from || a.to - b.to || (a.id < b.id ? -1 : 1));
  return { changes: live, gaps };
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
    if (!value || typeof value.file !== "string" || seen.has(value.file)) continue;
    seen.add(value.file);
    const text = fileText(doc, value.file);
    if (!text) continue;
    const body = text.toString();
    for (const c of resolveChanges(doc, value.file)) out.push({ ...c, file: value.file, inserted: body.slice(c.from, c.to) });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Writing the records

function gapValue(text, fileId, at, fragments) {
  const start = text.getCursor(at, -1);
  if (!start) return null;
  return { kind: "gap", file: fileId, start: start.encode(), fragments: fragments.map((f) => ({ change: f.change, text: f.text })) };
}

function changeValue(text, fileId, c) {
  let start = null;
  let end = null;
  if (c.to > c.from) {
    start = text.getCursor(c.from, -1);
    end = text.getCursor(lastCharStart(text.toString(), c.to), 1);
    if (!start || !end) return null;
  }
  return { kind: "change", file: fileId, who: c.who, author: c.author, run: c.run ?? null, at: c.at, start: start ? start.encode() : null, end: end ? end.encode() : null };
}

/**
 * Writes a plan: every "set" record with fresh cursors against the file's
 * current text, and deletes every "removed" id. Does not commit. Returns the
 * ids of the change records it wrote.
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
  for (const g of plan.gaps) {
    if (g.state === "removed") map.delete(g.id);
    else if (g.state === "set") {
      const value = gapValue(text, fileId, g.at, g.fragments);
      if (value) map.set(g.id, value);
    }
  }
  for (const c of plan.changes) {
    if (c.state === "removed") map.delete(c.id);
    else if (c.state === "set") {
      const value = changeValue(text, fileId, c);
      if (!value) continue;
      map.set(c.id, value);
      written.push(c.id);
    }
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
  if (!value || value.kind !== "change") return false;
  for (const [gid, gv] of [...map.entries()]) {
    if (gv?.kind !== "gap" || !Array.isArray(gv.fragments)) continue;
    const kept = gv.fragments.filter((f) => f?.change !== id);
    if (kept.length === gv.fragments.length) continue;
    if (kept.length === 0) map.delete(gid);
    else map.set(gid, { ...gv, fragments: kept });
  }
  map.delete(id);
  doc.commit();
  return true;
}

// Rejects one change without committing. See `reject`.
function rejectOne(doc, id) {
  const map = doc.getMap(CHANGES);
  const value = map.get(id);
  if (!value || value.kind !== "change") return false;
  const text = fileText(doc, value.file);
  if (text) {
    const { changes, gaps } = resolveFile(doc, value.file);
    const target = changes.find((c) => c.id === id);
    const span = target && target.to > target.from ? { from: target.from, to: target.to } : null;
    // The gaps to put back: every gap holding my fragments, and with a span
    // every gap from its start to the character after it. Those collapse onto
    // one point when the span goes, so they come back as one sequence there.
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
      const fragments = list.flatMap((g) => g.fragments);
      for (const g of list) map.delete(g.id);
      // Walk the fragments: my text comes back, and each run of others'
      // fragments becomes a gap anchored on what now follows it.
      let pos = at;
      let pending = [];
      const flush = (anchor) => {
        if (pending.length === 0) return;
        const v = gapValue(text, value.file, anchor, pending);
        if (v) map.set(crypto.randomUUID(), v);
        pending = [];
      };
      for (const f of fragments) {
        if (f.change === id) {
          text.insert(pos, f.text);
          flush(pos);
          pos += f.text.length;
        } else pending.push(f);
      }
      flush(pos);
    };
    // Gaps beyond the span first, while its text still holds their offsets;
    // then the span goes and the gaps at and before it come back.
    const positions = [...groups.keys()].sort((a, b) => b - a);
    for (const at of positions) if (!span || at > span.to) restore(at);
    if (span) text.delete(span.from, span.to - span.from);
    for (const at of positions) if (span && at <= span.to) restore(at);
  }
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

// Records from before the gap model become a change record and, when text was
// deleted, a gap of one fragment. Their old fields are dropped. The server
// ignores them until an editor does this.
function convertOld(doc) {
  const map = doc.getMap(CHANGES);
  for (const [id, entry] of [...map.entries()]) {
    const v = /** @type {any} */ (entry);
    if (!v || v.kind !== undefined || typeof v.file !== "string" || typeof v.deleted !== "string") continue;
    const end = v.end ?? null;
    if (!(v.start instanceof Uint8Array) || !(end === null || end instanceof Uint8Array)) continue;
    map.set(id, { kind: "change", file: v.file, who: v.who, author: v.author, run: v.run ?? null, at: v.at, start: v.start, end });
    if (v.deleted !== "") {
      map.set(crypto.randomUUID(), { kind: "gap", file: v.file, start: v.start, fragments: [{ change: id, text: v.deleted }] });
    }
  }
}

/**
 * Drops orphaned and empty records, re-anchors stale cursors and converts
 * records of the earlier model. Does not commit. Returns the number of records
 * deleted. Converted records are not counted.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 */
export function prune(doc) {
  convertOld(doc);
  const map = doc.getMap(CHANGES);
  let count = 0;
  const files = new Set();
  for (const [, v] of map.entries()) if (v && typeof v.file === "string") files.add(v.file);
  for (const file of files) {
    const text = fileText(doc, file);
    const { changes, gaps } = text ? resolveFile(doc, file) : { changes: [], gaps: [] };
    const liveIds = new Set([...changes.map((c) => c.id), ...gaps.map((g) => g.id)]);
    for (const [id, v] of [...map.entries()]) {
      if (v?.file !== file) continue;
      if (!liveIds.has(id)) { map.delete(id); count += 1; }
    }
    for (const g of gaps) if (g.stale) { const v = gapValue(text, file, g.at, g.fragments); if (v) map.set(g.id, v); }
    for (const c of changes) if (c.stale && c.to > c.from) { const v = changeValue(text, file, c); if (v) map.set(c.id, v); }
  }
  return count;
}
