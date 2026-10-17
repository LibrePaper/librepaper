// The tracked-change planner and its Loro records.
//
// The shared fixtures pin the planner; the Rust port runs the same file. The
// Loro tests pin what the planner cannot: that cursors resolve back to the
// planned offsets, that a decision restores the right text in any order, that
// a remote peer typing beside a pending insertion does not grow it, that nested
// deletions come back in every order, that records of the earlier model are
// read in place, convert when touched and then decide in any order, that a
// record rests before prune drops it, that a settled stale cursor is healed,
// that a fragment whose change is gone is re-parented, and that two sessions of
// one author converge. The seeded fuzzers at the end check random histories
// against their oracles, and two peers editing at once must converge.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LoroDoc, LoroMap, LoroText } from "loro-crdt";
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
  // The fixtures describe one session, so every stored fragment may join.
  const planned = planEdits(f.before, state, f.edits, f.me, { now: 2, mintId: () => "new-" + ++n, untracked: Boolean(f.untracked), joinable: () => true });
  // A foreign span the edit emptied stays in the planner's output with
  // emptied set. It holds no text, so the fixture does not describe it.
  const changes = planned.changes.filter((c) => c.state !== "removed" && !c.emptied).sort(byPlace);
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

// Writes a record as a map container with the given fields, one key at a time,
// the way a peer writes it.
function putRecord(doc, id, fields) {
  const record = doc.getMap(CHANGES).setContainer(id, new LoroMap());
  for (const [key, value] of Object.entries(fields)) record.set(key, value);
  return record;
}

// Writes a copy of a record under another id, with some fields replaced.
function copyRecord(doc, from, to, overrides) {
  const source = doc.getMap(CHANGES).get(from);
  const fields = Object.fromEntries([...source.keys()].map((key) => [key, source.get(key)]));
  return putRecord(doc, to, { ...fields, ...overrides });
}

// Two copies of one document exchange their updates both ways.
function syncBoth(a, b) {
  a.import(b.export({ mode: "update", from: a.oplogVersion() }));
  b.import(a.export({ mode: "update", from: b.oplogVersion() }));
}

// The rows of the file, sorted, with the fields that identify them.
const rowsOf = (doc) =>
  resolveChanges(doc, "f1")
    .map(({ id, who, from, to, deleted }) => ({ id, who, from, to, deleted }))
    .sort(byJson);

// How many records are still plain values, the earlier model.
const plainCount = (doc) => [...doc.getMap(CHANGES).entries()].filter(([, v]) => !(v instanceof LoroMap)).length;

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
  assert.equal(resolveChanges(doc, "f1").length, 0);
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
  assert.equal(resolveChanges(doc, "f1").length, 0);
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
  assert.equal(resolveChanges(doc, "f1").length, 0);
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

// A peer deletes the last character of a pending insertion. The span still
// resolves to the text left, and prune keeps the record as it is.
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
  assert.deepEqual(resolveChanges(a, "f1").map((c) => [c.from, c.to]), [[5, 7]]);
}

// ---------------------------------------------------------------------------
// prune

{
  const doc = makeDoc("abcd");
  const [keep] = track(doc, 1, 2, "Z");
  const map = doc.getMap(CHANGES);
  // Empty: a change with no insertion and no fragment, and nothing recent.
  const text = doc.getMap("files").get("f1");
  putRecord(doc, "empty", { kind: "change", file: "f1", who: "me", author: "Me", run: null, at: 0, start: text.getCursor(0, -1).encode(), end: null });
  // Orphaned: the file is gone, and the record has rested.
  copyRecord(doc, keep, "orphan", { file: "missing", at: 0 });
  doc.commit();
  assert.equal(resolveChanges(doc, "f1").length, 1);
  assert.equal(listChanges(doc).length, 1);
  assert.equal(prune(doc), 2);
  doc.commit();
  // The change and its fragment are left.
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
  for (const [, value] of [...map.entries()]) value.set("file", "missing");
  doc.commit();
  assert.equal(decide(doc, id), true);
  assert.equal(textOf(doc), "aZcd");
  assert.equal(map.get(id), undefined);
  prune(doc);
  doc.commit();
  assert.equal(map.size, 0);
}

