// The editor place a phone gets back after the browser reloads a hidden tab.
// Storage is faked: sessionStorage is per tab and absent under Node.
import assert from "node:assert/strict";
import { readPlace, savePlace } from "../../src/lib/reader/place.js";

function fakeStorage() {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, value),
    map,
  };
}

const place = {
  openFile: "chapters/intro.typ",
  caret: 412,
  top: 1800,
  offset: 240,
  layout: "split",
  preferredPane: "source",
  mobileView: "source",
};

// A saved place comes back as it was written.
{
  const storage = fakeStorage();
  savePlace("my-paper", place, storage);
  assert.deepEqual(readPlace("my-paper", storage), place);
  assert.equal(storage.map.get("librepaper-place:my-paper"), JSON.stringify(place));
}

// A project with nothing saved has no place.
{
  assert.equal(readPlace("never-opened", fakeStorage()), null);
}

// Garbage in storage reads as no place.
{
  const storage = fakeStorage();
  storage.setItem("librepaper-place:p", "{not json");
  assert.equal(readPlace("p", storage), null);
  storage.setItem("librepaper-place:p", "42");
  assert.equal(readPlace("p", storage), null, "a bare number is not a place");
  storage.setItem("librepaper-place:p", "null");
  assert.equal(readPlace("p", storage), null, "null is not a place");
}

// A place with no file to open is not restored.
{
  const storage = fakeStorage();
  storage.setItem("librepaper-place:p", JSON.stringify({ caret: 3, top: 4 }));
  assert.equal(readPlace("p", storage), null);
  storage.setItem("librepaper-place:p", JSON.stringify({ openFile: "", caret: 3 }));
  assert.equal(readPlace("p", storage), null);
}

// Offsets that are negative, fractional or not numbers become whole non-negative
// numbers, and missing ones become zero. Layout names that are not strings are dropped.
{
  const storage = fakeStorage();
  storage.setItem("librepaper-place:p", JSON.stringify({
    openFile: "main.tex",
    caret: -5,
    top: "soon",
    layout: 7,
    preferredPane: null,
    mobileView: { nested: true },
  }));
  assert.deepEqual(readPlace("p", storage), {
    openFile: "main.tex",
    caret: 0,
    top: 0,
    offset: 0,
    layout: "",
    preferredPane: "",
    mobileView: "",
  });
  storage.setItem("librepaper-place:p", JSON.stringify({ openFile: "main.tex", caret: 12.9, top: -1, offset: -30 }));
  assert.equal(readPlace("p", storage).caret, 12);
  assert.equal(readPlace("p", storage).top, 0);
  assert.equal(readPlace("p", storage).offset, 0, "a negative pixel offset is zero");
}

// Storage that throws is treated as no storage: reads give null, writes do nothing.
{
  const throwing = {
    getItem() { throw new Error("blocked"); },
    setItem() { throw new Error("blocked"); },
  };
  assert.equal(readPlace("p", throwing), null);
  assert.doesNotThrow(() => savePlace("p", place, throwing));
}

// No storage at all, as when sessionStorage is unavailable.
{
  assert.equal(readPlace("p", null), null);
  assert.doesNotThrow(() => savePlace("p", place, null));
}

console.log("place: ok");
