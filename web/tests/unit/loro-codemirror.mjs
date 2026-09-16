// The vendored loro-codemirror binding, driven without a browser.
//
// Every fault the fork fixes is in the two plugin values' private handling of
// events and updates, so that is what these exercise: a `LoroSyncPluginValue`
// and an `UndoPluginValue` on a document shaped like ours -- a map of files,
// with the editor bound to one text in it -- with a peer document supplying
// the imports a collaborator would.
//
// `EditorView` needs a DOM, and the plugins need only three things from it:
// `state`, `dispatch`, and to be told about each update. The stand-in below
// builds real transactions from real states and hands the plugins an object
// with the fields of a `ViewUpdate` they read. What it does not model is the
// DOM, which the plugins under test never touch.

import assert from "node:assert/strict";
import { test } from "node:test";
import { ChangeSet, EditorState, Transaction } from "@codemirror/state";
import { LoroDoc, LoroText, UndoManager } from "loro-crdt";
import { LoroSyncPluginValue, loroSyncAnnotation } from "../../vendor/loro-codemirror/sync.ts";
import { UndoPluginValue, redo, undo, undoManagerStateField } from "../../vendor/loro-codemirror/undo.ts";

class View {
  constructor(state) {
    this.state = state;
    this.plugins = [];
    this.updates = [];
    this.queue = null;
  }

  attach(plugin) {
    this.plugins.push(plugin);
    return plugin;
  }

  dispatch(spec) {
    const transaction = spec instanceof Transaction ? spec : this.state.update(spec);
    this.state = transaction.state;
    if (this.queue) {
      this.queue.push(transaction);
      return;
    }
    this.apply([transaction]);
  }

  // Several transactions dispatched together arrive as one ViewUpdate, which
  // is what `EditorView.dispatch(tr1, tr2)` does. Dispatches made inside
  // `run` are held and applied as one update afterwards.
  together(run) {
    this.queue = [];
    run();
    const transactions = this.queue;
    this.queue = null;
    this.apply(transactions);
  }

  apply(transactions) {
    const startState = transactions[0].startState;
    let changes = ChangeSet.empty(startState.doc.length);
    for (const transaction of transactions) changes = changes.compose(transaction.changes);
    const update = {
      view: this,
      startState,
      state: this.state,
      transactions,
      changes,
      docChanged: transactions.some((transaction) => transaction.docChanged),
      selectionSet: transactions.some((transaction) => transaction.selection !== undefined),
    };
    this.updates.push(update);
    for (const plugin of this.plugins) plugin.update(update);
  }

  get text() {
    return this.state.doc.toString();
  }
}

const getText = (doc) => doc.getMap("files").get("main.md");

// A peer holds the original; ours is a snapshot of it, the way a browser
// joins a session. Loro delivers events synchronously, so nothing here waits
// on them; the one await is the binding's own initial check, which it defers
// to a microtask.
const setup = async ({ initial } = {}) => {
  const peer = new LoroDoc();
  peer.setPeerId(1);
  const peerText = peer.getMap("files").setContainer("main.md", new LoroText());
  peerText.insert(0, "hello");
  peer.commit();

  const doc = new LoroDoc();
  doc.setPeerId(2);
  doc.import(peer.export({ mode: "snapshot" }));
  const batches = [];
  doc.subscribe((batch) => batches.push(batch));

  const manager = new UndoManager(doc, { excludeOriginPrefixes: ["directory"] });
  const view = new View(EditorState.create({
    doc: initial ?? getText(doc).toString(),
    extensions: [undoManagerStateField.init(() => manager)],
  }));
  const sync = view.attach(new LoroSyncPluginValue(view, doc, getText));
  view.attach(new UndoPluginValue(view, doc, manager, getText));
  await Promise.resolve();

  const push = () => doc.import(peer.export({ mode: "update", from: doc.oplogVersion() }));
  const inStep = () => {
    assert.equal(view.text, getText(doc).toString(), "view and document differ");
  };
  return { peer, peerText, doc, batches, manager, view, sync, push, inStep };
};

const kinds = (batch) => batch.events.map((event) => event.diff.type);

