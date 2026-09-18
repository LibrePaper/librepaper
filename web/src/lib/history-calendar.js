// A month of the document's life, and the versions on one of its days.
//
// The version panel asks two questions. *Which day?* -- a month laid out as a
// grid, each cell knowing how many versions landed on it and whether anybody
// wrote on it at all. *Which version?* -- that day's versions, in order.
//
// The second answer has to survive the volume. Nothing writes a version on a
// timer any more, but the command line asks for one on every save, so a day
// spent editing a paper locally is still dozens of them. Listing them all is
// the one thing the panel must not do.
//
// The coarsening is by *significance*, not by the clock. A run of machine
// saves is one entry that says what the whole of it changed and opens into
// the individual times when asked, and a run ends where something happened
// that a reader would recognise as a boundary: a version somebody asked for
// by name, or a pause long enough to be a break in the work.
//
// A different person at the keyboard would be the third such boundary, and it
// is deliberately not one, because nothing in the history can tell us. A
// checkpoint's author is whoever sent the most recent update before it was
// written -- `session.by` is overwritten by every edit -- so on a document two
// people are writing at once it names whoever typed last, while the checkpoint
// holds both their work. `document_activity.peer` is written as the empty
// string on every path. Splitting on either would cut a collaborative
// afternoon into runs that reflect typing order and credit each of them to one
// person.
//
// None of those is a time bucket, and the depth stops here on purpose. A tree
// of finer and finer intervals answers "when" more precisely at every level,
// and precision-of-when is not the question anybody opens a history with --
// they are looking for the version before they cut the introduction, or the
// one they sent to a coauthor. Below a version there is nothing left to
// divide anyway: the finest thing that can be opened is a minute of the
// operation history, which is the one folded line at the end.
//
// Both are pure, because the arithmetic -- which week a day falls in, which
// minute a timestamp is at once the reader's own timezone is applied -- is
// the part worth checking without a browser.

import { level, minutesIn, withWork } from "./activity.js";
import { checkpointOrder } from "./history.js";
import { day as isoDay } from "./dates.js";

/// The month a day belongs to, which is what the grid is keyed by.
export const monthOf = (day) => String(day || "").slice(0, 7);

/// A day string moved by whole months, staying a day string, and staying
/// inside the month it lands in: a January 31st moved forward one month is
/// the end of February, not the third of March.
export function shiftMonth(month, months) {
  const match = /^(\d{4})-(\d{2})/.exec(month || "");
  if (!match) return month;
  const at = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1 + months, 1, 12));
  return Number.isNaN(at.getTime()) ? month : at.toISOString().slice(0, 7);
}

/// A day string moved by whole days. Done at noon UTC so a daylight-saving
/// boundary cannot land it on the day before.
export function shiftDay(day, days) {
  const at = new Date(`${day}T12:00:00Z`);
  if (Number.isNaN(at.getTime())) return day;
  at.setUTCDate(at.getUTCDate() + days);
  return at.toISOString().slice(0, 10);
}

/// One entry per day the document has anything on: how many versions landed
/// on it, whether any of them was given a name, the versions themselves
/// oldest first, and whether anybody wrote on it at all.
///
/// The shading is the version count, because that is what a reader is looking
/// for when they open a history. `worked` is the other thing a day can be --
/// a day somebody wrote on and nothing was saved from -- and it is kept apart
/// rather than added in, so a shaded cell always means versions.
///
/// A day is the reader's own day, because a history is read as "Tuesday" and
/// Tuesday is where the reader is.
export function versionDays(checkpoints, rows, timeZone) {
  const byDay = new Map();
  const at = (day) => {
    const found = byDay.get(day) || { day, count: 0, named: false, worked: false, changes: 0, points: [] };
    byDay.set(day, found);
    return found;
  };
  for (const point of [...(checkpoints || [])].sort(checkpointOrder)) {
    const day = isoDay(point.at, timeZone);
    if (!day) continue;
    const found = at(day);
    found.count += 1;
    found.named = found.named || Boolean(point.label);
    found.points.push(point);
  }
  for (const row of withWork(rows)) {
    const day = isoDay(row.at, timeZone);
    if (!day) continue;
    const found = at(day);
    found.worked = true;
    found.changes += row.changes;
  }
  return byDay;
}

