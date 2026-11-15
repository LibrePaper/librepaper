// Split the HTML of a local Quarto render into one output per code cell.
//
// The desktop app renders a qmd to HTML with the code run. This module cuts
// that page into the `div.cell` elements Quarto writes for each executed
// cell, drops the echoed source code, and pairs each output with the
// fingerprint of the cell it came from, so collaborators' drafts can show the
// output under the right cell.
//
// It runs in plain Node tests too, so it carries its own small tokenizer
// instead of using DOMParser. Quarto's HTML is well formed apart from void
// elements, and its data: URIs contain no angle brackets.

import { cellFingerprint, expandedCells, expandedInlines, inlineFingerprint } from "./engines/quarto.js";

const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);

class ParseError extends Error {}

// Tokens are { kind: "comment" | "text" | "open" | "close", raw, name }.
// Scripts are dropped here, together with everything inside them.
function tokenize(html) {
  const tokens = [];
  let i = 0;
  const length = html.length;
  while (i < length) {
    if (html[i] !== "<" || !/[A-Za-z/!]/.test(html[i + 1] ?? "")) {
      let end = html.indexOf("<", i + 1);
      // A "<" that does not start a tag is plain text.
      while (end >= 0 && !/[A-Za-z/!]/.test(html[end + 1] ?? "")) end = html.indexOf("<", end + 1);
      if (end < 0) end = length;
      tokens.push({ kind: "text", raw: html.slice(i, end) });
      i = end;
      continue;
    }
    if (html.startsWith("<!--", i)) {
      const end = html.indexOf("-->", i + 4);
      if (end < 0) throw new ParseError("unterminated comment");
      tokens.push({ kind: "comment", raw: html.slice(i, end + 3) });
      i = end + 3;
      continue;
    }
    let j = i + 1;
    let quote = "";
    while (j < length) {
      const char = html[j];
      if (quote) { if (char === quote) quote = ""; } else if (char === '"' || char === "'") quote = char;
      else if (char === ">") break;
      j += 1;
    }
    if (j >= length) throw new ParseError("unterminated tag");
    const raw = html.slice(i, j + 1);
    i = j + 1;
    const closing = raw[1] === "/";
    const name = (/^<\/?([A-Za-z][^\s/>]*)/.exec(raw)?.[1] || "").toLowerCase();
    if (!name) {
      // <!doctype ...> and the like.
      tokens.push({ kind: "comment", raw });
      continue;
    }
    if (!closing && (name === "script" || name === "style")) {
      const close = new RegExp(`</${name}\\s*>`, "i");
      const rest = html.slice(i);
      const found = close.exec(rest);
      if (!found) throw new ParseError(`unterminated ${name}`);
      if (name === "style") {
        tokens.push({ kind: "open", raw, name }, { kind: "raw", raw: rest.slice(0, found.index) }, { kind: "close", raw: found[0], name });
      }
      i += found.index + found[0].length;
      continue;
    }
    if (closing) tokens.push({ kind: "close", raw, name });
    else tokens.push({ kind: "open", raw, name, selfClosing: VOID.has(name) || /\/\s*>$/.test(raw) });
  }
  return tokens;
}

