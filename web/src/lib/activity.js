// When the document was worked on, and how to go and look.
//
// The server answers `/activity` with one row per minute in which a write
// landed: how many persistence cycles it held, how large the document was by
// the end of it, and the frontier that reproduces it. `/at` turns one of those
// frontiers back into a document. Between them a reader can reach a moment
// nobody checkpointed, which is the point of keeping the whole operation
// history rather than thinning it.
//
// Everything below the two fetches is pure, because that is the part with any
// judgement in it: what the work in a minute actually was, and which of five
// shades a value falls in, are both read off the same rows and are worth
// checking without a browser. The calendar that draws them is next door, in
// `history-calendar.js`, because it draws versions beside these.

import { SHELL_HEADERS } from "./api.js";
import { day as isoDay } from "./dates.js";

const asked = (headers) => ({ ...SHELL_HEADERS, ...headers });

/// Every bucket the document has, oldest first. `since` is an ISO instant
/// that bounds the scan for a caller drawing a window rather than a life.
export async function load(slug, headers = {}, { since = "" } = {}) {
  const query = since ? `?since=${encodeURIComponent(since)}` : "";
  const response = await fetch(`/api/documents/${slug}/activity${query}`, {
    headers: asked(headers),
    cache: "no-store",
  });
  if (!response.ok) throw new Error("this document's activity is not readable");
  const payload = await response.json();
  return Array.isArray(payload.activity) ? payload.activity : [];
}

/// The document as it stood at one frontier: `{ main, texts, assets }`.
export async function documentAt(slug, frontier, headers = {}) {
  const response = await fetch(
    `/api/documents/${slug}/at?frontier=${encodeURIComponent(frontier)}`,
    { headers: asked(headers), cache: "no-store" },
  );
  if (!response.ok) throw new Error("the document at that moment could not be read");
  return response.json();
}

/// What each row *added*.
///
/// A persisted row carries the size of the document's whole history, so a
/// row's own size measures the paper, not the work: the work is the growth
/// since the row before it. A row that shrinks -- a paragraph deleted -- is no
/// work rather than negative work, because a cell cannot be less than empty,
/// and its `changes` still says somebody was there.
///
/// The first row of all has nothing before it, so its growth is the document
/// arriving: that is the upload, and it is real work somebody did elsewhere.
export function withWork(rows = []) {
  let previous = null;
  return [...rows]
    .sort((left, right) => String(left.at).localeCompare(String(right.at)))
    .map((row) => {
      const bytes = Number(row.state_bytes) || 0;
      const work = previous === null ? bytes : Math.max(0, bytes - previous);
      previous = bytes;
      return { ...row, work, changes: Number(row.changes) || 0 };
    });
}

/// Which of five shades a value falls in, against the busiest one on screen.
///
/// Five steps because a reader can count five and read the legend back off
/// the page. Zero is reserved for nothing at all: a day with one keystroke is
/// a day somebody worked, and it must not look like a day nobody opened.
export function level(value, most) {
  if (!(value > 0)) return 0;
  if (!(most > 0)) return 1;
  return Math.min(4, Math.max(1, Math.ceil((value / most) * 4)));
}

/// The rows a day holds, in the reader's own timezone, oldest first.
export function momentsOf(rows, day, timeZone) {
  return withWork(rows).filter((row) => isoDay(row.at, timeZone) === day);
}

/// The minute of the day `at` falls in, in the reader's timezone, counted
/// from midnight, or -1 when it cannot be read.
///
/// Minutes from midnight because that is the coordinate the day timeline is
/// drawn in: one vertical axis, and everything placed on it -- an hour label,
/// a ten-minute bar of activity, a version -- is placed by this one number.
export function minutesIn(at, timeZone) {
  const when = new Date(at);
  if (Number.isNaN(when.getTime())) return -1;
  if (!timeZone) return when.getHours() * 60 + when.getMinutes();
  try {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone,
      hour: "2-digit",
      minute: "2-digit",
      hourCycle: "h23",
    }).formatToParts(when);
    const read = (type) => Number(parts.find((part) => part.type === type)?.value ?? NaN);
    const hour = read("hour");
    const minute = read("minute");
    return Number.isNaN(hour) || Number.isNaN(minute) ? -1 : hour * 60 + minute;
  } catch {
    return when.getUTCHours() * 60 + when.getUTCMinutes();
  }
}
