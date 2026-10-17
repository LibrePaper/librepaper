// The tracked-change planner and its Loro records.
//
// The shared fixtures pin the planner; the Rust port runs the same file. The
// Loro tests pin what the planner cannot: that cursors resolve back to the
// planned offsets, that a decision restores the right text in any order, that
// a remote peer typing beside a pending insertion does not grow it, and that
// nested deletions come back in every order, and that records of the earlier
// model convert to gaps that decide in any order. The seeded fuzz at the end
// checks random histories against their oracles.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Cursor, LoroDoc, LoroText } from "loro-crdt";
import {
  CHANGES,
  accept,
  listChanges,
  planEdits,
  prune,
  recordEdits,
  reject,
  rejectMany,
  resolveChanges,
  resolveFile,
  trackedSplice,
  untrackedSplice,
  writePlanned,
} from "../../src/lib/track-changes.js";

const fixtures = JSON.parse(
  readFileSync(new URL("../fixtures/track-changes.json", import.meta.url), "utf8"),
);

const ME = { who: "me", author: "Me" };
const BOB = { who: "bob", author: "Bob" };
const ALICE = { who: "alice", author: "Alice" };

const byPlace = (a, b) => a.from - b.from || a.to - b.to || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
const byAt = (a, b) => a.at - b.at;
const byJson = (a, b) => (JSON.stringify(a) < JSON.stringify(b) ? -1 : 1);

function applyEdits(text, edits) {
  let out = text;
  let adj = 0;
  for (const e of edits) {
    out = out.slice(0, e.from + adj) + e.insert + out.slice(e.to + adj);
    adj += e.insert.length - (e.to - e.from);
  }
  return out;
}

// The generator the fuzz scripts were checked with, so a failing seed replays
// the same case.
function random(seed) {
  let state = seed;
  const rnd = () => {
    state = (state * 1103515245 + 12345) & 0x7fffffff;
    return state / 0x7fffffff;
  };
  const pick = (n) => Math.floor(rnd() * n);
  const shuffle = (xs) => {
    const a = [...xs];
    for (let i = a.length - 1; i > 0; i--) {
      const j = pick(i + 1);
      [a[i], a[j]] = [a[j], a[i]];
    }
    return a;
  };
  return { rnd, pick, shuffle };
}

// ---------------------------------------------------------------------------
// Fixtures through the planner

for (const f of fixtures) {
  assert.equal(applyEdits(f.before, f.edits), f.after, `${f.name}: after text`);
  let n = 0;
  const state = {
    changes: f.state.changes.map((c) => ({ ...c, author: c.who, run: null, at: 1 })),
    gaps: f.state.gaps.map((g, i) => ({ ...g, id: "gap-" + i })),
  };
  const planned = planEdits(f.before, state, f.edits, f.me, { now: 2, mintId: () => "new-" + ++n, untracked: Boolean(f.untracked) });
  const changes = planned.changes.filter((c) => c.state !== "removed").sort(byPlace);
  const gaps = planned.gaps.filter((g) => g.state !== "removed").sort(byAt);
  assert.deepEqual(
    changes.map(({ id, who, from, to }) => ({ id, who, from, to })),
    f.expect.changes.slice().sort(byPlace),
    f.name,
  );
  assert.deepEqual(
    gaps.map(({ at, fragments }) => ({ at, fragments })),
    f.expect.gaps.slice().sort(byAt),
    f.name,
  );
}

// Unchanged records are kept, edited ones are set, dropped ones are removed.
{
  const state = {
    changes: [
      { id: "a", who: "me", author: "Me", run: null, at: 1, from: 2, to: 4 },
      { id: "b", who: "bob", author: "Bob", run: null, at: 1, from: 10, to: 12 },
    ],
    gaps: [],
  };
  const planned = planEdits("0123456789abcdef", state, [{ from: 4, to: 4, insert: "Z" }], ME, {
    now: 5,
    mintId: () => "new-1",
  });
  const states = Object.fromEntries(planned.changes.map((c) => [c.id, c.state]));
  assert.deepEqual(states, { a: "set", b: "kept" });
  assert.equal(planned.changes.find((c) => c.id === "a").at, 5);
  // A span deleted whole is removed.
  const gone = planEdits(
    "abXYcd",
    { changes: [{ id: "a", who: "me", author: "Me", run: null, at: 1, from: 2, to: 4 }], gaps: [] },
    [{ from: 2, to: 4, insert: "" }],
    ME,
    { now: 5, mintId: () => "new-1" },
  );
  assert.deepEqual(gone.changes.map((c) => [c.id, c.state]), [["a", "removed"]]);
  // The run tag lands on new changes.
  const tagged = planEdits("", { changes: [], gaps: [] }, [{ from: 0, to: 0, insert: "x" }], ME, {
    run: "pass-1",
    now: 5,
    mintId: () => "new-1",
  });
  assert.equal(tagged.changes[0].run, "pass-1");
}

