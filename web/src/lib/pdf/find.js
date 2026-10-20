// Search inside the drawn pages of a PDF.
//
// The text to search is the text layer's own text nodes, page by page, so a
// match is exactly what the reader can see and select. Matches are marked with
// the CSS Custom Highlight API ("lp-find" for all of them, "lp-find-current"
// for the one the reader is on) rather than by wrapping them in elements:
// `frame.js` republishes the document's text on every body mutation, and a
// search that rewrote the text layer would trigger that on every keystroke.
//
// The matching itself is pure and exported (`findOffsets`) so
// `tests/unit/pdf-find.mjs` can check it without a browser.

/// More matches than this are counted and highlighted no further. A one
/// letter query on a long book would otherwise build tens of thousands of
/// ranges for a result nobody reads.
export const MAX_MATCHES = 10000;

// Lowercase one UTF-16 unit without ever changing the length of the string,
// so an offset into the folded text is an offset into the original.
function fold(ch) {
  const lower = ch.toLocaleLowerCase();
  return lower.length === ch.length ? lower : ch;
}

const isSpace = (ch) => /\s/.test(ch);

/// `text` with every run of whitespace collapsed to one space and the rest
/// folded to lower case, plus for each output character the span of the
/// original it came from: `starts[i]` and `ends[i]` (exclusive).
export function normalize(text) {
  let norm = "";
  const starts = [];
  const ends = [];
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (isSpace(ch)) {
      if (norm.endsWith(" ")) {
        ends[ends.length - 1] = i + 1;
      } else {
        norm += " ";
        starts.push(i);
        ends.push(i + 1);
      }
    } else {
      norm += fold(ch);
      starts.push(i);
      ends.push(i + 1);
    }
  }
  return { norm, starts, ends };
}

/// Where `query` occurs in `text`, as `{ start, end }` offsets into `text`.
///
/// Case-insensitive, and a run of whitespace in the text (a space, a line
/// break from a gap element, several of them) matches a run in the query.
/// Matches do not overlap. The query is trimmed; an empty one matches nothing.
export function findOffsets(text, query, limit = MAX_MATCHES) {
  const needle = normalize(query).norm.trim();
  if (needle === "") return [];
  const { norm, starts, ends } = normalize(text);
  const found = [];
  let from = 0;
  while (found.length < limit) {
    const at = norm.indexOf(needle, from);
    if (at < 0) break;
    found.push({ start: starts[at], end: ends[at + needle.length - 1] });
    from = at + needle.length;
  }
  return found;
}

/// The index of the match a search lands on. Step 0 is the first match on or
/// after `fromPage` (wrapping to the first overall); 1 and -1 move from
/// `current` and wrap. `pages[i]` is the page of match i.
export function pickIndex(pages, current, step, fromPage) {
  const total = pages.length;
  if (total === 0) return -1;
  if (step === 0) {
    const index = pages.findIndex((page) => page >= fromPage);
    return index < 0 ? 0 : index;
  }
  const base = current < 0 ? (step > 0 ? -1 : 0) : current;
  return (((base + step) % total) + total) % total;
}

// The text node and offset inside it that a character offset of a page's
// concatenated text falls on. `starts` is the offset each node begins at.
// A range start prefers the node a boundary begins, an end the one it closes.
function locate(nodes, starts, offset, isEnd) {
  let low = 0;
  let high = nodes.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    const begins = starts[mid];
    if (isEnd ? begins < offset : begins <= offset) low = mid;
    else high = mid - 1;
  }
  return { node: nodes[low], offset: offset - starts[low] };
}

function textNodes(layer) {
  const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT);
  const nodes = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) nodes.push(node);
  return nodes;
}

/// Search the drawn text layers under `root()`. `onresult({ current, total })`
/// follows every search, step, refresh and clear; `current` is 1-based, 0 when
/// there is no match.
export function createFind({ root, onresult }) {
  let query = "";
  let matches = []; // { page, range }
  let index = -1;

  const highlights = globalThis.CSS?.highlights;
  const canHighlight = Boolean(highlights && globalThis.Highlight);

  function collect() {
    const container = root();
    const found = [];
    if (!container || query === "") return found;
    for (const page of container.querySelectorAll(".page[data-page]")) {
      const number = Number(page.dataset.page);
      const nodes = [];
      for (const layer of page.querySelectorAll(".textLayer")) nodes.push(...textNodes(layer));
      if (nodes.length === 0) continue;
      const starts = [];
      let text = "";
      for (const node of nodes) {
        starts.push(text.length);
        text += /** @type {Text} */ (node).data;
      }
      for (const { start, end } of findOffsets(text, query, MAX_MATCHES - found.length)) {
        const from = locate(nodes, starts, start, false);
        const to = locate(nodes, starts, end, true);
        const range = document.createRange();
        range.setStart(from.node, from.offset);
        range.setEnd(to.node, to.offset);
        found.push({ page: number, range });
      }
      if (found.length >= MAX_MATCHES) break;
    }
    return found;
  }

  function paint() {
    if (!canHighlight) return;
    if (matches.length === 0) {
      highlights.delete("lp-find");
      highlights.delete("lp-find-current");
      return;
    }
    highlights.set("lp-find", new Highlight(...matches.map((match) => match.range)));
    if (index >= 0) highlights.set("lp-find-current", new Highlight(matches[index].range));
    else highlights.delete("lp-find-current");
  }

  function reveal() {
    const rect = matches[index]?.range.getBoundingClientRect();
    if (!rect || (rect.width === 0 && rect.height === 0)) return;
    const top = rect.top + rect.height / 2 - window.innerHeight / 2;
    const offscreen = rect.left < 0 || rect.right > window.innerWidth;
    const left = offscreen ? rect.left + rect.width / 2 - window.innerWidth / 2 : 0;
    const calm = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    window.scrollBy({ top, left, behavior: calm ? "instant" : "smooth" });
  }

  function report() {
    onresult({ current: index >= 0 ? index + 1 : 0, total: matches.length });
  }

  function search(text, step, fromPage) {
    const same = text === query;
    query = text;
    matches = collect();
    index = pickIndex(
      matches.map((match) => match.page),
      same ? index : -1,
      step,
      fromPage,
    );
    paint();
    if (index >= 0) reveal();
    report();
  }

  function refresh() {
    if (query === "") return;
    matches = collect();
    index = matches.length === 0 ? -1 : Math.max(0, Math.min(index, matches.length - 1));
    paint();
    report();
  }

  function clear() {
    query = "";
    matches = [];
    index = -1;
    if (canHighlight) {
      highlights.delete("lp-find");
      highlights.delete("lp-find-current");
    }
    onresult({ current: 0, total: 0 });
  }

  return { search, refresh, clear };
}
