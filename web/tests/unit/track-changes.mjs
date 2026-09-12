import test from "node:test";
import assert from "node:assert/strict";
import * as Y from "yjs";
import { createRevisionController } from "../../src/lib/track-changes.js";

function setup() {
  const doc = new Y.Doc();
  const files = doc.getMap("files");
  const text = new Y.Text("hello");
  files.set("f", text);
  const controller = createRevisionController({
    doc,
    author: "alice",
    documentId: "doc",
    fileOf: () => "main.qmd",
    textOf: () => text,
  });
  return { doc, text, controller };
}

test("captures an insertion with an anchored record", () => {
  const { text, controller } = setup();
  controller.setEnabled(true);
  text.insert(5, " world");
  const [record] = controller.pending();
  assert.equal(record.kind, "insert");
  assert.equal(record.after, " world");
  assert.equal(typeof record.start, "string");
  assert.equal(controller.position(record), 5);
});
test("captures deleted text from the pre-transaction snapshot", () => {
  const { text, controller } = setup();
  controller.setEnabled(true);
  text.delete(1, 2);
  const [record] = controller.pending();
  assert.equal(record.kind, "delete");
  assert.equal(record.before, "el");
});

test("tracking off preserves text without creating a revision", () => {
  const { text, controller } = setup();
  text.insert(0, "x");
  assert.equal(controller.pending().length, 0);
  controller.setEnabled(true);
  controller.setEnabled(false);
  text.insert(0, "y");
  assert.equal(controller.pending().length, 0);
});
