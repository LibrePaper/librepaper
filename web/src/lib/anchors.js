// Comment anchors live in the shared document, in the `anchors` root map: one
// map container per comment id. A record holds the file it sits in, two Loro
// cursors (start and end) while it is attached, and the quote the comment was
// made on. A record whose text was deleted whole is detached: it keeps its
// quote and, when a tracked deletion took the text, the id of the fragment
// that holds it.
//
// The planner (planOperation) is pure. It moves the attached anchors through
// one batch of edits made in this session, and re-pins detached anchors when an
// insertion brings their text back exactly once. The move memory is per
// session: text deleted whole is remembered, so that cut and paste, drag and
// drop, undo and agent patches keep comments exact. Nothing searches the
// document for words.
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
import { CHANGES } from "./track-changes.js";

export const ANCHORS = "anchors";
export const ANCHORS_ORIGIN = "anchors";
export const CONTEXT_UNITS = 32;

/**
 * @typedef {{ from: number, to: number, insert: string }} Edit
 *   Before-coordinates (UTF-16), ascending, non-overlapping.
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
 * @typedef {{ id: string, offset: number, text: string }} Held
 *   offset is where the anchor starts within the deleted text; text is what
 *   the anchor covered.
 */

// What the session deleted whole, oldest first. Each entry keeps the anchors
// that were wholly inside the deleted text, with their offsets in it.
export class MoveMemory {
  #limit;
  #entries = [];

  /** @param {number} [limit] how many deletions are remembered */
  constructor(limit = 32) {
    this.#limit = limit;
  }

