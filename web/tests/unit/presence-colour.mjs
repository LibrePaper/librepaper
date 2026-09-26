import assert from "node:assert/strict";
import test from "node:test";
import { PRESENCE_COLOURS, presenceColour } from "../../src/lib/presence-colour.js";

test("a person keeps one colour, and it is one the stylesheet draws", () => {
  const mine = presenceColour("vincent");
  assert.ok(PRESENCE_COLOURS.includes(mine));
  // The same person in another tab, another session, another day.
  assert.equal(presenceColour("vincent"), mine);
  assert.equal(presenceColour("  vincent  "), mine);
});

test("nobody is nobody's colour", () => {
  // An empty seed means the session has nothing to colour by, and the
  // stylesheet's own fallback applies rather than the first hue standing in
  // for everyone at once.
  assert.equal(presenceColour(""), "");
  assert.equal(presenceColour(null), "");
  assert.equal(presenceColour(undefined), "");
});

test("the people in one file come out in different colours", () => {
  // Six people, six colours: the whole palette, one apiece. A hash alone
  // cannot promise this -- six names land in six buckets only by luck -- so
  // each new arrival is given a colour nobody in the file is wearing.
  const names = ["vincent", "ada", "grace", "alan", "edsger", "barbara"];
  const chosen = [];
  for (const who of names) chosen.push(presenceColour(who, chosen));
  assert.equal(new Set(chosen).size, names.length);
});

test("a seventh person shares rather than going uncoloured", () => {
  // The palette runs out at six. Sharing is worse than not, but a caret with
  // no colour at all is the theme colour, which is the preview's own.
  const seventh = presenceColour("zoe", PRESENCE_COLOURS);
  assert.ok(PRESENCE_COLOURS.includes(seventh));
});

test("a tab id colours whoever has not said who they are", () => {
  // Two readers who never signed in are two tabs, so they are two colours
  // rather than one shared "Anonymous".
  const first = presenceColour("tab-7f3a91");
  const second = presenceColour("tab-0c14de");
  assert.ok(PRESENCE_COLOURS.includes(first));
  assert.ok(PRESENCE_COLOURS.includes(second));
  assert.notEqual(first, second);
});
