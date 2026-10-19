// The anchor planner, pure and without a DOM or Loro. The fixtures in
// tests/fixtures/anchors.json pin each rule by hand: a move, a shrink, a
// detach, a paste that re-pins, a paste that does not, a drag either way,
// tracked fragments and surrogate pairs. The seeded fuzzer at the end runs
// random histories of typing, cutting, pasting, copying and dragging against a
// model that tags every unit with the anchor it belongs to. The model applies
// the same memory rule as the planner (each remembered text that occurs exactly
// once in an insertion, newest first, takes its held anchors back unless an
// earlier take of that insertion claimed its span), so the fuzzer checks
// positions, quotes, list membership and the memory size, not the policy itself.
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
  composeEdits,
  createAnchor,
  deriveAttachment,
  fragmentRangesFor,
  planOperation,
  resolveAnchors,
  restoreByFragments,
  statusOf,
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

// The planner's find, then drop. Every model paste re-pins whole anchors, so
// each hit applies and leaves the memory. The planner also keeps an entry whose
// pieces apply nothing. The model has no partial pieces, so it never does.
function takeMemory(model, inserted, free) {
  for (let i = model.memory.length - 1; i >= 0; i--) {
    const at = occurrences(inserted, model.memory[i].text);
    if (at.length === 1 && free(at[0], at[0] + model.memory[i].text.length)) {
      const [entry] = model.memory.splice(i, 1);
      return { entry, at: at[0] };
    }
  }
  return null;
}

