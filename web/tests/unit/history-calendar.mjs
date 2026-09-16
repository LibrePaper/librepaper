// The two steps the version panel is: a month of days, and one of those days
// laid out on a vertical axis of time.
//
// The arithmetic is the part worth checking without a browser. Which week a
// day falls in, which month a day either side of the grid belongs to, which
// minute of the day a timestamp is at once the reader's own timezone is
// applied -- get any of those wrong and the panel still draws, it is just
// wrong about when things happened. The last of those matters more than it
// used to: the day view positions a bar and a version by the same number, so
// one mistake in it moves both.

import assert from "node:assert/strict";
import {
  daySessions, dayVersions, monthGrid, monthOf, monthSpan, sessionBins,
  sessionSparkline, shiftDay, shiftMonth, versionDays,
} from "../../src/lib/history-calendar.js";

const mark = (at, extra = {}) => ({
  sha: String(at).replace(/\D/g, "").padEnd(64, "0"), at, by: "Vincent", why: "quiet", label: "", ...extra,
});
const row = (at, changes, state_bytes, frontier = "") => ({ at, changes, state_bytes, frontier, peer: "" });

// --- day and month arithmetic -----------------------------------------------

assert.equal(shiftDay("2026-03-01", -1), "2026-02-28");
assert.equal(shiftDay("2026-12-31", 1), "2027-01-01");
// A spring-forward day: the arithmetic is done at noon UTC, so the answer is
// the next day rather than the same one again.
assert.equal(shiftDay("2026-03-08", 1), "2026-03-09");

assert.equal(shiftMonth("2026-09", 1), "2026-10");
assert.equal(shiftMonth("2026-12", 1), "2027-01");
assert.equal(shiftMonth("2026-01", -1), "2025-12");
assert.equal(shiftMonth("2026-09", 12), "2027-09");
// A month has no thirty-first, so a month step is counted in months and not
// in days: January stepped forward is February, never the third of March.
assert.equal(shiftMonth("2026-01", 1), "2026-02");
assert.equal(monthOf("2026-09-16"), "2026-09");

// --- the days a document has ------------------------------------------------

{
  const byDay = versionDays(
    [
      mark("2026-09-01T09:00:00Z"),
      mark("2026-09-01T17:30:00Z", { label: "Sent to the journal" }),
      mark("2026-09-10T11:00:00Z"),
    ],
    [row("2026-09-05T08:00:00Z", 4, 1000), row("2026-09-10T11:00:00Z", 2, 1400)],
    "UTC",
  );
  assert.equal(byDay.get("2026-09-01").count, 2, "a day sums the versions on it");
  assert.equal(byDay.get("2026-09-01").named, true, "and remembers that one of them was named");
  assert.equal(byDay.get("2026-09-10").named, false);
  assert.equal(byDay.get("2026-09-05").count, 0, "a day nothing was saved on has no versions");
  assert.equal(byDay.get("2026-09-05").worked, true, "but is still a day somebody wrote on");
  assert.equal(byDay.get("2026-09-05").changes, 4);
  assert.deepEqual(
    byDay.get("2026-09-01").points.map((point) => point.at),
    ["2026-09-01T09:00:00Z", "2026-09-01T17:30:00Z"],
    "a day keeps its versions oldest first",
  );
  // The day boundary is the reader's. 17:30 UTC is past midnight in Tokyo,
  // so that version belongs to the second there.
  const tokyo = versionDays([mark("2026-09-01T17:30:00Z")], [], "Asia/Tokyo");
  assert.deepEqual([...tokyo.keys()], ["2026-09-02"]);
}

// --- the month grid ---------------------------------------------------------

