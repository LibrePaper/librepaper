import assert from "node:assert/strict";
import test from "node:test";
import { anonymousAllowed } from "../../src/components/reader/share/shared.js";

const all = { anonymous: { reads: true, comments: true, suggests: true, edits: true } };
const none = { anonymous: { reads: false, comments: false, suggests: false, edits: false } };

test("each role follows the deployment's flag for it", () => {
  const some = { anonymous: { reads: true, comments: false, suggests: true, edits: false } };
  assert.equal(anonymousAllowed(some, "reader"), true);
  assert.equal(anonymousAllowed(some, "commenter"), false);
  assert.equal(anonymousAllowed(some, "suggester"), true);
  assert.equal(anonymousAllowed(some, "editor"), false);
});

test("every role is allowed when the deployment allows them all, and none when it allows none", () => {
  for (const role of ["reader", "commenter", "suggester", "editor"]) {
    assert.equal(anonymousAllowed(all, role), true);
    assert.equal(anonymousAllowed(none, role), false);
  }
});

test("missing anonymous data means not allowed", () => {
  assert.equal(anonymousAllowed({}, "reader"), false);
  assert.equal(anonymousAllowed({ publishers: "everyone" }, "editor"), false);
  assert.equal(anonymousAllowed({ anonymous: {} }, "reader"), false);
});

test("null or undefined sharing means not allowed", () => {
  assert.equal(anonymousAllowed(null, "reader"), false);
  assert.equal(anonymousAllowed(undefined, "editor"), false);
});

test("unknown roles are not allowed", () => {
  assert.equal(anonymousAllowed(all, "owner"), false);
  assert.equal(anonymousAllowed(all, ""), false);
  assert.equal(anonymousAllowed(all, undefined), false);
});
