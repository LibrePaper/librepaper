// A month of the document's life, the sittings one of its days was written
// in, and the axis one of those sittings is drawn on.
//
// The version panel narrows three times, and this answers each step in turn.
// *Which day?* -- a month laid out as a grid, each cell knowing how many
// versions landed on it and whether anybody wrote on it at all. *Which
// sitting on that day?* -- the day cut where the work stopped, so the panel
// spends its height on the twenty minutes somebody wrote rather than on the
// five hours they did not. And, for a sitting busy enough to need it, *when
// exactly, and which version?* -- that one interval on a real axis of
// minutes, which is the only place a proportional time scale earns its space.
//
// Both are pure, because the arithmetic -- which week a day falls in, which
// minute a timestamp is at once the reader's own timezone is applied -- is
// the part worth checking without a browser.

import { level, minutesIn, momentsOf, withWork } from "./activity.js";
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

/// How long a pause has to be before it is a different sitting.
///
/// Twenty-five minutes: long enough that stopping to read something is still
/// the same sitting, short enough that lunch is not. It is a fact about how
/// people work rather than about how the panel draws, so it is a number here
/// and an argument below, not something baked into a component.
export const SESSION_GAP_MINUTES = 25;

/// The day's work as the sittings somebody actually had.
///
/// A day is not twenty-four hours of anything. Somebody writes for twenty
/// minutes after breakfast and an hour after lunch, and a view that gives the
/// five empty hours between them five hours of height has spent most of
/// itself on nothing. So the day is cut where the work stopped: every run of
/// writing and saving with no gap longer than `gap` in it is one session, and
/// the panel lists sessions rather than hours.
///
/// Versions are events in their own right here, not just the writes around
/// them: a publish from the command line leaves a version and no writes at
/// all, and it is still something that happened at half past one.
///
/// Returns `[{ from, to, span, since, writes, work, moments, versions }]`,
/// oldest first, every time in minutes from midnight in the reader's own
/// timezone. `since` is how long the silence before this session was, or
/// `null` for the first of the day -- the feed says that in words instead of
/// spending height on it.
export function daySessions(checkpoints, rows, day, timeZone, { gap = SESSION_GAP_MINUTES } = {}) {
  const moments = momentsOf(rows, day, timeZone)
    .map((row) => ({ ...row, minute: minutesIn(row.at, timeZone) }))
    .filter((one) => one.minute >= 0);
  // One shape for both kinds of event, rather than two: a sitting is cut from
  // the times alone, and the walk below should not have to ask which sort of
  // thing it is holding before it can ask when it was.
  const events = [
    ...moments.map((one) => ({ minute: one.minute, moment: one, version: null })),
    ...dayVersions(checkpoints, day, timeZone)
      .map((one) => ({ minute: one.minute, moment: null, version: one })),
  ].sort((left, right) => left.minute - right.minute);
  const quiet = Math.max(0, gap);
  const out = [];
  for (const event of events) {
    const last = out[out.length - 1];
    if (!last || event.minute - last.to > quiet) {
      out.push({ from: event.minute, to: event.minute, writes: 0, work: 0, moments: [], versions: [] });
    }
    const session = out[out.length - 1];
    session.to = Math.max(session.to, event.minute);
    if (event.moment) {
      session.moments.push(event.moment);
      session.writes += event.moment.changes;
      session.work += event.moment.work;
    }
    if (event.version) session.versions.push(event.version);
  }
  let previous = null;
  return out.map((session) => {
    const since = previous === null ? null : session.from - previous;
    previous = session.to;
    return { ...session, span: session.to - session.from, since };
  });
}

/// A session's shape, as a fixed number of bars.
///
/// Fixed, because this is a sparkline beside a heading rather than an axis:
/// it says "steady", or "one burst at the end", or "stop-start", and it has
/// to say it in the same width whether the session was six minutes or ninety.
/// The detail view is where a bar's place means a time.
export function sessionSparkline(session, bars = 16) {
  const count = Math.max(1, Math.round(bars));
  const width = Math.max(1, (session.to - session.from + 1) / count);
  const out = Array.from({ length: count }, () => 0);
  for (const one of session.moments) {
    const index = Math.min(count - 1, Math.max(0, Math.floor((one.minute - session.from) / width)));
    out[index] += one.changes;
  }
  const most = Math.max(0, ...out);
  return out.map((changes) => ({ changes, share: most > 0 ? changes / most : 0 }));
}

/// One session cut into bins of `span` minutes, in the same coordinates the
/// detail view's axis is drawn in: `at` is minutes from midnight, so a bin
/// and a version at the same time are at the same height by construction.
///
/// The minutes a bin holds are kept, because clicking a bar is how a reader
/// reaches a minute nobody saved a version of.
export function sessionBins(session, span = 1) {
  const width = Math.max(1, Math.round(span));
  const count = Math.max(1, Math.ceil((session.to - session.from + 1) / width));
  const bins = Array.from({ length: count }, (_, index) => ({
    at: session.from + index * width,
    span: width,
    changes: 0,
    work: 0,
    moments: [],
  }));
  for (const one of session.moments) {
    const index = Math.min(count - 1, Math.max(0, Math.floor((one.minute - session.from) / width)));
    bins[index].changes += one.changes;
    bins[index].work += one.work;
    bins[index].moments.push(one);
  }
  // Work is the honest measure -- how much the document grew -- but a
  // document whose activity was recorded before growth was has none, so the
  // strip falls back to counting writes rather than drawing nothing.
  const measure = bins.some((bin) => bin.work > 0) ? "work" : "changes";
  const most = Math.max(0, ...bins.map((bin) => bin[measure]));
  return bins.map((bin) => ({
    ...bin,
    level: level(bin[measure], most),
    share: most > 0 ? bin[measure] / most : 0,
  }));
}
