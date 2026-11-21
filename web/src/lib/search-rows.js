// How the project search's matches are laid out as rows in the results list.
//
// A line with several matches is one row with several highlights, so the same
// words are not shown again for each match. Rows are built from the matches
// alone, so the list and any test can use them without the editor.
//
// This module is pure. It imports nothing, so the Node unit tests can run it
// without a browser or the message catalogue.

const ELLIPSIS = "…";

/**
 * @typedef {{ from: number, to: number, line: number, column: number, before: string, text: string, after: string }} Match
 * @typedef {{ text: string, mark: boolean }} Segment
 * @typedef {{ line: number, matches: Match[], before: string, segments: Segment[] }} Row
 */

/**
 * Group one file's matches into display rows, in input order.
 * @param {Match[]} matches matches from searchProject for one file, ascending by `from`
 * @returns {Row[]}
 */
export function lineRows(matches) {
  /** @type {Row[]} */
  const rows = [];
  let row = null;
  let prev = null;
  for (const next of matches) {
    if (row && prev && joins(prev, next)) {
      const gap = next.from - prev.to;
      if (gap > 0) row.segments.push({ text: prev.after.slice(0, gap), mark: false });
      row.segments.push({ text: next.text, mark: true });
      row.matches.push(next);
    } else {
      row = {
        line: next.line,
        matches: [next],
        before: next.before,
        segments: [{ text: next.text, mark: true }],
      };
      rows.push(row);
    }
    prev = next;
  }
  // The row ends with its last match's context. Earlier matches' context is
  // shown only up to the next match, as a gap.
  for (const r of rows) {
    const last = r.matches[r.matches.length - 1];
    if (last.after !== "") r.segments.push({ text: last.after, mark: false });
  }
  return rows;
}

// A match stays in the row of the match before it only when that match's
// visible context reaches the next match. A truncated match has no reliable end.
/**
 * @param {Match} prev
 * @param {Match} next
 * @returns {boolean}
 */
function joins(prev, next) {
  if (next.line !== prev.line) return false;
  if (prev.text.endsWith(ELLIPSIS)) return false;
  const gap = next.from - prev.to;
  return withoutEllipsis(prev.after).length >= gap;
}

/**
 * @param {string} s
 * @returns {string} s with one trailing "…" removed
 */
function withoutEllipsis(s) {
  return s.endsWith(ELLIPSIS) ? s.slice(0, -ELLIPSIS.length) : s;
}

/**
 * Split a project path into its file name and its directory.
 * @param {string} path
 * @returns {{ name: string, dir: string }} dir is "" when the path has no slash
 */
export function splitPath(path) {
  const slash = path.lastIndexOf("/");
  if (slash === -1) return { name: path, dir: "" };
  return { name: path.slice(slash + 1), dir: path.slice(0, slash) };
}
