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
  dayEntries, dayVersions, deliberate, monthGrid, monthOf, monthSpan,
  shiftDay, shiftMonth, unsavedMinutes, versionDays,
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

// --- one day's versions -----------------------------------------------------

{
  const points = [
    mark("2026-09-14T09:05:00Z"),
    mark("2026-09-14T14:30:00Z", { label: "Draft" }),
    mark("2026-09-15T10:00:00Z"),
  ];
  const versions = dayVersions(points, "2026-09-14", "UTC");
  assert.deepEqual(versions.map((one) => one.minute), [545, 870],
    "a version is placed by minutes from midnight, which is what the list reads");
  assert.equal(versions.length, 2, "and a day holds only its own");
  assert.deepEqual(versions.map((one) => one.point.at),
    ["2026-09-14T09:05:00Z", "2026-09-14T14:30:00Z"], "oldest first");
  // The reader's own day and the reader's own clock: 14:30 UTC is 23:30 in
  // Tokyo, still that day, and 15:00 UTC is midnight of the next one.
  assert.deepEqual(dayVersions(points, "2026-09-14", "Asia/Tokyo").map((one) => one.minute),
    [1085, 1410]);
  assert.deepEqual(dayVersions([mark("2026-09-14T15:00:00Z")], "2026-09-15", "Asia/Tokyo")
    .map((one) => one.minute), [0]);
}

// --- a day that fits is a day that is listed --------------------------------

{
  // The rule that comes before all the others, and the one whose absence made
  // a seeded example show a single row that could not be chosen: coarsening
  // is for a day too big to read. Applied to a day of three it gathers them
  // into one entry that opens into three -- which saves nobody anything and
  // takes away the only thing the panel does, because a version is chosen by
  // clicking it and an entry standing for several cannot be.
  const day = [
    mark("2026-09-14T09:26:00Z", { sha: "a".repeat(64) }),
    mark("2026-09-14T09:29:00Z", { sha: "b".repeat(64) }),
    mark("2026-09-14T09:33:00Z", { sha: "c".repeat(64) }),
  ];
  const entries = dayEntries(day, "2026-09-14", "UTC");
  assert.deepEqual(entries.map((one) => one.kind), ["version", "version", "version"],
    "three autosaves minutes apart are three rows, not one entry standing for them");
  assert.deepEqual(entries.map((one) => one.points.length), [1, 1, 1]);
  // It is the ceiling that decides, and it is the caller's.
  assert.ok(dayEntries(day, "2026-09-14", "UTC", { most: 2, gathered: 2 })
    .some((one) => one.kind === "run"), "and a day past the ceiling is gathered again");
  // Newest first, whichever way the day was built: the last thing somebody
  // did is the first row, because it is what they nearly always came for.
  assert.deepEqual(entries.map((one) => one.points[0].sha[0]), ["c", "b", "a"],
    "the day is listed from its end");
}

// --- a day, coarsened by significance ---------------------------------------

// Every fixture below is a handful of versions, which the rule above would
// list rather than gather -- so each says `most` out loud. What is under test
// here is what happens to a day that does *not* fit.
const coarse = (points, options = {}) =>
  dayEntries(points, "2026-09-14", "UTC", { most: 4, gathered: 2, ...options });

{
  // The server saves a version after thirty seconds of quiet, so an
  // afternoon is dozens of them. What survives that is not a shorter list but
  // a different rule: anything somebody asked for stands alone, and the
  // autosaves between two of those are one entry. Every boundary is something
  // a reader would recognise -- never an interval the panel chose.
  const quiet = (minute, changed) =>
    mark(`2026-09-14T${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}:00Z`,
      { sha: String(minute).padStart(64, "0"), changed });
  const points = [
    quiet(540, ["main.tex"]),
    quiet(542, ["main.tex"]),
    quiet(544, ["references.bib"]),
    quiet(546, ["main.tex"]),
    { ...quiet(600), why: "cli", changed: ["main.tex"] },
    quiet(602, ["main.tex"]),
    quiet(604, ["main.tex"]),
    quiet(606, ["main.tex"]),
    { ...quiet(700), label: "Sent to coauthors", changed: ["main.tex"] },
  ];
  const entries = coarse(points);
  // Newest first: the named version, the autosaves after the publish, the
  // publish, and the morning's run last.
  assert.deepEqual(entries.map((one) => one.kind),
    ["version", "run", "version", "run"],
    "the autosaves gather; the publish and the named one never do");
  const morning = entries[3];
  assert.equal(morning.points.length, 4, "a run holds the versions it stands for");
  // The list is turned over; a run's own span is not. "9:00 - 9:06" is a fact
  // about the work rather than a reading order.
  assert.deepEqual([morning.minute, morning.to], [540, 546], "and says when it ran");
  assert.deepEqual(morning.points.map((one) => one.at.slice(11, 16)),
    ["09:06", "09:04", "09:02", "09:00"], "and opens onto its versions newest first");
  // What a clock range cannot say, and what somebody nearly always came for.
  assert.deepEqual(morning.changed, ["main.tex", "references.bib"],
    "a run says what the whole of it changed");
  assert.equal(entries[2].points[0].why, "cli");
  assert.equal(entries[0].points[0].label, "Sent to coauthors",
    "a name is enough on its own to keep a version out of a run");
  // Every version is still reachable: coarsening hides nothing.
  assert.equal(entries.flatMap((one) => one.points).length, points.length);
  // An entry is named by the oldest version in it, which is what makes the
  // key steady: turning the list over must not rename the rows an open run
  // is remembered by.
  assert.deepEqual(entries.map((one) => one.key),
    entries.map((one) => one.points[one.points.length - 1].sha),
    "and each entry is named by the first version in it");
}

