// The project-wide search: one matcher that the panel, the results list and the
// replace action all read, so the match the list shows is the same match the
// replacement rewrites.
//
// Offsets are UTF-16 code units, the unit both RegExp.exec and CodeMirror use,
// so a match can be dispatched into an editor without conversion. The `u` flag
// is used where the pattern allows it, so a match never splits a surrogate pair
// and word boundaries know about letters beyond ASCII. A pattern that is only
// valid without `u` falls back to a non-`u` RegExp.
//
// This module is pure. It imports nothing, so the Node unit tests can run it
// without a browser or the message catalogue.

const BEFORE_MAX = 40;
const AFTER_MAX = 80;
const TEXT_MAX = 200;
const LIMIT_DEFAULT = 1000;
const ELLIPSIS = "…";

/**
 * @typedef {{ from: number, to: number, line: number, column: number, before: string, text: string, after: string }} Match
 */

/**
 * @param {string} s
 * @returns {string} s with every RegExp metacharacter escaped
 */
function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Files are listed the way a person counts them: ch2 before ch10.
/** @param {{ path: string }} a @param {{ path: string }} b */
function byPath(a, b) {
  return a.path.localeCompare(b.path, undefined, { numeric: true });
}

/**
 * Compile the panel's options into a global RegExp.
 * @param {{ query: string, caseSensitive?: boolean, wholeWord?: boolean, regex?: boolean }} options
 * @returns {{ re: RegExp | null, error: "" | "invalid" }}
 *   re is null with error "" when the query is empty.
 */
export function compileQuery(options) {
  const query = options?.query ?? "";
  if (query === "") return { re: null, error: "" };
  const raw = options.regex ? query : escapeRegExp(query);
  // Literal queries are always escaped syntax characters, which are valid under
  // `u`. A regex tries `u` first and falls back to the plain form.
  const modes = options.regex ? [true, false] : [true];
  for (const unicode of modes) {
    try {
      // Check the person's own pattern before wrapping it: `\b(?:a)(\b` would
      // otherwise hide a broken pattern inside a valid group.
      new RegExp(raw, unicode ? "u" : "");
    } catch {
      continue;
    }
    const source = options.wholeWord ? wholeWordSource(raw, unicode) : raw;
    const flags = (options.caseSensitive ? "gm" : "gim") + (unicode ? "u" : "");
    return { re: new RegExp(source, flags), error: "" };
  }
  return { re: null, error: "invalid" };
}

// With `u`, a word is a letter, number or underscore in any script. Without it,
// \b is the ASCII word boundary.
/** @returns {string} */
function wholeWordSource(raw, unicode) {
  if (unicode) return `(?<![\\p{L}\\p{N}_])(?:${raw})(?![\\p{L}\\p{N}_])`;
  return `\\b(?:${raw})\\b`;
}

/**
 * Search every file.
 * @param {{ id: string, path: string, text: string }[]} files
 * @param {{ query: string, caseSensitive?: boolean, wholeWord?: boolean, regex?: boolean }} options
 * @param {{ limit?: number }} [opts]   default limit 1000
 * @returns {{
 *   groups: { id: string, path: string, matches: Match[] }[],
 *   total: number,
 *   capped: boolean,
 *   error: "" | "invalid",
 * }}
 */
export function searchProject(files, options, opts) {
  const limit = opts?.limit ?? LIMIT_DEFAULT;
  const { re, error } = compileQuery(options);
  if (!re) return { groups: [], total: 0, capped: false, error };

  // Walking the files in path order makes the cap cut at the same place no
  // matter what order the caller listed them in.
  const groups = [];
  let total = 0;
  for (const file of [...files].sort(byPath)) {
    const { matches, more } = findMatches(file.text, re, limit - total);
    if (matches.length > 0) {
      groups.push({ id: file.id, path: file.path, matches });
      total += matches.length;
    }
    if (more) return { groups, total, capped: true, error: "" };
  }
  return { groups, total, capped: false, error: "" };
}

/**
 * The matches in one text, up to `room`. `more` says whether a match exists
 * beyond the room, which is what makes the search report it was capped.
 * @param {string} text
 * @param {RegExp} re a global RegExp
 * @param {number} room
 * @returns {{ matches: Match[], more: boolean }}
 */
function findMatches(text, re, room) {
  const matches = [];
  // Line numbers are counted by walking forward through the newlines once,
  // not by rescanning the file from the start for every match.
  let line = 1;
  let lineStart = 0;
  let nextNewline = text.indexOf("\n");
  re.lastIndex = 0;
  let m;
  while ((m = re.exec(text)) !== null) {
    // A zero-length match (`^`, `a*`) would leave lastIndex where it was and
    // loop forever, so step past it and do not report it.
    if (m[0].length === 0) {
      re.lastIndex = m.index + (re.unicode ? codePointWidth(text, m.index) : 1);
      continue;
    }
    if (matches.length === room) return { matches, more: true };
    while (nextNewline !== -1 && nextNewline < m.index) {
      line += 1;
      lineStart = nextNewline + 1;
      nextNewline = text.indexOf("\n", lineStart);
    }
    matches.push(describe(text, m.index, m[0], line, lineStart));
  }
  return { matches, more: false };
}