test("a local edit is written to the document, once", async () => {
  const { doc, view, batches, inStep } = await setup();
  view.dispatch({ changes: { from: 5, insert: "!" } });
  assert.equal(getText(doc).toString(), "hello!");
  view.dispatch({ changes: { from: 1, to: 3, insert: "EL" } });
  assert.equal(getText(doc).toString(), "hELlo!");
  assert.equal(batches.filter((batch) => batch.by === "local").length, 2);
  inStep();
});

test("a remote edit reaches the view when the batch carries a map event first", async () => {
  const { peer, peerText, view, batches, push, inStep } = await setup();
  // One import: a file added to the map, and an edit to the text. This is the
  // shape of nearly every update in a document that is a map of files, and
  // the map event comes first.
  peer.getMap("files").set("other.md", "x");
  peerText.insert(5, " world");
  peerText.delete(0, 1);
  peer.commit();
  push();
  const batch = batches.at(-1);
  assert.equal(batch.by, "import");
  assert.deepEqual(kinds(batch), ["map", "text"]);
  assert.equal(view.text, "ello world");
  inStep();
});

test("a remote edit to another text is not applied to this view", async () => {
  const { peer, view, push, inStep } = await setup();
  peer.getMap("files").setContainer("other.md", new LoroText()).insert(0, "elsewhere");
  peer.commit();
  push();
  assert.equal(view.text, "hello");
  inStep();
});

test("undo reaches the view when the batch carries a map event first", async () => {
  const { doc, view, batches, inStep } = await setup();
  assert.equal(undo(view), false, "nothing to undo yet, so the key should fall through");

  // The edit and a map change land in one undo step, so the undo batch has
  // the map event ahead of the text event, which is where the old loop bailed.
  view.dispatch({ changes: { from: 5, insert: "!" } });
  doc.getMap("files").set("other.md", "x");
  doc.commit();

  assert.equal(undo(view), true);
  const batch = batches.at(-1);
  assert.equal(batch.origin, "undo");
  assert.deepEqual(kinds(batch), ["map", "text"]);
  assert.equal(view.text, "hello");
  inStep();

  assert.equal(redo(view), true);
  assert.equal(view.text, "hello!");
  inStep();
  assert.equal(redo(view), false);
});

test("undo takes back only this peer's edits", async () => {
  const { peer, peerText, view, push, inStep } = await setup();
  view.dispatch({ changes: { from: 5, insert: "!" } });
  peerText.insert(0, "R");
  peer.commit();
  push();
  assert.equal(view.text, "Rhello!");
  assert.equal(undo(view), true);
  assert.equal(view.text, "Rhello");
  inStep();
});

test("a sync write and a user edit in one update: the edit is written once", async () => {
  const { peer, peerText, doc, view, push, inStep } = await setup();
  view.together(() => {
    peerText.insert(0, "R");
    peer.commit();
    push();
    assert.equal(view.text, "Rhello", "the import's dispatch was made against the view");
    view.dispatch({ changes: { from: 6, insert: "U" } });
  });
  const update = view.updates.at(-1);
  assert.equal(update.transactions.length, 2);
  assert.equal(update.transactions[0].annotation(loroSyncAnnotation) !== undefined, true);
  assert.equal(view.text, "RhelloU");
  assert.equal(getText(doc).toString(), "RhelloU");
  inStep();
});

test("the initial check: equal texts dispatch nothing, and the first edit still lands", async () => {
  const { doc, view, batches, inStep } = await setup();
  assert.equal(view.updates.length, 0, "no seeding dispatch when the texts already agree");
  view.dispatch({ changes: { from: 0, insert: ">" } });
  assert.equal(getText(doc).toString(), ">hello");
  assert.equal(batches.filter((batch) => batch.by === "local").length, 1);
  inStep();
});

test("the initial check: a differing view is replaced from the document, not written back", async () => {
  const { doc, view, batches, inStep } = await setup({ initial: "stale" });
  assert.equal(view.text, "hello");
  assert.equal(getText(doc).toString(), "hello");
  assert.equal(batches.filter((batch) => batch.by === "local").length, 0);
  inStep();
});
