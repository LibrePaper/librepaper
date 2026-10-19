// Tests for the anchor layer on real loro-crdt documents.
//
// The directed tests pin what the contract names: an anchor follows the
// insertions around it and turns modified under one inside; a cut detaches it
// and a paste re-pins it, in the same file, in another file, and in both drag
// directions; two peers that each delete one half of a passage collapse it, and
// marking it leaves one detached record on both; a tracked deletion rejected or
// accepted in another session restores or drops it; and undo through the real
// UndoManager walks a cut and a paste back, and an undo of a cut head or tail
// puts the passage back exact. The AnchorIndex tests follow remote edits into a
// second peer's cache, which must agree with resolveAnchors.
//
// The seeded fuzzers check random histories against a tagged oracle. Each
// character carries the ids of the anchors it belongs to, so the expected place
// of every anchor is known after every step: where its tagged characters are
// contiguous the anchor is exact, where an untagged character splits them it is
// modified, and where none are left it is deleted.
import assert from "node:assert/strict";
import { test } from "node:test";
import { LoroDoc, LoroText, UndoManager } from "loro-crdt";
import {
  ANCHORS,
  ANCHORS_ORIGIN,
  AnchorIndex,
  CONTEXT_UNITS,
  MoveMemory,
  applyOperation,
  createAnchor,
  editsFromDelta,
  fragmentRangesFor,
  markCollapsed,
  recordOperation,
  resolveAnchors,
  restoreByFragments,
  statusOf,
} from "../../src/lib/anchors.js";
import { CHANGES, accept, reject, resolveFile, trackedSplice } from "../../src/lib/track-changes.js";

const BOB = { who: "bob", author: "Bob" };
const LETTERS = "abcdefghijklmnopqrstuvwxyz";
const SEP = " ZZZZ ";

// A seeded generator: the same seed replays the same case.
function random(seed) {
  let state = seed >>> 0;
  const rnd = () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 0x100000000;
  };
  const pick = (n) => Math.floor(rnd() * n);
  return { rnd, pick };
}

// ---------------------------------------------------------------------------
// Helpers

function makeDoc(peer, files) {
  const doc = new LoroDoc();
  doc.setPeerId(peer);
  const map = doc.getMap("files");
  for (const [fileId, text] of Object.entries(files)) {
    const body = map.setContainer(fileId, new LoroText());
    if (text) body.insert(0, text);
  }
  doc.commit();
  return doc;
}

// Two copies of one document exchange their updates both ways.
function sync(a, b) {
  a.import(b.export({ mode: "update", from: a.oplogVersion() }));
  b.import(a.export({ mode: "update", from: b.oplogVersion() }));
}

function textOf(doc, fileId = "f1") {
  return doc.getMap("files").get(fileId).toString();
}

function anchorsById(doc, fileId = "f1") {
  return new Map(resolveAnchors(doc, fileId).map((a) => [a.id, a]));
}

// The quote and the text around it, as an anchor record keeps them.
function ctx(text, from, to) {
  return {
    exact: text.slice(from, to),
    prefix: text.slice(Math.max(0, from - CONTEXT_UNITS), from),
    suffix: text.slice(to, to + CONTEXT_UNITS),
  };
}

function assertAttached(doc, id, from, to, exact, fileId = "f1") {
  const anchor = anchorsById(doc, fileId).get(id);
  assert.ok(anchor, `${id} is listed`);
  assert.equal(anchor.detached, false, `${id} is attached`);
  assert.deepEqual([anchor.from, anchor.to], [from, to], `${id} range`);
  assert.equal(anchor.exact, exact, `${id} exact`);
  assert.equal(statusOf(doc, anchor), "exact", `${id} status`);
}

// The fields that place an anchor. An index must agree with resolveAnchors on them.
function placed(a) {
  return {
    id: a.id,
    file: a.file,
    from: a.from,
    to: a.to,
    detached: a.detached,
    length: a.length,
    fragment: a.fragment,
    offset: a.offset,
    collapsed: a.collapsed,
  };
}

function assertIndexMatches(index, doc, fileId = "f1") {
  const byId = (x, y) => (x.id < y.id ? -1 : x.id > y.id ? 1 : 0);
  const cached = index.anchors(fileId).map(placed).sort(byId);
  const truth = resolveAnchors(doc, fileId).map(placed).sort(byId);
  assert.deepEqual(cached, truth, `index of ${fileId} equals resolveAnchors`);
}

// Edits the text of doc and records the edit through index as two commits: the
// text, then the anchors. Returns the update that carries the text alone, and
// the one that carries the rest.
function editInTwoCommits(doc, index, memory, fileId, edits) {
  const before = doc.oplogVersion();
  const text = doc.getMap("files").get(fileId);
  let adj = 0;
  for (const edit of edits) {
    const ef = edit.from + adj;
    const et = edit.to + adj;
    if (et > ef) text.delete(ef, et - ef);
    if (edit.insert) text.insert(ef, edit.insert);
    adj += edit.insert.length - (edit.to - edit.from);
  }
  doc.commit();
  const afterText = doc.oplogVersion();
  const textOnly = doc.export({ mode: "update", from: before });
  index.record([{ fileId, edits }], memory, {});
  doc.commit({ origin: ANCHORS_ORIGIN });
  const rest = doc.export({ mode: "update", from: afterText });
  return { textOnly, rest };
}

