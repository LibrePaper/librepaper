// The tracked-change planner and its Loro records.
//
// The shared fixtures pin the planner; the Rust port runs the same file. The
// Loro tests pin what the planner cannot: that cursors resolve back to the
// planned offsets, that a decision restores the right text in any order, and
// that a remote peer typing beside a pending insertion does not grow it.
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
  resolveChanges,
  trackedSplice,
  writePlanned,
} from "../../src/lib/track-changes.js";

const fixtures = JSON.parse(
  readFileSync(new URL("../fixtures/track-changes.json", import.meta.url), "utf8"),
);

const ME = { who: "me", author: "Me" };
const BOB = { who: "bob", author: "Bob" };

function shape(c) {
  return { id: c.id, who: c.who, from: c.from, to: c.to, deleted: c.deleted };
}

function applyEdits(text, edits) {
  let out = text;
  let adj = 0;
  for (const e of edits) {
    out = out.slice(0, e.from + adj) + e.insert + out.slice(e.to + adj);
    adj += e.insert.length - (e.to - e.from);
  }
  return out;
}

// ---------------------------------------------------------------------------
// Fixtures through the planner

for (const f of fixtures) {
  assert.equal(applyEdits(f.before, f.edits), f.after, `${f.name}: after text`);
  let n = 0;
  const planned = planEdits(
    f.before,
    f.changes.map((c) => ({ run: null, at: 1, author: c.who, ...c })),
    f.edits,
    f.me,
    { now: 2, mintId: () => `new-${++n}` },
  );
  const live = planned.filter((c) => c.state !== "removed").map(shape);
  assert.deepEqual(live, f.expect.map(({ points, ...c }) => c), f.name);
  // Points are compared only for the entries the fixture lists them on.
  for (const want of f.expect) {
    if (!want.points) continue;
    assert.deepEqual(planned.find((c) => c.id === want.id).points, want.points, `${f.name}: points`);
  }
}

