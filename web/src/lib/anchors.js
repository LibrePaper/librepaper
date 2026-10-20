// Comment anchors live in the shared document, in the `anchors` root map: one
// map container per comment id. A record holds the quote the comment was made
// on (exact, prefix, suffix) and a `place`. The place is one plain value, not
// a container: { file, start, end } while the anchor is attached, with two
// encoded Loro cursors. A record whose text was deleted whole is detached:
// its place is { file, detached: true } with the old cursors and, when
// known, length, fragment (the id of the tracked deletion that holds the
// text) and offset. The place is one value because Loro merges map keys one
// at a time. Two peers that re-place the same comment at once would otherwise
// leave a record neither wrote, such as one peer's start with the other's end.
// Every placement change writes the whole place with one set. A record
// without a place is skipped by every reader.
//
// The planner (planOperation) is pure. It moves the attached anchors through
// one batch of edits made in this session, and re-pins detached anchors when an
// insertion brings their text back exactly once. The move memory is per
// session: text deleted whole is remembered, so that cut and paste, drag and
// drop, undo and agent patches keep comments exact. A cut that takes only one
// end of an anchor is remembered too, so putting that end back rejoins the
// anchor. Nothing searches the document for words.
//
// Resolving a stale cursor commits pending operations with no origin, so a
// writer sets the commit origin first and does all its reads before its first
// write.
//
// AnchorIndex keeps each file's resolved offsets for the session, so an edit
// resolves no cursor. Only the anchors an edit touched get cursors written.
//
// Offsets are UTF-16 everywhere, the same as CodeMirror and loro-crdt in JS.

import { Cursor, LoroMap } from "loro-crdt";
import { CHANGES, resolveFile } from "./track-changes.js";

export const ANCHORS = "anchors";
export const ANCHORS_ORIGIN = "anchors";
export const CONTEXT_UNITS = 32;

/**
 * @typedef {{ from: number, to: number, insert: string }} Edit
 *   Before-coordinates (UTF-16), ascending, non-overlapping. A composed list
 *   (see composeEdits) is in the running coordinates of planOperation instead.
 * @typedef {{ id: string, file: string, from: number, to: number, detached: boolean, fragment: string | null, offset: number | null, length: number | null, exact: string, prefix: string, suffix: string, collapsed: boolean, unresolved: boolean }} Anchor
 *   When detached, from and to are 0 and mean nothing. length is the span
 *   length in UTF-16 units at detachment; null when attached or when the span
 *   is gone.
 *   collapsed: attached, but its cursors resolve to to <= from.
 *   unresolved: a cursor cannot be resolved at all.
 * @typedef {Anchor & { state: "kept" | "set", touched: boolean, movedFrom?: string }} PlannedAnchor
 *   touched: its text was cut, overlapped by a deletion, or re-pinned in this
 *   operation. False for an anchor that only shifted or grew.
 * @typedef {{ id: string, edit: number, start: number, end: number }} FragmentRange
 *   edit is the index of the deleting edit in the file's edits. start and end
 *   are offsets within that edit's deleted text.
 * @typedef {{ id: string, offset: number, text: string, side?: "head" | "tail" }} Held
 *   offset is where the held text starts within the deleted text, and text is
 *   that text. A deletion that took the whole anchor holds the whole anchor and
 *   has no side. A deletion that took only the head or only the tail of an
 *   attached anchor holds just the piece it took, with side "head" or "tail".
 *   Such an entry rejoins the anchor when the piece comes back at the edge it
 *   was cut from, and the anchor must still be attached in the same file.
 * @typedef {{ id: string, text: string, at: number }} CreatedFragment
 *   A fragment that the deletions of one list created. at is the gap it sits
 *   in, as an offset into the text after every edit of the list.
 */

// Where needle occurs exactly once in hay, with the shift its pieces gain
// there. Overlapping occurrences count as two: "aa" in "aaa" is ambiguous.
function once(hay, needle, shift) {
  const at = hay.indexOf(needle);
  if (at < 0 || hay.indexOf(needle, at + 1) >= 0) return null;
  return { at, shift };
}

// A linewise cut and paste in Vim mode moves the newline from one end of the
// text to the other: `dd` takes "line\n" and `p` puts back "\nline", and on
// the last line `dd` takes "\nline" and `P` puts back "line\n". A remembered
// text that the insertion does not hold as it was is looked for with its
// newline at the other end, where a piece's offset in it shifts by one.
function movedNewline(hay, text) {
  if (text.length < 2) return null;
  if (text.endsWith("\n")) return once(hay, "\n" + text.slice(0, -1), 1);
  if (text.startsWith("\n")) return once(hay, text.slice(1) + "\n", -1);
  return null;
}

// What the session deleted, oldest first. Each entry keeps the pieces that the
// deletion took: anchors taken whole, and the head or tail of an anchor cut at
// one end, with their offsets in the deleted text. An entry leaves the memory
// when an insertion applies it (see planOperation), or when forget removes its
// last piece. An entry whose pieces applied nothing stays for a later insertion.
export class MoveMemory {
  #limit;
  #entries = [];

  /** @param {number} [limit] how many deletions are remembered */
  constructor(limit = 32) {
    this.#limit = limit;
  }