function attribute(raw, name) {
  const match = new RegExp(`\\s${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, "i").exec(raw);
  return match ? (match[1] ?? match[2] ?? match[3] ?? "") : null;
}

function classes(token) {
  return new Set((attribute(token.raw, "class") || "").split(/\s+/).filter(Boolean));
}

// Closing tags pop to the nearest open element of the same name, so a stray
// or implied end tag cannot unbalance the walk. Returns false for a closing
// tag that matches nothing.
function popTo(stack, name) {
  const at = stack.lastIndexOf(name);
  if (at < 0) return false;
  stack.length = at;
  return true;
}

// The tokens between `<main id="quarto-document-content">` and its end, or
// all of them when the page has no such element.
function contentTokens(tokens) {
  const start = tokens.findIndex((token) => token.kind === "open" && token.name === "main" && attribute(token.raw, "id") === "quarto-document-content");
  if (start < 0) return tokens;
  const stack = ["main"];
  for (let i = start + 1; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token.kind === "open" && !token.selfClosing) stack.push(token.name);
    else if (token.kind === "close" && popTo(stack, token.name) && stack.length === 0) return tokens.slice(start + 1, i);
  }
  throw new ParseError("unterminated main");
}

// Top-level `.cell` elements, in document order, as token slices.
function findCells(tokens) {
  const cells = [];
  const stack = [];
  let start = -1;
  let depth = 0;
  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token.kind === "open") {
      if (token.selfClosing) continue;
      if (start < 0 && token.name === "div" && classes(token).has("cell")) {
        start = i;
        depth = stack.length;
      }
      stack.push(token.name);
    } else if (token.kind === "close") {
      if (popTo(stack, token.name) && start >= 0 && stack.length === depth) {
        cells.push(tokens.slice(start, i + 1));
        start = -1;
      }
    }
  }
  if (start >= 0) throw new ParseError("unterminated cell");
  return cells;
}

function isCode(token) {
  const own = classes(token);
  if (token.name === "details") return own.has("code-fold");
  if (token.name === "div") return own.has("code-copy-outer-scaffold") || own.has("cell-code");
  if (token.name === "pre") return own.has("cell-code");
  return false;
}

const HANDLER = /\s+on[a-z]+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+)/gi;

// One cell as a single line of HTML without its echoed code.
function cellHtml(tokens) {
  const out = [];
  const stack = [];
  let skip = -1;
  let pre = 0;
  for (const token of tokens) {
    if (token.kind === "open") {
      if (skip < 0 && !token.selfClosing && isCode(token)) skip = stack.length;
      if (!token.selfClosing) stack.push(token.name);
      if (skip >= 0) continue;
      if (token.name === "pre" && !token.selfClosing) pre += 1;
      out.push(token.raw.replace(HANDLER, "").replace(/\r?\n/g, " "));
    } else if (token.kind === "close") {
      if (!popTo(stack, token.name)) continue;
      if (skip >= 0) {
        if (stack.length <= skip) skip = -1;
        continue;
      }
      if (token.name === "pre" && pre > 0) pre -= 1;
      out.push(token.raw.replace(/\r?\n/g, " "));
    } else if (skip < 0) {
      const newline = token.kind === "text" && pre > 0 ? "&#10;" : " ";
      out.push(token.raw.replace(/\r?\n/g, newline));
    }
  }
  return out.join("");
}

// Quarto writes a `div.cell` only for a chunk that prints something, so a
// silent chunk (say `source("R/load.R")` under echo: false) has no div and
// cannot be paired by position. A `classes:` option puts a tag on its div.
const TAG = /^lp-cell-(\d+)$/;
const CLASS_ATTR = /(\sclass\s*=\s*)(?:(["'])([^"']*)\2|([^\s>]+))/i;
const OPTION_LINE = /^\s*#\|\s*[^:]+?\s*:/;
const CLASSES_LINE = /^\s*#\|\s*classes\s*:/;

// The tagged copy of each executed cell's source: a `#| classes: lp-cell-N`
// line after its opening fence, N being the cell's index in expandedCells.
// The tree passed in is left as it was.
export function tagCells(tree) {
  const texts = { ...(tree.texts ?? {}) };
  const files = tree.texts ?? {};
  const cells = expandedCells(texts[tree.main] ?? "", { path: tree.main, files });
  const inlines = expandedInlines(texts[tree.main] ?? "", { path: tree.main, files });
  // Inline values first. Each wrap stays inside its line, so the line numbers
  // the cell marks below count from still hold. Within a line the wraps run
  // right to left, so an earlier span's offsets are not moved by a later one.
  // Two records at one position (a file included twice) get one wrap: the
  // second has no tag of its own and is left without output.
  const inlineByFile = new Map();
  inlines.forEach((record, index) => {
    if (!inlineByFile.has(record.file)) inlineByFile.set(record.file, []);
    inlineByFile.get(record.file).push({ index, line: record.line, start: record.start, end: record.end });
  });
  for (const [file, marks] of inlineByFile) {
    const lines = String(texts[file] ?? "").split("\n");
    const seen = new Set();
    const ordered = marks
      .filter((mark) => {
        const key = `${mark.line}:${mark.start}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .sort((a, b) => a.line - b.line || b.start - a.start);
    for (const { index, line, start, end } of ordered) {
      const text = lines[line - 1];
      lines[line - 1] = `${text.slice(0, start)}[${text.slice(start, end)}]{.lp-inline-${index}}${text.slice(end)}`;
    }
    texts[file] = lines.join("\n");
  }
  const byFile = new Map();
  cells.forEach((cell, index) => {
    if (!byFile.has(cell.file)) byFile.set(cell.file, []);
    byFile.get(cell.file).push({ index, startLine: cell.startLine });
  });
  for (const [file, marks] of byFile) {
    const lines = String(texts[file] ?? "").split("\n");
    // Lines keep their "\r", so an inserted line gets one too in a CRLF file.
    const cr = lines.length > 1 && lines[0].endsWith("\r") ? "\r" : "";
    // Bottom up, so the fence lines of the cells still to be tagged hold.
    for (const { index, startLine } of marks.sort((a, b) => b.startLine - a.startLine)) {
      const tag = `lp-cell-${index}`;
      let at = startLine + 1;
      while (at < lines.length && OPTION_LINE.test(lines[at]) && !CLASSES_LINE.test(lines[at])) at += 1;
      if (at < lines.length && CLASSES_LINE.test(lines[at])) lines[at] = addClass(lines[at], tag);
      else lines.splice(startLine + 1, 0, `#| classes: ${tag}${cr}`);
    }
    texts[file] = lines.join("\n");
  }
  return { ...tree, texts };
}

// Appends a tag to an existing `classes:` value. A second `classes:` key would
// be a duplicate YAML key. Quarto writes the value as `{.cell .<value>}`, so
// only the first class gets its dot: every later class is written with its
// own, or the cell's div breaks into plain text. A flow list is left alone
// and its cell goes untagged, which costs that cell its output only.
function addClass(line, tag) {
  const [, head, space, value, tail] = /^(\s*#\|\s*classes\s*:)([ \t]*)(.*?)(\s*)$/.exec(line);
  if (value.startsWith("[")) return line;
  const quote = value.length >= 2 && (value[0] === '"' || value[0] === "'") && value.endsWith(value[0]) ? value[0] : "";
  const words = (quote ? value.slice(1, -1) : value).split(/\s+/).filter(Boolean);
  words.push(tag);
  const dotted = words.map((word, at) => (at === 0 || word.startsWith(".") ? word : `.${word}`)).join(" ");
  return `${head}${space || " "}${quote}${dotted}${quote}${tail}`;
}

// The index a tagged cell's outer div carries, or null for an untagged cell.
function cellTag(tokens) {
  for (const name of classes(tokens[0])) {
    const match = TAG.exec(name);
    if (match) return Number(match[1]);
  }
  return null;
}

// The tokens of a tagged cell with its tag class taken off every element:
// Quarto repeats it on the inner div of a labelled figure or table. The tag
// never reaches the stored record.
function untagged(tokens) {
  return tokens.map((token) => (token.kind !== "open" ? token : {
    ...token,
    raw: token.raw.replace(CLASS_ATTR, (_all, lead, quote, quoted, bare) => {
      const kept = (quoted ?? bare ?? "").split(/\s+/).filter((name) => name && !TAG.test(name)).join(" ");
      return quote === undefined ? `${lead}"${kept}"` : `${lead}${quote}${kept}${quote}`;
    }),
  }));
}

// The output of a tagged cell. A cell Quarto wrote with nothing in it (silent
// and without output) gives "", which draws as no output.
function taggedOutput(tokens) {
  if (cellHtml(tokens.slice(1, -1)).trim() === "") return "";
  return cellHtml(untagged(tokens));
}

const INLINE_TAG = /^lp-inline-(\d+)$/;

// The content of each `span.lp-inline-N` in the page, keyed by N. The span may
// hold markup, so the content runs to its matching close tag. The first span
// for an N wins.
function inlineSpans(tokens) {
  const spans = new Map();
  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token.kind !== "open" || token.name !== "span" || token.selfClosing) continue;
    let tag = null;
    for (const name of classes(token)) {
      const match = INLINE_TAG.exec(name);
      if (match) { tag = Number(match[1]); break; }
    }
    if (tag === null) continue;
    let depth = 1;
    let j = i + 1;
    for (; j < tokens.length && depth > 0; j += 1) {
      const next = tokens[j];
      if (next.kind === "open" && next.name === "span" && !next.selfClosing) depth += 1;
      else if (next.kind === "close" && next.name === "span") depth -= 1;
    }
    // An unclosed span has no value; its place is left blank.
    if (depth > 0 || spans.has(tag)) continue;
    spans.set(tag, cellHtml(tokens.slice(i + 1, j - 1)));
  }
  return spans;
}

// One entry per inline value of the document, in document order. Null when the
// document has none, or when the page carries no tagged span at all (an
// untagged page): then the run's own page is the only honest output.
async function inlineEntries(content, qmdSource, { path, files }) {
  const records = expandedInlines(qmdSource, { path, files });
  if (!records.length) return null;
  const spans = inlineSpans(content);
  if (!spans.size) return null;
  const entries = [];
  for (let i = 0; i < records.length; i += 1) {
    entries.push({
      fingerprint: await inlineFingerprint(records[i]),
      html: spans.get(i) ?? "",
      ...(records[i].main ? {} : { path: records[i].file }),
    });
  }
  return entries;
}

export async function splitResults(html, qmdSource, { path = "main.qmd", files = null } = {}) {
  let found;
  let content;
  try {
    content = contentTokens(tokenize(String(html ?? "")));
    found = findCells(content);
  } catch (error) {
    if (error instanceof ParseError) return { ok: false, reason: "parse" };
    throw error;
  }
  const expected = expandedCells(qmdSource, { path, files });
  if (!found.length || !expected.length) return { ok: false, reason: "no-cells" };
  const tags = found.map(cellTag);
  let htmlCells;
  if (tags.some((tag) => tag !== null)) {
    // Paired by tag. A tag outside the expected range is ignored, and an
    // expected cell with no div has no output.
    const byIndex = new Map();
    found.forEach((tokens, at) => {
      const tag = tags[at];
      if (tag !== null && tag < expected.length && !byIndex.has(tag)) byIndex.set(tag, taggedOutput(tokens));
    });
    htmlCells = expected.map((_, i) => byIndex.get(i) ?? "");
  } else {
    if (found.length !== expected.length) return { ok: false, reason: "count-mismatch" };
    htmlCells = found.map(cellHtml);
  }
  const cells = [];
  for (let i = 0; i < expected.length; i += 1) {
    cells.push({
      fingerprint: await cellFingerprint(expected[i]),
      label: expected[i].label || null,
      html: htmlCells[i],
      ...(expected[i].main ? {} : { path: expected[i].file }),
    });
  }
  // Inline values never fail the split: without them the record is the same as before.
  let inlines = null;
  try {
    inlines = await inlineEntries(content, qmdSource, { path, files });
  } catch {
    inlines = null;
  }
  return { ok: true, record: { version: 1, path, cells, ...(inlines ? { inlines } : {}) } };
}

/// How many of the document's executed cells have no saved output with the
/// same fingerprint: the cells whose code changed since the last run, or that
/// were never run. Saved fingerprints are counted, so a cell copied after the
/// run is stale: each saved output covers only one cell. The Run button shows
/// this count so a reader knows how much of the draft is out of date.
export async function staleCells(cells, qmdSource, { path = "main.qmd", files = null } = {}) {
  const saved = new Map();
  for (const cell of Array.isArray(cells) ? cells : []) saved.set(cell?.fingerprint, (saved.get(cell?.fingerprint) ?? 0) + 1);
  const expected = expandedCells(String(qmdSource ?? ""), { path, files });
  let stale = 0;
  for (const cell of expected) {
    const fingerprint = await cellFingerprint(cell);
    const left = saved.get(fingerprint) ?? 0;
    if (left > 0) saved.set(fingerprint, left - 1);
    else stale += 1;
  }
  return stale;
}
