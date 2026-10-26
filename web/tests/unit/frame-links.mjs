// A link in the preview frame must open in a new tab and never navigate the
// frame itself. Exercise the shipped click listener with fake DOM objects.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const frame = readFileSync(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
const start = frame.indexOf("  // A link in the document never navigates the preview. It opens in a new");
const end = frame.indexOf("  // A click that is not on a mark clears the ring;", start);
assert.notEqual(start, -1, "frame link handling is present");
assert.notEqual(end, -1, "frame link handling has a boundary");
const linkHandling = frame.slice(start, end);

class Element {
  constructor(link) {
    this.link = link;
  }
  closest(selector) {
    assert.equal(selector, "a[href], area[href]");
    return this.link;
  }
}

function harness() {
  const listeners = [];
  const opened = [];
  const document = {
    baseURI: "https://docs.example/p/doc/",
    addEventListener(type, listener, opts) { listeners.push({ type, listener, opts }); },
  };
  const location = { origin: "https://docs.example", pathname: "/p/doc/", search: "" };
  const window = { open(...args) { opened.push(args); } };
  const install = new Function(
    "document", "location", "window", "Element",
    `${linkHandling}\nreturn true;`,
  );
  install(document, location, window, Element);
  assert.equal(listeners.length, 1);
  assert.equal(listeners[0].type, "click");
  assert.deepEqual(listeners[0].opts, { capture: true });
  return { listener: listeners[0].listener, opened };
}

function click(agent, href, button = 0) {
  const link = { getAttribute: () => href };
  const event = {
    target: new Element(link),
    button,
    defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; },
  };
  agent.listener(event);
  return event;
}

// An absolute link is prevented and opened in a new tab.
{
  const agent = harness();
  const event = click(agent, "https://example.com/x");
  assert.equal(event.defaultPrevented, true);
  assert.deepEqual(agent.opened, [["https://example.com/x", "_blank", "noopener"]]);
}

// A relative link resolves against the document base and opens in a new tab.
{
  const agent = harness();
  const event = click(agent, "other.html");
  assert.equal(event.defaultPrevented, true);
  assert.deepEqual(agent.opened, [["https://docs.example/p/doc/other.html", "_blank", "noopener"]]);
}

// A same-document fragment link is left alone so the browser can scroll.
{
  const agent = harness();
  const event = click(agent, "#sec");
  assert.equal(event.defaultPrevented, false);
  assert.deepEqual(agent.opened, []);
}

// Only a primary click opens a link; other buttons are ignored.
{
  const agent = harness();
  const event = click(agent, "https://example.com/x", 1);
  assert.equal(event.defaultPrevented, false);
  assert.deepEqual(agent.opened, []);
}

// A javascript: link does nothing: it is prevented and never opened.
{
  const agent = harness();
  const event = click(agent, "javascript:alert(1)");
  assert.equal(event.defaultPrevented, true);
  assert.deepEqual(agent.opened, []);
}

console.log("frame link tests passed");