{
  // A different name on two checkpoints is NOT a boundary, and this is the
  // test that says so on purpose. An autosave is attributed to whoever sent
  // the last update before it fired, so on a document two people are writing
  // at once the name alternates with typing order while every checkpoint
  // holds both their work. Splitting on it would invent a handover that never
  // happened and credit each run to one of them.
  const hand = (minute, by) =>
    mark(`2026-09-14T09:${String(minute).padStart(2, "0")}:00Z`,
      { sha: `${by}${minute}`.padEnd(64, "0"), by, changed: ["main.tex"] });
  const together = coarse([
    hand(0, "vincent"), hand(2, "anne"), hand(4, "vincent"),
    hand(6, "anne"), hand(8, "anne"), hand(10, "vincent"),
  ]);
  // Two, because six is past the ceiling this fixture set -- not six, which
  // is what splitting on the name would have given.
  assert.equal(together.length, 2,
    "two people writing together are cut by the ceiling, never by the name");
  assert.deepEqual(together.map((one) => one.points.length), [3, 3]);
}

{
  // A pause long enough to be a break in the work is a boundary. The server
  // saves at least every five minutes of continuous writing, so ten leaves
  // room either side of "still working".
  const tick = (minute) =>
    mark(`2026-09-14T${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}:00Z`,
      { sha: String(minute).padStart(64, "0"), changed: ["main.tex"] });
  const broken = coarse([tick(540), tick(544), tick(548), tick(600), tick(604), tick(608)]);
  assert.deepEqual(broken.map((one) => [one.minute, one.to]), [[600, 608], [540, 548]],
    "fifty-two minutes of nothing ends a run");
  // How long a pause has to be is the caller's, like everything else here.
  assert.equal(coarse([tick(540), tick(544), tick(548)], { most: 2, pause: 2 }).length, 3,
    "with an impatient threshold, nothing gathers at all");
}

{
  // What one entry may stand for has a ceiling, because the arithmetic gives
  // it one: between two deliberate versions the server saves at least every
  // thirty seconds, so an unbroken afternoon reaches several hundred, and
  // opening one row onto three hundred near-identical times finds nothing.
  const steady = (count, step) => Array.from({ length: count }, (_, index) => {
    const minute = 540 + Math.round(index * step);
    return mark(
      `2026-09-14T${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}:00Z`,
      { sha: String(index).padStart(64, "0"), changed: ["main.tex"] },
    );
  });
  const long = dayEntries(steady(90, 2), "2026-09-14", "UTC");
  assert.ok(long.length > 1, "ninety versions in one stretch is not one entry");
  assert.ok(long.every((one) => one.points.length <= 25),
    "and no entry stands for more than a screenful: "
    + long.map((one) => one.points.length).join());
  assert.equal(long.flatMap((one) => one.points).length, 90, "with none of them dropped");
  // The ceiling is a ceiling, not a bucket size: a run under it is never cut.
  assert.equal(dayEntries(steady(90, 2), "2026-09-14", "UTC", { most: 200 }).length, 90,
    "past the ceiling, a day that fits is listed again -- one knob, one idea");
}

