// What the preview frame shows, and what each reset keeps. The module is
// loaded as the page would compile it, so `state` is the reactive object the
// markup reads.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createPainted } = await loadRunes(new URL("../../src/lib/reader/painted.svelte.js", import.meta.url));

const map = { forward() {}, inverse() {} };

// 1. A fresh frame shows nothing.
{
  const painted = createPainted();
  assert.equal(painted.state.shown, false);
  assert.equal(painted.state.drawn, false);
  assert.equal(painted.state.empty, false);
  assert.equal(painted.ever, false);
  assert.equal(painted.synctex, null);
}

// 2. A delivered payload is a shown page.
{
  const painted = createPainted();
  painted.delivered("pdf");
  assert.equal(painted.ever, true);
  assert.equal(painted.state.shown, true);
  assert.equal(painted.state.empty, false);
  assert.equal(painted.state.drawn, false, "delivered is not yet drawn");
}

// 3. A drawn page is recorded without touching the rest.
{
  const painted = createPainted();
  painted.delivered("pdf");
  painted.drawn();
  assert.equal(painted.state.drawn, true);
  assert.equal(painted.state.shown, true);
}

// 4. reset() clears everything, click map included.
{
  const painted = createPainted();
  painted.delivered("pdf");
  painted.drawn();
  painted.synctex = map;
  painted.reset();
  assert.equal(painted.state.shown, false);
  assert.equal(painted.state.drawn, false);
  assert.equal(painted.state.empty, false);
  assert.equal(painted.ever, false);
  assert.equal(painted.synctex, null);
}


// 6. reset({ keepSynctex: true }) keeps the click map.
{
  const painted = createPainted();
  painted.delivered("pdf");
  painted.synctex = map;
  painted.reset({ keepSynctex: true });
  assert.equal(painted.synctex, map);
  assert.equal(painted.state.shown, false);
}

// 7. reset({ empty: true }) says the render found nothing to draw.
{
  const painted = createPainted();
  painted.delivered("pdf");
  painted.reset({ empty: true });
  assert.equal(painted.state.empty, true);
  assert.equal(painted.state.shown, false);
  assert.equal(painted.ever, false);
  painted.reset();
  assert.equal(painted.state.empty, false, "a plain reset clears the empty mark");
}

// 8. navigated() clears drawn and synctex, and leaves ever, shown and
// empty as they were.
{
  const painted = createPainted();
  painted.delivered("pdf");
  painted.drawn();
  painted.synctex = map;
  painted.navigated();
  assert.equal(painted.state.drawn, false);
  assert.equal(painted.synctex, null);
  assert.equal(painted.ever, true, "ever survives a navigation");
  assert.equal(painted.state.shown, true, "shown survives a navigation");
  assert.equal(painted.state.empty, false);

  const empty = createPainted();
  empty.reset({ empty: true });
  empty.navigated();
  assert.equal(empty.state.empty, true, "empty survives a navigation");
}

console.log("painted: the frame's shown, drawn and empty facts reset as specified");
