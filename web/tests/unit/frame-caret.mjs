// A single click in the preview must not move the source editor. Only a
// double-click reports a place. Exercise the shipped listeners with fake DOM
// objects.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const frame = readFileSync(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
const start = frame.indexOf("  // A click that is not on a mark clears the ring;");
const end = frame.indexOf("  // The keys that act on a selection have to be heard here", start);
assert.notEqual(start, -1, "the mark ring listener is present");
assert.notEqual(end, -1, "the mark ring listener has a boundary");
const caretHandling = frame.slice(start, end);

class Element {
  closest() {
    return null;
  }
}
class Node {}
Node.TEXT_NODE = 3;

function harness({ selectedId = "", nodes = [], index = new Map(), starts = [], caret = null, pdf = null }) {
  const listeners = new Map();
  const posts = [];
  const document = {
    addEventListener(type, listener) { listeners.set(type, listener); },
    caretPositionFromPoint: () => caret,
  };
  const table = { nodes, index, starts };
  const install = new Function(
    "document", "Element", "Node", "table", "post",
    `let selectedId = ${JSON.stringify(selectedId)};\nconst select = (id) => { selectedId = id; };\n${caretHandling}\nreturn true;`,
  );
  globalThis.librepaperViewer = pdf ? { pointFromClient: () => pdf } : undefined;
  try {
    install(document, Element, Node, table, (message) => posts.push(message));
  } finally {
    globalThis.librepaperViewer = undefined;
  }
  return { listeners, posts };
}

const event = { target: new Element(), clientX: 1, clientY: 2, button: 0 };

// A single click on an unmarked place posts nothing at all.
{
  const { listeners, posts } = harness({});
  assert.equal(listeners.has("dblclick"), true, "a double-click listener is installed");
  listeners.get("click")(event);
  assert.deepEqual(posts, [], "a single click posts no caret");
}

// A single click on a text node posts nothing either.
{
  const text = new Node();
  text.nodeType = Node.TEXT_NODE;
  const { listeners, posts } = harness({
    nodes: [text], index: new Map([[text, 0]]), starts: [10], caret: { offsetNode: text, offset: 4 },
  });
  listeners.get("click")(event);
  assert.deepEqual(posts, [], "a single click on text posts no caret");
}

// A click on a mark ring clears it and nothing else.
{
  const { listeners, posts } = harness({ selectedId: "c1" });
  listeners.get("click")(event);
  assert.deepEqual(posts, [{ type: "unfocus" }], "a click outside the mark clears the ring");
}

// A double-click on text reports the offset into the published text.
{
  const text = new Node();
  text.nodeType = Node.TEXT_NODE;
  const { listeners, posts } = harness({
    nodes: [text], index: new Map([[text, 0]]), starts: [10], caret: { offsetNode: text, offset: 4 },
  });
  listeners.get("dblclick")(event);
  assert.deepEqual(posts, [{ type: "caret", offset: 14, pdf: null }], "a double-click reports the caret");
}

// A double-click on the PDF with no text under it reports the PDF point.
{
  const { listeners, posts } = harness({ pdf: { page: 1, x: 3, y: 4 } });
  listeners.get("dblclick")(event);
  assert.deepEqual(posts, [{ type: "pdf-caret", pdf: { page: 1, x: 3, y: 4 } }], "a double-click reports the PDF point");
}

console.log("frame-caret: single clicks leave the source alone, double-clicks report the place");
