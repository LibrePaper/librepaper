// Which of five shades a value falls in, against the busiest one on screen.
//
// This used to be one half of the activity graph: `withWork` turned a
// server-side `/activity` row into a minute of growth, and `level` shaded
// both that and the version calendar. The server-is-a-log cutover
// (SPEC-server-is-a-log.md §8.3) dropped the `document_activity` table and
// the route that read it, so there is no more per-minute work to shade --
// `withWork` and its row shape went with them.
//
// What is left is `level` on its own, still used by the version calendar in
// history-calendar.js to shade a day by how many versions landed on it. It
// stayed general -- a value against a busiest one -- because that is still
// the right shape for a count, not because a second caller is coming back.

/// Five steps because a reader can count five and read the legend back off
/// the page. Zero is reserved for nothing at all: a day with one version is
/// a day something happened, and it must not look like a day nothing did.
export function level(value, most) {
  if (!(value > 0)) return 0;
  if (!(most > 0)) return 1;
  return Math.min(4, Math.max(1, Math.ceil((value / most) * 4)));
}

/// The minute of the day `at` falls in, in the reader's timezone, counted
/// from midnight, or -1 when it cannot be read.
///
/// Minutes from midnight because that is the coordinate the day view places
/// a version on.
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
