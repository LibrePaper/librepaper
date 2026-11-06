import assert from "node:assert/strict";
import { passageContext, sourcePassage } from "../../src/lib/source-passage.js";

// A source passage is read from the open source text, and it needs no revision to be anchored.
const plain = sourcePassage("Alpha beta gamma", 6, 10, "main.md");
assert.deepEqual(plain, { path: "main.md", exact: "beta", prefix: "Alpha ", suffix: " gamma", position: 6 });
const wide = sourcePassage("x".repeat(40) + "word" + "y".repeat(40), 40, 44, "main.md");
assert.equal(wide.exact, "word");
assert.equal(wide.prefix.length, 32);
assert.equal(wide.suffix.length, 32);
assert.equal(wide.position, 40);
assert.equal(sourcePassage("abc", 1, 1, "main.md"), null);
assert.equal(sourcePassage("a   b", 1, 4, "main.md"), null);
assert.equal("path" in sourcePassage("Alpha beta", 6, 10), false);
// The 32 unit context edges fall inside emoji pairs, so each edge is trimmed to a whole character.
const emoji = "\u{1F600}";
const paired = "a".repeat(10) + emoji + "b".repeat(31) + "SEL" + "c".repeat(31) + emoji + "d".repeat(10);
const edged = sourcePassage(paired, 43, 46, "main.md");
assert.equal(edged.exact, "SEL");
assert.equal(edged.prefix.length, 31);
assert.equal(edged.suffix.length, 31);
assert.equal(edged.prefix.charCodeAt(0) >= 0xdc00 && edged.prefix.charCodeAt(0) <= 0xdfff, false);
assert.equal(edged.suffix.charCodeAt(edged.suffix.length - 1) >= 0xd800 && edged.suffix.charCodeAt(edged.suffix.length - 1) <= 0xdbff, false);
// passageContext holds the same edge rules on its own, for the editor's selections.
// (a) The prefix window starts on the low half of a leading emoji, so the window moves past it.
const leading = passageContext(emoji + "a".repeat(31) + "word", 33, 37);
assert.equal(leading.prefix, "a".repeat(31));
assert.equal(leading.prefix.charCodeAt(0) >= 0xdc00 && leading.prefix.charCodeAt(0) <= 0xdfff, false);
// (b) The suffix window ends on the high half of an emoji, so the window stops before it.
const trailing = passageContext("word" + "c".repeat(31) + emoji + "d".repeat(5), 0, 4);
assert.equal(trailing.prefix, "");
assert.equal(trailing.suffix, "c".repeat(31));
assert.equal(trailing.suffix.charCodeAt(trailing.suffix.length - 1) >= 0xd800 && trailing.suffix.charCodeAt(trailing.suffix.length - 1) <= 0xdbff, false);

console.log("source-passage: source passages and their context keep whole characters at the edges");
