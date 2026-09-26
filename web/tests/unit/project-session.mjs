import { VersionVector } from "loro-crdt";
import assert from "node:assert/strict";
import { createProjectSession } from "../../src/lib/project-session.js";
import { assertSameProject, projectIdentity, sameProject } from "../../src/lib/project-identity.js";

const encode = (bytes) => btoa(String.fromCharCode(...bytes));
const decode = encoded => Uint8Array.from(atob(encoded), char => char.charCodeAt(0));

const states = [];
let persistenceEvents;
let closed = false;
const sent = [];
const session = createProjectSession({
  send: (message) => sent.push(message),
  onState: (state) => states.push(state),
  presenceId: () => "test-presence",
  persistence: {
    open(_doc, events) {
      persistenceEvents = events;
      return { close: () => { closed = true; } };
    },
  },
});

try {
  assert.equal(states.at(-1).local, false);
  persistenceEvents.hydrated();
  assert.equal(states.at(-1).local, true);
  persistenceEvents.writing(2);
  assert.equal(states.at(-1).localPending, 2);
  persistenceEvents.persisted();
  assert.equal(states.at(-1).localPending, 0);

  session.addText("paper.md", "offline");
  assert.equal(states.at(-1).pending, 1);
  assert.equal(sent.at(-1).type, "doc-update");
  // §6.3: acknowledgement is transport bookkeeping. It cannot claim that
  // the server's buffered work is durable.
  session.acknowledge(sent.at(-1).seq);
  assert.equal(states.at(-1).pending, 1);
  session.durable(encode(session.doc.oplogVersion().encode()));
  assert.equal(states.at(-1).pending, 0);

  persistenceEvents.failed(new Error("quota full"));
  assert.equal(states.at(-1).localError, "quota full");
  assert.equal(states.at(-1).local, false);
} finally {
  session.leave();
}
assert.equal(closed, true);

// A swap is the main file becoming a different file. Editing the main file is
// not one: everything hanging off `onSwap` rebuilds itself, so a swap per
// keystroke used to throw the editor -- and the caret with it -- back to the
// top of the file on every commit.
const swapping = createProjectSession({ send: () => {}, onState: () => {} });
try {
  const main = swapping.addText("paper.md", "hello");
  swapping.setMain(main);
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  let swaps = 0;
  swapping.onSwap(() => { swaps += 1; });
  swapping.textOf(main).insert(5, " world");
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  assert.equal(swaps, 0);
  assert.equal(swapping.text.toString(), "hello world");

  const other = swapping.addText("other.md", "x");
  swapping.setMain(other);
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  assert.equal(swaps, 1);
  assert.equal(swapping.text.toString(), "x");
} finally {
  swapping.leave();
}

// The bound source must use the projection's canonical main selection. A raw
// meta.main value can name a deleted file, or a file whose path is rejected
// once deployment rules arrive; in either case the projection falls back to a
// deterministic text and the editor must follow that same text.
{
  const rules = { text_extensions: [".md"], asset_extensions: [], derived_extensions: [], max_path: 200 };
  const canonical = createProjectSession({ send: () => {}, onState: () => {} });
  try {
    const first = canonical.addText("first.md", "first");
    canonical.addText("second.md", "second");
    canonical.setMain(first);
    canonical.doc.commit();
    assert.equal(canonical.text.toString(), "first");

    canonical.setMain("deleted-main");
    canonical.doc.commit();
    assert.equal(canonical.mainId(), canonical.list()[0].id, "missing declarations use the projected fallback");
    assert.equal(canonical.text?.id, canonical.textOf(canonical.mainId()).id, "text follows the projected fallback id");
    assert.equal(canonical.text.toString(), canonical.textOf(canonical.mainId()).toString());

    const ruled = createProjectSession({ send: () => {}, onState: () => {} });
    try {
      ruled.addText("valid.md", "valid");
      const invalid = ruled.addText("invalid.txt", "invalid");
      ruled.setMain(invalid);
      ruled.doc.commit();
      let changes = 0;
      ruled.watchSource(() => { changes += 1; });
      ruled.setRules(rules);
      assert.equal(ruled.mainPath(), "valid.md", "rules reject the declared non-text main path");
      assert.notEqual(ruled.mainId(), invalid, "rules fallback does not bind the rejected file");
      assert.equal(ruled.text.toString(), "valid", "rules fallback and bound text agree");

      const beforeInvalidEdit = changes;
      ruled.textOf(invalid).insert(0, "still-invalid");
      ruled.doc.commit();
      assert.equal(changes, beforeInvalidEdit, "edits to the rejected old main do not reach the source watcher");

      ruled.textOf(ruled.mainId()).insert(0, "now-bound");
      ruled.doc.commit();
      assert.equal(changes, beforeInvalidEdit + 1, "edits to the projected fallback reach the source watcher");
    } finally {
      ruled.leave();
    }
  } finally {
    canonical.leave();
  }
}

const first = projectIdentity({ server: "https://paper.example/path", slug: "paper", createdAt: "one" });
const same = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "one" });
const recreated = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "two" });
assert.equal(sameProject(first, same), true);
assert.throws(() => assertSameProject(first, recreated), { code: "project-identity-changed" });