// The insertion points at which `piece` turns `before` into `after`.
function insertionPoints(before, after, piece) {
  const points = [];
  for (let p = 0; p <= before.length; p++) {
    if (after === before.slice(0, p) + piece + before.slice(p)) points.push(p);
  }
  return points;
}

// Runs a random case and adds the seed and the operations to any failure.
function withLog(label, log, fn) {
  try {
    fn();
  } catch (error) {
    throw new Error(`${label}\nlog: ${JSON.stringify(log)}\n${error.message}`);
  }
}

// ---------------------------------------------------------------------------
// Tagged oracle

const unit = (ch, tags = []) => ({ ch, tags: new Set(tags) });
const textOfUnits = (units) => units.map((u) => u.ch).join("");
const isLetter = (ch) => ch >= "a" && ch <= "z";
const isSpace = (ch) => ch === " " || ch === "\n";

function randomWord(rng) {
  let word = "";
  const n = 2 + rng.pick(4);
  for (let i = 0; i < n; i++) word += LETTERS[rng.pick(LETTERS.length)];
  return word;
}

// Words of letters separated by spaces or newlines. Each word is an anchor with
// probability 0.6; an anchor's id tags each of its characters.
function layoutWords(rng, prefix, count) {
  let text = "";
  const units = [];
  const anchors = [];
  for (let w = 0; w < count; w++) {
    if (w > 0) {
      const sep = rng.rnd() < 0.2 ? "\n" : " ";
      text += sep;
      units.push(unit(sep));
    }
    const word = randomWord(rng);
    const id = rng.rnd() < 0.6 ? `${prefix}-${anchors.length}` : null;
    const from = text.length;
    text += word;
    for (const ch of word) units.push(unit(ch, id ? [id] : []));
    if (id) anchors.push({ id, from, to: from + word.length, word });
  }
  return { text, units, anchors };
}

// Letter runs of two or more characters: the pieces the fuzzers cut and drag.
function wordRuns(units) {
  const runs = [];
  let i = 0;
  while (i < units.length) {
    if (!isLetter(units[i].ch)) {
      i++;
      continue;
    }
    let j = i;
    while (j < units.length && isLetter(units[j].ch)) j++;
    if (j - i >= 2) runs.push({ from: i, to: j });
    i = j;
  }
  return runs;
}

// Positions next to a space or at an edge, where a piece never lands between
// two letters, so an insertion there cannot split a word.
function edgePositions(units) {
  const out = [];
  for (let p = 0; p <= units.length; p++) {
    const before = p > 0 ? units[p - 1].ch : " ";
    const after = p < units.length ? units[p].ch : " ";
    if (isSpace(before) || isSpace(after)) out.push(p);
  }
  return out;
}

// The span of one anchor's tagged characters in a file, and whether no untagged
// character lies inside it.
function spanOf(units, id) {
  const at = [];
  units.forEach((u, i) => {
    if (u.tags.has(id)) at.push(i);
  });
  if (at.length === 0) return null;
  const from = at[0];
  const to = at[at.length - 1] + 1;
  return { from, to, contiguous: to - from === at.length };
}

// Every anchor must sit where the oracle puts it. `files` maps file ids to their
// oracle characters, and `wordOf` maps anchor ids to the word each one holds.
function checkAnchors(doc, files, wordOf) {
  const listed = new Map([...files.keys()].map((fileId) => [fileId, anchorsById(doc, fileId)]));
  for (const [fileId, units] of files) {
    assert.equal(textOf(doc, fileId), textOfUnits(units), `text of ${fileId}`);
  }
  for (const [id, word] of wordOf) {
    const spans = [];
    for (const [fileId, units] of files) {
      const span = spanOf(units, id);
      if (span) spans.push({ fileId, ...span });
    }
    assert.ok(spans.length <= 1, `${id} has tagged characters in ${spans.length} places`);
    const hits = [...listed]
      .filter(([, byId]) => byId.has(id))
      .map(([fileId, byId]) => ({ fileId, anchor: byId.get(id) }));
    assert.equal(hits.length, 1, `${id} is listed under ${hits.length} files`);
    const { fileId, anchor } = hits[0];
    if (spans.length === 0) {
      assert.equal(anchor.detached, true, `${id} should be detached`);
      assert.deepEqual([anchor.from, anchor.to], [0, 0], `${id} detached offsets`);
      assert.equal(statusOf(doc, anchor), "deleted", `${id} status`);
    } else {
      const span = spans[0];
      assert.equal(fileId, span.fileId, `${id} should be listed under ${span.fileId}`);
      assert.equal(anchor.detached, false, `${id} should be attached`);
      assert.deepEqual([anchor.from, anchor.to], [span.from, span.to], `${id} range`);
      assert.equal(anchor.exact, word, `${id} exact`);
      assert.equal(statusOf(doc, anchor), span.contiguous ? "exact" : "modified", `${id} status`);
    }
  }
}

