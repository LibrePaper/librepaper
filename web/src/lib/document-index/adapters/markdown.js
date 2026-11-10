// Index items for Markdown and Quarto sources. Math, inline and indented code,
// citations, cross references, fenced divs and attribute ids come from the
// Pandoc syntax tree, so code, math and addresses never read as citations.
// Headings and spellcheck skips come from the existing scanners; fences,
// executable cells, includes, assets and bibliographies are scanned here.
// Quarto mode (".qmd") adds cross-references, executable-cell labels and
// includes. Plain Markdown emits no references: its "@key" forms are citations.

import { extractOutline } from "../../outline.js";
import { frontMatterRange, markdownTree } from "../../source-trees.js";
import { skipRanges } from "../../spellcheck/index.js";
import {
  CELL_LABEL,
  HEADER_LABEL,
  KINDS,
  LABEL,
  fenceStates,
} from "../../quarto-labels.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {{ from: number, to: number, text: string }} Line */
/** @typedef {{ marker: string, length: number, info: string }} Fence */
/** @typedef {{ first: number, last: number, fence: Fence, closed: boolean }} FenceRun */

const LINE_BREAK = /\r\n|\n|\r/g;
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;
const OPTION_LINE = /^#\|/;
const INLINE_CODE = /(`+)(?!`)[^`\n]*?(?<!`)\1(?!`)/g;
const IMAGE = /!\[([^\]\n]*)\]\((<[^>\n]*>|[^)\s]*)(?:[ \t]+(?:"[^"\n]*"|'[^'\n]*'))?\)/g;
const INCLUDE = /\{\{<\s*include\s+([^\s>]+)\s*>\}\}/g;
const URL_SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;
const OPTION_REF = new RegExp(`(?<![\\w@])@((?:${KINDS.join("|")})-[\\w-]*\\w)`, "g");
const REF_NAME = new RegExp(`^(?:${KINDS.join("|")})-[\\w-]*\\w$`);
const CELL_CAPTION = /^#\|\s*(?:fig|tbl)-cap:\s*(.*?)\s*$/;
const IMAGE_BEFORE = /!\[([^\]\n]*)\]\([^()\n]*\)\s*$/;
const TABLE_CAPTION_BEFORE = /^\s{0,3}:\s+(.*?)\s*$/;
const HEADING_ATTRIBUTE = /\s*\{[^{}]*\}\s*$/;
const WWW = /^www\./i;
const ANCHOR_OR_ABSOLUTE = /^[#/]/;
const LABEL_PREFIX = /^([a-z]+)-/;

// The target of a label by its Quarto prefix. KINDS lists the kinds Quarto
// numbers; sol and rem are theorem-like as well.
const TARGET_OF_PREFIX = {
  sec: "section",
  fig: "figure",
  tbl: "table",
  eq: "equation",
  lst: "listing",
  thm: "theorem",
  lem: "theorem",
  cor: "theorem",
  prp: "theorem",
  cnj: "theorem",
  def: "theorem",
  exm: "theorem",
  exr: "theorem",
  sol: "theorem",
  rem: "theorem",
};

const METADATA_NAMES = { title: "title", subtitle: "subtitle", author: "author", date: "date", lang: "language", abstract: "abstract" };
const TOP_LEVEL_KEY = /^([A-Za-z_][\w-]*):(?=\s|$)/;
const CONTINUATION = /^(?:[ \t-]|\s*$)/;
const LIST_ENTRY = /^\s*-(?:[ \t]|$)/;

/**
 * Lines with their offsets. Separators are not part of any line.
 * @param {string} text
 * @returns {Line[]}
 */
function splitLines(text) {
  /** @type {Line[]} */
  const lines = [];
  let from = 0;
  for (const match of text.matchAll(LINE_BREAK)) {
    lines.push({ from, to: match.index, text: text.slice(from, match.index) });
    from = match.index + match[0].length;
  }
  lines.push({ from, to: text.length, text: text.slice(from) });
  return lines;
}

/** @param {string} value @returns {string} same length, line breaks kept */
const blankOf = (value) => value.replace(/[^\r\n]/g, " ");

/**
 * Blanks sorted, non-overlapping [from, to) ranges, keeping every offset.
 * @param {string} text
 * @param {Array<[number, number]>} ranges
 * @returns {string}
 */
function blankRanges(text, ranges) {
  let out = "";
  let cursor = 0;
  for (const [from, to] of ranges) {
    out += text.slice(cursor, from) + blankOf(text.slice(from, to));
    cursor = to;
  }
  return out + text.slice(cursor);
}

/**
 * Joins per-line texts back into one string, copying the original separators.
 * @param {string[]} texts one per line, each as long as its line
 * @param {Line[]} lines
 * @param {string} source
 * @returns {string}
 */
function joinLines(texts, lines, source) {
  let out = "";
  lines.forEach((line, index) => {
    out += texts[index];
    if (index + 1 < lines.length) out += source.slice(line.to, lines[index + 1].from);
  });
  return out;
}

/**
 * The line index of the closing front matter delimiter, or -1.
 * @param {Line[]} lines
 * @param {[number, number] | null} range from frontMatterRange
 * @returns {number}
 */
function frontMatterEnd(lines, range) {
  return range ? lines.findIndex((line) => line.to === range[1]) : -1;
}

/**
 * Runs of lines sharing one fence state, from fenceStates.
 * @param {Line[]} lines
 * @param {Array<Fence | null>} states for lines from `offset` on
 * @param {number} offset
 * @returns {FenceRun[]}
 */
function fenceRuns(lines, states, offset) {
  /** @type {FenceRun[]} */
  const runs = [];
  for (let k = 0; k < states.length; ) {
    const state = states[k];
    if (!state) {
      k++;
      continue;
    }
    let last = k;
    while (last + 1 < states.length && states[last + 1] === state) last++;
    const closing = FENCE.exec(lines[offset + last].text);
    const closed =
      last > k &&
      closing !== null &&
      closing[1][0] === state.marker &&
      closing[1].length >= state.length &&
      !closing[2].trim();
    runs.push({ first: offset + k, last: offset + last, fence: state, closed });
    k = last + 1;
  }
  return runs;
}

/**
 * File names in a bibliography value, with the line each one sits on.
 * Handles a scalar, an inline list and quoted names.
 * @param {string} raw
 * @param {number} offset absolute offset of raw[0]
 * @param {Line} line
 * @returns {IndexItem[]}
 */
function bibliographyItems(raw, offset, line) {
  /** @type {IndexItem[]} */
  const items = [];
  for (const match of raw.matchAll(/[^,[\]]+/g)) {
    const segment = match[0];
    const lead = segment.length - segment.trimStart().length;
    const trimmed = segment.trim();
    const quoted = trimmed.length >= 2 && /^(["']).*\1$/.test(trimmed);
    const name = quoted ? trimmed.slice(1, -1) : trimmed;
    if (!name) continue;
    const from = offset + match.index + lead + (quoted ? 1 : 0);
    items.push({
      kind: "bibliography",
      name,
      nameRange: [from, from + name.length],
      range: [line.from, line.to],
    });
  }
  return items;
}

/**
 * Front matter items: the block, the code, and each bibliography file.
 * @param {Line[]} lines
 * @param {number} end index of the closing delimiter line
 * @returns {IndexItem[]}
 */
function frontMatterItems(lines, end) {
  const first = lines[0];
  const last = lines[end];
  /** @type {IndexItem[]} */
  const items = [
    { kind: "block", name: "front-matter", range: [first.to, last.from] },
    { kind: "code", range: [first.from, last.to] },
  ];
  for (let i = 1; i < end; i++) {
    const key = /^bibliography:[ \t]*/.exec(lines[i].text);
    if (!key) continue;
    const value = lines[i].text.slice(key[0].length).replace(/[ \t]+$/, "");
    if (value) {
      items.push(...bibliographyItems(value, lines[i].from + key[0].length, lines[i]));
      continue;
    }
    // A block sequence of file names under the key.
    for (let j = i + 1; j < end; j++) {
      const dash = /^[ \t]+-[ \t]+/.exec(lines[j].text);
      if (!dash) break;
      const item = lines[j].text.slice(dash[0].length).replace(/[ \t]+$/, "");
      items.push(...bibliographyItems(item, lines[j].from + dash[0].length, lines[j]));
    }
  }
  items.push(...metadataItems(lines, end));
  return items;
}

/**
 * A scalar as written, without quotes or a trailing comment, with the span of
 * its text. `offset` is the absolute offset of raw[0].
 * @param {string} raw
 * @param {number} offset
 * @returns {{ value: string, from: number, to: number }}
 */
function scalarSpan(raw, offset) {
  const lead = raw.length - raw.trimStart().length;
  let value = raw.trim();
  if (!/^["']/.test(value)) value = value.replace(/[ \t]+#.*$/, "");
  let from = offset + lead;
  if (value.length >= 2 && /^(["']).*\1$/.test(value)) {
    value = value.slice(1, -1);
    from += 1;
  }
  return { value, from, to: from + value.length };
}

/**
 * Metadata items for one front matter key. Handles a scalar, an inline list,
 * a block scalar, a block list (plain or `name:` entries), a `name:` mapping,
 * and any other indented text, joined into one value.
 * @param {string} name the metadata item name
 * @param {Line} line the key's line
 * @param {Line[]} body the indented lines under the key
 * @returns {IndexItem[]}
 */
function valueItems(name, line, body) {
  const colon = line.text.indexOf(":");
  let p = colon + 1;
  while (p < line.text.length && (line.text[p] === " " || line.text[p] === "\t")) p++;
  const rest = line.text.slice(p).replace(/[ \t]+$/, "");
  const offset = line.from + p;
  const filled = body.filter((entry) => entry.text.trim());
  const last = body.at(-1) ?? line;

  /** @param {string} value @param {{ from: number, to: number } | null} span */
  const item = (value, span, range = [line.from, line.to]) => {
    if (!value) return [];
    /** @type {IndexItem} */
    const found = { kind: "metadata", name, title: value, range };
    if (span) found.nameRange = [span.from, span.to];
    return [found];
  };

  if (rest.startsWith("[")) {
    const inner = rest.endsWith("]") ? rest.slice(1, -1) : rest.slice(1);
    return [...inner.matchAll(/[^,]+/g)].flatMap((match) => {
      const span = scalarSpan(match[0], offset + 1 + match.index);
      return item(span.value, span);
    });
  }
  if (/^[|>][-+\d]*$/.test(rest)) {
    const value = filled.map((entry) => entry.text.trim()).join(" ");
    return item(value, null, [line.from, last.to]);
  }
  if (rest) {
    const span = scalarSpan(rest, offset);
    return item(span.value, span);
  }
  if (!filled.length) return [];

  const first = filled[0].text;
  if (LIST_ENTRY.test(first)) {
    // A block list. Only authors are lists: each entry is a name, or a `name:` mapping.
    if (name !== "author") return [];
    return body.filter((entry) => LIST_ENTRY.test(entry.text)).flatMap((entry) => {
      const dash = /^\s*-[ \t]*/.exec(entry.text)[0];
      let start = entry.from + dash.length;
      let content = entry.text.slice(dash.length);
      const named = /^name:/.exec(content);
      if (named) {
        content = content.slice(named[0].length);
        start += named[0].length;
      }
      const span = scalarSpan(content, start);
      return item(span.value, span, [entry.from, entry.to]);
    });
  }
  if (/^\s*[A-Za-z_][\w-]*:/.test(first)) {
    // A mapping under the key: only an author's `name:` is read.
    const named = filled.find((entry) => /^\s*name:/.test(entry.text));
    if (name !== "author" || !named) return [];
    const at = named.text.indexOf("name:") + "name:".length;
    const span = scalarSpan(named.text.slice(at), named.from + at);
    return item(span.value, span, [named.from, named.to]);
  }
  // Indented text under the key, such as a multi-line plain scalar.
  const value = filled.map((entry) => entry.text.trim()).join(" ");
  return item(value, null, [line.from, last.to]);
}

/**
 * Metadata items from the YAML front matter lines between the delimiters. A
 * small line reader: top-level keys with their indented continuation lines.
 * @param {Line[]} lines
 * @param {number} end index of the closing delimiter line
 * @returns {IndexItem[]}
 */
function metadataItems(lines, end) {
  /** @type {IndexItem[]} */
  const items = [];
  let i = 1;
  while (i < end) {
    let last = i;
    while (last + 1 < end && CONTINUATION.test(lines[last + 1].text)) last++;
    const key = TOP_LEVEL_KEY.exec(lines[i].text);
    if (key && Object.hasOwn(METADATA_NAMES, key[1])) {
      items.push(...valueItems(METADATA_NAMES[key[1]], lines[i], lines.slice(i + 1, last + 1)));
    }
    i = last + 1;
  }
  return items;
}

/**
 * The label of an executable cell: its `#| label:` option, else the chunk
 * header's `label=`, as quartoLabels reads them. Captions come from `#|`.
 * @param {Line[]} lines
 * @param {FenceRun} cell
 * @returns {IndexItem[]}
 */
function cellItems(lines, cell) {
  let option = null;
  let caption = "";
  for (let i = cell.first + 1; i <= cell.last; i++) {
    const line = lines[i];
    const match = CELL_LABEL.exec(line.text);
    if (match && !option && LABEL.test(match[1])) {
      option = {
        name: match[1],
        from: line.from + match.index + match[0].length - match[1].length,
        line,
      };
    }
    const cap = CELL_CAPTION.exec(line.text);
    if (cap && !caption) caption = unquote(cap[1]);
  }
  let target = option;
  if (!target) {
    const head = lines[cell.first];
    const header = HEADER_LABEL.exec(head.text);
    if (header && LABEL.test(header[1])) {
      target = {
        name: header[1],
        from: head.from + header.index + header[0].length - header[1].length,
        line: head,
      };
    }
  }
  if (!target) return [];
  /** @type {IndexItem} */
  const item = {
    kind: "label",
    name: target.name,
    nameRange: [target.from, target.from + target.name.length],
    range: [target.line.from, target.line.to],
  };
  if (caption) item.title = caption;
  item.attrs = { target: targetOf(target.name, false) };
  return [item];
}

/** @param {string} value @returns {string} */
function unquote(value) {
  const match = /^(["'])(.*)\1$/.exec(value);
  return match ? match[2] : value;
}

/**
 * Title shown for a label: the heading on its line, else the image or table
 * caption before its attribute block.
 * @param {string} shown the label's line, with code blanked
 * @param {number} index offset of the attribute block within `shown`
 * @param {{ title: string } | undefined} heading
 * @returns {string}
 */
function labelTitle(shown, index, heading) {
  if (heading) return heading.title.replace(HEADING_ATTRIBUTE, "").trim();
  const before = shown.slice(0, index);
  const image = IMAGE_BEFORE.exec(before);
  if (image) return image[1].trim();
  const table = TABLE_CAPTION_BEFORE.exec(before);
  if (table) return table[1].trim();
  return "";
}

/** The index of the line holding `offset`. @param {Line[]} lines @param {number} offset */
function lineAt(lines, offset) {
  let low = 0;
  let high = lines.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if (lines[mid].from <= offset) low = mid;
    else high = mid - 1;
  }
  return low;
}

/**
 * The kind a label names: its Quarto prefix decides; otherwise a heading's id
 * is a section, and anything else is other.
 * @param {string} name
 * @param {boolean} heading whether the label sits on a heading line
 * @returns {string}
 */
function targetOf(name, heading) {
  const prefix = LABEL_PREFIX.exec(name)?.[1];
  if (prefix && Object.hasOwn(TARGET_OF_PREFIX, prefix)) return TARGET_OF_PREFIX[prefix];
  return heading ? "section" : "other";
}

/**
 * A link item for one destination, or null. A destination with a scheme, or
 * a bare URL that starts with www., is external. A relative path is a file
 * link, with any #fragment kept apart. Anchors, absolute paths and addresses
 * without a scheme are not links to project files.
 * @param {string} raw the destination as written
 * @param {number} from absolute offset of raw[0]
 * @param {[number, number]} range span of the whole link
 * @param {boolean} bare the destination is a bare URL or an autolink, not `(...)`
 * @returns {IndexItem | null}
 */
function linkItem(raw, from, range, bare) {
  let target = raw;
  let start = from;
  if (target.startsWith("<") && target.endsWith(">")) {
    target = target.slice(1, -1);
    start += 1;
  }
  if (!target) return null;
  if (URL_SCHEME.test(target) || target.startsWith("//") || WWW.test(target)) {
    return { kind: "link", name: target, nameRange: [start, start + target.length], range, attrs: { external: "true" } };
  }
  if (bare || ANCHOR_OR_ABSOLUTE.test(target)) return null;
  const hash = target.indexOf("#");
  const path = hash < 0 ? target : target.slice(0, hash);
  if (!path) return null;
  // The file on disk has the decoded name, as image paths do.
  let name = path;
  try {
    name = decodeURIComponent(path);
  } catch {
    // A stray % is part of the name.
  }
  /** @type {Record<string, string>} */
  const attrs = { file: "true" };
  if (hash >= 0) attrs.fragment = target.slice(hash + 1);
  return { kind: "link", name, nameRange: [start, start + path.length], range, attrs };
}

/**
 * What the Pandoc syntax tree knows: math spans, code spans, fenced divs,
 * attribute ids, citation keys and cross references, all with absolute offsets.
 * The shared tree skips the front matter and keeps every offset.
 * @param {string} source
 * @param {Line[]} lines
 * @param {string} path
 */
function treeFacts(source, lines, path) {
  const { tree, body } = markdownTree(source, path);
  /** @type {Array<[number, number]>} */
  const math = [];
  /** @type {Array<[number, number]>} */
  const code = [];
  /** @type {Array<{ from: number, to: number, cls: string }>} */
  const divs = [];
  /** @type {Array<{ name: string, from: number, attributes: [number, number] }>} */
  const ids = [];
  /** @type {Array<{ name: string, from: number, to: number }>} */
  const cites = [];
  /** @type {Array<{ name: string, from: number, to: number }>} */
  const refs = [];
  /** @type {IndexItem[]} */
  const links = [];
  /** @type {Array<{ name: string, from: number, to: number }>} */
  const noteRefs = [];
  /** @type {Array<{ name: string, from: number, to: number }>} */
  const noteDefs = [];
  // Absolute offsets of URL nodes already read as a link's destination or an autolink.
  /** @type {Set<number>} */
  const handled = new Set();
  tree.iterate({
    enter(node) {
      switch (node.name) {
        case "Link": {
          // The destination follows the "(" mark, after the link text, which may hold a bare URL.
          let url = null;
          for (let child = node.node.lastChild; child; child = child.prevSibling) {
            if (child.name === "URL" && child.prevSibling?.name === "LinkMark" && body[child.prevSibling.from] === "(") {
              url = child;
              break;
            }
          }
          if (url) {
            handled.add(url.from);
            const item = linkItem(body.slice(url.from, url.to), url.from, [node.from, node.to], false);
            if (item) links.push(item);
          }
          return true;
        }
        case "Autolink": {
          const url = node.node.getChild("URL");
          if (url) {
            handled.add(url.from);
            const item = linkItem(body.slice(url.from, url.to), url.from, [node.from, node.to], true);
            if (item) links.push(item);
          }
          return false;
        }
        case "URL": {
          // A bare URL, from the GFM autolink extension. Image sources are assets.
          if (handled.has(node.from) || node.node.parent?.name === "Image") return false;
          const item = linkItem(body.slice(node.from, node.to), node.from, [node.from, node.to], true);
          if (item) links.push(item);
          return false;
        }
        case "FootnoteReference":
          noteRefs.push({ name: body.slice(node.from + 2, node.to - 1), from: node.from, to: node.to });
          return false;
        case "FootnoteLabel":
          if (node.node.parent?.name === "FootnoteDefinition") {
            noteDefs.push({ name: body.slice(node.from + 2, node.to - 1), from: node.from, to: node.to });
          }
          return false;
        case "InlineMath":
          math.push([node.from, node.to]);
          return false;
        case "DisplayMath": {
          // The span ends at the closing $$, before any attribute list, whose id is still a label.
          let to = node.to;
          for (let child = node.node.lastChild; child; child = child.prevSibling) {
            if (child.name === "DisplayMathMark") {
              to = child.to;
              break;
            }
          }
          math.push([node.from, to]);
          return true;
        }
        case "InlineCode":
        case "CodeBlock":
          code.push([node.from, node.to]);
          return false;
        case "FencedCode":
          return false;
        case "FencedDiv": {
          const first = node.node.firstChild;
          const last = node.node.lastChild;
          const open = body.indexOf("\n", node.from);
          const closed = last && last !== first && last.name === "FencedDivMark";
          const attributes = node.node.getChild("Attributes");
          const cls = attributes?.getChild("AttributeClass");
          divs.push({
            from: open < 0 || open > node.to ? node.to : open,
            to: closed ? lines[lineAt(lines, last.from)].from : source.length,
            cls: cls ? body.slice(body[cls.from] === "." ? cls.from + 1 : cls.from, cls.to) : "",
          });
          return true;
        }
        case "AttributeId": {
          const attributes = node.node.parent;
          if (attributes?.name === "Attributes") {
            ids.push({ name: body.slice(node.from + 1, node.to), from: node.from, attributes: [attributes.from, attributes.to] });
          }
          return false;
        }
        case "CitationKey":
        case "CrossRef": {
          const entry = { name: body.slice(node.from + 1, node.to), from: node.from, to: node.to };
          (node.name === "CitationKey" ? cites : refs).push(entry);
          return false;
        }
        default:
          return true;
      }
    },
  });

  return { math, code, divs, ids, cites, refs, links, noteRefs, noteDefs };
}

/**
 * The index items of one Markdown or Quarto source.
 * @param {string} text
 * @param {string} path a ".qmd" path (any case) is Quarto; anything else is plain Markdown
 * @returns {IndexItem[]}
 */
export function markdownAdapter(text, path) {
  const source = String(text ?? "");
  const quarto = /\.qmd$/i.test(String(path ?? ""));
  const format = quarto ? "quarto" : "markdown";
  const lines = splitLines(source);
  /** @type {IndexItem[]} */
  const items = [];

  const frontEnd = frontMatterEnd(lines, frontMatterRange(source));
  const bodyStart = frontEnd + 1;
  const states = fenceStates(lines.slice(bodyStart).map((line) => line.text));
  const runs = fenceRuns(lines, states, bodyStart);
  const cells = quarto ? runs.filter((run) => run.fence.info.startsWith("{")) : [];

  // Lines that are code: front matter and fences. Cell option lines are code
  // too, but Quarto reads them as Markdown for references and captions.
  const isCode = new Uint8Array(lines.length);
  for (let i = 0; i <= frontEnd; i++) isCode[i] = 1;
  for (const run of runs) for (let i = run.first; i <= run.last; i++) isCode[i] = 1;
  const isOption = new Uint8Array(lines.length);
  for (const cell of cells) {
    for (let i = cell.first + 1; i <= cell.last; i++) {
      if (OPTION_LINE.test(lines[i].text)) isOption[i] = 1;
    }
  }

  // visible: code blanked, offsets kept. labelText: option lines blanked too,
  // so a title is read from the label's line without a cell option. prose: math blanked too.
  const visibleLines = lines.map((line, i) =>
    isCode[i] && !isOption[i] ? blankOf(line.text) : line.text.replace(INLINE_CODE, blankOf),
  );
  const visible = joinLines(visibleLines, lines, source);
  const labelText = joinLines(
    visibleLines.map((value, i) => (isOption[i] ? blankOf(value) : value)),
    lines,
    source,
  );
  const tree = treeFacts(source, lines, String(path ?? ""));
  const prose = blankRanges(visible, tree.math);

  // Headings, from the existing outline scanner.
  /** @type {Map<number, { title: string }>} */
  const headingAt = new Map();
  for (const heading of extractOutline(source, format)) {
    headingAt.set(heading.line, heading);
    items.push({
      kind: "heading",
      range: [heading.from, lines[heading.line - 1].to],
      level: heading.level,
      title: heading.title,
    });
  }

  // Front matter, then fenced code and executable cells.
  if (frontEnd >= 0) items.push(...frontMatterItems(lines, frontEnd));
  for (const run of runs) {
    const first = lines[run.first];
    const last = lines[run.last];
    items.push({ kind: "code", range: [first.from, run.closed ? last.to : source.length] });
    if (run.last > run.first) {
      const lang = /^\{?([\w-]*)/.exec(run.fence.info)?.[1] ?? "";
      items.push({
        kind: "block",
        name: "code",
        range: [first.to, run.closed ? last.from : source.length],
        attrs: { lang },
      });
    }
  }
  for (const cell of cells) items.push(...cellItems(lines, cell));

  // Fenced divs, from the tree.
  for (const div of tree.divs) {
    items.push({
      kind: "block",
      name: "div",
      range: [div.from, div.to],
      attrs: { class: div.cls },
    });
  }

  // Labels: explicit attribute ids outside code.
  for (const id of tree.ids) {
    if (quarto && !LABEL.test(id.name)) continue;
    const line = lines[lineAt(lines, id.from)];
    const shown = labelText.slice(line.from, line.to);
    /** @type {IndexItem} */
    const item = {
      kind: "label",
      name: id.name,
      nameRange: [id.from + 1, id.from + 1 + id.name.length],
      range: [id.attributes[0], id.attributes[1]],
    };
    const heading = headingAt.get(lineAt(lines, id.from) + 1);
    const title = labelTitle(shown, id.attributes[0] - line.from, heading);
    if (title) item.title = title;
    item.attrs = { target: targetOf(id.name, heading !== undefined) };
    items.push(item);
  }

  // Math.
  for (const [from, to] of tree.math) items.push({ kind: "math", range: [from, to] });
  for (const [from, to] of tree.code) items.push({ kind: "code", range: [from, to] });

  // References (Quarto only), includes (Quarto only), and image assets.
  if (quarto) {
    for (const ref of tree.refs) {
      if (!REF_NAME.test(ref.name)) continue;
      items.push({
        kind: "ref",
        namespace: "label",
        name: ref.name,
        range: [ref.from, ref.to],
        nameRange: [ref.from + 1, ref.to],
      });
    }
    // Cell options are code, but Quarto reads them as Markdown for references.
    lines.forEach((line, i) => {
      if (!isOption[i]) return;
      for (const match of line.text.matchAll(OPTION_REF)) {
        const from = line.from + match.index;
        items.push({
          kind: "ref",
          namespace: "label",
          name: match[1],
          range: [from, from + match[0].length],
          nameRange: [from + 1, from + match[0].length],
        });
      }
    });
    for (const match of prose.matchAll(INCLUDE)) {
      const head = match[0].replace(/\s*>\}\}$/, "");
      let to = match.index + head.length;
      // Quarto allows the path in matching quotes; the name excludes them.
      const quoted = match[1].length >= 2 && /^(["']).*\1$/.test(match[1]);
      const name = quoted ? match[1].slice(1, -1) : match[1];
      if (quoted) to -= 1;
      const from = to - name.length;
      items.push({
        kind: "include",
        name,
        range: [match.index, match.index + match[0].length],
        nameRange: [from, to],
      });
    }
  }
  for (const match of prose.matchAll(IMAGE)) {
    const angled = match[2].startsWith("<");
    const target = angled ? match[2].slice(1, -1) : match[2];
    const rawName = target.replace(/[?#].*$/, "");
    if (!rawName || target.startsWith("#") || target.startsWith("//") || URL_SCHEME.test(target)) continue;
    // The file on disk has the decoded name: my%20fig.png is "my fig.png".
    let name = rawName;
    try {
      name = decodeURIComponent(rawName);
    } catch {
      // A stray % is part of the name.
    }
    const from = match.index + match[0].indexOf("](") + 2 + (angled ? 1 : 0);
    items.push({
      kind: "asset",
      name,
      range: [match.index, match.index + match[0].length],
      nameRange: [from, from + rawName.length],
    });
  }

  // Footnotes, in their own namespace; an inline note (^[...]) has no label and emits nothing.
  for (const note of tree.noteDefs) {
    const line = lines[lineAt(lines, note.from)];
    items.push({
      kind: "label",
      namespace: "footnote",
      name: note.name,
      nameRange: [note.from + 2, note.to - 1],
      range: [line.from, line.to],
      attrs: { target: "footnote" },
    });
  }
  for (const note of tree.noteRefs) {
    items.push({
      kind: "ref",
      namespace: "footnote",
      name: note.name,
      nameRange: [note.from + 2, note.to - 1],
      range: [note.from, note.to],
    });
  }
  items.push(...tree.links);

  // Citations and spellcheck skips, from the existing scanners.
  for (const cite of tree.cites) {
    items.push({
      kind: "cite",
      name: cite.name,
      namespace: "cite",
      range: [cite.from + 1, cite.to],
      nameRange: [cite.from + 1, cite.to],
    });
  }
  for (const [from, to] of skipRanges(source, format)) {
    items.push({ kind: "markup", range: [from, to], attrs: { prose: "skip" } });
  }

  return items;
}
