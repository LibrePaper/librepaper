// Citation keys in a source file, and the keys that have no bibliography entry.
// Pure text helpers, so the editor and the check scripts share them.
const WORD = /[-\p{L}\p{N}_:.+]/u;
const LATEX_CITE = /\\(?:(?:no|paren|text|auto|foot|smart|super|Paren|Text|Auto|Foot|Smart|Super)?[Cc]ite(?:p|t|author|year)?\*?)\s*(?:\[[^\]]*\]\s*)*\{([^}]*)\}/g;
const MARKDOWN_AT = /@[-\p{L}\p{N}_:.+]+/gu;
const TYPST_AT = /@[-\p{L}\p{N}_:.]+/gu;
const TYPST_CITE = /#cite\(\s*<([-\p{L}\p{N}_:.]+)>/gu;
const LABEL = /<([\p{L}\p{N}_\-:.]+)>/gu;
const INLINE_CODE = /(`+)(?!`)[\s\S]*?(?<!`)\1(?!`)/g;
const TYPST_RAW = INLINE_CODE;
const TYPST_COMMENT = /(?<!:)\/\/.*$/;
const CROSS_REF = /^(fig|tbl|sec|eq|lst|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|tip|nte|wrn|imp|cau)[-:]/;

const blankLine = (line) => " ".repeat(line.length);

function latexKeys(text) {
  const found = [];
  for (const match of text.matchAll(LATEX_CITE)) {
    if (commentedBefore(text, match.index)) continue;
    const bodyStart = match.index + match[0].length - 1 - match[1].length;
    found.push(...splitKeys(match[1], bodyStart));
  }
  return found;
}

function commentedBefore(text, index) {
  const start = text.lastIndexOf("\n", index - 1) + 1;
  for (let i = start; i < index; i++) if (text[i] === "%" && text[i - 1] !== "\\") return true;
  return false;
}

function splitKeys(body, start) {
  const keys = [];
  let at = 0;
  for (const part of body.split(",")) {
    const key = part.trim();
    const from = start + at + (part.length - part.trimStart().length);
    if (key && key !== "*") keys.push({ key, from, to: from + key.length });
    at += part.length + 1;
  }
  return keys;
}

// Matches of the at-pattern that are not part of a word, an address or a path.
function atKeys(text, pattern) {
  const found = [];
  for (const match of text.matchAll(pattern)) {
    const previous = text[match.index - 1] || "";
    if (previous && ((WORD.test(previous) && previous !== "-") || previous === "@" || previous === "/")) continue;
    const key = match[0].slice(1).replace(/[.:]+$/, "");
    if (!key) continue;
    found.push({ key, from: match.index + 1, to: match.index + 1 + key.length });
  }
  return found;
}

// The markdown source with code and front matter blanked. Blanking keeps every
// offset and every line break, so the positions of the keys stay valid.
function markdownMasked(text) {
  const rows = text.split("\n");
  let fence = null, frontMatter = false;
  return rows.map((line, index) => {
    if (index === 0 && /^---\s*$/.test(line)) { frontMatter = true; return blankLine(line); }
    if (frontMatter) { if (/^(---|\.\.\.)\s*$/.test(line)) frontMatter = false; return blankLine(line); }
    const match = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (fence) {
      if (match && match[1][0] === fence.marker && match[1].length >= fence.length && !match[2].trim()) fence = null;
      return blankLine(line);
    }
    if (match) { fence = { marker: match[1][0], length: match[1].length }; return blankLine(line); }
    return line.replace(INLINE_CODE, blankLine);
  }).join("\n");
}

// The typst source with raw spans and line comments blanked.
function typstMasked(text) {
  return text.split("\n").map((line) => line.replace(TYPST_RAW, blankLine).replace(TYPST_COMMENT, blankLine)).join("\n");
}

/**
 * The citation keys in a source file, with the UTF-16 offsets of each key.
 * @param {string} text
 * @param {string} format "latex", "tex", "typst", "markdown" or "quarto"
 * @param {{ labels?: Set<string> }} [options] typst labels, which are cross references
 * @returns {Array<{ key: string, from: number, to: number }>}
 */
export function citedKeys(text, format, { labels = new Set() } = {}) {
  text = String(text ?? "");
  let found;
  if (format === "latex" || format === "tex") {
    found = latexKeys(text);
  } else if (format === "typst") {
    const masked = typstMasked(text);
    found = [...atKeys(masked, TYPST_AT), ...[...masked.matchAll(TYPST_CITE)].map((match) => {
      const from = match.index + match[0].indexOf("<") + 1;
      return { key: match[1], from, to: from + match[1].length };
    })].filter((cite) => !labels.has(cite.key));
  } else if (format === "markdown" || format === "quarto") {
    found = atKeys(markdownMasked(text), MARKDOWN_AT).filter((cite) => !CROSS_REF.test(cite.key));
  } else {
    return [];
  }
  return found.sort((a, b) => a.from - b.from);
}

/**
 * The labels defined in typst sources, which a typst @key may refer to.
 * @param {Record<string, string>} texts path -> source
 * @returns {Set<string>}
 */
export function typstLabels(texts) {
  const labels = new Set();
  for (const [path, source] of Object.entries(texts || {})) {
    if (!/\.typ$/i.test(path)) continue;
    for (const match of String(source ?? "").matchAll(LABEL)) labels.add(match[1]);
  }
  return labels;
}

/**
 * The citations whose key has no bibliography entry. Nothing is unknown until
 * the bibliography is parsed, so null entries give none.
 * @param {{ text: string, format: string, entries: Array<{ key: string }> | null | undefined, labels?: Set<string> }} request
 * @returns {Array<{ key: string, from: number, to: number }>}
 */
export function unknownCitations({ text, format, entries, labels }) {
  if (entries == null) return [];
  const known = new Set(entries.map((entry) => entry?.key));
  return citedKeys(text, format, { labels }).filter((cite) => !known.has(cite.key));
}
