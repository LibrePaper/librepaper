// What the preview cache keeps, and what it lets go of.
//
// The database is a browser's and is exercised in the browser suite; the
// policy is not, and the policy is where a mistake costs something. Keeping
// too much fills a quota that something unrelated then fails to write into;
// keeping too little means the pane is blank on open, which is the whole
// thing this exists to fix.
import assert from "node:assert/strict";
import { bytesOf, pageRecord, pagesToDrop } from "../../src/lib/reader/preview-cache.js";

const pdf = (slug, savedAt, bytes) => ({ slug, kind: "pdf", bytes: new Uint8Array(bytes), savedAt });

// The newest pages are kept and the oldest are dropped, by when they were
// drawn rather than by when the document was made.
{
  const pages = [pdf("a", 30, 1), pdf("b", 10, 1), pdf("c", 20, 1)];
  assert.deepEqual(pagesToDrop(pages, { kept: 2, ceiling: 1e9 }), ["b"]);
  assert.deepEqual(pagesToDrop(pages, { kept: 1, ceiling: 1e9 }), ["c", "b"]);
  assert.deepEqual(pagesToDrop(pages, { kept: 9, ceiling: 1e9 }), []);
}

// The byte ceiling is the second bound, and it counts what is actually held.
{
  const pages = [pdf("new", 30, 100), pdf("mid", 20, 100), pdf("old", 10, 100)];
  assert.deepEqual(pagesToDrop(pages, { kept: 9, ceiling: 250 }), ["old"]);
  assert.deepEqual(pagesToDrop(pages, { kept: 9, ceiling: 150 }), ["mid", "old"]);
}

// A single page larger than the whole budget is still kept when it is the
// newest. Dropping it would mean a paper that can never open on its own
// page, which is worse than being briefly over.
{
  const pages = [pdf("huge", 30, 5000)];
  assert.deepEqual(pagesToDrop(pages, { kept: 9, ceiling: 10 }), []);
  const withOthers = [pdf("huge", 30, 5000), pdf("small", 20, 1)];
  assert.deepEqual(pagesToDrop(withOthers, { kept: 9, ceiling: 10 }), ["small"]);
}

// An HTML page is measured as UTF-16, which is what the store holds.
{
  assert.equal(bytesOf({ kind: "html", html: "abc" }), 6);
  assert.equal(bytesOf({ kind: "pdf", bytes: new Uint8Array(7) }), 7);
  assert.equal(bytesOf({ kind: "pdf" }), 0, "a record with nothing in it weighs nothing");
  assert.equal(bytesOf(null), 0);
}

// A record carries the identity of the source it was rendered from: that is
// what lets the next visit say whether the page it is showing is the current
// one or only the last one.
{
  const bytes = new Uint8Array([1, 2, 3]);
  const record = pageRecord("paper", { kind: "pdf", bytes }, "sha-1", 99);
  assert.equal(record.slug, "paper");
  assert.equal(record.identity, "sha-1");
  assert.equal(record.savedAt, 99);
  assert.deepEqual([...record.bytes], [1, 2, 3]);
  // Its own copy: the frame is handed a fresh buffer on every transfer and
  // the one it was given may be detached by the time this is written.
  bytes[0] = 9;
  assert.equal(record.bytes[0], 1, "the record does not alias the page it was given");
}

{
  const record = pageRecord("paper", { kind: "html", html: "<p>x</p>" }, "sha-2", 5);
  assert.equal(record.kind, "html");
  assert.equal(record.html, "<p>x</p>");
  assert.equal(pageRecord("", { kind: "html" }, "", 0), null, "a page needs a document");
  assert.equal(pageRecord("paper", {}, "", 0), null, "and a kind");
}

console.log("preview-cache: the last page drawn is kept, and the oldest let go");
