// The anchor planner, pure and without a DOM or Loro. The fixtures in
// tests/fixtures/anchors.json pin each rule by hand: a move, a shrink, a
// detach, a paste that re-pins, a paste that does not, a drag either way,
// tracked fragments and surrogate pairs. The seeded fuzzer at the end runs
// random histories of typing, cutting, pasting, copying and dragging against a
// model that tags every unit with the anchor it belongs to. The model applies
// the same memory rule as the planner (the newest remembered text that occurs
// exactly once in an insertion takes its held anchors back), so the fuzzer
// checks positions, quotes, list membership and the memory size, not the
// policy itself.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { LoroDoc, LoroText } from "loro-crdt";
import {
  ANCHORS_ORIGIN,
  AnchorIndex,
  CONTEXT_UNITS,
  MoveMemory,
  attachByHand,
  createAnchor,
  deriveAttachment,
  fragmentRangesFor,
  planOperation,
  resolveAnchors,
  restoreByFragments,
  statusOf,
  throughEdits,
} from "../../src/lib/anchors.js";
import { rejectRestoring, resolveFile, trackedSplice } from "../../src/lib/track-changes.js";

const fixtures = JSON.parse(readFileSync(new URL("../fixtures/anchors.json", import.meta.url), "utf8"));

// The generator the fuzz scripts use, so a failing seed replays the same case.
function random(seed) {
  let state = seed;
  const rnd = () => {
    state = (state * 1103515245 + 12345) & 0x7fffffff;
    return state / 0x7fffffff;
  };
  const pick = (n) => Math.floor(rnd() * n);
  return { rnd, pick };
}

// ---------------------------------------------------------------------------
// Fixtures through the planner

// A fixture gives every file's input, optional pre-remembered entries and the
// expected output. Anchor lists are compared by id, and each expected key is
// compared. Keys that a fixture does not name are not checked.
function runFixture(f) {
  const memory = new MoveMemory();
  for (const entry of f.memory ?? []) memory.remember(entry.text, entry.held);
  const out = planOperation(structuredClone(f.files), memory);
  assert.deepEqual(
    out.files.map((file) => file.fileId),
    f.expect.files.map((file) => file.fileId),
    `${f.name}: file order`,
  );
  for (const want of f.expect.files) {
    const got = out.files.find((file) => file.fileId === want.fileId);
    assert.equal(got.after, want.after, `${f.name}: ${want.fileId} after text`);
    assert.deepEqual(
      got.anchors.map((a) => a.id).sort(),
      want.anchors.map((a) => a.id).sort(),
      `${f.name}: ${want.fileId} anchor ids`,
    );
    for (const wantAnchor of want.anchors) {
      const gotAnchor = got.anchors.find((a) => a.id === wantAnchor.id);
      for (const [key, value] of Object.entries(wantAnchor)) {
        assert.deepEqual(gotAnchor[key], value, `${f.name}: ${wantAnchor.id}.${key}`);
      }
    }
  }
  if ("memorySize" in f.expect) {
    assert.equal(memory.size, f.expect.memorySize, `${f.name}: memory size`);
  }
}

for (const f of fixtures) {
  test(`fixture: ${f.name}`, () => runFixture(f));
}

// ---------------------------------------------------------------------------
// Seeded fuzzer: typing, cutting, pasting, copying and dragging, with the
// planner checked against a tagged text model after every operation

const CASES = 300;
const CLIPBOARD_MAX = 4;
const LETTERS = "abcdefghijklmnopqrstuvwxyz";

const textOf = (units) => units.map((u) => u.ch).join("");

// The span of every tag. A tag must be one run of units.
function spansOf(units) {
  const spans = new Map();
  units.forEach((u, i) => {
    for (const id of u.tags) {
      const span = spans.get(id);
      if (span) {
        span.end = i + 1;
        span.count += 1;
      } else {
        spans.set(id, { start: i, end: i + 1, count: 1 });
      }
    }
  });
  for (const [id, span] of spans) {
    assert.equal(span.end - span.start, span.count, `model tag ${id} is not one run`);
  }
  return spans;
}

// Offsets where text may go: never strictly inside a tagged span.
function validPositions(units) {
  const spans = [...spansOf(units).values()];
  const out = [];
  for (let p = 0; p <= units.length; p++) {
    if (!spans.some((s) => s.start < p && p < s.end)) out.push(p);
  }
  return out;
}