{
  // Where the cut falls. A run of work is divided where the work paused
  // longest, not at every twenty-fifth version -- a cut at a count lands in
  // the middle of whatever somebody was doing.
  const beat = (minutes) => minutes.map((minute, index) => mark(
    `2026-09-14T${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}:00Z`,
    { sha: String(index).padStart(64, "0"), changed: ["main.tex"] },
  ));
  // Six versions a minute apart, with a nine-minute pause after the fourth --
  // not long enough to end a run on its own, but the obvious place to cut.
  const uneven = beat([540, 541, 542, 543, 552, 553]);
  assert.deepEqual(coarse(uneven).map((one) => [one.minute, one.to]), [[552, 553], [540, 543]],
    "the cut falls at the longest pause inside the run");
  assert.deepEqual(coarse(uneven, { gathered: 3 }).map((one) => one.kind),
    ["version", "version", "run"],
    "and a leftover too short to gather is simply its versions");

  // The degenerate case, and the reason the tie-break exists: somebody typing
  // steadily for hours has every gap identical. Taking the first of the tied
  // pauses would shave one version off the front over and over and leave a
  // row for nearly every version.
  const flat = beat(Array.from({ length: 40 }, (_, index) => 540 + index));
  const halved = dayEntries(flat, "2026-09-14", "UTC", { most: 10 });
  assert.ok(halved.length <= 5,
    "an unvarying run is halved, not shaved: " + halved.length + " rows");
  assert.ok(halved.every((one) => one.points.length <= 10));
  assert.equal(halved.flatMap((one) => one.points).length, 40);
}

{
  // A run that cannot account for itself claims nothing: one version's
  // silence makes the union a guess rather than a total.
  const points = [
    mark("2026-09-14T09:00:00Z", { sha: "a".repeat(64), changed: ["main.tex"] }),
    mark("2026-09-14T09:02:00Z", { sha: "b".repeat(64) }),
    mark("2026-09-14T09:04:00Z", { sha: "c".repeat(64), changed: ["main.tex"] }),
  ];
  assert.equal(coarse(points, { most: 2 })[0].changed, null);
}

{
  // What counts as asked for. A name is the most deliberate thing anybody
  // does to a checkpoint, whatever the document's own reason for taking it.
  assert.equal(deliberate({ why: "quiet" }), false);
  assert.equal(deliberate({ why: "left" }), false);
  assert.equal(deliberate({ why: "sync" }), false);
  assert.equal(deliberate({ why: "quiet", label: "Draft" }), true);
  assert.equal(deliberate({ why: "cli" }), true);
  assert.equal(deliberate({ why: "restore" }), true);
}

// --- what was written and never saved --------------------------------------

{
  const rows = [
    row("2026-09-14T09:05:00Z", 2, 1000, "one"),
    row("2026-09-14T09:05:40Z", 1, 1100, "two"),
    row("2026-09-14T09:40:00Z", 3, 1600, "three"),
    row("2026-09-14T14:00:00Z", 1, 1650, "four"),
    row("2026-09-15T10:00:00Z", 9, 9000, "elsewhere"),
  ];
  // The minute a version was taken in is that version's minute, and belongs
  // to the list of versions rather than to this one.
  const points = [mark("2026-09-14T09:40:30Z")];
  const left = unsavedMinutes(points, rows, "2026-09-14", "UTC");
  // Newest first, like the versions these hang under.
  assert.deepEqual(left.map((one) => one.minute), [840, 545],
    "only the minutes no version covers");
  assert.equal(left[1].changes, 3, "a minute sums the writes in it");
  assert.deepEqual(left.map((one) => one.frontier), ["four", "two"],
    "and each opens the last anchor left in it");
  assert.ok(!left.some((one) => one.frontier === "elsewhere"), "the day holds only its own");
  // A minute with no anchor cannot be opened, so offering it would be a row
  // that does nothing.
  const anchorless = unsavedMinutes([], [
    { at: "2026-09-14T11:00:00Z", changes: 1, state_bytes: 10, frontier: "", peer: "" },
  ], "2026-09-14", "UTC");
  assert.deepEqual(anchorless, []);
  // The reader's own day and clock again: two in the afternoon in UTC is
  // eleven at night in Tokyo, still the same day and near the end of it.
  const tokyo = unsavedMinutes([], rows, "2026-09-14", "Asia/Tokyo");
  assert.deepEqual(tokyo.map((one) => [one.frontier, one.minute]),
    [["four", 1380], ["three", 1120], ["two", 1085]]);
  assert.deepEqual(
    unsavedMinutes([], rows, "2026-09-15", "Asia/Tokyo").map((one) => one.frontier),
    ["elsewhere"],
  );
}

console.log("history calendar: whole weeks either side of the month, and a day coarsened by significance");