// One random edit to a target: a file, or the region of a file that one peer
// owns. `units` is the oracle for the target and `base` its offset in the file.
// Returns the clipboard: the cut waiting for its paste, or null.
function stepOn(
  rng,
  doc,
  memory,
  target,
  clip,
  log,
  { who = "", typeAnywhere = true, apply = (files) => applyOperation(doc, files, memory, {}) } = {},
) {
  const { fileId, units, base } = target;
  const note = (text) => log.push(who ? `${who} ${text}` : text);
  const at = (p) => base + p;
  const runs = wordRuns(units);
  const kinds = ["type", "type", "type", "cut", "cut", "paste", "paste", "copy", "drag"];
  const kind = kinds[rng.pick(kinds.length)];

  if (kind === "cut" && !clip && runs.length > 0) {
    const run = runs[rng.pick(runs.length)];
    const removed = units.slice(run.from, run.to);
    apply([{ fileId, edits: [{ from: at(run.from), to: at(run.to), insert: "" }] }]);
    units.splice(run.from, run.to - run.from);
    note(`cut ${fileId}[${run.from},${run.to})`);
    return { text: textOfUnits(removed), units: removed };
  }

  if (kind === "paste" && clip) {
    const spots = edgePositions(units);
    const p = spots[rng.pick(spots.length)];
    apply([{ fileId, edits: [{ from: at(p), to: at(p), insert: clip.text }] }]);
    units.splice(p, 0, ...clip.units);
    note(`paste ${fileId}@${p} '${clip.text}'`);
    return null;
  }

  if (kind === "copy" && runs.length > 0) {
    const run = runs[rng.pick(runs.length)];
    const copied = textOfUnits(units.slice(run.from, run.to));
    // A copy that contains the pending cut would be read as a move of it.
    if (!clip || !copied.includes(clip.text)) {
      const spots = edgePositions(units);
      const p = spots[rng.pick(spots.length)];
      apply([{ fileId, edits: [{ from: at(p), to: at(p), insert: copied }] }]);
      units.splice(p, 0, ...[...copied].map((ch) => unit(ch)));
      note(`copy ${fileId}@${p} '${copied}'`);
      return clip;
    }
  }

  if (kind === "drag" && !clip && runs.length > 0) {
    const run = runs[rng.pick(runs.length)];
    const spots = edgePositions(units).filter((p) => p < run.from || p > run.to);
    if (spots.length > 0) {
      const p = spots[rng.pick(spots.length)];
      const moved = units.slice(run.from, run.to);
      const text = textOfUnits(moved);
      const lift = { from: at(run.from), to: at(run.to), insert: "" };
      const drop = { from: at(p), to: at(p), insert: text };
      apply([{ fileId, edits: p < run.from ? [drop, lift] : [lift, drop] }]);
      units.splice(run.from, run.to - run.from);
      units.splice(p < run.from ? p : p - (run.to - run.from), 0, ...moved);
      note(`drag ${fileId}[${run.from},${run.to}) to ${p}`);
      return clip;
    }
  }

  // Typing: a letter anywhere, or only at edges when the caller keeps words whole.
  const spots = typeAnywhere ? Array.from({ length: units.length + 1 }, (_, i) => i) : edgePositions(units);
  const p = spots[rng.pick(spots.length)];
  const ch = LETTERS[rng.pick(LETTERS.length)];
  apply([{ fileId, edits: [{ from: at(p), to: at(p), insert: ch }] }]);
  units.splice(p, 0, unit(ch));
  note(`type ${fileId}@${p} ${ch}`);
  return clip;
}

// ---------------------------------------------------------------------------
// Directed tests

test("createAnchor round-trips the range and the quote", () => {
  const text = "one two three";
  const doc = makeDoc(1, { f1: text });
  assert.equal(createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7)), true);
  doc.commit();
  const anchors = resolveAnchors(doc, "f1");
  assert.equal(anchors.length, 1);
  assert.equal(anchors[0].id, "a1");
  assert.deepEqual([anchors[0].from, anchors[0].to, anchors[0].exact], [4, 7, "two"]);
  assert.equal(anchors[0].detached, false);
  assert.equal(statusOf(doc, anchors[0]), "exact");
});

test("an insertion before an anchor moves it, and one inside makes it modified", () => {
  const text = "one two three";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7));
  doc.commit();
  const memory = new MoveMemory();
  applyOperation(doc, [{ fileId: "f1", edits: [{ from: 0, to: 0, insert: "zz " }] }], memory, {});
  assertAttached(doc, "a1", 7, 10, "two");
  applyOperation(doc, [{ fileId: "f1", edits: [{ from: 8, to: 8, insert: "X" }] }], memory, {});
  assert.equal(textOf(doc), "zz one tXwo three");
  const anchor = anchorsById(doc, "f1").get("a1");
  assert.deepEqual([anchor.from, anchor.to, anchor.detached], [7, 11, false]);
  assert.equal(anchor.exact, "two");
  assert.equal(statusOf(doc, anchor), "modified");
});

test("a cut detaches the anchor and a paste re-pins it", () => {
  const text = "one two three";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7));
  doc.commit();
  const memory = new MoveMemory();
  applyOperation(doc, [{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }] }], memory, {});
  assert.equal(textOf(doc), "one three");
  const cut = anchorsById(doc, "f1").get("a1");
  assert.equal(cut.detached, true);
  assert.deepEqual([cut.from, cut.to], [0, 0]);
  assert.equal(cut.exact, "two");
  assert.equal(statusOf(doc, cut), "deleted");
  applyOperation(doc, [{ fileId: "f1", edits: [{ from: 4, to: 4, insert: "two " }] }], memory, {});
  assert.equal(textOf(doc), text);
  assertAttached(doc, "a1", 4, 7, "two");
});

