// The version panel's grouping: which versions make one event, and how the
// events fall into the periods the panel lists.
//
// This is pure arithmetic over the points the reader holds. It has no DOM and
// no Svelte, so every rule can be checked in plain node. Every threshold is
// read from HISTORY_DEFAULTS merged with what the caller passes, and the same
// points always group the same way, whatever order they arrive in.

import { labelOrder } from "./history.js";
import { day as dayOf } from "./dates.js";

/// The thresholds the grouping reads. A caller overrides any of them by
/// passing the same name in `options`.
export const HISTORY_DEFAULTS = {
  sessionGapMinutes: 30, // inactivity that ends an editing session
  gitGapMinutes: 5, // consecutive git points within this gap form one Git event
  recentDays: 7, // today and the six days before it each get a day group
  flattenEvents: 4, // a month/rest/year group holding at most this many events lists them directly
  openYesterdayEvents: 6, // "yesterday" starts open when it holds at most this many events
};

const MINUTE = 60_000;

// Machine writers show up as "system" or as a UUID. Neither is a person, so
// neither can make a session collaborative.
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/// Shifts a YYYY-MM-DD day by a number of calendar days. The arithmetic runs
/// at noon UTC, so no offset can move the answer onto the wrong day.
/** @param {string} day @param {number} days @returns {string} */
export function shiftDay(day, days) {
  const at = new Date(`${day}T12:00:00Z`);
  if (Number.isNaN(at.getTime())) return day;
  at.setUTCDate(at.getUTCDate() + days);
  return at.toISOString().slice(0, 10);
}

/// What a point is for, as the panel groups it. A saved point is always its
/// own event, whatever else it is.
/** @param {object} point @returns {"saved" | "restore" | "git" | "created" | "edit"} */
export function eventKind(point) {
  if (point?.label) return "saved";
  if (point?.why === "restore" || point?.why === "superseded") return "restore";
  if (point?.why === "git") return "git";
  if (point?.why === "created") return "created";
  return "edit";
}

/// Events, newest first. An event never spans two days, where a day is read
/// in `timeZone`. Its key is the sha of its oldest point, which does not
/// change when newer points join the same event.
/** @param {object[]} points @param {string} [timeZone] @param {object} [options] @returns {object[]} */
export function buildEvents(points, timeZone, options = {}) {
  const o = { ...HISTORY_DEFAULTS, ...options };
  const items = [];
  for (const point of [...(points || [])].sort(labelOrder)) {
    const when = dayOf(point.at, timeZone);
    if (!when) continue;
    items.push({ point, day: when, kind: eventKind(point), ms: Date.parse(point.at) });
  }

  const groups = [];
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    // A superseded point and the restore that replaced it are one act, so
    // they share an event when they fall on the same day.
    if (item.kind === "restore" && item.point.why === "superseded") {
      const next = items[index + 1];
      if (next && next.day === item.day && next.kind === "restore" && next.point.why === "restore") {
        groups.push({ kind: "restore", day: item.day, items: [item, next] });
        index += 1;
        continue;
      }
    }
    const tail = groups[groups.length - 1];
    if (tail && joins(tail, item, o)) tail.items.push(item);
    else groups.push({ kind: item.kind, day: item.day, items: [item] });
  }

  return groups.map(finish).reverse();
}

// Only the group the previous point belongs to can take this point, so any
// other point in between ends the run. Saved, created and restore points never
// join anything.
function joins(tail, item, o) {
  if (tail.day !== item.day) return false;
  const last = tail.items[tail.items.length - 1];
  const gap = item.ms - last.ms;
  if (item.kind === "git") return tail.kind === "git" && gap <= o.gitGapMinutes * MINUTE;
  if (item.kind === "edit") return tail.kind === "edit" && gap <= o.sessionGapMinutes * MINUTE;
  return false;
}

