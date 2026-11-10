// Citation keys in a source file, and the keys that have no bibliography entry.
// Pure text helpers, so the editor and the check scripts share them.
import { orgTree, typstTree } from "./source-trees.js";

const WORD = /[-\p{L}\p{N}_:.+]/u;
const LATEX_CITE = /\\(?:(?:no|paren|text|auto|foot|smart|super|Paren|Text|Auto|Foot|Smart|Super)?[Cc]ite(?:p|t|author|year)?\*?)\s*(?:\[[^\]]*\]\s*)*\{([^}]*)\}/g;
const MARKDOWN_AT = /@[-\p{L}\p{N}_:.+]+/gu;
// A label declaration, not a label used as an argument such as #cite(<key>).
export const LABEL = /(?<![(,]\s*)<([\p{L}\p{N}_\-:.]+)>/gu;
const INLINE_CODE = /(`+)(?!`)[\s\S]*?(?<!`)\1(?!`)/g;
const CROSS_REF = /^(fig|tbl|sec|eq|lst|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|tip|nte|wrn|imp|cau)[-:]/;

// Replaces everything but line breaks, so offsets and line numbers stay valid.
const blank = (match) => match.replace(/[^\n]/g, " ");

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
export function atKeys(text, pattern) {
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
    if (index === 0 && /^---\s*$/.test(line)) { frontMatter = true; return blank(line); }
    if (frontMatter) { if (/^(---|\.\.\.)\s*$/.test(line)) frontMatter = false; return blank(line); }
    const match = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (fence) {
      if (match && match[1][0] === fence.marker && match[1].length >= fence.length && !match[2].trim()) fence = null;
      return blank(line);
    }
    if (match) { fence = { marker: match[1][0], length: match[1].length }; return blank(line); }
    return line.replace(INLINE_CODE, blank);
  }).join("\n");
}

// The Org node types whose text is not citations: block begin and end lines,
// the bodies of src, example, export and comment blocks, comment lines,
// =verbatim=, ~code~ and inline source. Quote, center and verse bodies are prose.
const ORG_MASKED_NODES = new Set(["BlockBegin", "BlockEnd", "VerbatimBody", "CommentBody", "Comment", "Verbatim", "Code", "InlineSrc"]);

/**
 * The Org source with the regions that are not citations blanked: the blocks,
 * comment lines, =verbatim= and ~code~ spans and inline source that the Org
 * tree finds. Blanking keeps every offset and line break.
 * @param {string} text
 * @returns {string}
 */
export function orgMasked(text) {
  text = String(text ?? "");
  const ranges = [];
  orgTree(text).iterate({
    enter(ref) {
      if (!ORG_MASKED_NODES.has(ref.name)) return;
      ranges.push([ref.from, ref.to]);
      // The text inside a masked node is masked with it.
      return false;
    },
  });
  let out = "";
  let at = 0;
  for (const [from, to] of ranges) {
    out += text.slice(at, from) + blank(text.slice(from, to));
    at = to;
  }
  return out + text.slice(at);
}

// The keys of the citations of an Org tree: code, comments and verbatim text
// are not parsed as citations.
function orgKeys(text) {
  const found = [];
  orgTree(text).iterate({
    enter(ref) {
      if (ref.name === "CitationKey") found.push({ key: text.slice(ref.from, ref.to), from: ref.from, to: ref.to });
    },
  });
  return found;
}

/**
 * Whether the typst reference whose `@` is at `at` follows a letter or digit,
 * as in an e-mail address. Typst reads `me@example.org` as a reference; the
 * index and the citation checks leave it alone, as they always have.
 * @param {string} text
 * @param {number} at
 */
export function typstRefInWord(text, at) {
  return at > 0 && /[\p{L}\p{N}]/u.test(text[at - 1]);
}

// The citation keys of a typst tree: @key references that name no label in
// the project, and the labels of #cite(<key>) calls. Comments, raw text and
// strings are not parsed as references, so they cannot yield keys.
function typstKeys(text, labels) {
  const tree = typstTree(text);
  if (!tree) return [];
  const found = [];
  tree.iterate({
    enter(ref) {
      if (ref.name === "Ref") {
        const marker = ref.node.getChild("RefMarker");
        if (!marker || typstRefInWord(text, marker.from)) return;
        // An @ may name a label as well as a bibliography key.
        const key = text.slice(marker.from + 1, marker.to);
        if (key && !labels.has(key)) found.push({ key, from: marker.from + 1, to: marker.to });
        return;
      }
      if (ref.name !== "FuncCall") return;
      const callee = ref.node.firstChild;
      if (!callee || callee.name !== "Ident" || text.slice(callee.from, callee.to) !== "cite") return;
      for (let arg = ref.node.getChild("Args")?.firstChild; arg; arg = arg.nextSibling) {
        if (arg.name !== "Label" || arg.to - arg.from < 3) continue;
        found.push({ key: text.slice(arg.from + 1, arg.to - 1), from: arg.from + 1, to: arg.to - 1 });
      }
    },
  });
  return found;
}

/**
 * The citation keys in a source file, with the UTF-16 offsets of each key.
 * @param {string} text
 * @param {string} format "latex", "tex", "typst", "markdown", "quarto" or "org"
 * @param {{ labels?: Set<string> }} [options] typst labels, which are cross references
 * @returns {Array<{ key: string, from: number, to: number }>}
 */
export function citedKeys(text, format, { labels = new Set() } = {}) {
  text = String(text ?? "");
  let found;
  if (format === "latex" || format === "tex") {
    found = latexKeys(text);
  } else if (format === "typst") {
    found = typstKeys(text, labels);
  } else if (format === "markdown" || format === "quarto") {
    found = atKeys(markdownMasked(text), MARKDOWN_AT).filter((cite) => !CROSS_REF.test(cite.key));
  } else if (format === "org") {
    found = orgKeys(text);
  } else {
    return [];
  }
  return found.sort((a, b) => a.from - b.from);
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
