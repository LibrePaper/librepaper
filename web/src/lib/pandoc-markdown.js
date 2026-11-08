// Pandoc and Quarto syntax on top of CodeMirror's Markdown parser: dollar math,
// fenced divs, citations, cross references and attribute lists. The grammar
// agrees with the plain-text scanners in citation-keys.js (what is a citation)
// and quarto-labels.js (what is a label).
import { foldNodeProp } from "@codemirror/language";
import { GFM, InlineContext, Subscript, Superscript, parser as commonmark } from "@lezer/markdown";
import { styleTags, tags as t } from "@lezer/highlight";

const AT = 64, DOLLAR = 36, OPEN_BRACKET = 91, CLOSE_BRACKET = 93, OPEN_PAREN = 40, OPEN_BRACE = 123, BACKTICK = 96;
const WORD = /[-\p{L}\p{N}_:.+]/u;
const CROSS_REF = /^(fig|tbl|sec|eq|lst|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|tip|nte|wrn|imp|cau)[-:]/;

// ---- helpers -------------------------------------------------------------

const isSpace = (code) => code === 32 || code === 9 || code === 10 || code === 13 || code !== code || code < 0;
const isDigit = (code) => code >= 48 && code <= 57;

function escaped(text, index) {
  let slashes = 0;
  while (index - 1 - slashes >= 0 && text[index - 1 - slashes] === "\\") slashes++;
  return slashes % 2 === 1;
}

// An @ that continues a word, an address or a path is not a citation.
function blocksCitation(previous) {
  if (!previous) return false;
  return (WORD.test(previous) && previous !== "-") || previous === "@" || previous === "/";
}

