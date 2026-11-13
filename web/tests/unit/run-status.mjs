import assert from "node:assert/strict";
import { test } from "node:test";
import { RUN_STATUS_DELAY_MS, elapsedSeconds, runStatusVisible } from "../../src/lib/reader/run-status.js";

test("a fast run never shows the corner status", () => {
  assert.equal(runStatusVisible(0), false);
  assert.equal(runStatusVisible(RUN_STATUS_DELAY_MS - 1), false);
  assert.equal(runStatusVisible(RUN_STATUS_DELAY_MS), true);
});

test("elapsed time counts whole seconds and never goes negative", () => {
  assert.equal(elapsedSeconds(2999), 2);
  assert.equal(elapsedSeconds(4000), 4);
  assert.equal(elapsedSeconds(-50), 0);
});
