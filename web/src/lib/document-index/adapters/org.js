// The adapter for Org-mode sources. Headings come from the outline. Labels
// (<<targets>>, :CUSTOM_ID: properties and #+NAME keywords), figures, tables,
// citations, bibliography files and includes are read line by line here, so
// that nothing inside a source, example, export or comment block is reported.
// Pure: same text, same items. No DOM, no project access, so it runs in Node as
// well as the browser.

import { extractOutline } from "../../outline.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {{ first: number, last: number, name: string, lang: string }} CodeBlock */

// Blocks whose content is code or a comment: nothing in them is indexed.
const CODE_BLOCKS = new Set(["src", "example", "export", "comment"]);
const BLOCK_BEGIN = /^[ \t]*#\+begin_(\w+)(?:[ \t]+(\S+))?/i;
const BLOCK_END = /^[ \t]*#\+end_(\w+)\b/i;
const DRAWER_OPEN = /^[ \t]*:([\w-]+):[ \t]*$/;
const DRAWER_END = /^[ \t]*:END:[ \t]*$/i;
const HEADLINE = /^\*+[ \t]+/;
const KEYWORD = /^[ \t]*#\+[\w-]+(?:\[[^\]]*\])?:/;
const NAME = /^([ \t]*#\+name:[ \t]*)(\S.*?)[ \t]*$/i;
const CAPTION = /^[ \t]*#\+caption(?:\[[^\]]*\])?:[ \t]*(.*?)[ \t]*$/i;
const CUSTOM_ID = /^([ \t]*:CUSTOM_ID:[ \t]+)(\S+)[ \t]*$/i;
const INCLUDE = /^([ \t]*#\+include:[ \t]*)(.*)$/i;
const BIBLIOGRAPHY = /^([ \t]*#\+bibliography:[ \t]*)(.*)$/i;
const TABLE_ROW = /^[ \t]*\|/;
const TARGET = /<<([^>\n]+)>>/g;
const LINK = /\[\[([^[\]\n]+)\](?:\[[^[\]\n]*\])?\]/g;
const IMAGE_EXTENSION = /\.(?:png|jpe?g|gif|svg|webp|pdf)$/i;
const CITE = /\[cite(?:\/[^:\]\n]*)?:([^\]\n]*)\]/g;
const CITE_KEY = /(?:^|[\s;])@([\p{L}\p{N}_\-.:+]*[\p{L}\p{N}_+])/gu;

/**
 * Lines with their offsets. Separators are not part of any line.
 * @param {string} text
 * @returns {Array<{ from: number, to: number, text: string }>}
 */
function splitLines(text) {
  const lines = [];
  let from = 0;
  const pattern = /\r\n|\r|\n/g;
  for (const match of text.matchAll(pattern)) {
    lines.push({ from, to: match.index, text: text.slice(from, match.index) });
    from = match.index + match[0].length;
  }
  lines.push({ from, to: text.length, text: text.slice(from) });
  return lines;
}

/**
 * Code blocks, and which lines are hidden from indexing: the lines of a code
 * or comment block, delimiters included. Also the property drawer name of
 * each line inside a drawer.
 * @param {Array<{ from: number, to: number, text: string }>} lines
 * @returns {{ hidden: Uint8Array, drawerOf: string[], codeBlocks: CodeBlock[] }}
 */
function classify(lines) {
  const hidden = new Uint8Array(lines.length);
  const drawerOf = new Array(lines.length).fill("");
  /** @type {CodeBlock[]} */
  const codeBlocks = [];
  /** @type {{ name: string, line: number, lang: string, code: boolean } | null} */
  let block = null;
  let drawer = "";
  for (let i = 0; i < lines.length; i++) {
    const text = lines[i].text;
    if (block) {
      if (block.code) hidden[i] = 1;
      const end = BLOCK_END.exec(text);
      if (end && end[1].toLowerCase() === block.name) {
        if (block.code) codeBlocks.push({ first: block.line, last: i, name: block.name, lang: block.lang });
        block = null;
      }
      continue;
    }
    // A headline ends an open drawer, as Org does.
    if (HEADLINE.test(text)) drawer = "";
    if (drawer) {
      if (DRAWER_END.test(text)) drawer = "";
      else drawerOf[i] = drawer;
      continue;
    }
    const begin = BLOCK_BEGIN.exec(text);
    if (begin) {
      const name = begin[1].toLowerCase();
      block = { name, line: i, lang: begin[2] ?? "", code: CODE_BLOCKS.has(name) };
      if (block.code) hidden[i] = 1;
      continue;
    }
    if (HEADLINE.test(text)) continue;
    const open = DRAWER_OPEN.exec(text);
    if (open && open[1].toUpperCase() !== "END") drawer = open[1].toUpperCase();
  }
  // An unclosed code block runs to the end of the text.
  if (block?.code) codeBlocks.push({ first: block.line, last: lines.length - 1, name: block.name, lang: block.lang });
  return { hidden, drawerOf, codeBlocks };
}

/**
 * The range and code block items of one code block.
 * @param {Array<{ from: number, to: number, text: string }>} lines
 * @param {CodeBlock} block
 * @returns {IndexItem[]}
 */
function codeItems(lines, block) {
  const first = lines[block.first];
  const last = lines[block.last];
  /** @type {IndexItem[]} */
  const items = [{ kind: "code", range: [first.from, last.to] }];
  if (block.last > block.first) {
    items.push({
      kind: "block",
      name: "code",
      range: [first.to, last.from],
      attrs: { lang: block.lang },
    });
  }
  return items;
}

/**
 * Whitespace-separated words of a keyword value, quotes removed, each with its
 * span in the source.
 * @param {string} value
 * @param {number} offset absolute offset of value[0]
 * @returns {Array<{ text: string, from: number, to: number }>}
 */
function words(value, offset) {
  const out = [];
  for (const match of value.matchAll(/"([^"\n]*)"|(\S+)/g)) {
    const quoted = match[1] !== undefined;
    const text = quoted ? match[1] : match[2];
    const from = offset + match.index + (quoted ? 1 : 0);
    out.push({ text, from, to: from + text.length });
  }
  return out;
}

