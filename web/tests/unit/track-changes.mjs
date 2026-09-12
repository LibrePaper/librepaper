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
    send: () => ({ ok: true }),
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

test("publishes tracked text and revision metadata in one Yjs update", () => {
  const { doc, text, controller } = setup();
  controller.setEnabled(true);
  const base = Y.encodeStateAsUpdate(doc);
  const updates = [];
  doc.on("update", (update) => updates.push(update));
  controller.capture("f", [{ from: 5, to: 5, insert: "!" }], () => text.insert(5, "!"), { userEvent: "input.type" });
  assert.equal(updates.length, 1);
  const peer = new Y.Doc();
  Y.applyUpdate(peer, base);
  Y.applyUpdate(peer, updates[0]);
  assert.equal(peer.getMap("files").get("f").toString(), "hello!");
  assert.equal(peer.getMap("revisions").size, 1);
});
test("captures deleted text from the pre-transaction snapshot", () => {
  const { text, controller } = setup();
  controller.setEnabled(true);
  text.delete(1, 2);
  const [record] = controller.pending();
  assert.equal(record.kind, "delete");
  assert.equal(record.before, "el");
});

test("captures a selection replacement as one review unit", () => {
  const { doc, text, controller } = setup();
  controller.setEnabled(true);
  doc.transact(() => {
    text.delete(0, 5);
    text.insert(0, "goodbye");
  });
  const records = controller.pending();
  assert.equal(records.length, 1);
  assert.equal(records[0].kind, "replace");
  assert.equal(records[0].before, "hello");
  assert.equal(records[0].after, "goodbye");
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

test("deleting an author's pending insertion cancels its revision", () => {
  const { text, controller } = setup();
  controller.setEnabled(true);
  text.insert(5, " world");
  text.delete(5, 6);
  assert.equal(text.toString(), "hello");
  assert.equal(controller.pending().length, 0);
});

test("editing a pending insertion while tracking is off updates the proposal", () => {
  const { text, controller } = setup();
  controller.setEnabled(true);
  text.insert(5, " world");
  controller.setEnabled(false);
  text.delete(6, 5);
  text.insert(6, "there");
  const [record] = controller.pending();
  assert.equal(record.before, "");
  assert.equal(record.after, " there");
});

test("decision transport consumes generic revision errors", async () => {
  const { text, controller } = setup();
  controller.setEnabled(true);
  text.insert(5, "!");
  const [record] = controller.pending();
  const request = controller.decide(record.id, "accept", "request-1");
  assert.equal(controller.receive({ type: "error", request_id: "request-1", revision_id: record.id, message: "not allowed" }), true);
  await assert.rejects(request, /not allowed/);
});
