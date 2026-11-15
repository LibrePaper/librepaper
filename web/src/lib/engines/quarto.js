// Browser side Quarto subset.
//
// A qmd is always kept as source.  This module builds a short lived draft
// representation for the Markdown renderer and records enough structure to
// map durable source annotations without guessing from rendered paragraphs.
// It deliberately does not execute code, filters, shortcodes, or JavaScript.

import { sha256, sha256Bytes } from "../results-hash.js";
import { m } from "../paraglide/messages.js";

export { sha256, sha256Bytes };

const CELL_FENCE = /^ {0,3}(`{3,}|~{3,})\s*(.*?)\s*$/;
const DIV_FENCE = /^ {0,3}(:{3,})(?:(?:\s+|\s*(?=\{))(.+?))?\s*$/;
const INLINE_ENGINE = /^(?:r|python|julia|ojs|bash|embed)(?:\s+.+|\}.*)$/;

function executableInline(line) {
  let start = 0;
  while (start < line.length) {
    const open = line.indexOf("`", start);
    if (open < 0) return false;
    const close = line.indexOf("`", open + 1);
    if (close < 0) return false;
    const segment = line.slice(open + 1, close).trim();
    if (INLINE_ENGINE.test(segment) || /^(?:\{r\}|\{python\}|\{julia\}|\{ojs\}|\{bash\}|\{embed\})\s*\S/.test(segment)) return true;
    start = close + 1;
  }
  return false;
}
const OPTION = /^\s*#\|\s*([^:]+?)\s*:\s*(.*?)\s*$/;

function lineOffsets(source) {
  const offsets = [0];
  for (let i = 0; i < source.length; i += 1) if (source[i] === "\n") offsets.push(i + 1);
  return offsets;
}

