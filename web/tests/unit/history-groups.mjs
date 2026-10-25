// The version panel's grouping, checked without a browser. Each block states
// the rule it tests, because the rules are the contract: which versions make
// one event, and how events fall into the periods the panel lists.

import assert from "node:assert/strict";
import {
  HISTORY_DEFAULTS, buildEvents, buildPeriods, eventKind, nearestPeriod, pathTo, periodTitle, shiftDay,
} from "../../src/lib/history-groups.js";

const TODAY = "2026-10-23";

// Every point gets a unique sha and a seq that follows creation order, so a
// fixture written in chronological order is read in that order.
let counter = 0;
const point = (iso, extra = {}) => {
  counter += 1;
  return {
    sha: String(counter).padStart(64, "0"),
    seq: counter,
    at: iso,
    by: "Vincent",
    label: "",
    why: "sync",
    tree_sha: "",
    frontier: "",
    archive_status: "",
    ...extra,
  };
};

const minutesAfter = (iso, minutes) => new Date(Date.parse(iso) + minutes * 60000).toISOString();

// Sessions on one day, one point every `spacing` minutes from `start`.
const run = (start, count, spacing, extra = {}) => Array.from(
  { length: count },
  (_, index) => point(minutesAfter(start, index * spacing), extra),
);

// Walks every period and its children, depth first.
const walkPeriods = (periods, visit) => {
  for (const period of periods) {
    visit(period);
    if (period.children) walkPeriods(period.children, visit);
  }
};

// --- day arithmetic ---------------------------------------------------------

assert.equal(shiftDay("2026-03-01", -1), "2026-02-28");
assert.equal(shiftDay("2026-12-31", 1), "2027-01-01");
// A spring-forward day: the arithmetic is done at noon UTC, so the answer is
// the next day rather than the same one again.
assert.equal(shiftDay("2026-03-08", 1), "2026-03-09");

assert.equal(HISTORY_DEFAULTS.sessionGapMinutes, 30);
assert.equal(HISTORY_DEFAULTS.recentDays, 7);
assert.equal(HISTORY_DEFAULTS.flattenEvents, 4);
assert.equal(HISTORY_DEFAULTS.openYesterdayEvents, 6);

// --- what a point is for ----------------------------------------------------

{
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { label: "Draft" })), "saved");
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "restore" })), "restore");
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "superseded" })), "restore");
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "push" })), "push");
  // A push carries its message as a label, and is a push all the same.
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { label: "Fix the abstract", why: "push" })), "push");
  // The old reason is an ordinary version now.
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "git" })), "edit");
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "created" })), "created");
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "sync" })), "edit");
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { why: "agent-patch" })), "edit");
  // A label that is empty is no label at all.
  assert.equal(eventKind(point("2026-10-22T10:00:00Z", { label: "", why: "push" })), "push");
}

// --- one version, and a few -------------------------------------------------

{
  const one = point("2026-10-22T10:00:00Z");
  const events = buildEvents([one], "UTC");
  assert.equal(events.length, 1, "one version is one event");
  assert.equal(events[0].type, "editing");
  assert.equal(events[0].key, one.sha);
  assert.equal(events[0].day, "2026-10-22");
  assert.equal(events[0].start, one.at);
  assert.equal(events[0].end, one.at);
  assert.deepEqual(events[0].authors, ["Vincent"]);
}

{
  const five = run("2026-10-22T10:00:00Z", 5, 2);
  const events = buildEvents(five, "UTC");
  assert.equal(events.length, 1, "five versions two minutes apart are one session");
  assert.equal(events[0].points.length, 5);
  assert.equal(events[0].key, five[0].sha, "the key is the oldest point");
  assert.equal(events[0].points[0].sha, five[4].sha, "points are newest first");
}

// --- sessions ---------------------------------------------------------------

{
  // Dozens of autosaves in one day are still one session while each gap is
  // short.
  const hundred = run("2026-10-22T00:00:00Z", 100, 5);
  const events = buildEvents(hundred, "UTC");
  assert.equal(events.length, 1, "100 versions in one day, all within the gap, form one session");
  assert.equal(events[0].points.length, 100);
}

{
  // A long afternoon: one point every twenty minutes never reaches the gap.
  const long = run("2026-10-22T03:00:00Z", 60, 15);
  const events = buildEvents(long, "UTC");
  assert.equal(events.length, 1, "a long session is still one event");
  assert.equal(events[0].points.length, 60);
  assert.equal(events[0].end, long[59].at);
}