  /**
   * @param {string} text deleted text
   * @param {Held[]} held anchors the deletion took whole or cut at one end, with offsets relative to text
   */
  remember(text, held) {
    if (!text || !held || held.length === 0) return;
    this.#entries.push({ text, held: held.map((h) => ({ ...h })) });
    while (this.#entries.length > this.#limit) this.#entries.shift();
  }

  /**
   * Finds the newest remembered text that occurs exactly once in `inserted`
   * and whose occurrence `free` accepts. An entry that `free` refuses is
   * skipped, and the scan goes on to the older entries. The entry stays
   * remembered: the caller removes it with drop once one of its pieces applied.
   * @param {string} inserted
   * @param {(from: number, to: number) => boolean} [free] tells whether the span
   *   of `inserted` from `from` to `to` is still unclaimed. By default every
   *   span is free.
   * @param {number} [below] only entries below this index are scanned, so a
   *   caller passes the last hit's index to pass over an entry whose pieces did
   *   not apply. By default every entry is scanned.
   * @returns {{ entry: { text: string, held: Held[] }, index: number, at: number, shift: number } | null}
   *   at is where the text starts in `inserted`, as it was or with its newline
   *   moved (see movedNewline); a held offset lands at `at + shift + offset`.
   */
  find(inserted, free = () => true, below = this.#entries.length) {
    for (let i = Math.min(below, this.#entries.length) - 1; i >= 0; i--) {
      const entry = this.#entries[i];
      const hit = once(inserted, entry.text, 0) ?? movedNewline(inserted, entry.text);
      if (!hit || !free(hit.at, hit.at + entry.text.length)) continue;
      return { entry, index: i, at: hit.at, shift: hit.shift };
    }
    return null;
  }

  /**
   * Removes an entry that find returned. Entries match by identity, so an
   * entry that is already gone is left alone.
   * @param {{ text: string, held: Held[] }} entry
   */
  drop(entry) {
    const i = this.#entries.indexOf(entry);
    if (i >= 0) this.#entries.splice(i, 1);
  }

  /**
   * Forgets every held piece of one comment, in every entry, and drops the
   * entries left with no pieces. attachByHand calls this when it moves a
   * comment. Without it, a head or tail piece from the old passage could
   * rejoin the comment at the edge of the new passage, and a whole piece could
   * re-pin the comment over pasted text after the comment is cut again.
   * @param {string} id
   */
  forget(id) {
    for (let i = this.#entries.length - 1; i >= 0; i--) {
      const entry = this.#entries[i];
      entry.held = entry.held.filter((h) => h.id !== id);
      if (entry.held.length === 0) this.#entries.splice(i, 1);
    }
  }

  get size() {
    return this.#entries.length;
  }
}

/**
 * Pure, operation-level planner. No Loro here.
 *
 * Phase 1 runs each file on its own, edits in order, and moves its attached
 * anchors. The edits may be composed (see composeEdits), so a later edit can
 * land before an earlier insertion. That insertion moves when the later edit
 * lands before it, and is dropped when the later edit reaches into it.
 * Phase 2 runs after every file: each remembered text that occurs exactly once
 * in an insertion is found, newest first, and its pieces re-pin the detached
 * anchors that were in it, into the file the insertion landed in. Every hit
 * claims the span it covers in that insertion, so a later hit cannot claim the
 * same occurrence. A piece cut from one end of an attached anchor rejoins that
 * anchor when the piece comes back at the same end. An entry whose pieces
 * applied is removed from the memory. A remembered text whose pieces could not
 * be applied (the piece was pasted somewhere else, say) stays remembered for a
 * later insertion. Deletions are remembered.
 *
 * @param {{ fileId: string, before: string, anchors: Anchor[], edits: Edit[], fragments?: FragmentRange[] }[]} files
 * @param {MoveMemory} memory
 * @param {{ context?: number, foreign?: boolean }} [opts] foreign: a remote text
 *   change (see AnchorIndex). Phase 2 is skipped and nothing is remembered.
 * @returns {{ files: { fileId: string, after: string, anchors: PlannedAnchor[] }[] }}
 */
export function planOperation(files, memory, { context = CONTEXT_UNITS, foreign = false } = {}) {
  const work = files.map((file) => ({
    fileId: file.fileId,
    before: file.before,
    edits: file.edits,
    fragments: file.fragments ?? [],
    anchors: /** @type {PlannedAnchor[]} */ (file.anchors.map((a) => ({ ...a, state: "kept", touched: false }))),
    after: file.before,
    inserts: /** @type {{ at: number, text: string }[]} */ ([]),
  }));

  // Phase 1: each file on its own, edits in order.
  for (const w of work) {
    const moved = new Set();
    let cur = w.before;
    let adj = 0;
    for (let index = 0; index < w.edits.length; index++) {
      const edit = w.edits[index];
      const ef = edit.from + adj;
      const et = edit.to + adj;
      const n = edit.insert.length;
      if (ef === et && n === 0) continue;
      // Anchors this edit's deletion overlapped without covering them whole.
      const partial = new Set();
      if (et > ef) {
        const deleted = cur.slice(ef, et);
        /** @type {Held[]} */
        const held = [];
        for (const a of w.anchors) {
          if (!attached(a)) continue;
          const { from, to } = a;
          if (ef <= from && et >= to) {
            a.touched = true;
            if (foreign) {
              // The remote session writes the detached record with its text.
              Object.assign(a, { collapsed: true, from: ef, to: ef });
              continue;
            }
            // Wholly deleted. The quote keys stay as they are; the length tells a
            // later re-pin how much text to bring back.
            const fragment = w.fragments.find((f) => f.edit === index && from - ef >= f.start && to - ef <= f.end);
            held.push({ id: a.id, offset: from - ef, text: cur.slice(from, to) });
            Object.assign(a, {
              detached: true,
              from: 0,
              to: 0,
              length: to - from,
              fragment: fragment ? fragment.id : null,
              offset: fragment ? from - ef - fragment.start : null,
              state: "set",
            });
          } else {
            if (from < et && to > ef) {
              partial.add(a);
              a.touched = true;
              // A cut at one end is remembered with the piece it took, so that
              // putting the piece back at that end rejoins the anchor. A cut
              // strictly inside the anchor remembers nothing. A remote change
              // remembers nothing either.
              if (!foreign && ef <= from) {
                held.push({ id: a.id, offset: from - ef, text: cur.slice(from, et), side: "head" });
              } else if (!foreign && et >= to) {
                held.push({ id: a.id, offset: 0, text: cur.slice(ef, to), side: "tail" });
              }
            }
            a.from = afterDelete(from, ef, et);
            a.to = afterDelete(to, ef, et);
            if (a.from !== from || a.to !== to) moved.add(a);
          }
        }
        if (held.length > 0) memory.remember(deleted, held);
      }
      // Earlier insertions of this batch follow this edit, in running coordinates.
      followInserts(w.inserts, ef, et, n);
      if (n > 0) {
        for (const a of w.anchors) {
          if (!attached(a)) continue;
          // An anchor the deletion overlapped takes the new text inside it.
          // Otherwise the start side is Left: an insertion at the start moves the
          // anchor. The end side is Right: an insertion at the end stays outside.
          if (partial.has(a)) {
            a.to += n;
            moved.add(a);
          } else if (ef <= a.from) {
            a.from += n;
            a.to += n;
            moved.add(a);
          } else if (ef < a.to) {
            a.to += n;
            moved.add(a);
          }
        }
        w.inserts.push({ at: ef, text: edit.insert });
      }
      cur = cur.slice(0, ef) + edit.insert + cur.slice(et);
      adj += n - (et - ef);
    }
    w.after = cur;
    // A moved anchor is written again so that its cursors are checked. Its quote
    // keys stay as they are: a change inside the passage must show as modified.
    for (const a of moved) if (attached(a)) a.state = "set";
  }

  // Phase 2: an insertion re-pins the anchors that were in the text it brings
  // back. Each remembered text that occurs exactly once in the insertion is
  // found, newest first, and each hit is processed. A hit claims its span in
  // the insertion, so the spans that earlier hits claimed are not free, and two
  // entries for one pasted occurrence cannot both take it. Two disjoint pieces
  // each still take their own span. A whole anchor comes back from detached. A
  // cut head or tail rejoins an anchor that is still attached, in the same
  // file, at the edge the piece was cut from. A hit whose pieces applied
  // nothing stays remembered for a later insertion. The loop then looks for
  // the next hit. This entry is skipped, since its span is claimed.
  for (const w of foreign ? [] : work) {
    for (const ins of w.inserts) {
      /** @type {{ from: number, to: number }[]} */
      const claimed = [];
      const free = (from, to) => !claimed.some((c) => from < c.to && to > c.from);
      // Each find looks below the last hit, so an entry whose pieces did not
      // apply is passed over, and does not shadow an older entry with the same
      // text.
      let below = memory.size;
      for (let hit = memory.find(ins.text, free, below); hit; hit = memory.find(ins.text, free, below)) {
        const { entry } = hit;
        below = hit.index;
        let applied = false;
        for (const h of entry.held) {
          if (!h.text) continue;
          const owner = work.find((o) => o.anchors.some((a) => a.id === h.id));
          if (!owner) continue;
          const index = owner.anchors.findIndex((a) => a.id === h.id);
          const anchor = owner.anchors[index];
          const from = ins.at + hit.at + hit.shift + h.offset;
          if (from < 0) continue;
          const to = from + h.text.length;
          if (h.side) {
            const edge = h.side === "head" ? to === anchor.from : from === anchor.to;
            if (!attached(anchor) || owner !== w || !edge || w.after.slice(from, to) !== h.text) continue;
            if (h.side === "head") anchor.from = from;
            else anchor.to = to;
            anchor.state = "set";
            anchor.touched = true;
            applied = true;
            continue;
          }
          // A record with no length has no span to bring back.
          if (attached(anchor) || typeof anchor.length !== "number") continue;
          if (w.after.slice(from, to) !== h.text) continue;
          owner.anchors.splice(index, 1);
          /** @type {PlannedAnchor} */
          const pinned = {
            ...anchor,
            file: w.fileId,
            from,
            to,
            detached: false,
            fragment: null,
            offset: null,
            length: null,
            collapsed: false,
            unresolved: false,
            state: "set",
            touched: true,
          };
          if (owner !== w) pinned.movedFrom = owner.fileId;
          w.anchors.push(pinned);
          applied = true;
        }
        if (applied) {
          claimed.push({ from: hit.at, to: hit.at + entry.text.length });
          memory.drop(entry);
        }
      }
    }
  }

  return { files: work.map((w) => ({ fileId: w.fileId, after: w.after, anchors: w.anchors })) };
}

// Where a position goes when [ef, et) is deleted.
function afterDelete(p, ef, et) {
  if (p < ef) return p;
  if (p >= et) return p - (et - ef);
  return ef;
}

// Moves the insertions of one batch past an edit, in running coordinates. An
// insertion at or after the edit's end shifts by the edit's net length change.
// One that the edit overlaps is removed, since it no longer holds the text it
// was made with. One before the edit stays where it is.
function followInserts(inserts, ef, et, n) {
  for (let i = inserts.length - 1; i >= 0; i--) {
    const ins = inserts[i];
    if (et <= ins.at) {
      ins.at += n - (et - ef);
    } else if (ef >= ins.at + ins.text.length) {
      continue;
    } else {
      inserts.splice(i, 1);
    }
  }
}

// The planner moves only anchors that resolve to a span.
function attached(a) {
  return !a.detached && !a.collapsed && !a.unresolved;
}

// ---------------------------------------------------------------------------
// Loro layer

function fileText(doc, fileId) {
  const text = /** @type {any} */ (doc.getMap("files").get(fileId));
  return text && typeof text.toString === "function" && typeof text.getCursor === "function" ? text : null;
}

// The offset, and whether the cursor is stale, that a stored cursor resolves to
// in text, or null. A cursor made for another text does not resolve here.
function cursorPos(doc, text, bytes) {
  try {
    const cursor = Cursor.decode(bytes);
    if (!text || String(cursor.containerId()) !== String(text.id)) return null;
    const result = doc.getCursorPos(cursor);
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

const isBytes = (v) => v instanceof Uint8Array;
const isNum = (v) => typeof v === "number" && Number.isFinite(v);

// The record's place, normalized, or null when it has none. Cursor bytes are
// kept only when they are bytes, and the numbers only when they are finite.
function placeOf(record) {
  const p = record.get("place");
  if (!p || typeof p !== "object" || typeof p.file !== "string") return null;
  return {
    file: p.file,
    detached: p.detached === true,
    start: isBytes(p.start) ? p.start : undefined,
    end: isBytes(p.end) ? p.end : undefined,
    length: isNum(p.length) ? p.length : undefined,
    fragment: typeof p.fragment === "string" ? p.fragment : undefined,
    offset: isNum(p.offset) ? p.offset : undefined,
  };
}

function sameValue(a, b) {
  if (isBytes(a) && isBytes(b)) {
    return a.length === b.length && a.every((x, i) => x === b[i]);
  }
  return a === b;
}

// Whether the stored place is exactly `next`, so that the write can be skipped.
function samePlace(record, next) {
  const have = record.get("place");
  if (!have || typeof have !== "object") return false;
  const keys = Object.keys(next);
  if (Object.keys(have).length !== keys.length) return false;
  return keys.every((k) => sameValue(have[k], next[k]));
}

// The detached place that follows `old`: the cursors are kept when it had them.
function detachedPlace(fileId, old, { length, fragment, offset }) {
  const next = { file: fileId, detached: true };
  if (isNum(length)) next.length = length;
  if (typeof fragment === "string") next.fragment = fragment;
  if (isNum(offset)) next.offset = offset;
  if (old?.start) next.start = old.start;
  if (old?.end) next.end = old.end;
  return next;
}

// The span the stored cursors resolve to now, or null without a span.
function resolvedSpan(doc, text, place) {
  const startBytes = place?.start;
  const endBytes = place?.end;
  if (!(startBytes instanceof Uint8Array) || !(endBytes instanceof Uint8Array)) return null;
  const s = cursorPos(doc, text, startBytes);
  const e = cursorPos(doc, text, endBytes);
  if (!s || !e) return null;
  const to = e.stale ? e.offset : e.offset + charWidth(text.toString(), e.offset);
  return { from: s.offset, to: Math.max(to, s.offset) };
}

const str = (value) => (typeof value === "string" ? value : "");

// The fields every record carries, read without resolving its cursors.
function recordFields(value, id, fileId) {
  const place = placeOf(value);
  return {
    id,
    file: fileId,
    fragment: place?.fragment ?? null,
    offset: place?.offset ?? null,
    length: place?.length ?? null,
    exact: str(value.get("exact")),
    prefix: str(value.get("prefix")),
    suffix: str(value.get("suffix")),
  };
}

// A detached record as an anchor. Its cursors are not resolved.
function detachedAnchor(value, id, fileId) {
  return { ...recordFields(value, id, fileId), detached: true, from: 0, to: 0, collapsed: false, unresolved: false };
}

// One record as an anchor, its cursors resolved against body (the file's text).
function recordAnchor(doc, id, value, fileId, body) {
  const place = placeOf(value);
  if (place?.detached) return detachedAnchor(value, id, fileId);
  const base = recordFields(value, id, fileId);
  const text = fileText(doc, fileId);
  const s = place?.start ? cursorPos(doc, text, place.start) : null;
  const e = place?.end ? cursorPos(doc, text, place.end) : null;
  if (!s || !e) return { ...base, detached: false, from: 0, to: 0, collapsed: false, unresolved: true };
  const to = Math.max(s.offset, e.stale ? e.offset : e.offset + charWidth(body, e.offset));
  return { ...base, detached: false, from: s.offset, to, collapsed: to <= s.offset, unresolved: false };
}

/**
 * The anchor records of one file, resolved to offsets. Detached records are
 * returned without resolving their cursors.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @returns {Anchor[]}
 */
export function resolveAnchors(doc, fileId) {
  const text = fileText(doc, fileId);
  const body = text ? text.toString() : "";
  const out = [];
  for (const [id, entry] of doc.getMap(ANCHORS).entries()) {
    if (!(entry instanceof LoroMap) || placeOf(entry)?.file !== fileId) continue;
    out.push(recordAnchor(doc, id, /** @type {any} */ (entry), fileId, body));
  }
  return out;
}

/**
 * Creates an attached anchor record. Does not commit.
 * @returns {boolean} false when the file has no text, to <= from, or a cursor cannot be made
 */
export function createAnchor(doc, fileId, id, from, to, { exact, prefix, suffix }) {
  const text = fileText(doc, fileId);
  if (!text || !(to > from)) return false;
  const body = text.toString();
  const start = text.getCursor(from, -1);
  const end = text.getCursor(lastCharStart(body, to), 1);
  if (!start || !end) return false;
  doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  const record = doc.getMap(ANCHORS).setContainer(id, new LoroMap());
  record.set("place", { file: fileId, start: start.encode(), end: end.encode() });
  record.set("exact", exact);
  record.set("prefix", prefix);
  record.set("suffix", suffix);
  return true;
}

/**
 * Attaches a comment to a passage the author selected by hand. The record gets
 * a file, two fresh cursors and a quote taken from the selected text, and loses
 * whatever held it detached. A record that is not in the document yet is made.
 * The comment's remembered pieces are forgotten too (see MoveMemory.forget),
 * since they belong to its old passage. Commits under ANCHORS_ORIGIN, then
 * refreshes the index's entry for it.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {AnchorIndex} index
 * @param {MoveMemory} memory the session's move memory
 * @param {string} id
 * @param {string} fileId
 * @param {number} from UTF-16 offset in the file
 * @param {number} to UTF-16 offset in the file, past from
 * @returns {boolean} false when the file has no text, the span is empty or out of range, or a cursor cannot be made
 */
export function attachByHand(doc, index, memory, id, fileId, from, to) {
  const text = fileText(doc, fileId);
  if (!text) return false;
  const body = text.toString();
  if (!(to > from) || to > body.length) return false;
  // Reads come first. The commit origin is set before the first write.
  const start = text.getCursor(from, -1);
  const end = text.getCursor(lastCharStart(body, to), 1);
  if (!start || !end) return false;
  // The context stops short of a split surrogate pair at either edge.
  let prefixAt = Math.max(0, from - CONTEXT_UNITS);
  if (isLowSurrogate(body, prefixAt)) prefixAt++;
  let suffixEnd = Math.min(body.length, to + CONTEXT_UNITS);
  if (suffixEnd > to && isHighSurrogate(body, suffixEnd - 1)) suffixEnd--;
  const map = doc.getMap(ANCHORS);
  const held = map.get(id);
  doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  const record = held instanceof LoroMap ? held : map.setContainer(id, new LoroMap());
  record.set("place", { file: fileId, start: start.encode(), end: end.encode() });
  record.set("exact", body.slice(from, to));
  record.set("prefix", body.slice(prefixAt, from));
  record.set("suffix", body.slice(to, suffixEnd));
  doc.commit({ origin: ANCHORS_ORIGIN });
  memory.forget(id);
  index.refresh([id]);
  return true;
}

function isHighSurrogate(body, at) {
  const code = body.charCodeAt(at);
  return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(body, at) {
  const code = body.charCodeAt(at);
  return code >= 0xdc00 && code <= 0xdfff;
}

/**
 * Writes the anchors marked "set" in a plan. A detached record keeps its
 * cursors. An attached anchor that was touched gets both cursors, fresh. One
 * that only shifted (touched false) is skipped: its cursors follow its text.
 * A plan built by hand has no touched flag: there a cursor is written only
 * when the stored one resolves to another offset, and a record coming back
 * from detached, or from another file, always gets both. Does not commit.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {{ files: { fileId: string, after: string, anchors: PlannedAnchor[] }[] }} planned
 * @returns {string[]} the ids written
 */
export function writeAnchors(doc, planned) {
  const map = doc.getMap(ANCHORS);
  // Pass 1 reads everything and works out the writes. Resolving a stale cursor
  // commits pending operations, so no read may come after the first write.
  /** @type {{ id: string, record: any, place: object | null }[]} */
  const pending = [];
  for (const file of planned.files) {
    const text = fileText(doc, file.fileId);
    for (const a of file.anchors) {
      if (a.state !== "set") continue;
      if (!a.detached && a.touched === false) continue;
      const record = map.get(a.id);
      if (!(record instanceof LoroMap)) continue;
      const old = placeOf(record);
      if (a.detached) {
        const next = detachedPlace(file.fileId, old, a);
        pending.push({ id: a.id, record, place: samePlace(record, next) ? null : next });
        continue;
      }
      if (!text || !(a.to > a.from)) continue;
      const body = text.toString();
      const start = text.getCursor(a.from, -1);
      const end = text.getCursor(lastCharStart(body, a.to), 1);
      if (!start || !end) continue;
      const reattach = !old || old.detached || old.file !== file.fileId;
      const have = a.touched === true || reattach ? null : resolvedSpan(doc, text, old);
      const next = {
        file: file.fileId,
        start: have && have.from === a.from && old?.start ? old.start : start.encode(),
        end: have && have.to === a.to && old?.end ? old.end : end.encode(),
      };
      pending.push({ id: a.id, record, place: samePlace(record, next) ? null : next });
    }
  }
  // Pass 2 writes. The commit origin is set before the first write, so the
  // commit that carries these writes is labelled as anchor work.
  const dirty = pending.filter((p) => p.place);
  if (dirty.length > 0) doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  for (const p of dirty) p.record.set("place", p.place);
  return pending.map((p) => p.id);
}

/**
 * Marks the attached anchors of a file whose cursors resolve collapsed as
 * detached. Their quotes stay as they are, and no length is recorded: the span
 * is gone, so nothing can be re-pinned. Does not commit.
 * @returns {string[]} the ids marked
 */
export function markCollapsed(doc, fileId) {
  const map = doc.getMap(ANCHORS);
  // Reads come first. The commit origin is set before the first write.
  const marked = [];
  for (const a of resolveAnchors(doc, fileId)) {
    const record = map.get(a.id);
    if (!a.collapsed || !(record instanceof LoroMap)) continue;
    const stored = record.get("place");
    if (stored && typeof stored === "object") marked.push({ id: a.id, record, place: { ...stored, detached: true } });
  }
  if (marked.length > 0) doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  for (const { record, place } of marked) record.set("place", place);
  return marked.map((m) => m.id);
}

/**
 * Plans and writes an operation whose text is already edited. The anchors in
 * `files` are the ones resolved before the edits. An anchor that arrived
 * collapsed is treated as detached and written detached. Detached anchors of
 * files this operation does not name join the plan with no edits, so that an
 * insertion here can re-pin them. Those come from opts.others when it is
 * given, and otherwise from a scan of the document. Does not commit.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {{ fileId: string, before: string, anchors: Anchor[], edits: Edit[], fragments?: FragmentRange[] }[]} files
 * @param {MoveMemory} memory
 * @param {{ context?: number, others?: { fileId: string, before: string, anchors: Anchor[], edits: Edit[] }[] }} [opts]
 *   others: the pseudo-files for files this operation does not name, each with
 *   its detached anchors. When given, the document is not scanned.
 * @returns {{ files: { fileId: string, after: string, anchors: PlannedAnchor[] }[] }}
 */
export function recordOperation(doc, files, memory, opts) {
  const { others, ...planOpts } = opts ?? {};
  const collapsed = new Set();
  const input = files.map((file) => ({
    ...file,
    anchors: file.anchors.map((a) => {
      if (a.detached || a.unresolved || !a.collapsed) return a;
      collapsed.add(a.id);
      return { ...a, detached: true, collapsed: false, from: 0, to: 0, length: null };
    }),
  }));
  if (others) {
    input.push(...others);
  } else {
    const named = new Set(files.map((file) => file.fileId));
    const elsewhere = new Map();
    for (const [id, entry] of doc.getMap(ANCHORS).entries()) {
      if (!(entry instanceof LoroMap)) continue;
      const value = /** @type {any} */ (entry);
      const place = placeOf(value);
      if (!place || named.has(place.file) || !place.detached) continue;
      const fileId = place.file;
      if (!elsewhere.has(fileId)) elsewhere.set(fileId, []);
      elsewhere.get(fileId).push(detachedAnchor(value, id, fileId));
    }
    for (const [fileId, anchors] of elsewhere) {
      const text = fileText(doc, fileId);
      input.push({ fileId, before: text ? text.toString() : "", anchors, edits: [] });
    }
  }
  const plan = planOperation(input, memory, planOpts);
  for (const file of plan.files) {
    for (const a of file.anchors) if (collapsed.has(a.id)) a.state = "set";
  }
  writeAnchors(doc, plan);
  return plan;
}

/**
 * Convenience for tests and agents: applies the edits to each file's text,
 * then records them. Commits the text, then the anchors under ANCHORS_ORIGIN.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {{ fileId: string, edits: Edit[], fragments?: FragmentRange[] }[]} files
 * @param {MoveMemory} memory
 * @param {{ context?: number }} [opts]
 * @returns {{ files: { fileId: string, after: string, anchors: PlannedAnchor[] }[] }}
 */
export function applyOperation(doc, files, memory, opts) {
  const prepared = [];
  for (const file of files) {
    const text = fileText(doc, file.fileId);
    if (!text) continue;
    prepared.push({
      fileId: file.fileId,
      before: text.toString(),
      anchors: resolveAnchors(doc, file.fileId),
      edits: file.edits,
      fragments: file.fragments,
      text,
    });
  }
  for (const p of prepared) {
    let adj = 0;
    for (const edit of p.edits) {
      const ef = edit.from + adj;
      const et = edit.to + adj;
      if (et > ef) p.text.delete(ef, et - ef);
      if (edit.insert) p.text.insert(ef, edit.insert);
      adj += edit.insert.length - (edit.to - edit.from);
    }
  }
  doc.commit();
  const plan = recordOperation(
    doc,
    prepared.map(({ fileId, before, anchors, edits, fragments }) => ({ fileId, before, anchors, edits, fragments })),
    memory,
    opts,
  );
  doc.commit({ origin: ANCHORS_ORIGIN });
  return plan;
}

/**
 * Converts a Loro text delta into edits in before-coordinates. A delete and an
 * insert at the same point make one replacement edit.
 *
 * @param {({ retain?: number, insert?: string, delete?: number })[]} delta
 * @returns {Edit[]}
 */
export function editsFromDelta(delta) {
  /** @type {Edit[]} */
  const edits = [];
  let pos = 0;
  /** @type {Edit | null} */
  let open = null;
  for (const op of delta) {
    if (op.retain) {
      pos += op.retain;
      open = null;
    } else if (typeof op.insert === "string") {
      if (!open) {
        open = { from: pos, to: pos, insert: "" };
        edits.push(open);
      }
      open.insert += op.insert;
    } else if (typeof op.delete === "number") {
      if (!open) {
        open = { from: pos, to: pos, insert: "" };
        edits.push(open);
      }
      open.to += op.delete;
      pos += op.delete;
    }
  }
  return edits.filter((e) => e.to > e.from || e.insert.length > 0);
}

/**
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {Anchor} anchor
 * @param {string} [body] the file's text, when the caller already has it
 * @returns {"exact" | "modified" | "pending_deletion" | "deleted" | "unresolved"}
 */
export function statusOf(doc, anchor, body) {
  if (anchor.unresolved) return "unresolved";
  if (anchor.detached) {
    const record = anchor.fragment ? doc.getMap(CHANGES).get(anchor.fragment) : undefined;
    return record instanceof LoroMap ? "pending_deletion" : "deleted";
  }
  if (anchor.collapsed) return "deleted";
  if (body === undefined) {
    const text = fileText(doc, anchor.file);
    body = text ? text.toString() : "";
  }
  return body.slice(anchor.from, anchor.to) === anchor.exact ? "exact" : "modified";
}

/**
 * Joins the edit lists of one update into one list for planOperation. The
 * planner runs a single batch, but an update can hold several editor
 * transactions. Each list is in the coordinates of the text its transaction
 * started from, which is the text after the lists before it. planOperation
 * adds the net length change of the edits before an edit to its position, so
 * each edit of a later list is reduced by the net length change of all earlier
 * lists. Its insert is unchanged. The result is in planOperation's coordinates,
 * and its edits apply in order without ascending: a later list may edit before
 * an earlier one.
 *
 * @param {Edit[][]} lists one list per transaction, oldest first
 * @returns {Edit[]}
 */
export function composeEdits(lists) {
  /** @type {Edit[]} */
  const out = [];
  let delta = 0;
  for (const list of lists) {
    let listDelta = 0;
    for (const edit of list) {
      out.push({ from: edit.from - delta, to: edit.to - delta, insert: edit.insert });
      listDelta += edit.insert.length - (edit.to - edit.from);
    }
    delta += listDelta;
  }
  return out;
}

/**
 * The fragment ranges of the deletions in one transaction. The walk mirrors the
 * Rust fragment_ranges. Each deletion, in edit order, looks for the first run of
 * created fragments that sit at its gap and whose texts join up to exactly its
 * deleted text. The gap is where the deleted text sits in the final text, after
 * every edit of the list. It starts at the deletion's own position and each
 * later edit moves it. A later edit that ends at or before the position shifts
 * it by that edit's net length change, so an insertion exactly at the position
 * moves it past the inserted text. A later edit that deletes across the position
 * moves it to the start of that edit. A run is contiguous in the list and every
 * fragment in it is still unconsumed. In the gap search every fragment also sits
 * at that gap, and the run may start at any such fragment. If no run sits at the
 * gap, the deletion takes the first run at any gap whose texts join up to its
 * deleted text. That fallback exists because a deletion that joins the author's
 * own pending change files its fragment at that change's start, not at the gap.
 * Each fragment of the run becomes one range, with start and end inside that
 * text, and is consumed. A deletion with no such run gets no ranges and consumes
 * nothing, so a later deletion can still take those fragments. That covers an
 * untracked deletion and a deletion whose text was joined with something else.
 *
 * Matching by gap keeps two deletions of the same text apart. A composed list
 * (see composeEdits) is in transaction order, so a later transaction may delete
 * text that sits before an earlier one's deletion. The edits are in
 * planOperation's coordinates, so each one is matched against the running text.
 *
 * @param {string} before the text the edits apply to
 * @param {Edit[]} edits
 * @param {CreatedFragment[]} fragments the created fragments, each with its gap (at) in the text after every edit
 * @returns {FragmentRange[]}
 */
export function fragmentRangesFor(before, edits, fragments) {
  const consumed = fragments.map(() => false);
  // The first run of unconsumed fragments that joins up to `deleted`,
  // contiguous in the list. With a gap, every fragment of the run sits at that
  // gap; with null, the run may sit at any gap.
  const firstRun = (deleted, gap) => {
    const fits = (i) => !consumed[i] && (gap === null || fragments[i].at === gap);
    for (let first = 0; first < fragments.length; first++) {
      if (!fits(first)) continue;
      let last = first;
      let joined = "";
      while (last < fragments.length && fits(last) && joined.length < deleted.length) {
        joined += fragments[last].text;
        last++;
      }
      if (joined === deleted) return { first, last };
    }
    return null;
  };
  // Each edit's span in the running text, computed the way planOperation does.
  const spans = [];
  let adj = 0;
  for (const edit of edits) {
    spans.push({ ef: edit.from + adj, et: edit.to + adj, n: edit.insert.length });
    adj += edit.insert.length - (edit.to - edit.from);
  }
  // Each deletion's text, and where it sits in the final text: each later
  // edit moves that point.
  const deletions = [];
  let cur = before;
  edits.forEach((edit, index) => {
    const { ef, et } = spans[index];
    if (et > ef) {
      let point = ef;
      for (const later of spans.slice(index + 1)) {
        if (later.et <= point) point += later.n - (later.et - later.ef);
        else if (later.ef < point) point = later.ef;
      }
      deletions.push({ index, deleted: cur.slice(ef, et), point, run: null });
    }
    cur = cur.slice(0, ef) + edit.insert + cur.slice(et);
  });
  // The gap search runs for every deletion before any fallback, so identical
  // texts at different gaps keep their own fragments. The fallback covers a
  // deletion that joins the author's own pending change, which files its
  // fragment at that change's start.
  const take = (deletion, gap) => {
    const run = firstRun(deletion.deleted, gap);
    if (!run) return;
    deletion.run = run;
    for (let i = run.first; i < run.last; i++) consumed[i] = true;
  };
  for (const deletion of deletions) take(deletion, deletion.point);
  for (const deletion of deletions) if (!deletion.run) take(deletion, null);
  const out = [];
  for (const { index, run } of deletions) {
    if (!run) continue;
    let start = 0;
    for (let i = run.first; i < run.last; i++) {
      const fragment = fragments[i];
      out.push({ id: fragment.id, edit: index, start, end: start + fragment.text.length });
      start += fragment.text.length;
    }
  }
  return out;
}

/**
 * The source attachment of a comment, read from the document's anchor records:
 * its status, the range it resolves to and the file that holds it. This is the
 * shape the server used to send. A comment with no source passage has none.
 *
 * `attached_by_hand` is true when the record's quote is not the quote the
 * comment was made with. The record's quote is written when the comment is
 * made and rewritten only by `attachByHand`, so a difference means an editor
 * attached the comment to a passage by hand. It is false when the comment has
 * no record in the document, and when its original quote is not a string.
 *
 * `diagnostic` is "removed_file" when the file the record names is gone, or is
 * no longer a text. It is null on every other path, the no-record path included.
 *
 * `bodies` caches each file's text across one pass over many comments. Pass
 * the same Map to every call of that pass. A file that is gone is cached as
 * null.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {AnchorIndex} index
 * @param {{ id: string, original_anchor?: { kind?: string, target?: { exact?: string } } }} comment
 * @param {Map<string, string | null>} [bodies]
 * @returns {{ status: string, resolved_range_utf16: [number, number] | null, resolved_file_id: string | null, tree_digest: string, attached_by_hand: boolean, diagnostic: "removed_file" | null } | null}
 */
export function deriveAttachment(doc, index, comment, bodies = new Map()) {
  if (comment?.original_anchor?.kind !== "source_text") return null;
  const hit = index.find(String(comment.id));
  if (!hit) return { status: "unresolved", resolved_range_utf16: null, resolved_file_id: null, tree_digest: "", attached_by_hand: false, diagnostic: null };
  const { anchor } = hit;
  const original = comment.original_anchor?.target?.exact;
  const attached_by_hand = typeof original === "string" && anchor.exact !== original;
  // A file that was removed, or is no longer a text, is not a deleted passage.
  // The comment is unresolved, with the removed_file diagnostic, and no status is read.
  if (!bodies.has(anchor.file)) {
    const text = fileText(doc, anchor.file);
    bodies.set(anchor.file, text ? text.toString() : null);
  }
  const body = bodies.get(anchor.file) ?? null;
  if (body === null) {
    return { status: "unresolved", resolved_range_utf16: null, resolved_file_id: null, tree_digest: "", attached_by_hand, diagnostic: "removed_file" };
  }
  const status = statusOf(doc, anchor, body);
  const placed = status === "exact" || status === "modified";
  return {
    status,
    resolved_range_utf16: placed ? [anchor.from, anchor.to] : null,
    resolved_file_id: anchor.file,
    tree_digest: "",
    attached_by_hand,
    diagnostic: null,
  };
}

/**
 * Re-pins the anchors whose fragment was restored as new text: a rejected
 * tracked deletion put its text back. The restored text must already be in
 * the document. Does not commit.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {string} fileId
 * @param {{ fragment: string, at: number, text: string }[]} restored
 * @returns {string[]} the ids re-pinned
 */
export function restoreByFragments(doc, fileId, restored) {
  const text = fileText(doc, fileId);
  if (!text || restored.length === 0) return [];
  const body = text.toString();
  const byFragment = new Map(restored.map((r) => [r.fragment, r]));
  const map = doc.getMap(ANCHORS);
  // Reads come first. The commit origin is set before the first write.
  const repins = [];
  for (const [id, entry] of map.entries()) {
    if (!(entry instanceof LoroMap)) continue;
    const value = /** @type {any} */ (entry);
    const place = placeOf(value);
    if (!place || place.file !== fileId || !place.detached) continue;
    const { fragment, offset, length } = place;
    const r = typeof fragment === "string" ? byFragment.get(fragment) : undefined;
    if (!r || typeof offset !== "number" || typeof length !== "number") continue;
    const from = r.at + offset;
    const to = Math.min(from + length, r.at + r.text.length);
    if (!(to > from)) continue;
    const start = text.getCursor(from, -1);
    const end = text.getCursor(lastCharStart(body, to), 1);
    if (!start || !end) continue;
    repins.push({ id, value, place: { file: fileId, start: start.encode(), end: end.encode() } });
  }
  if (repins.length > 0) doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  for (const { value, place } of repins) value.set("place", place);
  return repins.map((r) => r.id);
}

// The fields an Anchor carries, without the planner's bookkeeping.
function toAnchor(a) {
  return {
    id: a.id,
    file: a.file,
    from: a.from,
    to: a.to,
    detached: a.detached,
    fragment: a.fragment,
    offset: a.offset,
    length: a.length,
    exact: a.exact,
    prefix: a.prefix,
    suffix: a.suffix,
    collapsed: a.collapsed,
    unresolved: a.unresolved,
  };
}

/**
 * The resolved anchors of each file, kept current in this session. Local
 * operations go through record() and shift the cache with the planner, so an
 * edit resolves no cursor. Remote text changes shift the cache in foreign mode,
 * and the records another session changed are resolved again on import.
 *
 * An undo or redo in a file that is not on screen produces no editor
 * transaction, so the index follows undo and redo from the document itself
 * (see attach). The editor plugin records only the user's own edits.
 */
export class AnchorIndex {
  #doc;
  /** @type {Map<string, { text: string, anchors: Anchor[] }>} */
  #files = new Map();
  /** text container id to file id, for the files that are cached */
  #texts = new Map();
  /** record container id to record id, and back */
  #recordIds = new Map();
  #containerIds = new Map();
  #scratch = new MoveMemory();

  /** @param {import("loro-crdt").LoroDoc} doc */
  constructor(doc) {
    this.#doc = doc;
  }

  /**
   * Resolves a file's records from the document and caches them, with the
   * text. Each record's container is remembered for its later changes. The
   * file maps to its current text container only, so an older container of
   * the file no longer maps to it.
   * @param {string} fileId
   * @returns {{ text: string, anchors: Anchor[] }}
   */
  load(fileId) {
    const doc = this.#doc;
    const text = fileText(doc, fileId);
    const body = text ? text.toString() : "";
    const anchors = [];
    for (const [id, entry] of doc.getMap(ANCHORS).entries()) {
      if (!(entry instanceof LoroMap)) continue;
      const value = /** @type {any} */ (entry);
      if (placeOf(value)?.file !== fileId) continue;
      this.#remember(id, value);
      anchors.push(toAnchor(recordAnchor(doc, id, value, fileId, body)));
    }
    this.#unmap(fileId);
    if (text) this.#texts.set(text.id, fileId);
    const entry = { text: body, anchors };
    this.#files.set(fileId, entry);
    return entry;
  }

  /**
   * The cached text and anchors of a file, loading it when absent.
   * @param {string} fileId
   * @returns {{ text: string, anchors: Anchor[] }}
   */
  get(fileId) {
    return this.#files.get(fileId) ?? this.load(fileId);
  }

  /**
   * Whether a file is cached, so that an edit to it can be recorded.
   * @param {string} fileId
   * @returns {boolean}
   */
  has(fileId) {
    return this.#files.has(fileId);
  }

  /**
   * The cached anchors of a file, loading it when absent.
   * @param {string} fileId
   * @returns {Anchor[]}
   */
  anchors(fileId) {
    return this.get(fileId).anchors;
  }

  /**
   * The anchor of one comment and the file it sits in. A record that is in the
   * document but not yet cached is loaded from its file first.
   * @param {string} id
   * @returns {{ fileId: string, anchor: Anchor } | null} null when there is no record
   */
  find(id) {
    const hit = this.#locate(id);
    if (hit) return hit;
    const record = this.#doc.getMap(ANCHORS).get(id);
    const fileId = record instanceof LoroMap ? placeOf(record)?.file : undefined;
    if (typeof fileId !== "string") return null;
    this.load(fileId);
    return this.#locate(id);
  }

  /**
   * Records an operation whose text is already in the document. The cached
   * text and anchors are the before state, so no cursor is resolved. The cache
   * takes the planned text and anchors of every file in the plan. Files must be
   * cached before their text is edited. Does not commit.
   *
   * The detached anchors of the other cached files are the only ones a paste
   * here can re-pin. Files that were never loaded are not consulted, so a
   * detached anchor there stays where it is. The application loads every file
   * of a project when it opens, so this does not come up in practice.
   *
   * @param {{ fileId: string, edits: Edit[], fragments?: FragmentRange[] }[]} files
   * @param {MoveMemory} memory
   * @param {{ context?: number }} [opts]
   * @returns {{ files: { fileId: string, after: string, anchors: PlannedAnchor[] }[] }}
   */
  record(files, memory, opts) {
    const input = files.map((file) => {
      const entry = this.#loaded(file.fileId);
      return {
        fileId: file.fileId,
        before: entry.text,
        anchors: entry.anchors,
        edits: file.edits,
        fragments: file.fragments,
      };
    });
    const named = new Set(input.map((file) => file.fileId));
    const others = [];
    for (const [fileId, entry] of this.#files) {
      if (named.has(fileId)) continue;
      const anchors = entry.anchors.filter((a) => a.detached);
      if (anchors.length > 0) others.push({ fileId, before: entry.text, anchors, edits: [] });
    }
    const plan = recordOperation(this.#doc, input, memory, { ...opts, others });
    for (const file of plan.files) {
      const anchors = file.anchors.map(toAnchor);
      if (named.has(file.fileId)) {
        this.#files.set(file.fileId, { text: file.after, anchors });
        continue;
      }
      // Recorded only for its detached anchors, which a paste here may re-pin.
      // Its attached anchors were not passed in, so they stay.
      const entry = this.#files.get(file.fileId);
      if (entry) {
        const kept = entry.anchors.filter((a) => !a.detached);
        this.#files.set(file.fileId, { text: file.after, anchors: [...kept, ...anchors] });
      }
    }
    return plan;
  }

  /**
   * Applies the edits to each file's text, commits, records them with
   * record(), and commits the anchor work under ANCHORS_ORIGIN. Like
   * applyOperation, but with no cursor resolution. Returns the plan.
   *
   * @param {{ fileId: string, edits: Edit[], fragments?: FragmentRange[] }[]} files
   * @param {MoveMemory} memory
   * @param {{ context?: number }} [opts]
   * @returns {{ files: { fileId: string, after: string, anchors: PlannedAnchor[] }[] }}
   */
  apply(files, memory, opts) {
    const doc = this.#doc;
    const edited = files.filter((file) => fileText(doc, file.fileId) !== null);
    // Loaded before the first edit, so the cache holds the before state.
    for (const file of edited) this.get(file.fileId);
    for (const file of edited) {
      const text = fileText(doc, file.fileId);
      let adj = 0;
      for (const edit of file.edits) {
        const ef = edit.from + adj;
        const et = edit.to + adj;
        if (et > ef) text.delete(ef, et - ef);
        if (edit.insert) text.insert(ef, edit.insert);
        adj += edit.insert.length - (edit.to - edit.from);
      }
    }
    doc.commit();
    const plan = this.record(edited, memory, opts);
    doc.commit({ origin: ANCHORS_ORIGIN });
    return plan;
  }

  /**
   * Keeps the cache current across the document's changes. An undo or redo is
   * recorded here from the text events it produced. The edits of each file it
   * touched go through record() under ANCHORS_ORIGIN, so the anchors follow the
   * text back even in a file that is not on screen. Every other local batch is
   * ignored: the editor plugin records the user's own edits through record().
   *
   * For each import batch: a cached file whose entry in the files root changed
   * is loaded again first, since a restored file keeps its id with a new text
   * container; then the cached files' text deltas shift their anchors in
   * foreign mode, and the records that the batch changed are resolved again by
   * refresh(). Imports are the only remote change, so no other batch is read.
   *
   * @param {MoveMemory} memory the session's memory, the one its edits are recorded with
   * @returns {() => void} the unsubscribe function
   */
  attach(memory) {
    return this.#doc.subscribe((batch) => {
      if (batch.by === "local" && batch.origin === "undo") {
        // Loro gives undo and redo commits the same origin. Successive events on
        // one text are composed into one list for that file.
        // A redo that deletes text as a tracked change brings its fragment
        // records back in this batch. The comment inside takes the fragment it
        // sits in, as it would from the editor plugin.
        /** @type {Map<string, Edit[][]>} */
        const lists = new Map();
        /** @type {Set<string>} */
        const appeared = new Set();
        const changesRoot = this.#doc.getMap(CHANGES).id;
        for (const event of batch.events) {
          if (event.target === changesRoot && event.diff.type === "map") {
            for (const [id, value] of Object.entries(event.diff.updated)) {
              if (value instanceof LoroMap && value.get("kind") === "fragment") appeared.add(id);
            }
            continue;
          }
          const fileId = this.#texts.get(event.target);
          if (fileId === undefined || event.diff.type !== "text") continue;
          if (!lists.has(fileId)) lists.set(fileId, []);
          lists.get(fileId).push(editsFromDelta(event.diff.diff));
        }
        if (lists.size > 0) {
          // All reads come before the first write. Resolving a stale cursor can
          // commit pending work.
          const files = [...lists].map(([fileId, edits]) => {
            const composed = composeEdits(edits);
            const deletes = composed.some((edit) => edit.to > edit.from);
            if (appeared.size === 0 || !deletes) return { fileId, edits: composed, fragments: [] };
            const created = resolveFile(this.#doc, fileId).gaps.flatMap((gap) =>
              gap.fragments.filter((f) => appeared.has(f.id)).map((f) => ({ id: f.id, text: f.text, at: gap.at })),
            );
            const fragments = fragmentRangesFor(this.get(fileId).text, composed, created);
            return { fileId, edits: composed, fragments };
          });
          this.record(files, memory);
          this.#doc.commit({ origin: ANCHORS_ORIGIN });
        }
        return;
      }
      if (batch.by !== "import") return;
      // A restored file keeps its id but gets a new text container, so the
      // cached files that changed are loaded again first. The text of a
      // container loaded here already holds this batch's edits, so its text
      // events are not applied a second time.
      const loaded = this.#followFiles(batch.events);
      for (const event of batch.events) {
        const fileId = this.#texts.get(event.target);
        if (fileId !== undefined && event.diff.type === "text" && !loaded.has(event.target)) {
          this.#shift(fileId, editsFromDelta(event.diff.diff));
        }
      }
      const root = this.#doc.getMap(ANCHORS).id;
      // The files root is a map too, but no record lives there.
      const filesRoot = this.#doc.getMap("files").id;
      const maps = batch.events.filter((event) => event.diff.type === "map" && event.target !== filesRoot);
      // Records that appeared or were replaced are learned first, so that the
      // events on their containers can be placed.
      const changed = new Set();
      for (const event of maps) {
        if (event.target !== root || event.diff.type !== "map") continue;
        for (const [id, value] of Object.entries(event.diff.updated)) {
          if (value instanceof LoroMap) this.#remember(id, value);
          changed.add(id);
        }
      }
      // A record of a file that is not cached can move into one that is, so an
      // unknown container triggers one pass that learns every record's container.
      if (maps.some((event) => event.target !== root && !this.#recordIds.has(event.target))) {
        this.#learn();
      }
      for (const event of maps) {
        if (event.target === root) continue;
        const id = this.#recordIds.get(event.target);
        if (id !== undefined) changed.add(id);
      }
      if (changed.size > 0) this.refresh([...changed]);
    });
  }

  /**
   * Resolves the given records from the document and replaces them in the
   * cache. A record that is gone is removed from every file. A record whose file
   * changed leaves its old file and joins the new one, loading that file when it
   * is not cached. Does not commit.
   *
   * @param {string[]} ids
   * @returns {string[]} the ids refreshed
   */
  refresh(ids) {
    const doc = this.#doc;
    const records = doc.getMap(ANCHORS);
    /** @type {Map<string, string>} */
    const bodies = new Map();
    for (const id of ids) {
      const value = /** @type {any} */ (records.get(id));
      const fileId = value instanceof LoroMap ? placeOf(value)?.file : undefined;
      if (!(value instanceof LoroMap) || typeof fileId !== "string") {
        this.#forget(id);
        continue;
      }
      this.#remember(id, value);
      this.#drop(id, fileId);
      if (!this.#files.has(fileId)) {
        this.load(fileId);
        continue;
      }
      if (!bodies.has(fileId)) {
        const text = fileText(doc, fileId);
        bodies.set(fileId, text ? text.toString() : "");
      }
      const anchor = toAnchor(recordAnchor(doc, id, value, fileId, /** @type {string} */ (bodies.get(fileId))));
      const entry = /** @type {{ text: string, anchors: Anchor[] }} */ (this.#files.get(fileId));
      this.#files.set(fileId, { ...entry, anchors: [...entry.anchors.filter((a) => a.id !== id), anchor] });
    }
    return [...ids];
  }

  /** @returns {{ text: string, anchors: Anchor[] }} */
  #loaded(fileId) {
    const entry = this.#files.get(fileId);
    if (!entry) throw new Error(`anchors: ${fileId} must be loaded before its edit is recorded`);
    return entry;
  }

  #locate(id) {
    for (const [fileId, entry] of this.#files) {
      const anchor = entry.anchors.find((a) => a.id === id);
      if (anchor) return { fileId, anchor };
    }
    return null;
  }

  #learn() {
    for (const [id, value] of this.#doc.getMap(ANCHORS).entries()) {
      if (value instanceof LoroMap) this.#remember(id, /** @type {any} */ (value));
    }
  }

  #remember(id, value) {
    const old = this.#containerIds.get(id);
    if (old !== undefined) this.#recordIds.delete(old);
    this.#containerIds.set(id, value.id);
    this.#recordIds.set(value.id, id);
  }

  #forget(id) {
    const container = this.#containerIds.get(id);
    if (container !== undefined) this.#recordIds.delete(container);
    this.#containerIds.delete(id);
    this.#drop(id, null);
  }

  // Removes a record from every cached file except keep.
  #drop(id, keep) {
    for (const [fileId, entry] of this.#files) {
      if (fileId === keep || !entry.anchors.some((a) => a.id === id)) continue;
      this.#files.set(fileId, { ...entry, anchors: entry.anchors.filter((a) => a.id !== id) });
    }
  }