// ---------------------------------------------------------------------------
// Loro helpers

function makeDoc(text, peer) {
  const doc = new LoroDoc();
  if (peer !== undefined) doc.setPeerId(peer);
  const body = doc.getMap("files").setContainer("f1", new LoroText());
  if (text) body.insert(0, text);
  doc.commit();
  return doc;
}

function textOf(doc) {
  return doc.getMap("files").get("f1").toString();
}

function track(doc, from, to, insert, me = ME, opts) {
  const ids = trackedSplice(doc, "f1", from, to, insert, me, opts);
  doc.commit();
  return ids;
}

// ---------------------------------------------------------------------------
// recordEdits then resolveChanges agrees with the fixtures

// The deleted text a fixture gives one change: its fragments in document order.
const fixtureDeleted = (f, id) =>
  f.expect.gaps
    .slice()
    .sort(byAt)
    .flatMap((g) => g.fragments)
    .filter((x) => x.change === id)
    .map((x) => x.text)
    .join("");

for (const f of fixtures.filter((x) => x.state.changes.length === 0 && x.state.gaps.length === 0)) {
  const doc = makeDoc(f.before);
  const text = doc.getMap("files").get("f1");
  let adj = 0;
  for (const e of f.edits) {
    if (e.to > e.from) text.delete(e.from + adj, e.to - e.from);
    if (e.insert) text.insert(e.from + adj, e.insert);
    adj += e.insert.length - (e.to - e.from);
  }
  assert.equal(text.toString(), f.after, f.name);
  recordEdits(doc, "f1", f.before, { changes: [], gaps: [] }, f.edits, f.me, { now: 3 });
  doc.commit();
  const rows = resolveChanges(doc, "f1")
    .map(({ who, from, to, deleted }) => ({ who, from, to, deleted }))
    .sort(byJson);
  const expected = f.expect.changes
    .map((c) => ({ who: c.who, from: c.from, to: c.to, deleted: fixtureDeleted(f, c.id) }))
    .sort(byJson);
  assert.deepEqual(rows, expected, `round trip: ${f.name}`);
}

// Typing character by character stays one change.
{
  const doc = makeDoc("hello world");
  track(doc, 5, 5, "A");
  track(doc, 6, 6, "B");
  track(doc, 7, 7, "C");
  const list = resolveChanges(doc, "f1");
  assert.equal(list.length, 1);
  assert.deepEqual([list[0].from, list[0].to, list[0].deleted], [5, 8, ""]);
  assert.equal(textOf(doc), "helloABC world");
}

// A replacement's row has its span and its deleted text.
{
  const doc = makeDoc("abcd");
  track(doc, 1, 3, "Z");
  assert.deepEqual(
    resolveChanges(doc, "f1").map(({ from, to, deleted }) => ({ from, to, deleted })),
    [{ from: 1, to: 2, deleted: "bc" }],
  );
}

// listChanges carries the inserted text across files.
{
  const doc = makeDoc("hello world");
  doc.getMap("files").setContainer("f2", new LoroText()).insert(0, "second");
  doc.commit();
  track(doc, 0, 5, "howdy");
  trackedSplice(doc, "f2", 6, 6, "!", ME);
  doc.commit();
  const all = listChanges(doc);
  assert.equal(all.length, 2);
  const first = all.find((c) => c.file === "f1");
  assert.deepEqual([first.inserted, first.deleted], ["howdy", "hello"]);
  const second = all.find((c) => c.file === "f2");
  assert.deepEqual([second.inserted, second.deleted], ["!", ""]);
  assert.equal(resolveChanges(doc, "f1").length, 1);
}

// ---------------------------------------------------------------------------
// Decisions