// Widens [from, to) until no tagged span is cut. A span that touches the
// range is taken whole.
function widen(units, from, to) {
  const spans = [...spansOf(units).values()];
  let changed = true;
  while (changed) {
    changed = false;
    for (const s of spans) {
      if (s.end > from && s.start < to) {
        if (s.start < from) {
          from = s.start;
          changed = true;
        }
        if (s.end > to) {
          to = s.end;
          changed = true;
        }
      }
    }
  }
  return { from, to };
}

// A range around a random tagged span, so it holds at least one whole span.
function pickRange(rng, units) {
  const spans = [...spansOf(units).values()];
  if (spans.length === 0) return null;
  const s = spans[rng.pick(spans.length)];
  const from = Math.max(0, s.start - rng.pick(4));
  const to = Math.min(units.length, s.end + rng.pick(4));
  return widen(units, from, to);
}

// The anchors a deletion takes whole, with their offsets in the deleted text.
function heldIn(units, from, to) {
  const held = [];
  for (const [id, span] of spansOf(units)) {
    if (span.end <= from || span.start >= to) continue;
    assert.ok(span.start >= from && span.end <= to, `model deletion cuts tag ${id}`);
    held.push({ id, offset: span.start - from, text: textOf(units.slice(span.start, span.end)) });
  }
  return held;
}

// Every start of needle in hay, overlaps included.
function occurrences(hay, needle) {
  const at = [];
  let i = hay.indexOf(needle);
  while (i !== -1) {
    at.push(i);
    i = hay.indexOf(needle, i + 1);
  }
  return at;
}

// The planner's take: the newest entry whose text occurs exactly once in the
// inserted text. The entry leaves the memory.
function takeMemory(model, inserted) {
  for (let i = model.memory.length - 1; i >= 0; i--) {
    const at = occurrences(inserted, model.memory[i].text);
    if (at.length === 1) {
      const [entry] = model.memory.splice(i, 1);
      return { entry, at: at[0] };
    }
  }
  return null;
}

// Applies one operation's edits to the model the way the planner does:
// deleted text is remembered with the anchors it takes whole, then each
// insertion asks the memory for held anchors and tags them back in place.
function applyEdits(model, fileIdx, edits) {
  const file = model.files[fileIdx];
  const src = file.units;
  const out = [];
  const pending = [];
  const inserts = [];
  let cursor = 0;
  for (const e of edits) {
    for (let i = cursor; i < e.from; i++) out.push(src[i]);
    if (e.to > e.from) {
      const held = heldIn(src, e.from, e.to);
      if (held.length > 0) pending.push({ text: textOf(src.slice(e.from, e.to)), held });
    }
    if (e.insert.length > 0) {
      inserts.push({ pos: out.length, text: e.insert });
      for (const ch of e.insert) out.push({ ch, tags: new Set() });
    }
    cursor = e.to;
  }
  for (let i = cursor; i < src.length; i++) out.push(src[i]);
  file.units = out;
  model.memory.push(...pending);
  for (const { pos, text } of inserts) {
    const hit = takeMemory(model, text);
    if (!hit) continue;
    for (const h of hit.entry.held) {
      const attached = model.files.some((f) => f.units.some((u) => u.tags.has(h.id)));
      assert.equal(attached, false, `held anchor ${h.id} is still attached`);
      const start = pos + hit.at + h.offset;
      assert.equal(textOf(out.slice(start, start + h.text.length)), h.text, `re-pin of ${h.id} misplaced`);
      for (let k = 0; k < h.text.length; k++) out[start + k].tags.add(h.id);
      model.home.set(h.id, file.fileId);
    }
  }
}

// A random document: 1 to 3 files of 40 to 200 words, with 1 to 6 anchors on
// whole-word ranges that do not overlap.
function newCase(rng) {
  const model = { files: [], memory: [], home: new Map(), exact: new Map(), clipboard: [] };
  const fileCount = 1 + rng.pick(3);
  for (let f = 0; f < fileCount; f++) {
    let text = "";
    const words = 40 + rng.pick(161);
    for (let w = 0; w < words; w++) {
      for (let n = 1 + rng.pick(8); n > 0; n--) text += LETTERS[rng.pick(LETTERS.length)];
      text += rng.pick(12) === 0 ? "\n" : " ";
    }
    model.files.push({ fileId: `f${f + 1}`, units: Array.from(text, (ch) => ({ ch, tags: new Set() })) });
  }
  let nextId = 0;
  const wanted = 1 + rng.pick(6);
  for (let k = 0; k < wanted; k++) {
    for (let tries = 0; tries < 50; tries++) {
      const file = model.files[rng.pick(model.files.length)];
      const words = [...textOf(file.units).matchAll(/[a-z]+/g)];
      const i = rng.pick(words.length);
      const j = Math.min(words.length - 1, i + rng.pick(3));
      const from = words[i].index;
      const to = words[j].index + words[j].length;
      if (file.units.slice(from, to).some((u) => u.tags.size > 0)) continue;
      const id = `a${nextId++}`;
      for (let x = from; x < to; x++) file.units[x].tags.add(id);
      model.exact.set(id, textOf(file.units.slice(from, to)));
      model.home.set(id, file.fileId);
      break;
    }
  }
  const carried = [];
  for (const file of model.files) {
    const text = textOf(file.units);
    for (const [id, s] of spansOf(file.units)) {
      carried.push({
        id,
        file: file.fileId,
        from: s.start,
        to: s.end,
        detached: false,
        fragment: null,
        offset: null,
        exact: model.exact.get(id),
        prefix: text.slice(Math.max(0, s.start - CONTEXT_UNITS), s.start),
        suffix: text.slice(s.end, s.end + CONTEXT_UNITS),
        collapsed: false,
        unresolved: false,
      });
    }
  }
  return { model, carried, memory: new MoveMemory() };
}