function finish(group) {
  const chronological = group.items.map((one) => one.point);
  const authors = [...new Set(chronological.map((point) => point.by).filter(isAuthor))].sort();
  const type = group.kind === "edit"
    ? (authors.length >= 2 ? "collaborative" : "editing")
    : group.kind;
  return {
    key: chronological[0].sha,
    type,
    points: chronological.slice().reverse(),
    start: chronological[0].at,
    end: chronological[chronological.length - 1].at,
    day: group.day,
    authors,
  };
}

function isAuthor(by) {
  return typeof by === "string" && by !== "" && by !== "system" && !UUID.test(by);
}

/// Period groups, newest first. Days inside the recent window, and any later
/// day, are "day" periods. Other days in today's month form one "rest"
/// period, other days in today's year form "month" periods, and older days
/// form "year" periods. A period that holds few enough events lists them
/// directly. The newest top-level period is open, as is "yesterday" when it is
/// small enough. When `partial` is set, the oldest chain is marked partial
/// because older history is still unloaded.
/** @param {object[]} points @param {{timeZone?: string, today: string, partial?: boolean, options?: object}} config @returns {object[]} */
export function buildPeriods(points, { timeZone, today, partial = false, options = {} }) {
  const o = { ...HISTORY_DEFAULTS, ...options };
  const events = buildEvents(points, timeZone, o);
  const windowStart = shiftDay(today, -(o.recentDays - 1));
  const thisMonth = today.slice(0, 7);
  const thisYear = today.slice(0, 4);

  // Events arrive newest first and the buckets are monotone in time, so the
  // first event of each bucket gives the order of the top level.
  const groups = new Map();
  for (const event of events) {
    const [kind, value] = bucketOf(event.day, windowStart, thisMonth, thisYear);
    const key = `${kind}:${value}`;
    if (!groups.has(key)) groups.set(key, { kind, value, events: [] });
    groups.get(key).events.push(event);
  }
  const periods = [...groups.values()].map((group) => makePeriod(group.kind, group.value, group.events, o));

  if (periods.length > 0) periods[0].open = true;
  const yesterday = shiftDay(today, -1);
  for (const period of periods) {
    if (period.kind === "day" && period.value === yesterday && period.events.length <= o.openYesterdayEvents) {
      period.open = true;
    }
  }

  if (partial && periods.length > 0) {
    let node = periods[periods.length - 1];
    while (node) {
      node.partial = true;
      node = node.children ? node.children[node.children.length - 1] : null;
    }
  }
  return periods;
}

function bucketOf(day, windowStart, thisMonth, thisYear) {
  if (day >= windowStart) return ["day", day];
  if (day.slice(0, 7) === thisMonth) return ["rest", thisMonth];
  if (day.slice(0, 4) === thisYear) return ["month", day.slice(0, 7)];
  return ["year", day.slice(0, 4)];
}

function makePeriod(kind, value, events, o) {
  const period = {
    key: `${kind}:${value}`,
    kind,
    value,
    count: events.reduce((sum, event) => sum + event.points.length, 0),
    named: events.some((event) => event.points.some((point) => Boolean(point.label))),
    events: null,
    children: null,
    open: false,
    partial: false,
    newest: events[0].end,
    oldest: events[events.length - 1].start,
  };
  // A day always lists its events: it is the level the reader works at.
  if (kind === "day" || events.length <= o.flattenEvents) {
    period.events = events;
    return period;
  }
  if (kind === "year") {
    period.children = groupBy(events, (event) => event.day.slice(0, 7))
      .map(([month, list]) => makePeriod("month", month, list, o));
  } else {
    period.children = groupBy(events, (event) => event.day)
      .map(([day, list]) => makePeriod("day", day, list, o));
  }
  return period;
}

// Keeps first-appearance order, so a list built from newest-first events
// stays newest first.
function groupBy(list, keyOf) {
  const map = new Map();
  for (const item of list) {
    const key = keyOf(item);
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(item);
  }
  return [...map];
}

