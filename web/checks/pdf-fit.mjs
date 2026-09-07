// What scale a PDF page is drawn at, without a browser.
//
// The bug this stands against: a letter-sized page was always drawn at 1.5
// CSS pixels per point, so on anything narrower than about 950px -- the pane
// beside the source, a phone in either orientation -- the page was clipped on
// both sides and read by scrolling sideways one line at a time.

import assert from "node:assert/strict";
import { GUTTER, MIN_SCALE, SCALE, scaleFor } from "../src/lib/pdf/fit.js";

// US letter and A4, in points, which is what `getViewport({scale: 1})` gives.
const LETTER = 612;
const A4 = 595;

const fits = (width, points) => scaleFor(width, points) * points <= width - GUTTER + 0.001;

// A frame with room to spare is drawn at the scale we would have chosen
// anyway: fitting the width must never mean magnifying a page to fill it.
assert.equal(scaleFor(1600, LETTER), SCALE, "a wide frame keeps the natural scale");
assert.equal(scaleFor(LETTER * SCALE + GUTTER, LETTER), SCALE, "exactly enough room is enough");

// The frames the reader actually gets. Each has to hold a whole page.
for (const [what, width] of [
  ["a phone", 390],
  ["a phone, turned", 844],
  ["the pane beside the source at 1440", 534],
  ["a tablet", 768],
  ["the document-only layout at 1024", 1024],
]) {
  for (const [paper, points] of [["letter", LETTER], ["A4", A4]]) {
    assert.ok(fits(width, points), `${paper} fits ${what} (${width}px)`);
    assert.ok(scaleFor(width, points) >= MIN_SCALE, `${paper} on ${what} stays readable`);
  }
}

// A frame too narrow to hold a page at a readable size stops shrinking and
// lets the reader scroll instead. Nothing is ever drawn at a scale of zero.
assert.equal(scaleFor(60, LETTER), MIN_SCALE, "a frame narrower than the floor stops at it");
assert.ok(scaleFor(1, LETTER) > 0, "an absurd width still draws something");

// A width nobody has measured yet is not evidence of a narrow frame: the
// first paint of a detached node must not commit the document to 0.35.
assert.equal(scaleFor(0, LETTER), SCALE, "an unmeasured frame keeps the natural scale");
assert.equal(scaleFor(undefined, LETTER), SCALE, "and so does one with no width at all");
assert.equal(scaleFor(1600, 0), SCALE, "a page with no width does not divide by it");
assert.equal(scaleFor(NaN, LETTER), SCALE, "nor does a width that is not a number");

// Widening is never a downgrade: the redraw after a drag or a rotation can
// only give the reader a page at least as large as the one they had.
let last = 0;
for (let width = 100; width <= 2000; width += 37) {
  const now = scaleFor(width, LETTER);
  assert.ok(now >= last - 1e-9, `scale does not fall as the frame grows (at ${width}px)`);
  last = now;
}

console.log("pdf-fit: page scale fits every frame the reader gets, and never magnifies");