// prune reads a record of the earlier model in place: the row is there, the
// plain record stays plain, and nothing is dropped.
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
  const row = { id: "old", who: "me", from: 1, to: 2, deleted: "b" };
  assert.deepEqual(rowsOf(doc), [row]);
  assert.equal(prune(doc), 0);
  doc.commit();
  assert.equal(plainCount(doc), 1);
  assert.deepEqual(rowsOf(doc), [row]);
}

// A change emptied by another author's deletion leaves the rows at once but
// stays in the map until it has rested: prune keeps a fresh record and drops
// one that has rested.
{
  const doc = makeDoc("abcd");
  const [alice] = track(doc, 2, 2, "XY", ALICE);
  track(doc, 2, 4, "", BOB);
  assert.equal(textOf(doc), "abcd");
  assert.equal(resolveChanges(doc, "f1").length, 0);
  assert.notEqual(doc.getMap(CHANGES).get(alice), undefined);
  assert.equal(prune(doc), 0);
  doc.commit();
  assert.notEqual(doc.getMap(CHANGES).get(alice), undefined);
  doc.getMap(CHANGES).get(alice).set("at", 0);
  doc.commit();
  assert.equal(prune(doc), 1);
  doc.commit();
  assert.equal(doc.getMap(CHANGES).get(alice), undefined);
}

// A fragment whose change is gone, as a peer leaves it when it decides that
// change, is re-parented by prune onto a change of its own. The change shows as
// a row with the fragment's text, and rejecting the row puts the text back.
{
  const doc = makeDoc("acd");
  const text = doc.getMap("files").get("f1");
  putRecord(doc, "orphan", {
    kind: "fragment",
    file: "f1",
    change: "gone",
    text: "b",
    start: text.getCursor(1, -1).encode(),
    rank: 0,
    who: "bob",
    author: "Bob",
    run: null,
    at: 1,
  });
  doc.commit();
  assert.equal(resolveChanges(doc, "f1").length, 0);
  assert.equal(prune(doc), 0);
  doc.commit();
  const rows = resolveChanges(doc, "f1");
  assert.deepEqual(rows.map(({ who, from, to, deleted }) => ({ who, from, to, deleted })), [{ who: "bob", from: 1, to: 1, deleted: "b" }]);
  assert.equal(reject(doc, rows[0].id), true);
  assert.equal(textOf(doc), "abcd");
  assert.equal(resolveChanges(doc, "f1").length, 0);
}

// A raw deletion takes the character a fragment is anchored on, which leaves
// its cursor stale. Once the record has rested, prune heals the cursor onto a
// live character, and the fragment stays where it was.
{
  const doc = makeDoc("abcdef");
  track(doc, 2, 3, "", ME);
  doc.getMap("files").get("f1").delete(2, 1);
  doc.commit();
  const [before] = resolveFile(doc, "f1").gaps;
  assert.equal(before.stale, true);
  doc.getMap(CHANGES).get(before.fragments[0].id).set("at", 0);
  doc.commit();
  prune(doc);
  doc.commit();
  const [after] = resolveFile(doc, "f1").gaps;
  assert.equal(after.stale, false);
  assert.equal(after.at, 2);
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
  assert.equal(resolveChanges(doc, "f1").length, 0);
}

