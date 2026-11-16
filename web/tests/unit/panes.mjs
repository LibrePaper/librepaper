// The limits of the side panel (the column) and the source/document split in
// web/src/lib/panes.js: how wide the column may be dragged, and how the source
// gives way to it in a split. Values are hand-computed at a 16px rem.
import assert from "node:assert/strict";

const { PANES, clamp, pixels, range } = await import("../../src/lib/panes.js");

const state = (width, layout, size = 360) => ({
  width,
  layout,
  comments: true,
  editing: true,
  sourceSide: "left",
  sizes: { [PANES.sidebar.key]: size, [PANES.editor.key]: 0.5 },
});

assert.equal(
  range(PANES.sidebar, state(1440, "split")).max,
  860,
  "a split leaves the document 360 and the source 160",
);
assert.equal(
  range(PANES.sidebar, state(1920, "split")).max,
  1340,
  "a wide split still leaves the document its 360 and the source 160",
);
assert.equal(
  range(PANES.sidebar, state(1440, "document")).max,
  1026,
  "alone, the document keeps 360",
);
assert.equal(
  range(PANES.sidebar, state(1440, "source")).max,
  1066,
  "alone, the source keeps its full 320",
);
assert.equal(
  range(PANES.sidebar, state(900, "split")).max,
  294,
  "a narrow window still holds the column to 38%",
);
assert.equal(
  range(PANES.sidebar, state(1440, "split")).min,
  240,
  "the column's minimum is 15rem",
);
assert.equal(
  clamp(PANES.sidebar, state(1440, "split", 5000)),
  860,
  "a stored width past the limit is held to it",
);
assert.equal(
  pixels(PANES.editor, state(1440, "split", 860)),
  160,
  "the source gives way when the column is at its widest",
);
assert.equal(
  pixels(PANES.editor, state(1920, "split", 360)),
  750,
  "at a normal width the even split is untouched (room 1500, half of it)",
);