{
  // A 31 minute pause ends the session, and a 30 minute one does not.
  const split = [
    point("2026-10-22T09:00:00Z"),
    point("2026-10-22T09:10:00Z"),
    point("2026-10-22T09:20:00Z"),
    point("2026-10-22T09:51:00Z"),
    point("2026-10-22T09:55:00Z"),
  ];
  const events = buildEvents(split, "UTC");
  assert.equal(events.length, 2, "a 31 minute gap splits the day into two sessions");
  assert.deepEqual(events.map((event) => event.points.length), [2, 3]);

  const exact = [point("2026-10-22T10:00:00Z"), point("2026-10-22T10:30:00Z")];
  assert.equal(buildEvents(exact, "UTC").length, 1, "a gap of exactly thirty minutes stays one session");
}

{
  // The gap is a default that a caller may override.
  const tight = run("2026-10-22T10:00:00Z", 2, 10);
  assert.equal(buildEvents(tight, "UTC", { sessionGapMinutes: 5 }).length, 2);
  assert.equal(buildEvents(tight, "UTC").length, 1);
}

{
  // Two sessions on different days never share an event, even with a gap of
  // a few minutes across midnight.
  const midnight = [point("2026-10-22T23:58:00Z"), point("2026-10-23T00:02:00Z")];
  const events = buildEvents(midnight, "UTC");
  assert.equal(events.length, 2, "an event never spans two days");
  assert.deepEqual(events.map((event) => event.day), ["2026-10-23", "2026-10-22"]);
}

// --- push ---------------------------------------------------------------------

{
  // Pushes in quick succession stay separate: each has its own message.
  const pushes = [
    point("2026-10-22T10:00:00Z", { why: "push", label: "First", by: "Alice" }),
    point("2026-10-22T10:02:00Z", { why: "push", label: "Second", by: "Alice" }),
    point("2026-10-22T10:04:00Z", { why: "push", label: "Third", by: "Bob" }),
  ];
  const events = buildEvents(pushes, "UTC");
  assert.equal(events.length, 3);
  assert.ok(events.every((event) => event.type === "push" && event.points.length === 1));
  assert.deepEqual(events.map((event) => event.authors), [["Bob"], ["Alice"], ["Alice"]]);
}

{
  // An edit between two pushes is its own event.
  const same = "2026-10-22T10:00:00Z";
  const points = [
    point(same, { why: "push", label: "One" }),
    point(same, { why: "sync" }),
    point(same, { why: "push", label: "Two" }),
  ];
  const events = buildEvents(points, "UTC");
  assert.deepEqual(events.map((event) => event.type), ["push", "editing", "push"]);
}

// --- authors ----------------------------------------------------------------

{
  // Two people alternate in one session: it is collaborative.
  const shared = [
    point("2026-10-22T10:00:00Z", { by: "Ann" }),
    point("2026-10-22T10:05:00Z", { by: "Ben" }),
    point("2026-10-22T10:10:00Z", { by: "Ann" }),
  ];
  const events = buildEvents(shared, "UTC");
  assert.equal(events.length, 1);
  assert.equal(events[0].type, "collaborative");
  assert.deepEqual(events[0].authors, ["Ann", "Ben"], "authors are distinct and sorted");
}

{
  // "system" and UUID writers are not people, so they do not count.
  const machine = [
    point("2026-10-22T11:00:00Z", { by: "system" }),
    point("2026-10-22T11:05:00Z", { by: "Ann" }),
  ];
  const events = buildEvents(machine, "UTC");
  assert.equal(events[0].type, "editing");
  assert.deepEqual(events[0].authors, ["Ann"]);

  const service = [
    point("2026-10-22T13:00:00Z", { by: "3f2a1b4c-1234-4abc-8def-0123456789ab" }),
    point("2026-10-22T13:05:00Z", { by: "Ann" }),
    point("2026-10-22T13:08:00Z", { by: "" }),
  ];
  assert.deepEqual(buildEvents(service, "UTC")[0].authors, ["Ann"]);
}

// --- saved versions and restores --------------------------------------------