/// A month as whole Sunday-to-Saturday weeks.
///
/// The days either side of the month are drawn in place rather than left
/// blank: a reader counts across a calendar, and a grid with holes in its
/// corners is a grid they have to count around. They carry their own counts
/// and are `inMonth: false`, so the panel can dim them and still open them.
///
/// The shading scale is the month's own busiest day, not the document's, so
/// a quiet month is legible instead of uniformly pale.
export function monthGrid(month, byDay, { today = "" } = {}) {
  const match = /^(\d{4})-(\d{2})$/.exec(month || "");
  if (!match) return { weeks: [], most: 0 };
  const year = Number(match[1]);
  const index = Number(match[2]) - 1;
  const first = `${month}-01`;
  const length = new Date(Date.UTC(year, index + 1, 0, 12)).getUTCDate();
  const last = `${month}-${String(length).padStart(2, "0")}`;
  const most = Math.max(0, ...[...byDay.values()]
    .filter((entry) => monthOf(entry.day) === month)
    .map((entry) => entry.count));
  const cells = [];
  const start = shiftDay(first, -new Date(`${first}T12:00:00Z`).getUTCDay());
  for (let at = start; at <= last || cells.length % 7; at = shiftDay(at, 1)) {
    const found = byDay.get(at);
    cells.push({
      day: at,
      date: Number(at.slice(8, 10)),
      inMonth: monthOf(at) === month,
      count: found?.count || 0,
      named: Boolean(found?.named),
      worked: Boolean(found?.worked),
      changes: found?.changes || 0,
      level: level(found?.count || 0, most),
      today: at === today,
    });
  }
  const weeks = [];
  for (let at = 0; at < cells.length; at += 7) weeks.push(cells.slice(at, at + 7));
  return { weeks, most };
}

/// The months the panel can walk between: the first one anybody worked in
/// and the one today is in, whichever way round the two sets run.
export function monthSpan(byDay, today) {
  const days = [...byDay.keys()].sort();
  const here = monthOf(today);
  const first = days.length ? monthOf(days[0]) : here;
  const last = days.length ? monthOf(days[days.length - 1]) : here;
  return { first: first < here ? first : here, last: last > here ? last : here };
}

/// The versions saved on one day, oldest first, each placed on the day's own
/// axis: `minute` is minutes from midnight, in the reader's own timezone.
export function dayVersions(checkpoints, day, timeZone) {
  return [...(checkpoints || [])]
    .sort(checkpointOrder)
    .filter((point) => isoDay(point.at, timeZone) === day)
    .map((point) => ({ point, minute: Math.max(0, minutesIn(point.at, timeZone)) }));
}

/// The reasons a version was written by a machine rather than asked for by a
/// person. They are worth keeping and worth reaching, and they are never
/// worth a line each in a list somebody is scanning.
///
/// Only `sync` is still written -- a command-line sync asks for a version on
/// every save of a file. The other three are rows the clock wrote before
/// versions stopped being scheduled. Nothing produces them now, but documents
/// hold thousands of them, and a panel that stopped folding them would open
/// on a wall of them.
const QUIET = new Set(["sync", "quiet", "left", "automatic"]);

/// Whether a version is one somebody asked for. A name is enough on its own:
/// giving a checkpoint a name is the most deliberate thing anybody does to
/// one, whatever the document's own reason for taking it was.
export const deliberate = (point) => Boolean(point?.label) || !QUIET.has(point?.why);

/// How many machine saves in a row are worth gathering. Two collapse into an
/// entry that opens into two, which is the same height and one more click.
const WORTH_GATHERING = 3;

/// How long a pause between two machine saves has to be before it reads as a
/// break in the work rather than as thinking.
///
/// Ten minutes without saving a file is somebody who stopped, not somebody
/// mid-paragraph.
const RUN_PAUSE_MINUTES = 10;

/// How many versions one run may stand for.
///
/// The ceiling exists because the arithmetic says it has to. Between two
/// deliberate versions, machine saves are at most ten minutes apart -- or the
/// pause above would have ended the run -- and somebody who saves on every
/// keystroke pause is several hundred in an afternoon. Opening one entry onto
/// three hundred near-identical times is not a way to find anything.
///
/// Twenty-five is about a screenful in a sidebar. It is a ceiling on what one
/// entry may hide, not a bucket size: a run under it is never divided.
const MOST_IN_A_RUN = 25;

