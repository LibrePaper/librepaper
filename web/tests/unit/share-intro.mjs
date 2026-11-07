import assert from "node:assert/strict";
import test from "node:test";
import { shareIntroKind } from "../../src/lib/reader/share-intro.js";

test("each share role gets the line for what it can do", () => {
  assert.equal(shareIntroKind("reader"), "view");
  assert.equal(shareIntroKind("commenter"), "comment");
  assert.equal(shareIntroKind("editor"), "edit");
});

test("owners and unknown roles get no line", () => {
  assert.equal(shareIntroKind("owner"), null);
  assert.equal(shareIntroKind(""), null);
  assert.equal(shareIntroKind(undefined), null);
  assert.equal(shareIntroKind("admin"), null);
});
