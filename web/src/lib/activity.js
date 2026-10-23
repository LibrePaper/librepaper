// Where a version falls in its day.
//
// The day view places each version on a vertical axis of time, so the question
// is which minute of the day a timestamp is at, in the reader's own timezone.
// That is what `minutesIn` answers.

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
