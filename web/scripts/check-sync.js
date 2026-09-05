// The lock, over every place a caret can be in a real document.
//
// "Keep in step" is a heuristic: it takes the words at the caret and looks for
// them in the other pane. What matters is not that it is clever but that it
// almost never comes up empty -- a lock that says "nothing to jump to" every
// few clicks reads as broken, whatever it says about itself. It used to fail
// at 6% of the positions in the markdown example and 12% in the typst one,
// because it only ever looked at the caret's own line: a blank line between
// paragraphs, a formula and a fenced block have no words to find.
//
// Run by `make test`, against the examples as they are actually published.
import { readFileSync } from "node:fs";
import { documentPlaceFor, sourcePlaceFor } from "../src/lib/sync.js";
import { renderHtml, renderMarkdown, renderTypst } from "./render.js";

// A caret every few characters is enough to catch a whole line going missing,
// and keeps this a second rather than a minute.
const STEP = 3;
// What is tolerated. Zero would be a promise this cannot keep for every
// document anyone writes; a document where one position in fifty finds nothing
// is still a lock that works.
const ALLOWED = 0.02;

const EXAMPLES = [
  ["examples/regression-tables.md", renderMarkdown, "markdown"],
  ["examples/intervals.typ", renderTypst, "typst"],
  ["examples/bootstrap.html", renderHtml, "html"],
];

// Where a caret can be. In markdown and typst, anywhere: the whole file is
// text somebody typed. In HTML most of the file is not -- a tag, a stylesheet,
// a script, a base64 image -- and a caret there has nothing to find in the
// document for the same reason a reader cannot see it. What is measured is the
// text nodes with words in them, which is the source an author reads.
function places(source, format) {
  const out = new Uint8Array(source.length);
  if (format !== "html") return out.fill(1);
  const markup = /<(script|style)\b[^>]*>[\s\S]*?<\/\1>|<head\b[\s\S]*?<\/head>|<[^>]*>/gi;
  let at = 0;
  const keep = (from, to) => {
    if (/\w/.test(source.slice(from, to))) out.fill(1, from, to);
  };
  for (const match of source.matchAll(markup)) {
    keep(at, match.index);
    at = match.index + match[0].length;
  }
  keep(at, source.length);
  // A run of a hundred characters with no space in it is a data URI or a
  // minified line, not a sentence.
  for (const match of source.matchAll(/\S{100,}/g)) {
    out.fill(0, match.index, match.index + match[0].length);
  }
  return out;
}

let bad = false;
for (const [file, render, format] of EXAMPLES) {
  const source = readFileSync(new URL(`../../${file}`, import.meta.url), "utf8");
  const rendered = await render(source, file);
  const prose = places(source, format);
  if (rendered === null) {
    console.log(`sync: ${file} is not built; run \`make examples\``);
    continue;
  }

  let missed = 0;
  let tried = 0;
  const kinds = {};
  for (let at = 0; at < source.length; at += STEP) {
    if (!prose[at]) continue;
    tried++;
    if (documentPlaceFor(source, at, rendered, format)) continue;
    missed++;
    const line = source.slice(source.lastIndexOf("\n", Math.max(0, at - 1)) + 1).split("\n")[0];
    const kind = line.trim() === "" ? "blank line" : /^\s*[#=]/.test(line) ? "heading" : "prose";
    kinds[kind] = (kinds[kind] || 0) + 1;
  }

  // And the other way, from a place in the document back to the source. Here
  // the answer is an offset a caret is put at, so it is not enough that one
  // was found: it has to be the right one. The word the document has at that
  // place must be the word the source has where the caret would land -- a
  // position into some intermediate copy of the text would pass "found" and
  // put the caret in the header.
  const word = (text, at) => text.slice(at).match(/[\w'’-]+/)?.[0]?.toLowerCase() || "";
  let back = 0;
  let misplaced = 0;
  let backTried = 0;
  for (let at = 0; at < rendered.length; at += STEP) {
    backTried++;
    const place = sourcePlaceFor(rendered, at, source, format);
    if (place === null) {
      back++;
      continue;
    }
    // The phrase that matched may have been taken a few windows either side of
    // the caret, which is what `sourcePlaceFor` falls back to, so what is asked
    // is that the word landed on is one the document has within that reach --
    // not that it is the exact one under the caret.
    const REACH = 600;
    const landed = word(source, place);
    const near = rendered.slice(Math.max(0, at - REACH), at + REACH).toLowerCase();
    if (landed && !near.includes(landed)) misplaced++;
  }

  const rate = missed / tried;
  const backRate = back / backTried;
  const placedRate = misplaced / backTried;
  console.log(
    `sync: ${file} — ${(rate * 100).toFixed(1)}% of carets and ${(backRate * 100).toFixed(1)}% of document places find nowhere, ` +
      `${(placedRate * 100).toFixed(1)}% land on the wrong words`,
  );
  if (rate > ALLOWED || backRate > ALLOWED || placedRate > ALLOWED) {
    bad = true;
    console.error(`  over the ${(ALLOWED * 100).toFixed(0)}% this is allowed to be: ${JSON.stringify(kinds)}`);
  }
}

if (bad) process.exit(1);