// What counts as a change to the directory. The file list moves with four
// maps -- the texts, their paths, the figures and the metadata -- and only the
// session knows which containers those are. A reader that decided for itself,
// by watching the texts alone, drew every rename and every move as no change
// at all: the name moved in `paths`, the list was never told, and the file
// kept its old name on screen until the page was reloaded.
{
  const rules = { extensions: [".md"], text_extensions: [".md"], asset_extensions: [], derived_extensions: [],
    max_path: 200, max_files: 200, log_quota_bytes: 1000000, storage: { per_owner: 1000000 } };
  const directory = createProjectSession({ send: () => {}, onState: () => {} });
  try {
    const main = directory.addText("paper.md", "hello");
    directory.setMain(main);
    directory.doc.commit();
    await new Promise((resume) => setTimeout(resume, 20));
    const batches = [];
    const stop = directory.onFiles((events) => batches.push(directory.directoryChanged(events)));

    directory.relocate([{ id: main, kind: "text", path: "paper.md" }], "renamed.md", rules, true);
    await new Promise((resume) => setTimeout(resume, 20));
    assert.deepEqual(directory.list().map((file) => file.path), ["renamed.md"]);
    assert.equal(batches.at(-1), true, "a rename is a change to the directory");

    directory.addFolder("chapters", rules);
    await new Promise((resume) => setTimeout(resume, 20));
    assert.equal(batches.at(-1), true, "a new folder is a change to the directory");

    const seen = batches.length;
    directory.textOf(main).insert(5, " there");
    directory.doc.commit();
    await new Promise((resume) => setTimeout(resume, 20));
    assert.equal(batches.length, seen + 1, "typing still reaches the watcher, for the preview");
    assert.equal(batches.at(-1), false, "but typing inside a file is not a change to the directory");
    stop();
  } finally {
    directory.leave();
  }
}

// `librepaper.room.v3`: join and gap handling (§6.2, §5 step 4, §14.2). A
// base64 helper matching the wire encoding `project-session.js` uses
// internally, since that function is not exported.
// A protocol string other than the one this client speaks is refused before
// any update is accepted (§6.1): there is no compatibility path to fall
// back to.
{
  const refused = createProjectSession({ send: () => {}, onState: () => {} });
  try {
    await assert.rejects(
      () => refused.start({ protocol: "librepaper.room.v2", vector: "", updates: [] }),
      /requires a newer LibrePaper version/,
    );
    await assert.rejects(
      () => refused.rows({ protocol: "librepaper.room.v2", vector: "", updates: [] }),
      /requires a newer LibrePaper version/,
    );
  } finally {
    refused.leave();
  }
}

// A stand-in for the server's log: one document with a base commit and two
// rows after it, so the three join shapes 6.2 describes can each be handed
// only the part of that history their scenario says they are missing.
function sourceLog() {
  const source = createProjectSession({ send: () => {}, onState: () => {} });
  const main = source.addText("paper.md", "hello");
  source.setMain(main);
  source.doc.commit();
  const base = source.doc.export({ mode: "update" });
  const afterBase = source.doc.oplogVersion();

  source.textOf(main).insert(5, " there");
  source.doc.commit();
  const afterRow1 = source.doc.oplogVersion();
  const row1 = source.doc.export({ mode: "update", from: afterBase });

  source.textOf(main).insert(11, " world");
  source.doc.commit();
  const head = source.doc.oplogVersion();
  const row2 = source.doc.export({ mode: "update", from: afterRow1 });

  const text = "hello there world";
  assert.equal(source.text.toString(), text, "the fixture reads back what it was built to hold");
  source.leave();
  return { base, row1, row2, head, text };
}

