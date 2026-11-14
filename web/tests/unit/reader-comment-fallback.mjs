// anchorComments: a comment placed by its source passage because it was
// attached by hand goes back to its quotation when the attachment is removed,
// even in a `rendered: false` pass.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const reader = readFileSync(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
const from = reader.indexOf("const fallbackPlaced = new WeakSet();");
const to = reader.indexOf("function reanchor()", from);
assert.ok(from !== -1 && to !== -1, "anchorComments not found in Reader.svelte");
const code = reader.slice(from, to);

const anchored = [];
const context = vm.createContext({
  docText: "text",
  docView: {},
  mayEdit: true,
  session: {},
  anchorAll: (_text, list) => { anchored.push([...list]); for (const c of list) { c.start = 1; c.end = 2; c.orphaned = false; } },
  placeSources: () => {},
  applyAnchorFlags: () => {},
  treeNow: () => ({ texts: { "a.tex": "source" } }),
  renderers: { formatOf: () => "latex" },
  sync: { documentRangeFor: () => ({ start: 50, end: 60 }) },
  WeakSet,
});
vm.runInContext(`${code}; this.anchorComments = anchorComments;`, context);

const comment = { attachment: { attached_by_hand: true }, start: null, end: null, sourceStart: 0, sourceEnd: 3, sourcePath: "a.tex" };
const other = { attachment: {}, start: 5, end: 6, sourceStart: null };
context.anchorComments([comment, other]);
assert.equal(comment.start, 50, "a hand attachment is placed by its source passage");
anchored.length = 0;
// The stub's full pass placed `other` too; put it back where its quotation is.
other.start = 5;
other.end = 6;

comment.attachment = { attached_by_hand: false };
context.anchorComments([comment, other], { rendered: false });
assert.equal(anchored.length, 1);
assert.deepEqual(anchored[0], [comment], "only the released comment is placed again");
assert.equal(comment.start, 1, "it is back on its quotation");
assert.equal(other.start, 5);

anchored.length = 0;
context.anchorComments([comment, other], { rendered: false });
assert.equal(anchored.length, 0, "a settled comment costs no search");

// A hand attachment whose source passage is not found on the page leaves the
// comment unplaced; removing the attachment still brings its quotation back.
context.sync.documentRangeFor = () => null;
const unmatched = { attachment: { attached_by_hand: true }, start: null, end: null, sourceStart: 0, sourceEnd: 3, sourcePath: "a.tex" };
context.anchorComments([unmatched], { rendered: false });
assert.equal(unmatched.start, null, "the fallback found nothing");
anchored.length = 0;
unmatched.attachment = { attached_by_hand: false };
context.anchorComments([unmatched], { rendered: false });
assert.deepEqual(anchored[0], [unmatched], "the released comment is searched by its quotation");
assert.equal(unmatched.start, 1);
console.log("reader-comment-fallback: ok");
