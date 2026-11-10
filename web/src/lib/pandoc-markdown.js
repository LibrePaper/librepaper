// Pandoc and Quarto syntax on top of CodeMirror's Markdown parser: dollar math,
// fenced divs, citations, cross references and attribute lists. The grammar
// agrees with the plain-text scanners in citation-keys.js (what is a citation)
// and quarto-labels.js (what is a label).
import { foldNodeProp } from "@codemirror/language";
import { GFM, InlineContext, Subscript, Superscript, parser as commonmark } from "@lezer/markdown";
import { styleTags, tags as t } from "@lezer/highlight";
import { FIELD, TAG, fieldProblem } from "./tasks/grammar.js";

const AT = 64, DOLLAR = 36, OPEN_BRACKET = 91, CLOSE_BRACKET = 93, OPEN_PAREN = 40, OPEN_BRACE = 123, BACKTICK = 96;
const LESS_THAN = 60, EQUALS = 61, CARET = 94, HASH = 35;
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
    // Without a closing fence the opener is plain paragraph text, as in Pandoc.
    if (close > cx.to) return false;
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

// The closing fence is the div's last line. It returns false, so the div ends on
// this line: a true return would leave the consumed fence to an open indented
// code block, which reads an empty line and runs on past the fence.
function continueDiv(cx, line, close) {
  if (cx.lineStart < close) return true;
  if (cx.lineStart > close) return false;
  if (line.indent - line.baseIndent >= 4) return false;
  const match = /^:{3,}/.exec(line.text.slice(line.pos));
  if (!match || !DIV_CLOSE.test(line.text.slice(line.pos))) return false;
  const from = cx.lineStart + line.pos;
  line.addMarker(cx.elt("FencedDivMark", from, from + match[0].length));
  line.moveBase(line.text.length);
  return false;
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

// ---- Quarto shortcodes -----------------------------------------------------

// `{{< name args >}}`, within the paragraph. The arguments are not Markdown.
const shortcode = {
  name: "Shortcode",
  parse(cx, next, pos) {
    if (next !== OPEN_BRACE || cx.char(pos + 1) !== OPEN_BRACE || cx.char(pos + 2) !== LESS_THAN) return -1;
    const close = cx.text.indexOf(">}}", pos + 3 - cx.offset);
    if (close < 0) return -1;
    const end = cx.offset + close + 3;
    return cx.addElement(cx.elt("Shortcode", pos, end, [
      cx.elt("ShortcodeMark", pos, pos + 3),
      cx.elt("ShortcodeMark", end - 3, end),
    ]));
  },
};

// ---- highlight, inline notes and footnote references -------------------------

const PUNCTUATION = /[\p{P}\p{S}]/u;
const HighlightDelim = { resolve: "Highlight", mark: "HighlightMark" };

// `==mark==`, flanked like `~~strike~~`.
const highlight = {
  name: "Highlight",
  after: "Emphasis",
  parse(cx, next, pos) {
    if (next !== EQUALS || cx.char(pos + 1) !== EQUALS || cx.char(pos + 2) === EQUALS || cx.char(pos - 1) === EQUALS) return -1;
    const before = cx.slice(pos - 1, pos), after = cx.slice(pos + 2, pos + 3);
    const spaceBefore = /\s|^$/.test(before), spaceAfter = /\s|^$/.test(after);
    const punctBefore = PUNCTUATION.test(before), punctAfter = PUNCTUATION.test(after);
    return cx.addDelimiter(
      HighlightDelim, pos, pos + 2,
      !spaceAfter && (!punctAfter || spaceBefore || punctBefore),
      !spaceBefore && (!punctBefore || spaceAfter || punctAfter),
    );
  },
};

// The `^[` of an inline note is a delimiter that the matching `]` turns into an InlineNote.
const InlineNoteDelim = {};

const inlineNoteStart = {
  name: "InlineNoteStart",
  before: "Emphasis",
  parse(cx, next, pos) {
    if (next !== CARET || cx.char(pos + 1) !== OPEN_BRACKET || cx.char(pos - 1) === CARET) return -1;
    return cx.addDelimiter(InlineNoteDelim, pos, pos + 2, true, false);
  },
};

// A `]` closes the note when no link or image bracket opened after the note did.
const inlineNoteEnd = {
  name: "InlineNoteEnd",
  before: "LinkEnd",
  parse(cx, next, pos) {
    if (next !== CLOSE_BRACKET) return -1;
    const open = cx.findOpeningDelimiter(InlineNoteDelim);
    if (open === null) return -1;
    const parts = /** @type {any} */ (cx).parts;
    for (let i = open + 1; i < parts.length; i++) {
      const part = cx.getDelimiterAt(i);
      if (part && (part.type === InlineContext.linkStart || part.type === InlineContext.imageStart)) return -1;
    }
    const start = /** @type {any} */ (cx.getDelimiterAt(open)).from;
    const content = cx.takeContent(open);
    return cx.addElement(cx.elt("InlineNote", start, pos + 1, [
      cx.elt("InlineNoteMark", start, start + 2),
      ...content,
      cx.elt("InlineNoteMark", pos, pos + 1),
    ]));
  },
};

const FOOTNOTE_REFERENCE = /^\[\^[^\]\s^]+\]/;