function scalar(value) {
  const text = String(value ?? "").trim();
  if (!text) return "";
  if (/^(true|false)$/i.test(text)) return text.toLowerCase() === "true";
  if (/^(null|~)$/i.test(text)) return null;
  if (/^-?\d+(?:\.\d+)?$/.test(text)) return Number(text);
  if ((text.startsWith("\"") && text.endsWith("\"")) || (text.startsWith("'") && text.endsWith("'"))) {
    return text.slice(1, -1).replace(text[0] === '"' ? /\\([\\"nrt])/g : /''/g, (m, c) =>
      text[0] === '"' ? ({ n: "\n", r: "\r", t: "\t", "\\": "\\", '"': '"' }[c] || c) : "'",
    );
  }
  if (text.startsWith("[") && text.endsWith("]")) return text.slice(1, -1).split(",").map((item) => scalar(item));
  if (text.startsWith("{") && text.endsWith("}")) {
    const out = {};
    for (const item of text.slice(1, -1).split(",")) {
      const at = item.indexOf(":");
      if (at > 0) out[item.slice(0, at).trim()] = scalar(item.slice(at + 1));
    }
    return out;
  }
  return text;
}

// Small YAML reader for front matter and #| options.  Unknown YAML is kept as
// a diagnostic; the source itself is never reserialized.
export function parseFrontMatter(source) {
  const lines = String(source || "").split(/\n/);
  if (lines[0]?.replace(/^\uFEFF/, "").trim() !== "---") {
    return { value: {}, raw: "", start: 0, end: 0, diagnostics: [] };
  }
  const close = lines.findIndex((line, index) => index > 0 && /^(---|\.\.\.)\s*$/.test(line.trim()));
  // Keep an unfinished header in the visible draft.  There is no safe
  // metadata boundary yet, so callers must leave the original bytes alone
  // and report the localized diagnostic rather than dropping the document.
  if (close < 0) return { value: {}, raw: "", start: 0, end: 0, diagnostics: [{ severity: "warning", message: m.quarto_yaml_unfinished(), line: 1, column: 1 }] };
  const value = {};
  const diagnostics = [];
  let current = value;
  let currentKey = null;
  let arrayKey = null;
  let arrayItem = null;
  let arrayIndent = -1;
  for (let i = 1; i < close; i += 1) {
    const line = lines[i];
    if (!line.trim() || /^\s*#/.test(line)) continue;
    const list = /^(\s*)-\s*(.*)$/.exec(line);
    if (list && arrayKey) {
      if (!Array.isArray(value[arrayKey])) value[arrayKey] = [];
      const item = list[2].trim();
      const at = item.indexOf(":");
      arrayItem = at > 0 ? { [item.slice(0, at).trim()]: scalar(item.slice(at + 1)) } : null;
      value[arrayKey].push(arrayItem || scalar(item));
      arrayIndent = list[1].length;
      current = arrayItem || value;
      continue;
    }
    const match = /^(\s*)([^:#][^:]*):(?:\s*(.*))?$/.exec(line);
    if (!match) {
      diagnostics.push({ severity: "warning", message: m.quarto_yaml_parse_failed(), line: i + 1, column: 1 });
      continue;
    }
    const indent = match[1].length;
    const key = match[2].trim();
    const rest = match[3] || "";
    if (indent === 0) {
      current = value;
      currentKey = key;
      arrayKey = !rest && /^(authors?|keywords?|bibliography|csl|filters?)$/i.test(key) ? key : null;
      arrayItem = null;
      arrayIndent = -1;
      if (rest) current[key] = scalar(rest);
      else current[key] = arrayKey ? [] : {};
    } else if (arrayItem && indent > arrayIndent && typeof arrayItem === "object" && !Array.isArray(arrayItem)) {
      arrayItem[key] = scalar(rest);
      current = arrayItem;
    } else if (currentKey && typeof value[currentKey] === "object" && !Array.isArray(value[currentKey])) {
      arrayKey = null;
      current = value[currentKey];
      current[key] = scalar(rest);
    } else {
      diagnostics.push({ severity: "warning", message: m.quarto_yaml_nesting(), line: i + 1, column: indent + 1 });
    }
  }
  const offsets = lineOffsets(source);
  const end = offsets[Math.min(close + 1, offsets.length - 1)] ?? source.length;
  return { value, raw: source.slice(0, end), start: 0, end, diagnostics };
}

export function parseAttributes(input) {
  const text = String(input || "").trim().replace(/^\{/, "").replace(/\}$/, "").trim();
  const result = { id: "", classes: [], attributes: {} };
  const tokenRe = /(?:[^\s"']+|"(?:\\.|[^"\\])*"|'(?:[^']|'')*')+/g;
  for (const token of text.match(tokenRe) || []) {
    if (token[0] === "#") result.id = token.slice(1);
    else if (token[0] === ".") result.classes.push(token.slice(1));
    else {
      const at = token.indexOf("=");
      if (at > 0) result.attributes[token.slice(0, at)] = scalar(token.slice(at + 1));
      else if (token) result.classes.push(token);
    }
  }
  return result;
}

function languageOf(info) {
  const value = String(info || "").trim().replace(/^\{/, "").replace(/\}$/, "").trim();
  if (!value) return "";
  return value.split(/\s+/)[0].replace(/^\./, "").replace(/[,{].*$/, "").toLowerCase();
}

function optionValue(value) { return scalar(value); }

function headingOf(line) {
  const match = /^ {0,3}#{1,6}\s+(.+?)\s+\{#([A-Za-z][\w:.-]*)\}\s*$/.exec(line);
  return match ? { text: match[1], id: match[2] } : null;
}

// The engine of an inline span: `r` for `r x`, the braced name for `{python} x`.
function inlineLanguage(segment) {
  const braced = /^\{([A-Za-z][\w-]*)\}/.exec(segment);
  if (braced) return braced[1];
  return /^([A-Za-z][\w-]*)/.exec(segment)?.[1] ?? "";
}

// The code spans of one line, CommonMark style. A run of n backticks opens a
// span that closes at the next run of exactly n backticks; with no closing run
// the opening run is literal text. A backslash before a backtick outside a span
// escapes it. Each span carries `start`/`end` (UTF-16 offsets, backticks
// included) and `n` (the run length).
function codeSpans(line) {
  const spans = [];
  let i = 0;
  while (i < line.length) {
    if (line[i] === "\\" && line[i + 1] === "`") {
      i += 2;
      continue;
    }
    if (line[i] !== "`") {
      i += 1;
      continue;
    }
    let run = i;
    while (line[run] === "`") run += 1;
    const n = run - i;
    let close = -1;
    let at = run;
    while (at < line.length && close < 0) {
      if (line[at] !== "`") {
        at += 1;
        continue;
      }
      let end = at;
      while (line[end] === "`") end += 1;
      if (end - at === n) close = at;
      at = end;
    }
    if (close < 0) {
      i = run;
      continue;
    }
    spans.push({ start: i, end: close + n, n, content: line.slice(run, close) });
    i = close + n;
  }
  return spans;
}

function inlineOccurrence(line, path, lineNumber, occurrence) {
  const matches = [];
  // Only a single-backtick span can hold an inline value. A longer run is
  // literal text, and its content is not scanned for spans of its own.
  for (const span of codeSpans(line)) {
    if (span.n !== 1) continue;
    const whole = line.slice(span.start, span.end);
    if (!executableInline(whole)) continue;
    const expression = span.content.trim();
    matches.push({
      id: `${path}#inline-${lineNumber}-${occurrence + matches.length}`,
      expression,
      language: inlineLanguage(expression),
      line: lineNumber,
      // Rust records the byte column immediately after the opening tick.
      column: new TextEncoder().encode(line.slice(0, span.start + 1)).length + 1,
      // UTF-16 offsets of the whole span, backticks included, within its line.
      start: span.start,
      end: span.end,
      source: line,
    });
  }
  return matches;
}

/** @typedef {{id: string, path: string, label: string, language: string, info: string, options: Record<string, unknown>, code: string, rawCode: string, source: string, sourceStart: number, sourceEnd: number, startLine: number, endLine: number, codeStart: number, codeEnd: number, span: {start: number, end: number}, ambiguous?: boolean, source_sha256?: string, source_ambiguous?: boolean}} QuartoCell */
// The last parse, kept for one call. The sidecar and the Reader's diagnostics
// parse the same main text on every keystroke, so the second call gets the
// first call's result. Callers must treat the result as read-only: every caller
// that asks for the same text and path receives the same object.
let lastParse = null;

/** @param {string} source @param {{path?: string}} [options] */
export function parseQuarto(source, { path = "main.qmd" } = {}) {
  source = String(source ?? "");
  if (lastParse && lastParse.path === path && lastParse.source === source) return lastParse.result;
  const result = parseQuartoUncached(source, { path });
  lastParse = { path, source, result };
  return result;
}

/** @param {string} source @param {{path?: string}} [options] */
function parseQuartoUncached(source, { path = "main.qmd" } = {}) {
  source = String(source ?? "");
  const lines = source.split("\n");
  const offsets = lineOffsets(source);
  const frontMatter = parseFrontMatter(source);
  /** @type {QuartoCell[]} */
  const cells = [];
  const divs = [];
  const inlineExpressions = [];
  const includes = [];
  const inlineRecords = [];
  const headings = [];
  /** @type {{severity: string, message: string, line: number, column: number, cell?: string}[]} */
  const diagnostics = [...frontMatter.diagnostics];
  const stack = [];
  let fence = null;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (fence) {
      const close = new RegExp(`^ {0,3}${fence.char}{${fence.length},}\\s*$`);
      if (close.test(line)) {
        const endLine = i;
        const raw = lines.slice(fence.startLine + 1, endLine);
        const optionLines = [];
        while (raw.length && OPTION.test(raw[0])) optionLines.push(raw.shift());
        /** @type {Record<string, unknown>} */
        const options = {};
        for (const optionLine of optionLines) {
          const match = OPTION.exec(optionLine);
          if (match) options[match[1].trim()] = optionValue(match[2]);
        }
        const rawCode = source.slice(offsets[fence.startLine + 1] ?? source.length, offsets[endLine] ?? source.length);
        const code = raw.join("\n");
        const attributeInfo = fence.info.includes("{") ? fence.info.slice(fence.info.indexOf("{")) : fence.info;
        const parsed = parseAttributes(attributeInfo);
        const infoText = fence.info.replace(/^\{/, "").replace(/\}$/, "").trim();
        const infoMatch = /^[^,\s]+([\s,].*)?$/.exec(infoText);
        let infoAttributes = infoMatch?.[1] || "";
        if (infoAttributes.trimStart().startsWith(",")) infoAttributes = infoAttributes.trimStart().slice(1).replace(/^\s+/, "");
        const infoParsed = parseAttributes(infoAttributes.replace(/,(?=\s*[A-Za-z][\w-]*\s*=)/g, " "));
        const infoOptions = infoParsed.attributes;
        Object.assign(infoOptions, options);
        Object.assign(options, infoOptions);
        const label = String(options.label || infoParsed.id || parsed.id || "");
        const id = label ? `${path}#${label}` : `${path}#cell-${cells.length + 1}`;
        cells.push({
          id, path, label, language: fence.language, info: infoAttributes, options, code, rawCode,
          source: source.slice(offsets[fence.startLine], offsets[endLine + 1] ?? source.length),
          sourceStart: offsets[fence.startLine], sourceEnd: offsets[endLine + 1] ?? source.length,
          startLine: fence.startLine, endLine: endLine,
          codeStart: offsets[fence.startLine + 1] ?? offsets[endLine], codeEnd: offsets[endLine],
          span: { start: offsets[fence.startLine], end: offsets[endLine + 1] ?? source.length },
        });
        fence = null;
      }
      continue;
    }
    const cell = CELL_FENCE.exec(line);
    const braced = cell && String(cell[2]).trim().startsWith("{");
    const rawInfo = cell ? String(cell[2]).trim() : "";
    const innerInfo = rawInfo.replace(/^\{/, "").replace(/\}$/, "").trimStart();
    const hasLanguage = braced && /^[A-Za-z][A-Za-z0-9_-]*(?:\s|,|$)/.test(innerInfo);
    if (hasLanguage) {
      const info = rawInfo.replace(/^\{/, "").replace(/\}$/, "");
      fence = { char: cell[1][0], length: cell[1].length, info, language: languageOf(info), startLine: i };
      continue;
    }
    // Consume every other Markdown fence as opaque. Otherwise a braced fence
    // inside a documentation example would be mistaken for a live cell.
    if (cell) {
      const opener = cell[1];
      const close = new RegExp(`^ {0,3}${opener[0]}{${opener.length},}\\s*$`);
      let end = i + 1;
      while (end < lines.length && !close.test(lines[end])) end += 1;
      i = end < lines.length ? end : lines.length;
      continue;
    }
    if (line.includes("{{") || executableInline(line)) inlineExpressions.push(line.trim());
    // Occurrence numbering is per source line, matching the native parser.
    inlineRecords.push(...inlineOccurrence(line, path, i + 1, 0));
    for (const match of line.matchAll(/\{\{<\s*include\s+([^ >]+).*?>\}\}/g)) includes.push(match[0]);
    const heading = headingOf(line);
    if (heading) headings.push({ ...heading, startLine: i });
    const div = DIV_FENCE.exec(line);
    if (div) {
      const info = String(div[2] || "").trim();
      if (!info) {
        if (stack.length) stack[stack.length - 1].endLine = i;
        stack.pop();
      } else {
        const parsed = parseAttributes(info);
        const entry = { startLine: i, endLine: null, depth: stack.length, fence: div[1], info, ...parsed };
        stack.push(entry);
        divs.push(entry);
      }
    }
  }
  if (fence) diagnostics.push({ severity: "warning", message: m.quarto_cell_unfinished(), line: fence.startLine + 1, column: 1 });
  if (stack.length) diagnostics.push({ severity: "warning", message: m.quarto_div_unfinished(), line: stack[stack.length - 1].startLine + 1, column: 1 });
  const duplicateLabels = new Set();
  const labels = new Set();
  for (const cell of cells) {
    if (!cell.label) continue;
    if (labels.has(cell.label)) duplicateLabels.add(cell.label);
    labels.add(cell.label);
  }
  for (const cell of cells) if (duplicateLabels.has(cell.label)) {
    cell.ambiguous = true;
    diagnostics.push({ severity: "warning", message: m.quarto_cell_label_duplicate({ label: cell.label }), line: cell.startLine + 1, column: 1, cell: cell.id });
  }
  return {
    schema: "librepaper-quarto-source/v1", path, source,
    frontMatter, metadata: frontMatter.value, cells, divs, diagnostics,
    inlineExpressions, inlineRecords, includes, headings,
    spans: { frontMatter: [frontMatter.start, frontMatter.end], cells: cells.map((cell) => cell.span) },
  };
}

// The cells Quarto actually executes into the HTML: those not switched off by
// `include: false`, in the cell's own options or the front matter's
// `execute:` defaults. The shared-results splitter and the draft both count
// cells this way, so they agree on which output belongs to which cell.
export function executedCells(parsed) {
  const meta = parsed.metadata?.execute;
  const defaults = meta && typeof meta === "object" ? meta : {};
  return parsed.cells.filter((cell) => ({ ...defaults, ...cell.options }).include !== false);
}

const MAX_INCLUDE_DEPTH = 8;

// A project path resolved like the renderer's `shortcodes::resolve`: relative
// to the including file's directory, `.` and `..` folded, never absolute and
// never above the project root. Null when the name is not allowed.
function resolveInclude(name, from, lookup) {
  const requested = String(name).replace(/\\/g, "/");
  if (!requested || requested.startsWith("/") || /^[A-Za-z]:/.test(requested) || requested.split("/").some((part) => !part)) return null;
  const parts = from.split("/");
  parts.pop();
  const normalized = [];
  let above = false;
  for (const part of [...parts, ...requested.split("/")]) {
    if (part === ".") continue;
    if (part === "..") { if (!normalized.length) { above = true; break; } normalized.pop(); } else normalized.push(part);
  }
  if (!above) {
    const joined = normalized.join("/");
    const text = lookup(joined);
    if (typeof text === "string") return { path: joined, text };
  }
  const text = lookup(requested);
  return typeof text === "string" ? { path: requested, text } : null;
}

// One pass over the document in document order: the main file and its
// block-level `{{< include >}}` files, expanded like the renderer does. Yields
// the executed cells and the inline values of the prose (front matter and
// fenced code left out). `files` is a function, a Map or an object from
// project path to text. Each record carries `file` and `main`.
function walkDocument(source, { path = "main.qmd", files = null } = {}) {
  const lookup = typeof files === "function" ? files
    : files instanceof Map ? (name) => files.get(name)
    : files && typeof files === "object" ? (name) => (Object.hasOwn(files, name) ? files[name] : undefined)
    : () => undefined;
  const mainParsed = parseQuarto(source, { path });
  const meta = mainParsed.metadata?.execute;
  const defaults = meta && typeof meta === "object" ? meta : {};
  const cells = [];
  const inlines = [];
  const walk = (text, file, chain, isMain) => {
    text = String(text ?? "");
    const parsed = isMain ? mainParsed : parseQuarto(text, { path: file });
    const lines = text.split("\n");
    const front = parsed.frontMatter.end ? text.slice(0, parsed.frontMatter.end).split("\n").length - 1 : 0;
    const cellAt = new Map(parsed.cells.map((cell) => [cell.startLine, cell]));
    let fence = null;
    for (let i = front; i < lines.length; i += 1) {
      const line = lines[i].replace(/\r$/, "");
      if (fence) {
        if (new RegExp(`^ {0,3}${fence.char}{${fence.length},}\\s*$`).test(line)) fence = null;
        continue;
      }
      const open = /^ {0,3}(`{3,}|~{3,})/.exec(line);
      if (open) {
        const cell = cellAt.get(i);
        if (cell && ({ ...defaults, ...cell.options }).include !== false) cells.push({ ...cell, file, main: isMain });
        fence = { char: open[1][0], length: open[1].length };
        continue;
      }
      for (const record of inlineOccurrence(line, file, i + 1, 0)) inlines.push({ ...record, file, main: isMain });
      const include = /^\s*\{\{<\s*include\s+([^\s>]+)\s*>\}\}\s*$/.exec(line);
      if (!include) continue;
      const found = resolveInclude(include[1], file, lookup);
      if (!found || chain.length >= MAX_INCLUDE_DEPTH || chain.includes(found.path)) continue;
      walk(found.text, found.path, [...chain, found.path], false);
    }
  };
  walk(source, path, [], true);
  return { cells, inlines };
}

// The executed cells of the whole document in document order. Each cell
// carries `file` (its normalized path) and `main`.
export function expandedCells(source, options = {}) {
  return walkDocument(source, options).cells;
}

// The inline values of the whole document in document order, in the same walk
// as expandedCells. Each record carries `file`, `main`, `language`, `line`,
// `column`, `expression`, and `start`/`end` (UTF-16 offsets of the span in its line).
export function expandedInlines(source, options = {}) {
  return walkDocument(source, options).inlines;
}

// Identity of an inline value: its engine and expression as the record holds them.
export async function inlineFingerprint(record) {
  return sha256(`${record.language}\n${record.expression}`);
}

// Pair saved outputs with the cells of the document. A result whose
// fingerprint equals the cell's is fresh; others fall back to the same label,
// then to the same position when the cell counts agree, and are stale.
export function matchResults(results, cells) {
  const matched = new Map();
  const saved = Array.isArray(results?.cells) ? results.cells : [];
  if (!saved.length) return matched;
  const used = new Set();
  const queues = new Map();
  saved.forEach((entry, index) => {
    if (!queues.has(entry.fingerprint)) queues.set(entry.fingerprint, []);
    queues.get(entry.fingerprint).push(index);
  });
  for (const cell of cells) {
    const queue = cell.source_sha256 ? queues.get(cell.source_sha256) : null;
    if (!queue?.length) continue;
    const index = queue.shift();
    used.add(index);
    matched.set(cell, { html: saved[index].html, stale: false });
  }
  cells.forEach((cell, position) => {
    if (matched.has(cell)) return;
    let index = cell.label ? saved.findIndex((entry, at) => !used.has(at) && entry.label === cell.label) : -1;
    if (index < 0 && saved.length === cells.length && !used.has(position)) index = position;
    if (index < 0) return;
    used.add(index);
    matched.set(cell, { html: saved[index].html, stale: true });
  });
  return matched;
}


/// The file the Markdown module reads beside a .qmd: the saved outputs placed
/// by the line of their cell's opening fence, the results note, and the
/// interface strings the page needs. The matching of saved results to cells
/// stays here, in the browser; the module only places what it is given.
export const QUARTO_SIDECAR = ".librepaper/quarto.json";

// Pair saved inline values with the document's inline values, in order, within
// each file: the Nth current occurrence of a fingerprint in a file takes the Nth
// saved entry for that file and fingerprint. Every saved entry takes a slot,
// even an empty one, so a silent occurrence keeps its place; only the emitted
// entry is skipped when the consumed html is empty. The main file's key is "".
async function matchInlines(records, saved) {
  const keyOf = (file, fingerprint) => JSON.stringify([file, fingerprint]);
  const queues = new Map();
  for (const entry of Array.isArray(saved) ? saved : []) {
    if (typeof entry?.fingerprint !== "string") continue;
    const key = keyOf(entry.path ? String(entry.path) : "", entry.fingerprint);
    if (!queues.has(key)) queues.set(key, []);
    queues.get(key).push(String(entry.html ?? ""));
  }
  const out = [];
  for (const record of records) {
    const queue = queues.get(keyOf(record.main ? "" : record.file, await inlineFingerprint(record)));
    if (!queue?.length) continue;
    const html = queue.shift();
    if (!html) continue;
    const entry = { line: record.line, column: record.column, html: html.replace(/\r?\n/g, " "), stale: false };
    if (!record.main) entry.path = record.file;
    out.push(entry);
  }
  return out;
}

/** @param {string} source @param {{path?: string, results?: any, strings?: Record<string, string>, files?: any}} [options] */
export async function quartoSidecar(source, { path = "main.qmd", results = null, strings, files = null } = {}) {
  const cells = expandedCells(source, { path, files });
  const hashes = await Promise.all(cells.map((cell) => cellFingerprint(cell)));
  cells.forEach((cell, index) => { cell.source_sha256 = hashes[index]; });
  const matched = matchResults(results, cells);
  const ordered = [];
  for (const [cell, output] of matched) {
    // An empty output is a silent cell: nothing is placed under it.
    if (!String(output.html ?? "")) continue;
    const entry = { line: cell.startLine + 1, html: String(output.html ?? "").replace(/\r?\n/g, " "), stale: output.stale };
    if (!cell.main) entry.path = cell.file;
    ordered.push({ entry, order: cells.indexOf(cell) });
  }
  const outputs = ordered.sort((a, b) => a.order - b.order).map((item) => item.entry);
  const inlines = await matchInlines(expandedInlines(source, { path, files }), results?.inlines);
  return {
    outputs,
    ...(inlines.length ? { inlines } : {}),
    note: typeof results?.note === "string" ? results.note.trim() : "",
    strings: strings ?? {
      output_stale: m.quarto_output_stale(),
      include_unavailable: m.quarto_include_unavailable({ name: "{name}" }),
      include_cycle: m.quarto_include_cycle({ normalized: "{normalized}" }),
      orcid: m.quarto_orcid_label({ name: "{name}" }),
      corresponding: m.quarto_corresponding_author(),
      reuse: m.quarto_reuse(),
      copyright: m.quarto_copyright(),
      view_license: m.quarto_view_license(),
      abstract: m.insert_abstract(),
    },
  };
}

/// Diagnostics whose message is a catalogue key (`quarto_*`) are translated
/// with the arguments the module put in `hints` as `name=value`.
export function translateQuartoDiagnostics(diagnostics) {
  return (diagnostics || []).map((diagnostic) => {
    const key = String(diagnostic?.message || "");
    if (!/^quarto_[a-z_]+$/.test(key) || typeof m[key] !== "function") return diagnostic;
    const args = {};
    for (const hint of diagnostic.hints || []) {
      const at = String(hint).indexOf("=");
      if (at > 0) args[hint.slice(0, at)] = hint.slice(at + 1);
    }
    return { ...diagnostic, message: m[key](args), hints: [] };
  });
}

/// Diagnostics of a draft that went through another module, put back on the
/// lines of the .qmd. A generated line has no source line and is left
/// unplaced rather than attached to nearby prose.
export function mapDraftDiagnostics(diagnostics, lineMap, sourcePath) {
  return (diagnostics || []).map((diagnostic) => {
    const line = Number(diagnostic?.line || 0);
    const mapped = line > 0 ? lineMap[line - 1] : null;
    if (mapped == null) return { ...diagnostic, file: sourcePath, line: 0, column: 0, end_line: 0, end_column: 0, generated: true };
    const endMapped = lineMap[Number(diagnostic?.end_line || line) - 1];
    return { ...diagnostic, file: sourcePath, line: mapped, end_line: endMapped == null ? mapped : endMapped, generated: false };
  });
}

// Keep parameter identity byte-for-byte aligned with Rust. Valid parameter
// names are ASCII, so code-unit ordering is the same as BTreeMap ordering.
// Numbers use IEEE-754 bits because JavaScript and serde_json choose different
// decimal spellings around exponent boundaries.
function parameterMaterial(parameters = {}) {
  const encoder = new TextEncoder();
  const byteLength = (value) => encoder.encode(value).byteLength;
  const parts = ["librepaper-quarto-parameters-v1\0"];
  const append = (value) => parts.push(`${byteLength(value)}:${value}`);
  for (const key of Object.keys(parameters).sort()) {
    append(key);
    const value = parameters[key];
    if (value === null) {
      parts.push("n");
      parts.push("0:");
    } else if (typeof value === "boolean") {
      parts.push("b");
      parts.push(value ? "1:1" : "1:0");
    } else if (typeof value === "number") {
      const view = new DataView(new ArrayBuffer(8));
      view.setFloat64(0, value, false);
      let bits = "";
      for (let index = 0; index < 8; index++) bits += view.getUint8(index).toString(16).padStart(2, "0");
      parts.push("d");
      append(bits);
    } else {
      parts.push("s");
      append(value);
    }
    parts.push("\0");
  }
  return encoder.encode(parts.join(""));
}

export async function parameterSha256(parameters = {}) {
  return sha256Bytes(parameterMaterial(parameters));
}

export async function cellFingerprint(cell) {
  // Keep this byte protocol aligned with the native collector: language, the
  // raw info attributes, and the exact body separated by NUL bytes.
  return sha256(`${cell.language}\0${String(cell.info || "")}\0${cell.rawCode ?? cell.code ?? ""}`);
}

export async function contextFingerprint(parsed, { main = parsed.path, format = "html", profiles = [], parameters = {}, parametersSha256 = null, includes = parsed.includes || [], dependencies = [] } = {}) {
  const encoder = new TextEncoder();
  const chunks = [encoder.encode(`librepaper-quarto-context-v1\0${main}\0${format}\0`)];
  for (const profile of profiles) chunks.push(encoder.encode(`${profile}\0`));
  if (parametersSha256) chunks.push(encoder.encode(String(parametersSha256)));
  else if (parameters && Object.keys(parameters).length) {
    chunks.push(encoder.encode(await parameterSha256(parameters)));
  }
  if (parsed.frontMatter?.raw) chunks.push(encoder.encode("\u00fe"), encoder.encode(parsed.frontMatter.raw));
  for (const include of includes) chunks.push(encoder.encode("\u00fd"), encoder.encode(include));
  for (const expression of parsed.inlineExpressions || []) chunks.push(encoder.encode("\u00fc"), encoder.encode(expression));
  for (const dependency of dependencies) chunks.push(encoder.encode("\u00fb"), encoder.encode(dependency));
  const unlabelledOccurrences = new Map();
  for (const cell of parsed.cells) {
    const rawCode = cell.rawCode ?? cell.code ?? "";
    const sourceDigest = await sha256(`${cell.language}\0${String(cell.info || "")}\0${rawCode}`);
    let id = cell.id;
    if (!cell.label) {
      const base = `${cell.path || parsed.path}#cell-${sourceDigest.slice(0, 16)}`;
      const occurrence = unlabelledOccurrences.get(sourceDigest) || 0;
      unlabelledOccurrences.set(sourceDigest, occurrence + 1);
      id = `${base}-${occurrence + 1}`;
    }
    chunks.push(encoder.encode(`${id}\0${cell.language}\0${String(cell.info || "")}\0${rawCode}`), encoder.encode("\u00ff"));
  }
  const bytes = new Uint8Array(chunks.reduce((size, chunk) => size + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return sha256Bytes(bytes);
}

// Bundle selection is a presentation choice. Keep its key stable while prose
// or computational cells change so a new render can replace the selected
// result for the same target/profile/parameter context.
export async function contextId({ format = "html", profiles = [], parameters = {} } = {}) {
  const parametersSha256 = await parameterSha256(parameters);
  const material = `librepaper-quarto-selection-v1\0${format}\0${profiles.map(String).join("\0")}\0${parametersSha256}`;
  const digest = await sha256(material);
  return `ctx-${digest.slice(0, 16)}`;
}

