// unsuggestedText gives back the file without its pending suggestions. The
// Rust twin runs the same fixture.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LoroDoc, LoroText } from "loro-crdt";
import { trackedSplice, unsuggestedText } from "../../src/lib/track-changes.js";

const cases = JSON.parse(readFileSync(new URL("../fixtures/unsuggested.json", import.meta.url), "utf8"));
const ME = { who: "me", author: "Me" };

for (const c of cases) {
  const doc = new LoroDoc();
  const body = doc.getMap("files").setContainer("f1", new LoroText());
  if (c.text) body.insert(0, c.text);
  doc.commit();
  for (const e of c.edits) {
    trackedSplice(doc, "f1", e.from, e.to, e.insert, ME, { now: 1 });
    doc.commit();
  }
  assert.equal(unsuggestedText(doc, "f1"), c.expected, c.name);
}

assert.equal(unsuggestedText(new LoroDoc(), "missing"), null);

console.log(`unsuggested: ${cases.length} cases ok`);
