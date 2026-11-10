// The HTML adapter. It maps one HTML source to the editor's document index:
// headings come from the outline, prose skips from the spellchecker, and the
// names that cross files (ids, fragment links, local files) come from the tags.
//
// Tags are scanned here, not by the outline, so that comments and the bodies
// of script and style are never read as markup. Pure: same text, same items.
// No DOM, no project access, so it runs in Node as well as the browser.

import { extractOutline } from "../../outline.js";
import { asciiLower, rawEnd } from "../../spellcheck/html.js";
import { skipRanges } from "../../spellcheck/index.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */

/**
 * One attribute of a start tag. `from` and `to` span the whole attribute
 * (name, `=` and value). `valueFrom` and `valueTo` span the value without its
 * quotes. A boolean attribute has `value: null`.
 * @typedef {{
 *   name: string,
 *   value: string | null,
 *   from: number,
 *   to: number,
 *   valueFrom: number,
 *   valueTo: number,
 * }} Attr
 */

/**
 * A start or end tag. `from` is its `<` and `end` the index past its `>`.
 * Paired tags link to each other: a start tag through `close`, an end tag
 * through `open`. `index` is its position in the token list.
 * @typedef {{
 *   name: string,
 *   closing: boolean,
 *   from: number,
 *   end: number,
 *   selfClosing: boolean,
 *   attrs: Attr[],
 *   index: number,
 *   close: Token | null,
 *   open: Token | null,
 * }} Token
 */

const HEADING_TAGS = new Set(["h1", "h2", "h3", "h4", "h5", "h6"]);
const BLOCK_TAGS = new Set([
  "section", "article", "aside", "nav", "header", "footer", "main",
  "div", "figure", "table", "ul", "ol", "details", "pre",
]);
const CODE_TAGS = new Set(["pre", "code"]);
const RAW_TAGS = new Set(["script", "style"]);
const SRC_TAGS = new Set(["img", "script", "source", "video", "audio"]);
// Tags matched to their end tags by name, so that nesting pairs correctly.
const PAIRED_TAGS = new Set([...HEADING_TAGS, ...BLOCK_TAGS, ...CODE_TAGS, "figcaption"]);

const NAMED_ENTITIES = new Map([
  ["amp", "&"],
  ["lt", "<"],
  ["gt", ">"],
  ["quot", '"'],
  ["apos", "'"],
  ["nbsp", " "],
]);

/**
 * The index items of one HTML file.
 * @param {string} text
 * @param {string} _path unused: the HTML adapter reads only the text
 * @returns {IndexItem[]}
 */