test("a cut in one file and a paste in another moves the record to the new file", () => {
  const doc = makeDoc(1, { f1: "one two three", f2: "alpha beta" });
  createAnchor(doc, "f1", "a1", 4, 7, ctx("one two three", 4, 7));
  doc.commit();
  const memory = new MoveMemory();
  applyOperation(doc, [{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }] }], memory, {});
  applyOperation(doc, [{ fileId: "f2", edits: [{ from: 6, to: 6, insert: "two " }] }], memory, {});
  assert.equal(textOf(doc, "f2"), "alpha two beta");
  assert.equal(doc.getMap(ANCHORS).get("a1").get("file"), "f2");
  assert.equal(anchorsById(doc, "f1").has("a1"), false);
  assertAttached(doc, "a1", 6, 9, "two", "f2");
});

test("a drag re-pins the anchor in the upward direction", () => {
  // Upward: the insertion comes first in before-coordinates.
  const text = "one two three four";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 8, 13, ctx(text, 8, 13));
  doc.commit();
  applyOperation(
    doc,
    [{ fileId: "f1", edits: [{ from: 0, to: 0, insert: "three " }, { from: 8, to: 14, insert: "" }] }],
    new MoveMemory(),
    {},
  );
  assert.equal(textOf(doc), "three one two four");
  assertAttached(doc, "a1", 0, 5, "three");
});

test("a drag re-pins the anchor in the downward direction", () => {
  const text = "one two three four";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7));
  doc.commit();
  applyOperation(
    doc,
    [{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }, { from: 14, to: 14, insert: "two " }] }],
    new MoveMemory(),
    {},
  );
  assert.equal(textOf(doc), "one three two four");
  assertAttached(doc, "a1", 10, 13, "two");
});

test("two peers that delete each half of a passage collapse it, and marking leaves one detached record", () => {
  const text = "one two three";
  const A = makeDoc(1, { f1: text });
  createAnchor(A, "f1", "a1", 4, 7, ctx(text, 4, 7));
  A.commit();
  const B = new LoroDoc();
  B.setPeerId(2);
  B.import(A.export({ mode: "snapshot" }));
  // A deletes "tw", B deletes "o": the two halves of "two".
  applyOperation(A, [{ fileId: "f1", edits: [{ from: 4, to: 6, insert: "" }] }], new MoveMemory(), {});
  applyOperation(B, [{ fileId: "f1", edits: [{ from: 6, to: 7, insert: "" }] }], new MoveMemory(), {});
  sync(A, B);
  assert.equal(textOf(A), "one  three");
  assert.equal(textOf(B), "one  three");
  for (const doc of [A, B]) {
    const anchor = anchorsById(doc, "f1").get("a1");
    assert.equal(anchor.collapsed, true, "collapsed after the sync");
    assert.equal(statusOf(doc, anchor), "deleted");
  }
  assert.deepEqual(markCollapsed(A, "f1"), ["a1"]);
  A.commit({ origin: ANCHORS_ORIGIN });
  sync(A, B);
  for (const doc of [A, B]) {
    assert.equal(doc.getMap(ANCHORS).get("a1").get("detached"), true, "record is detached");
    const anchor = anchorsById(doc, "f1").get("a1");
    assert.equal(anchor.detached, true);
    assert.equal(anchor.exact, "two");
  }
});

// A tracked deletion of "two " made on doc A, with the anchor recorded against
// it. Returns the doc, the change id and the fragment it left.
function trackedDeletion() {
  const text = "one two three";
  const A = makeDoc(1, { f1: text });
  createAnchor(A, "f1", "a1", 4, 7, ctx(text, 4, 7));
  A.commit();
  const memory = new MoveMemory();
  const before = textOf(A);
  const resolved = resolveAnchors(A, "f1");
  const [changeId] = trackedSplice(A, "f1", 4, 8, "", BOB);
  A.commit();
  assert.equal(typeof changeId, "string");
  const fragments = resolveFile(A, "f1")
    .gaps.flatMap((gap) => gap.fragments)
    .filter((fragment) => fragment.change === changeId);
  assert.equal(fragments.length, 1, "one fragment for one deletion");
  const [fragment] = fragments;
  recordOperation(
    A,
    [
      {
        fileId: "f1",
        before,
        anchors: resolved,
        edits: [{ from: 4, to: 8, insert: "" }],
        fragments: [{ id: fragment.id, edit: 0, start: 0, end: fragment.text.length }],
      },
    ],
    memory,
    {},
  );
  A.commit({ origin: ANCHORS_ORIGIN });
  return { A, changeId, fragment };
}

test("a tracked deletion rejected in another session restores the anchor", () => {
  const { A, changeId, fragment } = trackedDeletion();
  const pending = anchorsById(A, "f1").get("a1");
  assert.equal(statusOf(A, pending), "pending_deletion");
  const record = A.getMap(ANCHORS).get("a1");
  assert.equal(record.get("fragment"), fragment.id);
  assert.notEqual(record.get("offset"), undefined);

  const B = new LoroDoc();
  B.setPeerId(2);
  B.import(A.export({ mode: "snapshot" }));
  const beforeReject = textOf(B);
  assert.equal(reject(B, changeId), true);
  const afterReject = textOf(B);
  // The restored text lands where the deletion was; prefer that spot when the
  // text would fit at more than one point.
  const points = insertionPoints(beforeReject, afterReject, fragment.text);
  assert.ok(points.length > 0, `reject gave ${JSON.stringify(afterReject)}`);
  const at = points.reduce((best, p) => (Math.abs(p - 4) < Math.abs(best - 4) ? p : best));
  restoreByFragments(B, "f1", [{ fragment: fragment.id, at, text: fragment.text }]);
  B.commit({ origin: ANCHORS_ORIGIN });
  assert.equal(textOf(B), "one two three");
  assertAttached(B, "a1", 4, 7, "two");

  sync(A, B);
  assert.equal(A.getMap(CHANGES).get(changeId), undefined, "the change is gone on A");
  assertAttached(A, "a1", 4, 7, "two");
});