// Unchanged entries are kept, edited ones are set, dropped ones are removed.
{
  const changes = [
    { id: "a", who: "me", author: "Me", run: null, at: 1, from: 2, to: 4, deleted: "" },
    { id: "b", who: "bob", author: "Bob", run: null, at: 1, from: 10, to: 12, deleted: "" },
  ];
  const planned = planEdits("0123456789abcdef", changes, [{ from: 4, to: 4, insert: "Z" }], ME, {
    now: 5,
    mintId: () => "new-1",
  });
  const state = Object.fromEntries(planned.map((c) => [c.id, c.state]));
  assert.deepEqual(state, { a: "set", b: "kept" });
  assert.equal(planned.find((c) => c.id === "a").at, 5);
  const removed = planEdits("abXYcd", [{ id: "a", who: "me", author: "Me", run: null, at: 1, from: 2, to: 4, deleted: "" }], [{ from: 2, to: 4, insert: "" }], ME, {
    now: 5,
    mintId: () => "new-1",
  });
  assert.deepEqual(removed.map((c) => [c.id, c.state, c.from, c.to]), [["a", "removed", 2, 2]]);
  // The run tag lands on new changes.
  const tagged = planEdits("", [], [{ from: 0, to: 0, insert: "x" }], ME, {
    run: "pass-1",
    now: 5,
    mintId: () => "new-1",
  });
  assert.equal(tagged[0].run, "pass-1");
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
// recordEdits then resolveChanges agrees with the planner

for (const f of fixtures.filter((x) => x.changes.length === 0)) {
  const doc = makeDoc(f.before);
  const text = doc.getMap("files").get("f1");
  let adj = 0;
  for (const e of f.edits) {
    if (e.to > e.from) text.delete(e.from + adj, e.to - e.from);
    if (e.insert) text.insert(e.from + adj, e.insert);
    adj += e.insert.length - (e.to - e.from);
  }
  assert.equal(text.toString(), f.after, f.name);
  recordEdits(doc, "f1", f.before, [], f.edits, f.me, { now: 3 });
  doc.commit();
  const resolved = resolveChanges(doc, "f1").map((c) => ({ who: c.who, from: c.from, to: c.to, deleted: c.deleted }));
  const expected = f.expect.map((c) => ({ who: c.who, from: c.from, to: c.to, deleted: c.deleted }));
  assert.deepEqual(resolved, expected, `round trip: ${f.name}`);
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

// Accept keeps the text and drops the entry.
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

// Reject restores a replacement, an insertion and a deletion.
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
  // Empty: no insertion and nothing deleted.
  const text = doc.getMap("files").get("f1");
  map.set("empty", { ...live, start: text.getCursor(0, -1).encode(), end: null, deleted: "" });
  // Orphaned: the file is gone.
  map.set("orphan", { ...live, file: "missing" });
  doc.commit();
  assert.equal(resolveChanges(doc, "f1").length, 1);
  assert.equal(listChanges(doc).length, 1);
  assert.equal(prune(doc), 2);
  doc.commit();
  assert.deepEqual([...map.keys()], [keep]);
  assert.equal(prune(doc), 0);
}

// Accept and reject drop an orphan without touching text.
for (const decide of [accept, reject]) {
  const doc = makeDoc("abcd");
  const [id] = track(doc, 1, 2, "Z");
  const map = doc.getMap(CHANGES);
  map.set(id, { ...map.get(id), file: "missing" });
  doc.commit();
  assert.equal(decide(doc, id), true);
  assert.equal(textOf(doc), "aZcd");
  assert.equal(map.size, 0);
}

// ---------------------------------------------------------------------------
// Swallowed deletion points

// Rejects `first`, then every change that is still pending, one at a time.
function rejectAll(doc, first) {
  assert.equal(reject(doc, first), true);
  for (const id of [...doc.getMap(CHANGES).keys()]) assert.equal(reject(doc, id), true);
}

for (const bobFirst of [true, false]) {
  const doc = makeDoc("abcd");
  const [a] = track(doc, 1, 2, "", ME);
  const b = track(doc, 1, 2, "", BOB).find((id) => id !== a);
  assert.equal(textOf(doc), "ad");
  const entry = doc.getMap(CHANGES).get(b);
  assert.deepEqual(entry.points, [{ id: a, at: 0 }]);
  rejectAll(doc, bobFirst ? b : a);
  assert.equal(textOf(doc), "abcd", `points: bobFirst=${bobFirst}`);
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// A point that sat in the middle of the new deletion.
for (const bobFirst of [true, false]) {
  const doc = makeDoc("abcde");
  const [a] = track(doc, 2, 3, "", ME);
  assert.equal(textOf(doc), "abde");
  const b = track(doc, 1, 3, "", BOB).find((id) => id !== a);
  assert.equal(textOf(doc), "ae");
  assert.deepEqual(doc.getMap(CHANGES).get(b).points, [{ id: a, at: 1 }]);
  rejectAll(doc, bobFirst ? b : a);
  assert.equal(textOf(doc), "abcde", `points middle: bobFirst=${bobFirst}`);
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// The owner is a replacement and the swallowed change is rejected first: the
// inserted text stays pending under the right part.
{
  const doc = makeDoc("abcde");
  const [a] = track(doc, 2, 3, "", ME);
  const b = track(doc, 1, 3, "Z", BOB).find((id) => id !== a);
  assert.equal(textOf(doc), "aZe");
  assert.equal(reject(doc, a), true);
  assert.equal(textOf(doc), "acZe");
  const pending = resolveChanges(doc, "f1");
  assert.equal(pending.length, 2);
  const left = pending.find((c) => c.id === b);
  assert.deepEqual([left.from, left.to, left.deleted], [1, 1, "b"]);
  const right = pending.find((c) => c.id !== b);
  assert.deepEqual([right.from, right.to, right.deleted], [2, 3, "d"]);
  assert.equal(reject(doc, b), true);
  assert.equal(textOf(doc), "abcZe");
  assert.equal(reject(doc, right.id), true);
  assert.equal(textOf(doc), "abcde");
  assert.equal(doc.getMap(CHANGES).size, 0);
}

// Accepting the swallowing change drops its points with it.
{
  const doc = makeDoc("abcd");
  const [a] = track(doc, 1, 2, "", ME);
  const b = track(doc, 1, 2, "", BOB).find((id) => id !== a);
  assert.equal(accept(doc, b), true);
  assert.equal(reject(doc, a), true);
  assert.equal(textOf(doc), "abd");
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

// An entry written the way Rust writes it: the end cursor sits on the emoji's
// first unit, side 1.
{
  const doc = makeDoc("a😀b");
  const text = doc.getMap("files").get("f1");
  doc.getMap(CHANGES).set("rust", {
    file: "f1",
    author: "Me",
    who: "me",
    at: 1,
    run: null,
    start: text.getCursor(1, -1).encode(),
    end: text.getCursor(1, 1).encode(),
    deleted: "",
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
  const pre = resolveChanges(doc, "f1");
  const text = doc.getMap("files").get("f1");
  text.delete(1, 1);
  const plan = planEdits("aZcd", pre, [{ from: 1, to: 2, insert: "" }], ME, { now: 9, mintId: () => "n" });
  const ids = writePlanned(doc, "f1", plan);
  doc.commit();
  assert.deepEqual(ids, [old]);
  assert.equal(resolveChanges(doc, "f1")[0].deleted, "b");
}

console.log("track-changes: ok");
