import assert from "node:assert/strict";
import * as Y from "yjs";
import { join } from "../../src/lib/collab.js";
import { needsSourceRefresh } from "../../src/lib/reader/source-events.js";

const session = join({ send: () => {}, mayEdit: true });
const document = session.files.doc;
const assets = document.getMap("assets");
const main = new Y.Text("main\n");
session.files.set("main", main);
session.paths.set("main", "main.qmd");
session.meta.set("main", "main");
const included = new Y.Text("included\n");
const seenTransactions = new WeakSet();
let sourceChanges = 0;
session.watchSource(() => sourceChanges++);
session.onFiles((events) => {
  if (needsSourceRefresh(events, session.text, seenTransactions)) sourceChanges++;
});

// Ignore setup notifications; every assertion below is one logical Yjs change.
sourceChanges = 0;
document.transact(() => {
  session.files.set("included", included);
  session.paths.set("included", "included.qmd");
});
assert.equal(sourceChanges, 1, "structural changes repaint once");

sourceChanges = 0;
main.insert(main.length, "next\n");
assert.equal(sourceChanges, 1, "main text changes use the source watcher once");

sourceChanges = 0;
included.insert(included.length, "next\n");
assert.equal(sourceChanges, 1, "included text changes repaint once");

sourceChanges = 0;
document.transact(() => {
  main.insert(main.length, "mixed main\n");
  included.insert(included.length, "mixed included\n");
});
assert.equal(sourceChanges, 1, "mixed main and included changes repaint once");

sourceChanges = 0;
document.transact(() => assets.set("fig/plot.png", "a".repeat(64)));
assert.equal(sourceChanges, 1, "asset structure changes repaint once");

document.destroy();
console.log("reader source event: Yjs main, included, mixed, and structural changes repaint once");
