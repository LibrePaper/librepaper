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
import { EphemeralStore, LoroDoc, LoroText, UndoManager } from "loro-crdt";
import { LoroSyncPluginValue, loroSyncAnnotation } from "../../vendor/loro-codemirror/sync.ts";
import { UndoPluginValue } from "../../vendor/loro-codemirror/undo.ts";
import {
  EphemeralPlugin,
  ephemeralEffect,
  ephemeralStateField,
  getCursorEffect,
} from "../../vendor/loro-codemirror/ephemeral.ts";
// `undo` and `redo` are ours, not the binding's -- see lib/loro-undo.js.
import { redo, undo, undoManagerField } from "../../src/lib/loro-undo.js";

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
    extensions: [undoManagerField.init(() => manager)],
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

test("the initial check: a plugin destroyed before it runs writes nothing", async () => {
  // The editor swaps one file's state for another's with `setState`, which
  // destroys the first file's plugin. When the swap comes in the same tick the
  // view was built, the first plugin's deferred check must not run: it would
  // put the first file's text into the view now showing the second, and the
  // second file's plugin would write that into its own text.
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  files.setContainer("main.md", new LoroText()).insert(0, "main text");
  files.setContainer("notes.md", new LoroText()).insert(0, "notes text");
  doc.commit();
  const batches = [];
  doc.subscribe((batch) => batches.push(batch));
  const textOf = (path) => (document_) => document_.getMap("files").get(path);

  const view = new View(EditorState.create({ doc: "main text" }));
  const first = view.attach(new LoroSyncPluginValue(view, doc, textOf("main.md")));
  first.destroy();
  view.plugins = [];
  view.state = EditorState.create({ doc: "notes text" });
  view.attach(new LoroSyncPluginValue(view, doc, textOf("notes.md")));
  await Promise.resolve();
  await Promise.resolve();

  assert.equal(view.text, "notes text");
  assert.equal(textOf("notes.md")(doc).toString(), "notes text");
  assert.equal(batches.filter((batch) => batch.by === "local").length, 0);
});

// Remote carets. The plugin's ephemeral handling is what the fork fixes, so
// these drive `EphemeralPlugin` on the same fake View as above, with a second
// `EphemeralStore` standing in for the peer's.
const tick = () => new Promise((resolve) => setTimeout(resolve, 5));
const mainText = (doc) => doc.getMap("files").get("main.md");

test("a remote caret stays on its characters when text before it is edited", () => {
  const state = EditorState.create({ doc: "hello", extensions: [ephemeralStateField] });
  const withCaret = state.update({
    effects: ephemeralEffect.of({ type: "cursor", peer: "1", cursor: { anchor: 5, head: 2 } }),
  }).state;
  const edited = withCaret.update({ changes: { from: 0, insert: "XY" } }).state;
  assert.deepEqual(edited.field(ephemeralStateField).remoteCursors.get("1"), { anchor: 7, head: 4 });
});

test("a remote caret sent with an edit takes its own offsets, not the mapped ones", () => {
  const state = EditorState.create({ doc: "hello", extensions: [ephemeralStateField] });
  const next = state.update({
    changes: { from: 0, insert: "XY" },
    effects: ephemeralEffect.of({ type: "cursor", peer: "1", cursor: { anchor: 9, head: 9 } }),
  }).state;
  assert.deepEqual(next.field(ephemeralStateField).remoteCursors.get("1"), { anchor: 9, head: 9 });
});

test("a remote caret in another text of the document resolves to a delete", () => {
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const main = files.setContainer("main.md", new LoroText());
  const notes = files.setContainer("notes.md", new LoroText());
  main.insert(0, "hello");
  notes.insert(0, "other text");
  doc.commit();

  const here = { anchor: main.getCursor(3).encode() };
  const elsewhere = { anchor: notes.getCursor(3).encode() };
  assert.deepEqual(getCursorEffect(doc, main, "1", here).value, {
    type: "cursor",
    peer: "1",
    cursor: { anchor: 3, head: 3 },
  });
  assert.deepEqual(getCursorEffect(doc, main, "1", elsewhere).value, { type: "delete", peer: "1" });
});