  /**
   * @param {string} text deleted text
   * @param {Held[]} held anchors that were wholly inside it, with offsets relative to text
   */
  remember(text, held) {
    if (!text || !held || held.length === 0) return;
    this.#entries.push({ text, held: held.map((h) => ({ ...h })) });
    while (this.#entries.length > this.#limit) this.#entries.shift();
  }

  /**
   * Takes the newest remembered text that occurs exactly once in `inserted`.
   * @param {string} inserted
   * @returns {{ text: string, held: Held[], at: number } | null}
   */
  take(inserted) {
    for (let i = this.#entries.length - 1; i >= 0; i--) {
      const entry = this.#entries[i];
      const at = inserted.indexOf(entry.text);
      // Overlapping occurrences count as two: "aa" in "aaa" is ambiguous.
      if (at < 0 || inserted.indexOf(entry.text, at + 1) >= 0) continue;
      this.#entries.splice(i, 1);
      return { text: entry.text, held: entry.held, at };
    }
    return null;
  }

  get size() {
    return this.#entries.length;
  }
}

/**
 * Pure, operation-level planner. No Loro here.
 *
 * Phase 1 runs each file on its own, edits in order, and moves its attached
 * anchors. Phase 2 runs after every file: an insertion that takes a remembered
 * text re-pins the detached anchors that were in it, into the file the
 * insertion landed in. Deletions are remembered and taken texts are removed
 * from the memory as a side effect.
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
            }
            a.from = afterDelete(from, ef, et);
            a.to = afterDelete(to, ef, et);
            if (a.from !== from || a.to !== to) moved.add(a);
          }
        }
        if (held.length > 0) memory.remember(deleted, held);
      }
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

  // Phase 2: an insertion that brings remembered text back re-pins the anchors
  // that were in it.
  for (const w of foreign ? [] : work) {
    for (const ins of w.inserts) {
      const hit = memory.take(ins.text);
      if (!hit) continue;
      for (const h of hit.held) {
        if (!h.text) continue;
        const owner = work.find((o) => o.anchors.some((a) => a.id === h.id));
        if (!owner) continue;
        const index = owner.anchors.findIndex((a) => a.id === h.id);
        const anchor = owner.anchors[index];
        // A record with no length has no span to bring back.
        if (attached(anchor) || typeof anchor.length !== "number") continue;
        const from = ins.at + hit.at + h.offset;
        const to = from + h.text.length;
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

const str = (value) => (typeof value === "string" ? value : "");

// The fields every record carries, read without resolving its cursors.
function recordFields(value, id, fileId) {
  const fragment = value.get("fragment");
  const offset = value.get("offset");
  const length = value.get("length");
  return {
    id,
    file: fileId,
    fragment: typeof fragment === "string" ? fragment : null,
    offset: typeof offset === "number" ? offset : null,
    length: typeof length === "number" ? length : null,
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
  if (value.get("detached") === true) return detachedAnchor(value, id, fileId);
  const base = recordFields(value, id, fileId);
  const start = value.get("start");
  const end = value.get("end");
  const s = start instanceof Uint8Array ? cursorPos(doc, start) : null;
  const e = end instanceof Uint8Array ? cursorPos(doc, end) : null;
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
    if (!(entry instanceof LoroMap) || entry.get("file") !== fileId) continue;
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
  record.set("file", fileId);
  record.set("start", start.encode());
  record.set("end", end.encode());
  record.set("exact", exact);
  record.set("prefix", prefix);
  record.set("suffix", suffix);
  return true;
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
  /** @type {{ id: string, record: any, sets: [string, any][], deletes: string[] }[]} */
  const pending = [];
  for (const file of planned.files) {
    const text = fileText(doc, file.fileId);
    for (const a of file.anchors) {
      if (a.state !== "set") continue;
      if (!a.detached && a.touched === false) continue;
      const record = map.get(a.id);
      if (!(record instanceof LoroMap)) continue;
      /** @type {[string, any][]} */
      const sets = [];
      /** @type {string[]} */
      const deletes = [];
      const put = (key, value) => {
        if (record.get(key) !== value) sets.push([key, value]);
      };
      const drop = (key) => {
        if (record.get(key) !== undefined) deletes.push(key);
      };
      if (a.detached) {
        put("file", file.fileId);
        put("detached", true);
        if (typeof a.length === "number") put("length", a.length);
        else drop("length");
        if (typeof a.fragment === "string") put("fragment", a.fragment);
        else drop("fragment");
        if (typeof a.offset === "number") put("offset", a.offset);
        else drop("offset");
        pending.push({ id: a.id, record, sets, deletes });
        continue;
      }
      if (!text || !(a.to > a.from)) continue;
      const body = text.toString();
      const start = text.getCursor(a.from, -1);
      const end = text.getCursor(lastCharStart(body, a.to), 1);
      if (!start || !end) continue;
      const reattach = record.get("detached") === true || record.get("file") !== file.fileId;
      const have = a.touched === true || reattach ? null : resolvedSpan(doc, text, record);
      if (!have || have.from !== a.from) sets.push(["start", start.encode()]);
      if (!have || have.to !== a.to) sets.push(["end", end.encode()]);
      put("file", file.fileId);
      drop("detached");
      drop("length");
      drop("fragment");
      drop("offset");
      pending.push({ id: a.id, record, sets, deletes });
    }
  }
  // Pass 2 writes. The commit origin is set before the first write, so the
  // commit that carries these writes is labelled as anchor work.
  const dirty = pending.filter((p) => p.sets.length > 0 || p.deletes.length > 0);
  if (dirty.length > 0) doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  for (const p of dirty) {
    for (const [key, value] of p.sets) p.record.set(key, value);
    for (const key of p.deletes) p.record.delete(key);
  }
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
    if (a.collapsed && record instanceof LoroMap) marked.push({ id: a.id, record });
  }
  if (marked.length > 0) doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  for (const { record } of marked) record.set("detached", true);
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
      const fileId = value.get("file");
      if (typeof fileId !== "string" || named.has(fileId) || value.get("detached") !== true) continue;
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
 * Where a point goes when a batch of edits is applied to the text it is in.
 * The edits are in that text's coordinates, ascending. A point inside deleted
 * text goes to the start of the deletion, where a tracked deletion's gap sits;
 * a point where text is inserted stays before the insertion.
 *
 * @param {number} point
 * @param {Edit[]} edits
 * @returns {number}
 */
export function throughEdits(point, edits) {
  let p = point;
  let adj = 0;
  for (const edit of edits) {
    const ef = edit.from + adj;
    const et = edit.to + adj;
    const n = edit.insert.length;
    adj += n - (edit.to - edit.from);
    if (ef > p) break;
    if (et < p || (et === p && et > ef)) p += n - (et - ef);
    else if (ef < p) p = ef;
  }
  return p;
}

/**
 * The fragment ranges of the deletions in one transaction. A deletion gets
 * ranges when its text is exactly the texts of the gap at its point, in rank
 * order; each fragment becomes one range. Anything else, such as an untracked
 * deletion or fragments that were joined, gets none.
 *
 * The edits are in the coordinates of `before`. A deletion's point is its
 * `from` after the edits before it, so the gaps are read in the coordinates of
 * the text after these edits, unless `toGap` maps them. A caller whose update
 * has later transactions maps the point through them with `throughEdits`.
 *
 * @param {string} before the text the edits apply to
 * @param {Edit[]} edits
 * @param {{ at: number, fragments: { id: string, text: string, rank: number }[] }[]} gaps
 * @param {(point: number) => number} [toGap]
 * @returns {FragmentRange[]}
 */
