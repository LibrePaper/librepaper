// The adapter for Org-mode sources. It walks the Org syntax tree
// (org-language/module.js), so nothing inside a source, example, export or
// comment block is reported, and no regex searches the source for syntax.
// Headings come from the outline. Labels (<<targets>>, :CUSTOM_ID: properties,
// #+NAME keywords, headline titles), footnotes, internal, file and external
// links, image links, citations, bibliography files, includes and document
// metadata come from the tree. A regex below only reads the text of one node.
// Pure: same text, same items. No DOM, no project access, so it runs in Node as
// well as the browser.

import { orgTree } from "../../source-trees.js";
import { extractOutline } from "../../outline.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {import("@lezer/common").SyntaxNode} SyntaxNode */

// Blocks whose content is code or a comment: nothing in them is indexed.
const CODE_BLOCKS = new Set(["src", "example", "export", "comment"]);
const IMAGE_EXTENSION = /\.(?:png|jpe?g|gif|svg|webp|pdf)$/i;
// A URL or other scheme (`https:`, `mailto:`, `doi:`). `note: x` is not one:
// Org reads a colon followed by a blank as plain text.
const SCHEME = /^[A-Za-z][\w+.-]*:\S/;
const FILE_PREFIX = /^file:/i;
// A link target without a scheme that names a project file: a relative or
// absolute path, a path with a slash, or a file name with a known extension.
const FILE_EXTENSION = /\.(?:org|tex|md|qmd|rmd|typ|bib|txt|csv|html?|json|pdf|png|jpe?g|gif|svg|webp)$/i;
// Keyword metadata, by keyword name, and the kind each becomes.
const METADATA = new Map([
  ["TITLE", "title"],
  ["SUBTITLE", "subtitle"],
  ["AUTHOR", "author"],
  ["DATE", "date"],
  ["LANGUAGE", "language"],
]);
const MATH_ENVIRONMENT = /^(?:equation|align|alignat|flalign|gather|multline|eqnarray|displaymath|math)\*?$/;
// Inline nodes whose delimiters or markup are not text a reader sees.
const NO_TEXT = new Set(["EmphasisMark", "Target", "RadioTarget", "FootnoteReference", "Citation", "Macro"]);

/**
 * The start of the line that holds `from`, when `from` follows only blanks.
 * @param {string} text
 * @param {number} from
 * @returns {number}
 */
function lineStart(text, from) {
  while (from > 0 && (text[from - 1] === " " || text[from - 1] === "\t")) from--;
  return from;
}

/**
 * The text of a node's first child with this name, or undefined.
 * @param {string} text
 * @param {SyntaxNode} node
 * @param {string} name
 */
