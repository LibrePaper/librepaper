// The source editor's right-click menu: where a right click leaves the caret,
// what the menu offers, and whether the clipboard can be read for Paste.
import assert from "node:assert/strict";
import { SOURCE_MENU, clipboardReadable, rightClickSelection } from "../../src/lib/source-menu.js";
import { COMMANDS } from "../../src/lib/commands.js";

/* ------------------------------------------------------ where the caret goes */

// A click inside a selection keeps it, so the menu acts on what was pointed at.
assert.equal(rightClickSelection({ from: 4, to: 9 }, 6), null, "inside a selection keeps it");
assert.equal(rightClickSelection({ from: 4, to: 9 }, 4), null, "at the start of a selection keeps it");
assert.equal(rightClickSelection({ from: 4, to: 9 }, 9), null, "at the end of a selection keeps it");

// A click elsewhere moves the caret there first, with or without a selection.
assert.equal(rightClickSelection({ from: 4, to: 9 }, 12), 12, "outside a selection moves the caret");
assert.equal(rightClickSelection({ from: 4, to: 9 }, 0), 0, "before a selection moves the caret");
assert.equal(rightClickSelection({ from: 5, to: 5 }, 5), null, "a click on the caret leaves it");
assert.equal(rightClickSelection({ from: 5, to: 5 }, 2), 2, "a click elsewhere moves a bare caret");

// A click that found no text moves nothing.
assert.equal(rightClickSelection({ from: 4, to: 9 }, null), null, "no position, no move");
assert.equal(rightClickSelection({ from: 4, to: 9 }, undefined), null, "no position, no move");

/* -------------------------------------------------------------- the items */

// Comment and Link first, then the clipboard trio; every item is a registry command.
assert.deepEqual(SOURCE_MENU, [["comment", "link"], ["cut", "copy", "paste"]]);
for (const group of SOURCE_MENU) {
  for (const id of group) {
    assert.ok(COMMANDS.some((command) => command.id === id), `${id} is a registry command`);
  }
}

/* ---------------------------------------------------- the clipboard's answer */

const readable = { clipboard: { readText: async () => "" } };

assert.equal(await clipboardReadable({}), false, "no clipboard API at all");
assert.equal(await clipboardReadable({ clipboard: {} }), false, "a clipboard without readText");
assert.equal(await clipboardReadable(readable), true, "readText and no permission API: readable");
assert.equal(
  await clipboardReadable({ ...readable, permissions: { query: async () => ({ state: "granted" }) } }),
  true,
  "granted",
);
assert.equal(
  await clipboardReadable({ ...readable, permissions: { query: async () => ({ state: "prompt" }) } }),
  true,
  "prompt: the read asks",
);
assert.equal(
  await clipboardReadable({ ...readable, permissions: { query: async () => ({ state: "denied" }) } }),
  false,
  "denied greys Paste",
);
assert.equal(
  await clipboardReadable({ ...readable, permissions: { query: async () => { throw new TypeError("unsupported"); } } }),
  true,
  "a browser that does not answer the query is taken as readable",
);