  // Forgets the text container that a file was mapped to, if any.
  #unmap(fileId) {
    for (const [textId, owner] of this.#texts) {
      if (owner === fileId) this.#texts.delete(textId);
    }
  }

  /**
   * Follows the files whose value an import replaced or removed. A restored
   * file keeps its file id but has a new text container, and the old one is no
   * longer its text. A cached file that holds a text is loaded from that text.
   * A cached file that holds no text is dropped, so that find() reads the
   * record from the document and deriveAttachment reports the file as gone.
   * Does not commit.
   *
   * @param {{ target: string, diff: any }[]} events the events of one import batch
   * @returns {Set<string>} the ids of the text containers loaded here
   */
  #followFiles(events) {
    const filesRoot = this.#doc.getMap("files").id;
    /** @type {Set<string>} */
    const loaded = new Set();
    for (const event of events) {
      if (event.target !== filesRoot || event.diff.type !== "map") continue;
      for (const fileId of Object.keys(event.diff.updated)) {
        if (!this.#files.has(fileId)) continue;
        const text = fileText(this.#doc, fileId);
        if (text) {
          this.load(fileId);
          loaded.add(text.id);
        } else {
          this.#unmap(fileId);
          this.#files.delete(fileId);
        }
      }
    }
    return loaded;
  }

  #shift(fileId, edits) {
    const entry = this.#files.get(fileId);
    if (!entry || edits.length === 0) return;
    const [file] = planOperation(
      [{ fileId, before: entry.text, anchors: entry.anchors, edits }],
      this.#scratch,
      { foreign: true },
    ).files;
    this.#files.set(fileId, { text: file.after, anchors: file.anchors.map(toAnchor) });
  }
}
