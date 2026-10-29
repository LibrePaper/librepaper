import { EditorSelection } from "@codemirror/state";
import { getLanguage, visualTokens, walk } from "./markup.ts";
import { menuKeys } from "../commands.js";
import { emphasisCommand, MARKUP } from "../emphasis.js";

// The formatting toolbar's tools, and how each one edits the source. The
// formats are the ones the toolbar writes: LaTeX and Typst have visual layers
// and language definitions, Markdown has neither and uses MARKDOWN below.

const FORMATS = new Set(["latex", "typst", "markdown"]);
const MARKS = ["bold", "italic", "underline"];
const PLACEHOLDER = "Heading";
const PLACEHOLDER_ITEM = "Item";
const SLOT = "\u0001";

const KEYS = menuKeys();

// Every tool the bar shows, in the order it draws them. Consecutive tools that
// share a `group` are drawn together, with a divider before the next group.
// To add a tool, add an entry here. Exactly one of these keys says what it does:
//   mark     "bold" | "italic" | "underline": toggles that markup on the selection
//   heading  1 | 2 | 3: makes the lines a heading of that level, or a paragraph again
//   list     "bullet" | "number": makes the lines a list, or removes the list
// `glyph` is the text on the button, `icon` a name from Icon.svelte drawn instead.
// `shortcut` is the printed key from lib/commands.js, when one exists.
export const FORMAT_TOOLS = [
  { id: "bold", label: "Bold", group: "emphasis", mark: "bold", glyph: "B", shortcut: KEYS.bold },
  { id: "italic", label: "Italic", group: "emphasis", mark: "italic", glyph: "I", shortcut: KEYS.italic },
  { id: "underline", label: "Underline", group: "emphasis", mark: "underline", glyph: "U" },
  { id: "h1", label: "Heading 1", group: "heading", heading: 1, glyph: "H1" },
  { id: "h2", label: "Heading 2", group: "heading", heading: 2, glyph: "H2" },
  { id: "h3", label: "Heading 3", group: "heading", heading: 3, glyph: "H3" },
  { id: "bullets", label: "Bulleted list", group: "list", list: "bullet", icon: "list" },
  { id: "numbers", label: "Numbered list", group: "list", list: "number", glyph: "1." },
];

// Markdown's markup, used where no language definition supplies it. Its bold
// and italic come from MARKUP in lib/emphasis.js.
const MARKDOWN = {
  wrap: { underline: ["[", "]{.underline}"] },
  heading: (level, text) => `${"#".repeat(level)} ${text}`,
  list: (kind, items) => items.map((item, index) => (kind === "number" ? `${index + 1}. ${item}` : `- ${item}`)).join("\n"),
};

// The LaTeX or Typst commands, or Markdown's defaults. A format with no
// language definition, Markdown included, uses the defaults.
function commandsFor(format) {
  if (!FORMATS.has(format)) return null;
  let language = null;
  try {
    language = getLanguage(format);
  } catch {
    language = null;
  }
  return language?.commands ?? MARKDOWN;
}

// Open and close markup for a mark in a format, or null when it has none.
function markersFor(format, mark) {
  const table = MARKUP[format];
  if ((mark === "bold" || mark === "italic") && table?.[mark]) return table[mark];
  return commandsFor(format)?.wrap?.[mark] ?? null;
}

// The two markers around a heading of a level: `\section{` and `}` for LaTeX,
// `= ` and nothing for Typst. Found by building the heading around a slot.
function headingMarkers(format, level) {
  const text = commandsFor(format).heading(level, SLOT);
  const at = text.indexOf(SLOT);
  return [text.slice(0, at), text.slice(at + SLOT.length)];
}

// The heading a line body is, as its level and its markers, or null.
function headingOf(format, body) {
  for (const level of [1, 2, 3]) {
    const [open, close] = headingMarkers(format, level);
    if (body.length >= open.length + close.length && body.startsWith(open) && body.endsWith(close)) {
      return { level, open, close };
    }
  }
  return null;
}

function headingLevelOf(format, text) {
  return headingOf(format, splitIndent(text)[1])?.level ?? 0;
}

// A line's leading whitespace, and the rest of the line.
function splitIndent(text) {
  const indent = /^\s*/.exec(text)[0];
  return [indent, text.slice(indent.length)];
}

// The lines a selection covers. A selection that ends just after a line break
// does not take in the line it starts.
function linesOf(state, range) {
  const doc = state.doc;
  const first = doc.lineAt(range.from);
  let last = doc.lineAt(range.to);
  if (range.to > range.from && range.to === last.from) last = doc.lineAt(range.to - 1);
  return { first, last, from: first.from, to: last.to };
}

function blockLines(doc, block) {
  const lines = [];
  for (let n = block.first.number; n <= block.last.number; n++) lines.push(doc.line(n));
  return lines;
}

