// The page and desktop app version check, as pure functions over a range.
import assert from "node:assert/strict";
import { test } from "node:test";
import { compareVersions, isLegacy, LEGACY_RANGE, major, outcome, PAGE, rangeOf, sinceMet } from "../../src/lib/desktop/negotiate.js";

test("the page speaks 3.1.0 and an app without protocol is taken as 3.0.0", () => {
  assert.equal(PAGE, "3.1.0");
  assert.deepEqual(LEGACY_RANGE, { oldest: 3, current: "3.0.0" });
  assert.equal(major("3.1.0"), 3);
  assert.equal(compareVersions("3.10.0", "3.9.0"), 1);
  assert.equal(compareVersions("3.1.0", "3.1.0"), 0);
  assert.equal(compareVersions("3.1.0", "3.1.1"), -1);
  assert.deepEqual(rangeOf({}), LEGACY_RANGE);
  assert.deepEqual(rangeOf(null), LEGACY_RANGE);
  assert.equal(isLegacy({ version: "3.0.0" }), true);
  assert.equal(isLegacy({ protocol: { oldest: 3, current: "3.1.0" } }), false);
});

test("outcome compares the page's major with the app's range", () => {
  assert.equal(outcome({ oldest: 3, current: "3.1.0" }, "3.1.0"), "ok");
  assert.equal(outcome(LEGACY_RANGE, "3.1.0"), "ok");
  assert.equal(outcome({ oldest: 3, current: "3.5.0" }, "3.1.0"), "ok");
  assert.equal(outcome({ oldest: 4, current: "4.0.0" }, "3.1.0"), "server_outdated");
  assert.equal(outcome({ oldest: 3, current: "3.1.0" }, "4.0.0"), "app_outdated");
  assert.equal(outcome({ oldest: 3, current: "4.0.0" }, "3.1.0"), "ok");
  assert.equal(outcome({ oldest: 3, current: "3.1.0" }), "ok");
});

test("sinceMet compares versions within the app's major and accepts any minor of an older one", () => {
  assert.equal(sinceMet({ oldest: 3, current: "3.1.0" }, "3.2.0"), false);
  assert.equal(sinceMet({ oldest: 3, current: "3.1.0" }, "3.1.0"), true);
  assert.equal(sinceMet({ oldest: 3, current: "3.1.1" }, "3.1.0"), true);
  assert.equal(sinceMet({ oldest: 3, current: "3.10.0" }, "3.9.0"), true);
  assert.equal(sinceMet({ oldest: 3, current: "3.9.0" }, "3.10.0"), false);
  assert.equal(sinceMet({ oldest: 3, current: "4.0.0" }, "3.9.0", "3.1.0"), true);
});
