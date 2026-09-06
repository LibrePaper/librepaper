// Keeping the source and the document pointing at the same place.
//
// The two are different texts: one has the markup in it and the other has what
// the markup produced. Nothing maps between them exactly -- a heading in typst
// is `= Title` and in the document it is `Title`, and a formula is neither.
// What they do share is the prose, so that is the bridge: take the words at
// the place someone just clicked, and find those words in the other one.
//
// It is a heuristic, and it says so by doing nothing when it is unsure. A
// wrong jump is worse than none: the reader loses their place and has to find
// it again themselves.

// Markup, as far as this is concerned: characters that are syntax in markdown
// or typst and never part of a word. Replaced by spaces rather than removed,
// so an offset into the flattened copy is still an offset into the original.
const MARKUP = /[#*_`~=$@<>[\]()|\\{}]/g;

// The markup of an HTML source is not a character but a tag, and its entities
// are words spelled sideways. Both are blanked rather than removed, and an
// entity keeps its decoded character followed by enough spaces to hold its
// place, so an offset into the flattened copy is still an offset into the
// source.
const TAG = /<[^>]*>/g;
const ENTITY = /&(#x?[0-9a-f]+|[a-z]+);/gi;
const NAMED = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };

function decodeInPlace(match, body) {
  let decoded = NAMED[body.toLowerCase()];
  if (decoded === undefined && body[0] === "#") {
    const code = body[1] === "x" || body[1] === "X"
      ? parseInt(body.slice(2), 16)
      : parseInt(body.slice(1), 10);
    // Browsers replace null, surrogate, and out-of-range numeric references
    // with U+FFFD. Keep this heuristic decoder just as forgiving: an invalid
    // source entity must not throw while the caller is trying to move a caret.
    const valid = Number.isInteger(code) && code > 0 && code <= 0x10ffff &&
      !(code >= 0xd800 && code <= 0xdfff);
    decoded = valid ? String.fromCodePoint(code) : "\ufffd";
  }
  if (decoded === undefined) return " ".repeat(match.length);
  return decoded + " ".repeat(Math.max(0, match.length - decoded.length));
}

// How much text to take. Long enough to be somewhere in particular, short
// enough not to cross whatever was reflowed between the two.
const WINDOW = 140;

// Below this a phrase is too common to identify a place: "the interval"
// appears throughout a document about intervals.
const ENOUGH = 16;

// The rendered side is always flattened by MARKUP, so the source side must be
// too, whatever it is written in: a needle that kept its brackets would never
// be found in a haystack whose brackets are spaces. For HTML the tags and the
// entities go first, and then the same rule as everywhere else.
//
// Blanking preserves length, so an offset survives it; collapsing whitespace
// does not, and a run of spaces where a tag used to be is most of an HTML
// source. So the collapse is done by hand, keeping for each character of the
// flattened copy the offset it came from. Without that map a match found here
// is a position in a text nobody has, and the caret lands somewhere else
// entirely.
function flattenWith(text, format) {
  const blanked = (
    format === "html"
      ? text.replace(TAG, (tag) => " ".repeat(tag.length)).replace(ENTITY, decodeInPlace)
      : text
  ).replace(MARKUP, " ");
  const out = [];
  const from = [];
  let wasSpace = false;
  for (let at = 0; at < blanked.length; at++) {
    const character = blanked[at];
    // The characters a run of whitespace can be made of. Tested by hand rather
    // than by a regular expression, because this runs once per character of a
    // document that may be megabytes of base64.
    if (character === " " || character === "\n" || character === "\t" || character === "\r" || character === "\f" || character === "\v") {
      // One space for the run, remembered at the first character of it.
      if (wasSpace) continue;
      wasSpace = true;
      out.push(" ");
      from.push(at);
      continue;
    }
    wasSpace = false;
    out.push(character);
    from.push(at);
  }
  // One past the end, so a match that runs to the end of the text has an end
  // offset to map to.
  from.push(blanked.length);
  return { text: out.join(""), from };
}

// The two haystacks -- a source and the document rendered from it -- are asked
// for again on every keystroke and every click, and each is flattened whole.
// Two slots is exactly enough to keep both, and the windows a phrase is taken
// from go straight to `flattenWith` so they cannot evict either.
const memo = [];
function flattened(text, format) {
  const known = memo.find((entry) => entry.text === text && entry.format === format);
  if (known) return known.result;
  const result = flattenWith(text, format);
  memo.unshift({ text, format, result });
  memo.length = Math.min(memo.length, 2);
  return result;
}

// A window of the text, flattened for matching only: small, different every
// time, and therefore never put in the memo the two haystacks live in.
const flatten = (text, format) => flattenWith(text, format).text;

// phrase takes the words at `at`, reading forwards -- what was clicked on is
// what follows the click, not what precedes it. Near the end of a paragraph
// there may not be enough of them, and then it reads backwards instead.
//
// withinLine keeps the window inside one line of the source, where a newline
// usually means a new paragraph and the words either side are not adjacent in
// the document. The rendered text has no such structure to respect: typst
// writes it as one line.
function phrase(text, at, withinLine, format) {
  // In an HTML source a newline is a break between tags, not between
  // paragraphs, and most of the characters in the window are markup rather
  // than words -- so the window is taken over more of it, and not stopped at
  // the end of a line.
  const html = format === "html";
  const width = html ? WINDOW * 6 : WINDOW;
  const stop = withinLine && !html ? text.indexOf("\n", at) : -1;
  const end = Math.min(text.length, at + width, stop === -1 ? Infinity : stop);

  // Never start mid-word: half a word matches nothing. Reading forward to the
  // next whole one rather than back to the start of this one, because the
  // rendered text has no spaces at element boundaries -- a heading runs
  // straight into the paragraph under it -- so backing up can produce a word
  // that appears in neither text ("noteThe interval").
  let start = at;
  if (start > 0 && !/\s/.test(text[start - 1])) {
    while (start < end && !/\s/.test(text[start])) start++;
  }

  let taken = flatten(text.slice(start, end), format).trim();
  if (taken.length >= ENOUGH) return taken;

  // Not enough ahead, so look behind: the end of a paragraph is still a place.
  let back = Math.max(0, at - width);
  if (withinLine && !html) {
    const line = text.lastIndexOf("\n", Math.max(0, at - 1));
    if (line !== -1) back = Math.max(back, line + 1);
  }
  taken = flatten(text.slice(back, end), format).trim();
  return taken.length >= ENOUGH ? taken : "";
}

// findOnce is a match that means something: a phrase appearing twice
// identifies neither place, so an ambiguous one counts as not found. The
// phrase is shortened from the right until it either matches once or runs out,
// since its tail is the part most likely to have been reflowed away.
function findOnce(haystack, needle) {
  for (let words = needle.split(" "); words.length >= 2; words = words.slice(0, -1)) {
    const candidate = words.join(" ");
    if (candidate.length < ENOUGH) break;
    const first = haystack.indexOf(candidate);
    if (first === -1) continue;
    if (haystack.indexOf(candidate, first + 1) === -1) {
      return { at: first, length: candidate.length };
    }
  }
  return null;
}

// How far to look for something findable when the caret is somewhere that has
// no words of its own. Three lines either way reaches the paragraph around a
// formula, a fenced block, or the blank line between two paragraphs, and stops
// well short of a different section.
const NEARBY_LINES = 3;

// The places worth trying, nearest first: where the caret actually is, then
// the start of each line around it. A caret on a blank line, in a formula or
// in a code fence has nothing to match on -- but the prose beside it does, and
// that is what the reader is looking at.
function* nearby(text, caret) {
  yield caret;
  const starts = [];
  for (let at = 0; at !== -1; at = text.indexOf("\n", at + 1)) starts.push(at === 0 ? 0 : at + 1);
  const here = starts.findIndex((start, index) => start <= caret && (starts[index + 1] ?? Infinity) > caret);
  if (here === -1) return;
  for (let step = 1; step <= NEARBY_LINES; step++) {
    // Backwards first: a formula or a fence belongs to the prose that
    // introduced it more often than to what follows.
    if (starts[here - step] !== undefined) yield starts[here - step];
    if (starts[here + step] !== undefined) yield starts[here + step];
  }
}

// Where in the document the caret in the source is pointing. `rendered` is the
// text the frame published; the offset returned is into that text, which is
// what the frame anchors everything else by.
export function documentPlaceFor(source, caret, rendered, format) {
  // A match is a place in the flattened copy, and what the frame wants is a
  // place in the text it published, so it is mapped back through the copy's
  // own record of where each character came from.
  const { text: haystack, from } = flattened(rendered);
  for (const at of nearby(source, caret)) {
    const wanted = phrase(source, at, true, format);
    if (!wanted) continue;
    const found = findOnce(haystack, wanted);
    if (found) {
      const start = from[found.at];
      return { at: start, length: from[found.at + found.length] - start };
    }
  }
  return null;
}

// And the other way: where in the source a place in the document is, as an
// offset for a caret rather than a scroll. The rendered text has no lines to
// speak of -- it is one long run -- so what is tried instead is a little
// further along it each time, which lands past whatever could not be matched.
export function sourcePlaceFor(rendered, at, source, format, { exact = false } = {}) {
  const { text: haystack, from } = flattened(source, format);
  // `exact` asks only the first question -- are these very words in this
  // text -- and skips the widening search around it. It exists because the
  // widening is what makes a search across several files meaningless: every
  // file has *something* within six hundred characters of anywhere, so the
  // first file tried would always answer and the rest would never be asked.
  const reach = exact ? 0 : NEARBY_LINES;
  for (let step = 0; step <= reach; step++) {
    for (const start of step === 0 ? [at] : [at - step * WINDOW, at + step * WINDOW]) {
      if (start < 0 || start >= rendered.length) continue;
      const wanted = phrase(rendered, start, false);
      if (!wanted) continue;
      const found = findOnce(haystack, wanted);
      // The caret goes where those words are in the source, which is where the
      // flattened copy says the match came from -- not where it was found in
      // the copy, which is a position in a text nobody is looking at.
      if (found) return from[found.at];
    }
  }
  return null;
}

// A document is a directory, so a place in the rendered page can be in any
// file in it: the caret for a paragraph a reader clicked may belong to a
// chapter the editor is not showing. The lock is therefore keyed by file as
// well as by position -- it says which file, and where in it.
//
// The order files are tried in is the answer to "where is it most likely",
// not an arbitrary sweep: the file on screen first, because a reader usually
// clicks near what they are editing; then the main file, which is most of a
// short paper; then the rest, sorted, so the answer does not depend on the
// order a map happened to iterate in. The first file whose words match wins,
// and a phrase that appears in two files is ambiguous in the document as
// well -- `findOnce` refuses it there for the same reason.
export function sourcePlaceInTree(rendered, at, tree, { open = "", formatOf } = {}) {
  const paths = Object.keys(tree.texts || {});
  const ordered = [
    ...(open && tree.texts[open] !== undefined ? [open] : []),
    ...(tree.main && tree.main !== open ? [tree.main] : []),
    ...paths.filter((path) => path !== open && path !== tree.main).sort(),
  ];
  // Two passes, and the order of them is the whole of why this works. The
  // first asks each file whether it contains these very words; only if no
  // file does does the second let each file search around the place, the way
  // it does for a one-file document. Without that, the widening search in the
  // first file answers every question and no other file is ever reached.
  for (const strict of [true, false]) {
    for (const path of ordered) {
      const format = formatOf ? formatOf(path) : "";
      const found = sourcePlaceFor(rendered, at, tree.texts[path], format, { exact: strict });
      if (found !== null) return { path, at: found };
    }
  }
  return null;
}
