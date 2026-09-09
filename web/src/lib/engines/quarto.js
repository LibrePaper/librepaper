// Browser side Quarto subset.
//
// A qmd is always kept as source.  This module builds a short lived draft
// representation for the Markdown renderer and records enough structure to
// associate saved display outputs without guessing from rendered paragraphs.
// It deliberately does not execute code, filters, shortcodes, or JavaScript.

import { sha256, sha256Bytes } from "../results-hash.js";
import { escapeHtml, safeFragment } from "../results-content.js";

export { sha256, sha256Bytes };

const CELL_FENCE = /^ {0,3}(`{3,}|~{3,})\s*(.*?)\s*$/;
const DIV_FENCE = /^ {0,3}(:{3,})(?:(?:\s+|\s*(?=\{))(.+?))?\s*$/;
const INCLUDE = /\{\{<\s*include\s+([^ >]+).*?>\}\}/g;
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

export const QUARTO_SCHEMA = "librepaper-quarto-bundle/v1";

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
  if (close < 0) return { value: {}, raw: "", start: 0, end: 0, diagnostics: [{ severity: "warning", message: "unfinished YAML front matter", line: 1, column: 1 }] };
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
    if (list && arrayKey && list[1].length > 0) {
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
      diagnostics.push({ severity: "warning", message: "could not parse YAML front matter", line: i + 1, column: 1 });
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
      diagnostics.push({ severity: "warning", message: "unsupported YAML nesting", line: i + 1, column: indent + 1 });
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

function inlineOccurrence(line, path, lineNumber, occurrence) {
  const matches = [];
  const pattern = /`([^`\n]+)`/g;
  for (const match of line.matchAll(pattern)) {
    if (!executableInline(match[0])) continue;
    matches.push({
      id: `${path}#inline-${lineNumber}-${occurrence + matches.length}`,
      expression: match[1].trim(),
      line: lineNumber,
      // Rust records the byte column immediately after the opening tick.
      column: new TextEncoder().encode(line.slice(0, match.index + 1)).length + 1,
      source: line,
    });
  }
  return matches;
}

export function parseQuarto(source, { path = "main.qmd" } = {}) {
  source = String(source ?? "");
  const lines = source.split("\n");
  const offsets = lineOffsets(source);
  const frontMatter = parseFrontMatter(source);
  const cells = [];
  const divs = [];
  const inlineExpressions = [];
  const includes = [];
  const inlineRecords = [];
  const headings = [];
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
  if (fence) diagnostics.push({ severity: "warning", message: "unfinished code cell", line: fence.startLine + 1, column: 1 });
  if (stack.length) diagnostics.push({ severity: "warning", message: "unfinished fenced div", line: stack[stack.length - 1].startLine + 1, column: 1 });
  const duplicateLabels = new Set();
  const labels = new Set();
  for (const cell of cells) {
    if (!cell.label) continue;
    if (labels.has(cell.label)) duplicateLabels.add(cell.label);
    labels.add(cell.label);
  }
  for (const cell of cells) if (duplicateLabels.has(cell.label)) {
    cell.ambiguous = true;
    diagnostics.push({ severity: "warning", message: `duplicate cell label ${cell.label}`, line: cell.startLine + 1, column: 1, cell: cell.id });
  }
  return {
    schema: "librepaper-quarto-source/v1", path, source,
    frontMatter, metadata: frontMatter.value, cells, divs, diagnostics,
    inlineExpressions, inlineRecords, includes, headings,
    spans: { frontMatter: [frontMatter.start, frontMatter.end], cells: cells.map((cell) => cell.span) },
  };
}

function outputFor(bundle, cell) {
  if (!bundle) return null;
  if (cell.ambiguous || cell.source_ambiguous) return null;
  const entries = Array.isArray(bundle.cells) ? bundle.cells : [];
  if (!cell.label) {
    // Ordinals are locations, not identity. An inserted cell must never
    // acquire the old occupant's plot merely by taking its position.
    if (!cell.source_sha256 || cell.source_ambiguous) return null;
    const candidates = entries.filter((entry) => entry.source_path === cell.path && entry.source_sha256 === cell.source_sha256);
    return candidates.length === 1 && !candidates[0].ambiguous && candidates[0].coverage !== "ambiguous" ? candidates[0] : null;
  }
  return entries.find((entry) => entry.id === cell.id && !entry.ambiguous && entry.coverage !== "ambiguous") ||
    (cell.label && entries.find((entry) => entry.label === cell.label && entry.source_path === cell.path && !entry.ambiguous && entry.coverage !== "ambiguous")) || null;
}

function outputMarkup(output, assets = {}, cell = null, references = new Map()) {
  if (!output || output.coverage === "hidden" || output.coverage === "unavailable") return "";
  const outputs = Array.isArray(output.outputs) ? output.outputs : [output];
  return outputs.map((item, index) => {
    if (item.kind === "image" || /^image\//.test(item.mime || "")) {
      const url = item.url || assets[item.asset || item.path] || "";
      if (!url) return `<div class="quarto-output-missing">Saved image is unavailable.</div>`;
      const label = index === 0 && cell?.label && /^(?:fig|tbl)-/.test(cell.label) ? ` id="${escapeHtml(cell.label)}"` : "";
      const caption = (outputs.length === 1 && (cell?.options?.["fig-cap"] || cell?.options?.["tbl-cap"])) || item.caption || "";
      const target = cell?.label ? references.get(cell.label) : null;
      const number = target && (target.kind === "fig" || target.kind === "tbl") ? `<span class="quarto-figure-number">${target.kind === "fig" ? "Figure" : "Table"} ${target.number}.</span> ` : "";
      return `<figure class="quarto-cached-output"${label} data-librepaper-generated="quarto"><img src="${escapeHtml(url)}" alt="${escapeHtml(item.alt || cell?.options?.["fig-alt"] || "")}">${caption ? `<figcaption>${number}${escapeHtml(caption)}</figcaption>` : number ? `<figcaption>${number.trim()}</figcaption>` : ""}</figure>`;
    }
    if (item.kind === "table") {
      const label = index === 0 && cell?.label && /^tbl-/.test(cell.label) ? ` id="${escapeHtml(cell.label)}"` : "";
      const html = safeFragment(item.html || item.text || "");
      const caption = (outputs.length === 1 && cell?.options?.["tbl-cap"]) || item.caption || "";
      const target = cell?.label ? references.get(cell.label) : null;
      const number = target?.kind === "tbl" ? `<span class="quarto-figure-number">Table ${target.number}.</span> ` : "";
      return `<div class="quarto-cached-table"${label} data-librepaper-generated="quarto">${html}${caption || number ? `<div class="quarto-table-caption">${number}${escapeHtml(caption)}</div>` : ""}</div>`;
    }
    if (item.kind === "html") return `<div data-librepaper-generated="quarto">${safeFragment(item.html || item.text || "")}</div>`;
    return `<div class="quarto-cached-text" data-librepaper-generated="quarto"><pre><code>${escapeHtml(item.text ?? item.value ?? "")}</code></pre></div>`;
  }).filter(Boolean).join("\n");
}

function htmlClass(value) {
  return String(value || "").split(/\s+/).filter((item) => /^[A-Za-z][\w-]*$/.test(item)).join(" ");
}

function htmlId(value) {
  const text = String(value || "");
  return /^[A-Za-z][\w:.-]*$/.test(text) ? text : "";
}

function referenceKind(id) {
  const match = /^(fig|tbl|sec|eq|lst)-/.exec(String(id || "").split("#").pop());
  return match?.[1] || "";
}

function referencesFor(parsed) {
  const targets = [
    ...(parsed.headings || []),
    ...(parsed.divs || []),
    ...(parsed.cells || []).filter((cell) => cell.label),
  ].filter((item) => item?.id && referenceKind(item.id));
  targets.sort((left, right) => left.startLine - right.startLine);
  const counts = new Map();
  const references = new Map();
  for (const target of targets) {
    const id = target.id;
    if (references.has(id)) continue;
    const kind = referenceKind(id);
    const number = (counts.get(kind) || 0) + 1;
    counts.set(kind, number);
    references.set(id, { id, kind, number, startLine: target.startLine });
    // Cell labels are stored without the source path, while div and heading
    // IDs are already document-local. Resolve both spellings for callers.
    const short = id.split("#").pop();
    if (short && !references.has(short)) references.set(short, { id, kind, number, startLine: target.startLine });
  }
  return references;
}

function divMarkup(info, closing = false, references = new Map()) {
  if (closing) return "</div>";
  const attributes = parseAttributes(info);
  const classes = htmlClass(attributes.classes.join(" "));
  const id = htmlId(attributes.id);
  const callout = attributes.classes.find((item) => /^callout-/.test(item));
  const label = callout ? String(attributes.attributes.title || callout.slice("callout-".length)).replace(/^[a-z]/, (c) => c.toUpperCase()) : "";
  const heading = label ? `<div class="quarto-callout-title">${escapeHtml(label)}</div>` : "";
  const semantic = attributes.classes.includes("columns") ? "quarto-columns"
    : attributes.classes.includes("column") ? "quarto-column"
      : attributes.classes.includes("panel-tabset") ? "quarto-tabset" : "";
  const target = id ? references.get(id) : null;
  const number = target && (target.kind === "fig" || target.kind === "tbl")
    ? `<span class="quarto-figure-number">${target.kind === "fig" ? "Figure" : "Table"} ${target.number}.</span> ` : "";
  const className = htmlClass([classes, "quarto-div", semantic, callout ? "quarto-callout" : ""].filter(Boolean).join(" "));
  const layout = attributes.classes.includes("panel-tabset")
    ? ` data-quarto-tabset="static" role="group" aria-label="Tabset sections"`
    : attributes.classes.includes("columns")
      ? ` data-quarto-layout="columns" role="group" aria-label="Columns" style="display:flex;flex-wrap:wrap;gap:1.5rem"`
      : attributes.classes.includes("column") ? ` style="flex:1 1 16rem;min-width:0"`
        : callout ? ` role="note" style="border-inline-start:.25rem solid currentColor;padding:.5rem 1rem;margin-block:1rem"` : "";
  return `<div${id ? ` id="${escapeHtml(id)}"` : ""}${className ? ` class="${escapeHtml(className)}"` : ""}${layout}>${heading}${number}`;
}

function metadataMarkup(parsed) {
  const meta = parsed.metadata || {};
  const title = meta.title == null ? "" : String(meta.title).trim();
  const authors = Array.isArray(meta.author) ? meta.author : Array.isArray(meta.authors) ? meta.authors : meta.author ? [meta.author] : meta.authors ? [meta.authors] : [];
  const names = authors.map((author) => typeof author === "string" ? author : author?.name || author?.literal || "").filter(Boolean);
  const date = meta.date == null ? "" : String(meta.date).trim();
  const abstract = meta.abstract == null ? "" : String(meta.abstract).trim();
  if (!title && !names.length && !date && !abstract) return "";
  const parts = [`<header class="quarto-title-block" data-librepaper-generated="quarto">`];
  if (title) parts.push(`<h1>${escapeHtml(title)}</h1>`);
  if (names.length) parts.push(`<p class="quarto-author">${escapeHtml(names.join(", "))}</p>`);
  if (date) parts.push(`<p class="quarto-date">${escapeHtml(date)}</p>`);
  if (abstract) parts.push(`<div class="quarto-abstract"><strong>Abstract</strong><p>${escapeHtml(abstract)}</p></div>`);
  parts.push("</header>");
  return parts.join("");
}

function inlineValueFor(values, record, currentContext = "") {
  if (!values || !record) return null;
  const candidate = Array.isArray(values)
    ? values.find((item) => item?.id === record.id)
    : values[record.id];
  if (!candidate || typeof candidate !== "object" || candidate.expression !== record.expression || Number(candidate.line) !== record.line) return null;
  if (candidate.source_path != null && candidate.source_path !== record.id.split("#", 1)[0]) return null;
  if (candidate.column != null && Number(candidate.column) !== record.column) return null;
  if (!currentContext || candidate.context_sha256 !== currentContext) return null;
  const value = candidate.text ?? candidate.value;
  return value == null ? null : String(value);
}

function proseLine(line, labels, { lineNumber = 0, inlineRecords = [], inlineValues = null, currentContext = "" } = {}) {
  // Keep inline code and links opaque while making the common Quarto cross
  // references readable.  An unresolved reference remains source text.
  const inlineOrdinals = new Map();
  const chunks = String(line).split(/(`[^`]*`|!?\[[^\]]*\]\([^)]*\))/g);
  return chunks.map((chunk, index) => {
    if (index % 2) {
      const expression = chunk.slice(1, -1).trim();
      const ordinal = inlineOrdinals.get(expression) || 0;
      inlineOrdinals.set(expression, ordinal + 1);
      const record = inlineRecords.filter((item) => item.line === lineNumber && item.expression === expression)[ordinal];
      const value = inlineValueFor(inlineValues, record, currentContext);
      return value == null ? chunk : `<span class="quarto-inline-value" data-inline-id="${escapeHtml(record.id)}" title="Captured inline result" aria-label="Captured inline result from saved computation">${escapeHtml(value)}</span>`;
    }
    return chunk.replace(/@(fig|tbl|sec|eq|lst)-([A-Za-z0-9_:-]+(?:\.[A-Za-z0-9_:-]+)*)/g, (whole, kind, label) => {
      const id = `${kind}-${label}`;
      const target = labels instanceof Map ? labels.get(id) : null;
      if (!target && !(labels instanceof Set && labels.has(id))) return whole;
      const noun = kind === "fig" ? "Figure" : kind === "tbl" ? "Table" : kind === "sec" ? "Section" : kind === "eq" ? "Equation" : "Listing";
      return `<a class="quarto-crossref" href="#${escapeHtml(id)}">${noun} ${target?.number || escapeHtml(label)}</a>`;
    }).replace(/\[([^\]]+)\]\{#([A-Za-z][\w:.-]*)\}/g, (_, text, id) => `<span id="${escapeHtml(id)}">${text}</span>`);
  }).join("");
}

export function composeDraft(source, { path = "main.qmd", bundle = null, assets = {}, expandIncludes: includes = {}, maxIncludeDepth = 8, cellFingerprints = {}, inlineValues = null, currentContext = "" } = {}) {
  const parsed = parseQuarto(source, { path });
  for (const cell of parsed.cells) {
    cell.source_sha256 = cellFingerprints[cell.id]?.sha256 || "";
    cell.source_ambiguous = cell.label ? false : (cellFingerprints[cell.id]?.ambiguous ?? true);
  }
  const lines = source.split("\n");
  const out = [];
  const lineMap = [];
  const generatedDiagnostics = [];
  const references = referencesFor(parsed);
  const push = (text, sourceLine = null) => {
    const value = String(text ?? "");
    const parts = value.split("\n");
    for (let index = 0; index < parts.length; index += 1) {
      out.push(parts[index]);
      lineMap.push(sourceLine == null ? null : sourceLine + index);
    }
  };
  let opaqueFence = null;
  const byStart = new Map(parsed.cells.map((cell) => [cell.startLine, cell]));
  const labels = new Set([
    ...parsed.cells.map((cell) => cell.label),
    ...parsed.divs.map((div) => div.id),
  ].filter(Boolean));
  const frontMatterText = parsed.frontMatter.end ? source.slice(0, parsed.frontMatter.end) : "";
  const frontMatterLines = frontMatterText ? frontMatterText.split("\n").length - (frontMatterText.endsWith("\n") ? 1 : 0) : 0;
  const defaults = parsed.metadata.execute && typeof parsed.metadata.execute === "object" ? parsed.metadata.execute : {};
  const resolveInclude = (name, sourceName) => {
    const requested = String(name || "").replaceAll("\\", "/");
    if (!requested || requested.startsWith("/") || /^[A-Za-z]:/.test(requested) || requested.split("/").some((part) => !part)) return null;
    const base = String(sourceName || path).split("/");
    base.pop();
    const parts = [...base, ...requested.split("/")];
    const normalizedParts = [];
    for (const part of parts) {
      if (part === ".") continue;
      if (part === "..") {
        if (!normalizedParts.length) return null;
        normalizedParts.pop();
      } else normalizedParts.push(part);
    }
    const normalized = normalizedParts.join("/");
    if (Object.prototype.hasOwnProperty.call(includes, normalized)) return normalized;
    if (Object.prototype.hasOwnProperty.call(includes, requested)) return requested;
    return null;
  };
  const expandedInclude = (name, depth = 0, seen = new Set(), sourceName = path, sourceLine = 0) => {
    const normalized = resolveInclude(name, sourceName);
    if (!normalized) {
      generatedDiagnostics.push({ severity: "warning", generated: true, message: `Include unavailable or unauthorized: ${name}`, file: sourceName, line: sourceLine, column: 0 });
      return `<span class="quarto-diagnostic">Include unavailable: ${escapeHtml(name)}</span>`;
    }
    if (depth >= maxIncludeDepth || seen.has(normalized)) {
      generatedDiagnostics.push({ severity: "warning", generated: true, message: `Include cycle or depth limit: ${normalized}`, file: sourceName, line: sourceLine, column: 0 });
      return `<span class="quarto-diagnostic">Include cycle or depth limit: ${escapeHtml(normalized)}</span>`;
    }
    const value = includes[normalized];
    const next = new Set(seen).add(normalized);
    const includedLines = String(value).split("\n");
    const includedParsed = parseQuarto(value, {path:normalized});
    const includedDefaults = {...defaults, ...(includedParsed.metadata.execute || {})};
    for (const cell of includedParsed.cells) {
      const visibility = {...includedDefaults, ...cell.options};
      if (visibility.include === false || visibility.echo === false) {
        for (let line = cell.startLine; line <= cell.endLine; line++) includedLines[line] = "";
      }
    }
    let includedFence = null;
    return includedLines.map((includedLine) => {
      if (includedFence) {
        const close = new RegExp(`^ {0,3}${includedFence.char}{${includedFence.length},}\\s*$`);
        if (close.test(includedLine)) includedFence = null;
        return includedLine;
      }
      const opener = /^ {0,3}(`{3,}|~{3,})/.exec(includedLine);
      if (opener) includedFence = { char: opener[1][0], length: opener[1].length };
      return includedLine.replace(INCLUDE, (_, child) => expandedInclude(child, depth + 1, next, normalized, 0));
    }).join("\n");
  };
  for (let i = 0; i < lines.length; i += 1) {
    if (i < frontMatterLines) {
      if (i === frontMatterLines - 1) {
        const header = metadataMarkup(parsed);
        if (header) push(header);
      }
      continue;
    }
    const cell = byStart.get(i);
    if (!cell && opaqueFence) {
      push(lines[i], i);
      if (new RegExp(`^ {0,3}${opaqueFence.char}{${opaqueFence.length},}\\s*$`).test(lines[i])) opaqueFence = null;
      continue;
    }
    if (!cell) {
      let line = lines[i];
      const opaque = /^ {0,3}(`{3,}|~{3,})/.exec(line);
      if (opaque) opaqueFence = { char: opaque[1][0], length: opaque[1].length };
      if (!opaque) {
        const div = DIV_FENCE.exec(line);
        if (div) line = div[2] ? divMarkup(div[2], false, references) : divMarkup("", true, references);
        else line = line.replace(INCLUDE, (_, name) => expandedInclude(name, 0, new Set(), path, i + 1));
        line = proseLine(line, references, {
          lineNumber: i + 1,
          inlineRecords: parsed.inlineRecords,
          inlineValues: inlineValues || bundle?.inline_results || bundle?.inline || bundle?.inline_values || null,
          currentContext: currentContext || bundle?.context?.computation_sha256 || "",
        });
      }
      // Included text has no one-to-one source line in the main qmd. Keep
      // diagnostics generated from it unmapped instead of attaching them to
      // the include directive or a nearby authored paragraph.
      push(line, lines[i].includes("{{<") ? null : i);
      continue;
    }
    const options = { ...defaults, ...cell.options };
    const include = options.include !== false;
    const echo = options.echo !== false && include;
    const outputEntry = options.output !== false && include && options.eval !== false ? outputFor(bundle, cell) : null;
    const output = outputMarkup(outputEntry, assets, cell, references);
    if (echo) {
      const fence = lines[i].match(/^\s*(`{3,}|~{3,})/)?.[1] || "```";
      const info = lines[i].trim().slice(fence.length).trim();
      push(`${fence}${info}`, i);
      i += 1;
      while (i < lines.length && !new RegExp(`^ {0,3}${fence[0]}{${fence.length},}\\s*$`).test(lines[i])) {
        // Option declarations control presentation and are not useful code.
        if (!OPTION.test(lines[i])) push(lines[i], i);
        i += 1;
      }
      if (i < lines.length) push(lines[i], i);
    } else {
      // Consume through the closing fence while retaining a source location
      // independent placeholder. Hidden code is never copied into output.
      i = cell.endLine;
      if (i < lines.length) {
        if (output) push(output);
        else if (include && options.output !== false && options.eval !== false) push(`<div class="quarto-output-missing" data-cell="${escapeHtml(cell.id)}">No saved result for this cell.</div>`);
      }
      continue;
    }
    if (output) push(output);
    else if (include && options.output !== false && options.eval !== false) push(`<div class="quarto-output-missing" data-cell="${escapeHtml(cell.id)}">No saved result for this cell.</div>`);
  }
  return { ...parsed, markdown: out.join("\n"), lineMap, diagnostics: [...parsed.diagnostics, ...generatedDiagnostics], source, assets, bundle };
}