// `[^id]`, a reference to a footnote definition.
const footnoteReference = {
  name: "FootnoteReference",
  before: "Link",
  parse(cx, next, pos) {
    if (next !== OPEN_BRACKET || cx.char(pos + 1) !== CARET) return -1;
    const match = FOOTNOTE_REFERENCE.exec(cx.slice(pos, cx.end));
    return match ? cx.addElement(cx.elt("FootnoteReference", pos, pos + match[0].length)) : -1;
  },
};

// ---- footnote definitions ----------------------------------------------------

const FOOTNOTE_DEFINITION = /^\[\^[^\]\s^]+\]:[ \t]*/;
const FOOTNOTE_INDENT = 4;

// A definition continues over blank lines and lines indented four columns. Like a list
// item, it ends at the first other line.
function continueFootnote(cx, line, value) {
  if (line.indent < line.baseIndent + value && line.next > -1) return false;
  line.moveBaseColumn(line.baseIndent + value);
  return true;
}

// `[^id]: text`, whose body is parsed as blocks. A new definition ends the paragraph before it.
const footnoteDefinition = {
  name: "FootnoteDefinition",
  before: "LinkReference",
  parse(cx, line) {
    if (line.next !== OPEN_BRACKET || line.indent - line.baseIndent >= 4) return false;
    const match = FOOTNOTE_DEFINITION.exec(line.text.slice(line.pos));
    if (!match) return false;
    const from = cx.lineStart + line.pos;
    cx.startComposite("FootnoteDefinition", line.pos, FOOTNOTE_INDENT);
    cx.addElement(cx.elt("FootnoteLabel", from, from + match[0].indexOf("]") + 1));
    line.moveBase(line.pos + match[0].length);
    return null;
  },
  endLeaf: (cx, line) => FOOTNOTE_DEFINITION.test(line.text.slice(line.pos)),
};

// ---- table captions and grid tables ------------------------------------------

const CAPTION_START = /^(Table:|:)[ \t]/;

class CaptionParser {
  nextLine() { return false; }
  finish(cx, leaf) {
    const mark = /** @type {RegExpExecArray} */ (CAPTION_START.exec(leaf.content))[1].length;
    cx.addLeafElement(leaf, cx.elt("TableCaption", leaf.start, leaf.start + leaf.content.length, [
      cx.elt("TableCaptionMark", leaf.start, leaf.start + mark),
      ...cx.parser.parseInline(leaf.content.slice(mark), leaf.start + mark),
    ]));
    return true;
  }
}

// A paragraph that starts with `Table:` or `:`. Whether a table sits next to it is for the reader of the tree.
const tableCaption = {
  name: "TableCaption",
  before: "SetextHeading",
  leaf: (cx, leaf) => (CAPTION_START.test(leaf.content) ? new CaptionParser() : null),
};

const GRID_BORDER = /^\+[-=:+]+\+[ \t]*$/;
const GRID_ROW = /^[|+]/;

class GridParser {
  nextLine() { return false; }
  finish(cx, leaf) {
    cx.addLeafElement(leaf, cx.elt("GridTable", leaf.start, leaf.start + leaf.content.length,
      cx.parser.parseInline(leaf.content, leaf.start)));
    return true;
  }
}