test("a tracked deletion accepted in another session leaves the anchor deleted", () => {
  const { A, changeId, fragment } = trackedDeletion();
  assert.equal(accept(A, changeId), true);
  assert.equal(A.getMap(CHANGES).get(fragment.id), undefined, "the fragment is gone");
  assert.equal(textOf(A), "one three");
  assert.equal(statusOf(A, anchorsById(A, "f1").get("a1")), "deleted");

  const B = new LoroDoc();
  B.setPeerId(2);
  B.import(A.export({ mode: "snapshot" }));
  assert.equal(statusOf(B, anchorsById(B, "f1").get("a1")), "deleted");
});

test("editsFromDelta turns a text delta into before-coordinate edits", () => {
  const delta = [{ retain: 3 }, { delete: 2 }, { insert: "xy" }, { retain: 1 }, { insert: "z" }];
  assert.deepEqual(editsFromDelta(delta), [
    { from: 3, to: 5, insert: "xy" },
    { from: 6, to: 6, insert: "z" },
  ]);
});

test("undo through the UndoManager walks a cut and a paste back", () => {
  const text = "one two three";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7));
  doc.commit();
  const textId = doc.getMap("files").get("f1").id;
  let pending = [];
  doc.subscribe((batch) => {
    for (const event of batch.events) {
      if (event.target === textId && event.diff.type === "text") pending.push(event.diff.diff);
    }
  });
  const manager = new UndoManager(doc, { mergeInterval: 0, excludeOriginPrefixes: [ANCHORS_ORIGIN] });
  const memory = new MoveMemory();
  const index = new AnchorIndex(doc);
  index.load("f1");
  index.attach(memory);
  index.apply([{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }] }], memory, {});
  index.apply([{ fileId: "f1", edits: [{ from: 4, to: 4, insert: "two " }] }], memory, {});
  const undoOnce = () => {
    pending = [];
    assert.equal(manager.undo(), true, "undo applied");
    assert.equal(pending.length, 1, "one text event per undo");
    pending = [];
  };

  undoOnce();
  assertIndexMatches(index, doc, "f1");
  const afterPaste = anchorsById(doc, "f1").get("a1");
  assert.equal(afterPaste.detached, true, "the paste is undone, so the anchor is detached");
  assert.equal(statusOf(doc, afterPaste), "deleted");

  undoOnce();
  assertIndexMatches(index, doc, "f1");
  assert.equal(textOf(doc), text);
  assertAttached(doc, "a1", 4, 7, "two");
  assert.equal(manager.canUndo(), false, "only anchor commits were left, and they are excluded");
});

test("undo through the UndoManager brings a passage back to a file that is not being edited", () => {
  const doc = makeDoc(1, { f1: "one two three", f2: "alpha beta" });
  createAnchor(doc, "f1", "a1", 4, 7, ctx("one two three", 4, 7));
  doc.commit();
  const manager = new UndoManager(doc, { mergeInterval: 0, excludeOriginPrefixes: [ANCHORS_ORIGIN] });
  const memory = new MoveMemory();
  const index = new AnchorIndex(doc);
  index.load("f1");
  index.load("f2");
  index.attach(memory);
  // The cut takes "two " out of f1, and the paste puts it into f2.
  index.apply([{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }] }], memory, {});
  index.apply([{ fileId: "f2", edits: [{ from: 6, to: 6, insert: "two " }] }], memory, {});
  const pasted = index.find("a1");
  assert.equal(pasted.fileId, "f2");
  assert.deepEqual([pasted.anchor.from, pasted.anchor.to, pasted.anchor.detached], [6, 9, false]);

  assert.equal(manager.undo(), true, "the paste is undone");
  const undone = index.find("a1");
  assert.equal(undone.anchor.detached, true, "the anchor is detached once the paste is undone");
  assert.equal(statusOf(doc, undone.anchor), "deleted");
  assertIndexMatches(index, doc, "f1");
  assertIndexMatches(index, doc, "f2");

  assert.equal(manager.undo(), true, "the cut is undone");
  assert.equal(textOf(doc, "f1"), "one two three");
  assert.equal(textOf(doc, "f2"), "alpha beta");
  assertAttached(doc, "a1", 4, 7, "two");
  const back = index.find("a1");
  assert.equal(back.fileId, "f1");
  assert.deepEqual([back.anchor.from, back.anchor.to, back.anchor.detached], [4, 7, false]);
  assert.equal(statusOf(doc, back.anchor), "exact");
  assertIndexMatches(index, doc, "f1");
  assertIndexMatches(index, doc, "f2");
});

// Cuts one end of the anchor on "two" in "one two three", checks the anchor on
// what is left, then undoes the cut. The undo puts the piece back at the edge
// it was cut from, so the anchor reads "two" again.
function cutEndThenUndo(edit, leftFrom, leftTo) {
  const text = "one two three";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7));
  doc.commit();
  const manager = new UndoManager(doc, { mergeInterval: 0, excludeOriginPrefixes: [ANCHORS_ORIGIN] });
  const memory = new MoveMemory();
  const index = new AnchorIndex(doc);
  index.load("f1");
  index.attach(memory);
  index.apply([{ fileId: "f1", edits: [edit] }], memory, {});
  const cut = index.find("a1").anchor;
  assert.deepEqual([cut.from, cut.to, cut.detached], [leftFrom, leftTo, false]);
  assert.equal(statusOf(doc, cut), "modified");
  assertIndexMatches(index, doc, "f1");

  assert.equal(manager.undo(), true, "the cut is undone");
  assert.equal(textOf(doc), text);
  assertAttached(doc, "a1", 4, 7, "two");
  const back = index.find("a1").anchor;
  assert.deepEqual([back.from, back.to, back.detached], [4, 7, false]);
  assert.equal(statusOf(doc, back), "exact");
  assertIndexMatches(index, doc, "f1");
}