export async function virtualTree(tree, options = {}) {
  const main = tree.main || "main.qmd";
  const source = tree.texts?.[main] || "";
  const parsed = parseQuarto(source, { path: main });
  const hashes = await Promise.all(parsed.cells.map((cell) => cellFingerprint(cell)));
  const counts = new Map();
  hashes.forEach((hash) => counts.set(hash, (counts.get(hash) || 0) + 1));
  const cellFingerprints = Object.fromEntries(parsed.cells.map((cell, index) => [cell.id, { sha256: hashes[index], ambiguous: counts.get(hashes[index]) !== 1 }]));
  const draft = composeDraft(source, { ...options, path: main, assets: tree.urls || {}, cellFingerprints });
  let virtualMain = main.replace(/\.qmd$/i, ".md");
  while (Object.hasOwn(tree.texts || {}, virtualMain)) virtualMain = virtualMain.replace(/\.md$/, "-draft.md");
  const texts = { ...(tree.texts || {}), [virtualMain]: draft.markdown };
  if (virtualMain !== main) delete texts[main];
  return { ...tree, main: virtualMain, texts, quarto: draft, quartoSourcePath: main, quartoLineMap: draft.lineMap };
}

// The Markdown worker sees the generated `.md` draft. Keep diagnostics on
// original qmd coordinates whenever a generated line has a reliable source
// counterpart; generated title/output/include diagnostics stay explicitly
// unmappable instead of being attached to nearby prose.
export function mapQuartoDiagnostics(diagnostics, virtualTree) {
  const lineMap = virtualTree?.quartoLineMap || virtualTree?.quarto?.lineMap || [];
  const sourcePath = virtualTree?.quartoSourcePath || virtualTree?.quarto?.path || virtualTree?.main || "main.qmd";
  return (diagnostics || []).map((diagnostic) => {
    const line = Number(diagnostic?.line || 0);
    const mapped = line > 0 ? lineMap[line - 1] : null;
    if (mapped == null) return { ...diagnostic, file: sourcePath, line: 0, column: 0, end_line: 0, end_column: 0, generated: true };
    const endLine = Number(diagnostic?.end_line || line);
    const mappedEnd = lineMap[endLine - 1];
    return {
      ...diagnostic,
      file: sourcePath,
      line: mapped + 1,
      end_line: mappedEnd == null ? mapped + 1 : mappedEnd + 1,
      generated: false,
    };
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

export function classifyFreshness(parsed, bundle, { currentContext = "", sourceCompatible = false, trackedInputsMatch = null } = {}) {
  if (!bundle) return { state: "missing", message: "No saved result" };
  if (parsed?.diagnostics?.length) return { state: "unknown", message: "Could not verify saved results for this source" };
  if (bundle.provenance?.kind === "imported" || bundle.provenance?.kind === "import") return { state: "unknown", message: "Imported results; freshness unknown" };
  if (trackedInputsMatch === false) return { state: "potentially-stale", message: "Results may be outdated" };
  if (bundle.source?.verification === "unverified" || bundle.provenance?.input_stable === false || bundle.provenance?.inputStable === false) {
    return { state: "unknown", message: "Saved results; freshness unknown" };
  }
  if (currentContext && bundle.context?.computation_sha256) {
    return currentContext === bundle.context.computation_sha256
      ? { state: "matches-recorded-inputs", message: "Saved results; computation source unchanged" }
      : { state: "potentially-stale", message: "Results may be outdated" };
  }
  if (sourceCompatible && bundle.context?.computation_sha256) return { state: "source-compatible", message: "Showing saved results" };
  return { state: "unknown", message: "Saved results; freshness unknown" };
}

export function cellAssociation(parsed, bundle) {
  const result = new Map();
  for (const cell of parsed.cells) {
    const match = outputFor(bundle, cell);
    result.set(cell.id, match ? (cell.ambiguous ? "ambiguous" : "mapped") : "unmapped");
  }
  return result;
}

export { escapeHtml, safeFragment, outputMarkup };
// Short aliases used by native/browser parity checks and future adapters.
export const parseQmd = parseQuarto;
export const transformQuarto = composeDraft;
export const draftOf = composeDraft;