/**
 * Image links on one line: [[file:a.png]], [[./a.png]] and [[a.png][text]].
 * Only a local path to an image counts. A `::suffix` is not part of the name.
 * @param {string} text
 * @returns {Array<{ name: string, from: number, to: number, nameFrom: number, nameTo: number }>}
 */
function imageLinks(text) {
  const out = [];
  for (const match of text.matchAll(LINK)) {
    const target = match[1];
    const prefixed = /^file:/i.test(target);
    if (!prefixed && /^[A-Za-z][\w+.-]*:/.test(target)) continue;
    const raw = prefixed ? target.slice(5) : target;
    const name = raw.replace(/::.*$/, "");
    if (!IMAGE_EXTENSION.test(name)) continue;
    const nameFrom = match.index + 2 + (target.length - raw.length);
    out.push({
      name,
      from: match.index,
      to: match.index + match[0].length,
      nameFrom,
      nameTo: nameFrom + name.length,
    });
  }
  return out;
}

/**
 * The run of keyword lines (#+KEY: value) that holds line `i`, first and last
 * index inclusive. Hidden lines end a run.
 * @param {Array<{ text: string }>} lines
 * @param {Uint8Array} hidden
 * @param {number} i
 * @returns {[number, number]}
 */
function keywordRun(lines, hidden, i) {
  const keyword = (j) => j >= 0 && j < lines.length && !hidden[j] && KEYWORD.test(lines[j].text);
  let first = i;
  let last = i;
  while (keyword(first - 1)) first--;
  while (keyword(last + 1)) last++;
  return [first, last];
}

/**
 * The caption in a keyword run nearest to line `i`, or undefined.
 * @param {Array<{ text: string }>} lines
 * @param {Uint8Array} hidden
 * @param {[number, number]} run
 * @param {number} i
 * @returns {string | undefined}
 */
function captionNear(lines, hidden, [first, last], i) {
  let best;
  let bestDistance = Infinity;
  for (let j = first; j <= last; j++) {
    if (hidden[j]) continue;
    const caption = CAPTION.exec(lines[j].text);
    if (!caption || !caption[1]) continue;
    const distance = Math.abs(j - i);
    if (distance < bestDistance) {
      best = caption[1];
      bestDistance = distance;
    }
  }
  return best;
}

/**
 * The element a #+NAME run names: a table (its rows) or a figure link (its
 * line), directly below the run. Null for anything else.
 * @param {Array<{ from: number, to: number, text: string }>} lines
 * @param {Uint8Array} hidden
 * @param {number} last index of the run's last line
 * @returns {[number, number] | null}
 */