// One operation, chosen at random. Falls back to typing when the chosen kind
// has nothing to act on. Pastes pop the clipboard; copies leave it unchanged.
function chooseOp(rng, model) {
  const kinds = ["type", "cut", "paste", "copy", "drag"];
  const kind = kinds[rng.pick(kinds.length)];
  const fileIdx = rng.pick(model.files.length);
  const file = model.files[fileIdx];
  const positions = validPositions(file.units);
  const insertAt = (label, insert) => {
    const p = positions[rng.pick(positions.length)];
    const shown = JSON.stringify(insert.slice(0, 24));
    return { label: `${label} ${shown} ${file.fileId}@${p}`, fileIdx, edits: [{ from: p, to: p, insert }] };
  };
  if (kind === "paste" && model.clipboard.length > 0) {
    return insertAt("paste", model.clipboard.pop());
  }
  if (kind === "copy" && model.clipboard.length > 0) {
    return insertAt("copy", model.clipboard[model.clipboard.length - 1]);
  }
  if (kind === "cut") {
    const range = pickRange(rng, file.units);
    if (range) {
      model.clipboard.push(textOf(file.units.slice(range.from, range.to)));
      if (model.clipboard.length > CLIPBOARD_MAX) model.clipboard.shift();
      return {
        label: `cut ${file.fileId}[${range.from},${range.to})`,
        fileIdx,
        edits: [{ from: range.from, to: range.to, insert: "" }],
      };
    }
  }
  if (kind === "drag") {
    const range = pickRange(rng, file.units);
    const targets = range ? positions.filter((p) => p < range.from || p > range.to) : [];
    if (range && targets.length > 0) {
      const p = targets[rng.pick(targets.length)];
      const moved = textOf(file.units.slice(range.from, range.to));
      const cut = { from: range.from, to: range.to, insert: "" };
      const paste = { from: p, to: p, insert: moved };
      return {
        label: `drag ${file.fileId}[${range.from},${range.to}) to ${p}`,
        fileIdx,
        edits: p < range.from ? [paste, cut] : [cut, paste],
      };
    }
  }
  let letters = "";
  for (let n = 1 + rng.pick(5); n > 0; n--) letters += LETTERS[rng.pick(LETTERS.length)];
  return insertAt("type", letters);
}

// After an operation the planner's output must match the model: the same
// text in every file, every tagged run attached with its exact quote, every
// untagged anchor detached and listed where it was last attached, and the
// same memory size.
function check(model, out, memory, where) {
  assert.equal(out.files.length, model.files.length, `${where}: file count`);
  assert.equal(memory.size, model.memory.length, `${where}: memory size`);
  const listedIn = new Map();
  const byId = new Map();
  const afterOf = new Map();
  out.files.forEach((planned, i) => {
    const file = model.files[i];
    assert.equal(planned.fileId, file.fileId, `${where}: file order`);
    assert.equal(planned.after, textOf(file.units), `${where}: after text of ${file.fileId}`);
    afterOf.set(file.fileId, planned.after);
    for (const a of planned.anchors) {
      assert.equal(listedIn.has(a.id), false, `${where}: ${a.id} is listed twice`);
      listedIn.set(a.id, file.fileId);
      byId.set(a.id, a);
      assert.equal(a.file, file.fileId, `${where}: ${a.id} file`);
    }
  });
  assert.equal(byId.size, model.exact.size, `${where}: anchor count`);
  const present = new Map();
  for (const file of model.files) {
    for (const [id, span] of spansOf(file.units)) present.set(id, { fileId: file.fileId, ...span });
  }
  for (const [id, exact] of model.exact) {
    const a = byId.get(id);
    assert.ok(a, `${where}: ${id} missing from the planner output`);
    assert.equal(a.exact, exact, `${where}: ${id} exact`);
    const here = present.get(id);
    if (here) {
      assert.equal(listedIn.get(id), here.fileId, `${where}: ${id} listed in the wrong file`);
      assert.equal(a.detached, false, `${where}: ${id} should be attached`);
      assert.equal(a.length ?? null, null, `${where}: ${id} attached length`);
      assert.equal(a.from, here.start, `${where}: ${id} from`);
      assert.equal(a.to, here.end, `${where}: ${id} to`);
      assert.equal(afterOf.get(here.fileId).slice(a.from, a.to), a.exact, `${where}: ${id} text under the anchor`);
    } else {
      assert.equal(a.detached, true, `${where}: ${id} should be detached`);
      assert.equal(a.length, exact.length, `${where}: ${id} detached length`);
      assert.equal(a.from, 0, `${where}: ${id} detached from`);
      assert.equal(a.to, 0, `${where}: ${id} detached to`);
      assert.equal(listedIn.get(id), model.home.get(id), `${where}: ${id} home file`);
    }
  }
}

