// Matching and cache policy for bibliography completion. Parsing itself lives
// in the engine WASM module; these helpers are also usable by check scripts.
import { m } from "./paraglide/messages.js";
import { normaliseDoi } from "./doi.js";

const WORD = /[-\p{L}\p{N}_:.+]/u;
const KEY = /^[-\p{L}\p{N}_:.+]*$/u;
const string = (value) => value == null ? "" : String(value);

function authors(entry) {
  const value = entry?.authors;
  const display = (author) => author && typeof author === "object"
    ? string(author.name || author.family || [author.given, author.last].filter(Boolean).join(" "))
    : string(author);
  return (Array.isArray(value) ? value : value ? [value] : []).map(display).filter(Boolean);
}
function normalized(entry) {
  const fields = entry?.fields && Object.keys(entry.fields).length ? entry.fields : entry?.raw || {};
  return { ...entry, key: string(entry?.key), type: string(entry?.type), authors: authors(entry),
    year: string(entry?.year), title: string(entry?.title), container: string(entry?.container),
    doi: string(entry?.doi), url: string(entry?.url), fields };
}
const lower = (value) => string(value).toLocaleLowerCase();
function surnames(entry) {
  return authors(entry).map((author) => {
    const value = author.trim();
    return (value.includes(",") ? value.split(",", 1)[0] : value.split(/\s+/).at(-1) || value).replace(/^[{(]+|[})]+$/g, "");
  });
}
function score(entry, query) {
  const needle = lower(query).trim();
  if (!needle) return [0, 0];
  const key = lower(entry.key), names = surnames(entry).map(lower), title = lower(entry.title), year = lower(entry.year);
  if (key === needle) return [0, 0];
  if (key.startsWith(needle)) return [0, key.length - needle.length];
  if (key.includes(needle)) return [0, 100 + key.indexOf(needle)];
  if (names.some((name) => name === needle)) return [1, 0];
  if (names.some((name) => name.startsWith(needle))) return [1, 20];
  if (names.some((name) => name.includes(needle))) return [1, 100];
  const words = needle.split(/\s+/).filter(Boolean), titleWords = title.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (words.length && words.every((word) => titleWords.some((candidate) => candidate.startsWith(word)))) return [2, titleWords.length - words.length];
  if (title.includes(needle)) return [2, 100 + title.indexOf(needle)];
  if (year === needle) return [3, 0];
  if (year.startsWith(needle)) return [3, 20];
  return null;
}
export function rankEntries(entries, query = "") {
  return (entries || []).map((entry, index) => ({ entry: normalized(entry), index }))
    .map((item) => ({ ...item, score: score(item.entry, query) })).filter((item) => item.score)
    .sort((a, b) => a.score[0] - b.score[0] || a.score[1] - b.score[1] || a.index - b.index).map((item) => item.entry);
}
function markdownCode(text, position) {
  const before = text.slice(0, position);
  let fence = null;
  for (const line of before.split("\n")) {
    const match = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (!match) continue;
    if (!fence) fence = { marker: match[1][0], length: match[1].length };
    else if (match[1][0] === fence.marker && match[1].length >= fence.length && !match[2].trim()) fence = null;
  }
  if (fence) return true;
  const line = before.slice(before.lastIndexOf("\n") + 1);
  let ticks = 0;
  for (const match of line.matchAll(/`+/g)) {
    const prefix = line.slice(0, match.index);
    if ((prefix.match(/\\+$/)?.[0].length || 0) % 2) continue;
    if (!ticks) ticks = match[0].length;
    else if (ticks === match[0].length) ticks = 0;
  }
  return ticks > 0;
}
function markupContext(text, position, format) {
  if (format === "markdown" || format === "quarto") { if (markdownCode(text, position)) return null; }
  else if (format !== "typst") return null;
  let from = position;
  while (from > 0 && WORD.test(text[from - 1])) from--;
  if (!from || text[from - 1] !== "@") return null;
  const previous = text[from - 2] || "";
  if (previous && ((WORD.test(previous) && previous !== "-") || previous === "@" || previous === "/")) return null;
  return { kind: format, from, to: position, query: text.slice(from, position), trigger: "@" };
}
const CITE = /\\(?:textcite|parencite|autocite|smartcite|supercite|footcite|cite)(?:[A-Za-z]*)?\s*(?:\[[^\]]*\]\s*)*\{/g;
function latexContext(text, position) {
  const line = text.slice(text.lastIndexOf("\n", position - 1) + 1, position);
  for (let at = 0; at < line.length; at++) if (line[at] === "%" && line[at - 1] !== "\\") return null;
  let match, latest = null;
  const prefix = text.slice(0, position);
  while ((match = CITE.exec(prefix))) latest = match;
  CITE.lastIndex = 0;
  if (!latest) return null;
  const open = latest.index + latest[0].length, body = text.slice(open, position);
  if (body.includes("}")) return null;
  const comma = Math.max(body.lastIndexOf(","), body.lastIndexOf(";"));
  let from = open + comma + 1;
  while (from < position && /\s/.test(text[from])) from++;
  if (from < position && !KEY.test(text.slice(from, position))) return null;
  return { kind: "latex", from, to: position, query: text.slice(from, position), trigger: latest[0].trim() };
}
// An Org citation group opens with [cite:, [cite/t:, [cite/na: and so on.
const ORG_CITE_OPEN = /\[cite(?:\/[a-z/]+)?:/g;
const ORG_KEY = /[-\p{L}\p{N}_:.+/]/u;
// The last citation group opened on the line before `position`, and where its body starts.
function orgGroup(text, position) {
  const lineStart = text.lastIndexOf("\n", position - 1) + 1;
  const prefix = text.slice(lineStart, position);
  let latest = null;
  for (const match of prefix.matchAll(ORG_CITE_OPEN)) latest = match;
  if (!latest) return null;
  const open = lineStart + latest.index + latest[0].length;
  return text.slice(open, position).includes("]") ? null : { open };
}
function orgContext(text, position) {
  const group = orgGroup(text, position);
  if (!group) return null;
  let from = position;
  while (from > group.open && ORG_KEY.test(text[from - 1])) from--;
  if (text[from - 1] !== "@" || from - 1 < group.open) return null;
  const previous = text[from - 2] || "";
  if (/[\p{L}\p{N}@/]/u.test(previous)) return null;
  return { kind: "org", from, to: position, query: text.slice(from, position), trigger: "@" };
}
// The end of the citation group around `position`: its "]" when the group is closed, else `position`.
function orgInsertionPoint(text, position) {
  const close = text.indexOf("]", position);
  if (close === -1) return position;
  const between = text.slice(position, close);
  return /[\n[]/.test(between) ? position : close;
}
export function citationContext(text, position = text.length, format = "markdown") {
  text = string(text);
  position = Math.max(0, Math.min(Number(position), text.length));
  if (format === "org") return orgContext(text, position);
  return format === "latex" || format === "tex" ? latexContext(text, position) : markupContext(text, position, format);
}
export function insertCitation(text, position, key, format = "markdown") {
  if (format === "org") return insertOrgCitation(string(text), Math.max(0, Math.min(Number(position), string(text).length)), string(key));
  const context = citationContext(text, position, format);
  return context && key ? text.slice(0, context.from) + string(key) + text.slice(context.to) : string(text);
}
// Replaces a partial key being typed; otherwise adds the key to the group around the
// cursor, after a ";"; otherwise starts a new [cite:@key] group at the cursor.
function insertOrgCitation(text, position, key) {
  if (!key) return text;
  const context = orgContext(text, position);
  if (context) return text.slice(0, context.from) + key + text.slice(context.to);
  const group = orgGroup(text, position);
  if (group) {
    const at = orgInsertionPoint(text, position);
    const body = text.slice(group.open, at);
    const separator = body.trim() && !/;\s*$/.test(body) ? ";" : "";
    return text.slice(0, at) + separator + "@" + key + text.slice(at);
  }
  return text.slice(0, position) + `[cite:@${key}]` + text.slice(position);
}
export function entryLabel(entry) {
  const value = normalized(entry), names = surnames(value);
  const author = names.length > 1 ? m.bibliography_et_al({ author: names[0] }) : names[0] || "";
  return [author, value.year, value.title, value.container].filter(Boolean).join(" — ");
}
export function entryDetail(entry) { const value = normalized(entry); return value.container || value.type || ""; }
export function bibliographyCompletion({ entries, format }) {
  return (context) => {
    const kind = typeof format === "function" ? format() : format;
    const { doc } = context.state;
    const line = doc.lineAt(context.pos);
    let base = 0, text;
    if (kind === "latex" || kind === "tex") {
      // A \cite argument may wrap, but never across more than a few lines.
      base = doc.line(Math.max(1, line.number - 20)).from;
      text = doc.sliceString(base, context.pos);
    } else if (line.text.slice(0, context.pos - line.from).includes("@")) {
      text = doc.toString(); // fenced code needs the whole prefix; only after an `@`
    } else return null;
    const found = citationContext(text, context.pos - base, kind);
    if (!found) return null;
    found.from += base;
    found.to += base;
    const available = typeof entries === "function" ? entries() : entries;
    const ranked = rankEntries(available, found.query);
    if (!ranked.length) return null;
    return { from: found.from, to: found.to, filter: false,
      options: ranked.map((entry) => ({ label: entry.key, displayLabel: entryLabel(entry), detail: entryDetail(entry), info: entryLabel(entry), type: "reference" })) };
  };
}
// A \cite argument that opened on an earlier line and has not been closed.
const OPEN_CITE = /\\[A-Za-z]*cite[A-Za-z]*\s*(?:\[[^\]]*\]\s*)*\{[^}]*$/;
/**
 * Where bibliographyCompletion can answer, for a format: after an "@" on the
 * line, or in LaTeX inside an unclosed \cite argument, on this line or one of
 * the 20 before it (the source reads back that far).
 * @param {string} format
 * @returns {(before: string, state: import("@codemirror/state").EditorState, pos: number) => boolean}
 */
export function bibliographyTrigger(format) {
  return (before, state, pos) => {
    if (format !== "latex" && format !== "tex") return before.includes("@");
    const { doc } = state;
    const from = doc.line(Math.max(1, doc.lineAt(pos).number - 20)).from;
    return OPEN_CITE.test(doc.sliceString(from, pos));
  };
}
export function selectedBibliographyFiles(source, format) {
  const names = [];
  if (format === "markdown" || format === "quarto") {
    // Only the leading front matter declares a bibliography; a "Bibliography:"
    // line and bullet list in the body are prose.
    const all = string(source).replace(/^\uFEFF/, "").split(/\r?\n/);
    const close = all[0]?.trim() === "---" ? all.findIndex((line, index) => index > 0 && /^(?:---|\.\.\.)\s*$/.test(line)) : -1;
    const lines = close > 0 ? all.slice(1, close) : [];
    for (let i = 0; i < lines.length; i++) {
      const match = lines[i].match(/^\s*bibliography\s*:\s*(.*?)\s*$/i);
      if (!match) continue;
      names.push(...match[1].replace(/[\[\]"']/g, "").split(",").map((name) => name.trim()).filter(Boolean));
      for (let j = i + 1; j < lines.length; j++) {
        const item = lines[j].match(/^\s*-\s+["']?(.*?)["']?\s*$/);
        if (item) { if (item[1].trim()) names.push(item[1].trim()); } else if (lines[j].trim()) break;
      }
    }
  } else if (format === "typst") {
    // The call's sources are its positional argument: a string or an array of strings. Named arguments (style, title) are dropped first.
    for (const match of string(source).matchAll(/#bibliography\s*\(((?:[^()]|\([^()]*\))*)\)/g)) {
      const positional = match[1].replace(/[A-Za-z][\w-]*\s*:\s*(?:"[^"]*"|[^,)]*)/g, "");
      names.push(...[...positional.matchAll(/"([^"]+)"/g)].map((item) => item[1].trim()).filter(Boolean));
    }
  } else if (format === "latex" || format === "tex") {
    for (const match of string(source).matchAll(/\\(?:addbibresource|bibliography)\s*(?:\[[^]]*\])?\{([^}]+)\}/g)) names.push(...match[1].split(",").map((name) => name.trim()).filter(Boolean));
  } else if (format === "org") {
    // One path per #+BIBLIOGRAPHY: line, optionally quoted.
    for (const match of string(source).matchAll(/^[ \t]*#\+BIBLIOGRAPHY:[ \t]*(.*?)[ \t]*\r?$/gim)) {
      const name = match[1].replace(/^["']|["']$/g, "").trim();
      if (name) names.push(name);
    }
  }
  return [...new Set(names)].sort();
}
export function bibliographyFiles({ texts = {} } = {}) { return Object.keys(texts).filter((path) => /\.bib$/i.test(path)).sort(); }
export function bibliographyNeedsAnalysis(request = {}) { return bibliographyFiles(request).length > 0 || selectedBibliographyFiles(request.source, request.format).length > 0; }
export function bibliographyCacheKey({ main = "", format = "", source = "", texts = {} } = {}) {
  const config = string(source).split(/\r?\n/).flatMap((line, index) =>
    /^\s*bibliography\s*:/i.test(line) || /^\s*-\s*[^#].*\.bib\s*$/i.test(line) ||
    /#bibliography\s*\(|\\(?:addbibresource|bibliography)\b/.test(line) || /^\s*(?:---|\.\.\.)\s*$/.test(line) || /\/\*|\*\//.test(line)
      ? [`${index + 1}:${line}`] : []).join("\n");
  return JSON.stringify({ main: string(main), format: string(format), config, selected: selectedBibliographyFiles(source, format), bib: bibliographyFiles({ texts }).map((path) => [path, string(texts[path]).length, contentHash(string(texts[path]))]) });
}
// A 53-bit hash of a text, so that the key holds a number where it once held
// the whole file. Together with the length, a collision is not a concern.
function contentHash(text) {
  let h1 = 0xdeadbeef, h2 = 0x41c6ce57;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return 4294967296 * (2097151 & h2) + (h1 >>> 0);
}

function collisionSuffix(index) {
  let value = "";
  while (index > 0) { index--; value = String.fromCharCode(97 + index % 26) + value; index = Math.floor(index / 26); }
  return value;
}

export function zoteroCitationKey(bibtex, item, base) {
  const marker = new RegExp(`x-librepaper-zotero-item\\s*=\\s*[{"]${String(item).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[}"]`, "i");
  const found = marker.exec(string(bibtex));
  if (found) {
    const prefix = string(bibtex).slice(0, found.index);
    const headers = [...prefix.matchAll(/@[A-Za-z]+\s*\{\s*([^,\s]+)\s*,/g)];
    if (headers.length) return headers.at(-1)[1];
  }
  const used = new Set([...string(bibtex).matchAll(/@[A-Za-z]+\s*\{\s*([^,\s]+)\s*,/g)].map((match) => match[1].toLocaleLowerCase()));
  if (!used.has(base.toLocaleLowerCase())) return base;
  for (let index = 1; ; index++) {
    const candidate = base + collisionSuffix(index);
    if (!used.has(candidate.toLocaleLowerCase())) return candidate;
  }
}

export function replaceBibtexKey(entry, key) {
  return string(entry).replace(/^\s*(@[A-Za-z]+\s*\{\s*)[^,\s]+(\s*,)/, `$1${key}$2`).trim() + "\n";
}

// The latex source with every unescaped % comment blanked. Lengths and line
// breaks are kept, so offsets found in the copy are valid in the source.
function latexMasked(source) {
  return source.replace(/(^|[^\\])%[^\n]*/g, (match, lead) => lead + " ".repeat(match.length - lead.length));
}

function bibliographyRegistrationLatex(source, path) {
  const name = path.replace(/\.bib$/i, "");
  const masked = latexMasked(source);
  const usepackage = /\\usepackage(?:\[[^\]]*\])?\{([^}]*)\}/g;
  const natbib = [...masked.matchAll(usepackage)].some((match) => match[1].split(",").map((item) => item.trim()).includes("natbib"));
  for (const match of masked.matchAll(usepackage)) {
    if (!match[1].split(",").map((item) => item.trim()).includes("biblatex")) continue;
    const end = source.indexOf("\n", match.index);
    if (end === -1) return { from: source.length, to: source.length, insert: `\n\\addbibresource{${path}}` };
    // The new line ends the way the package line does, so a CRLF file stays CRLF.
    const eol = source[end - 1] === "\r" ? "\r\n" : "\n";
    return { from: end + 1, to: end + 1, insert: `\\addbibresource{${path}}${eol}` };
  }
  const style = /\\bibliographystyle\b/.test(masked) ? "" : `\\bibliographystyle{${natbib ? "plainnat" : "plain"}}\n`;
  const block = `${style}\\bibliography{${name}}\n`;
  const end = /\\end\{document\}/.exec(masked);
  if (end) {
    const at = source.lastIndexOf("\n", end.index) + 1;
    return { from: at, to: at, insert: block };
  }
  const lead = source.endsWith("\n") ? "" : "\n";
  return { from: source.length, to: source.length, insert: lead + block };
}

export function bibliographyRegistration(source, path = "references.bib", format = "markdown") {
  source = string(source);
  if (format === "latex" || format === "tex") {
    if (selectedBibliographyFiles(latexMasked(source), format).length) return null;
    return bibliographyRegistrationLatex(source, path);
  }
  if (format === "typst") {
    if (selectedBibliographyFiles(source, format).length) return null;
    const lead = source.endsWith("\n") ? "" : "\n";
    return { from: source.length, to: source.length, insert: `${lead}#bibliography("${path}")\n` };
  }
  if (format === "org") {
    if (selectedBibliographyFiles(source, format).length) return null;
    const eol = source.includes("\r\n") ? "\r\n" : "\n";
    const line = `#+BIBLIOGRAPHY: ${/\s/.test(path) ? `"${path}"` : path}`;
    // The header is the run of #+KEY: lines at the top; the new line goes after its last line.
    const lines = source.split("\n");
    let count = 0;
    while (count < lines.length && /^#\+[A-Za-z_0-9-]+:/.test(lines[count])) count++;
    if (!count) return { from: 0, to: 0, insert: line + eol };
    // The last line of a file without a final line break holds the header's end.
    if (count === lines.length) return { from: source.length, to: source.length, insert: eol + line };
    let from = 0;
    for (let i = 0; i < count; i++) from += lines[i].length + 1;
    return { from, to: from, insert: line + eol };
  }
  if (selectedBibliographyFiles(source, "markdown").length) return null;
  const line = `bibliography: ${path}\n`;
  if (/^---\s*\r?\n/.test(source)) {
    const closing = /^---\s*$/m.exec(source.slice(source.indexOf("\n") + 1));
    if (closing) {
      const from = source.indexOf("\n") + 1 + closing.index;
      return { from, to: from, insert: line };
    }
  }
  return { from: 0, to: 0, insert: `---\n${line}---\n\n` };
}

function projectBibliographyPath(filePath, reference) {
  if (!reference || /^[a-z]+:/i.test(reference) || reference.startsWith("/")) return null;
  const parts = String(filePath || "").split("/").slice(0, -1);
  for (const part of reference.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") { if (!parts.length) return null; parts.pop(); }
    else parts.push(part);
  }
  return parts.join("/");
}

// Where a Markdown or Quarto document's bibliography is declared: its own front
// matter, else the preview's entry file, else (Quarto) the nearest _quarto.yml.
// files are project-relative paths, resolved against the declaring file.
export function declaredBibliography({ format = "", chapterPath = "", chapterText = "", entryPath = "", entryText = "", texts = {} } = {}) {
  const resolve = (from, source, kind) => selectedBibliographyFiles(source, kind).map((name) => projectBibliographyPath(from, name)).filter(Boolean);
  const none = { files: [], owner: null, ownerPath: null };
  const inChapter = resolve(chapterPath, chapterText, format);
  if (inChapter.length) return { files: inChapter, owner: "chapter", ownerPath: chapterPath || null };
  if (format !== "markdown" && format !== "quarto") return none;
  if (entryPath && entryPath !== chapterPath && /\.(?:md|qmd|rmd|markdown)$/i.test(entryPath)) {
    const inEntry = resolve(entryPath, entryText, format);
    if (inEntry.length) return { files: inEntry, owner: "entry", ownerPath: entryPath };
  }
  if (format === "quarto") {
    const dirs = String(chapterPath || "").split("/").slice(0, -1);
    for (let depth = dirs.length; depth >= 0; depth--) {
      const prefix = dirs.slice(0, depth).join("/");
      for (const name of ["_quarto.yml", "_quarto.yaml"]) {
        const path = prefix ? `${prefix}/${name}` : name;
        if (!Object.hasOwn(texts, path)) continue;
        const files = resolve(path, `---\n${string(texts[path])}\n---\n`, "quarto");
        if (files.length) return { files, owner: "quarto", ownerPath: path };
      }
    }
  }
  return none;
}

// The part of an import plan the two sources share: which .bib the entry goes
// into, its text so far, and the registration the citing file may need.
/**
 * @typedef {{ source: string, targetPath: string, texts: Record<string, string>, format: string,
 *   chapterPath?: string, chapterText?: string, entryPath?: string, entryText?: string }} ImportWhere
 */

// The declared (or, failing that, existing) .bib files an import can go into.
/** @param {ImportWhere} where */
function resolveTargets({ source, targetPath, texts, format, chapterPath, chapterText, entryPath, entryText }) {
  const project = (format === "markdown" || format === "quarto") && chapterPath !== undefined;
  const declared = project ? declaredBibliography({ format, chapterPath, chapterText, entryPath, entryText, texts }) : null;
  const configured = declared ? declared.files : selectedBibliographyFiles(format === "latex" || format === "tex" ? latexMasked(string(source)) : source, format);
  const configuredPaths = (declared ? configured : configured.map((path) => projectBibliographyPath(targetPath, path))).filter(Boolean)
    .map((path) => (format === "latex" || format === "tex") && !/\.bib$/i.test(path) ? `${path}.bib` : path);
  const existing = bibliographyFiles({ texts });
  // BibTeX is only ever appended to a .bib file, never to a CSL-JSON or YAML bibliography.
  const bibPaths = configuredPaths.filter((name) => /\.bib$/i.test(name));
  if (configuredPaths.length && !bibPaths.length) throw Object.assign(new Error("bibliography_not_bibtex"), { code: "bibliography_not_bibtex" });
  return { configured, configuredPaths, existing, bibPaths };
}

/**
 * The .bib files an import can add to, sorted, and whether the document declares them.
 * Takes the same inputs as the import planners.
 * @param {ImportWhere} where
 * @returns {{ files: string[], declared: boolean }}
 */
export function bibliographyTargets(where) {
  let resolved;
  try { resolved = resolveTargets(where); }
  catch (error) { if (error?.code === "bibliography_not_bibtex") return { files: [], declared: true }; throw error; }
  if (resolved.configuredPaths.length) return { files: [...new Set(resolved.bibPaths)].sort(), declared: true };
  return { files: resolved.existing, declared: false };
}

/** @param {ImportWhere & { bibPath?: string }} where */
function importTarget({ bibPath, ...where }) {
  const { source, targetPath, format } = where;
  const { configured, existing, bibPaths } = resolveTargets(where);
  const candidates = configured.length ? bibPaths : existing;
  const chosen = bibPath && candidates.includes(bibPath) ? bibPath : null;
  const path = chosen || bibPaths.find((name) => Object.hasOwn(where.texts, name)) || bibPaths[0] || existing[0] || "references.bib";
  const previous = string(where.texts[path]);
  const registration = () => {
    if (configured.length) return null;
    const depth = String(targetPath || "").split("/").slice(0, -1).length;
    return bibliographyRegistration(source, "../".repeat(depth) + path, format);
  };
  return { path, previous, registration, bibPaths };
}

function importPlan({ texts, path, previous, registration }, key, entryText, already) {
  const addition = already ? "" : (previous.trim() ? "\n" : "") + replaceBibtexKey(entryText, key);
  const bibtex = already ? previous : previous.replace(/\s*$/, "") + addition;
  return { key, path, bibtex, addition, create: !Object.hasOwn(texts, path), registration: registration(), already };
}

export function planZoteroImport({ source, targetPath, texts, item, format = "markdown", ...where }) {
  const target = importTarget({ source, targetPath, texts, format, ...where });
  const already = new RegExp(`x-librepaper-zotero-item\\s*=\\s*[{"]${String(item.zotero_item).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[}"]`, "i").test(target.previous);
  // An item already in the target keeps its key; a new one avoids the keys of every .bib.
  let key;
  if (already) key = zoteroCitationKey(target.previous, item.zotero_item, item.citation_key);
  else {
    const used = new Set([target.previous, ...bibliographyFiles({ texts }).map((name) => string(texts[name]))]
      .flatMap((text) => [...text.matchAll(/@[A-Za-z]+\s*\{\s*([^,\s]+)\s*,/g)]).map((match) => match[1].toLocaleLowerCase()));
    key = item.citation_key;
    for (let index = 1; used.has(key.toLocaleLowerCase()); index++) key = item.citation_key + collisionSuffix(index);
  }
  return importPlan({ ...target, texts }, key, item.bibtex, already);
}

// The key of the entry in a .bib whose doi field equals `doi`, or null.
function doiEntryKey(bibtex, doi) {
  const same = (text) => (normaliseDoi(text) ?? String(text).trim()).toLocaleLowerCase();
  const wanted = same(doi);
  const field = /(?<=^|[\s,{])doi\s*=\s*(?:\{([^}]*)\}|"([^"]*)")/gim;
  for (const found of string(bibtex).matchAll(field)) {
    const lineStart = string(bibtex).lastIndexOf("\n", found.index - 1) + 1;
    if (string(bibtex).slice(lineStart, found.index).trimStart().startsWith("%")) continue;
    if (same(found[1] ?? found[2]) !== wanted) continue;
    const headers = [...string(bibtex).slice(0, found.index).matchAll(/@[A-Za-z]+\s*\{\s*([^,\s]+)\s*,/g)];
    if (headers.length) return headers.at(-1)[1];
  }
  return null;
}

// Plans adding a reference fetched by DOI. entry is { doi, key, bibtex }. A DOI a loaded .bib
// already holds is reused under its own key; otherwise the key is de-duplicated like Zotero's.
export function planDoiImport({ source, targetPath, texts, entry, format = "markdown", ...where }) {
  const target = importTarget({ source, targetPath, texts, format, ...where });
  const bibTexts = bibliographyFiles({ texts }).map((name) => string(texts[name]));
  // A DOI is reused only from a bibliography the document loads; every .bib still counts for collisions.
  const loaded = (target.bibPaths.length ? target.bibPaths : [target.path]).map((name) => string(texts[name]));
  for (const text of loaded) {
    const existingKey = doiEntryKey(text, entry.doi);
    if (existingKey) return importPlan({ ...target, texts }, existingKey, entry.bibtex, true);
  }
  const used = new Set([target.previous, ...bibTexts].flatMap((text) => [...text.matchAll(/@[A-Za-z]+\s*\{\s*([^,\s]+)\s*,/g)]).map((match) => match[1].toLocaleLowerCase()));
  let key = entry.key;
  for (let index = 1; used.has(key.toLocaleLowerCase()); index++) key = entry.key + collisionSuffix(index);
  return importPlan({ ...target, texts }, key, entry.bibtex, false);
}
export class BibliographyCache {
  constructor(limit = 4) { this.limit = limit; this.values = new Map(); }
  get(request, analyze, key = bibliographyCacheKey(request)) {
    if (this.values.has(key)) { const value = this.values.get(key); this.values.delete(key); this.values.set(key, value); return value; }
    const promise = Promise.resolve().then(() => analyze(request)).then((result) => ({ entries: (result?.entries || []).map(normalized), diagnostics: result?.diagnostics || [] })).catch((error) => { this.values.delete(key); throw error; });
    this.values.set(key, promise);
    while (this.values.size > this.limit) this.values.delete(this.values.keys().next().value);
    return promise;
  }
  clear() { this.values.clear(); }
}
export const bibliographyCache = new BibliographyCache(4);
