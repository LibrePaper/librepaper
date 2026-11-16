// The reader's place in the preview, as the Reader asks for it on a phone. The
// place is measured while the frame is visible (in the scroll batch) and the
// reply comes from that cache when the frame has been hidden, since a hidden
// frame has no layout to probe. Exercise the shipped functions with fake DOM
// objects.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const frame = readFileSync(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
function between(from, to) {
  const start = frame.indexOf(from);
  const end = frame.indexOf(to, start);
  assert.notEqual(start, -1, `${from.trim()} is present`);
  assert.notEqual(end, -1, `${to.trim()} follows it`);
  return frame.slice(start, end);
}
// The offset helpers: textPointOf through offsetAtPoint.
const offsets = between("  // Resolves a Range boundary to a {node, offset} pair", "  function captureSelection() {");
// The cached place, the scroll batch that measures it, and the scroll listener.
const scrolling = between("  const scrollOffsets = new WeakMap();", "  // The agent is injected before </body>");
// The reply branch of the message listener.
const reply = between('    if (message.type === "reading-position") {', '    if (message.type === "synctex-locate") {');

class Element {}
class Node {}
Node.TEXT_NODE = 3;

function setViewport(height, width) {
  globalThis.innerHeight = height;
  globalThis.innerWidth = width;
}

function harness({ table = null, caret = () => null, pdf = null, scrollingElement }) {
  const listeners = [];
  const posts = [];
  const frames = [];
  const document = {
    scrollingElement,
    documentElement: scrollingElement,
    caretPositionFromPoint: (x, y) => caret(x, y),
    addEventListener(type, listener) {
      if (type === "scroll") listeners.push(listener);
    },
  };
  // The viewer is read when the measurement runs, so it stays set for the harness.
  globalThis.librepaperViewer = pdf ? { pointFromClient: () => pdf } : undefined;
  const requestAnimationFrame = (callback) => frames.push(callback);
  const build = new Function(
    "document", "Element", "Node", "requestAnimationFrame", "post", "tableArgument",
    `let table = tableArgument;\n${offsets}\n${scrolling}\nfunction handleReading(message) {\n${reply}\n}\nreturn handleReading;`,
  );
  const handleReading = build(document, Element, Node, requestAnimationFrame, (message) => posts.push(message), table);
  return {
    posts,
    handleReading,
    // Fires a scroll of the page to `top`, as the browser does.
    scrollTo(top) {
      scrollingElement.scrollTop = top;
      for (const listener of listeners) listener({ target: document });
    },
    pendingFrames: () => frames.length,
    flushFrames() {
      while (frames.length) frames.shift()();
    },
  };
}

// A page whose one text node starts at offset 10 of the published text.
function pageWithText(caret) {
  const text = new Node();
  text.nodeType = Node.TEXT_NODE;
  text.data = "x".repeat(40);
  const table = { nodes: [text], starts: [10], index: new Map([[text, 0]]), joined: null };
  return { table, caret: caret(text) };
}

// a. A scroll measures the place while the frame is visible; a reply after the
// frame is hidden (no viewport) carries that earlier measurement.
{
  setViewport(600, 400);
  const scrollingElement = { scrollTop: 0, scrollHeight: 2000, clientHeight: 600 };
  // The caret answers only while there is a layout, as a hidden frame has none.
  const { table, caret } = pageWithText((text) => (x, y) =>
    globalThis.innerHeight > 0 ? { offsetNode: text, offset: 7 } : null);
  const page = harness({
    table,
    caret,
    pdf: { page: 1, x: 10, y: 20 },
    scrollingElement,
  });
  page.scrollTo(0);
  page.scrollTo(120);
  assert.equal(page.pendingFrames(), 1, "a scroll batch is pending for the flush");
  page.flushFrames();
  assert.equal(page.posts.at(-1).type, "scroll", "the batch posts the scroll");

  setViewport(0, 0);
  page.handleReading({ type: "reading-position" });
  assert.deepEqual(
    page.posts.at(-1),
    { type: "reading-position", start: 17, pdf: { page: 1, x: 10, y: 20 } },
    "a hidden frame replies with the place measured during the earlier scroll",
  );
}

// b. A frame with no text and no viewer replies with nulls, whether the text
// table is empty or has not been built yet.
for (const table of [null, { nodes: [], starts: [], index: new Map(), joined: null }]) {
  setViewport(600, 400);
  const page = harness({ table, scrollingElement: { scrollTop: 0, scrollHeight: 0, clientHeight: 0 } });
  page.handleReading({ type: "reading-position" });
  assert.deepEqual(
    page.posts.at(-1),
    { type: "reading-position", start: null, pdf: null },
    "no text and no viewer replies with nulls",
  );
}

delete globalThis.innerHeight;
delete globalThis.innerWidth;
delete globalThis.librepaperViewer;

console.log("frame-reading: the reading position is measured in the scroll batch and answered from the cache");
