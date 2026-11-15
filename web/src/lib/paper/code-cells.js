// The code cells of a Quarto or Markdown source, for visual mode. A chunk that
// the renderer draws as nothing (echo: false, include: false) would fall
// through to its raw fence, and a chunk with echo on is drawn as a plain code
// block. Either way the reader sees source with `#|` option lines. So every
// executable chunk is drawn here as a card: a caption bar (language, label, and
// a note when the code is not in the document) over the code, without options
// and never folded.

import { parseQuarto } from "../engines/quarto.js";
import { m } from "../paraglide/messages.js";

/** @typedef {{ from: number, to: number, kind: string, render: boolean, html?: string }} PaperBlock */

// The start offset of every line, counting "\n" breaks as parseQuarto does.
function lineOffsets(text) {
  const offsets = [0];
  for (let i = 0; i < text.length; i += 1) if (text[i] === "\n") offsets.push(i + 1);
  return offsets;
}

function escapeHtml(value) {
  return String(value ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

// The execute defaults of the front matter, as expandedCells reads them.
function defaultsOf(parsed) {
  const meta = parsed.metadata?.execute;
  return meta && typeof meta === "object" ? meta : {};
}

// The chunk's range on whole lines: from its fence line to the end of its closing fence line.
function rangeOf(text, offsets, cell) {
  const from = offsets[cell.startLine];
  const lineStart = offsets[cell.endLine];
  if (from === undefined || lineStart === undefined) return null;
  let to = text.indexOf("\n", lineStart);
  if (to < 0) to = text.length;
  else if (text[to - 1] === "\r") to -= 1;
  return { from, to };
}

// The language as the opening fence writes it, so `{R}` shows as `R`.
function languageOf(text, from, fallback) {
  const end = text.indexOf("\n", from);
  const opener = text.slice(from, end < 0 ? text.length : end);
  const match = /^\s*(?:`{3,}|~{3,})\s*\{\s*([A-Za-z][A-Za-z0-9_-]*)/.exec(opener);
  return match ? match[1] : fallback;
}

// Why the code is not in the document, by the same precedence the document uses.
function noteOf(options) {
  if (options.include === false) return m.codecells_not_in_output();
  if (options.echo === false) return m.codecells_code_hidden();
  if (options.eval === false) return m.codecells_not_run();
  return null;
}

// The fence for the code: longer than any backtick run in it, so the code cannot close it.
function fenceFor(code) {
  const longest = Math.max(0, ...(code.match(/`+/g) || []).map((run) => run.length));
  return "`".repeat(Math.max(3, longest + 1));
}

// The card for one chunk: its caption bar, then the code drawn through the renderer.
function cardHtml(cell, language, defaults, renderer) {
  const options = { ...defaults, ...cell.options };
  const note = noteOf(options);
  const quiet = note !== null ? " lp-paper-cell-quiet" : "";
  let caption = `<span class="lp-paper-cell-lang">${escapeHtml(language)}</span>`;
  if (cell.label) caption += `<span class="lp-paper-cell-label">${escapeHtml(cell.label)}</span>`;
  if (note !== null) caption += `<span class="lp-paper-cell-note">${escapeHtml(note)}</span>`;
  const code = String(cell.code ?? "");
  const body = code
    ? renderer.render(`${fenceFor(code)}${language}\n${code}\n${fenceFor(code)}\n`)
    : "<pre><code></code></pre>";
  return `<div class="code-filename lp-paper-cell-head${quiet}">${caption}</div>${body}`;
}

/**
 * The blocks of a Quarto or Markdown source with every executable chunk drawn
 * as a code card. A chunk's blocks are replaced by one CodeCell block; a chunk
 * that a block straddles is left as it was, and so is a chunk that fails.
 * @param {string} text
 * @param {PaperBlock[] | null} blocks the blocks the module renders, or null
 * @param {{ render: (markdown: string) => string }} renderer
 * @param {string} path the main file's project path
 * @returns {PaperBlock[] | null}
 */
export function withCodeCells(text, blocks, renderer, path) {
  if (!blocks) return null;
  let parsed;
  try {
    parsed = parseQuarto(text, { path });
  } catch {
    return blocks;
  }
  const offsets = lineOffsets(text);
  const defaults = defaultsOf(parsed);
  const replaced = new Set();
  const cards = [];
  for (const cell of parsed.cells) {
    try {
      if (!cell.language) continue;
      const range = rangeOf(text, offsets, cell);
      if (!range) continue;
      const touching = blocks.filter((block) => block.from <= range.to && block.to >= range.from);
      const inside = touching.filter((block) => block.from >= range.from && block.to <= range.to);
      // A block that runs past the chunk's edge belongs to the chunk's neighbour; this chunk is left alone.
      if (inside.length !== touching.length) continue;
      const language = languageOf(text, range.from, cell.language);
      const html = cardHtml(cell, language, defaults, renderer);
      for (const block of inside) replaced.add(block);
      cards.push({ kind: "CodeCell", from: range.from, to: range.to, render: true, html });
    } catch {
      // One bad chunk leaves that chunk as it was.
    }
  }
  if (!cards.length) return blocks;
  return [...blocks.filter((block) => !replaced.has(block)), ...cards].sort((a, b) => a.from - b.from);
}