// Replaces whole lines with `text`. `at` is the selection, as offsets into `text`.
/** @param {number[]} at */
function replaceBlock(view, block, text, at) {
  const [start, end] = at;
  view.dispatch({
    changes: { from: block.from, to: block.to, insert: text },
    selection: EditorSelection.range(block.from + start, block.from + end),
    scrollIntoView: true,
    userEvent: "input.format",
  });
  return true;
}

// Applies several changes, given against the current document, and keeps the
// range selected through them.
function commitMapped(view, changes, range) {
  const set = view.state.changes([...changes].sort((a, b) => a.from - b.from));
  view.dispatch({
    changes: set,
    selection: EditorSelection.range(set.mapPos(range.from, 1), set.mapPos(range.to, -1)),
    scrollIntoView: true,
    userEvent: "input.format",
  });
  return true;
}

// Removes a whole line, with its line break.
function deleteLine(doc, line) {
  if (line.number < doc.lines) return { from: line.from, to: line.to + 1, insert: "" };
  return { from: Math.max(0, line.from - 1), to: line.to, insert: "" };
}

// The `open` ... `close` pairs on one line of text, as the offsets of their
// markers. Symmetric markers such as `*` pair off in order; others pair with
// the nearest close that balances their brackets.
function pairsIn(text, open, close) {
  const pairs = [];
  if (open === close) {
    const hits = [];
    for (let i = text.indexOf(open); i !== -1; i = text.indexOf(open, i + open.length)) hits.push(i);
    for (let k = 0; k + 1 < hits.length; k += 2) pairs.push([hits[k], hits[k + 1]]);
    return pairs;
  }
  for (let i = text.indexOf(open); i !== -1; i = text.indexOf(open, i + 1)) {
    const end = closeAfter(text, i + open.length, open, close);
    if (end !== -1) pairs.push([i, end]);
  }
  return pairs;
}

// The offset of the close marker that ends an open marker, or -1. The open
// marker's last character counts as a bracket, so \textbf{a {b} c} closes at
// the last brace.
function closeAfter(text, start, open, close) {
  const opener = open[open.length - 1];
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if (depth === 0 && text.startsWith(close, i)) return i;
    if (text[i] === opener) depth += 1;
    else if (text[i] === close[0] && depth > 0) depth -= 1;
  }
  return -1;
}

// The markup around a selection, as the absolute positions of its open and
// close markers, or null. The markup counts when the selection holds the
// markers, when they sit just outside it, or when the selection is inside a
// pair on its line.
function enclosingMarkup(state, range, markers) {
  if (!markers) return null;
  const [open, close] = markers;
  const doc = state.doc;
  const { from, to } = range;
  const slice = (start, end) => (start >= 0 && end <= doc.length ? doc.sliceString(start, end) : "");
  const found = (openAt, closeAt) => ({ open: [openAt, openAt + open.length], close: [closeAt, closeAt + close.length] });
  const text = doc.sliceString(from, to);
  if (text.length >= open.length + close.length && text.startsWith(open) && text.endsWith(close)) {
    return found(from, to - close.length);
  }
  if (slice(from - open.length, from) === open && slice(to, to + close.length) === close) {
    return found(from - open.length, to);
  }
  const line = doc.lineAt(from);
  if (to > line.to) return null;
  const start = from - line.from;
  const end = to - line.from;
  for (const [s, c] of pairsIn(line.text, open, close)) {
    if (s + open.length <= start && c >= end) return found(line.from + s, line.from + c);
  }
  return null;
}

// Whether a mark is present at the selection, read from the visual layer's
// tokens. A command token such as \textbf{...} or *...* counts when its body
// holds the selection.
const TOKEN_NAMES = {
  latex: { bold: ["textbf"], italic: ["emph", "textit"], underline: ["underline"] },
  typst: { bold: ["strong"], italic: ["emph"], underline: ["underline"] },
};

function markedInTokens(tokens, format, mark, range) {
  const names = TOKEN_NAMES[format]?.[mark] ?? [];
  let marked = false;
  walk(tokens, (token) => {
    if (marked || token.kind !== "command" || !names.includes(token.name)) return;
    const body = token.body ?? token.args?.at(-1);
    if (body && body.from <= range.from && range.to <= body.to) marked = true;
  });
  return marked;
}

// The level of the heading a line is, read from the visual layer's tokens.
function headingInTokens(tokens, line) {
  let level = 0;
  walk(tokens, (token) => {
    if (level === 0 && token.kind === "heading" && token.from >= line.from && token.from <= line.to) {
      level = token.level ?? 0;
    }
  });
  return level >= 1 && level <= 3 ? level : 0;
}