test("undo after a boundary deletion brings the passage back exact", () => {
  // "tw" goes from the head, so the anchor is left on the "o" at 4..5.
  cutEndThenUndo({ from: 4, to: 6, insert: "" }, 4, 5);
});

test("undo after a tail deletion brings the passage back exact", () => {
  // "wo" goes from the tail, so the anchor is left on the "t" at 4..5.
  cutEndThenUndo({ from: 5, to: 7, insert: "" }, 4, 5);
});

test("redo of a tracked deletion detaches the comment with its fragment", () => {
  const text = "one two three";
  const doc = makeDoc(1, { f1: text });
  createAnchor(doc, "f1", "a1", 4, 7, ctx(text, 4, 7));
  doc.commit();
  const manager = new UndoManager(doc, { mergeInterval: 0, excludeOriginPrefixes: [ANCHORS_ORIGIN] });
  const memory = new MoveMemory();
  const index = new AnchorIndex(doc);
  index.load("f1");
  index.attach(memory);

  // The tracked deletion as the editor makes it: the text and its fragment
  // records in one commit, then the anchors under ANCHORS_ORIGIN.
  const edits = [{ from: 4, to: 8, insert: "" }];
  trackedSplice(doc, "f1", 4, 8, "", { who: "me", author: "Me" });
  doc.commit();
  const created = resolveFile(doc, "f1").gaps.flatMap((gap) =>
    gap.fragments.map((f) => ({ id: f.id, text: f.text })),
  );
  assert.equal(created.length, 1, "one fragment for one deletion");
  const [fragment] = created;
  index.record([{ fileId: "f1", edits, fragments: fragmentRangesFor(text, edits, created) }], memory);
  doc.commit({ origin: ANCHORS_ORIGIN });
  const pending = anchorsById(doc, "f1").get("a1");
  assert.equal(pending.detached, true, "the comment is detached");
  assert.equal(pending.fragment, fragment.id, "the comment holds its fragment");
  assert.equal(statusOf(doc, pending), "pending_deletion");

  // The undo puts the text back, and the comment follows it back to 4..7.
  assert.equal(manager.undo(), true, "undo applied");
  assert.equal(textOf(doc), text);
  assertAttached(doc, "a1", 4, 7, "two");
  assertIndexMatches(index, doc, "f1");

  // The redo deletes the text again. The fragment records come back in the same
  // batch, and the comment takes the fragment it sits in.
  assert.equal(manager.redo(), true, "redo applied");
  assert.equal(textOf(doc), "one three");
  const redone = anchorsById(doc, "f1").get("a1");
  assert.equal(redone.detached, true, "the comment is detached again");
  assert.equal(redone.fragment, fragment.id, "it takes back the fragment it had before the undo");
  assert.equal(redone.offset, 0, "it starts at the front of the fragment");
  assert.equal(statusOf(doc, redone), "pending_deletion");
  assertIndexMatches(index, doc, "f1");
});

// A peer B that starts as a snapshot of A, with an index on each.
function twoIndexedPeers(A) {
  const B = new LoroDoc();
  B.setPeerId(2);
  B.import(A.export({ mode: "snapshot" }));
  const indexA = new AnchorIndex(A);
  const indexB = new AnchorIndex(B);
  return { B, indexA, indexB };
}

test("an index on another peer follows a remote cut and paste across files", () => {
  const A = makeDoc(1, { f1: "one two three", f2: "alpha beta" });
  createAnchor(A, "f1", "a1", 4, 7, ctx("one two three", 4, 7));
  createAnchor(A, "f2", "a2", 6, 10, ctx("alpha beta", 6, 10));
  A.commit();
  const { B, indexA, indexB } = twoIndexedPeers(A);
  for (const index of [indexA, indexB]) {
    index.load("f1");
    index.load("f2");
  }
  indexA.attach(new MoveMemory());
  const memory = new MoveMemory();
  indexB.apply([{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }] }], memory, {});
  indexB.apply([{ fileId: "f2", edits: [{ from: 6, to: 6, insert: "two " }] }], memory, {});
  sync(A, B);
  assert.equal(textOf(A, "f2"), "alpha two beta");
  for (const [index, doc] of [
    [indexA, A],
    [indexB, B],
  ]) {
    assertIndexMatches(index, doc, "f1");
    assertIndexMatches(index, doc, "f2");
  }
  assert.equal(indexA.anchors("f1").some((a) => a.id === "a1"), false, "a1 left f1");
  const moved = indexA.anchors("f2").find((a) => a.id === "a1");
  assert.deepEqual([moved.from, moved.to, moved.detached], [6, 9, false]);
  assert.equal(indexA.anchors("f2").find((a) => a.id === "a2").from, 10);
});