// Applies one operation's edits to the model the way the planner does:
// deleted text is remembered with the anchors it takes whole, then each
// insertion takes the remembered texts that occur exactly once in it, newest
// first, and whose occurrence no earlier take of that insertion claimed. The
// held anchors of each take are tagged back in place.
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
    // The spans of this insertion that a take has claimed, as the planner keeps them.
    const claimed = [];
    const free = (from, to) => !claimed.some((c) => from < c.to && to > c.from);
    for (let hit = takeMemory(model, text, free); hit; hit = takeMemory(model, text, free)) {
      claimed.push({ from: hit.at, to: hit.at + hit.entry.text.length });
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

test("fragmentRangesFor: a deletion takes the two created fragments of its text, in order", () => {
  const before = "one two three";
  // The gap of "two " deleted at 4..8 is at 4.
  const fragments = [
    { id: "f-a", text: "tw", at: 4 },
    { id: "f-b", text: "o ", at: 4 },
  ];
  assert.deepEqual(fragmentRangesFor(before, [{ from: 4, to: 8, insert: "" }], fragments), [
    { id: "f-a", edit: 0, start: 0, end: 2 },
    { id: "f-b", edit: 0, start: 2, end: 4 },
  ]);
});

test("fragmentRangesFor: a deletion whose fragments do not make its text takes none and leaves them", () => {
  const before = "one two three";
  const fragments = [
    { id: "f-a", text: "tw", at: 4 },
    { id: "f-b", text: "o ", at: 4 },
  ];
  // "two t" is joined with the "t" after it, so the fragments do not make it.
  assert.deepEqual(fragmentRangesFor(before, [{ from: 4, to: 9, insert: "" }], fragments), []);
  // "one" is not the fragments either. The deletion of "two " after it still takes both.
  // Once "one" is gone, the gap of "two " in " three" is at 1.
  const edits = [
    { from: 0, to: 3, insert: "" },
    { from: 4, to: 8, insert: "" },
  ];
  const moved = fragments.map((f) => ({ ...f, at: 1 }));
  assert.deepEqual(fragmentRangesFor(before, edits, moved), [
    { id: "f-a", edit: 1, start: 0, end: 2 },
    { id: "f-b", edit: 1, start: 2, end: 4 },
  ]);
});

test("fragmentRangesFor: two deletions in one transaction take their fragments one after the other", () => {
  const before = "one two three four";
  // After both deletions the gap of "two " is at 4 and the gap of "four" is at 10.
  const fragments = [
    { id: "f-two", text: "two ", at: 4 },
    { id: "f-four", text: "four", at: 10 },
  ];
  // "four" is at 14..18 in the text before the edits.
  const edits = [
    { from: 4, to: 8, insert: "" },
    { from: 14, to: 18, insert: "" },
  ];
  assert.deepEqual(fragmentRangesFor(before, edits, fragments), [
    { id: "f-two", edit: 0, start: 0, end: 4 },
    { id: "f-four", edit: 1, start: 0, end: 4 },
  ]);
});

test("fragmentRangesFor: a composed second step matches through the running text", () => {
  const before = "one two three";
  // List 1 inserts "XX" at 0. List 2 deletes "two " from "XXone two three" at 6..10.
  const edits = composeEdits([
    [{ from: 0, to: 0, insert: "XX" }],
    [{ from: 6, to: 10, insert: "" }],
  ]);
  // In "XXone three" the gap of the deleted "two " is at 6.
  assert.deepEqual(fragmentRangesFor(before, edits, [{ id: "f-two", text: "two ", at: 6 }]), [
    { id: "f-two", edit: 1, start: 0, end: 4 },
  ]);
});

test("fragmentRangesFor: a later transaction's deletion takes a fragment that sits before an earlier one's", () => {
  const before = "one two three four";
  // List 1 deletes "four" at 14..18. List 2 deletes "two " at 4..8 in the text that results.
  const edits = composeEdits([
    [{ from: 14, to: 18, insert: "" }],
    [{ from: 4, to: 8, insert: "" }],
  ]);
  // After both deletions the text is "one three ". The gap of "four" is at 10 and the gap of "two " at 4.
  const fragments = [
    { id: "f-two", text: "two ", at: 4 },
    { id: "f-four", text: "four", at: 10 },
  ];
  assert.deepEqual(fragmentRangesFor(before, edits, fragments), [
    { id: "f-four", edit: 0, start: 0, end: 4 },
    { id: "f-two", edit: 1, start: 0, end: 4 },
  ]);
});

test("fragmentRangesFor: identical texts deleted in reverse order keep their own fragments", () => {
  const before = "one two three two four";
  // The later "two " goes first. The earlier one is then at 4..8 of the running text.
  const edits = composeEdits([
    [{ from: 14, to: 18, insert: "" }],
    [{ from: 4, to: 8, insert: "" }],
  ]);
  // After both deletions the text is "one three four". The early gap is at 4 and the late one at 10.
  const fragments = [
    { id: "f-early", text: "two ", at: 4 },
    { id: "f-late", text: "two ", at: 10 },
  ];
  assert.deepEqual(fragmentRangesFor(before, edits, fragments), [
    { id: "f-late", edit: 0, start: 0, end: 4 },
    { id: "f-early", edit: 1, start: 0, end: 4 },
  ]);
});

test("fragmentRangesFor: a deletion that joins the author's own pending insertion still finds its fragment", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  text.insert(0, "one two three");
  doc.commit();
  // The author inserts "X" at 4, then deletes "two" at 5..8, which joins that insertion.
  trackedSplice(doc, "f1", 4, 4, "X", { who: "me", author: "Me" });
  doc.commit();
  trackedSplice(doc, "f1", 5, 8, "", { who: "me", author: "Me" });
  doc.commit();
  const created = resolveFile(doc, "f1").gaps.flatMap((gap) =>
    gap.fragments.map((f) => ({ id: f.id, text: f.text, at: gap.at })),
  );
  // The fragment "two" files at gap 4, not at the deletion point 5.
  assert.deepEqual(created.map((f) => [f.text, f.at]), [["two", 4]]);
  assert.deepEqual(fragmentRangesFor("one Xtwo three", [{ from: 5, to: 8, insert: "" }], created), [
    { id: created[0].id, edit: 0, start: 0, end: 3 },
  ]);
});

