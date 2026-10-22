import assert from "node:assert/strict";
import test from "node:test";
import { EditorState } from "@codemirror/state";
import { LoroDoc } from "loro-crdt";
import { healLineEndings, toLf } from "../../src/lib/line-endings.js";
import { createProjectSession } from "../../src/lib/project-session.js";

test("toLf turns CRLF into LF", () => {
  assert.equal(toLf("a\r\nb\r\n"), "a\nb\n");
});

test("toLf turns a lone CR into LF", () => {
  assert.equal(toLf("a\rb"), "a\nb");
});

test("toLf handles a mix in one string", () => {
  assert.equal(toLf("a\r\nb\rc\nd"), "a\nb\nc\nd");
});

test("toLf returns an LF-only string unchanged", () => {
  assert.equal(toLf("a\nb\n"), "a\nb\n");
});

test("healLineEndings rewrites to LF and keeps cursors on their characters", () => {
  const doc = new LoroDoc();
  const text = doc.getText("t");
  text.insert(0, "a\r\nb\r\nc");
  doc.commit();
  const cursor = text.getCursor(6);
  assert.equal(healLineEndings(text), true);
  doc.commit();
  assert.equal(text.toString(), "a\nb\nc");
  assert.equal(doc.getCursorPos(cursor).offset, 4);
});

test("healLineEndings leaves an LF-only text untouched", () => {
  const doc = new LoroDoc();
  const text = doc.getText("t");
  text.insert(0, "a\nb\nc");
  doc.commit();
  const before = text.toString();
  assert.equal(healLineEndings(text), false);
  assert.equal(text.toString(), before);
});

test("addText stores LF line endings", () => {
  const session = createProjectSession({ send: () => {}, onState: () => {} });
  const id = session.addText("a.tex", "x\r\ny\rz");
  assert.equal(session.textOf(id).toString(), "x\ny\nz");
});

test("a healed body keeps view and text offsets aligned", () => {
  const doc = new LoroDoc();
  const text = doc.getText("t");
  text.insert(0, ["one", "two", "three", "four"].join("\r\n"));
  doc.commit();
  healLineEndings(text);
  doc.commit();
  const state = EditorState.create({ doc: text.toString() });
  assert.equal(state.doc.length, text.length);
  const line = state.doc.line(3);
  const tr = state.update({ changes: { from: line.from, to: line.to } });
  tr.changes.iterChanges((fromA, toA) => text.delete(fromA, toA - fromA));
  doc.commit();
  assert.equal(text.toString(), "one\ntwo\n\nfour");
});

// The rewrite of CRLF only deletes, so two editors that rewrite the same body
// at the same time converge on one line feed. A lone CR needs an insert, and
// two peers inserting at the same place keep both; that case is left to a
// single writer and is not covered here.
test("two editors that rewrite CRLF at the same time converge on one line feed", () => {
  const first = new LoroDoc();
  first.setPeerId(1);
  const text = first.getText("t");
  text.insert(0, "a\r\nb\r\nc");
  first.commit();
  const second = new LoroDoc();
  second.setPeerId(2);
  second.import(first.export({ mode: "snapshot" }));
  assert.equal(healLineEndings(text), true);
  first.commit();
  assert.equal(healLineEndings(second.getText("t")), true);
  second.commit();
  first.import(second.export({ mode: "update" }));
  second.import(first.export({ mode: "update" }));
  assert.equal(text.toString(), "a\nb\nc");
  assert.equal(second.getText("t").toString(), "a\nb\nc");
});