{
  const byDay = versionDays(
    [
      mark("2026-09-01T09:00:00Z"), mark("2026-09-01T10:00:00Z"),
      mark("2026-09-01T11:00:00Z"), mark("2026-09-01T12:00:00Z"),
      mark("2026-09-10T11:00:00Z", { label: "Draft" }),
    ],
    [row("2026-09-05T08:00:00Z", 4, 1000)],
    "UTC",
  );
  const { weeks, most } = monthGrid("2026-09", byDay, { today: "2026-09-16" });
  assert.equal(most, 4, "the busiest day of this month sets the shading scale");
  assert.ok(weeks.every((week) => week.length === 7), "every row is a whole week");
  const cells = weeks.flat();
  assert.equal(new Date(`${cells[0].day}T12:00:00Z`).getUTCDay(), 0, "the grid starts on a Sunday");
  assert.equal(new Date(`${cells[cells.length - 1].day}T12:00:00Z`).getUTCDay(), 6,
    "and ends on a Saturday, so the grid is rectangular");
  // September 2026 starts on a Tuesday, so the first two cells are August's.
  assert.equal(cells[0].day, "2026-08-30");
  assert.equal(cells[0].inMonth, false, "a day either side of the month is drawn, and says so");
  assert.equal(cells[2].day, "2026-09-01");
  assert.equal(cells[2].date, 1, "a cell carries its own date, because a month is counted across");
  assert.equal(cells[2].count, 4);
  assert.equal(cells[2].level, 4, "the busiest day takes the strongest shade");
  const find = (day) => cells.find((cell) => cell.day === day);
  assert.equal(find("2026-09-10").count, 1);
  assert.equal(find("2026-09-10").level, 1, "one version is the faintest shade, never nothing");
  assert.equal(find("2026-09-10").named, true);
  assert.equal(find("2026-09-05").level, 0, "a day nothing was saved on takes no shade");
  assert.equal(find("2026-09-05").worked, true, "and says separately that somebody wrote on it");
  assert.equal(find("2026-09-16").today, true);
  assert.ok(cells.some((cell) => cell.day.startsWith("2026-10")),
    "the run-on into October is drawn rather than left as holes in the corner");
  assert.equal(monthGrid("nonsense", byDay).weeks.length, 0, "a month that is not one draws nothing");
}

// --- how far the panel can walk ---------------------------------------------

{
  const byDay = versionDays([mark("2025-11-03T09:00:00Z"), mark("2026-02-01T09:00:00Z")], [], "UTC");
  assert.deepEqual(monthSpan(byDay, "2026-09-16"), { first: "2025-11", last: "2026-09" },
    "from the first month worked to the month today is in");
  assert.deepEqual(monthSpan(new Map(), "2026-09-16"), { first: "2026-09", last: "2026-09" },
    "a document with no past is still somewhere");
}

// --- the day, cut where the work stopped -----------------------------------

{
  // Two sittings an hour apart, and a publish on its own in between: the day
  // is three things that happened, not twenty-four hours of mostly nothing.
  const rows = [
    row("2026-09-14T09:05:00Z", 2, 1000, "one"),
    row("2026-09-14T09:07:00Z", 3, 1600, "two"),
    row("2026-09-14T09:20:00Z", 1, 1700, "three"),
    row("2026-09-14T14:00:00Z", 4, 2000, "four"),
    row("2026-09-14T14:02:00Z", 1, 2100, "five"),
    row("2026-09-15T10:00:00Z", 9, 9000, "elsewhere"),
  ];
  const points = [
    mark("2026-09-14T09:06:00Z"),
    mark("2026-09-14T11:30:00Z", { why: "cli" }),
    mark("2026-09-14T14:01:00Z", { label: "Draft" }),
    mark("2026-09-15T10:00:00Z"),
  ];
  const sessions = daySessions(points, rows, "2026-09-14", "UTC");
  assert.equal(sessions.length, 3, "a day is its sittings, and a five-hour gap is not one of them");
  assert.deepEqual(sessions.map((one) => [one.from, one.to]),
    [[545, 560], [690, 690], [840, 842]]);
  assert.equal(sessions[0].writes, 6, "a sitting sums the writes in it");
  assert.equal(sessions[0].versions.length, 1);
  assert.equal(sessions[0].span, 15);
  // A publish leaves a version and no writes at all, and it is still
  // something that happened at half past eleven.
  assert.equal(sessions[1].writes, 0, "a version with nothing written around it is its own event");
  assert.equal(sessions[1].versions.length, 1);
  assert.equal(sessions[1].moments.length, 0);
  assert.equal(sessions[2].versions[0].point.label, "Draft");
  assert.ok(!sessions.some((one) => one.moments.some((m) => m.frontier === "elsewhere")),
    "and a day holds only its own");
  // The silence before each one, so the feed can say it in words instead of
  // spending height on it. The first of the day has nothing before it.
  assert.equal(sessions[0].since, null);
  assert.equal(sessions[1].since, 130, "two hours and ten minutes of nothing");
  assert.equal(sessions[2].since, 150);
}

{
  // The gap is the caller's to set: what counts as one sitting is a fact
  // about how somebody works, not about how the panel draws.
  const rows = [
    row("2026-09-14T09:00:00Z", 1, 100, "a"),
    row("2026-09-14T09:40:00Z", 1, 200, "b"),
  ];
  assert.equal(daySessions([], rows, "2026-09-14", "UTC").length, 2,
    "forty minutes apart is two sittings at the default");
  assert.equal(daySessions([], rows, "2026-09-14", "UTC", { gap: 60 }).length, 1,
    "and one when an hour of quiet still counts as the same sitting");
  assert.equal(daySessions([], rows, "2026-09-14", "UTC", { gap: 0 }).length, 2,
    "and a gap of nothing never merges two different minutes");
  assert.deepEqual(daySessions([], [], "2026-09-14", "UTC"), [],
    "a day nobody worked has no sittings at all, rather than one empty one");
}