// Join from an empty vector (§6.2 step 5): the client has nothing, so the
// base arrives inline on `doc-state` and the rows that came after it arrive
// on the `doc-rows` that follows. `start` handles the first message, `rows`
// the second; each is a complete catch-up in its own right (§6.2 step 6), so
// running it twice -- once too early, once for real -- costs nothing.
{
  const { base, row1, row2, head, text } = sourceLog();
  const sentA = [];
  const joiner = createProjectSession({ send: (message) => sentA.push(message), onState: () => {}, presenceId: () => "join-empty" });
  try {
    await joiner.start({ protocol: "librepaper.room.v3", vector: encode(head.encode()), base: encode(base), updates: [] });
    assert.equal(joiner.joined, true, "doc-state is a valid join reply on its own, so this browser is announced already");
    await joiner.rows({ protocol: "librepaper.room.v3", vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
    assert.equal(joiner.text_(), text, "importing the base and then both rows reproduces the source's text");
    assert.equal(joiner.joined, true);
    assert.equal(sentA.filter((message) => message.type === "doc-update").length, 2,
      "each of the two join frames runs its own catch-up export");
  } finally {
    joiner.leave();
  }
}

// Join from a vector covering the base only (§6.2 step 4): the base was
// already local -- from a prior session's persistence, say -- so the server
// never sends `doc-state` at all, only `doc-rows` for what that vector does
// not cover.
{
  const { base, row1, row2, head, text } = sourceLog();
  const sentB = [];
  const joiner = createProjectSession({ send: (message) => sentB.push(message), onState: () => {}, presenceId: () => "join-base-only" });
  try {
    joiner.doc.import(base);
    assert.equal(joiner.joined, false);
    await joiner.rows({ protocol: "librepaper.room.v3", vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
    assert.equal(joiner.text_(), text, "the rows close the gap between the local base and the head");
    assert.equal(joiner.joined, true);
  } finally {
    joiner.leave();
  }
}

// Join from a vector missing one row (§6.2 step 4 again, the narrower case):
// over-sending the row the client already had is harmless because Loro
// discards operations it already holds, so `doc-rows` naming both rows still
// converges correctly even though only the second one was actually missing.
{
  const { base, row1, row2, head, text } = sourceLog();
  const sentC = [];
  const joiner = createProjectSession({ send: (message) => sentC.push(message), onState: () => {}, presenceId: () => "join-one-row" });
  try {
    joiner.doc.import(base);
    joiner.doc.import(row1);
    await joiner.rows({ protocol: "librepaper.room.v3", vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
    assert.equal(joiner.text_(), text, "re-importing the row already held costs nothing and the missing one lands");
    assert.equal(joiner.joined, true);
  } finally {
    joiner.leave();
  }
}

// SPEC-server-is-a-log.md priority-1 item: "serialize browser join
// processing: await local hydration ... before completing doc-rows
// catch-up." A join can be answered by `doc-rows` alone (§6.2 step 4), with
// no preceding `doc-state`, so `rows` cannot assume `start` already awaited
// hydration -- it has to await it itself. This reproduces a browser that
// still has an offline edit sitting in IndexedDB, not yet imported into
// `doc`, when `doc-rows` arrives: against the unfixed code (`rows` exporting
// synchronously, before hydration lands) the catch-up export is sent before
// `doc.import` for the offline edit ever runs, and that edit never reaches
// the server -- exactly the silent data loss the outbox exists to prevent.
{
  const { base, row1, row2, head } = sourceLog();

  // An edit this browser made, offline, in an earlier session -- durable in
  // IndexedDB but not yet folded into `doc`. Built from a second document
  // instance so it carries its own peer id, the same as a real hydration
  // import would.
  const offlineSource = createProjectSession({ send: () => {}, onState: () => {} });
  offlineSource.doc.import(base);
  const fromBase = offlineSource.doc.oplogVersion();
  offlineSource.textOf(offlineSource.mainId()).insert(5, " OFFLINE");
  offlineSource.doc.commit();
  const offlineEdit = offlineSource.doc.export({ mode: "update", from: fromBase });
  offlineSource.leave();

  const decode = (encoded) => Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0));

  let hydrationRan = false;
  const hydrationGate = {};
  hydrationGate.promise = new Promise((resolve) => { hydrationGate.resolve = resolve; });
  const persistence = {
    open(doc) {
      return {
        // The real persister's `hydration` promise only resolves once its
        // IndexedDB reads are imported; this stands in for exactly that
        // ordering without touching IndexedDB.
        hydration: hydrationGate.promise.then(() => {
          doc.import(offlineEdit);
          hydrationRan = true;
        }),
        close() {},
      };
    },
  };

  const sentF = [];
  const joiner = createProjectSession({
    send: (message) => sentF.push(message),
    onState: () => {},
    presenceId: () => "join-hydration-race",
    persistence,
  });
  try {
    // This browser already had the base from a prior session -- otherwise
    // the server would answer with `doc-state`, not a bare `doc-rows` -- so
    // only the two rows are missing.
    joiner.doc.import(base);

    const rowsPromise = joiner.rows({ protocol: "librepaper.room.v3", vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
    assert.equal(hydrationRan, false, "hydration has not resolved yet");
    assert.equal(sentF.filter((message) => message.type === "doc-update").length, 0,
      "no catch-up is sent while hydration is still in flight");

    // A local edit can happen while the join is waiting for IndexedDB. It is
    // sent immediately and must still be part of the later catch-up target.
    joiner.addText("during.md", "DURING");
    assert.equal(sentF.filter((message) => message.type === "doc-update").length, 1,
      "an edit during hydration is retained and sent");

    hydrationGate.resolve();
    await rowsPromise;

    assert.equal(hydrationRan, true, "rows waited for hydration before finishing the join");
    assert.equal(joiner.joined, true);
    const sent = sentF.filter((message) => message.type === "doc-update");
    assert.equal(sent.length, 2, "the edit during hydration and the later catch-up are both sent");

    // Replay what the server would have received against a copy of its own
    // log: the offline edit must be in there, not just the two rows.
    const verify = createProjectSession({ send: () => {}, onState: () => {} });
    verify.doc.import(base);
    verify.doc.import(row1);
    verify.doc.import(row2);
    for (const frame of sent) verify.doc.import(decode(frame.update));
    assert.ok(verify.textOf(verify.mainId()).toString().includes("OFFLINE"),
      "the catch-up export carries the offline edit hydration imported, not a snapshot taken before it landed");
    assert.ok(verify.list().some((file) => file.path === "during.md"),
      "the edit made while hydration was in flight reaches the catch-up");
    verify.leave();
  } finally {
    joiner.leave();
  }
}

// The same ordering hole, for a join whose base arrives by reference
// (§6.2 step 5): `doc-state` names the base and `doc-rows` follows as a
// second frame. `start` is still awaiting the referenced fetch when that
// second frame is handled, so `rows` must not import or catch up ahead of
// it -- `joinGate` is what lines the two up.
{
  const { base, row1, row2, head, text } = sourceLog();
  const referenceGate = {};
  referenceGate.promise = new Promise((resolve) => { referenceGate.resolve = resolve; });
  const order = [];

  const sentG = [];
  const joiner = createProjectSession({
    send: (message) => sentG.push(message),
    onState: () => {},
    presenceId: () => "join-ref-race",
    fetchReference: async (ref) => {
      order.push(`fetch:${ref}`);
      await referenceGate.promise;
      order.push("fetch-resolved");
      return base;
    },
  });
  try {
    const digest = await (async () => {
      // Matches `sha256Hex` in project-session.js closely enough for a test
      // digest: the server's actual digest algorithm is exercised elsewhere,
      // this only needs `start` to accept the fetched bytes.
      const hashed = await crypto.subtle.digest("SHA-256", base);
      return [...new Uint8Array(hashed)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
    })();

    const startPromise = joiner.start({
      protocol: "librepaper.room.v3",
      vector: encode(head.encode()),
      ref: "https://example.test/base",
      digest,
      updates: [],
    });
    const rowsPromise = joiner.rows({ protocol: "librepaper.room.v3", vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });

    // Checked before anything has had a chance to run past its first await:
    // unfixed code ran `rows` fully synchronously (no `joinGate` to queue
    // behind), so it would already have imported the rows and sent a
    // catch-up by this point, ahead of the base those rows build on.
    assert.equal(sentG.filter((message) => message.type === "doc-update").length, 0,
      "neither join step has sent anything while the referenced base is still downloading");
    assert.equal(joiner.text_(), "", "rows has not imported ahead of the base it depends on");

    // Let both steps run as far as they can without the fetch: `start`
    // reaches `fetchReference` and suspends there; `rows`, if it is correctly
    // queued behind `start`, has not started at all. A macrotask flushes
    // every pending microtask first, so this does not depend on knowing how
    // many `.then` hops `joinGate` adds.
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(order, ["fetch:https://example.test/base"],
      "rows made no progress while start's referenced-base fetch was still in flight");
    assert.equal(sentG.filter((message) => message.type === "doc-update").length, 0);

    referenceGate.resolve();
    await startPromise;
    await rowsPromise;

    assert.deepEqual(order, ["fetch:https://example.test/base", "fetch-resolved"]);
    assert.equal(joiner.text_(), text, "the base landed before the rows that build on it were imported");
    assert.equal(joiner.joined, true);
  } finally {
    joiner.leave();
  }
}

// `doc-gap` (§5 step 4): the server refused a batch because its start vector
// was not covered and dropped it rather than queueing it, so the client's
// answer is an export from the vector the server actually holds -- exactly
// what `catchUp` already does for a join, which is why `gap` is one line.
{
  const sentD = [];
  const editor = createProjectSession({ send: (message) => sentD.push(message), onState: () => {}, presenceId: () => "gap-editor" });
  try {
    const early = editor.doc.oplogVersion();
    editor.addText("a.md", "first");
    editor.addText("b.md", "second");
    sentD.length = 0;
    editor.gap(encode(early.encode()));
    assert.equal(sentD.length, 1, "a gap reply produces exactly one export, not one per refused batch");
    assert.equal(sentD[0].type, "doc-update");
    const expected = encode(editor.doc.export({ mode: "update", from: early }));
    assert.equal(sentD[0].update, expected, "the export starts at the vector the server named, not at whatever was pending before");
  } finally {
    editor.leave();
  }
}

// Acknowledgement clears a contiguous prefix of the unacknowledged map and
// nothing more (§6.3). It does not affect save pending, which follows durable
// coverage rather than the number of transport batches.
{
  const statesE = [];
  const sentE = [];
  const editor = createProjectSession({ send: (message) => sentE.push(message), onState: (state) => statesE.push(state), presenceId: () => "ack-editor" });
  try {
    editor.addText("one.md", "1");
    editor.addText("two.md", "2");
    editor.addText("three.md", "3");
    assert.equal(statesE.at(-1).pending, 1, "pending is save coverage, not a batch count");
    const [first, second, third] = sentE.filter((message) => message.type === "doc-update").map((message) => message.seq);

    editor.acknowledge(second);
    assert.equal(statesE.at(-1).pending, 1, "acking the second send clears the first two, which came before it");

    editor.acknowledge(second);
    assert.equal(statesE.at(-1).pending, 1, "acking the same boundary again changes nothing");

    editor.acknowledge(third);
    assert.equal(statesE.at(-1).pending, 1, "acking the last send still does not prove durability");
    editor.durable(encode(editor.doc.oplogVersion().encode()));
    assert.equal(statesE.at(-1).pending, 0, "durable coverage clears the save pending state");
    void first;
  } finally {
    editor.leave();
  }
}

// A reconnect can catch up from the server head while the server still has a
// buffered row. The empty catch-up acknowledgement retires transport state,
// but only a later durable vector may clear save pending.
{
  const states = [];
  const sent = [];
  const editor = createProjectSession({ send: (message) => sent.push(message), onState: (state) => states.push(state), presenceId: () => "durable-reconnect" });
  try {
    editor.addText("buffered.md", "buffered");
    const head = encode(editor.doc.oplogVersion().encode());
    assert.equal(states.at(-1).pending, 1);

    editor.disconnected();
    await editor.start({ protocol: "librepaper.room.v3", vector: head, durableVector: "", updates: [] });
    const catchup = sent.filter((message) => message.type === "doc-update").at(-1);
    editor.acknowledge(catchup.seq);
    assert.equal(states.at(-1).pending, 1, "an empty catch-up ack does not claim buffered work is durable");

    editor.durable(head);
    assert.equal(states.at(-1).pending, 0, "durable coverage clears the pending save");
  } finally {
    editor.leave();
  }
}

// A peer's later typing is not this browser's save target. Coverage of the
// earlier local vector is sufficient even while that later work is buffered.
{
  const states = [];
  const editor = createProjectSession({ send: () => {}, onState: (state) => states.push(state), presenceId: () => "target-editor" });
  const peer = createProjectSession({ send: () => {}, onState: () => {}, presenceId: () => "target-peer" });
  try {
    editor.addText("mine.md", "mine");
    const localTarget = encode(editor.doc.oplogVersion().encode());
    peer.addText("theirs.md", "theirs");
    editor.apply(encode(peer.doc.export({ mode: "update" })));
    const liveVector = encode(editor.doc.oplogVersion().encode());
    assert.notEqual(liveVector, localTarget, "the peer edit advanced the live document vector");

    editor.durable(localTarget);
    assert.equal(states.at(-1).pending, 0, "the local target remains coverable after a peer types later");
  } finally {
    editor.leave();
    peer.leave();
  }
}

// A reload whose local document is restored during hydration is conservatively
// pending until the server explicitly reports coverage for that restored
// vector. A normal join head or catch-up acknowledgement is not enough.
{
  const local = createProjectSession({ send: () => {}, onState: () => {}, presenceId: () => "hydrated-source" });
  const restoredId = local.addText("restored.md", "restored");
  const saved = local.doc.export({ mode: "update" });
  const savedVector = encode(local.doc.oplogVersion().encode());
  local.leave();
  const remote = createProjectSession({ send: () => {}, onState: () => {}, presenceId: () => "hydrated-peer" });
  const remoteId = remote.addText("peer.md", "peer");
  const remoteUpdate = encode(remote.doc.export({ mode: "update" }));

  const states = [];
  const reloaded = createProjectSession({
    send: () => {},
    onState: (state) => states.push(state),
    presenceId: () => "hydrated-reload",
    persistence: {
      open(doc, events) {
        return {
          hydration: Promise.resolve().then(() => {
            doc.import(saved);
            events.hydrated();
          }),
          close() {},
        };
      },
    },
  });
  try {
    // This arrives before local hydration finishes. It must wait until the
    // join's own state is imported, and it must not become the local target.
    reloaded.apply(remoteUpdate);
    await reloaded.start({ protocol: "librepaper.room.v3", vector: "", updates: [] });
    assert.equal(reloaded.textOf(restoredId).toString(), "restored");
    assert.equal(reloaded.textOf(remoteId).toString(), "peer", "the early relay is imported after hydration");
    assert.equal(states.at(-1).pending, 1, "restored local work waits for durable coverage");
    reloaded.acknowledge(999);
    assert.equal(states.at(-1).pending, 1);
    reloaded.durable(savedVector);
    assert.equal(states.at(-1).pending, 0);
  } finally {
    reloaded.leave();
    remote.leave();
  }
}

// Coverage delivered while a join is importing must not be regressed by the
// older durableVector carried on that join's frame.
{
  const states = [];
  const hydrationGate = {};
  hydrationGate.promise = new Promise((resolve) => { hydrationGate.resolve = resolve; });
  const editor = createProjectSession({
    send: () => {},
    onState: (state) => states.push(state),
    presenceId: () => "coverage-race",
    persistence: { open() { return { hydration: hydrationGate.promise, close() {} }; } },
  });
  try {
    editor.addText("old.md", "old");
    const older = encode(editor.doc.oplogVersion().encode());
    editor.addText("race.md", "race");
    const newer = encode(editor.doc.oplogVersion().encode());
    const joining = editor.start({ protocol: "librepaper.room.v3", vector: newer, durableVector: older, updates: [] });
    await new Promise((resolve) => setTimeout(resolve, 0));
    editor.durable(newer);
    assert.equal(states.at(-1).pending, 0);
    hydrationGate.resolve();
    await joining;
    assert.equal(states.at(-1).pending, 0, "an old join durableVector cannot regress newer coverage");
  } finally {
    editor.leave();
  }
}


// ---------------------------------------------------------------------------
// Join work outliving the connection it was scheduled on.
//
// A join is several awaits long: local hydration, a base fetched over HTTP,
// the digest that binds it. None of them stops the socket from dropping or
// the tab from closing underneath. Each of the checks below suspends the join
// at one of those awaits, ends the connection, and then releases the await
// with a perfectly valid answer -- which is the case that used to revive a
// session that had already been given up.
// ---------------------------------------------------------------------------

const digestOf = async (bytes) => {
  const hashed = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(hashed)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
};

const gate = () => {
  const held = {};
  held.promise = new Promise((resolve, reject) => { held.resolve = resolve; held.reject = reject; });
  return held;
};

// `leave()` while the referenced base is downloading. Releasing the fetch with
// the right bytes and the right digest must not join, report or send.
{
  const { base, head } = sourceLog();
  const held = gate();
  const sentL = [];
  const statesL = [];
  const joiner = createProjectSession({
    send: (message) => sentL.push(message),
    onState: (state) => statesL.push(state),
    presenceId: () => "leave-during-fetch",
    fetchReference: async () => { await held.promise; return base; },
  });
  const digest = await digestOf(base);
  const startPromise = joiner.start({
    protocol: "librepaper.room.v3", vector: encode(head.encode()),
    ref: "https://example.test/base", digest, updates: [],
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  joiner.leave();
  const reportsAtLeave = statesL.length;
  held.resolve();
  await startPromise;

  assert.equal(joiner.joined, false, "a session that was left does not come back joined");
  assert.equal(joiner.text_(), "", "the referenced base is not imported into a document nobody holds");
  assert.equal(statesL.length, reportsAtLeave, "a left session reports no state");
  assert.equal(sentL.filter((message) => message.type === "doc-update").length, 0,
    "a left session sends no catch-up");
  assert.equal(sentL.filter((message) => message.type === "doc-presence").length, 0,
    "a left session announces no presence");
}

// The same, for `disconnected()`: the socket went, so the answer is for a
// server this browser is no longer talking to.
{
  const { base, head } = sourceLog();
  const held = gate();
  const sentD = [];
  const joiner = createProjectSession({
    send: (message) => sentD.push(message),
    onState: () => {},
    presenceId: () => "disconnect-during-fetch",
    fetchReference: async () => { await held.promise; return base; },
  });
  try {
    const digest = await digestOf(base);
    const startPromise = joiner.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      ref: "https://example.test/base", digest, updates: [],
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    joiner.disconnected();
    held.resolve();
    await startPromise;

    assert.equal(joiner.joined, false, "a dropped socket's join does not revive itself");
    assert.equal(joiner.text_(), "", "the obsolete base is not imported");
    assert.equal(sentD.filter((message) => message.type === "doc-update").length, 0);
    assert.equal(sentD.filter((message) => message.type === "doc-presence").length, 0,
      "presence is not announced on a socket that has gone");
  } finally {
    joiner.leave();
  }
}

// A fresh join must not wait on the abandoned one. The first fetch is never
// released at all: the reconnect that replaces it has to complete anyway.
{
  const { base, row1, row2, head, text } = sourceLog();
  const never = gate();
  const sentF = [];
  let fetches = 0;
  const joiner = createProjectSession({
    send: (message) => sentF.push(message),
    onState: () => {},
    presenceId: () => "fresh-join-after-cancel",
    fetchReference: async () => { fetches += 1; await never.promise; return base; },
  });
  try {
    const digest = await digestOf(base);
    const abandoned = joiner.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      ref: "https://example.test/base", digest, updates: [],
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(fetches, 1);

    joiner.disconnected();
    // The new socket sends the base inline. Its rows follow as a second frame
    // and must still be ordered behind it.
    const startPromise = joiner.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      base: encode(base), updates: [],
    });
    const rowsPromise = joiner.rows({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      updates: [encode(row1), encode(row2)],
    });
    await startPromise;
    await rowsPromise;

    assert.equal(joiner.joined, true, "the reconnect joined without waiting for the abandoned fetch");
    assert.equal(joiner.text_(), text, "the rows landed on the base they build on, in order");
    assert.equal(fetches, 1, "the abandoned fetch was not retried, and is still in flight");
    // Abandoning it now must not disturb the join that replaced it.
    never.resolve();
    await abandoned;
    assert.equal(joiner.text_(), text);
    assert.equal(joiner.joined, true);
  } finally {
    joiner.leave();
  }
}

// A `doc-rows` frame queued behind a `doc-state` whose connection then drops
// is obsolete too: it was sent by the same server on the same socket.
{
  const { base, row1, row2, head } = sourceLog();
  const held = gate();
  const sentQ = [];
  const joiner = createProjectSession({
    send: (message) => sentQ.push(message),
    onState: () => {},
    presenceId: () => "queued-rows-obsolete",
    fetchReference: async () => { await held.promise; return base; },
  });
  try {
    const digest = await digestOf(base);
    const startPromise = joiner.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      ref: "https://example.test/base", digest, updates: [],
    });
    const rowsPromise = joiner.rows({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      updates: [encode(row1), encode(row2)],
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    joiner.disconnected();
    held.resolve();
    await startPromise;
    await rowsPromise;

    assert.equal(joiner.joined, false, "queued rows from a dropped socket do not join");
    assert.equal(joiner.text_(), "", "queued rows are not imported without the base they name");
    assert.equal(sentQ.filter((message) => message.type === "doc-update").length, 0);
  } finally {
    joiner.leave();
  }
}

// Failures are continuations too. A fetch that rejects after teardown must not
// throw at whoever is joining now, and a `restartBaseline` refusal must not
// re-open a document on a socket that has gone.
{
  const { head } = sourceLog();
  const held = gate();
  const sentE = [];
  const joiner = createProjectSession({
    send: (message) => sentE.push(message),
    onState: () => {},
    presenceId: () => "late-error",
    fetchReference: async () => { await held.promise; throw new Error("the base went away"); },
  });
  const startPromise = joiner.start({
    protocol: "librepaper.room.v3", vector: encode(head.encode()),
    ref: "https://example.test/base", digest: "0".repeat(64), updates: [],
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  joiner.leave();
  held.resolve();
  await startPromise;
  assert.equal(sentE.filter((message) => message.type === "doc-open").length, 0);
}
{
  const { head } = sourceLog();
  const held = gate();
  const sentR = [];
  const joiner = createProjectSession({
    send: (message) => sentR.push(message),
    onState: () => {},
    presenceId: () => "late-restart-baseline",
    fetchReference: async () => {
      await held.promise;
      const error = new Error("that baseline is gone");
      error.restartBaseline = true;
      throw error;
    },
  });
  try {
    const startPromise = joiner.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      ref: "https://example.test/base", digest: "0".repeat(64), updates: [],
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    joiner.disconnected();
    held.resolve();
    await startPromise;
    assert.equal(sentR.filter((message) => message.type === "doc-open").length, 0,
      "a refused baseline does not re-open the document on a socket that has gone");
  } finally {
    joiner.leave();
  }
}

// A digest that does not match is still an integrity failure for whoever is
// connected. On a connection that has gone it is nobody's failure: the check
// is skipped rather than raised at the join that replaced it.
{
  const { base, head } = sourceLog();
  const held = gate();
  const joiner = createProjectSession({
    send: () => {},
    onState: () => {},
    presenceId: () => "late-digest",
    fetchReference: async () => { await held.promise; return base; },
  });
  try {
    const startPromise = joiner.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      ref: "https://example.test/base", digest: "0".repeat(64), updates: [],
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    joiner.disconnected();
    held.resolve();
    await startPromise;
    assert.equal(joiner.text_(), "", "bytes that failed their digest are never imported");
  } finally {
    joiner.leave();
  }

  // The same frame on a live connection still rejects, so the check itself is
  // intact rather than merely unreachable.
  const live = gate();
  const strict = createProjectSession({
    send: () => {},
    onState: () => {},
    presenceId: () => "live-digest",
    fetchReference: async () => { await live.promise; return base; },
  });
  try {
    const startPromise = strict.start({
      protocol: "librepaper.room.v3", vector: encode(head.encode()),
      ref: "https://example.test/base", digest: "0".repeat(64), updates: [],
    });
    live.resolve();
    await assert.rejects(startPromise, /integrity check/);
  } finally {
    strict.leave();
  }
}

// Cancelling a join is not cancelling recoverable work. A disconnect during
// hydration still captures the local baseline, so an edit made offline is
// still waiting for durable coverage when the next socket arrives.
{
  const hydration = gate();
  const statesH = [];
  const sentH = [];
  const editor = createProjectSession({
    send: (message) => sentH.push(message),
    onState: (state) => statesH.push(state),
    presenceId: () => "offline-edit-survives-cancel",
    persistence: { open() { return { hydration: hydration.promise, close() {} }; } },
  });
  try {
    editor.addText("offline.md", "typed while the socket was down");
    const startPromise = editor.start({ protocol: "librepaper.room.v3", vector: "", updates: [] });
    editor.disconnected();
    hydration.resolve();
    await startPromise;
    assert.equal(editor.joined, false);
    assert.equal(statesH.at(-1).pending, 1, "the offline edit is still waiting for durable coverage");

    sentH.length = 0;
    await editor.start({ protocol: "librepaper.room.v3", vector: "", updates: [] });
    assert.equal(editor.joined, true);
    assert.equal(statesH.at(-1).pending, 1, "a join is not durability");
    assert.equal(sentH.filter((message) => message.type === "doc-update").length, 1,
      "the reconnect offered the offline edit exactly once");
    editor.durable(encode(editor.doc.oplogVersion().encode()));
    assert.equal(statesH.at(-1).pending, 0);
  } finally {
    editor.leave();
  }
}

// An outstanding local write is allowed to finish after the session is left:
// the bytes it puts on disk are what makes the next reload safe. What stops is
// the delivery to a UI that is no longer there.
{
  const statesP = [];
  let events;
  let closes = 0;
  const editor = createProjectSession({
    send: () => {},
    onState: (state) => statesP.push(state),
    presenceId: () => "late-persistence",
    persistence: { open(_doc, handlers) { events = handlers; return { close() { closes += 1; } }; } },
  });
  editor.addText("paper.md", "work");
  editor.leave();
  const reportsAtLeave = statesP.length;
  assert.equal(closes, 1);
  // The adapter is closed, but a write it had already started can still land.
  events.persisted();
  events.failed(new Error("quota full"));
  assert.equal(statesP.length, reportsAtLeave, "a left session delivers no persistence state");
}

// Frames that arrive after teardown are dropped rather than applied to a
// destroyed document.
{
  const { base } = sourceLog();
  const editor = createProjectSession({ send: () => {}, onState: () => {}, presenceId: () => "late-frames" });
  editor.leave();
  editor.apply(encode(base));
  editor.acknowledge(1);
  editor.durable("");
  assert.equal(editor.joined, false);
}

// SPEC-frugal §2: an update the server had no room to hold.
//
// The refusal is the whole failure this covers. The edit is in this browser's
// document and nowhere else; the socket is still up, so no reconnect will
// carry it; the server's head did not move, so no `doc-gap` will ask for it;
// and `subscribeLocalUpdates` fires on edits, not on refusals, so without
// something here it takes another keystroke -- which an editor who has
// stopped typing will not make.
{
  const { base, head } = sourceLog();
  const sentR = [];
  const statesR = [];
  // Timers are driven by hand rather than waited on: this is about whether a
  // retry is scheduled at all, and how long it waits, not about the clock.
  const timers = new Map();
  let nextTimer = 1;
  const pressed = createProjectSession({
    send: (message) => sentR.push(message),
    onState: (state) => statesR.push(state),
    presenceId: () => "pressure",
    setTimer: (run, delay) => {
      const id = nextTimer++;
      timers.set(id, { run, delay });
      return id;
    },
    clearTimer: (id) => timers.delete(id),
  });
  const fire = () => {
    const [id, timer] = [...timers.entries()].at(-1);
    timers.delete(id);
    timer.run();
    return timer.delay;
  };
  try {
    await pressed.start({ protocol: "librepaper.room.v3", vector: encode(head.encode()), base: encode(base), updates: [] });
    const headVector = encode(pressed.doc.oplogVersion().encode());
    sentR.length = 0;

    pressed.addText("refused.md", "work the server could not keep");
    const refused = sentR.filter((message) => message.type === "doc-update");
    assert.equal(refused.length, 1, "the edit went out once");
    const before = pressed.text_();

    // The server refuses it and names its head, which does not cover the edit.
    pressed.pressure(headVector);
    assert.equal(timers.size, 1, "a refusal with nothing scheduled after it is work that is never sent again");
    assert.equal(
      statesR.at(-1).pending, 1,
      "a refused edit is not durable, so it must still read as unsaved",
    );
    assert.equal(pressed.text_(), before, "the refused edit is still in this browser's document");
    assert.equal(
      sentR.filter((message) => message.type === "doc-update").length, 1,
      "nothing is resent before the timer fires: an immediate retry is the resend loop this avoids",
    );

    // A second refusal while a retry is already armed schedules nothing more.
    pressed.pressure(headVector);
    assert.equal(timers.size, 1, "refusals stacked into a queue of retries");

    const firstDelay = fire();
    assert.equal(firstDelay, 1000, "the first retry waits a second");
    const resent = sentR.filter((message) => message.type === "doc-update");
    assert.equal(resent.length, 2, "the retry did not resend anything");
    // The resend is an export from the server's head, so a peer that holds
    // only what the server held gets exactly the refused work out of it. That
    // is the claim worth making, and it is stronger than comparing bytes: it
    // says the resent update actually carries the edit.
    {
      const peer = createProjectSession({ send: () => {}, onState: () => {}, presenceId: () => "peer" });
      try {
        await peer.start({ protocol: "librepaper.room.v3", vector: encode(head.encode()), base: encode(base), updates: [] });
        peer.apply(resent.at(-1).update);
        assert.equal(
          peer.textOf(peer.idOf("refused.md"))?.toString() ?? "",
          "work the server could not keep",
          "the resent update did not carry the refused edit",
        );
      } finally {
        peer.leave();
      }
    }

    // Still under pressure. The wait doubles rather than repeating, so a long
    // outage is not answered by one client resending as fast as it can be
    // refused.
    pressed.pressure(headVector);
    assert.equal(fire(), 2000);
    pressed.pressure(headVector);
    assert.equal(fire(), 4000);

    // An acknowledgement must not cancel an armed retry. `doc-ack` names
    // batches the server took, and a flush of somebody else's work -- or of
    // this browser's own earlier work -- acknowledges this peer while the
    // refused edit is still refused. Cancelling on that left the one edit
    // that needed resending as the one edit nothing would resend, which is
    // exactly the failure this whole mechanism exists to prevent. It was
    // found by the integration test, not by reasoning, which is why it is
    // pinned here.
    pressed.pressure(headVector);
    assert.equal(timers.size, 1);
    pressed.acknowledge(resent.at(-1).seq);
    assert.equal(timers.size, 1, "an acknowledgement cancelled the resend of work it did not cover");
    // The armed retry keeps the time it was given; what the acknowledgement
    // resets is the wait the NEXT refusal starts from.
    assert.equal(fire(), 8000);
    pressed.pressure(headVector);
    assert.equal(fire(), 1000, "the backoff did not reset when the server started accepting again");

    // A gap recovery can save the refused edit before its timer fires.
    // Only durable coverage proves the resend is obsolete; an unrelated
    // acknowledgement above must still leave it armed.
    pressed.pressure(headVector);
    pressed.gap(headVector);
    const recovered = sentR.filter(message => message.type === "doc-update").at(-1);
    pressed.acknowledge(recovered.seq);
    assert.equal(timers.size, 1, "transport ack alone cannot cancel recovery");
    pressed.durable(encode(pressed.doc.oplogVersion().encode()));
    assert.equal(timers.size, 0, "durable work must not be resent from a stale refusal vector");

    // Partial durable coverage must advance the retry's starting vector
    // without cancelling recovery of a newer refused edit.
    const savedHead = encode(pressed.doc.oplogVersion().encode());
    pressed.addText("later.md", "new unsaved work");
    pressed.pressure(headVector);
    pressed.durable(savedHead);
    assert.equal(timers.size, 1, "partial durability must retain the timer");
    fire();
    const partial = sentR.filter(message => message.type === "doc-update").at(-1);
    const expected = pressed.doc.export({ mode: "update", from: pressed.doc.oplogVersion() });
    assert.ok(decode(partial.update).length > expected.length, "newer work remains in the retry");
    const onlyNew = pressed.doc.export({ mode: "update", from: VersionVector.decode(decode(savedHead)) });
    assert.equal(partial.update, encode(onlyNew), "retry must exclude the already durable prefix");

    // A socket that drops has its own catch-up on rejoin, so an armed retry
    // is dropped rather than firing into nothing.
    pressed.pressure(headVector);
    assert.equal(timers.size, 1);
    pressed.disconnected();
    assert.equal(timers.size, 0, "a retry armed for a socket that has gone would send into nothing");
  } finally {
    pressed.leave();
  }
  assert.equal(timers.size, 0, "leaving a session left a timer running");
}

// A reader never sends updates, so a refusal should not reach one -- but
// nothing here assumes that holds, the same as `doc-gap`.
{
  const reader = createProjectSession({
    send: () => { throw new Error("a reader must not send"); },
    onState: () => {},
    presenceId: () => "reader-pressure",
    mayEdit: false,
    setTimer: () => { throw new Error("a reader must not schedule a resend"); },
  });
  try {
    reader.pressure("");
  } finally {
    reader.leave();
  }
}

console.log("project-session: persistence, identity, swap, directory changes, protocol join shapes, doc-gap, acknowledgement boundaries, pressure retry and join cancellation passed");
