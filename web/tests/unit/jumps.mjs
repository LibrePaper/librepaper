// The Back stack: what a jump leaves behind, what Back returns, and the
// chord that asks for it.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createJumps, isBackChord } = await loadRunes(new URL("../../src/lib/reader/jumps.svelte.js", import.meta.url));

const place = (file, offset) => ({ file, offset });

// Remember and back come out in the order they went in, newest first.
{
  const jumps = createJumps();
  jumps.remember(place("a", 1));
  jumps.remember(place("b", 2));
  jumps.remember(place("c", 3));
  assert.equal(jumps.canGoBack, true);
  assert.deepEqual(jumps.last, place("c", 3));
  assert.deepEqual(jumps.back(), place("c", 3));
  assert.deepEqual(jumps.back(), place("b", 2));
  assert.deepEqual(jumps.last, place("a", 1));
  assert.deepEqual(jumps.back(), place("a", 1));
  assert.equal(jumps.canGoBack, false);
  assert.equal(jumps.last, null);
}

// A place equal to the newest one is not stored twice, so Back does not
// return to the same spot twice in a row. A different offset in the same
// file is a different place.
{
  const jumps = createJumps();
  jumps.remember(place("a", 1));
  jumps.remember(place("a", 1));
  assert.deepEqual(jumps.back(), place("a", 1));
  assert.equal(jumps.back(), null);
  jumps.remember(place("a", 1));
  jumps.remember(place("a", 2));
  jumps.remember(place("a", 2));
  assert.deepEqual(jumps.back(), place("a", 2));
  assert.deepEqual(jumps.back(), place("a", 1));
  assert.equal(jumps.back(), null);
}

// Past the limit the oldest place is dropped, and the newest are kept.
{
  const jumps = createJumps({ limit: 3 });
  for (let i = 1; i <= 5; i += 1) jumps.remember(place("f", i));
  assert.deepEqual(jumps.back(), place("f", 5));
  assert.deepEqual(jumps.back(), place("f", 4));
  assert.deepEqual(jumps.back(), place("f", 3));
  assert.equal(jumps.back(), null);
}

// The default limit is fifty.
{
  const jumps = createJumps();
  for (let i = 1; i <= 51; i += 1) jumps.remember(place("f", i));
  let count = 0;
  while (jumps.back()) count += 1;
  assert.equal(count, 50);
}

// Back on an empty stack is null, and so is Back after clear.
{
  const jumps = createJumps();
  assert.equal(jumps.back(), null);
  assert.equal(jumps.canGoBack, false);
  jumps.remember(place("a", 1));
  jumps.clear();
  assert.equal(jumps.canGoBack, false);
  assert.equal(jumps.back(), null);
}

// Places in files that are gone are skipped and dropped, and Back returns
// the newest one that still exists.
{
  const jumps = createJumps();
  jumps.remember(place("kept", 1));
  jumps.remember(place("gone", 2));
  jumps.remember(place("kept", 3));
  jumps.remember(place("gone", 4));
  const exists = (file) => file !== "gone";
  assert.deepEqual(jumps.back(exists), place("kept", 3));
  assert.deepEqual(jumps.back(exists), place("kept", 1));
  assert.equal(jumps.back(exists), null);
  assert.equal(jumps.canGoBack, false);
}

// The chord: Alt+Left off a Mac, Ctrl+Minus on one. Option+Left is not Back
// on a Mac, and a Mac Ctrl+Minus is not Back elsewhere.
{
  const key = (fields) => ({ key: "", altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, ...fields });
  assert.equal(isBackChord(key({ key: "ArrowLeft", altKey: true }), false), true);
  assert.equal(isBackChord(key({ key: "-", ctrlKey: true }), false), false);
  assert.equal(isBackChord(key({ key: "-", ctrlKey: true }), true), true);
  assert.equal(isBackChord(key({ key: "ArrowLeft", altKey: true }), true), false);
  assert.equal(isBackChord(key({ key: "ArrowLeft", altKey: true, ctrlKey: true }), false), false);
}

console.log("jumps: ok");