// A change written the way Rust writes it: the end cursor sits on the emoji's
// first unit, side 1.
{
  const doc = makeDoc("a😀b");
  const text = doc.getMap("files").get("f1");
  putRecord(doc, "rust", {
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
  track(doc, 1, 2, "Z");
  const pre = resolveFile(doc, "f1");
  const text = doc.getMap("files").get("f1");
  text.delete(1, 1);
  const plan = planEdits("aZcd", pre, [{ from: 1, to: 2, insert: "" }], ME, { now: 9, mintId: () => "n" });
  const ids = writePlanned(doc, "f1", plan);
  doc.commit();
  // My own span, deleted whole by me, leaves a pure deletion of "b".
  assert.equal(ids.length, 1);
  assert.deepEqual(resolveChanges(doc, "f1").map((c) => [c.from, c.to, c.deleted]), [[1, 1, "b"]]);
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
    ids.push(got.find((id) => doc.getMap(CHANGES).get(id)?.get("who") === name));
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
    assert.equal(resolveChanges(doc, "f1").length, 0, `${label}: no record left, one at a time in ${order}`);
  }
  for (const order of orders) {
    const { doc, ids } = buildNested(start, steps);
    rejectMany(doc, order.map((k) => ids[k]));
    assert.equal(textOf(doc), start, `${label}: rejectMany in ${order}`);
    assert.equal(resolveChanges(doc, "f1").length, 0, `${label}: no record left after rejectMany in ${order}`);
  }
}

// Accept the middle change, reject the other two: what the middle one kept.
{
  const { doc, ids } = buildNested("abcde", [[2, 3], [1, 3], [0, 1]]);
  assert.equal(accept(doc, ids[1]), true);
  assert.equal(reject(doc, ids[0]), true);
  assert.equal(reject(doc, ids[2]), true);
  assert.equal(textOf(doc), "ace");
  assert.equal(resolveChanges(doc, "f1").length, 0);
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

// The records are read in place: the rows show them, nothing is written, and
// the gap holds the fragments in order.
{
  const doc = oldAliceBob();
  assert.deepEqual(rowsOf(doc), [
    { id: "alice", who: "alice", from: 1, to: 1, deleted: "c" },
    { id: "bob", who: "bob", from: 1, to: 1, deleted: "bd" },
  ]);
  assert.deepEqual(resolveFile(doc, "f1").gaps.map((g) => g.fragments.map((f) => f.text)), [["b", "c", "d"]]);
  assert.equal(plainCount(doc), 2);
  assert.equal(prune(doc), 0);
  assert.equal(plainCount(doc), 2);
}

// Rejecting in either order restores the text. Each reject converts the records
// it decides, so the file reads as containers from then on.
for (const order of [["bob", "alice"], ["alice", "bob"]]) {
  const doc = oldAliceBob();
  for (const id of order) assert.equal(reject(doc, id), true, `old records, ${order}`);
  assert.equal(textOf(doc), "abcde", `old records, ${order}`);
  assert.equal(resolveChanges(doc, "f1").length, 0);
}

// A tracked edit elsewhere in the file leaves the plain records as they are.
{
  const doc = oldAliceBob();
  track(doc, 2, 2, "X", ME);
  assert.equal(textOf(doc), "aeX");
  assert.equal(plainCount(doc), 2);
  assert.equal(resolveChanges(doc, "f1").length, 3);
}

// A tracked deletion right before the gap swallows it. The records it swallows
// are rewritten as containers, and rejecting everything restores the text.
{
  const doc = oldAliceBob();
  track(doc, 0, 1, "", ME);
  assert.equal(textOf(doc), "e");
  assert.equal(plainCount(doc), 0);
  for (const r of resolveChanges(doc, "f1")) reject(doc, r.id);
  assert.equal(textOf(doc), "abcde");
  assert.equal(resolveChanges(doc, "f1").length, 0);
}

// Cara's three levels: the rows show all three, the gap holds the four
// fragments in order, and rejecting them in any order restores the text.
{
  const doc = oldCara();
  assert.deepEqual(rowsOf(doc).map(({ id, deleted }) => [id, deleted]), [["alice", "c"], ["bob", "bd"], ["cara", "a"]]);
  assert.deepEqual(resolveFile(doc, "f1").gaps.map((g) => g.fragments.map((f) => f.text)), [["a", "b", "c", "d"]]);
  assert.equal(plainCount(doc), 3);
}
for (const order of permutations(["cara", "bob", "alice"])) {
  const copy = oldCara();
  for (const id of order) assert.equal(reject(copy, id), true, `old records, ${order}`);
  assert.equal(textOf(copy), "abcde", `old records, three levels, ${order}`);
  assert.equal(resolveChanges(copy, "f1").length, 0);
}

// An old pending insertion is read as a span. A deletion inside that span
// shrinks it, and rejecting what is left gives back the text without it.
{
  const doc = makeDoc("ab");
  const text = doc.getMap("files").get("f1");
  text.insert(1, "XY");
  doc.getMap(CHANGES).set("old", {
    file: "f1",
    who: "me",
    author: "Me",
    at: 1,
    run: null,
    start: text.getCursor(1, -1).encode(),
    end: text.getCursor(2, 1).encode(),
    deleted: "",
  });
  doc.commit();
  assert.equal(plainCount(doc), 1);
  assert.deepEqual(resolveChanges(doc, "f1").map(({ from, to, deleted }) => [from, to, deleted]), [[1, 3, ""]]);
  const count = doc.getMap(CHANGES).size;
  trackedSplice(doc, "f1", 1, 2, "", ME);
  doc.commit();
  assert.equal(textOf(doc), "aYb");
  assert.deepEqual(resolveChanges(doc, "f1").map(({ from, to }) => [from, to]), [[1, 2]]);
  assert.equal(doc.getMap(CHANGES).size, count);
  for (const row of resolveChanges(doc, "f1")) reject(doc, row.id);
  assert.equal(textOf(doc), "ab");
}

// Two copies read the same old record. One converts it by editing next to it,
// the other edits elsewhere. After syncing both ways the rows agree, and
// rejecting everything on one copy and syncing leaves the start text on both.
{
  const a = oldAliceBob();
  const b = new LoroDoc();
  b.setPeerId(2);
  b.import(a.export({ mode: "snapshot" }));
  track(a, 0, 1, "", ME);
  track(b, 2, 2, "X", ME);
  syncBoth(a, b);
  assert.equal(textOf(a), "eX");
  assert.equal(textOf(b), "eX");
  assert.deepEqual(rowsOf(a), rowsOf(b));
  for (const r of resolveChanges(a, "f1")) reject(a, r.id);
  syncBoth(a, b);
  assert.equal(textOf(a), "abcde");
  assert.equal(textOf(b), "abcde");
  assert.equal(resolveChanges(b, "f1").length, 0);
}

// ---------------------------------------------------------------------------
// Two sessions of one author

// One author in two sessions, each with its own copy. The first session deletes
// c. Then the sessions delete b in one and d in the other. After syncing, both
// deletions are kept, and rejecting everything restores the text.
{
  const a = makeDoc("abcdef", 1);
  track(a, 2, 3, "", ME);
  const b = new LoroDoc();
  b.setPeerId(2);
  b.import(a.export({ mode: "snapshot" }));
  track(a, 1, 2, "", ME);
  track(b, 2, 3, "", ME);
  syncBoth(a, b);
  assert.equal(textOf(a), textOf(b));
  assert.equal(textOf(a), "aef");
  for (const r of resolveChanges(a, "f1")) reject(a, r.id);
  syncBoth(a, b);
  assert.equal(textOf(a), "abcdef");
  assert.equal(textOf(b), "abcdef");
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
    assert.equal(resolveChanges(doc, "f1").length, 0, label);
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
    assert.equal(resolveChanges(doc, "f1").length, 0, label);
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
    assert.equal(resolveChanges(doc, "f1").length, 0, label);
  }
}

// ---------------------------------------------------------------------------
// Seeded fuzz: two peers edit the same text at the same moment, then exchange
// updates. The peers must converge on the same text and the same rows, and
// rejecting every change must give back the start text's characters. "apart"
// keeps a character between the two regions an edit touches; "touching" lets
// them meet.

// Two regions of the text, the first ending before the second starts.
function regions(text, rng, apart) {
  const cut = apart ? 2 + rng.pick(Math.max(1, text.length - 3)) : 1 + rng.pick(Math.max(1, text.length - 2));
  const f1 = rng.pick(cut);
  const t1 = f1 + rng.pick(Math.min(3, cut - f1) + 1);
  const f2 = apart ? cut + 1 + rng.pick(Math.max(1, text.length - cut - 1)) : cut + rng.pick(Math.max(1, text.length - cut));
  const t2 = Math.min(text.length, f2 + rng.pick(3));
  return { f1, t1, f2, t2 };
}

function twoPeers(variant) {
  const apart = variant === "apart";
  const rng = random(31);
  const CASES = 150;
  let insertCounter = 0;
  let misordered = 0;
  for (let iter = 0; iter < CASES; iter++) {
    const start = "abcdefghijkl".slice(0, 7 + rng.pick(6));
    const a = makeDoc(start, 1);
    const b = new LoroDoc();
    b.setPeerId(2);
    b.import(a.export({ mode: "snapshot" }));
    const sync = () => {
      a.import(b.export({ mode: "update", from: a.oplogVersion() }));
      b.import(a.export({ mode: "update", from: b.oplogVersion() }));
    };
    const rowKeys = (doc) => resolveChanges(doc, "f1").map((c) => `${c.who}:${c.deleted}+${c.to - c.from}`).sort().join(" ");
    const log = [];
    const where = () => `two peers ${variant} case=${iter} start=${start} log=${JSON.stringify(log)}`;
    try {
      for (let round = 0; round < 2 + rng.pick(3); round++) {
        const text = textOf(a);
        if (text.length < 4) break;
        const { f1, t1, f2, t2 } = regions(text, rng, apart);
        const i1 = t1 === f1 || rng.rnd() < 0.3 ? String.fromCharCode(0x391 + (insertCounter++ % 24)) : "";
        const i2 = t2 === f2 || rng.rnd() < 0.3 ? String.fromCharCode(0x391 + (insertCounter++ % 24)) : "";
        if (f1 === t1 && !i1) continue;
        // Alice edits the right region and Bob the left one, both against the same text.
        log.push(`A ${f2}-${t2} '${i2}' | B ${f1}-${t1} '${i1}'`);
        if (t2 > f2 || i2) track(a, f2, t2, i2, ALICE);
        track(b, f1, t1, i1, BOB);
        sync();
        assert.equal(textOf(a), textOf(b), where());
        assert.equal(rowKeys(a), rowKeys(b), where());
      }
      for (const r of resolveChanges(a, "f1")) reject(a, r.id);
      sync();
      const sorted = (t) => [...t].sort().join("");
      assert.equal(sorted(textOf(a)), sorted(start), where());
      assert.equal(textOf(a), textOf(b), where());
      assert.equal(resolveChanges(a, "f1").length, 0, where());
      if (textOf(a) !== start) misordered += 1;
    } catch (e) {
      throw new Error(`${where()}: ${e.message}`);
    }
  }
  assert.ok(misordered < CASES / 4, `two peers ${variant}: the order differed in ${misordered} of ${CASES} cases`);
}

twoPeers("apart");
twoPeers("touching");


// A document whose only records are legacy ones still lists them, and listing
// converts nothing.
{
  const doc = makeDoc("ae");
  const text = doc.getMap("files").get("f1");
  doc.getMap(CHANGES).set("alice", { file: "f1", who: "alice", author: "Alice", at: 1, run: null, start: text.getCursor(1, -1).encode(), end: null, deleted: "c" });
  doc.commit();
  const rows = listChanges(doc);
  assert.deepEqual(rows.map((r) => [r.file, r.who, r.deleted, r.inserted]), [["f1", "alice", "c", ""]]);
  assert.equal([...doc.getMap(CHANGES).values()].every((v) => !(v instanceof LoroMap)), true);
}

console.log("track-changes: ok");