// Pandoc grid tables: a border line such as `+---+---+`, then lines that start with `|` or `+`.
const gridTable = {
  name: "GridTable",
  before: "SetextHeading",
  leaf(cx, leaf) {
    const end = leaf.content.indexOf("\n");
    return GRID_BORDER.test(end < 0 ? leaf.content : leaf.content.slice(0, end)) ? new GridParser() : null;
  },
  endLeaf(cx, line, leaf) {
    const text = line.text.slice(line.pos);
    if (/** @type {any} */ (leaf).parsers.some((parser) => parser instanceof GridParser)) return !GRID_ROW.test(text);
    return GRID_BORDER.test(text);
  },
};

// ---- tasks ---------------------------------------------------------------------

// The span of the first line of the task whose inline content is being parsed, or null. The
// metadata chips exist only there, so the inline parser asks. The parse is synchronous.
let taskLine = null;

class TaskParser {
  nextLine() { return false; }
  finish(cx, leaf) {
    const lineEnd = leaf.content.indexOf("\n");
    const from = leaf.start + 3;
    taskLine = { from, to: leaf.start + (lineEnd < 0 ? leaf.content.length : lineEnd) };
    let inline;
    try {
      inline = cx.parser.parseInline(leaf.content.slice(3), from);
    } finally {
      taskLine = null;
    }
    cx.addLeafElement(leaf, cx.elt("Task", leaf.start, leaf.start + leaf.content.length, [
      cx.elt("TaskMarker", leaf.start, from),
      ...inline,
    ]));
    return true;
  }
}

// GFM's task list takes `[ ]`, `[x]` and `[X]`. Tasks here also have `[/]` (doing) and `[-]` (cancelled),
// and a box may end the line.
const taskList = {
  name: "TaskList",
  after: "SetextHeading",
  leaf: (cx, leaf) => (/^\[[ xX\/-]\](?=[ \t\n]|$)/.test(leaf.content) && cx.parentType().name === "ListItem" ? new TaskParser() : null),
};

// A tag ends before whitespace, the end of the input or closing punctuation.
const TAG_END = /[\s.,;:!?)]/;

// `key:value` fields and `#tags` on the first line of a task, after a space or tab. A gap after the
// colon makes the text a field only when the value is valid, so prose such as "done: the tables" stays text.
const taskMeta = {
  name: "TaskMeta",
  parse(cx, next, pos) {
    if (!taskLine || pos < taskLine.from || pos >= taskLine.to) return -1;
    const before = cx.char(pos - 1);
    if (before !== 32 && before !== 9) return -1;
    const at = pos - cx.offset;
    if (next === HASH) {
      TAG.lastIndex = at;
      const tag = TAG.exec(cx.text);
      if (!tag) return -1;
      const after = cx.text[at + tag[0].length];
      if (after !== undefined && !TAG_END.test(after)) return -1;
      return cx.addElement(cx.elt("TaskTag", pos, pos + tag[0].length));
    }
    FIELD.lastIndex = at;
    const field = FIELD.exec(cx.text);
    if (!field) return -1;
    if (field[2] !== "" && fieldProblem(field[1], field[3]) !== null) return -1;
    return cx.addElement(cx.elt("TaskField", pos, pos + field[0].length));
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
    "Shortcode", "ShortcodeMark", "Highlight", "HighlightMark", "InlineNote", "InlineNoteMark",
    "FootnoteReference", { name: "FootnoteDefinition", composite: continueFootnote }, "FootnoteLabel",
    { name: "TableCaption", block: true }, "TableCaptionMark", { name: "GridTable", block: true },
    "TaskField", "TaskTag",
  ],
  parseBlock: [fencedDiv, mathBlock, footnoteDefinition, tableCaption, gridTable, taskList],
  parseInline: [
    shortcode, inlineNoteStart, inlineNoteEnd, footnoteReference, highlight, taskMeta,
    citationBracket, citationBare, mathInline, attributesInline,
  ],
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
      "ShortcodeMark HighlightMark InlineNoteMark TableCaptionMark": t.processingInstruction,
      Shortcode: t.special(t.string),
      Highlight: t.special(t.content),
      "FootnoteLabel FootnoteReference": t.labelName,
      TaskField: t.attributeName,
      TaskTag: t.labelName,
    }),
  ],
}];

/**
 * The Markdown parser the editor uses, without the editor: GFM and Pandoc
 * syntax. For reading a tree outside a CodeMirror state, such as the document index.
 */
export const pandocMarkdownParser = commonmark.configure([GFM, Subscript, Superscript, pandocExtensions]);