// 2 when a surrogate pair starts at `index`, otherwise 1.
/** @returns {number} */
function codePointWidth(text, index) {
  const high = text.charCodeAt(index);
  const low = text.charCodeAt(index + 1);
  return high >= 0xd800 && high <= 0xdbff && low >= 0xdc00 && low <= 0xdfff ? 2 : 1;
}

/**
 * @param {string} text
 * @param {number} from
 * @param {string} matched
 * @param {number} line
 * @param {number} lineStart offset of the line the match starts on
 * @returns {Match}
 */
function describe(text, from, matched, line, lineStart) {
  const to = from + matched.length;
  return {
    from,
    to,
    line,
    column: from - lineStart + 1,
    before: contextBefore(text, from, lineStart),
    text: shownText(matched),
    after: contextAfter(text, to),
  };
}

// The context stays on the match's own line: a snippet that ran across a
// newline would look like two lines in the list.
/** @returns {string} */
function contextBefore(text, from, lineStart) {
  const start = Math.max(lineStart, from - BEFORE_MAX);
  return (start > lineStart ? ELLIPSIS : "") + text.slice(start, from);
}

// Only the slice that could be shown is read, so a huge file with no newline
// after a match does not cost a scan to the end of the file for each match.
/** @returns {string} */
function contextAfter(text, to) {
  const tail = text.slice(to, to + AFTER_MAX + 1);
  const newline = tail.indexOf("\n");
  const rest = newline === -1 ? tail : tail.slice(0, newline);
  return rest.length > AFTER_MAX ? rest.slice(0, AFTER_MAX) + ELLIPSIS : rest;
}

/** @returns {string} */
function shownText(matched) {
  const newline = matched.indexOf("\n");
  if (newline === -1 && matched.length <= TEXT_MAX) return matched;
  const head = newline === -1 ? matched : matched.slice(0, newline);
  return head.slice(0, TEXT_MAX) + ELLIPSIS;
}

/**
 * The edits that replace the given matches in one file's current text.
 * @param {string} text            the file's current text
 * @param {Match[]} matches        matches from searchProject for this file (any subset, any order)
 * @param {string} replacement
 * @param {{ query: string, caseSensitive?: boolean, wholeWord?: boolean, regex?: boolean }} options
 * @returns {{ from: number, to: number, insert: string }[]}  sorted by `from` DESCENDING, non-overlapping
 */
export function replacementsFor(text, matches, replacement, options) {
  const { re } = compileQuery(options);
  if (!re) return [];
  // The sticky copy tests the pattern at one offset. The search's own regex
  // cannot do that, because it would scan forward from lastIndex.
  const sticky = new RegExp(re.source, re.flags.replace("g", "y"));
  const edits = [];
  // Working from the end of the file backwards keeps every earlier offset
  // valid while the edits are applied in order.
  let floor = Infinity;
  for (const match of [...matches].sort((a, b) => b.from - a.from)) {
    if (match.to > floor) continue;
    sticky.lastIndex = match.from;
    const hit = sticky.exec(text);
    // The file changed since the search: this offset no longer holds the match.
    if (!hit || match.from + hit[0].length !== match.to) continue;
    const insert = options.regex ? expandTemplate(replacement, hit, text, match) : replacement;
    edits.push({ from: match.from, to: match.to, insert });
    floor = match.from;
  }
  return edits;
}

// JavaScript's own substitution rules, applied to this match's groups. The
// full text was matched, so lookarounds still see their context, which is why
// String.prototype.replace is not run on the matched slice alone.
/**
 * @param {string} template
 * @param {RegExpExecArray} hit
 * @param {string} text the file's current text
 * @param {{ from: number, to: number }} match
 * @returns {string}
 */
function expandTemplate(template, hit, text, match) {
  const count = hit.length - 1;
  return template.replace(/\$(\$|&|`|'|<[^>]*>|\d{1,2})/g, (whole, token) => {
    if (token === "$") return "$";
    if (token === "&") return hit[0];
    if (token === "`") return text.slice(0, match.from);
    if (token === "'") return text.slice(match.to);
    if (token[0] === "<") {
      if (!hit.groups) return whole;
      return hit.groups[token.slice(1, -1)] ?? "";
    }
    return groupFor(token, hit, count, whole);
  });
}

// `$12` is group 12 when there are that many groups, and otherwise group 1
// followed by a literal 2. A number with no such group is left as written.
/**
 * @param {string} token one or two digits
 * @param {RegExpExecArray} hit
 * @param {number} count number of capture groups
 * @param {string} whole the text as written, returned when there is no group
 * @returns {string}
 */
function groupFor(token, hit, count, whole) {
  const n = Number(token);
  if (n >= 1 && n <= count) return hit[n] ?? "";
  const first = Number(token[0]);
  if (token.length === 2 && first >= 1 && first <= count) return (hit[first] ?? "") + token[1];
  return whole;
}