export function htmlAdapter(text, _path) {
  if (typeof text !== "string") return [];
  const { tokens, raw } = scanTags(text);
  pairTags(tokens);
  const n = text.length;
  /** @type {IndexItem[]} */
  const items = [];

  /** @type {Map<number, Token>} */
  const startAt = new Map();
  for (const token of tokens) {
    if (!token.closing) startAt.set(token.from, token);
  }

  // Headings: the outline's titles, each running to its closing tag or, when
  // there is none, to the end of its line.
  /** @type {Map<number, string>} */
  const titles = new Map();
  for (const heading of extractOutline(text, "html")) {
    const open = startAt.get(heading.from);
    const to = open && open.close ? open.close.end : lineEnd(text, heading.from);
    items.push({
      kind: "heading",
      range: [heading.from, to],
      level: heading.level,
      title: heading.title,
    });
    titles.set(heading.from, heading.title);
  }

  /**
   * The title a label carries: a heading's text, or a figure's caption.
   * @param {Token} token
   * @returns {string | undefined}
   */
  const labelTitle = (token) => {
    if (HEADING_TAGS.has(token.name)) return titles.get(token.from);
    if (token.name === "figure") return captionText(text, tokens, token);
    return undefined;
  };

  // Figures, so that a label inside one can be told apart as a figure target.
  const figures = tokens.filter((token) => !token.closing && token.name === "figure" && token.close);

  // Labels, references, links, assets and includes, read from the start tags.
  for (const token of tokens) {
    if (token.closing) continue;
    for (const attr of token.attrs) {
      if (attr.value === null) continue;

      if (attr.name === "id") {
        if (attr.value === "") continue;
        const inFigure = figures.some(
          (figure) => figure.from < token.from && token.from < figure.close.from,
        );
        /** @type {IndexItem} */
        const label = {
          kind: "label",
          range: [attr.from, attr.to],
          name: attr.value,
          nameRange: [attr.valueFrom, attr.valueTo],
          attrs: { target: labelTarget(token, inFigure) },
        };
        const title = labelTitle(token);
        if (title !== undefined) label.title = title;
        items.push(label);
        continue;
      }

      if (attr.name === "href" && attr.value.startsWith("#")) {
        if (attr.value.length > 1) {
          items.push({
            kind: "ref",
            range: [attr.from, attr.to],
            name: attr.value.slice(1),
            nameRange: [attr.valueFrom + 1, attr.valueTo],
            namespace: "label",
          });
        }
        continue;
      }

      // A link to another page or to a URL is a link, not an embedding.
      if (attr.name === "href" && token.name === "a") {
        const link = linkItem(attr);
        if (link) items.push(link);
        continue;
      }

      const asset =
        (attr.name === "src" && SRC_TAGS.has(token.name)) ||
        (attr.name === "href" && token.name === "link");
      // An iframe embeds another page, so it is an include.
      const include = attr.name === "src" && token.name === "iframe";
      if (!asset && !include) continue;
      const name = localPath(attr.value);
      if (name === null) continue;
      items.push({
        kind: asset ? "asset" : "include",
        range: [attr.from, attr.to],
        name,
        nameRange: [attr.valueFrom, attr.valueFrom + name.length],
      });
    }
  }

  // Document metadata: the title, author and description, and the language.
  items.push(...metadataItems(text, tokens));

  // Code: pre and code run to their end tag, or to the end of the text when
  // unclosed. Script and style bodies were consumed whole by the scanner.
  for (const token of tokens) {
    if (token.closing || token.selfClosing || !CODE_TAGS.has(token.name)) continue;
    items.push({ kind: "code", range: [token.from, token.close ? token.close.end : n] });
  }
  for (const element of raw) {
    items.push({ kind: "code", range: [element.from, element.end] });
  }

  // Blocks: multi-line elements, from the end of the opening tag to the start
  // of the closing tag.
  for (const token of tokens) {
    if (token.closing || !token.close || !BLOCK_TAGS.has(token.name)) continue;
    const from = token.end;
    const to = token.close.from;
    if (/[\r\n]/.test(text.slice(from, to))) {
      items.push({ kind: "block", range: [from, to], attrs: { tag: token.name } });
    }
  }

  // Prose skips: the spellchecker's ranges, one item each.
  for (const [from, to] of skipRanges(text, "html")) {
    items.push({ kind: "markup", range: [from, to], attrs: { prose: "skip" } });
  }

  return items.sort((a, b) => a.range[0] - b.range[0] || a.range[1] - b.range[1]);
}

/**
 * Every tag in the source, in order. Comments and declarations are skipped.
 * The body of a script or style element is skipped whole, so markup inside
 * it is never read.
 * @param {string} text
 * @returns {{ tokens: Token[], raw: Array<{ name: string, from: number, end: number }> }}
 */
function scanTags(text) {
  const n = text.length;
  /** @type {Token[]} */
  const tokens = [];
  /** @type {Array<{ name: string, from: number, end: number }>} */
  const raw = [];
  let lower = null;
  let i = 0;
  while (i < n) {
    const lt = text.indexOf("<", i);
    if (lt === -1) break;
    i = lt;
    const next = text[i + 1];
    if (text.startsWith("<!--", i)) {
      const close = text.indexOf("-->", i + 4);
      i = close === -1 ? n : close + 3;
    } else if (next === "!" || next === "?") {
      const gt = text.indexOf(">", i + 2);
      i = gt === -1 ? n : gt + 1;
    } else if (next === "/" && isLetter(text[i + 2])) {
      const name = readName(text, i + 2);
      const gt = text.indexOf(">", i + 2);
      const end = gt === -1 ? n : gt + 1;
      tokens.push({
        name, closing: true, from: i, end, selfClosing: false, attrs: [],
        index: tokens.length, close: null, open: null,
      });
      i = end;
    } else if (isLetter(next)) {
      const tag = parseStartTag(text, i);
      tokens.push({
        name: tag.name, closing: false, from: i, end: tag.end, selfClosing: tag.selfClosing,
        attrs: tag.attrs, index: tokens.length, close: null, open: null,
      });
      if (RAW_TAGS.has(tag.name) && !tag.selfClosing) {
        if (lower === null) lower = asciiLower(text);
        const stop = rawEnd(text, lower, tag.name, tag.end);
        raw.push({ name: tag.name, from: i, end: stop });
        i = stop;
      } else {
        i = tag.end;
      }
    } else {
      i++;
    }
  }
  return { tokens, raw };
}

/**
 * Parse the start tag at `start` (a `<`). A quoted `>` does not end the tag.
 * @param {string} text
 * @param {number} start
 * @returns {{ name: string, end: number, selfClosing: boolean, attrs: Attr[] }}
 */
