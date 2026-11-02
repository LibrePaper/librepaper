// The two ends of a version comparison, and where each one sits in time.
//
// A comparison has an older endpoint and a newer one. An endpoint is a label's
// sha (a version somebody saved), a moment (a `frontier:` position in the
// editing history), CURRENT (the live project, always the newest), or NOTHING
// (before the first version: an empty project). `ordered` puts a pair in time
// order, so the caller never has to know which way round it was asked.
import { isMoment } from "./moment.js";
import { labelOrder } from "./history.js";

/// The live project as it stands. Always the newest endpoint.
export const CURRENT = "current";
/// Before the first version: an empty project.
export const NOTHING = "nothing";

/// Where an endpoint sits in time, as a number that only orders. `labels` is
/// the list of saved versions. An id that is none of these answers `null`.
export function rankOf(sha, labels) {
  if (sha === NOTHING) return -Infinity;
  if (sha === CURRENT) return Infinity;
  // A moment is a position in the editing history, so it is newer than every
  // saved version and older than the live project.
  if (isMoment(sha)) return Number.MAX_SAFE_INTEGER;
  const index = [...labels].sort(labelOrder).findIndex(point => point.sha === sha);
  return index === -1 ? null : index;
}

/// The pair as `[older, newer]`. Left as given when either side has no rank,
/// so an unknown id is never silently moved.
export function ordered(a, b, labels) {
  const left = rankOf(a, labels);
  const right = rankOf(b, labels);
  if (left === null || right === null) return [a, b];
  return left > right ? [b, a] : [a, b];
}