function runCase(seed) {
  const rng = random(seed);
  const { model, carried, memory } = newCase(rng);
  const log = [];
  let list = carried;
  const steps = 8 + rng.pick(13);
  for (let step = 0; step < steps; step++) {
    const op = chooseOp(rng, model);
    log.push(op.label);
    const where = `seed=${seed} step=${step} ops=${JSON.stringify(log)}`;
    const input = model.files.map((file, i) => ({
      fileId: file.fileId,
      before: textOf(file.units),
      anchors: list.filter((a) => a.file === file.fileId).map((a) => ({ ...a })),
      edits: i === op.fileIdx ? op.edits : [],
    }));
    const out = planOperation(input, memory);
    applyEdits(model, op.fileIdx, op.edits);
    check(model, out, memory, where);
    list = out.files.flatMap((file) => file.anchors.map((a) => ({ ...a, file: file.fileId })));
  }
}

test("seeded fuzz: anchors follow typing, cutting, pasting, copying and dragging", () => {
  for (let iter = 0; iter < CASES; iter++) runCase(1000 + iter);
});

// ---------------------------------------------------------------------------
// The browser side: the fragments of a deletion, and the attachment of a comment

test("throughEdits: a point moves past earlier edits and stays before an insertion", () => {
  assert.equal(throughEdits(5, [{ from: 0, to: 0, insert: "ab" }]), 7);
  assert.equal(throughEdits(5, [{ from: 5, to: 5, insert: "ab" }]), 5);
  assert.equal(throughEdits(5, [{ from: 4, to: 7, insert: "" }]), 4);
  assert.equal(throughEdits(5, [{ from: 3, to: 5, insert: "" }]), 3);
});

test("fragmentRangesFor: a deletion takes the two fragments of its gap in rank order", () => {
  const before = "one two three";
  // "two " is deleted at 4..8, so its point is 4.
  const gaps = [{ at: 4, fragments: [{ id: "f-b", text: "o ", rank: 1 }, { id: "f-a", text: "tw", rank: 0 }] }];
  assert.deepEqual(fragmentRangesFor(before, [{ from: 4, to: 8, insert: "" }], gaps), [
    { id: "f-a", edit: 0, start: 0, end: 2 },
    { id: "f-b", edit: 0, start: 2, end: 4 },
  ]);
  // An insertion before the deletion moves its point to 6 in the new text.
  const later = [{ at: 6, fragments: gaps[0].fragments }];
  const edits = [{ from: 0, to: 0, insert: "XX" }, { from: 4, to: 8, insert: "" }];
  assert.deepEqual(fragmentRangesFor(before, edits, later).map((r) => [r.id, r.edit]), [["f-a", 1], ["f-b", 1]]);
  // A gap that is mapped by the caller is found where the mapping sends it.
  assert.equal(fragmentRangesFor(before, [{ from: 4, to: 8, insert: "" }], [{ at: 9, fragments: gaps[0].fragments }], () => 9).length, 2);
});

test("fragmentRangesFor: a gap whose texts do not make the deleted text gets no range", () => {
  const before = "one two three";
  const gaps = [{ at: 4, fragments: [{ id: "f-a", text: "tw", rank: 0 }, { id: "f-b", text: "x ", rank: 1 }] }];
  assert.deepEqual(fragmentRangesFor(before, [{ from: 4, to: 8, insert: "" }], gaps), []);
  // An untracked deletion has no gap at its point.
  assert.deepEqual(fragmentRangesFor(before, [{ from: 0, to: 3, insert: "" }], gaps), []);
});