function parseStartTag(text, start) {
  const n = text.length;
  const name = readName(text, start + 1);
  let i = start + 1 + name.length;
  /** @type {Attr[]} */
  const attrs = [];
  while (i < n) {
    const c = text[i];
    if (c === ">") {
      return { name, end: i + 1, selfClosing: text[i - 1] === "/", attrs };
    }
    if (c === "/" || /\s/.test(c)) {
      i++;
      continue;
    }
    const from = i;
    while (i < n && !/[\s=>\/]/.test(text[i])) i++;
    if (i === from) {
      // A stray character such as `=` where a name should start.
      i++;
      continue;
    }
    const attrName = text.slice(from, i).toLowerCase();
    let j = i;
    while (j < n && /\s/.test(text[j])) j++;
    if (text[j] !== "=") {
      attrs.push({ name: attrName, value: null, from, to: i, valueFrom: i, valueTo: i });
      continue;
    }
    j++;
    while (j < n && /\s/.test(text[j])) j++;
    const quote = text[j];
    let valueFrom;
    let valueTo;
    let to;
    if (quote === '"' || quote === "'") {
      const close = text.indexOf(quote, j + 1);
      valueFrom = j + 1;
      valueTo = close === -1 ? n : close;
      to = close === -1 ? n : close + 1;
    } else {
      valueFrom = j;
      let k = j;
      while (k < n && !/[\s>]/.test(text[k])) k++;
      valueTo = k;
      to = k;
    }
    attrs.push({ name: attrName, value: text.slice(valueFrom, valueTo), from, to, valueFrom, valueTo });
    i = to;
  }
  // An unterminated tag runs to the end of the text.
  return { name, end: n, selfClosing: false, attrs };
}

/**
 * Link each start tag of a paired name to its end tag, matching by name so
 * that nested elements of the same name pair correctly.
 * @param {Token[]} tokens
 */
function pairTags(tokens) {
  /** @type {Map<string, Token[]>} */
  const stacks = new Map();
  for (const token of tokens) {
    if (!PAIRED_TAGS.has(token.name) || token.selfClosing) continue;
    if (token.closing) {
      const start = stacks.get(token.name)?.pop();
      if (start) {
        start.close = token;
        token.open = start;
      }
    } else {
      const stack = stacks.get(token.name) ?? [];
      stack.push(token);
      stacks.set(token.name, stack);
    }
  }
}

/**
 * The text of the first figcaption inside a figure, as plain text, or
 * undefined when there is none.
 * @param {string} text
 * @param {Token[]} tokens
 * @param {Token} figure
 * @returns {string | undefined}
 */
function captionText(text, tokens, figure) {
  if (!figure.close) return undefined;
  for (let k = figure.index + 1; k < tokens.length && tokens[k].from < figure.close.from; k++) {
    const token = tokens[k];
    if (!token.closing && token.name === "figcaption" && token.close) {
      return plainText(text.slice(token.end, token.close.from)) || undefined;
    }
  }
  return undefined;
}

/**
 * Markup removed, common entities decoded, whitespace collapsed.
 * @param {string} html
 * @returns {string}
 */
function plainText(html) {
  return html
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<[^>]*>/g, " ")
    .replace(/&(#[xX][0-9A-Fa-f]+|#[0-9]+|[A-Za-z]+);/g, (whole, body) => {
      if (body[0] === "#") {
        const hex = /^#[xX]/.test(body);
        const code = hex ? Number.parseInt(body.slice(2), 16) : Number.parseInt(body.slice(1), 10);
        const valid = Number.isInteger(code) && code >= 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff);
        return valid ? String.fromCodePoint(code) : whole;
      }
      return NAMED_ENTITIES.get(body.toLowerCase()) ?? whole;
    })
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * The non-null attribute of a start tag by name, or undefined.
 * @param {Token} token
 * @param {string} name
 * @returns {Attr | undefined}
 */
function attrOf(token, name) {
  const attr = token.attrs.find((item) => item.name === name);
  return attr && attr.value !== null ? attr : undefined;
}

/**
 * The kind of target a label on this element names.
 * @param {Token} token
 * @param {boolean} inFigure whether the element sits inside a figure
 * @returns {"section" | "figure" | "table" | "equation" | "listing" | "footnote" | "item" | "other"}
 */
function labelTarget(token, inFigure) {
  const classes = (attrOf(token, "class")?.value ?? "").split(/\s+/);
  const roles = (attrOf(token, "role")?.value ?? "").split(/\s+/);
  if (HEADING_TAGS.has(token.name)) return "section";
  if (token.name === "figure" || inFigure) return "figure";
  if (token.name === "table") return "table";
  if (CODE_TAGS.has(token.name)) return "listing";
  if (token.name === "math" || classes.some((name) => /math|equation/i.test(name))) return "equation";
  if (token.name === "li") return "item";
  if (
    (token.name === "aside" || token.name === "section") &&
    (roles.includes("doc-footnote") || classes.includes("footnote"))
  ) {
    return "footnote";
  }
  return "other";
}