{
  // The reader's own day and clock: 23:50 UTC is the small hours in Tokyo, so
  // the sitting is on the next day there, and ten to midnight is ten to nine.
  const rows = [row("2026-09-14T23:50:00Z", 1, 100, "late")];
  assert.deepEqual(daySessions([], rows, "2026-09-15", "Asia/Tokyo").map((one) => one.from), [530]);
  assert.deepEqual(daySessions([], rows, "2026-09-14", "Asia/Tokyo"), []);
}

// --- the shape of one sitting ----------------------------------------------

{
  const rows = [
    row("2026-09-14T09:00:00Z", 1, 100, "a"),
    row("2026-09-14T09:14:00Z", 8, 900, "b"),
    row("2026-09-14T09:15:00Z", 2, 1000, "c"),
  ];
  const [sitting] = daySessions([], rows, "2026-09-14", "UTC");
  const spark = sessionSparkline(sitting, 8);
  assert.equal(spark.length, 8, "a sparkline is the same width whatever the sitting's length");
  assert.equal(sessionSparkline(sitting, 20).length, 20);
  assert.equal(spark[0].changes, 1, "the first two minutes hold the first write");
  assert.equal(spark[7].changes, 10, "and the last two the burst at the end");
  assert.equal(spark[7].share, 1, "the busiest bar is full");
  assert.equal(spark[3].share, 0, "and the quiet ones are empty rather than missing");
  // A sitting one minute long is still a shape, not a division by zero.
  const [instant] = daySessions([], [row("2026-09-14T09:00:00Z", 3, 50, "x")], "2026-09-14", "UTC");
  assert.equal(sessionSparkline(instant).filter((bar) => bar.share > 0).length, 1);
}

// --- one sitting, on a real axis -------------------------------------------

{
  const rows = [
    row("2026-09-14T14:00:00Z", 2, 1000, "a"),
    row("2026-09-14T14:01:00Z", 3, 1600, "b"),
    row("2026-09-14T14:09:00Z", 1, 1650, "c"),
  ];
  const [sitting] = daySessions([], rows, "2026-09-14", "UTC");
  const bins = sessionBins(sitting);
  assert.equal(bins.length, 10, "a ten-minute sitting is ten one-minute bins, all of them");
  assert.equal(bins[0].at, 840, "and a bin is placed on the day's own axis, not the sitting's");
  assert.equal(bins[9].at, 849);
  assert.equal(bins[1].changes, 3);
  // The work is the growth since the minute before, so the first minute of a
  // document's life carries the whole of it and the scale is set by that.
  assert.equal(bins[1].work, 600);
  assert.equal(bins[0].share, 1, "the busiest minute fills its bar");
  assert.equal(bins[1].share, 0.6);
  assert.equal(bins[9].level, 1, "one write is never nothing");
  assert.equal(bins[5].share, 0, "a minute nobody wrote in draws no bar at all");
  assert.deepEqual(bins[0].moments.map((one) => one.frontier), ["a"],
    "and a bin keeps its minutes, because clicking the bar is how one is opened");
  // Coarser bins still cover the whole sitting and nothing beyond it.
  const coarse = sessionBins(sitting, 4);
  assert.equal(coarse.length, 3);
  assert.equal(coarse[0].at, 840);
  assert.equal(coarse.reduce((sum, bin) => sum + bin.changes, 0), 6, "nothing is dropped");
}

{
  // A document written before the server recorded growth has none, so the
  // strip falls back to counting writes rather than drawing a flat sitting.
  const rows = [
    { at: "2026-09-14T09:00:00Z", changes: 1, state_bytes: 0, frontier: "a", peer: "" },
    { at: "2026-09-14T09:01:00Z", changes: 4, state_bytes: 0, frontier: "b", peer: "" },
  ];
  const [sitting] = daySessions([], rows, "2026-09-14", "UTC");
  const bins = sessionBins(sitting);
  assert.equal(bins[0].level, 1);
  assert.equal(bins[1].level, 4, "the busier of the two minutes is the busier bar");
}

console.log("history calendar: whole weeks either side of the month, a day cut into sittings, and one sitting on an axis of minutes");