/// One day's versions, coarsened.
///
/// Returns entries in order, each either a single version or a run of
/// consecutive autosaves. A run knows what the whole of it changed -- the
/// union of the paths its versions touched -- because "when" alone does not
/// answer the question somebody opens a history with, which is nearly always
/// "where did I write that". A run whose versions never recorded what they
/// changed says nothing rather than guessing.
///
/// Newest first. A version history is read from the end: what somebody is
/// looking for is nearly always the last thing they did, and a list that puts
/// it at the bottom makes every reader scroll past a day of autosaves to reach
/// the one row they came for. The runs are built in clock order -- a run is a
/// stretch of time and can only be found forwards -- and the whole is turned
/// over once, at the end, together with the versions inside each run. A run
/// still says its own span forwards ("3:14 - 4:02"): the order of the list is
/// a reading convenience, while the span is a fact about the work.
///
/// `gathered` sets how long a run has to be before it is gathered at all,
/// `pause` how long a silence has to be to end one, and `most` how many
/// versions one may stand for. All three are the caller's: they are claims
/// about how people work, not facts about the data.
export function dayEntries(
  checkpoints,
  day,
  timeZone,
  { gathered = WORTH_GATHERING, pause = RUN_PAUSE_MINUTES, most = MOST_IN_A_RUN } = {},
) {
  const versions = dayVersions(checkpoints, day, timeZone);
  // A day that fits is a day that is listed. Coarsening exists to make a day
  // of three hundred versions readable; applied to a day of three it gathers
  // them into one entry that opens into three, which saves nobody a thing and
  // costs them the one thing the panel is for -- a version is chosen by
  // clicking it, and an entry standing for several cannot be chosen at all.
  if (versions.length <= Math.max(1, most)) {
    return newestFirst(versions.map((one) => entry([one], "version")));
  }
  const runs = [];
  for (const one of versions) {
    const last = runs[runs.length - 1];
    // What ends a run, and both of them are something a reader would
    // recognise as a boundary rather than an interval the panel chose. Who
    // was typing is not among them: see the note at the top of this file.
    const continues = !deliberate(one.point)
      && last
      && !last.asked
      && one.minute - last.items[last.items.length - 1].minute <= Math.max(0, pause);
    if (continues) last.items.push(one);
    else runs.push({ asked: deliberate(one.point), items: [one] });
  }

  return newestFirst(runs
    .flatMap((run) => (run.asked ? [run.items] : divide(run.items, Math.max(1, most))))
    .flatMap((items) => {
      // A run too short to be worth gathering is just its versions.
      if (items.length > 1 && items.length < Math.max(2, gathered)) {
        return items.map((one) => entry([one], "version"));
      }
      return [entry(items, items.length > 1 ? "run" : "version")];
    }));
}

// The one place the clock order is turned over, so that everything above it
// can be written forwards. An entry keeps its own `minute` and `to`: those
// are the span it ran for, not its place in the list.
function newestFirst(entries) {
  return entries
    .map((one) => (one.points.length > 1 ? { ...one, points: [...one.points].reverse() } : one))
    .reverse();
}

// One entry, in the shape the panel reads: what it stands for, when it ran,
// and -- for a run -- what the whole of it changed.
function entry(items, kind) {
  const points = items.map((one) => one.point);
  return {
    kind,
    key: points[0].sha,
    minute: items[0].minute,
    to: items[items.length - 1].minute,
    points,
    changed: kind === "run" ? changedAcross(points) : null,
  };
}

/// Cuts an over-long run at the longest pause inside it, and keeps cutting
/// until no piece stands for more than `most` versions.
///
/// The place to divide a run of real work is where the work paused longest,
/// not at every twenty-fifth version: a cut at a count falls in the middle of
/// whatever somebody was doing and means nothing to them, while a cut at the
/// longest pause is the same kind of boundary as the ones above -- the reader
/// stopped there. Recursing on both halves finds the next-longest pause in
/// each, so the pieces follow the shape of the afternoon rather than its
/// length.
///
/// Equal pauses are cut nearest the middle, which matters more than it looks:
/// a run with no variation in it at all -- somebody typing steadily for three
/// hours -- has every gap tied, and taking the first of them would shave one
/// version off the front three hundred times over and leave three hundred
/// rows. Cutting in the middle halves it instead.
function divide(items, most) {
  if (items.length <= most) return [items];
  const middle = items.length / 2;
  let at = 1;
  let longest = -1;
  let nearest = Infinity;
  for (let index = 1; index < items.length; index += 1) {
    const gap = items[index].minute - items[index - 1].minute;
    const away = Math.abs(index - middle);
    if (gap > longest || (gap === longest && away < nearest)) {
      longest = gap;
      nearest = away;
      at = index;
    }
  }
  return [...divide(items.slice(0, at), most), ...divide(items.slice(at), most)];
}

// What a run of versions changed, all told. `null` when any of them could not
// answer: a run that cannot account for itself should not claim to, and one
// version's silence makes the union a guess rather than a total.
function changedAcross(points) {
  const paths = new Set();
  for (const point of points) {
    if (!Array.isArray(point.changed)) return null;
    for (const path of point.changed) paths.add(path);
  }
  return [...paths].sort();
}