test("deriveAttachment: an attached, a detached and a missing record, through a real document", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  text.insert(0, "Hello brave world");
  createAnchor(doc, "f1", "c-attached", 12, 17, { exact: "world", prefix: "brave ", suffix: "" });
  createAnchor(doc, "f1", "c-detached", 6, 11, { exact: "brave", prefix: "Hello ", suffix: " world" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  // Cutting "brave" detaches its record, and "world" moves left by five.
  index.apply([{ fileId: "f1", edits: [{ from: 6, to: 11, insert: "" }] }], new MoveMemory());

  const source = { original_anchor: { kind: "source_text" } };
  assert.deepEqual(deriveAttachment(doc, index, { id: "c-attached", ...source }), {
    status: "exact", resolved_range_utf16: [7, 12], resolved_file_id: "f1", tree_digest: "",
  });
  assert.deepEqual(deriveAttachment(doc, index, { id: "c-detached", ...source }), {
    status: "deleted", resolved_range_utf16: null, resolved_file_id: "f1", tree_digest: "",
  });
  assert.deepEqual(deriveAttachment(doc, index, { id: "c-missing", ...source }), {
    status: "unresolved", resolved_range_utf16: null, resolved_file_id: null, tree_digest: "",
  });
  assert.equal(deriveAttachment(doc, index, { id: "c-attached" }), null);
});

test("attachByHand: a deleted comment attaches to the selection and its quote is refreshed", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  text.insert(0, "Hello brave world");
  createAnchor(doc, "f1", "c1", 6, 11, { exact: "brave", prefix: "Hello ", suffix: " world" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  index.apply([{ fileId: "f1", edits: [{ from: 6, to: 11, insert: "" }] }], new MoveMemory());
  const source = { id: "c1", original_anchor: { kind: "source_text" } };
  assert.equal(deriveAttachment(doc, index, source).status, "deleted");

  // "world" is at 7..12 in "Hello  world".
  assert.equal(attachByHand(doc, index, "c1", "f1", 12, 12), false, "an empty selection attaches nothing");
  assert.equal(attachByHand(doc, index, "c1", "f1", 7, 12), true);
  assert.deepEqual(deriveAttachment(doc, index, source), {
    status: "exact", resolved_range_utf16: [7, 12], resolved_file_id: "f1", tree_digest: "",
  });
  const record = doc.getMap("anchors").get("c1");
  assert.equal(record.get("detached"), undefined);
  assert.equal(record.get("exact"), "world");
  assert.equal(record.get("prefix"), "Hello  ");
  assert.equal(record.get("suffix"), "");
  assert.equal(statusOf(doc, resolveAnchors(doc, "f1").find((a) => a.id === "c1")), "exact");
});

test("rejectRestoring: the text a rejection puts back is listed, and restoreByFragments re-pins the anchor", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  const source = "one two three";
  text.insert(0, source);
  createAnchor(doc, "f1", "a1", 4, 7, { exact: "two", prefix: "one ", suffix: " three" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");

  // A tracked deletion of "two ", recorded the way the editor records one.
  const edit = { from: 4, to: 8, insert: "" };
  const [changeId] = trackedSplice(doc, "f1", edit.from, edit.to, edit.insert, { who: "me", author: "Me" });
  doc.commit();
  const fragments = fragmentRangesFor(source, [edit], resolveFile(doc, "f1").gaps);
  assert.equal(fragments.length, 1);
  index.record([{ fileId: "f1", edits: [edit], fragments }], new MoveMemory());
  doc.commit({ origin: ANCHORS_ORIGIN });
  const pending = resolveAnchors(doc, "f1").find((a) => a.id === "a1");
  assert.equal(statusOf(doc, pending), "pending_deletion");

  const rejected = rejectRestoring(doc, changeId);
  assert.equal(rejected.ok, true);
  assert.deepEqual(rejected.restored, [{ file: "f1", fragment: fragments[0].id, at: 4, text: "two " }]);
  assert.equal(text.toString(), source);
  assert.deepEqual(restoreByFragments(doc, "f1", rejected.restored), ["a1"]);
  doc.commit({ origin: ANCHORS_ORIGIN });
  index.load("f1");
  const anchor = resolveAnchors(doc, "f1").find((a) => a.id === "a1");
  assert.equal(statusOf(doc, anchor), "exact");
  assert.deepEqual([anchor.from, anchor.to], [4, 7]);
  assert.deepEqual(rejectRestoring(doc, "no-such-change"), { ok: false, restored: [] });
});
