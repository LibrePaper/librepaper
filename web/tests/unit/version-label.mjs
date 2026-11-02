import assert from "node:assert/strict";
import test from "node:test";
import { versionLabels, endpointLabel } from "../../src/lib/version-label.js";
import { CURRENT, NOTHING } from "../../src/lib/history-endpoints.js";
import { locale } from "../../src/lib/locale.js";
import { m } from "../../src/lib/paraglide/messages.js";

// Fixed in UTC, so the day boundaries are the same wherever the suite runs.
const now = new Date("2026-11-02T12:00:00Z");
const timeZone = "UTC";
const clock = (at, extra = {}) => new Date(at).toLocaleTimeString(locale(), { hour: "numeric", minute: "2-digit", timeZone, ...extra });
const dated = (at, extra = {}) => new Date(at).toLocaleString(locale(), {
  month: "short", day: "numeric", hour: "numeric", minute: "2-digit", timeZone, ...extra,
});

test("today and yesterday are named by day, with the clock time", () => {
  const labels = versionLabels([
    { sha: "today", at: "2026-11-02T09:30:00Z" },
    { sha: "yesterday", at: "2026-11-01T18:05:00Z" },
  ], { timeZone, now });
  assert.equal(labels.get("today"), m.common_today_at({ time: clock("2026-11-02T09:30:00Z") }));
  assert.equal(labels.get("yesterday"), m.common_yesterday_at({ time: clock("2026-11-01T18:05:00Z") }));
});

test("earlier this year gives the month and day, and other years add the year", () => {
  const labels = versionLabels([
    { sha: "spring", at: "2026-03-14T08:00:00Z" },
    { sha: "last-year", at: "2025-03-14T08:00:00Z" },
  ], { timeZone, now });
  assert.equal(labels.get("spring"), dated("2026-03-14T08:00:00Z"));
  assert.equal(labels.get("last-year"), dated("2025-03-14T08:00:00Z", { year: "numeric" }));
});

test("the day before midnight of a new year is yesterday, not last year", () => {
  const labels = versionLabels([
    { sha: "late", at: "2026-12-31T23:10:00Z" },
  ], { timeZone, now: new Date("2027-01-01T00:30:00Z") });
  assert.equal(labels.get("late"), m.common_yesterday_at({ time: clock("2026-12-31T23:10:00Z") }));
});

test("versions that would read the same get their seconds, and only those", () => {
  const labels = versionLabels([
    { sha: "first", at: "2026-03-14T08:00:05Z" },
    { sha: "second", at: "2026-03-14T08:00:40Z" },
    { sha: "other", at: "2026-03-14T09:00:00Z" },
  ], { timeZone, now });
  const withSeconds = (at) => dated(at, { second: "2-digit" });
  assert.equal(labels.get("first"), withSeconds("2026-03-14T08:00:05Z"));
  assert.equal(labels.get("second"), withSeconds("2026-03-14T08:00:40Z"));
  assert.notEqual(labels.get("first"), labels.get("second"));
  assert.equal(labels.get("other"), dated("2026-03-14T09:00:00Z"));
});

test("a bookmark leads with its name", () => {
  const labels = versionLabels([
    { sha: "named", at: "2026-11-02T09:30:00Z", label: "Draft review" },
  ], { timeZone, now });
  const when = m.common_today_at({ time: clock("2026-11-02T09:30:00Z") });
  assert.equal(labels.get("named"), m.common_named_version({ name: "Draft review", when }));
});

test("a point without a usable time keeps its name, or has no entry", () => {
  const labels = versionLabels([
    { sha: "named", label: "Submitted" },
    { sha: "untimed", at: "not a date" },
  ], { timeZone, now });
  assert.equal(labels.get("named"), "Submitted");
  assert.equal(labels.has("untimed"), false);
});

test("endpoints: the sentinels and moments by their words, a version by its name", () => {
  const names = new Map([["abc1234def", "Named version"]]);
  assert.equal(endpointLabel(CURRENT, names), m.history_current_version());
  assert.equal(endpointLabel(NOTHING, names), m.history_before_first());
  assert.equal(endpointLabel("frontier:xyz", names), m.history_source_earlier_moment());
  assert.equal(endpointLabel("abc1234def", names), "Named version");
  assert.equal(endpointLabel("0123456789abcdef", names), "0123456");
});
