import assert from "node:assert/strict";
import { mapDraftSourcepos } from "../../src/lib/engines/quarto.js";

// A draft line that maps directly keeps its columns and takes the .qmd line.
assert.equal(
  mapDraftSourcepos('<p data-sourcepos="3:1-4:7">', [1, 2, 5, 6]),
  '<p data-sourcepos="5:1-6:7">',
);

// A generated start line falls back to the preceding mapped line at column 1.
assert.equal(
  mapDraftSourcepos('<p data-sourcepos="2:4-3:2">', [10, null, 12]),
  '<p data-sourcepos="10:1-12:2">',
);

// A leading generated line has no preceding mapped line, so the following one is used.
assert.equal(
  mapDraftSourcepos('<p data-sourcepos="1:3-1:9">', [null, 7]),
  '<p data-sourcepos="7:1-7:1">',
);

// A map with no mapped line at all drops the attribute and its leading space.
assert.equal(
  mapDraftSourcepos('<p data-sourcepos="1:1-1:5">x</p>', [null, null]),
  "<p>x</p>",
);

// Tagged positions are not the untagged shape, so they are left alone.
assert.equal(
  mapDraftSourcepos('<p data-sourcepos="0:1:1-0:1:5">x</p>', [1, 2, 5, 6]),
  '<p data-sourcepos="0:1:1-0:1:5">x</p>',
);

// Every position in one string is rewritten.
assert.equal(
  mapDraftSourcepos('<h1 data-sourcepos="1:1-1:9"><em data-sourcepos="3:2-4:3">a</em></h1>', [1, 2, 5, 6]),
  '<h1 data-sourcepos="1:1-1:9"><em data-sourcepos="5:2-6:3">a</em></h1>',
);