function childText(text, node, name) {
  const child = node.getChild(name);
  return child ? text.slice(child.from, child.to) : undefined;
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
 * The image a Link node points to, or null: [[file:a.png]], [[./a.png]] and
 * [[a.png][text]]. Only a local path to an image counts. A `::suffix` is not
 * part of the name.
 * @param {string} text
 * @param {SyntaxNode} link
 * @returns {{ name: string, nameFrom: number, nameTo: number } | null}
 */
function imageLink(text, link) {
  if (!text.startsWith("[[", link.from)) return null;
  const targetNode = link.getChild("LinkTarget");
  if (!targetNode) return null;
  const target = text.slice(targetNode.from, targetNode.to);
  const prefixed = FILE_PREFIX.test(target);
  if (!prefixed && SCHEME.test(target)) return null;
  const raw = prefixed ? target.slice(5) : target;
  const name = raw.replace(/::.*$/, "");
  if (!IMAGE_EXTENSION.test(name)) return null;
  const nameFrom = targetNode.from + (target.length - raw.length);
  return { name, nameFrom, nameTo: nameFrom + name.length };
}

/**
 * Whitespace collapsed to single spaces and trimmed: how a headline title reads
 * in a `*Title` link.
 * @param {string} text
 * @returns {string}
 */
function collapse(text) {
  return text.replace(/\s+/g, " ").trim();
}

/**
 * The text a reader sees in an inline node: markup and delimiters dropped, a
 * link shown as its description (or its target when it has none).
 * @param {string} text
 * @param {SyntaxNode} node
 * @returns {string}
 */
function plainText(text, node) {
  let out = "";
  let at = node.from;
  for (let child = node.firstChild; child; child = child.nextSibling) {
    out += text.slice(at, child.from);
    if (child.name === "Link") {
      const description = child.getChild("LinkDescription");
      const target = child.getChild("LinkTarget");
      if (description) out += plainText(text, description);
      else if (target) out += text.slice(target.from, target.to);
    } else if (!NO_TEXT.has(child.name)) {
      out += plainText(text, child);
    }
    at = child.to;
  }
  return out + text.slice(at, node.to);
}

/**
 * Whether a link target without a scheme names a file path: it starts with
 * ./ ../ / or ~/, has a slash, or ends in a known file extension.
 * @param {string} name the target before any `::search`
 */
function pathLike(name) {
  return /^(?:\.\.?\/|~\/|\/)/.test(name) || name.includes("/") || FILE_EXTENSION.test(name);
}

/**
 * The label an internal link target names, or null. Internal means `#id`,
 * `*Title` (a TODO keyword before the title is dropped) or a bare name, which
 * Org resolves to a #+NAME, a target or a headline title. A `::` search is not
 * internal, and file and scheme targets are handled before this.
 * @param {string} target the raw link target
 * @param {number} from absolute offset of target[0]
 * @param {Set<string>} todoWords the TODO keywords this file uses
 * @returns {{ name: string, from: number, to: number, heading: boolean } | null}
 */
function internalTarget(target, from, todoWords) {
  if (target.includes("::")) return null;
  const to = from + target.length;
  if (target.startsWith("#")) {
    return target.length > 1 ? { name: target.slice(1), from: from + 1, to, heading: false } : null;
  }
  if (target.startsWith("*")) {
    const words = collapse(target.slice(1)).split(" ");
    let start = from + 1;
    if (words.length > 1 && todoWords.has(words[0])) {
      // The name range starts at the title, after the keyword and its blanks.
      const keyword = /^\s*\S+\s+/.exec(target.slice(1));
      start += keyword ? keyword[0].length : 0;
      words.shift();
    }
    const name = words.join(" ");
    return name ? { name, from: start, to, heading: true } : null;
  }
  return collapse(target) ? { name: target, from, to, heading: false } : null;
}

/**
 * The keyword's upper-cased name without an `[option]`, or "".
 * @param {string} text
 * @param {SyntaxNode} keyword
 */
function keywordName(text, keyword) {
  const key = childText(text, keyword, "KeywordKey") ?? "";
  const at = key.indexOf("[");
  return (at < 0 ? key : key.slice(0, at)).toUpperCase();
}

/**
 * The run of Keyword siblings, on consecutive lines, that holds `keyword`.
 * @param {string} text
 * @param {SyntaxNode} keyword
 * @returns {SyntaxNode[]}
 */
function keywordRun(text, keyword) {
  const adjacent = (a, b) => /^(?:\r\n|\r|\n)$/.test(text.slice(a.to, b.from));
  const run = [keyword];
  for (let n = keyword.prevSibling; n && n.name === "Keyword" && adjacent(n, run[0]); n = n.prevSibling) run.unshift(n);
  for (let n = keyword.nextSibling; n && n.name === "Keyword" && adjacent(run[run.length - 1], n); n = n.nextSibling) run.push(n);
  return run;
}

/**
 * The caption in a keyword run nearest to the keyword at index `at`.
 * @param {string} text
 * @param {SyntaxNode[]} run
 * @param {number} at
 * @returns {string | undefined}
 */
function captionNear(text, run, at) {
  let best;
  let bestDistance = Infinity;
  run.forEach((keyword, j) => {
    if (keywordName(text, keyword) !== "CAPTION") return;
    const caption = childText(text, keyword, "KeywordValue");
    if (!caption) return;
    const distance = Math.abs(j - at);
    if (distance < bestDistance) {
      best = caption;
      bestDistance = distance;
    }
  });
  return best;
}

/**
 * The element a #+NAME run names, directly below the run, with its target kind
 * and its range: a table (its rows) is a table, a figure paragraph (its first
 * line) a figure, a src block a listing, a LaTeX environment an equation.
 * Null for anything else.
 * @param {string} text
 * @param {SyntaxNode} last the run's last keyword
 * @returns {{ target: string, range: [number, number] } | null}
 */
function namedElement(text, last) {
  const next = last.nextSibling;
  if (!next || !/^(?:\r\n|\r|\n)$/.test(text.slice(last.to, next.from))) return null;
  if (next.name === "LatexEnvironment") return { target: "equation", range: [next.from, next.to] };
  if (next.name === "Block") {
    const begin = next.getChild("BlockBegin");
    const block = begin ? (childText(text, begin, "BlockName") ?? "").toLowerCase() : "";
    return block === "src" ? { target: "listing", range: [next.from, next.to] } : null;
  }
  if (next.name === "Table") {
    let end = next.from;
    for (let row = next.firstChild; row; row = row.nextSibling) if (row.name === "TableRow" || row.name === "TableRule") end = row.to;
    return end > next.from ? { target: "table", range: [next.from, end] } : null;
  }
  if (next.name !== "Paragraph") return null;
  const newline = text.slice(next.from, next.to).search(/[\r\n]/);
  const to = newline < 0 ? next.to : next.from + newline;
  const cursor = next.cursor();
  do {
    if (cursor.name === "Link" && cursor.from < to && imageLink(text, cursor.node)) return { target: "figure", range: [next.from, to] };
  } while (cursor.next());
  return null;
}

/**
 * The index items of one Org-mode source.
 * @param {string} text
 * @param {string} path the file path, the key of its parse
 * @returns {IndexItem[]}
 */
export function orgAdapter(text, path) {
  const source = String(text ?? "");
  const tree = orgTree(source, path);
  if (!tree) return [];
  /** @type {IndexItem[]} */
  const items = [];

  // Headings, from the outline. The title of the latest one names a
  // property drawer's CUSTOM_ID.
  const titleAt = new Map(extractOutline(source, "org").map((heading) => [heading.from, heading.title]));
  let title;

  // The TODO keywords this file uses, so a `*TODO Title` link names the title.
  const todoWords = new Set();
  tree.iterate({
    enter(ref) {
      if (ref.name === "TodoKeyword") todoWords.add(source.slice(ref.from, ref.to));
    },
  });

  // Labels are one namespace, the one Org's internal links resolve in. A
  // :CUSTOM_ID: is named without its `#` (a `#id` link strips it), a #+NAME
  // and a <<target>> by their text, and a headline by its title as a `*Title`
  // link spells it, so a bare name that is a headline title also resolves to
  // it. Each label carries attrs.target, the kind of thing it names. A headline
  // is marked `form: "heading"`: its title is not a rename site, and its label
  // is implicit (Org wrote it, not the author). Footnotes are their own
  // namespace, "footnote", named by their bare label.

  tree.iterate({
    enter(ref) {
      const node = ref.node;
      switch (ref.name) {
        case "Headline": {
          if (titleAt.has(ref.from)) {
            title = titleAt.get(ref.from);
            const stars = node.getChild("Stars");
            items.push({
              kind: "heading",
              range: [lineStart(source, ref.from), ref.to],
              level: stars ? stars.to - stars.from : 1,
              title,
            });
            // A COMMENT headline is not a link target.
            const titleNode = node.getChild("Title");
            const headline = titleNode ? collapse(source.slice(titleNode.from, titleNode.to)) : "";
            if (headline && !/^COMMENT(?:\s|$)/.test(headline)) {
              items.push({
                kind: "label",
                name: headline,
                range: [lineStart(source, ref.from), ref.to],
                title: headline,
                attrs: { form: "heading", target: "section", implicit: "true" },
              });
            }
          }
          // The title holds targets, links and citations.
          return;
        }
        case "Block": {
          const begin = node.getChild("BlockBegin");
          const end = node.getChild("BlockEnd");
          const name = (begin ? childText(source, begin, "BlockName") ?? "" : "").toLowerCase();
          if (name === "abstract") {
            // The abstract's paragraphs, as one plain-text value. Its body is
            // walked too, so its links and footnotes are indexed.
            const body = node.getChild("BlockBody");
            const parts = [];
            for (let child = body?.firstChild; child; child = child.nextSibling) {
              if (child.name === "Paragraph") parts.push(plainText(source, child));
            }
            const value = collapse(parts.join(" "));
            if (value) items.push({ kind: "metadata", name: "abstract", title: value, range: [lineStart(source, ref.from), ref.to] });
            return;
          }
          if (!CODE_BLOCKS.has(name)) return;
          const from = lineStart(source, ref.from);
          items.push({ kind: "code", range: [from, ref.to] });
          if (begin) {
            const parameters = childText(source, begin, "BlockParameters") ?? "";
            items.push({
              kind: "block",
              name: "code",
              range: [begin.to, end ? lineStart(source, end.from) : ref.to],
              attrs: { lang: parameters.split(/\s+/)[0] },
            });
          }
          return false;
        }
        case "LatexEnvironment": {
          // A \label in a math environment labels the equation. A #+NAME that
          // already names this environment is the same label, not a second one.
          const begin = node.getChild("LatexEnvBegin");
          if (!begin || !MATH_ENVIRONMENT.test(childText(source, begin, "LatexEnvName") ?? "")) return;
          const end = node.getChild("LatexEnvEnd");
          const body = source.slice(begin.to, end ? end.from : node.to);
          for (const match of body.matchAll(/\\label\{([^{}\s]+)\}/g)) {
            const label = match[1];
            const nameFrom = begin.to + match.index + 7;
            if (items.some((item) => item.kind === "label" && item.name === label && item.range[0] === node.from && item.range[1] === node.to)) continue;
            items.push({
              kind: "label",
              name: label,
              nameRange: [nameFrom, nameFrom + label.length],
              range: [begin.to + match.index, begin.to + match.index + match[0].length],
              attrs: { target: "equation" },
            });
          }
          return;
        }
        case "Property": {
          const key = childText(source, node, "PropertyKey");
          const value = node.getChild("PropertyValue");
          if (key?.toUpperCase() !== "CUSTOM_ID" || node.parent?.name !== "PropertyDrawer" || !value) return;
          const id = source.slice(value.from, value.to);
          if (/\s/.test(id)) return;
          /** @type {IndexItem} */
          const label = {
            kind: "label",
            name: id,
            nameRange: [value.from, value.to],
            range: [lineStart(source, ref.from), ref.to],
            attrs: { target: "section" },
          };
          if (title !== undefined) label.title = title;
          items.push(label);
          return;
        }
        case "Keyword": {
          const name = keywordName(source, node);
          const valueNode = node.getChild("KeywordValue");
          if (!valueNode) return;
          const value = source.slice(valueNode.from, valueNode.to);
          if (name === "NAME") {
            const run = keywordRun(source, node);
            const element = namedElement(source, run[run.length - 1]);
            /** @type {IndexItem} */
            const label = {
              kind: "label",
              name: value,
              nameRange: [valueNode.from, valueNode.to],
              range: element?.range ?? [lineStart(source, ref.from), ref.to],
              attrs: { target: element?.target ?? "other" },
            };
            const caption = captionNear(source, run, run.findIndex((keyword) => keyword.from === ref.from));
            if (caption) label.title = caption;
            items.push(label);
          } else if (name === "INCLUDE") {
            const first = words(value, valueNode.from)[0];
            if (first) {
              const file = first.text.replace(/::.*$/, "");
              items.push({
                kind: "include",
                name: file,
                nameRange: [first.from, first.from + file.length],
                range: [lineStart(source, ref.from), ref.to],
              });
            }
          } else if (name === "BIBLIOGRAPHY") {
            for (const word of words(value, valueNode.from)) {
              if (word.text.startsWith(":")) break;
              items.push({
                kind: "bibliography",
                name: word.text,
                nameRange: [word.from, word.to],
                range: [lineStart(source, ref.from), ref.to],
              });
            }
          } else if (METADATA.has(name)) {
            const metadata = collapse(plainText(source, valueNode));
            if (metadata) {
              items.push({
                kind: "metadata",
                name: METADATA.get(name),
                title: metadata,
                range: [lineStart(source, ref.from), ref.to],
                nameRange: [valueNode.from, valueNode.to],
              });
            }
          }
          // A caption and a metadata value hold inline nodes, so they are walked
          // for targets and links.
          return name === "CAPTION" || METADATA.has(name) ? undefined : false;
        }
        case "Target":
        case "RadioTarget": {
          // <<x>> and <<<x>>>: the delimiters differ in length only.
          const open = ref.name === "RadioTarget" ? 3 : 2;
          const target = source.slice(ref.from + open, ref.to - open);
          if (!target.trim()) return;
          items.push({
            kind: "label",
            name: target,
            nameRange: [ref.from + open, ref.to - open],
            range: [ref.from, ref.to],
            attrs: { target: "other" },
          });
          return;
        }
        case "FootnoteDefinition": {
          const label = node.getChild("FootnoteLabel");
          if (label && label.to > label.from) {
            items.push({
              kind: "label",
              namespace: "footnote",
              name: source.slice(label.from, label.to),
              nameRange: [label.from, label.to],
              range: [ref.from, ref.to],
              attrs: { target: "footnote" },
            });
          }
          return;
        }
        case "FootnoteReference": {
          const label = node.getChild("FootnoteLabel");
          // [fn::text] has no label. [fn:x:text] defines x, and [fn:x] refers to it.
          // The inline text of a definition is walked for links and targets.
          if (!label || label.to === label.from) return;
          const name = source.slice(label.from, label.to);
          /** @type {[number, number]} */
          const nameRange = [label.from, label.to];
          /** @type {[number, number]} */
          const range = [ref.from, ref.to];
          items.push(
            source[label.to] === ":"
              ? { kind: "label", namespace: "footnote", name, nameRange, range, attrs: { target: "footnote", inline: "true" } }
              : { kind: "ref", namespace: "footnote", name, nameRange, range },
          );
          return;
        }
        case "Link": {
          const image = imageLink(source, node);
          if (image) {
            items.push({
              kind: "asset",
              name: image.name,
              range: [ref.from, ref.to],
              nameRange: [image.nameFrom, image.nameTo],
            });
            return;
          }
          const targetNode = node.getChild("LinkTarget");
          if (!targetNode) return;
          const target = source.slice(targetNode.from, targetNode.to);
          const bracketed = source.startsWith("[[", ref.from);
          // A file: link, or a bracketed path to a project file. A `::search`
          // after the name is kept as the fragment.
          const prefix = FILE_PREFIX.test(target) ? 5 : 0;
          // The scheme test reads the part before any `::search`: "notes.org::x" is a path, not a "notes.org:" URL.
          const beforeSearch = target.replace(/::.*$/, "");
          if (prefix || (bracketed && !SCHEME.test(beforeSearch) && !/^[#*]/.test(target) && pathLike(beforeSearch))) {
            const body = target.slice(prefix);
            const at = body.indexOf("::");
            const name = at < 0 ? body : body.slice(0, at);
            if (!name) return;
            const from = targetNode.from + prefix;
            items.push({
              kind: "link",
              name,
              nameRange: [from, from + name.length],
              range: [ref.from, ref.to],
              attrs: at < 0 ? { file: "true" } : { file: "true", fragment: body.slice(at + 2) },
            });
            return;
          }
          // A URL or other scheme, bracketed or plain. An id: link names an Org
          // ID, not a URL, so it is not a link item.
          if (SCHEME.test(target)) {
            if (!/^id:/i.test(target)) {
              items.push({
                kind: "link",
                name: target,
                nameRange: [targetNode.from, targetNode.to],
                range: [ref.from, ref.to],
                attrs: { external: "true" },
              });
            }
            return;
          }
          if (!bracketed) return;
          const link = internalTarget(target, targetNode.from, todoWords);
          if (link) {
            items.push({
              kind: "ref",
              name: link.name,
              namespace: "label",
              range: [ref.from, ref.to],
              nameRange: [link.from, link.to],
              ...(link.heading ? { attrs: { form: "heading" } } : {}),
            });
          }
          return;
        }
        case "Citation": {
          for (let key = node.firstChild; key; key = key.nextSibling) {
            if (key.name !== "CitationKey") continue;
            items.push({
              kind: "cite",
              name: source.slice(key.from, key.to),
              namespace: "cite",
              range: [key.from, key.to],
              nameRange: [key.from, key.to],
            });
          }
          return false;
        }
      }
    },
  });

  return items.sort((a, b) => a.range[0] - b.range[0] || a.range[1] - b.range[1]);
}
