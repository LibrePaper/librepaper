import assert from "node:assert/strict";
import { passageContext } from "../../src/lib/source-passage.js";

// Context is read from the open text: `context` units on each side of the selection.
const wide = passageContext("x".repeat(40) + "word" + "y".repeat(40), 40, 44);
assert.equal(wide.prefix.length, 32);
assert.equal(wide.suffix.length, 32);
assert.equal(passageContext("Alpha beta gamma", 6, 10).prefix, "Alpha ");
assert.equal(passageContext("Alpha beta gamma", 6, 10).suffix, " gamma");
// The 32 unit context edges fall inside emoji pairs, so each edge is trimmed to a whole character.
const emoji = "\u{1F600}";
const paired = "a".repeat(10) + emoji + "b".repeat(31) + "SEL" + "c".repeat(31) + emoji + "d".repeat(10);
const edged = passageContext(paired, 43, 46);
assert.equal(paired.slice(43, 46), "SEL");
assert.equal(edged.prefix, "b".repeat(31));
assert.equal(edged.suffix, "c".repeat(31));
assert.equal(edged.prefix.charCodeAt(0) >= 0xdc00 && edged.prefix.charCodeAt(0) <= 0xdfff, false);
assert.equal(edged.suffix.charCodeAt(edged.suffix.length - 1) >= 0xd800 && edged.suffix.charCodeAt(edged.suffix.length - 1) <= 0xdbff, false);
// (a) The prefix window starts on the low half of a leading emoji, so the window moves past it.
const leading = passageContext(emoji + "a".repeat(31) + "word", 33, 37);
assert.equal(leading.prefix, "a".repeat(31));
assert.equal(leading.prefix.charCodeAt(0) >= 0xdc00 && leading.prefix.charCodeAt(0) <= 0xdfff, false);
// (b) The suffix window ends on the high half of an emoji, so the window stops before it.
const trailing = passageContext("word" + "c".repeat(31) + emoji + "d".repeat(5), 0, 4);
assert.equal(trailing.prefix, "");
assert.equal(trailing.suffix, "c".repeat(31));
assert.equal(trailing.suffix.charCodeAt(trailing.suffix.length - 1) >= 0xd800 && trailing.suffix.charCodeAt(trailing.suffix.length - 1) <= 0xdbff, false);

console.log("source-passage: selection context keeps whole characters at the edges");
