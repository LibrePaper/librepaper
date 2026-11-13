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

import { cellFingerprint, expandedCells } from "./engines/quarto.js";

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

export async function splitResults(html, qmdSource, { path = "main.qmd", files = null } = {}) {
  let htmlCells;
  try {
    htmlCells = findCells(contentTokens(tokenize(String(html ?? "")))).map(cellHtml);
  } catch (error) {
    if (error instanceof ParseError) return { ok: false, reason: "parse" };
    throw error;
  }
  const expected = expandedCells(qmdSource, { path, files });
  if (!htmlCells.length || !expected.length) return { ok: false, reason: "no-cells" };
  if (htmlCells.length !== expected.length) return { ok: false, reason: "count-mismatch" };
  const cells = [];
  for (let i = 0; i < expected.length; i += 1) {
    cells.push({
      fingerprint: await cellFingerprint(expected[i]),
      label: expected[i].label || null,
      html: htmlCells[i],
      ...(expected[i].main ? {} : { path: expected[i].file }),
    });
  }
  return { ok: true, record: { version: 1, path, cells } };
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
