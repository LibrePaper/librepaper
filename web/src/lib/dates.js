// How a date is written, in the one place that decides it.
//
// Every date a reader sees is a calendar day, and the same day has to look
// the same wherever it is shown: the project listing writes the day a
// document was updated, the history heading writes the day a checkpoint was
// taken, and a comment writes the day its passage went. `toLocaleDateString()`
// on its own gives each of those the browser's own order -- 9/7/2026 in one
// place beside 2026-09-07 in another, which reads as two different days
// rather than one written twice.
//
// The order is the ISO one, unambiguous in every locale. The *day* is the
// reader's own: a history is read as "the day I was working", and that day is
// where the reader is, not where the server is.

/// `at` -- an ISO string, a Date, or a number -- as YYYY-MM-DD in the
/// reader's timezone. An unparseable value is the empty string, which is what
/// every caller here wants to render as nothing.
export function day(at, timeZone) {
  const when = at instanceof Date ? at : new Date(at);
  if (Number.isNaN(when.getTime())) return "";
  if (timeZone) {
    try {
      const parts = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).formatToParts(when);
      const part = (name) => parts.find((value) => value.type === name)?.value;
      return `${part("year")}-${part("month")}-${part("day")}`;
    } catch { return day(at, "UTC"); }
  }
  const pad = (part) => String(part).padStart(2, "0");
  return `${when.getFullYear()}-${pad(when.getMonth() + 1)}-${pad(when.getDate())}`;
}

/// When something was said, as short as it can be written without becoming
/// ambiguous: the clock time for a message posted today, and the calendar day
/// for anything older. A thread is read in one sitting, where every message
/// carrying its full date is noise; the full timestamp stays in the tooltip.
/// Both halves are in the reader's timezone, for the reason `day` gives.
export function moment(at, now = new Date()) {
  const when = at instanceof Date ? at : new Date(at);
  if (Number.isNaN(when.getTime())) return "";
  const today = day(when) === day(now);
  if (!today) return day(when);
  const pad = (part) => String(part).padStart(2, "0");
  return `${pad(when.getHours())}:${pad(when.getMinutes())}`;
}