// An anchor record as planOperation takes it.
const anchorOn = (id, from, to, exact) => ({
  id,
  file: "f1",
  from,
  to,
  detached: false,
  fragment: null,
  offset: null,
  length: null,
  exact,
  prefix: "",
  suffix: "",
  collapsed: false,
  unresolved: false,
});

// Runs each list through planOperation on the output of the list before it.
function runInSequence(before, anchors, lists) {
  let text = before;
  let current = anchors;
  for (const edits of lists) {
    const [file] = planOperation([{ fileId: "f1", before: text, anchors: current, edits }], new MoveMemory()).files;
    text = file.after;
    current = file.anchors;
  }
  return { after: text, anchors: current };
}

test("composeEdits: one planOperation over the composed lists matches running them in sequence", () => {
  const before = "one two three four";
  const lists = [
    // Deletes "two ".
    [{ from: 4, to: 8, insert: "" }],
    // In the coordinates of "one three four": inserts "zero " at the start.
    [{ from: 0, to: 0, insert: "zero " }],
  ];
  const sequential = runInSequence(before, [anchorOn("a1", 8, 13, "three")], lists);
  const [file] = planOperation(
    [{ fileId: "f1", before, anchors: [anchorOn("a1", 8, 13, "three")], edits: composeEdits(lists) }],
    new MoveMemory(),
  ).files;
  assert.equal(file.after, sequential.after);
  assert.equal(file.after, "zero one three four");
  const anchor = file.anchors.find((a) => a.id === "a1");
  const seqAnchor = sequential.anchors.find((a) => a.id === "a1");
  assert.deepEqual([anchor.from, anchor.to], [seqAnchor.from, seqAnchor.to]);
  assert.deepEqual([anchor.from, anchor.to], [9, 14]);
  assert.equal(file.after.slice(anchor.from, anchor.to), "three");
});

test("composeEdits: a later list may edit inside the text an earlier list inserted", () => {
  const before = "one two three";
  const lists = [
    // Inserts "ABCD" at 4: "one ABCDtwo three".
    [{ from: 4, to: 4, insert: "ABCD" }],
    // In that text, deletes "BC" at 5..7.
    [{ from: 5, to: 7, insert: "" }],
  ];
  const plan = planOperation([{ fileId: "f1", before, anchors: [], edits: composeEdits(lists) }], new MoveMemory());
  const [file] = plan.files;
  assert.equal(file.after, "one ADtwo three");
  assert.equal(file.after.slice(4, 6), "AD");
  assert.equal(file.after, runInSequence(before, [], lists).after);
});

test("composeEdits: an insertion before a composed paste moves the paste's re-pin with it", () => {
  const memory = new MoveMemory();
  // "brave" was cut from "Hi there! brave bye.", so its anchor is detached and remembered.
  memory.remember("brave", [{ id: "a1", offset: 0, text: "brave" }]);
  const detached = { ...anchorOn("a1", 0, 0, "brave"), detached: true, length: 5 };
  const lists = [
    // Pastes "brave" at 10, between the two spaces.
    [{ from: 10, to: 10, insert: "brave" }],
    // In the text after the paste, inserts "XX" at 0.
    [{ from: 0, to: 0, insert: "XX" }],
  ];
  const [file] = planOperation(
    [{ fileId: "f1", before: "Hi there!  bye.", anchors: [detached], edits: composeEdits(lists) }],
    memory,
  ).files;
  assert.equal(file.after, "XXHi there! brave bye.");
  const anchor = file.anchors.find((a) => a.id === "a1");
  assert.deepEqual([anchor.from, anchor.to, anchor.detached, anchor.state], [12, 17, false, "set"]);
  assert.equal(file.after.slice(anchor.from, anchor.to), "brave");
  assert.equal(memory.size, 0);
});

// One planOperation over the single file f1, with the memory the caller keeps.
function planOne(before, anchors, edits, memory) {
  const [file] = planOperation([{ fileId: "f1", before, anchors, edits }], memory).files;
  return file;
}