/**
 * The link item of an href attribute: an external item for a URL with a
 * scheme or protocol-relative, a file item for a relative path with any
 * fragment kept in attrs.fragment, or null for an empty value, a bare
 * fragment (a ref) or a value with no path.
 * @param {Attr} attr
 * @returns {IndexItem | null}
 */
function linkItem(attr) {
  const value = attr.value ?? "";
  if (value === "") return null;
  if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(value) || value.startsWith("//")) {
    return {
      kind: "link",
      range: [attr.from, attr.to],
      name: value,
      nameRange: [attr.valueFrom, attr.valueTo],
      attrs: { external: "true" },
    };
  }
  if (value.startsWith("#")) return null;
  const name = value.split(/[?#]/, 1)[0];
  if (name === "") return null;
  const hash = value.indexOf("#");
  const fragment = hash === -1 ? "" : value.slice(hash + 1);
  /** @type {Record<string, string>} */
  const attrs = { file: "true" };
  if (fragment !== "") attrs.fragment = fragment;
  return {
    kind: "link",
    range: [attr.from, attr.to],
    name,
    nameRange: [attr.valueFrom, attr.valueFrom + name.length],
    attrs,
  };
}

/** The meta names that carry metadata, mapped to their metadata names. */
const META_NAMES = new Map([
  ["author", "author"],
  ["description", "abstract"],
]);

/**
 * The metadata items of the head: the first title, each meta author and
 * description, and the language of the html element.
 * @param {string} text
 * @param {Token[]} tokens
 * @returns {IndexItem[]}
 */
function metadataItems(text, tokens) {
  /** @type {IndexItem[]} */
  const items = [];
  let sawBody = false;
  let sawTitle = false;
  for (const token of tokens) {
    if (token.closing) continue;
    if (token.name === "body") sawBody = true;

    if (token.name === "html") {
      const lang = attrOf(token, "lang");
      const value = lang?.value?.trim() ?? "";
      if (lang && value !== "") {
        items.push({
          kind: "metadata",
          name: "language",
          title: value,
          range: [lang.from, lang.to],
          nameRange: [lang.valueFrom, lang.valueTo],
        });
      }
    }

    if (token.name === "meta") {
      const key = attrOf(token, "name")?.value?.trim().toLowerCase();
      const name = key === undefined ? undefined : META_NAMES.get(key);
      const content = attrOf(token, "content");
      const value = content ? plainText(content.value ?? "") : "";
      if (name && content && value !== "") {
        items.push({
          kind: "metadata",
          name,
          title: value,
          range: [content.from, content.to],
          nameRange: [content.valueFrom, content.valueTo],
        });
      }
    }

    // The first title before the body: an svg title inside the body is not
    // the document's.
    if (token.name === "title" && !sawTitle && !sawBody) {
      const rest = text.slice(token.end);
      const close = /<\/title\s*>/i.exec(rest);
      if (!close) continue;
      const innerTo = token.end + close.index;
      const value = plainText(text.slice(token.end, innerTo));
      if (value === "") continue;
      sawTitle = true;
      items.push({
        kind: "metadata",
        name: "title",
        title: value,
        range: [token.from, innerTo + close[0].length],
        nameRange: [token.end, innerTo],
      });
    }
  }
  return items;
}

/**
 * The file a URL-ish attribute value names, without its query and fragment,
 * or null when the value is empty, a fragment, a URL with a scheme (data URIs
 * included), or protocol-relative.
 * @param {string} value
 * @returns {string | null}
 */
function localPath(value) {
  if (value === "" || value.startsWith("#") || value.startsWith("//")) return null;
  if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(value)) return null;
  const path = value.split(/[?#]/, 1)[0];
  return path === "" ? null : path;
}

/**
 * The lowercased tag or attribute name that starts at `from`.
 * @param {string} text
 * @param {number} from
 * @returns {string}
 */
function readName(text, from) {
  let to = from;
  while (to < text.length && /[A-Za-z0-9-]/.test(text[to])) to++;
  return text.slice(from, to).toLowerCase();
}

/**
 * @param {string | undefined} ch
 * @returns {boolean}
 */
function isLetter(ch) {
  return ch !== undefined && /[A-Za-z]/.test(ch);
}

/**
 * The index of the first line break at or after `from`, or the text length.
 * @param {string} text
 * @param {number} from
 * @returns {number}
 */
function lineEnd(text, from) {
  let to = from;
  while (to < text.length && text[to] !== "\n" && text[to] !== "\r") to++;
  return to;
}
