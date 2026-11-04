// The version check between this page and the desktop app.
//
// Pure: it reads a shell global and returns an answer, with no request, no
// message and no import from client.js. `client.js` decides what to do with
// the answer. See SPEC-wire.md 5.2.

/** The version this page speaks. */
export const PAGE = "3.1.0";

/** What an app released before `protocol` existed is taken to speak. */
export const LEGACY_RANGE = Object.freeze({ oldest: 3, current: "3.0.0" });

/** @param {string} version @returns {number} */
export function major(version) {
  return Number(String(version).split(".")[0]);
}

/** Numeric comparison of MAJOR.MINOR.PATCH. @returns {-1 | 0 | 1} */
export function compareVersions(a, b) {
  const left = String(a).split(".").map(Number);
  const right = String(b).split(".").map(Number);
  for (let i = 0; i < 3; i += 1) {
    const x = left[i] ?? 0;
    const y = right[i] ?? 0;
    if (x !== y) return x < y ? -1 : 1;
  }
  return 0;
}

/** @param {{ protocol?: { oldest: number, current: string } } | null | undefined} shell */
export function rangeOf(shell) {
  return shell?.protocol ?? LEGACY_RANGE;
}

/**
 * @param {{ oldest: number, current: string }} range
 * @param {string} [page]
 * @returns {"ok" | "app_outdated" | "server_outdated"}
 */
export function outcome(range, page = PAGE) {
  if (major(page) > major(range.current)) return "app_outdated";
  if (major(page) < range.oldest) return "server_outdated";
  return "ok";
}

/**
 * Whether the app meets a minimum version. Only a same-major app is compared
 * by version; a newer major's migrations cover every addition of this one.
 * @param {{ oldest: number, current: string }} range
 * @param {string} since
 * @param {string} [page]
 */
export function sinceMet(range, since, page = PAGE) {
  if (major(page) < major(range.current)) return true;
  return compareVersions(range.current, since) >= 0;
}

/** True when the app predates the `protocol` field. */
export function isLegacy(shell) {
  return !shell?.protocol;
}