test("the head of a passage cut and put back rejoins it", () => {
  const memory = new MoveMemory();
  const cut = planOne("one two three", [anchorOn("a1", 4, 7, "two")], [{ from: 4, to: 6, insert: "" }], memory);
  // "tw" goes, and the anchor is left on the "o" at 4..5.
  assert.equal(cut.after, "one o three");
  assert.deepEqual([cut.anchors[0].from, cut.anchors[0].to, cut.anchors[0].detached], [4, 5, false]);
  assert.equal(memory.size, 1);

  const back = planOne(cut.after, cut.anchors, [{ from: 4, to: 4, insert: "tw" }], memory);
  assert.equal(back.after, "one two three");
  const anchor = back.anchors[0];
  assert.deepEqual([anchor.from, anchor.to, anchor.detached, anchor.state], [4, 7, false, "set"]);
  assert.equal(back.after.slice(anchor.from, anchor.to), "two");
  assert.equal(memory.size, 0);
});

test("the tail of a passage cut and put back rejoins it", () => {
  const memory = new MoveMemory();
  const cut = planOne("one two three", [anchorOn("a1", 4, 7, "two")], [{ from: 5, to: 7, insert: "" }], memory);
  // "wo" goes, and the anchor is left on the "t" at 4..5.
  assert.equal(cut.after, "one t three");
  assert.deepEqual([cut.anchors[0].from, cut.anchors[0].to, cut.anchors[0].detached], [4, 5, false]);

  const back = planOne(cut.after, cut.anchors, [{ from: 5, to: 5, insert: "wo" }], memory);
  assert.equal(back.after, "one two three");
  const anchor = back.anchors[0];
  assert.deepEqual([anchor.from, anchor.to, anchor.detached, anchor.state], [4, 7, false, "set"]);
  assert.equal(memory.size, 0);
});

test("a cut head pasted elsewhere does not extend the passage", () => {
  const memory = new MoveMemory();
  const cut = planOne("one two three", [anchorOn("a1", 4, 7, "two")], [{ from: 4, to: 6, insert: "" }], memory);
  // "tw" goes back in at the start, away from the edge the anchor was cut at.
  const pasted = planOne(cut.after, cut.anchors, [{ from: 0, to: 0, insert: "tw" }], memory);
  assert.equal(pasted.after, "twone o three");
  const anchor = pasted.anchors[0];
  assert.deepEqual([anchor.from, anchor.to, anchor.detached], [6, 7, false]);
  assert.equal(pasted.after.slice(anchor.from, anchor.to), "o");
});

test("a remembered piece pasted elsewhere stays remembered and rejoins later", () => {
  const memory = new MoveMemory();
  const cut = planOne("one two three", [anchorOn("a1", 4, 7, "two")], [{ from: 4, to: 6, insert: "" }], memory);
  assert.equal(cut.after, "one o three");
  // "tw" goes back in at the start. It does not rejoin, so it stays remembered.
  const pasted = planOne(cut.after, cut.anchors, [{ from: 0, to: 0, insert: "tw" }], memory);
  assert.equal(pasted.after, "twone o three");
  assert.deepEqual([pasted.anchors[0].from, pasted.anchors[0].to, pasted.anchors[0].detached], [6, 7, false]);
  assert.equal(memory.size, 1);

  // "tw" comes back at the edge it was cut from, and the anchor rejoins.
  const back = planOne(pasted.after, pasted.anchors, [{ from: 6, to: 6, insert: "tw" }], memory);
  assert.equal(back.after, "twone two three");
  const anchor = back.anchors[0];
  assert.deepEqual([anchor.from, anchor.to, anchor.detached, anchor.state], [6, 9, false, "set"]);
  assert.equal(back.after.slice(anchor.from, anchor.to), "two");
  assert.equal(memory.size, 0);
});

