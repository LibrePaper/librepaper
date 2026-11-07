// A plain click in the preview reports where it landed, so the source caret can
// follow; a drag, a modified click or a second click of a double-click does not.
// A double-click reports its place as before. Exercise the shipped listeners
// with fake DOM objects.
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
  // Every listener for a type, in order: the frame installs two for "click".
  const all = new Map();
  const listeners = { has: (type) => all.has(type), get: (type) => (event) => (all.get(type) || []).forEach((listener) => listener(event)) };
  const posts = [];
  const document = {
    addEventListener(type, listener) { all.set(type, [...(all.get(type) || []), listener]); },
    caretPositionFromPoint: () => caret,
  };
  const table = { nodes, index, starts };
  const install = new Function(
    "document", "Element", "Node", "table", "post",
    `let selectedId = ${JSON.stringify(selectedId)};\nconst select = (id) => { selectedId = id; };\n${caretHandling}\nreturn true;`,
  );
  // The listener reads the viewer when the event arrives, so it stays set until the next harness.
  globalThis.librepaperViewer = pdf ? { pointFromClient: () => pdf } : undefined;
  install(document, Element, Node, table, (message) => posts.push(message));
  return { listeners, posts };
}

const event = { target: new Element(), clientX: 1, clientY: 2, button: 0, detail: 1 };
// A press and its click at the same point, as a plain click arrives.
const click = (listeners, at = event) => { listeners.get("pointerdown")(at); listeners.get("click")(at); };

// A single click on an unmarked place posts nothing at all.
{
  const { listeners, posts } = harness({});
  assert.equal(listeners.has("dblclick"), true, "a double-click listener is installed");
  click(listeners);
  assert.deepEqual(posts, [], "a click on nothing textual posts nothing");
}

// A plain click on text reports its offset into the published text.
{
  const text = new Node();
  text.nodeType = Node.TEXT_NODE;
  const { listeners, posts } = harness({
    nodes: [text], index: new Map([[text, 0]]), starts: [10], caret: { offsetNode: text, offset: 4 },
  });
  click(listeners);
  assert.deepEqual(posts, [{ type: "click", offset: 14, pdf: null }], "a plain click reports where it landed");
}

// A drag, a modified click and the second click of a double-click report nothing.
for (const [variant, why] of [
  [{ clientX: 40 }, "a drag"],
  [{ ctrlKey: true }, "a modified click"],
  [{ detail: 2 }, "a second click"],
]) {
  const text = new Node();
  text.nodeType = Node.TEXT_NODE;
  const { listeners, posts } = harness({
    nodes: [text], index: new Map([[text, 0]]), starts: [10], caret: { offsetNode: text, offset: 4 },
  });
  listeners.get("pointerdown")(event);
  listeners.get("click")({ ...event, ...variant });
  assert.deepEqual(posts, [], `${why} reports no place`);
}

// A click on a mark ring clears it and nothing else.
{
  const { listeners, posts } = harness({ selectedId: "c1" });
  click(listeners);
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

console.log("frame-caret: plain clicks and double-clicks report the place; drags and modified clicks do not");
