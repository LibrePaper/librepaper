// cursorAt against a real LoroText, on the positions where loro-crdt 1.16.1
// answers no cursor: the first UTF-16 unit of an astral character that began
// an insertion of its own, which is what typing an emoji does.
import assert from "node:assert/strict";
import test from "node:test";
import { LoroDoc } from "loro-crdt";
import { cursorAt } from "../../src/lib/loro-cursor.js";

function make(body) {
  const doc = new LoroDoc();
  const text = doc.getText("t");
  text.insert(0, body);
  doc.commit();
  return { doc, text };
}

/// The example as typing builds it: the text first, then the emoji at 4 as
/// an edit of its own. This is the shape that loses the cursor.
function typed() {
  const { doc, text } = make("café ☕ naïve");
  text.insert(4, "😀");
  doc.commit();
  return { doc, text };
}

const posOf = (doc, cursor) => {
  const p = doc.getCursorPos(cursor);
  return { offset: p.offset, side: p.side };
};

const EXAMPLE = "café😀 ☕ naïve";

test("Loro itself answers no cursor at the first unit of an astral character", () => {
  const { text } = typed();
  assert.equal(text.toString(), EXAMPLE);
  // If this starts failing, Loro changed and the fallback in cursorAt can be reviewed.
  assert.equal(text.getCursor(4, -1), undefined);
  // The same text inserted in one piece does answer.
  assert.ok(make(EXAMPLE).text.getCursor(4, -1));
});

test("the measured table, through cursorAt", () => {
  const { doc, text } = typed();
  for (const side of [-1, 0, 1]) {
    assert.ok(cursorAt(text, 4, side), `a cursor at 4, side ${side}`);
  }
  assert.deepEqual(posOf(doc, cursorAt(text, 4, 1)), { offset: 4, side: 1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 4, -1)), { offset: 4, side: -1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 5, 1)), { offset: 4, side: 1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 5, -1)), { offset: 4, side: -1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 6, -1)), { offset: 6, side: -1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 3, 1)), { offset: 3, side: 1 });
});

test("an emoji at the very start", () => {
  const { doc, text } = make("😀abc");
  assert.deepEqual(posOf(doc, cursorAt(text, 0, -1)), { offset: 0, side: -1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 0, 1)), { offset: 0, side: 1 });
});

test("an emoji at the very end", () => {
  const { doc, text } = make("abc😀");
  assert.deepEqual(posOf(doc, cursorAt(text, 3, 1)), { offset: 3, side: 1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 3, -1)), { offset: 3, side: -1 });
});

test("two emoji in a row", () => {
  const { doc, text } = make("a😀😀b");
  assert.deepEqual(posOf(doc, cursorAt(text, 1, 1)), { offset: 1, side: 1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 3, 1)), { offset: 3, side: 1 });
  assert.deepEqual(posOf(doc, cursorAt(text, 3, -1)), { offset: 3, side: -1 });
});

test("the body argument gives the same answer", () => {
  const { doc, text } = make(EXAMPLE);
  assert.deepEqual(posOf(doc, cursorAt(text, 4, -1, EXAMPLE)), { offset: 4, side: -1 });
});

test("a BMP-only text behaves exactly like getCursor", () => {
  const { doc, text } = make("café ☕ naïve");
  for (const pos of [0, 1, 4, 5, 9]) {
    for (const side of [-1, 0, 1]) {
      const a = text.getCursor(pos, side);
      const b = cursorAt(text, pos, side);
      assert.equal(b === undefined, a === undefined, `pos ${pos} side ${side}`);
      if (a) assert.deepEqual(posOf(doc, b), posOf(doc, a));
    }
  }
});

test("a position that is not a surrogate pair gets Loro's own answer", () => {
  const { text } = make("abc");
  for (const pos of [0, 3, 50]) {
    assert.equal(cursorAt(text, pos, -1) === undefined, text.getCursor(pos, -1) === undefined, `pos ${pos}`);
  }
});

test("an insert target captured after an emoji resolves to the same UTF-16 offset", () => {
  const { doc, text } = make("ab|c");
  text.insert(1, "😀");
  doc.commit();
  assert.equal(text.toString(), "a😀b|c");
  const from = 4; // UTF-16 offset of "|"; the code point offset would be 3
  const to = 5;
  const fromCursor = cursorAt(text, from, 0);
  const toCursor = cursorAt(text, to, 0);
  assert.equal(doc.getCursorPos(fromCursor).offset, from);
  assert.equal(doc.getCursorPos(toCursor).offset, to);
  text.insert(0, "xy");
  doc.commit();
  assert.equal(doc.getCursorPos(fromCursor).offset, from + 2);
});