test("a remote caret that arrives before its edit is resolved again when the edit lands", async (t) => {
  const peer = new LoroDoc();
  peer.setPeerId(1);
  const peerText = peer.getMap("files").setContainer("main.md", new LoroText());
  peerText.insert(0, "hello");
  peer.commit();
  const doc = new LoroDoc();
  doc.setPeerId(2);
  doc.import(peer.export({ mode: "snapshot" }));

  const ours = new EphemeralStore();
  const theirs = new EphemeralStore();
  const view = new View(EditorState.create({ doc: "hello", extensions: [ephemeralStateField] }));
  const plugin = view.attach(new EphemeralPlugin(view, doc, { name: "Ada", colorClassName: "c1" }, ours, mainText));
  t.after(() => {
    plugin.destroy();
    ours.destroy();
    theirs.destroy();
  });

  // The peer's caret after "hello" arrives and resolves against our copy.
  theirs.set("1-cm-cursor", { anchor: peerText.getCursor(5).encode() });
  ours.apply(theirs.encodeAll());
  await tick();
  assert.equal(view.state.field(ephemeralStateField).remoteCursors.get("1").anchor, 5);

  // The peer types "XY" and moves its caret after it. The caret reaches us
  // before the edit does, so it cannot resolve past "hello" yet.
  peerText.insert(5, "XY");
  peer.commit();
  theirs.set("1-cm-cursor", { anchor: peerText.getCursor(7).encode() });
  ours.apply(theirs.encodeAll());
  await tick();

  // The edit lands: the caret is resolved again, after "XY".
  doc.import(peer.export({ mode: "update", from: doc.oplogVersion() }));
  await tick();
  assert.equal(view.state.field(ephemeralStateField).remoteCursors.get("1").anchor, 7);
});

test("a peer's first caret, attached to characters not yet imported, is stored once they land", async (t) => {
  const peer = new LoroDoc();
  peer.setPeerId(1);
  const peerText = peer.getMap("files").setContainer("main.md", new LoroText());
  peerText.insert(0, "hello");
  peer.commit();
  const doc = new LoroDoc();
  doc.setPeerId(2);
  doc.import(peer.export({ mode: "snapshot" }));

  const ours = new EphemeralStore();
  const theirs = new EphemeralStore();
  const view = new View(EditorState.create({ doc: "hello", extensions: [ephemeralStateField] }));
  const plugin = view.attach(new EphemeralPlugin(view, doc, { name: "Ada", colorClassName: "c1" }, ours, mainText));
  t.after(() => {
    plugin.destroy();
    ours.destroy();
    theirs.destroy();
  });

  // The peer types "XY" after "hello" and its caret sits between X and Y.
  // The caret reaches us first: nothing in our copy resolves it, so nothing is stored.
  peerText.insert(5, "XY");
  peer.commit();
  theirs.set("1-cm-cursor", { anchor: peerText.getCursor(6).encode() });
  ours.apply(theirs.encodeAll());
  await tick();
  assert.equal(view.state.field(ephemeralStateField).remoteCursors.has("1"), false);

  // The edit lands: the caret is stored at 6, between X and Y.
  doc.import(peer.export({ mode: "update", from: doc.oplogVersion() }));
  await tick();
  assert.equal(view.state.field(ephemeralStateField).remoteCursors.get("1").anchor, 6);
});

test("creating a doc-changing transaction does not move the carets of its start state", () => {
  const s1 = EditorState.create({ doc: "hello world", extensions: [ephemeralStateField] }).update({
    effects: ephemeralEffect.of({ type: "cursor", peer: "7", cursor: { anchor: 6 } }),
  }).state;
  const next = s1.update({ changes: { from: 0, insert: "XX" } }).state;
  assert.equal(s1.field(ephemeralStateField).remoteCursors.get("7").anchor, 6);
  assert.equal(next.field(ephemeralStateField).remoteCursors.get("7").anchor, 8);
});