test("an index on another peer shrinks an anchor that a remote deletion overlapped", () => {
  const text = "one two three";
  const A = makeDoc(1, { f1: text });
  createAnchor(A, "f1", "a1", 4, 7, ctx(text, 4, 7));
  A.commit();
  const { B, indexA, indexB } = twoIndexedPeers(A);
  indexA.load("f1");
  indexA.attach(new MoveMemory());
  indexB.load("f1");
  // B deletes "o " at 6 and 7, the end of "two".
  indexB.apply([{ fileId: "f1", edits: [{ from: 6, to: 8, insert: "" }] }], new MoveMemory(), {});
  sync(A, B);
  assert.equal(textOf(A), "one twthree");
  const shrunk = indexA.anchors("f1").find((a) => a.id === "a1");
  assert.deepEqual([shrunk.from, shrunk.to, shrunk.detached], [4, 6, false]);
  assertIndexMatches(indexA, A);
  assertIndexMatches(indexB, B);
});

test("an index on another peer detaches an anchor when a remote cut takes its text", () => {
  const text = "one two three";
  const A = makeDoc(1, { f1: text });
  createAnchor(A, "f1", "a1", 4, 7, ctx(text, 4, 7));
  A.commit();
  const { B, indexA, indexB } = twoIndexedPeers(A);
  indexA.load("f1");
  indexA.attach(new MoveMemory());
  indexB.load("f1");
  indexB.apply([{ fileId: "f1", edits: [{ from: 4, to: 8, insert: "" }] }], new MoveMemory(), {});
  sync(A, B);
  const cut = indexA.anchors("f1").find((a) => a.id === "a1");
  assert.equal(cut.detached, true);
  assert.equal(cut.exact, "two");
  assert.equal(statusOf(A, cut), "deleted");
  assertIndexMatches(indexA, A);
  assertIndexMatches(indexB, B);
});

test("a remote cut seen as text alone collapses the anchor, and its record write detaches it", () => {
  const text = "one two three";
  const A = makeDoc(1, { f1: text });
  createAnchor(A, "f1", "a1", 4, 7, ctx(text, 4, 7));
  A.commit();
  const { B, indexA, indexB } = twoIndexedPeers(A);
  indexA.load("f1");
  indexA.attach(new MoveMemory());
  indexB.load("f1");
  const { textOnly, rest } = editInTwoCommits(B, indexB, new MoveMemory(), "f1", [{ from: 4, to: 8, insert: "" }]);

  A.import(textOnly);
  assert.equal(textOf(A), "one three");
  const collapsed = indexA.anchors("f1").find((a) => a.id === "a1");
  assert.equal(collapsed.collapsed, true);
  assert.equal(collapsed.detached, false);
  assert.equal(statusOf(A, collapsed), "deleted");
  assertIndexMatches(indexA, A);

  A.import(rest);
  const detached = indexA.anchors("f1").find((a) => a.id === "a1");
  assert.equal(detached.detached, true);
  assert.equal(detached.collapsed, false);
  assert.equal(statusOf(A, detached), "deleted");
  assertIndexMatches(indexA, A);
});

// ---------------------------------------------------------------------------
// Fuzzers

// One session edits one to three files, with the clipboard moving text between
// them. Every step is checked against the oracle.
function oneSession(seed) {
  const rng = random(seed);
  const log = [];
  const doc = new LoroDoc();
  doc.setPeerId(1);
  const memory = new MoveMemory();
  const files = new Map();
  const wordOf = new Map();
  const fileCount = 1 + rng.pick(3);
  for (let i = 1; i <= fileCount; i++) {
    const fileId = `f${i}`;
    const layout = layoutWords(rng, fileId, 3 + rng.pick(6));
    doc.getMap("files").setContainer(fileId, new LoroText()).insert(0, layout.text);
    files.set(fileId, layout.units);
    for (const a of layout.anchors) {
      wordOf.set(a.id, a.word);
      createAnchor(doc, fileId, a.id, a.from, a.to, ctx(layout.text, a.from, a.to));
    }
  }
  doc.commit();
  const index = new AnchorIndex(doc);
  for (const fileId of files.keys()) index.load(fileId);
  const apply = (edits) => index.apply(edits, memory, {});
  const checkAll = () => {
    checkAnchors(doc, files, wordOf);
    for (const fileId of files.keys()) assertIndexMatches(index, doc, fileId);
  };
  withLog(`one session seed=${seed}`, log, () => {
    checkAll();
    let clip = null;
    const steps = 4 + rng.pick(13);
    for (let k = 0; k < steps; k++) {
      const ids = [...files.keys()];
      const fileId = ids[rng.pick(ids.length)];
      clip = stepOn(rng, doc, memory, { fileId, units: files.get(fileId), base: 0 }, clip, log, { apply });
      checkAll();
    }
  });
}