{
  // A named version inside a long run of edits is its own saved event, and it
  // splits the session around it.
  const run1 = run("2026-10-22T10:00:00Z", 5, 2);
  const named = point("2026-10-22T10:10:00Z", { label: "Draft" });
  const run2 = run("2026-10-22T10:12:00Z", 3, 2);
  const events = buildEvents([...run1, named, ...run2], "UTC");
  assert.deepEqual(events.map((event) => event.type), ["editing", "saved", "editing"]);
  assert.equal(events[1].points.length, 1);
  assert.equal(events[1].points[0].label, "Draft");
  assert.equal(events[0].points.length, 3, "the edits after the saved version");
  assert.equal(events[2].points.length, 5, "the edits before it");
}

{
  // A superseded point and the restore that replaced it are one restore
  // event. Edits after them start a new session.
  const pair = [
    point("2026-10-22T10:00:00Z", { why: "superseded" }),
    point("2026-10-22T10:00:00Z", { why: "restore" }),
    point("2026-10-22T10:01:00Z"),
    point("2026-10-22T10:03:00Z"),
  ];
  const events = buildEvents(pair, "UTC");
  assert.deepEqual(events.map((event) => event.type), ["editing", "restore"]);
  assert.deepEqual(events[1].points.map((one) => one.why), ["restore", "superseded"]);
  assert.equal(events[1].key, pair[0].sha);
  assert.equal(events[0].points.length, 2);
}

{
  // A restore with no superseded point before it is an event on its own.
  const lone = [point("2026-10-22T10:00:00Z", { why: "restore" })];
  assert.equal(buildEvents(lone, "UTC")[0].type, "restore");
  assert.equal(buildEvents(lone, "UTC")[0].points.length, 1);
}

// --- keys are stable --------------------------------------------------------

{
  // A newer point appended to the same session does not change the key.
  const first = [point("2026-10-22T10:00:00Z"), point("2026-10-22T10:05:00Z")];
  const before = buildEvents(first, "UTC");
  const after = buildEvents([...first, point("2026-10-22T10:09:00Z")], "UTC");
  assert.equal(after.length, 1);
  assert.equal(before[0].key, first[0].sha);
  assert.equal(after[0].key, before[0].key);
}

{
  // The input order does not matter: the same points group the same way.
  const points = [
    ...run("2026-10-22T09:00:00Z", 4, 3),
    point("2026-10-22T09:20:00Z", { label: "Saved" }),
    point("2026-10-22T11:00:00Z", { why: "push", label: "Update" }),
  ];
  assert.deepEqual(buildEvents([...points].reverse(), "UTC"), buildEvents(points, "UTC"));
}

// --- the timezone decides the day ---------------------------------------------

{
  const late = point("2026-10-22T17:30:00Z");
  assert.equal(buildEvents([late], "UTC")[0].day, "2026-10-22");
  assert.equal(buildEvents([late], "Asia/Tokyo")[0].day, "2026-10-23", "17:30 UTC is the next day in Tokyo");
  const periods = buildPeriods([late], { timeZone: "Asia/Tokyo", today: TODAY });
  assert.equal(periods[0].key, "day:2026-10-23");
}

// --- periods ----------------------------------------------------------------

// Today, yesterday, inside the last seven days, earlier this month, previous
// months, and previous years, each with one event.
// Built oldest first so each point's seq follows its time, as the server's
// does, then listed newest first.
const spread = [
  "2024-03-01T09:00:00Z",
  "2025-12-31T09:00:00Z",
  "2026-08-10T09:00:00Z",
  "2026-09-03T09:00:00Z",
  "2026-10-16T09:00:00Z",
  "2026-10-18T09:00:00Z",
  "2026-10-22T09:00:00Z",
  "2026-10-23T09:00:00Z",
].map((at) => point(at)).reverse();

{
  const periods = buildPeriods(spread, { timeZone: "UTC", today: TODAY });
  assert.deepEqual(periods.map((period) => period.key), [
    "day:2026-10-23",
    "day:2026-10-22",
    "day:2026-10-18",
    "rest:2026-10",
    "month:2026-09",
    "month:2026-08",
    "year:2025",
    "year:2024",
  ]);
  assert.deepEqual(periods.map((period) => period.kind),
    ["day", "day", "day", "rest", "month", "month", "year", "year"]);
  // Few events, so every period lists them directly.
  assert.ok(periods.every((period) => period.events && period.children === null));
  assert.equal(periods[0].count, 1);
  assert.equal(periods[0].newest, "2026-10-23T09:00:00Z");
  assert.equal(periods[0].oldest, "2026-10-23T09:00:00Z");
  assert.equal(periods[0].named, false);

  assert.equal(periodTitle(periods[0], TODAY), "Today");
  assert.equal(periodTitle(periods[1], TODAY), "Yesterday");
  assert.match(periodTitle(periods[2], TODAY), /Oct.*18|18.*Oct/, "a recent day is written with its weekday and date");
  assert.ok(!periodTitle(periods[2], TODAY).includes("2026"), "and without the year when it is this year");
  assert.equal(periodTitle(periods[3], TODAY), "Earlier in October");
  assert.equal(periodTitle(periods[4], TODAY), "September");
  assert.equal(periodTitle(periods[6], TODAY), "2025");
  assert.equal(periodTitle(periods[7], TODAY), "2024");
}

