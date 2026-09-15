// Where the bar over a selection goes.
//
// The page measures; this decides. Both rules it enforces are about not
// putting the bar somewhere nobody can use it: off the edge of the window, or
// underneath the bar at the top of the page.
import assert from "node:assert/strict";
import { LIFT, MARGIN, correctedLeft, placeBar, withinWindow } from "../../src/lib/annotation-bar.js";

const frame = { left: 100, top: 50 };
const place = (rect, extra = {}) => placeBar({ rect, frame, width: 200, minTop: 60, windowWidth: 1000, ...extra });

// The ordinary case: centred over the selection, lifted clear of it.
{
  const at = place({ left: 300, right: 500, top: 200 });
  assert.equal(at.left, 100 + 400 - 100, "centred on the middle of the selection");
  assert.equal(at.top, 50 + 200 - LIFT, "and lifted above it");
}

// A selection near the top of the frame would put the bar under the page's
// own top bar, so it stops there instead.
{
  const at = place({ left: 300, right: 500, top: 0 });
  assert.equal(at.top, 60, "never rises under the bar at the top of the page");
}

// Neither edge of the window is crossed.
{
  assert.equal(place({ left: -200, right: -190, top: 200 }).left, MARGIN, "clamped to the left margin");
  assert.equal(
    place({ left: 2000, right: 2010, top: 200 }).left,
    1000 - 200 - MARGIN,
    "clamped to the right margin",
  );
}

// A bar wider than the window loses its right-hand end rather than its
// left-hand one: the left edge wins when the two rules disagree.
{
  assert.equal(withinWindow(0, 1200, 1000), MARGIN);
  assert.equal(place({ left: 300, right: 500, top: 200 }, { width: 1200, windowWidth: 1000 }).left, MARGIN);
}

// The bar is placed before it is drawn and corrected once its real width is
// known -- but only when the correction is worth a redraw, because the effect
// that applies it reads the width it is about to move.
{
  assert.equal(correctedLeft({ left: 400, width: 200, windowWidth: 1000 }), null, "already in bounds");
  assert.equal(correctedLeft({ left: -50, width: 200, windowWidth: 1000 }), MARGIN, "out of bounds is corrected");
  assert.equal(
    correctedLeft({ left: MARGIN - 0.5, width: 200, windowWidth: 1000 }),
    null,
    "a sub-pixel difference is not a redraw",
  );
  // Widening the bar (choosing the highlight tool adds five swatches) can
  // push it off the right edge, which is exactly what this corrects.
  assert.equal(correctedLeft({ left: 900, width: 300, windowWidth: 1000 }), 1000 - 300 - MARGIN);
}

console.log("annotation bar: centred on the selection, inside the window, clear of the top bar");