test("a merged undo of two backspaces rejoins both pieces", () => {
  const memory = new MoveMemory();
  const first = planOne("one two three", [anchorOn("a1", 4, 7, "two")], [{ from: 4, to: 5, insert: "" }], memory);
  // "t" goes, and the anchor is left on "wo" at 4..6.
  assert.equal(first.after, "one wo three");
  const second = planOne(first.after, first.anchors, [{ from: 4, to: 5, insert: "" }], memory);
  // "w" goes too, and the anchor is left on the "o" at 4..5.
  assert.equal(second.after, "one o three");
  assert.deepEqual([second.anchors[0].from, second.anchors[0].to], [4, 5]);
  assert.equal(memory.size, 2);

  // One undo step puts both pieces back as one insertion.
  const back = planOne(second.after, second.anchors, [{ from: 4, to: 4, insert: "tw" }], memory);
  assert.equal(back.after, "one two three");
  const anchor = back.anchors[0];
  assert.deepEqual([anchor.from, anchor.to, anchor.detached, anchor.state], [4, 7, false, "set"]);
  assert.equal(back.after.slice(anchor.from, anchor.to), "two");
  assert.equal(memory.size, 0);
});

test("two cuts of the same word and one paste attach one comment", () => {
  const memory = new MoveMemory();
  const anchors = [anchorOn("a", 4, 7, "two"), anchorOn("b", 14, 17, "two")];
  // The later "two" is cut first, so the earlier one is the newest entry.
  const cutLater = planOne("one two three two four", anchors, [{ from: 14, to: 17, insert: "" }], memory);
  const cutEarlier = planOne(cutLater.after, cutLater.anchors, [{ from: 4, to: 7, insert: "" }], memory);
  assert.equal(memory.size, 2);
  // One "two" comes back at the end. The newest entry takes it, and the older one finds it claimed.
  const end = cutEarlier.after.length;
  const pasted = planOne(cutEarlier.after, cutEarlier.anchors, [{ from: end, to: end, insert: "two" }], memory);
  const a = pasted.anchors.find((x) => x.id === "a");
  const b = pasted.anchors.find((x) => x.id === "b");
  assert.deepEqual([a.from, a.to, a.detached], [end, end + 3, false]);
  assert.equal(pasted.after.slice(a.from, a.to), "two");
  assert.equal(b.detached, true);
  assert.equal(memory.size, 1);
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
    status: "exact", resolved_range_utf16: [7, 12], resolved_file_id: "f1", tree_digest: "", attached_by_hand: false, diagnostic: null,
  });
  assert.deepEqual(deriveAttachment(doc, index, { id: "c-detached", ...source }), {
    status: "deleted", resolved_range_utf16: null, resolved_file_id: "f1", tree_digest: "", attached_by_hand: false, diagnostic: null,
  });
  assert.deepEqual(deriveAttachment(doc, index, { id: "c-missing", ...source }), {
    status: "unresolved", resolved_range_utf16: null, resolved_file_id: null, tree_digest: "", attached_by_hand: false, diagnostic: null,
  });
  assert.equal(deriveAttachment(doc, index, { id: "c-attached" }), null);
});