// Attribute lists: `{#id .class key=value key2="v w"}`. Returns the Attributes
// element, or null when the text is not a well-formed list.
// Ids and classes may contain dots (`#intro.v2`), so attributes are separated by blanks.
const ATTRIBUTE = /(?:#([^\s#={}"']+))|(?:\.([^\s#={}"']+))|(?:([^\s=#.{}"']+)=("[^"]*"|'[^']*'|[^\s"'{}]+))/y;
function attributeList(cx, text, from) {
  if (text[0] !== "{" || text[text.length - 1] !== "}") return null;
  const inner = text.slice(1, -1);
  const children = [];
  let i = 0;
  for (;;) {
    while (i < inner.length && /\s/.test(inner[i])) i++;
    if (i >= inner.length) break;
    ATTRIBUTE.lastIndex = i;
    const match = ATTRIBUTE.exec(inner);
    if (!match) return null;
    const start = from + 1 + i;
    if (match[1] !== undefined) children.push(cx.elt("AttributeId", start, start + 1 + match[1].length));
    else if (match[2] !== undefined) children.push(cx.elt("AttributeClass", start, start + 1 + match[2].length));
    else {
      children.push(cx.elt("AttributeKey", start, start + match[3].length));
      children.push(cx.elt("AttributeValue", start + match[3].length + 1, start + match[0].length));
    }
    i = ATTRIBUTE.lastIndex;
    if (i < inner.length && !/\s/.test(inner[i])) return null;
  }
  if (!children.length) return null;
  return cx.elt("Attributes", from, from + text.length, children);
}

// ---- inline math ---------------------------------------------------------

const mathInline = {
  name: "PandocMath",
  parse(cx, next, pos) {
    if (next !== DOLLAR) return -1;
    const text = cx.text, offset = cx.offset, i = pos - offset;
    if (text[i + 1] === "$") {
      // Display math inside a paragraph. An unmatched $$ is plain text.
      let k = i + 2;
      for (;;) {
        k = text.indexOf("$$", k);
        if (k < 0) return pos + 2;
        if (k > i + 2 && !escaped(text, k)) break;
        k++;
      }
      return cx.addElement(cx.elt("DisplayMath", pos, offset + k + 2, [
        cx.elt("DisplayMathMark", pos, pos + 2),
        cx.elt("DisplayMathMark", offset + k, offset + k + 2),
      ]));
    }
    // Pandoc's tex_math_dollars: the opening $ is followed by a non-space, the
    // closing $ is preceded by a non-space and not followed by a digit.
    if (isSpace(text.charCodeAt(i + 1))) return -1;
    for (let k = i + 1; k < text.length; k++) {
      if (text[k] !== "$" || escaped(text, k)) continue;
      if (k === i + 1 || isSpace(text.charCodeAt(k - 1))) continue;
      if (isDigit(text.charCodeAt(k + 1))) continue;
      return cx.addElement(cx.elt("InlineMath", pos, offset + k + 1, [
        cx.elt("InlineMathMark", pos, pos + 1),
        cx.elt("InlineMathMark", offset + k, offset + k + 1),
      ]));
    }
    return -1;
  },
};

// A paragraph-level `$$` opens a block that runs to the line holding the next
// `$$`. A block keeps lines such as `- x` or `= y` from being read as list items
// or headings. It needs a closing line before the next blank line, so an unfinished
// `$$` never swallows the rest of the document.
function closesMath(cx) {
  let pos = cx.absoluteLineEnd + 1;
  while (pos < cx.to) {
    const { text, end } = /** @type {any} */ (cx).scanLine(pos);
    if (!text.trim()) return false;
    if (text.includes("$$")) return true;
    pos = end + 1;
  }
  return false;
}

const mathBlock = {
  name: "DisplayMathBlock",
  before: "FencedCode",
  parse(cx, line) {
    if (line.next !== DOLLAR || line.text.charCodeAt(line.pos + 1) !== DOLLAR) return false;
    if (line.indent - line.baseIndent >= 4) return false;
    if (line.text.indexOf("$$", line.pos + 2) >= 0) return false;
    if (!closesMath(cx)) return false;
    const from = cx.lineStart + line.pos;
    const kids = [cx.elt("DisplayMathMark", from, from + 2)];
    let to = cx.lineStart + line.text.length;
    while (cx.nextLine() && line.depth >= cx.stack.length) {
      for (const marker of line.markers) kids.push(marker);
      to = cx.lineStart + line.text.length;
      const at = line.text.indexOf("$$", line.basePos);
      if (at < 0) continue;
      kids.push(cx.elt("DisplayMathMark", cx.lineStart + at, cx.lineStart + at + 2));
      const rest = line.text.slice(at + 2);
      const trimmed = rest.trim();
      if (trimmed) {
        const attributes = attributeList(cx, trimmed, cx.lineStart + at + 2 + rest.indexOf(trimmed));
        if (attributes) kids.push(attributes);
      }
      cx.nextLine();
      break;
    }
    cx.addElement(cx.elt("DisplayMath", from, to, kids));
    return true;
  },
};

// ---- fenced divs ---------------------------------------------------------

const DIV_OPEN = /^(:{3,})[ \t]*(\{[^}\n]*\}|[^\s{}:][^\s{}]*)[ \t]*$/;
const DIV_CLOSE = /^:{3,}[ \t]*$/;
const FENCE = /^(`{3,}|~{3,})/;

// Columns of indentation after any blockquote markers; a tab reaches the next multiple of 4.
function indentAfterQuote(text) {
  const rest = /^(?:[ \t]*>)*(.*)$/s.exec(text)[1];
  let column = 0;
  let index = 0;
  if (rest !== text && rest[0] === " ") index = 1; // the optional space after >
  for (; index < rest.length; index++) {
    if (rest[index] === " ") column++;
    else if (rest[index] === "\t") column += 4 - column % 4;
    else break;
  }
  return column;
}

// The number of blockquote markers that start a line.
function quoteDepth(text) {
  return (/^[\s>]*/.exec(text)[0].match(/>/g) || []).length;
}

// The start of the line holding the colons that close the div opened on the
// current line, or past the end of the document when it is never closed.
function divClose(cx) {
  let start = cx.lineStart + cx.line.text.length + 1;
  let pos = cx.absoluteLineEnd + 1, depth = 1, boundary = true, fence = null;
  // Indentation counts from the opener's container, such as a list item's content column.
  const base = cx.line.baseIndent;
  // Fences count only at the opener's blockquote depth: a quoted `> :::`
  // inside the div is content.
  const quotes = quoteDepth(cx.line.text.slice(0, cx.line.pos));
  while (pos < cx.to) {
    const { text, end } = /** @type {any} */ (cx).scanLine(pos);
    const body = text.replace(/^[\s>]*/, "");
    if (indentAfterQuote(text) - base >= 4 || (body.trim() && quoteDepth(text) !== quotes)) {
      boundary = false;
      start += text.length + 1;
      pos = end + 1;
      continue;
    }
    const code = FENCE.exec(body);
    if (fence) {
      if (code && code[1][0] === fence[0] && code[1].length >= fence.length && !body.slice(code[1].length).trim()) fence = null;
    } else if (code) {
      fence = code[1];
      boundary = false;
    } else if (DIV_CLOSE.test(body)) {
      if (--depth === 0) return start;
      boundary = true;
    } else if (DIV_OPEN.test(body) && boundary) {
      depth++;
    } else {
      boundary = !body.trim();
    }
    start += text.length + 1;
    pos = end + 1;
  }
  return cx.to + 1;
}

const fencedDiv = {
  name: "FencedDiv",
  before: "FencedCode",
  parse(cx, line) {
    if (line.next !== 58 || line.indent - line.baseIndent >= 4) return false;
    const body = line.text.slice(line.pos);
    const match = DIV_OPEN.exec(body);
    if (!match) return false;
    const close = divClose(cx);
    const from = cx.lineStart + line.pos;
    cx.startComposite("FencedDiv", line.pos, close);
    cx.addElement(cx.elt("FencedDivMark", from, from + match[1].length));
    const specFrom = from + body.indexOf(match[2], match[1].length);
    const spec = match[2];
    const attributes = spec[0] === "{"
      ? attributeList(cx, spec, specFrom) || cx.elt("Attributes", specFrom, specFrom + spec.length)
      : cx.elt("Attributes", specFrom, specFrom + spec.length, [cx.elt("AttributeClass", specFrom, specFrom + spec.length)]);
    cx.addElement(attributes);
    line.moveBase(line.text.length);
    return null;
  },
};

function continueDiv(cx, line, close) {
  if (cx.lineStart < close) return true;
  if (cx.lineStart > close) return false;
  if (line.indent - line.baseIndent >= 4) return false;
  const match = /^:{3,}/.exec(line.text.slice(line.pos));
  if (!match || !DIV_CLOSE.test(line.text.slice(line.pos))) return false;
  const from = cx.lineStart + line.pos;
  line.addMarker(cx.elt("FencedDivMark", from, from + match[0].length));
  line.moveBase(line.text.length);
  return true;
}

// ---- citations and cross references ---------------------------------------

function keyAt(cx, pos, text) {
  // `text` is the key with its @; trailing dots and colons are punctuation.
  const key = text.replace(/[.:]+$/, "");
  const name = CROSS_REF.test(key.slice(1)) ? "CrossRef" : "CitationKey";
  return cx.elt(name, pos, pos + key.length);
}

// A citation bracket reuses the Link parser's `[` delimiter. When the matching
// `]` closes content that holds a key and is not followed by `(` or `[`, the
// already parsed inline content (emphasis, math, keys) is wrapped in a Citation.
// Anything else is left for LinkEnd, so links, footnotes and spans are unchanged.
function flattenCitations(cx, elements, citation, mark) {
  const out = [];
  for (const element of elements) {
    // A bare Citation holds just its key; a bracketed one starts with a mark.
    if (element.type === citation && element.children[0]?.type !== mark) out.push(...element.children);
    else {
      if (element.children.length) element.children = flattenCitations(cx, element.children, citation, mark);
      out.push(element);
    }
  }
  return out;
}

const citationBracket = {
  name: "PandocCitation",
  before: "LinkEnd",
  parse(cx, next, pos) {
    if (next !== CLOSE_BRACKET) return -1;
    const after = cx.char(pos + 1);
    if (after === OPEN_PAREN || after === OPEN_BRACKET) return -1;
    const parts = /** @type {any} */ (cx).parts;
    const citation = cx.parser.getNodeType("Citation");
    const mark = cx.parser.getNodeType("CitationMark");
    const crossRef = cx.parser.getNodeType("CrossRef");
    let open = -1, keyed = false;
    for (let i = parts.length - 1; i >= 0; i--) {
      const part = parts[i];
      if (!part) continue;
      if (part.type === InlineContext.linkStart) {
        if (part.side) open = i;
        break;
      }
      if (part.type === InlineContext.imageStart) break;
      if (part.type === citation || part.type === crossRef) keyed = true;
    }
    if (open < 0 || !keyed) return -1;
    const start = parts[open].from;
    const content = flattenCitations(cx, cx.takeContent(open), citation, mark);
    return cx.addElement(cx.elt("Citation", start, pos + 1, [
      cx.elt("CitationMark", start, start + 1),
      ...content,
      cx.elt("CitationMark", pos, pos + 1),
    ]));
  },
};

const citationBare = {
  name: "PandocAuthorInText",
  parse(cx, next, pos) {
    if (next !== AT) return -1;
    const before = pos > cx.offset ? String.fromCharCode(cx.char(pos - 1)) : "";
    if (blocksCitation(before)) return -1;
    const match = /^@[\p{L}\p{N}_][-\p{L}\p{N}_:.+]*/u.exec(cx.slice(pos, cx.end));
    if (!match) return -1;
    const key = keyAt(cx, pos, match[0]);
    // A cross reference such as @fig-a stands alone; a citation wraps its key.
    return cx.addElement(CROSS_REF.test(match[0].slice(1)) ? key : cx.elt("Citation", key.from, key.to, [key]));
  },
};

// ---- attributes after headings, spans, links and images ---------------------

const attributesInline = {
  name: "PandocAttributes",
  parse(cx, next, pos) {
    if (next !== OPEN_BRACE) return -1;
    const text = cx.text, i = pos - cx.offset;
    const close = text.indexOf("}", i);
    if (close < 0) return -1;
    const before = text[i - 1];
    const after = text.slice(close + 1);
    const attached = before === ")" || before === "]" || before === "`";
    // After a space the list belongs to a heading or a display equation, so it must end the text.
    const trailing = (before === undefined || before === " " || before === "\t") && !after.trim();
    if (!attached && !trailing) return -1;
    const list = attributeList(cx, text.slice(i, close + 1), pos);
    return list ? cx.addElement(list) : -1;
  },
};

// ---- the extension ---------------------------------------------------------

function foldDiv(node, state) {
  const first = node.firstChild, last = node.lastChild;
  if (!first || first.name !== "FencedDivMark") return null;
  const from = state.doc.lineAt(first.from).to;
  const closed = last && last !== first && last.name === "FencedDivMark" && last.from > from;
  const to = closed ? state.doc.lineAt(last.from).from : node.to;
  return to > from ? { from, to } : null;
}

/** Pandoc and Quarto syntax for the Markdown parser's `extensions`. */
export const pandocExtensions = [{
  defineNodes: [
    { name: "FencedDiv", composite: continueDiv },
    { name: "DisplayMath", block: true },
    "FencedDivMark", "InlineMath", "InlineMathMark", "DisplayMathMark",
    "Attributes", "AttributeId", "AttributeClass", "AttributeKey", "AttributeValue",
    "Citation", "CitationKey", "CitationMark", "CrossRef",
  ],
  parseBlock: [fencedDiv, mathBlock],
  parseInline: [citationBracket, citationBare, mathInline, attributesInline],
  props: [
    foldNodeProp.add({ FencedDiv: foldDiv }),
    styleTags({
      "InlineMath DisplayMath": t.special(t.string),
      "InlineMathMark DisplayMathMark FencedDivMark": t.processingInstruction,
      Attributes: t.meta,
      "AttributeId AttributeClass": t.attributeName,
      AttributeKey: t.propertyName,
      AttributeValue: t.string,
      "CitationKey CrossRef": t.labelName,
      CitationMark: t.bracket,
    }),
  ],
}];

/**
 * The Markdown parser the editor uses, without the editor: GFM and Pandoc
 * syntax. For reading a tree outside a CodeMirror state, such as the document index.
 */
export const pandocMarkdownParser = commonmark.configure([GFM, Subscript, Superscript, pandocExtensions]);