// The list environments LaTeX lists are written in, and their kinds.
const LIST_BEGIN = /\\begin\{(itemize|enumerate)\}/;
const LIST_END = /\\end\{(itemize|enumerate)\}/;
const ENVIRONMENT_OF = { bullet: "itemize", number: "enumerate" };
const KIND_OF = { itemize: "bullet", enumerate: "number" };

// The list environment around a line, as its name and the numbers of its
// begin and end lines, or null. A line that is itself a begin or end is not
// inside its own list.
function listEnvironment(doc, number) {
  let depth = 0;
  let begin = null;
  for (let n = number - 1; n >= 1 && !begin; n--) {
    const text = doc.line(n).text;
    if (LIST_END.test(text)) {
      depth += 1;
    } else {
      const start = LIST_BEGIN.exec(text);
      if (start) {
        if (depth === 0) begin = { name: start[1], line: n };
        else depth -= 1;
      }
    }
  }
  if (!begin) return null;
  depth = 0;
  for (let n = number + 1; n <= doc.lines; n++) {
    const text = doc.line(n).text;
    if (LIST_BEGIN.test(text)) depth += 1;
    if (LIST_END.test(text)) {
      if (depth === 0) return { name: begin.name, begin: begin.line, end: n };
      depth -= 1;
    }
  }
  return null;
}

// The kind of list a line is in, "bullet", "number" or null.
function listKind(format, doc, line) {
  if (format === "latex") {
    const environment = listEnvironment(doc, line.number);
    return environment ? KIND_OF[environment.name] : null;
  }
  const body = splitIndent(line.text)[1];
  if (format === "typst") {
    if (/^-[ \t]/.test(body)) return "bullet";
    if (/^\+[ \t]/.test(body)) return "number";
    return null;
  }
  if (/^[-*+][ \t]/.test(body)) return "bullet";
  if (/^\d+[.)][ \t]/.test(body)) return "number";
  return null;
}

// A line body without its list marker: `\item ` for LaTeX, `- `, `+ ` or `1. `.
function stripList(format, body) {
  if (format === "latex") return body.replace(/^\\item(?:\[[^\]]*\])?[ \t]?/, "");
  if (format === "typst") return body.replace(/^[-+][ \t]/, "");
  return body.replace(/^(?:[-*+]|\d+[.)])[ \t]/, "");
}

function applyMark(view, format, mark) {
  const { state } = view;
  const range = state.selection.main;
  const markers = markersFor(format, mark);
  if (!markers) return false;
  const found = enclosingMarkup(state, range, markers);
  if (found) {
    // Removing the markers keeps the text they held selected.
    return commitMapped(view, [
      { from: found.open[0], to: found.open[1], insert: "" },
      { from: found.close[0], to: found.close[1], insert: "" },
    ], range);
  }
  // Bold and italic toggle through the editor's own command, which knows the
  // stars of Markdown and the math forms of LaTeX.
  if (mark === "bold" || mark === "italic") return emphasisCommand(mark, () => format)(view);
  const [open, close] = markers;
  view.dispatch({
    changes: [
      { from: range.from, insert: open },
      { from: range.to, insert: close },
    ],
    selection: EditorSelection.range(range.from + open.length, range.to + open.length),
    scrollIntoView: true,
    userEvent: "input.format",
  });
  return true;
}

function applyHeading(view, format, level) {
  const { state } = view;
  const block = linesOf(state, state.selection.main);
  const lines = blockLines(state.doc, block);
  const pivot = lines.find((line) => splitIndent(line.text)[1] !== "") ?? block.first;
  // The same level again makes the lines paragraphs.
  const off = headingLevelOf(format, pivot.text) === level;
  const allBlank = lines.every((line) => splitIndent(line.text)[1] === "");
  const single = block.first.number === block.last.number;
  const texts = lines.map((line, index) => {
    const [indent, body] = splitIndent(line.text);
    if (body === "" && !(allBlank && index === 0)) return line.text;
    const known = headingOf(format, body);
    const plain = known ? body.slice(known.open.length, body.length - known.close.length) : body;
    if (off) return indent + plain;
    return indent + commandsFor(format).heading(level, plain || PLACEHOLDER);
  });
  // A heading made on an empty line selects its placeholder, ready to type over.
  const placeholder = !off && single && splitIndent(block.first.text)[1] === "";
  const at = placeholder ? texts[0].indexOf(PLACEHOLDER) : -1;
  const selection = placeholder ? [at, at + PLACEHOLDER.length] : [0, texts.join("\n").length];
  return replaceBlock(view, block, texts.join("\n"), selection);
}