function namedElement(lines, hidden, last) {
  const next = last + 1;
  if (next >= lines.length || hidden[next]) return null;
  const text = lines[next].text;
  if (TABLE_ROW.test(text)) {
    let end = next;
    while (end + 1 < lines.length && !hidden[end + 1] && TABLE_ROW.test(lines[end + 1].text)) end++;
    return [lines[next].from, lines[end].to];
  }
  if (imageLinks(text).length) return [lines[next].from, lines[next].to];
  return null;
}

/**
 * The index items of one Org-mode source.
 * @param {string} text
 * @param {string} _path unused: the Org adapter reads only the text
 * @returns {IndexItem[]}
 */
export function orgAdapter(text, _path) {
  const source = String(text ?? "");
  const lines = splitLines(source);
  const { hidden, drawerOf, codeBlocks } = classify(lines);
  /** @type {IndexItem[]} */
  const items = [];

  // Headings, from the outline. The title of the latest one names a
  // property drawer's CUSTOM_ID.
  /** @type {Map<number, string>} */
  const titleAt = new Map();
  for (const heading of extractOutline(source, "org")) {
    const line = lines[heading.line - 1];
    titleAt.set(heading.line - 1, heading.title);
    items.push({
      kind: "heading",
      range: [heading.from, line.to],
      level: heading.level,
      title: heading.title,
    });
  }

  for (const block of codeBlocks) items.push(...codeItems(lines, block));

  let title;
  for (let i = 0; i < lines.length; i++) {
    if (hidden[i]) continue;
    const line = lines[i];
    const value = line.text;
    if (titleAt.has(i)) title = titleAt.get(i);

    const name = NAME.exec(value);
    if (name) {
      const [first, last] = keywordRun(lines, hidden, i);
      /** @type {IndexItem} */
      const label = {
        kind: "label",
        name: name[2],
        nameRange: [line.from + name[1].length, line.from + name[1].length + name[2].length],
        range: namedElement(lines, hidden, last) ?? [line.from, line.to],
      };
      const caption = captionNear(lines, hidden, [first, last], i);
      if (caption) label.title = caption;
      items.push(label);
      continue;
    }

    if (drawerOf[i] === "PROPERTIES") {
      const id = CUSTOM_ID.exec(value);
      if (id) {
        const from = line.from + id[1].length;
        /** @type {IndexItem} */
        const label = {
          kind: "label",
          name: id[2],
          nameRange: [from, from + id[2].length],
          range: [line.from, line.to],
        };
        if (title !== undefined) label.title = title;
        items.push(label);
        continue;
      }
    }

    const include = INCLUDE.exec(value);
    if (include) {
      const first = words(include[2], line.from + include[1].length)[0];
      if (first) {
        const name = first.text.replace(/::.*$/, "");
        items.push({
          kind: "include",
          name,
          nameRange: [first.from, first.from + name.length],
          range: [line.from, line.to],
        });
      }
      continue;
    }

    const bibliography = BIBLIOGRAPHY.exec(value);
    if (bibliography) {
      for (const word of words(bibliography[2], line.from + bibliography[1].length)) {
        if (word.text.startsWith(":")) break;
        items.push({
          kind: "bibliography",
          name: word.text,
          nameRange: [word.from, word.to],
          range: [line.from, line.to],
        });
      }
      continue;
    }

    for (const match of value.matchAll(TARGET)) {
      const target = match[1];
      if (!target.trim()) continue;
      const from = line.from + match.index + 2;
      items.push({
        kind: "label",
        name: target,
        nameRange: [from, from + target.length],
        range: [line.from + match.index, line.from + match.index + match[0].length],
      });
    }

    for (const link of imageLinks(value)) {
      items.push({
        kind: "asset",
        name: link.name,
        range: [line.from + link.from, line.from + link.to],
        nameRange: [line.from + link.nameFrom, line.from + link.nameTo],
      });
    }

    for (const match of value.matchAll(CITE)) {
      const body = line.from + match.index + match[0].indexOf(":") + 1;
      for (const key of match[1].matchAll(CITE_KEY)) {
        const from = body + key.index + key[0].length - key[1].length;
        items.push({
          kind: "cite",
          name: key[1],
          namespace: "cite",
          range: [from, from + key[1].length],
          nameRange: [from, from + key[1].length],
        });
      }
    }
  }

  return items.sort((a, b) => a.range[0] - b.range[0] || a.range[1] - b.range[1]);
}
