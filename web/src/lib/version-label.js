// Short, human names for the versions a selector lists, and for the two ends
// of a comparison.
//
// A version is written by when it was taken: the clock time today, "Yesterday"
// for the day before, the day and month this year, and the year as well for
// anything older. Two versions that would read the same get their seconds, so
// that every name in a list is distinct. A named version leads with its name.
import { m } from "./paraglide/messages.js";
import { locale } from "./locale.js";
import { day } from "./dates.js";
import { CURRENT, NOTHING } from "./history-endpoints.js";
import { isMoment } from "./moment.js";

/// The calendar day before `today` (a YYYY-MM-DD string), computed on the day
/// itself so that daylight saving changes cannot move it.
function dayBefore(today) {
  const [year, month, date] = today.split("-").map(Number);
  return new Date(Date.UTC(year, month - 1, date - 1)).toISOString().slice(0, 10);
}

/// One point written with or without its seconds, in `timeZone`.
function render(point, { today, yesterday, timeZone, seconds }) {
  const when = new Date(point.at);
  let text = "";
  if (!Number.isNaN(when.getTime())) {
    /** @type {Intl.DateTimeFormatOptions} */
    const clock = { hour: "numeric", minute: "2-digit", ...(seconds ? { second: "2-digit" } : {}), timeZone };
    const dayOf = day(when, timeZone);
    if (dayOf === today) {
      text = m.common_today_at({ time: when.toLocaleTimeString(locale(), clock) });
    } else if (dayOf === yesterday) {
      text = m.common_yesterday_at({ time: when.toLocaleTimeString(locale(), clock) });
    } else {
      const sameYear = dayOf.slice(0, 4) === today.slice(0, 4);
      text = when.toLocaleString(locale(), /** @type {Intl.DateTimeFormatOptions} */ ({
        month: "short",
        day: "numeric",
        ...(sameYear ? {} : { year: "numeric" }),
        ...clock,
      }));
    }
  }
  if (!point.label) return text;
  return text ? m.common_named_version({ name: point.label, when: text }) : point.label;
}

/// A `Map` from a version's sha to its name, for the points given (`{ sha, at,
/// label }`). The clock is read once, so every name in one call is written
/// against the same day.
/** @param {Array<{sha: string, at?: string, label?: string}>} points
 *  @param {{timeZone?: string, now?: Date}} [options]
 *  @returns {Map<string, string>} */
export function versionLabels(points, { timeZone, now = new Date() } = {}) {
  const today = day(now, timeZone);
  const yesterday = dayBefore(today);
  const base = { today, yesterday, timeZone };
  const plain = new Map(points.map(point => [point, render(point, { ...base, seconds: false })]));

  // Points that would read the same get their seconds, all of them together.
  const counts = new Map();
  for (const text of plain.values()) counts.set(text, (counts.get(text) || 0) + 1);
  const names = new Map();
  for (const [point, text] of plain) {
    const name = counts.get(text) > 1 ? render(point, { ...base, seconds: true }) : text;
    if (name && point.sha) names.set(point.sha, name);
  }
  return names;
}

/// What an endpoint of a comparison is called: the sentinels by their words, a
/// moment by the earlier-moment wording, and a saved version by its name in
/// `map`, falling back to the first seven characters of its sha.
/** @param {string} id @param {Map<string, string>} map @returns {string} */
export function endpointLabel(id, map) {
  if (id === CURRENT) return m.history_latest_version();
  if (id === NOTHING) return m.history_before_first();
  if (isMoment(id)) return m.history_source_earlier_moment();
  return map.get(id) || String(id).slice(0, 7);
}
