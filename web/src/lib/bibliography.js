// Matching and cache policy for bibliography completion. Parsing itself lives
// in the engine WASM module; these helpers are also usable by check scripts.
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
  while (from > 0 && (WORD.test(text[from - 1]) || /[ \t]/.test(text[from - 1]))) from--;
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
export function citationContext(text, position = text.length, format = "markdown") {
  text = string(text);
  position = Math.max(0, Math.min(Number(position), text.length));
  return format === "latex" || format === "tex" ? latexContext(text, position) : markupContext(text, position, format);
}
export function insertCitation(text, position, key, format = "markdown") {
  const context = citationContext(text, position, format);
  return context && key ? text.slice(0, context.from) + string(key) + text.slice(context.to) : string(text);
}
export function entryLabel(entry) {
  const value = normalized(entry), names = surnames(value);
  const author = names.length > 1 ? names[0] + " et al." : names[0] || "";
  return [author, value.year, value.title, value.container].filter(Boolean).join(" — ");
}
export function entryDetail(entry) { const value = normalized(entry); return value.container || value.type || ""; }
export function bibliographyCompletion({ entries, format, remote, onRemote }) {
  return (context) => {
    const found = citationContext(context.state.doc.toString(), context.pos, typeof format === "function" ? format() : format);
    if (!found) return null;
    const available = typeof entries === "function" ? entries() : entries;
    const exact = string(found.query).trim().toLocaleLowerCase();
    if (/[ \t]+$/.test(found.query) && (available || []).some((entry) => string(entry?.key).toLocaleLowerCase() === exact)) return null;
    const local = rankEntries(available, found.query);
    const options = (remoteEntries = []) => {
      const localKeys = new Set(local.map((entry) => entry.key.toLocaleLowerCase()));
      const ranked = [...local, ...rankEntries(remoteEntries, found.query).filter((entry) => !localKeys.has(entry.key.toLocaleLowerCase())).map((entry) => ({ ...entry, zotero: true }))];
      if (!ranked.length) return null;
      return { from: found.from, to: found.to, filter: false,
      options: ranked.map((entry) => ({ label: entry.key, displayLabel: entryLabel(entry), detail: entry.zotero ? "Zotero — import" : entryDetail(entry), info: entryLabel(entry), type: "reference",
        ...(entry.zotero && typeof onRemote === "function" ? { apply: (view, _completion, from, to) => onRemote(entry, { view, from, to }) } : {}) })) };
    };
    if (typeof remote !== "function") return options();
    return Promise.resolve(remote(found.query)).then(options, () => options());
  };
}
export function selectedBibliographyFiles(source, format) {
  const names = [];
  if (format === "markdown" || format === "quarto") {
    const lines = string(source).split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      const match = lines[i].match(/^\s*bibliography\s*:\s*(.*?)\s*$/i);
      if (!match) continue;
      names.push(...match[1].replace(/[\[\]"']/g, "").split(",").map((name) => name.trim()).filter(Boolean));
      for (let j = i + 1; j < lines.length; j++) {
        const item = lines[j].match(/^\s*-\s*["']?([^"']+\.bib)["']?\s*$/i);
        if (item) names.push(item[1].trim()); else if (lines[j].trim()) break;
      }
    }
  } else if (format === "typst") {
    for (const match of string(source).matchAll(/#bibliography\s*\(([^)]*)\)/g)) names.push(...[...match[1].matchAll(/["']([^"']+\.bib)["']/gi)].map((item) => item[1]));
  } else if (format === "latex" || format === "tex") {
    for (const match of string(source).matchAll(/\\(?:addbibresource|bibliography)\s*(?:\[[^]]*\])?\{([^}]+)\}/g)) names.push(...match[1].split(",").map((name) => name.trim()).filter(Boolean));
  }
  return [...new Set(names)].sort();
}
export function bibliographyFiles({ texts = {} } = {}) { return Object.keys(texts).filter((path) => /\.bib$/i.test(path)).sort(); }
export function bibliographyNeedsAnalysis(request = {}) { return bibliographyFiles(request).length > 0 || selectedBibliographyFiles(request.source, request.format).length > 0; }
export function bibliographyCacheKey({ main = "", format = "", source = "", texts = {} } = {}) {
  const config = string(source).split(/\r?\n/).map((line, index) => [index + 1, line]).filter(([, line]) =>
    /^\s*bibliography\s*:/i.test(line) || /^\s*-\s*[^#].*\.bib\s*$/i.test(line) ||
    /#bibliography\s*\(|\\(?:addbibresource|bibliography)\b/.test(line) || /^\s*(?:---|\.\.\.)\s*$/.test(line) || /\/\*|\*\//.test(line)).map(([index, line]) => `${index}:${line}`).join("\n");
  return JSON.stringify({ main: string(main), format: string(format), config, selected: selectedBibliographyFiles(source, format), bib: bibliographyFiles({ texts }).map((path) => [path, string(texts[path])]) });
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

export function bibliographyRegistration(source, path = "references.bib") {
  source = string(source);
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

function projectBibliographyPath(mainPath, reference) {
  if (!reference || /^[a-z]+:/i.test(reference) || reference.startsWith("/")) return null;
  const parts = String(mainPath || "").split("/").slice(0, -1);
  for (const part of reference.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") { if (!parts.length) return null; parts.pop(); }
    else parts.push(part);
  }
  return parts.join("/");
}

export function planZoteroImport({ source, mainPath, texts, item }) {
  const configured = selectedBibliographyFiles(source, "markdown");
  const configuredPaths = configured.map((path) => projectBibliographyPath(mainPath, path)).filter(Boolean);
  const existing = bibliographyFiles({ texts });
  const path = configuredPaths.find((name) => Object.hasOwn(texts, name)) || configuredPaths[0] || existing[0] || "references.bib";
  const previous = string(texts[path]);
  const key = zoteroCitationKey(previous, item.zotero_item, item.citation_key);
  const already = new RegExp(`x-librepaper-zotero-item\\s*=\\s*[{"]${String(item.zotero_item).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[}"]`, "i").test(previous);
  const addition = already ? "" : (previous.trim() ? "\n" : "") + replaceBibtexKey(item.bibtex, key);
  const bibtex = previous.replace(/\s*$/, "") + addition;
  let registration = null;
  if (!configured.length) {
    const depth = String(mainPath || "").split("/").slice(0, -1).length;
    registration = bibliographyRegistration(source, "../".repeat(depth) + path);
  }
  return { key, path, bibtex, addition, create: !Object.hasOwn(texts, path), registration, already };
}
export class BibliographyCache {
  constructor(limit = 4) { this.limit = limit; this.values = new Map(); }
  get(request, analyze) {
    const key = bibliographyCacheKey(request);
    if (this.values.has(key)) { const value = this.values.get(key); this.values.delete(key); this.values.set(key, value); return value; }
    const promise = Promise.resolve().then(() => analyze(request)).then((result) => ({ entries: (result?.entries || []).map(normalized), diagnostics: result?.diagnostics || [] })).catch((error) => { this.values.delete(key); throw error; });
    this.values.set(key, promise);
    while (this.values.size > this.limit) this.values.delete(this.values.keys().next().value);
    return promise;
  }
  clear() { this.values.clear(); }
}
export const bibliographyCache = new BibliographyCache(4);