// Accept keeps the text and drops the record.
{
  const doc = makeDoc("abcd");
  const [id] = track(doc, 1, 3, "Z");
  assert.equal(textOf(doc), "aZd");
  assert.equal(accept(doc, id), true);
  assert.equal(textOf(doc), "aZd");
  assert.equal(doc.getMap(CHANGES).get(id), undefined);
  assert.equal(accept(doc, id), false);
  assert.equal(reject(doc, id), false);
}

// Reject restores a replacement, an insertion and a pure deletion.
for (const [original, from, to, insert] of [
  ["abcd", 1, 3, "Z"],
  ["ab", 1, 1, "XY"],
  ["abcd", 1, 3, ""],
  ["a😀b", 1, 3, "Q"],
]) {
  const doc = makeDoc(original);
  const [id] = track(doc, from, to, insert);
  assert.notEqual(textOf(doc), original);
  assert.equal(reject(doc, id), true);
  assert.equal(textOf(doc), original, `reject ${original} ${from} ${to} ${insert}`);
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// Two changes decided in either order.
for (const order of [0, 1]) {
  const doc = makeDoc("one two three");
  const [a] = track(doc, 0, 3, "1");
  const [b] = track(doc, 6, 11, "3");
  assert.equal(textOf(doc), "1 two 3");
  const ids = order === 0 ? [a, b] : [b, a];
  for (const id of ids) assert.equal(reject(doc, id), true);
  assert.equal(textOf(doc), "one two three");
  assert.equal(doc.getMap(CHANGES).size, 0);
}
{
  const doc = makeDoc("one two three");
  const [a] = track(doc, 0, 3, "1");
  const [b] = track(doc, 6, 11, "3");
  accept(doc, a);
  reject(doc, b);
  assert.equal(textOf(doc), "1 two three");
}

// Rejecting another author's change works the same.
{
  const doc = makeDoc("abcd");
  const [id] = track(doc, 1, 3, "Z", BOB);
  assert.equal(resolveChanges(doc, "f1")[0].who, "bob");
  reject(doc, id);
  assert.equal(textOf(doc), "abcd");
}

// rejectMany rejects several changes in one commit, in either order.
for (const bobFirst of [true, false]) {
  const doc = makeDoc("abcde");
  const [a] = track(doc, 2, 3, "", ME);
  const b = track(doc, 1, 3, "Z", BOB).find((id) => id !== a);
  assert.equal(textOf(doc), "aZe");
  assert.equal(rejectMany(doc, bobFirst ? [b, a] : [a, b]), 2);
  assert.equal(textOf(doc), "abcde", `rejectMany: bobFirst=${bobFirst}`);
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// Rejecting one change leaves the other's text and its record in place.
{
  const doc = makeDoc("abcde");
  const [a] = track(doc, 2, 3, "", ME);
  track(doc, 1, 3, "Z", BOB);
  assert.equal(rejectMany(doc, [a]), 1);
  assert.equal(textOf(doc), "acZe");
  assert.equal(resolveChanges(doc, "f1").length, 1);
  assert.equal(rejectMany(doc, ["missing"]), 0);
}

// ---------------------------------------------------------------------------
// A remote peer typing beside a pending insertion does not grow it

{
  const a = makeDoc("hello world", 1);
  track(a, 5, 5, "XY");
  const b = new LoroDoc();
  b.setPeerId(2);
  b.import(a.export({ mode: "snapshot" }));
  const bt = b.getMap("files").get("f1");
  bt.insert(5, "a");
  bt.insert(8, "b");
  b.commit();
  assert.equal(bt.toString(), "helloaXYb world");
  a.import(b.export({ mode: "update" }));
  assert.equal(textOf(a), "helloaXYb world");
  const list = resolveChanges(a, "f1");
  assert.equal(list.length, 1);
  assert.deepEqual([list[0].from, list[0].to], [6, 8]);
  // The record syncs too.
  assert.equal(resolveChanges(b, "f1").length, 1);
  assert.equal(reject(b, list[0].id), true);
  assert.equal(textOf(b), "helloab world");
}

// A peer deletes the last character of a pending insertion; prune heals it.
{
  const a = makeDoc("hello world", 1);
  track(a, 5, 5, "XYZ");
  const b = new LoroDoc();
  b.setPeerId(2);
  b.import(a.export({ mode: "snapshot" }));
  b.getMap("files").get("f1").delete(7, 1);
  b.commit();
  a.import(b.export({ mode: "update" }));
  assert.equal(textOf(a), "helloXY world");
  const list = resolveChanges(a, "f1");
  assert.equal(list.length, 1);
  assert.deepEqual([list[0].from, list[0].to], [5, 7]);
  assert.equal(prune(a), 0);
  a.commit();
  const entry = a.getMap(CHANGES).get(list[0].id);
  const end = a.getCursorPos(Cursor.decode(entry.end));
  assert.equal(end.update, undefined);
  assert.deepEqual(resolveChanges(a, "f1").map((c) => [c.from, c.to]), [[5, 7]]);
}

// ---------------------------------------------------------------------------
// prune

{
  const doc = makeDoc("abcd");
  const [keep] = track(doc, 1, 2, "Z");
  const map = doc.getMap(CHANGES);
  const live = map.get(keep);
  // Empty: a change with no insertion and no fragment.
  const text = doc.getMap("files").get("f1");
  map.set("empty", { ...live, start: text.getCursor(0, -1).encode(), end: null });
  // Orphaned: the file is gone.
  map.set("orphan", { ...live, file: "missing" });
  doc.commit();
  assert.equal(resolveChanges(doc, "f1").length, 1);
  assert.equal(listChanges(doc).length, 1);
  assert.equal(prune(doc), 2);
  doc.commit();
  // The change and its gap are left.
  assert.equal(map.size, 2);
  assert.notEqual(map.get(keep), undefined);
  assert.equal(prune(doc), 0);
}

// Accept and reject drop an orphan, and its gap once prune has run, without
// touching text.
for (const decide of [accept, reject]) {
  const doc = makeDoc("abcd");
  const [id] = track(doc, 1, 2, "Z");
  const map = doc.getMap(CHANGES);
  for (const [key, value] of [...map.entries()]) map.set(key, { ...value, file: "missing" });
  doc.commit();
  assert.equal(decide(doc, id), true);
  assert.equal(textOf(doc), "aZcd");
  assert.equal(map.get(id), undefined);
  prune(doc);
  doc.commit();
  assert.equal(map.size, 0);
}

// prune converts a record written in the earlier model.
{
  const doc = makeDoc("abcd");
  const text = doc.getMap("files").get("f1");
  text.delete(1, 2);
  text.insert(1, "Z");
  doc.getMap(CHANGES).set("old", {
    file: "f1",
    author: "Me",
    who: "me",
    at: 1,
    run: null,
    start: text.getCursor(1, -1).encode(),
    end: text.getCursor(1, 1).encode(),
    deleted: "b",
  });
  doc.commit();
  assert.deepEqual(resolveChanges(doc, "f1"), []);
  assert.equal(prune(doc), 0);
  doc.commit();
  const kinds = [...doc.getMap(CHANGES).entries()].map(([, v]) => v.kind).sort();
  assert.deepEqual(kinds, ["change", "gap"]);
  assert.deepEqual(resolveChanges(doc, "f1"), [
    { id: "old", who: "me", author: "Me", run: null, at: 1, from: 1, to: 2, deleted: "b" },
  ]);
}

// ---------------------------------------------------------------------------
// Astral characters at the end of an insertion

{
  const doc = makeDoc("ab");
  const [id] = track(doc, 1, 1, "x😀");
  assert.equal(textOf(doc), "ax😀b");
  const list = resolveChanges(doc, "f1");
  assert.deepEqual([list[0].from, list[0].to], [1, 4]);
  assert.equal(reject(doc, id), true);
  assert.equal(textOf(doc), "ab");
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// A change written the way Rust writes it: the end cursor sits on the emoji's
// first unit, side 1.
{
  const doc = makeDoc("a😀b");
  const text = doc.getMap("files").get("f1");
  doc.getMap(CHANGES).set("rust", {
    kind: "change",
    file: "f1",
    author: "Me",
    who: "me",
    at: 1,
    run: null,
    start: text.getCursor(1, -1).encode(),
    end: text.getCursor(1, 1).encode(),
  });
  doc.commit();
  const list = resolveChanges(doc, "f1");
  assert.deepEqual([list[0].from, list[0].to], [1, 3]);
  assert.equal(reject(doc, "rust"), true);
  assert.equal(textOf(doc), "ab");
}

// writePlanned writes a plan and deletes removed ids.
{
  const doc = makeDoc("abcd");
  const [old] = track(doc, 1, 2, "Z");
  const pre = resolveFile(doc, "f1");
  const text = doc.getMap("files").get("f1");
  text.delete(1, 1);
  const plan = planEdits("aZcd", pre, [{ from: 1, to: 2, insert: "" }], ME, { now: 9, mintId: () => "n" });
  const ids = writePlanned(doc, "f1", plan);
  doc.commit();
  assert.deepEqual(ids, [old]);
  assert.equal(resolveChanges(doc, "f1")[0].deleted, "b");
}

// ---------------------------------------------------------------------------
// Nested deletions, every order

function permutations(xs) {
  return xs.length <= 1
    ? [xs]
    : xs.flatMap((x, i) => permutations([...xs.slice(0, i), ...xs.slice(i + 1)]).map((p) => [x, ...p]));
}

// Each step is [from, to, insert] by the next author in turn.
function buildNested(start, steps) {
  const doc = makeDoc(start);
  const ids = [];
  steps.forEach(([from, to, insert = ""], i) => {
    const name = String.fromCharCode(97 + i);
    const got = trackedSplice(doc, "f1", from, to, insert, { who: name, author: name.toUpperCase() });
    doc.commit();
    ids.push(got.find((id) => !ids.includes(id)));
  });
  return { doc, ids };
}

const nested = [
  ["abcde", [[2, 3], [1, 3], [0, 1]]],
  ["abcde", [[2, 3, "X"], [1, 4], [0, 1]]],
  ["abcdefg", [[3, 4], [2, 5], [1, 3], [0, 1]]],
  ["abcdefgh", [[3, 4], [2, 4], [1, 3], [0, 2]]],
  ["abcdefgh", [[3, 4], [2, 4], [0, 5]]],
  ["abcdefg", [[1, 2], [4, 5], [0, 5]]],
  ["abcdefgh", [[1, 2], [4, 5], [0, 5], [0, 1]]],
];

for (const [start, steps] of nested) {
  const label = `${start} ${JSON.stringify(steps)}`;
  const orders = permutations(steps.map((_, i) => i));
  for (const order of orders) {
    const { doc, ids } = buildNested(start, steps);
    for (const k of order) if (doc.getMap(CHANGES).get(ids[k])) reject(doc, ids[k]);
    for (const id of [...doc.getMap(CHANGES).keys()]) reject(doc, id);
    assert.equal(textOf(doc), start, `${label}: one at a time in ${order}`);
    assert.equal(doc.getMap(CHANGES).size, 0, `${label}: no record left, one at a time in ${order}`);
  }
  for (const order of orders) {
    const { doc, ids } = buildNested(start, steps);
    rejectMany(doc, order.map((k) => ids[k]));
    assert.equal(textOf(doc), start, `${label}: rejectMany in ${order}`);
    assert.equal(doc.getMap(CHANGES).size, 0, `${label}: no record left after rejectMany in ${order}`);
  }
}

// Accept the middle change, reject the other two: what the middle one kept.
{
  const { doc, ids } = buildNested("abcde", [[2, 3], [1, 3], [0, 1]]);
  assert.equal(accept(doc, ids[1]), true);
  assert.equal(reject(doc, ids[0]), true);
  assert.equal(reject(doc, ids[2]), true);
  assert.equal(textOf(doc), "ace");
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// ---------------------------------------------------------------------------
// Records of the earlier model

// "abcde" reduced to "ae": Alice deleted c, then Bob deleted bd around it. Both
// anchor on the e, and Bob's deleted text lists Alice's point at offset 1.
function oldAliceBob() {
  const doc = makeDoc("ae");
  const text = doc.getMap("files").get("f1");
  const start = text.getCursor(1, -1).encode();
  const map = doc.getMap(CHANGES);
  map.set("alice", { file: "f1", who: "alice", author: "Alice", at: 1, run: null, start, end: null, deleted: "c" });
  map.set("bob", { file: "f1", who: "bob", author: "Bob", at: 2, run: null, start, end: null, deleted: "bd", points: [{ id: "alice", at: 1 }] });
  doc.commit();
  return doc;
}

// Cara later deleted the a. Her deleted text lists Bob and Alice at offset 1,
// but Alice is Bob's point, so she reaches Cara's gap through Bob.
function oldCara() {
  const doc = oldAliceBob();
  const text = doc.getMap("files").get("f1");
  text.delete(0, 1);
  doc.getMap(CHANGES).set("cara", { file: "f1", who: "cara", author: "Cara", at: 3, run: null, start: text.getCursor(0, -1).encode(), end: null, deleted: "a", points: [{ id: "bob", at: 1 }, { id: "alice", at: 1 }] });
  doc.commit();
  return doc;
}

{
  const doc = oldAliceBob();
  assert.equal(prune(doc), 0);
  doc.commit();
  const entries = [...doc.getMap(CHANGES).entries()];
  assert.equal(entries.filter(([, v]) => v.kind === "change").length, 2);
  const gaps = entries.filter(([, v]) => v.kind === "gap");
  assert.equal(gaps.length, 1);
  assert.deepEqual(gaps[0][1].fragments, [
    { change: "bob", text: "b" },
    { change: "alice", text: "c" },
    { change: "bob", text: "d" },
  ]);
  for (const order of [["bob", "alice"], ["alice", "bob"]]) {
    const copy = oldAliceBob();
    prune(copy);
    copy.commit();
    for (const id of order) assert.equal(reject(copy, id), true);
    assert.equal(textOf(copy), "abcde", `old records, ${order}`);
    assert.equal(copy.getMap(CHANGES).size, 0);
  }
}

{
  const doc = oldCara();
  assert.equal(prune(doc), 0);
  doc.commit();
  const entries = [...doc.getMap(CHANGES).entries()];
  assert.equal(entries.filter(([, v]) => v.kind === "change").length, 3);
  const gaps = entries.filter(([, v]) => v.kind === "gap");
  assert.equal(gaps.length, 1);
  assert.deepEqual(gaps[0][1].fragments, [
    { change: "cara", text: "a" },
    { change: "bob", text: "b" },
    { change: "alice", text: "c" },
    { change: "bob", text: "d" },
  ]);
  for (const order of permutations(["cara", "bob", "alice"])) {
    const copy = oldCara();
    prune(copy);
    copy.commit();
    for (const id of order) assert.equal(reject(copy, id), true, `old records, ${order}`);
    assert.equal(textOf(copy), "abcde", `old records, three levels, ${order}`);
    assert.equal(copy.getMap(CHANGES).size, 0);
  }
}

// ---------------------------------------------------------------------------
// Regressions from review

{
  const doc = makeDoc("abcde");
  track(doc, 2, 3, "", ALICE);
  track(doc, 1, 3, "Z", BOB);
  reject(doc, resolveChanges(doc, "f1").find((c) => c.who === "alice").id);
  track(doc, 1, 1, "Q", BOB);
  for (const r of resolveChanges(doc, "f1").filter((c) => c.who === "bob")) reject(doc, r.id);
  assert.equal(textOf(doc), "abcde", "a touched fragment does not widen a distant span");
}

{
  const doc = makeDoc("abcdef");
  const M = { who: "m", author: "M" };
  track(doc, 0, 1, "", M);
  track(doc, 2, 3, "", { who: "n", author: "N" });
  track(doc, 1, 3, "", M);
  reject(doc, resolveChanges(doc, "f1").find((c) => c.who === "n").id);
  track(doc, 0, 1, "", M);
  for (const r of resolveChanges(doc, "f1").filter((c) => c.who === "m")) reject(doc, r.id);
  assert.equal(textOf(doc), "abcdef", "a merged change is relabelled in every gap");
}

{
  const doc = makeDoc("abcde");
  track(doc, 1, 2, "", { who: "p", author: "P" });
  track(doc, 2, 3, "", { who: "q", author: "Q" });
  untrackedSplice(doc, "f1", 1, 2, "");
  prune(doc);
  doc.commit();
  track(doc, 1, 1, "X", { who: "r", author: "R" });
  for (const r of resolveChanges(doc, "f1")) reject(doc, r.id);
  assert.equal(textOf(doc), "abde", "two gaps collapsed by an untracked edit");
}

// ---------------------------------------------------------------------------
// Seeded fuzz: pure deletions, some accepted and some rejected, in random order

{
  const rng = random(7);
  for (let iter = 0; iter < 200; iter++) {
    const start = "abcdefghijkl".slice(0, 5 + rng.pick(8));
    const doc = makeDoc(start);
    const steps = [];
    let cur = start;
    const n = 2 + rng.pick(4);
    for (let i = 0; i < n && cur.length > 0; i++) {
      const from = rng.pick(cur.length);
      const len = 1 + rng.pick(Math.min(4, cur.length - from));
      const w = "pqrs"[rng.pick(4)];
      steps.push([from, from + len, w]);
      trackedSplice(doc, "f1", from, from + len, "", { who: w, author: w });
      doc.commit();
      cur = cur.slice(0, from) + cur.slice(from + len);
    }
    const rows = resolveChanges(doc, "f1");
    const decisions = rows.map((r) => [r.id, rng.rnd() < 0.5 ? "accept" : "reject", r.deleted]);
    const gone = decisions.filter((d) => d[1] === "accept").map((d) => d[2]).join("");
    const expected = [...start].filter((ch) => !gone.includes(ch)).join("");
    const order = rng.shuffle(decisions);
    const label = `seed=7 iter=${iter} start=${start} steps=${JSON.stringify(steps)} decisions=${JSON.stringify(order.map((d) => d[1][0] + ":" + d[2]))}`;
    for (const [id, kind] of order) {
      if (!doc.getMap(CHANGES).get(id)) continue;
      (kind === "accept" ? accept : reject)(doc, id);
    }
    assert.equal(textOf(doc), expected, label);
    assert.equal(doc.getMap(CHANGES).size, 0, label);
  }
}

// ---------------------------------------------------------------------------
// Seeded fuzz: insertions, deletions and replacements by several authors, then
// every author's changes accepted or rejected in random order

{
  const rng = random(11);
  let insertCounter = 0;
  for (let iter = 0; iter < 200; iter++) {
    const start = "abcdefghijkl".slice(0, 5 + rng.pick(8));
    const doc = makeDoc(start);
    const steps = [];
    // model: one entry per character, with the author of its insertion (or null)
    let model = [...start].map((ch) => ({ ch, by: null }));
    const deletedBy = new Map();
    const n = 2 + rng.pick(5);
    for (let i = 0; i < n; i++) {
      const w = "pqrs"[rng.pick(4)];
      const from = rng.pick(model.length + 1);
      const len = rng.pick(Math.min(4, model.length - from) + 1);
      let ins = "";
      if (len === 0 || rng.rnd() < 0.4) {
        const k = 1 + rng.pick(2);
        for (let j = 0; j < k; j++) ins += String.fromCharCode(0x391 + (insertCounter++ % 24));
      }
      if (len === 0 && ins === "") continue;
      steps.push([from, from + len, ins, w]);
      trackedSplice(doc, "f1", from, from + len, ins, { who: w, author: w });
      doc.commit();
      for (const e of model.slice(from, from + len)) if (e.by === null && !deletedBy.has(e.ch)) deletedBy.set(e.ch, w);
      model = [...model.slice(0, from), ...[...ins].map((ch) => ({ ch, by: w })), ...model.slice(from + len)];
      assert.equal(textOf(doc), model.map((e) => e.ch).join(""), `drift seed=11 iter=${iter} after step ${i}`);
    }
    const verdict = Object.fromEntries("pqrs".split("").map((w) => [w, rng.rnd() < 0.5 ? "accept" : "reject"]));
    const rows = resolveChanges(doc, "f1");
    const label = `seed=11 iter=${iter} start=${start} steps=${JSON.stringify(steps)} verdict=${JSON.stringify(verdict)}`;
    for (const r of rng.shuffle(rows)) {
      if (!doc.getMap(CHANGES).get(r.id)) continue;
      (verdict[r.who] === "accept" ? accept : reject)(doc, r.id);
    }
    // Oracle: an original character is gone iff its first deleter was accepted;
    // an inserted character still in the text is kept iff its author was
    // accepted; original characters keep their order.
    const expectedOriginal = [...start].filter((ch) => !deletedBy.has(ch) || verdict[deletedBy.get(ch)] === "reject");
    const expectedInserted = model.filter((e) => e.by !== null && verdict[e.by] === "accept").map((e) => e.ch);
    const final = textOf(doc);
    const finalOriginal = [...final].filter((ch) => ch.charCodeAt(0) < 0x300);
    const finalInserted = [...final].filter((ch) => ch.charCodeAt(0) >= 0x300).sort().join("");
    assert.equal(finalOriginal.join(""), expectedOriginal.join(""), label);
    assert.equal(finalInserted, expectedInserted.sort().join(""), label);
    assert.equal(doc.getMap(CHANGES).size, 0, label);
  }
}

// ---------------------------------------------------------------------------
// Seeded fuzz: tracked edits, untracked edits and per-author decisions, with
// authors editing again after being decided. Every character is unique, so a
// state per character is the oracle: plain text (original, or inserted without
// tracking) is live, text deleted by a tracked change of author w is live again
// when w rejects and gone when w accepts, and a tracked insertion is pending
// until its author decides it. Original characters keep their order.

{
  const AUTHORS = ["p", "q", "r", "s"];
  const rng = random(23);
  let insertCounter = 0;
  for (let iter = 0; iter < 150; iter++) {
    const start = "abcdefghijkl".slice(0, 5 + rng.pick(8));
    const doc = makeDoc(start);
    const chars = new Map([...start].map((ch) => [ch, { plain: true, state: "live" }]));
    const verdict = Object.fromEntries(AUTHORS.map((w) => [w, rng.rnd() < 0.5 ? "accept" : "reject"]));
    const log = [];
    const decide = (w) => {
      for (const r of resolveChanges(doc, "f1").filter((c) => c.who === w)) (verdict[w] === "accept" ? accept : reject)(doc, r.id);
      for (const [, c] of chars) {
        if (c.plain && c.state === "deleted:" + w) c.state = verdict[w] === "accept" ? "gone" : "live";
        else if (!c.plain && c.by === w && c.state === "pending") {
          if (verdict[w] === "accept") {
            c.plain = true;
            c.state = "live";
          } else c.state = "gone";
        }
      }
    };
    const steps = 3 + rng.pick(6);
    try {
      for (let i = 0; i < steps; i++) {
        const kind = rng.rnd();
        if (kind < 0.2) {
          const w = AUTHORS[rng.pick(4)];
          decide(w);
          log.push(`decide ${w}=${verdict[w]}`);
          continue;
        }
        const text = textOf(doc);
        const from = rng.pick(text.length + 1);
        const len = rng.pick(Math.min(4, text.length - from) + 1);
        let insert = "";
        if (len === 0 || rng.rnd() < 0.3) insert = String.fromCharCode(0x391 + (insertCounter++ % 24));
        if (len === 0 && insert === "") continue;
        const untracked = kind > 0.85;
        const w = untracked ? null : AUTHORS[rng.pick(4)];
        log.push(`${untracked ? "untracked" : w} ${from}-${from + len} '${insert}'`);
        for (const ch of text.slice(from, from + len)) {
          const c = chars.get(ch);
          if (c.plain) c.state = untracked ? "gone" : "deleted:" + w;
          else c.state = "gone";
        }
        if (insert) chars.set(insert, untracked ? { plain: true, state: "live" } : { plain: false, by: w, state: "pending" });
        if (untracked) {
          untrackedSplice(doc, "f1", from, from + len, insert);
          prune(doc);
          doc.commit();
        } else {
          trackedSplice(doc, "f1", from, from + len, insert, { who: w, author: w });
          doc.commit();
        }
      }
      for (const w of AUTHORS) decide(w);
    } catch (e) {
      throw new Error(`fuzz seed=23 case=${iter} start=${start} log=${JSON.stringify(log)}: ${e.message}`);
    }
    const final = textOf(doc);
    const label = `fuzz seed=23 case=${iter} start=${start} verdict=${JSON.stringify(verdict)} log=${JSON.stringify(log)}`;
    const expected = [...chars].filter(([, c]) => c.state === "live").map(([ch]) => ch).sort().join("");
    const expectedOriginals = [...start].filter((ch) => chars.get(ch).state === "live").join("");
    assert.equal([...final].sort().join(""), expected, label);
    assert.equal([...final].filter((ch) => start.includes(ch)).join(""), expectedOriginals, label);
    assert.equal(doc.getMap(CHANGES).size, 0, label);
  }
}

console.log("track-changes: ok");