/// Keys of every period and event that contains `sha`, outermost first, ending
/// with the event key. An empty list when no event holds the sha.
/** @param {object[]} periods @param {string} sha @returns {string[]} */
export function pathTo(periods, sha) {
  for (const period of periods || []) {
    if (period.events) {
      const event = period.events.find((one) => one.points.some((point) => point.sha === sha));
      if (event) return [period.key, event.key];
    } else if (period.children) {
      const inner = pathTo(period.children, sha);
      if (inner.length > 0) return [period.key, ...inner];
    }
  }
  return [];
}

/// The period, at any depth, that best answers "jump to this month". `month`
/// is "YYYY-MM". An exact answer is the deepest period for that month, or a
/// year period for its year when no month-level period exists. Otherwise the
/// answer is the period whose range is closest to the middle of the month,
/// with `exact: false`. Returns null when there are no periods.
/** @param {object[]} periods @param {string} month @returns {{key: string, exact: boolean} | null} */
export function nearestPeriod(periods, month) {
  if (!periods || periods.length === 0) return null;
  const all = [];
  const walk = (list, depth) => {
    for (const period of list) {
      all.push({ period, depth });
      if (period.children) walk(period.children, depth + 1);
    }
  };
  walk(periods, 0);

  // Deeper beats shallower. Between equals the walk order wins, which is
  // newest first.
  let best = null;
  for (const entry of all) {
    const { period } = entry;
    const inside = ((period.kind === "month" || period.kind === "rest") && period.value === month)
      || (period.kind === "day" && period.value.startsWith(`${month}-`));
    if (inside && (!best || entry.depth > best.depth)) best = entry;
  }
  if (best) return { key: best.period.key, exact: true };

  const year = month.slice(0, 4);
  // A year that lists its events directly answers for a month only when one
  // of those events falls in it.
  const yearPeriod = all.find(({ period }) => period.kind === "year" && period.value === year
    && period.events?.some((event) => event.day.startsWith(`${month}-`)));
  if (yearPeriod) return { key: yearPeriod.period.key, exact: true };

  // Distance is measured from the middle of the month to the nearest edge of
  // each period's range, and zero when the middle falls inside it. Ties go to
  // the shallower period, so a top-level group is preferred.
  const start = Date.parse(`${month}-01T00:00:00Z`);
  const next = new Date(start);
  next.setUTCMonth(next.getUTCMonth() + 1);
  const middle = (start + next.getTime()) / 2;
  let nearest = null;
  for (const entry of all) {
    const low = Date.parse(entry.period.oldest);
    const high = Date.parse(entry.period.newest);
    const away = middle < low ? low - middle : middle > high ? middle - high : 0;
    if (Number.isNaN(away)) continue;
    if (!nearest || away < nearest.away || (away === nearest.away && entry.depth < nearest.depth)) {
      nearest = { ...entry, away };
    }
  }
  return nearest ? { key: nearest.period.key, exact: false } : null;
}

/// Short heading for a period: "Today", "Yesterday", "Wed, Oct 21" (with the
/// year when it is not this year), "Earlier in October", "September" (with the
/// year when it is a past year), or the year itself.
/** @param {object} period @param {string} today @returns {string} */
export function periodTitle(period, today) {
  const { kind, value } = period;
  if (kind === "year") return value;
  if (kind === "day") {
    if (value === today) return "Today";
    if (value === shiftDay(today, -1)) return "Yesterday";
    /** @type {Intl.DateTimeFormatOptions} */
    const options = { weekday: "short", month: "short", day: "numeric", timeZone: "UTC" };
    if (value.slice(0, 4) !== today.slice(0, 4)) options.year = "numeric";
    return new Date(`${value}T12:00:00Z`).toLocaleDateString([], options);
  }
  const name = new Date(`${value}-01T12:00:00Z`).toLocaleDateString([], { month: "long", timeZone: "UTC" });
  if (kind === "rest") return `Earlier in ${name}`;
  return value.slice(0, 4) === today.slice(0, 4) ? name : `${name} ${value.slice(0, 4)}`;
}
