// Rows for a chat transcript: each message with the author, the local time,
// the day separator it opens (if any) and whether it starts a new group.
const MINUTE = 60 * 1000;

function sameDay(a, b) {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

// "Today", "Yesterday", or a month and day; the year is added when it is not this one.
export function dayLabel(at, now = new Date()) {
  if (sameDay(at, now)) return "Today";
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (sameDay(at, yesterday)) return "Yesterday";
  const options = { month: "long", day: "numeric" };
  if (at.getFullYear() !== now.getFullYear()) options.year = "numeric";
  return at.toLocaleDateString(undefined, options);
}

function parseTime(created) {
  if (!created) return null;
  const at = new Date(created);
  return Number.isNaN(at.getTime()) ? null : at;
}

export function chatRows(messages, { authorKey, roleLabel, now = new Date(), groupMs = 5 * MINUTE }) {
  let latest = null; // the most recent message that carries a time
  return messages.map((message, index) => {
    const previous = index > 0 ? messages[index - 1] : null;
    const key = authorKey(message);
    const at = parseTime(message.created);
    const previousAt = previous ? parseTime(previous.created) : null;
    const date = at && (!latest || !sameDay(latest, at)) ? dayLabel(at, now) : null;
    const starts = index === 0 || authorKey(previous) !== key || date !== null ||
      Boolean(at && previousAt && at.getTime() - previousAt.getTime() > groupMs);
    if (at) latest = at;
    return {
      message,
      key,
      name: roleLabel(message),
      starts,
      date,
      time: at ? at.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : "",
      stamp: at ? at.toLocaleString() : "",
    };
  });
}