test("deriveAttachment: a comment whose file was removed is unresolved, and the index keeps its anchor", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  text.insert(0, "Hello brave world");
  createAnchor(doc, "f1", "c1", 6, 11, { exact: "brave", prefix: "Hello ", suffix: " world" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  doc.getMap("files").delete("f1");
  doc.commit();

  const source = { id: "c1", original_anchor: { kind: "source_text" } };
  assert.deepEqual(deriveAttachment(doc, index, source), {
    status: "unresolved", resolved_range_utf16: null, resolved_file_id: null, tree_digest: "", attached_by_hand: false, diagnostic: "removed_file",
  });
  // The cache is not told of the removal, so the record is still there.
  const hit = index.find("c1");
  assert.equal(hit.fileId, "f1");
  assert.deepEqual([hit.anchor.from, hit.anchor.to, hit.anchor.exact], [6, 11, "brave"]);
});

test("resolveAnchors: a record that names another text is unresolved, since its cursors belong to that text", () => {
  const doc = new LoroDoc();
  doc.getMap("files").setContainer("f1", new LoroText()).insert(0, "one two three");
  doc.getMap("files").setContainer("f2", new LoroText()).insert(0, "alpha beta gamma");
  createAnchor(doc, "f2", "c1", 6, 10, { exact: "beta", prefix: "alpha ", suffix: " gamma" });
  doc.commit();
  // The cursors still belong to f2. Only the file key names f1.
  doc.getMap("anchors").get("c1").set("file", "f1");
  doc.commit();

  const [anchor] = resolveAnchors(doc, "f1");
  assert.equal(anchor.id, "c1");
  assert.equal(anchor.unresolved, true);
  assert.equal(statusOf(doc, anchor), "unresolved");
  assert.deepEqual(resolveAnchors(doc, "f2"), []);
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
  assert.equal(
    attachByHand(doc, index, new MoveMemory(), "c1", "f1", 12, 12),
    false,
    "an empty selection attaches nothing",
  );
  assert.equal(attachByHand(doc, index, new MoveMemory(), "c1", "f1", 7, 12), true);
  assert.deepEqual(deriveAttachment(doc, index, source), {
    status: "exact", resolved_range_utf16: [7, 12], resolved_file_id: "f1", tree_digest: "", attached_by_hand: false, diagnostic: null,
  });
  const record = doc.getMap("anchors").get("c1");
  assert.equal(record.get("detached"), undefined);
  assert.equal(record.get("exact"), "world");
  assert.equal(record.get("prefix"), "Hello  ");
  assert.equal(record.get("suffix"), "");
  assert.equal(statusOf(doc, resolveAnchors(doc, "f1").find((a) => a.id === "c1")), "exact");
});

test("attaching by hand forgets the comment's remembered pieces", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  text.insert(0, "Hello brave world");
  createAnchor(doc, "f1", "c1", 6, 11, { exact: "brave", prefix: "Hello ", suffix: " world" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  const memory = new MoveMemory();
  // Cutting "brave" detaches the comment, and the cut is remembered.
  index.apply([{ fileId: "f1", edits: [{ from: 6, to: 11, insert: "" }] }], memory);
  assert.equal(memory.size, 1);

  // "world" is at 7..12 in "Hello  world".
  assert.equal(attachByHand(doc, index, memory, "c1", "f1", 7, 12), true);
  assert.equal(memory.size, 0, "the comment's remembered pieces are forgotten");

  // A paste of "brave" at the end leaves the comment on "world".
  index.apply([{ fileId: "f1", edits: [{ from: 12, to: 12, insert: "brave" }] }], memory);
  const hit = index.find("c1");
  assert.equal(hit.fileId, "f1");
  assert.deepEqual([hit.anchor.from, hit.anchor.to, hit.anchor.detached], [7, 12, false]);
  assert.equal(statusOf(doc, hit.anchor), "exact");
});

test("deriveAttachment: a comment attached by hand says so, and one left where it was does not", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  text.insert(0, "Hello brave world");
  createAnchor(doc, "f1", "c-kept", 6, 11, { exact: "brave", prefix: "Hello ", suffix: " world" });
  createAnchor(doc, "f1", "c-moved", 6, 11, { exact: "brave", prefix: "Hello ", suffix: " world" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  // Both comments were made on "brave". Only "c-moved" is attached to "world" by hand.
  assert.equal(attachByHand(doc, index, new MoveMemory(), "c-moved", "f1", 12, 17), true);
  const original_anchor = { kind: "source_text", target: { exact: "brave" } };
  assert.equal(deriveAttachment(doc, index, { id: "c-kept", original_anchor }).attached_by_hand, false);
  assert.equal(deriveAttachment(doc, index, { id: "c-moved", original_anchor }).attached_by_hand, true);
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
  const created = resolveFile(doc, "f1").gaps.flatMap((gap) =>
    gap.fragments.map((f) => ({ id: f.id, text: f.text, at: gap.at })),
  );
  const fragments = fragmentRangesFor(source, [edit], created);
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
  assert.deepEqual(rejectRestoring(doc, "no-such-change"), { ok: false, restored: [], edits: [] });
});

test("rejecting a tracked deletion that cut the head of a passage rejoins it", () => {
  const doc = new LoroDoc();
  const text = doc.getMap("files").setContainer("f1", new LoroText());
  const source = "one two three";
  text.insert(0, source);
  createAnchor(doc, "f1", "a1", 4, 7, { exact: "two", prefix: "one ", suffix: " three" });
  doc.commit();
  const index = new AnchorIndex(doc);
  index.load("f1");
  const memory = new MoveMemory();

  // A tracked deletion of "tw", recorded the way the editor records one.
  const deletion = { from: 4, to: 6, insert: "" };
  const [changeId] = trackedSplice(doc, "f1", deletion.from, deletion.to, deletion.insert, { who: "me", author: "Me" });
  doc.commit();
  const created = resolveFile(doc, "f1").gaps.flatMap((gap) =>
    gap.fragments.map((f) => ({ id: f.id, text: f.text, at: gap.at })),
  );
  const fragments = fragmentRangesFor(source, [deletion], created);
  index.record([{ fileId: "f1", edits: [deletion], fragments }], memory);
  doc.commit({ origin: ANCHORS_ORIGIN });
  const cut = resolveAnchors(doc, "f1").find((a) => a.id === "a1");
  assert.equal(statusOf(doc, cut), "modified");
  assert.deepEqual([cut.from, cut.to], [4, 5]);

  // The rejection puts "tw" back, and the reader records that as one edit.
  const { ok, restored, edits } = rejectRestoring(doc, changeId);
  assert.equal(ok, true);
  assert.deepEqual(edits, [{ from: 4, to: 4, insert: "tw" }]);
  assert.equal(restored.length, 1);
  index.record([{ fileId: "f1", edits: composeEdits(edits.map((edit) => [edit])), fragments: [] }], memory);
  assert.deepEqual(restoreByFragments(doc, "f1", restored), []);
  doc.commit({ origin: ANCHORS_ORIGIN });
  index.load("f1");
  assert.equal(text.toString(), source);
  const anchor = resolveAnchors(doc, "f1").find((a) => a.id === "a1");
  assert.equal(statusOf(doc, anchor), "exact");
  assert.deepEqual([anchor.from, anchor.to], [4, 7]);
});

test("an entry whose comment is gone does not shadow an older one with the same text", () => {
  const memory = new MoveMemory();
  const before = "one two three two four";
  const one = planOperation([{ fileId: "f1", before, anchors: [anchorOn("a", 4, 7, "two"), anchorOn("b", 14, 17, "two")], edits: [{ from: 14, to: 17, insert: "" }] }], memory).files[0];
  const two = planOperation([{ fileId: "f1", before: one.after, anchors: one.anchors, edits: [{ from: 4, to: 7, insert: "" }] }], memory).files[0];
  assert.equal(memory.size, 2);
  // Comment "a" was deleted since, so the paste sees only "b". The newer
  // entry, which held "a", is passed over and the older one applies.
  const paste = planOperation([{ fileId: "f1", before: two.after, anchors: two.anchors.filter((a) => a.id === "b"), edits: [{ from: 16, to: 16, insert: "two" }] }], memory).files[0];
  const b = paste.anchors.find((a) => a.id === "b");
  assert.equal(b.detached, false);
  assert.deepEqual([b.from, b.to], [16, 19]);
  assert.equal(memory.size, 1, "the entry that held a stays, the entry of b is gone");
});