{
  // A month of a past year carries its year in the heading.
  assert.equal(periodTitle({ kind: "month", value: "2025-09" }, TODAY), "September 2025");
  assert.equal(periodTitle({ kind: "month", value: "2026-09" }, TODAY), "September");
  assert.match(periodTitle({ kind: "day", value: "2025-12-31" }, TODAY), /2025/);
}

{
  // Flattening: a rest group with four events lists them, and with five it
  // lists its days instead.
  const four = ["02", "04", "06", "08"].map((dd) => point(`2026-10-${dd}T09:00:00Z`));
  const flatRest = buildPeriods(four, { timeZone: "UTC", today: TODAY });
  assert.equal(flatRest[0].key, "rest:2026-10");
  assert.equal(flatRest[0].events.length, 4);
  assert.equal(flatRest[0].children, null);

  const five = ["02", "04", "06", "08", "10"].map((dd) => point(`2026-10-${dd}T09:00:00Z`));
  const deepRest = buildPeriods(five, { timeZone: "UTC", today: TODAY });
  assert.equal(deepRest[0].key, "rest:2026-10");
  assert.equal(deepRest[0].events, null);
  assert.deepEqual(deepRest[0].children.map((period) => period.key), [
    "day:2026-10-10", "day:2026-10-08", "day:2026-10-06", "day:2026-10-04", "day:2026-10-02",
  ]);
  assert.equal(deepRest[0].count, 5);
}

// The 2025 fixture: five events in March, one in July, and one more in 2025
// so that the year holds more than the flatten limit.
const pastYear = [
  ...["01", "02", "03", "04", "05"].map((dd) => point(`2025-03-${dd}T09:00:00Z`)),
  point("2025-07-09T09:00:00Z"),
];

{
  const periods = buildPeriods([...pastYear, point("2026-10-23T09:00:00Z")], { timeZone: "UTC", today: TODAY });
  const year = periods.find((period) => period.key === "year:2025");
  assert.equal(year.events, null, "a year over the flatten limit lists its months instead");
  assert.deepEqual(year.children.map((period) => period.key), ["month:2025-07", "month:2025-03"]);
  const march = year.children[1];
  assert.equal(march.events, null, "a month over the flatten limit lists its days");
  assert.equal(march.children.length, 5);
  assert.equal(march.children[0].kind, "day");
  assert.equal(year.children[0].events.length, 1, "a small month lists its events directly");
  assert.equal(year.count, 6);
}

{
  // Each period in a large history lists a bounded number of events, and days
  // list their own.
  const periods = buildPeriods(pastYear, { timeZone: "UTC", today: TODAY });
  walkPeriods(periods, (period) => {
    if (!period.events) return;
    if (period.kind === "day") assert.equal(period.events.length, 1);
    else assert.ok(period.events.length <= HISTORY_DEFAULTS.flattenEvents, period.key);
  });
}

// --- open ---------------------------------------------------------------------

// A day of `count` separate sessions, one an hour from 08:00.
const sessions = (day, count) => Array.from(
  { length: count },
  (_, index) => point(`${day}T${String(8 + index).padStart(2, "0")}:00:00Z`),
);

{
  const periods = buildPeriods([...sessions("2026-10-22", 6), point("2026-10-23T10:00:00Z")], { timeZone: "UTC", today: TODAY });
  assert.equal(periods[0].open, true, "the newest top-level period is open");
  assert.equal(periods[1].key, "day:2026-10-22");
  assert.equal(periods[1].open, true, "yesterday with six events starts open");
}

{
  const periods = buildPeriods([...sessions("2026-10-22", 7), point("2026-10-23T10:00:00Z")], { timeZone: "UTC", today: TODAY });
  assert.equal(periods[0].open, true);
  assert.equal(periods[1].open, false, "yesterday with seven events starts closed");
}

