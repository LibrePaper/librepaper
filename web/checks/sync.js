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
import { documentPlaceFor, sourcePlaceFor, sourcePlaceInTree, sourceSelectorFor } from "../src/lib/sync.js";
import { renderHtml, renderMarkdown, renderTypst } from "../tools/render.js";

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

// The other anchor: not a caret but a whole selection, taken to a place in
// the source rather than an offset into it. What is asked here is weaker than
// above -- a heading, a formula or a citation is allowed to miss, since the
// selection crossing one of those is exactly the case a partial match gives
// up on -- but never wrong: whatever comes back has to be a real slice of the
// source at the position it claims, and the words it stands for have to be
// the words that were actually selected, not some other run of them.
const SOURCE_ALLOWED = 0.25;
const SOURCE_STEP = STEP * 20;
// The same blanking `flattenWith` does in src/lib/sync.js, reproduced here
// rather than imported because that copy also keeps an offset map this check
// has no use for. What matters is that it treats markup the same way: an
// HTML source loses its tags and decodes its entities before the generic
// markup characters go, and everything else just loses the generic ones.
const HTML_TAG = /<[^>]*>/g;
const HTML_ENTITY = /&(#x?[0-9a-f]+|[a-z]+);/gi;
const HTML_NAMED = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };
function decodeHtmlEntity(match, body) {
  let decoded = HTML_NAMED[body.toLowerCase()];
  if (decoded === undefined && body[0] === "#") {
    const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
    const valid = Number.isInteger(code) && code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff);
    decoded = valid ? String.fromCodePoint(code) : "�";
  }
  return decoded === undefined ? " ".repeat(match.length) : decoded;
}
function flattenLoosely(text, format) {
  const blanked = format === "html"
    ? text.replace(HTML_TAG, (tag) => " ".repeat(tag.length)).replace(HTML_ENTITY, decodeHtmlEntity)
    : text;
  return blanked.replace(/[#*_`~=$@<>[\]()|\\{}]/g, " ").replace(/\s+/g, " ").trim();
}

// `count` words starting at the first whole word at or after `at`, the way a
// reader's drag would land on one -- never opened mid-word.
function wordsAt(text, at, count) {
  let start = at;
  if (start > 0 && !/\s/.test(text[start - 1])) {
    while (start < text.length && !/\s/.test(text[start])) start++;
  }
  const match = text.slice(start).match(new RegExp(`^(?:\\S+\\s+){0,${count - 1}}\\S+`));
  if (!match || !match[0].trim()) return null;
  return { exact: match[0], position: start };
}

for (const [file, render, format] of EXAMPLES) {
  const source = readFileSync(new URL(`../../${file}`, import.meta.url), "utf8");
  const rendered = await render(source, file);
  if (rendered === null) continue; // already reported as unbuilt above

  const tree = { main: file, texts: { [file]: source } };
  const formatOf = () => format;

  let tried = 0;
  let missed = 0;
  for (let at = 0, i = 0; at < rendered.length; at += SOURCE_STEP, i++) {
    const count = 3 + (i % 10); // three to twelve words, cycling
    const selection = wordsAt(rendered, at, count);
    if (!selection) continue;
    tried++;
    const found = sourceSelectorFor(rendered, selection, tree, { formatOf });
    if (!found) {
      missed++;
      continue;
    }
    // Never wrong, whatever else it is: the exact it returns really is at
    // `position` in the source it named, and what it stands for is the
    // beginning of what was actually selected -- not something else the
    // needle happened to also match.
    const wanted = flattenLoosely(selection.exact, "");
    const got = flattenLoosely(found.exact, format);
    const consistent = source.slice(found.position, found.position + found.exact.length) === found.exact;
    const prefixed = wanted === got || wanted.startsWith(got);
    if (!consistent || !prefixed) {
      bad = true;
      console.error(
        `sync: ${file} — a source anchor for "${selection.exact}" came back wrong: ` +
          (consistent ? `"${got}" is not a prefix of "${wanted}"` : "its exact does not match its own position"),
      );
    }
  }

  const missRate = tried ? missed / tried : 0;
  console.log(`sync: ${file} — ${(missRate * 100).toFixed(1)}% of selections found no source anchor`);
  if (missRate > SOURCE_ALLOWED) {
    bad = true;
    console.error(`  over the ${(SOURCE_ALLOWED * 100).toFixed(0)}% this is allowed to be`);
  }
}

// A handful of small, hand-written cases for sourceSelectorFor, where what
// the source actually says is known exactly rather than measured statistically.
{
  const markdownCase = (name, source, exact, position, expectedExact) => {
    const tree = { main: "doc.md", texts: { "doc.md": source } };
    const formatOf = () => "markdown";
    const found = sourceSelectorFor(source, { exact, position }, tree, { formatOf });
    const ok = expectedExact === null ? found === null : found?.exact === expectedExact;
    console.log(`sync: ${name} — ${found ? `"${found.exact}" at ${found.position}` : "nowhere"}`);
    if (!ok) {
      bad = true;
      console.error(`  expected ${expectedExact === null ? "nothing" : `"${expectedExact}"`}`);
    }
  };

  markdownCase(
    "markup in the way is skipped over, not matched against",
    "Some **bold** words here for good measure.",
    "bold words here",
    null,
    "**bold** words here",
  );

  {
    // The selection reads across a citation, which the source spells with an
    // `@` the flattening rule blanks -- so the phrase as a whole is nowhere
    // in the source, and what comes back covers only the words before it.
    const src = "The result holds under the usual conditions [@smith2020] as shown.";
    const tree = { main: "doc.md", texts: { "doc.md": src } };
    const found = sourceSelectorFor(src, { exact: "usual conditions as shown", position: null }, tree, {
      formatOf: () => "markdown",
    });
    console.log(`sync: a citation in the middle — ${found ? `"${found.exact}"` : "nowhere"}`);
    if (!found || found.exact !== "usual conditions") {
      bad = true;
      console.error(`  expected the words before the citation, got ${found ? `"${found.exact}"` : "nothing"}`);
    }
  }

  {
    // The phrase is repeated, and a position hint says which copy was meant.
    const src = "First: the interesting result holds. Later: the interesting result holds again.";
    const tree = { main: "doc.md", texts: { "doc.md": src } };
    const formatOf = () => "markdown";
    const near = sourceSelectorFor(src, { exact: "the interesting result holds", position: 60 }, tree, { formatOf });
    console.log(`sync: a repeated phrase with a hint — ${near ? `"${near.exact}" at ${near.position}` : "nowhere"}`);
    if (!near || near.position !== src.lastIndexOf("the interesting result holds")) {
      bad = true;
      console.error("  expected the occurrence nearest the hint");
    }
    const blind = sourceSelectorFor(src, { exact: "the interesting result holds", position: null }, tree, { formatOf });
    console.log(`sync: a repeated phrase with no hint — ${blind ? `"${blind.exact}"` : "nowhere"}`);
    if (blind !== null) {
      bad = true;
      console.error("  an ambiguous phrase with no hint should not be guessed at");
    }
  }

  markdownCase(
    "a short, unique selection is still found",
    "The estimator converges to the population parameter eventually.",
    "population parameter",
    null,
    "population parameter",
  );

  {
    // Short enough that a hint would normally break the tie -- but a short
    // phrase is held to the stricter, hint-free standard, so it is refused.
    const src = "First, the interval is wide. Later, the interval is narrow instead.";
    const tree = { main: "doc.md", texts: { "doc.md": src } };
    const found = sourceSelectorFor(src, { exact: "the interval", position: 40 }, tree, { formatOf: () => "markdown" });
    console.log(`sync: a short, repeated selection — ${found ? `"${found.exact}"` : "nowhere"}`);
    if (found !== null) {
      bad = true;
      console.error("  a two-word phrase that repeats should not be resolved by a hint");
    }
  }
}

// A document is a directory, so the lock has to say which file as well as
// where in it. This is the case the one-file examples above cannot cover: two
// texts, both of whose prose is in the one rendered page, and a click in the
// page that has to land in the right one.
//
// Built here rather than added to examples/: it exists to exercise the lock
// across files, and a document that is only ever compiled by this check does
// not belong in the set a reader can open.
{
  const tree = {
    main: "main.typ",
    texts: {
      "main.typ": [
        '#import "chapter.typ": later',
        "",
        "= The opening",
        "",
        "The first chapter argues that the estimator is consistent under",
        "the stated assumptions, which is weaker than it sounds.",
        "",
        "#later()",
        "",
      ].join("\n"),
      "chapter.typ": [
        "#let later() = [",
        "  = The second part",
        "",
        "  Here the argument turns to the variance, where the interesting",
        "  behaviour is, and where the simulations disagree with the theory.",
        "]",
        "",
      ].join("\n"),
    },
  };
  const rendered = renderTypst(tree, "main.typ");
  if (rendered === null) {
    console.log("sync: no typst module built; skipping the two-file case");
  } else {
    const formatOf = () => "typst";
    // A phrase from each file, found in the rendered page, and asked for
    // back: the lock must name the file the words were written in, not the
    // file that happens to be open.
    const cases = [
      ["consistent under", "main.typ"],
      ["simulations disagree", "chapter.typ"],
    ];
    for (const [phrase, wanted] of cases) {
      const at = rendered.indexOf(phrase.split(" ")[0]);
      const found = at < 0 ? null : sourcePlaceInTree(rendered, at, tree, { formatOf });
      const ok = found && found.path === wanted;
      console.log(
        `sync: "${phrase}" traced to ${found ? found.path : "nowhere"}` +
          (ok ? "" : ` — expected ${wanted}`),
      );
      if (!ok) bad = true;
    }
    // And the file on screen is tried first, which is what stops a phrase
    // that appears in two chapters jumping somebody out of the one they are
    // editing.
    const both = {
      main: "main.typ",
      texts: { "main.typ": "the same sentence here\n", "other.typ": "the same sentence here\n" },
    };
    const open = sourcePlaceInTree("the same sentence here", 4, both, {
      open: "other.typ",
      formatOf,
    });
    if (!open || open.path !== "other.typ") {
      console.error("  the open file was not tried first");
      bad = true;
    }
  }
}

if (bad) process.exit(1);