export function fragmentRangesFor(before, edits, gaps, toGap = (point) => point) {
  const out = [];
  let adj = 0;
  edits.forEach((edit, index) => {
    const point = edit.from + adj;
    adj += edit.insert.length - (edit.to - edit.from);
    if (!(edit.to > edit.from)) return;
    const gap = gaps.find((g) => g.at === toGap(point));
    if (!gap) return;
    const fragments = [...gap.fragments].sort((a, b) => a.rank - b.rank);
    if (fragments.map((f) => f.text).join("") !== before.slice(edit.from, edit.to)) return;
    let start = 0;
    for (const fragment of fragments) {
      out.push({ id: fragment.id, edit: index, start, end: start + fragment.text.length });
      start += fragment.text.length;
    }
  });
  return out;
}

/**
 * The source attachment of a comment, read from the document's anchor records:
 * its status, the range it resolves to and the file that holds it. This is the
 * shape the server used to send. A comment with no source passage has none.
 *
 * `bodies` caches each file's text across one pass over many comments. Pass
 * the same Map to every call of that pass.
 *
 * @param {import("loro-crdt").LoroDoc} doc
 * @param {AnchorIndex} index
 * @param {{ id: string, original_anchor?: { kind?: string } }} comment
 * @param {Map<string, string>} [bodies]
 * @returns {{ status: string, resolved_range_utf16: [number, number] | null, resolved_file_id: string | null, tree_digest: string } | null}
 */
export function deriveAttachment(doc, index, comment, bodies = new Map()) {
  if (comment?.original_anchor?.kind !== "source_text") return null;
  const hit = index.find(String(comment.id));
  if (!hit) return { status: "unresolved", resolved_range_utf16: null, resolved_file_id: null, tree_digest: "" };
  const { anchor } = hit;
  let body = bodies.get(anchor.file);
  if (body === undefined) {
    const text = fileText(doc, anchor.file);
    body = text ? text.toString() : "";
    bodies.set(anchor.file, body);
  }
  const status = statusOf(doc, anchor, body);
  const placed = status === "exact" || status === "modified";
  return {
    status,
    resolved_range_utf16: placed ? [anchor.from, anchor.to] : null,
    resolved_file_id: anchor.file,
    tree_digest: "",
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
    if (!(entry instanceof LoroMap) || entry.get("file") !== fileId || entry.get("detached") !== true) continue;
    const value = /** @type {any} */ (entry);
    const fragment = value.get("fragment");
    const offset = value.get("offset");
    const r = typeof fragment === "string" ? byFragment.get(fragment) : undefined;
    const length = value.get("length");
    if (!r || typeof offset !== "number" || typeof length !== "number") continue;
    const from = r.at + offset;
    const to = Math.min(from + length, r.at + r.text.length);
    if (!(to > from)) continue;
    const start = text.getCursor(from, -1);
    const end = text.getCursor(lastCharStart(body, to), 1);
    if (!start || !end) continue;
    repins.push({ id, value, start: start.encode(), end: end.encode() });
  }
  if (repins.length > 0) doc.setNextCommitOrigin(ANCHORS_ORIGIN);
  for (const { value, start, end } of repins) {
    value.set("start", start);
    value.set("end", end);
    value.delete("detached");
    value.delete("length");
    value.delete("fragment");
    value.delete("offset");
  }
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
   * text. Each record's container is remembered for its later changes.
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
      if (value.get("file") !== fileId) continue;
      this.#remember(id, value);
      anchors.push(toAnchor(recordAnchor(doc, id, value, fileId, body)));
    }
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
    const fileId = record instanceof LoroMap ? record.get("file") : undefined;
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
   * Keeps the cache current across imports. For each import batch: the cached
   * files' text deltas shift their anchors in foreign mode, then the records
   * that the batch changed are resolved again by refresh(). Local batches are
   * ignored: they go through record().
   *
   * @returns {() => void} the unsubscribe function
   */
  attach() {
    return this.#doc.subscribe((batch) => {
      if (batch.by !== "import") return;
      for (const event of batch.events) {
        const fileId = this.#texts.get(event.target);
        if (fileId !== undefined && event.diff.type === "text") {
          this.#shift(fileId, editsFromDelta(event.diff.diff));
        }
      }
      const root = this.#doc.getMap(ANCHORS).id;
      const maps = batch.events.filter((event) => event.diff.type === "map");
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
      const fileId = value instanceof LoroMap ? value.get("file") : undefined;
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