{
  const periods = buildPeriods(spread, { timeZone: "UTC", today: TODAY });
  // Yesterday holds one event, so it starts open with the newest period.
  assert.deepEqual(periods.map((period) => period.open), [true, true, false, false, false, false, false, false]);
}

// --- partial ------------------------------------------------------------------

{
  const periods = buildPeriods([...pastYear, point("2026-10-23T09:00:00Z")], { timeZone: "UTC", today: TODAY, partial: true });
  assert.equal(periods[0].partial, false, "the newest period is loaded");
  const year = periods[1];
  assert.equal(year.key, "year:2025");
  assert.equal(year.partial, true, "the oldest top-level period is partial");
  assert.equal(year.children[1].key, "month:2025-03");
  assert.equal(year.children[1].partial, true, "and so is its oldest descendant");
  assert.equal(year.children[0].partial, false);
}

{
  const periods = buildPeriods(pastYear, { timeZone: "UTC", today: TODAY });
  walkPeriods(periods, (period) => assert.equal(period.partial, false));
}

// --- pathTo -------------------------------------------------------------------

{
  const today = point("2026-10-23T09:00:00Z");
  const periods = buildPeriods([...pastYear, today], { timeZone: "UTC", today: TODAY });
  const march = pastYear[2];
  assert.deepEqual(pathTo(periods, march.sha), ["year:2025", "month:2025-03", "day:2025-03-03", march.sha],
    "keys run outermost first and end with the event key");
  assert.deepEqual(pathTo(periods, today.sha), ["day:2026-10-23", today.sha]);
  assert.deepEqual(pathTo(periods, "f".repeat(64)), [], "an absent sha has no path");
  assert.deepEqual(pathTo([], today.sha), []);
}

// --- nearestPeriod ------------------------------------------------------------

{
  const periods = buildPeriods(spread, { timeZone: "UTC", today: TODAY });
  assert.deepEqual(nearestPeriod(periods, "2026-09"), { key: "month:2026-09", exact: true });
  assert.deepEqual(nearestPeriod(periods, "2025-12"), { key: "year:2025", exact: true },
    "a year that lists its events directly answers for a month one of them is in");
  assert.deepEqual(nearestPeriod(periods, "2025-06"), { key: "year:2025", exact: false },
    "and only for such a month: June 2025 has no versions");
}

{
  // Inside a year that lists its months, the month is the deepest match.
  const periods = buildPeriods(pastYear, { timeZone: "UTC", today: TODAY });
  assert.deepEqual(nearestPeriod(periods, "2025-03"), { key: "day:2025-03-05", exact: true },
    "the deepest match is a day in that month when the month lists its days");
  assert.deepEqual(nearestPeriod(periods, "2025-07"), { key: "month:2025-07", exact: true });
}

{
  // A month with nothing in it: the closest period answers, and says so.
  const periods = buildPeriods(spread, { timeZone: "UTC", today: TODAY });
  assert.deepEqual(nearestPeriod(periods, "2026-02"), { key: "year:2025", exact: false },
    "February 2026 is closer to late December 2025 than to August 2026");
  assert.equal(nearestPeriod([], "2026-02"), null);
}

// --- performance ----------------------------------------------------------------

{
  // Ten thousand versions, one every twenty minutes from May. A day holds 72 of
  // them and they are one session, so each day is one event.
  const start = Date.parse("2026-05-01T00:00:00Z");
  const many = Array.from(
    { length: 10000 },
    (_, index) => point(new Date(start + index * 20 * 60000).toISOString()),
  );
  const began = performance.now();
  const periods = buildPeriods(many, { timeZone: "UTC", today: TODAY });
  const took = performance.now() - began;
  assert.ok(took < 1000, `10,000 points took ${Math.round(took)} ms`);
  assert.equal(periods.reduce((sum, period) => sum + period.count, 0), 10000);
  walkPeriods(periods, (period) => {
    if (!period.events) return;
    if (period.kind === "day") assert.equal(period.events.length, 1, period.key);
    else assert.ok(period.events.length <= HISTORY_DEFAULTS.flattenEvents, period.key);
  });
}

// --- the same rules through the options -----------------------------------------

{
  // A caller that lowers the flatten limit gets day children sooner.
  const periods = buildPeriods(pastYear, { timeZone: "UTC", today: TODAY, options: { flattenEvents: 0 } });
  const year = periods.find((period) => period.key === "year:2025");
  assert.ok(year.children, "with no flatten allowance the year lists its months");
  assert.equal(year.children[0].events, null);
}

console.log("history-groups: ok");
