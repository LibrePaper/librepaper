import assert from "node:assert/strict";
import { createProjectSession } from "../../src/lib/project-session.js";
import { assertSameProject, projectIdentity, sameProject } from "../../src/lib/project-identity.js";

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
  // §6.3: acknowledgement clears the in-memory map; there is no coverage
  // argument to carry any more, since nothing is persisted per batch.
  session.acknowledge(sent.at(-1).seq);
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
    max_path: 200, max_files: 200, max_document: 1000000, max_assets: 1000000, max_asset: 500000 };
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

// `librepaper.room.v2`: join and gap handling (§6.2, §5 step 4, §14.2). A
// base64 helper matching the wire encoding `project-session.js` uses
// internally, since that function is not exported.
const encode = (bytes) => {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
};

// A protocol string other than the one this client speaks is refused before
// any update is accepted (§6.1): there is no compatibility path to fall
// back to.
{
  const refused = createProjectSession({ send: () => {}, onState: () => {} });
  try {
    await assert.rejects(
      () => refused.start({ protocol: "librepaper.room.v1", vector: "", updates: [] }),
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
    await joiner.start({ protocol: "librepaper.room.v2", vector: encode(head.encode()), base: encode(base), updates: [] });
    assert.equal(joiner.joined, true, "doc-state is a valid join reply on its own, so this browser is announced already");
    await joiner.rows({ vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
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
    await joiner.rows({ vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
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
    await joiner.rows({ vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
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

    const rowsPromise = joiner.rows({ vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });
    assert.equal(hydrationRan, false, "hydration has not resolved yet");
    assert.equal(sentF.filter((message) => message.type === "doc-update").length, 0,
      "no catch-up is sent while hydration is still in flight");

    hydrationGate.resolve();
    await rowsPromise;

    assert.equal(hydrationRan, true, "rows waited for hydration before finishing the join");
    assert.equal(joiner.joined, true);
    const sent = sentF.filter((message) => message.type === "doc-update");
    assert.equal(sent.length, 1, "rows sends exactly one catch-up, once hydration and the rows import have both landed");

    // Replay what the server would have received against a copy of its own
    // log: the offline edit must be in there, not just the two rows.
    const verify = createProjectSession({ send: () => {}, onState: () => {} });
    verify.doc.import(base);
    verify.doc.import(row1);
    verify.doc.import(row2);
    verify.doc.import(decode(sent[0].update));
    assert.ok(verify.textOf(verify.mainId()).toString().includes("OFFLINE"),
      "the catch-up export carries the offline edit hydration imported, not a snapshot taken before it landed");
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
      protocol: "librepaper.room.v2",
      vector: encode(head.encode()),
      ref: "https://example.test/base",
      digest,
      updates: [],
    });
    const rowsPromise = joiner.rows({ vector: encode(head.encode()), updates: [encode(row1), encode(row2)] });

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
// nothing more (§6.3): three local edits queue three sends, and acking the
// second by its transport sequence clears the first two and leaves the
// third pending, exactly as `doc-ack` naming a peer's highest `client_seq`
// in a flushed row is documented to mean (§5.1).
{
  const statesE = [];
  const sentE = [];
  const editor = createProjectSession({ send: (message) => sentE.push(message), onState: (state) => statesE.push(state), presenceId: () => "ack-editor" });
  try {
    editor.addText("one.md", "1");
    editor.addText("two.md", "2");
    editor.addText("three.md", "3");
    assert.equal(statesE.at(-1).pending, 3);
    const [first, second, third] = sentE.filter((message) => message.type === "doc-update").map((message) => message.seq);

    editor.acknowledge(second);
    assert.equal(statesE.at(-1).pending, 1, "acking the second send clears the first two, which came before it");

    editor.acknowledge(second);
    assert.equal(statesE.at(-1).pending, 1, "acking the same boundary again changes nothing");

    editor.acknowledge(third);
    assert.equal(statesE.at(-1).pending, 0, "acking the last send clears what remains");
    void first;
  } finally {
    editor.leave();
  }
}

console.log("project-session: persistence, identity, swap, directory changes, protocol join shapes, doc-gap and acknowledgement boundaries passed");
