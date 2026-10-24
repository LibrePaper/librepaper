import assert from "node:assert/strict";
import { LoroText } from "loro-crdt";
import { join } from "../../src/lib/collab.js";
import { needsSourceRefresh } from "../../src/lib/reader/source-events.js";

const session = join({ send: () => {}, mayEdit: true });
const document = session.doc;
const assets = document.getMap("assets");
let first = new LoroText();
first.insert(0, "first\n");
session.files.setContainer("first", first);
session.paths.set("first", "first.qmd");
document.commit();
first = session.textOf("first");
let included = new LoroText();
included.insert(0, "included\n");
let sourceChanges = 0;
session.onFiles((events) => {
  if (needsSourceRefresh(events)) sourceChanges++;
});

// Ignore setup notifications; every assertion below is one logical Loro change.
sourceChanges = 0;
session.files.setContainer("included", included);
session.paths.set("included", "included.qmd");
document.commit();
included = session.textOf("included");
assert.equal(sourceChanges, 1, "structural changes repaint once");

sourceChanges = 0;
first.insert(first.length, "next\n");
document.commit();
assert.equal(sourceChanges, 1, "a text change repaints once");

sourceChanges = 0;
included.insert(included.length, "next\n");
document.commit();
assert.equal(sourceChanges, 1, "included text changes repaint once");

sourceChanges = 0;
first.insert(first.length, "mixed first\n");
included.insert(included.length, "mixed included\n");
document.commit();
assert.equal(sourceChanges, 1, "mixed changes to two texts repaint once");

sourceChanges = 0;
assets.set("fig/plot.png", "a".repeat(64));
document.commit();
assert.equal(sourceChanges, 1, "asset structure changes repaint once");

console.log("reader source event: Loro text, mixed, and structural changes repaint once");