// Two peers share one file: region A is the text before the sentinel, region B
// the text after it. Each peer edits only its own region, so the regions never
// overlap and the separator is never touched. They sync at random points.
function twoPeers(seed) {
  const rng = random(seed);
  const log = [];
  const left = layoutWords(rng, "L", 2 + rng.pick(3));
  const right = layoutWords(rng, "R", 2 + rng.pick(3));
  const text = left.text + SEP + right.text;
  const rightStart = left.text.length + SEP.length;
  const A = makeDoc(1, { f1: text });
  const wordOf = new Map();
  for (const a of left.anchors) {
    wordOf.set(a.id, a.word);
    createAnchor(A, "f1", a.id, a.from, a.to, ctx(text, a.from, a.to));
  }
  for (const a of right.anchors) {
    const from = a.from + rightStart;
    const to = a.to + rightStart;
    wordOf.set(a.id, a.word);
    createAnchor(A, "f1", a.id, from, to, ctx(text, from, to));
  }
  A.commit();
  const { B, indexA, indexB } = twoIndexedPeers(A);
  indexA.load("f1");
  indexB.load("f1");
  const peers = [
    { who: "A", side: "A", doc: A, index: indexA, units: left.units, memory: new MoveMemory(), clip: null },
    { who: "B", side: "B", doc: B, index: indexB, units: right.units, memory: new MoveMemory(), clip: null },
  ];
  indexA.attach(peers[0].memory);
  indexB.attach(peers[1].memory);
  // The whole file as the oracle sees it: region A, the separator, region B.
  const expected = () => new Map([["f1", [...left.units, ...[...SEP].map((ch) => unit(ch)), ...right.units]]]);
  // Where a peer's region starts in its own copy of the file. Region A starts at
  // 0; region B starts after the separator's trailing space, so at the sentinel
  // plus five.
  const regionBase = (doc, side) => {
    const body = textOf(doc);
    const at = body.indexOf("ZZZZ");
    assert.ok(at > 0 && body.slice(at - 1, at + 5) === SEP, "separator intact");
    return side === "A" ? 0 : at + 5;
  };

  // Both peers' documents match the oracle, and both indexes match the records.
  const checkPeers = () => {
    checkAnchors(A, expected(), wordOf);
    checkAnchors(B, expected(), wordOf);
    for (const peer of peers) assertIndexMatches(peer.index, peer.doc, "f1");
  };

  withLog(`two peers seed=${seed}`, log, () => {
    checkPeers();
    let untilSync = 1 + rng.pick(4);
    const steps = 4 + rng.pick(12);
    for (let k = 0; k < steps; k++) {
      const peer = peers[rng.pick(2)];
      const target = { fileId: "f1", units: peer.units, base: regionBase(peer.doc, peer.side) };
      peer.clip = stepOn(rng, peer.doc, peer.memory, target, peer.clip, log, {
        who: peer.who,
        typeAnywhere: false,
        apply: (edits) => peer.index.apply(edits, peer.memory, {}),
      });
      untilSync -= 1;
      if (untilSync === 0) {
        sync(A, B);
        log.push("sync");
        checkPeers();
        untilSync = 1 + rng.pick(4);
      }
    }
    sync(A, B);
    log.push("sync");
    checkPeers();
  });
}

// One doc, one file, the real UndoManager. A random history is undone one step
// at a time, each undo recorded as the anchor layer would see it. Afterwards
// every anchor must be back where it started.
function undoCase(seed) {
  const rng = random(seed);
  const log = [];
  const layout = layoutWords(rng, "f1", 3 + rng.pick(6));
  const doc = new LoroDoc();
  doc.setPeerId(1);
  doc.getMap("files").setContainer("f1", new LoroText()).insert(0, layout.text);
  const wordOf = new Map();
  const ranges = new Map();
  for (const a of layout.anchors) {
    wordOf.set(a.id, a.word);
    ranges.set(a.id, [a.from, a.to]);
    createAnchor(doc, "f1", a.id, a.from, a.to, ctx(layout.text, a.from, a.to));
  }
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  const textId = doc.getMap("files").get("f1").id;
  let pending = [];
  doc.subscribe((batch) => {
    for (const event of batch.events) {
      if (event.target === textId && event.diff.type === "text") pending.push(event.diff.diff);
    }
  });
  const manager = new UndoManager(doc, { mergeInterval: 0, excludeOriginPrefixes: [ANCHORS_ORIGIN] });
  const memory = new MoveMemory();
  const apply = (edits) => index.apply(edits, memory, {});
  index.attach(memory);

  withLog(`undo seed=${seed}`, log, () => {
    let clip = null;
    const steps = 3 + rng.pick(6);
    for (let k = 0; k < steps; k++) {
      clip = stepOn(rng, doc, memory, { fileId: "f1", units: layout.units, base: 0 }, clip, log, { apply });
    }
    for (let k = 0; k < steps; k++) {
      pending = [];
      assert.equal(manager.undo(), true, `undo ${k + 1} of ${steps} applied`);
      assert.equal(pending.length, 1, `undo ${k + 1} of ${steps} text events`);
      pending = [];
    }
    assert.equal(manager.canUndo(), false, "nothing left to undo");
    assert.equal(textOf(doc), layout.text, "text after every undo");
    assertIndexMatches(index, doc, "f1");
    const found = anchorsById(doc, "f1");
    for (const [id, [from, to]] of ranges) {
      const anchor = found.get(id);
      assert.ok(anchor, `${id} is listed`);
      assert.equal(anchor.detached, false, `${id} is attached`);
      assert.deepEqual([anchor.from, anchor.to], [from, to], `${id} range`);
      assert.equal(anchor.exact, wordOf.get(id), `${id} exact`);
      assert.equal(statusOf(doc, anchor), "exact", `${id} status`);
    }
  });
}

test("fuzz: one session of moves keeps every anchor on the tagged oracle", () => {
  for (let seed = 1; seed <= 150; seed++) oneSession(seed);
});

test("fuzz: two peers in disjoint halves converge on the oracle", () => {
  for (let seed = 1001; seed <= 1150; seed++) twoPeers(seed);
});

test("fuzz: undoing a random history walks every anchor back", () => {
  for (let seed = 2001; seed <= 2100; seed++) undoCase(seed);
});