function applyList(view, format, kind) {
  const { state } = view;
  const range = state.selection.main;
  const doc = state.doc;
  const block = linesOf(state, range);
  const lines = blockLines(doc, block);
  const off = listKind(format, doc, block.first) === kind;
  if (format === "latex") return applyLatexList(view, kind, block, lines, range);

  const rows = lines.map((line) => {
    const [indent, body] = splitIndent(line.text);
    return { indent, body: stripList(format, body) };
  });
  const filled = rows.filter((row) => row.body !== "");
  const placeholder = !off && filled.length === 0;
  if (placeholder) {
    rows[0].body = PLACEHOLDER_ITEM;
    filled.push(rows[0]);
  }
  const built = off ? [] : commandsFor(format).list(kind, filled.map((row) => row.body)).split("\n");
  let next = 0;
  const texts = rows.map((row, index) => {
    if (row.body === "") return lines[index].text;
    return row.indent + (off ? row.body : built[next++]);
  });
  const selection = placeholder ? [texts[0].length - PLACEHOLDER_ITEM.length, texts[0].length] : [0, texts.join("\n").length];
  return replaceBlock(view, block, texts.join("\n"), selection);
}

// A LaTeX list is an environment, so the list is a begin line, the items, and
// an end line. Toggling it off removes the environment and the \item markers;
// a list of the other kind is renamed; otherwise the lines become a new list.
function applyLatexList(view, kind, block, lines, range) {
  const doc = view.state.doc;
  const environment = listEnvironment(doc, block.first.number);
  if (environment && KIND_OF[environment.name] === kind) {
    const changes = [deleteLine(doc, doc.line(environment.begin)), deleteLine(doc, doc.line(environment.end))];
    // Items go back to the indentation of the environment, which the list template added to.
    const [outer] = splitIndent(doc.line(environment.begin).text);
    for (let n = environment.begin + 1; n < environment.end; n++) {
      const line = doc.line(n);
      const body = splitIndent(line.text)[1];
      const plain = stripList("latex", body);
      if (plain !== body) changes.push({ from: line.from, to: line.to, insert: outer + plain });
    }
    return commitMapped(view, changes, range);
  }
  if (environment) {
    const name = ENVIRONMENT_OF[kind];
    const begin = doc.line(environment.begin);
    const end = doc.line(environment.end);
    return commitMapped(view, [
      { from: begin.from, to: begin.to, insert: begin.text.replace(`\\begin{${environment.name}}`, `\\begin{${name}}`) },
      { from: end.from, to: end.to, insert: end.text.replace(`\\end{${environment.name}}`, `\\end{${name}}`) },
    ], range);
  }
  const [indent] = splitIndent(block.first.text);
  const filled = lines.map((line) => stripList("latex", splitIndent(line.text)[1])).filter((item) => item !== "");
  const items = filled.length ? filled : [PLACEHOLDER_ITEM];
  const text = commandsFor("latex").list(kind, items).split("\n").map((line) => indent + line).join("\n");
  // The items are selected, not the \begin and \end lines: a caret on those is outside the list.
  const at = filled.length ? [text.indexOf("\n") + 1, text.lastIndexOf("\n")] : [text.lastIndexOf(PLACEHOLDER_ITEM), text.length];
  return replaceBlock(view, block, text, at);
}

// The formats a tool can format in, as its id in FORMAT_TOOLS.
export function applyFormat(view, format, id) {
  const tool = FORMAT_TOOLS.find((each) => each.id === id);
  if (!tool || !FORMATS.has(format) || view.state.readOnly) return false;
  if (tool.mark) return applyMark(view, format, tool.mark);
  if (tool.heading) return applyHeading(view, format, tool.heading);
  if (tool.list) return applyList(view, format, tool.list);
  return false;
}

// What the caret and selection are in: whether bold, italic and underline are
// on, the heading level of the caret's line (0 for none), and its list kind.
// The visual layer's tokens are read when they are current, else the source.
export function formatState(view, format) {
  const none = { bold: false, italic: false, underline: false, heading: 0, list: null };
  if (!FORMATS.has(format)) return none;
  const state = view.state;
  const range = state.selection.main;
  const line = state.doc.lineAt(range.head);
  const tokens = visualTokens(state);
  const marks = {};
  for (const mark of MARKS) {
    marks[mark] = tokens
      ? markedInTokens(tokens, format, mark, range)
      : enclosingMarkup(state, range, markersFor(format, mark)) !== null;
  }
  return {
    ...marks,
    heading: tokens ? headingInTokens(tokens, line) : headingLevelOf(format, line.text),
    list: listKind(format, state.doc, line),
  };
}

// Whether a tool is on in a state from formatState.
export function isToolActive(tool, state) {
  if (!state) return false;
  if (tool.mark) return Boolean(state[tool.mark]);
  if (tool.heading) return state.heading === tool.heading;
  if (tool.list) return state.list === tool.list;
  return false;
}
