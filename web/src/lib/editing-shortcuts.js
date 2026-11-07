// The text edits behind the shortcuts that write markup into the open file:
// links, headings and lists. Each builder takes the editor state and the
// format the file is written in, and returns a transaction spec for
// CodeMirror, or null when that format has no such markup. Nothing here
// touches the view or the DOM, so every rule below is reachable from a plain
// Node test. Editor.svelte applies the spec, as it applies bold and italic.
//
// Quarto is Markdown, so it shares Markdown's markup throughout. A `.sty` file
// is LaTeX source but has no document format, and it gets LaTeX markup the way
// bold does: the caller decides the format, this file only spells it.

/** @typedef {"latex"|"typst"|"markdown"|"quarto"|"html"} MarkupFormat */

const FAMILY = { latex: "latex", typst: "typst", markdown: "markdown", quarto: "markdown", html: "html" };

// The markup family a format writes in, or "" for a format with none.
export const markupFamily = (format) => (Object.hasOwn(FAMILY, format) ? FAMILY[format] : "");

export const canLink = (format) => markupFamily(format) !== "";
export const canList = (format) => markupFamily(format) !== "";
// LaTeX has sectioning commands for levels 1 to 5 only, so level 6 has none.
export const canHeading = (format, level) => {
  if (!Number.isInteger(level) || level < 1 || level > 6) return false;
  const family = markupFamily(format);
  if (family === "latex") return level <= 5;
  return family !== "";
};

/* ---------------------------------------------------------------- the link */

// Each entry gives the text before and after the URL slot, given the text the
// link wraps. The URL slot is where the cursor goes, selected, so a URL typed
// over it replaces the placeholder.
const LINK = {
  latex: (text) => ["\\href{", "}{" + text + "}"],
  markdown: (text) => ["[" + text + "](", ")"],
  typst: (text) => ['#link("', '")[' + text + "]"],
  html: (text) => ['<a href="', '">' + text + "</a>"],
};

const LINK_URL = "url";
const LINK_TEXT = "text";

/// A link around the selection, or around a placeholder when nothing is
/// selected. The placeholder URL is selected afterwards.
export function linkEdit(state, format) {
  const family = markupFamily(format);
  if (!family) return null;
  const range = state.selection.main;
  const shown = state.sliceDoc(range.from, range.to) || LINK_TEXT;
  const [before, after] = LINK[family](shown);
  const start = range.from + before.length;
  return {
    changes: { from: range.from, to: range.to, insert: before + LINK_URL + after },
    selection: { anchor: start, head: start + LINK_URL.length },
    scrollIntoView: true,
    userEvent: "input.format",
  };
}

/* ------------------------------------------------------------- the headings */

const LATEX_SECTIONS = ["section", "subsection", "subsubsection", "paragraph", "subparagraph"];
const LATEX_HEADING = /^\\(?:part|chapter|section|subsection|subsubsection|paragraph|subparagraph)\*?\{/;
const MARKDOWN_HEADING = /^#{1,6}(?=[ \t]|$)[ \t]*/;
const TYPST_HEADING = /^=+(?=[ \t]|$)[ \t]*/;
// An HTML heading's opening tag, attributes and all, and its closing tag.
const HTML_OPEN_HEADING = /^<h[1-6](?=[\s>])[^>]*>/i;
const HTML_CLOSE_HEADING = /<\/h[1-6]>[ \t]*$/i;

/// Every selected line becomes a heading of `level` in the file's markup. A
/// heading already there is replaced, not nested.
export function headingEdit(state, format, level) {
  if (!canHeading(format, level)) return null;
  const family = markupFamily(format);
  const edits = [];
  for (const line of linesOf(state.doc, state.selection.main)) {
    const { at, body } = where(line);
    if (family === "latex") {
      const old = LATEX_HEADING.exec(body);
      const wrapped = Boolean(old) && /\}[ \t]*$/.test(body);
      edits.push({ from: at, to: at + (wrapped ? old[0].length : 0), insert: `\\${LATEX_SECTIONS[level - 1]}{`, bias: "end" });
      if (!wrapped) edits.push({ from: line.to, to: line.to, insert: "}", bias: "start" });
    } else if (family === "html") {
      const open = HTML_OPEN_HEADING.exec(body);
      const close = open ? HTML_CLOSE_HEADING.exec(body) : null;
      if (open && close) {
        // Only the level digit of each tag name changes; the attributes stay.
        const openDigit = at + "<h".length;
        const closeDigit = at + close.index + "</h".length;
        edits.push({ from: openDigit, to: openDigit + 1, insert: String(level), bias: "end" });
        edits.push({ from: closeDigit, to: closeDigit + 1, insert: String(level), bias: "end" });
      } else {
        edits.push({ from: at, to: at, insert: `<h${level}>`, bias: "end" });
        edits.push({ from: line.to, to: line.to, insert: `</h${level}>`, bias: "start" });
      }
    } else {
      const old = (family === "typst" ? TYPST_HEADING : MARKDOWN_HEADING).exec(body);
      const mark = family === "typst" ? "=" : "#";
      edits.push({ from: at, to: at + (old ? old[0].length : 0), insert: `${mark.repeat(level)} `, bias: "end" });
    }
  }
  return specOf(state, edits);
}

/* ---------------------------------------------------------------- the lists */

const LIST_KINDS = ["numbered", "bulleted"];
const ENVIRONMENT = {
  latex: { numbered: "enumerate", bulleted: "itemize" },
  html: { numbered: "ol", bulleted: "ul" },
};
const ENVIRONMENT_KIND = {
  latex: { enumerate: "numbered", itemize: "bulleted" },
  html: { ol: "numbered", ul: "bulleted" },
};
const WRAP_OPEN = {
  latex: /^[ \t]*\\begin\{(enumerate|itemize)\}[ \t]*$/,
  html: /^[ \t]*<(ol|ul)>[ \t]*$/,
};
const WRAP_CLOSE = {
  latex: /^[ \t]*\\end\{(enumerate|itemize)\}[ \t]*$/,
  html: /^[ \t]*<\/(ol|ul)>[ \t]*$/,
};
const OPEN_TAG = { latex: (env) => `\\begin{${env}}`, html: (env) => `<${env}>` };
const CLOSE_TAG = { latex: (env) => `\\end{${env}}`, html: (env) => `</${env}>` };
// The marker a line already carries, whatever its kind, so it can be replaced.
const ANY_MARKER = {
  markdown: /^(?:\d+[.)]|[-*+])[ \t]+/,
  typst: /^(?:\+|-)[ \t]+/,
};
// The marker a line carries for one kind, so that kind can be taken off.
const KIND_MARKER = {
  markdown: { numbered: /^\d+[.)][ \t]+/, bulleted: /^[-*+][ \t]+/ },
  typst: { numbered: /^\+[ \t]+/, bulleted: /^-[ \t]+/ },
};
const NEW_MARKER = {
  markdown: { numbered: (n) => `${n}. `, bulleted: () => "- " },
  typst: { numbered: () => "+ ", bulleted: () => "- " },
};

/// Toggles a numbered or bulleted list over the selected lines. Blank lines
/// are left alone. When every selected line already has the kind, the list is
/// taken off; otherwise the lines are made into that kind. LaTeX and HTML wrap
/// the items in an environment, and an environment already around the lines is
/// renamed or removed rather than doubled.
export function listEdit(state, format, kind) {
  const family = markupFamily(format);
  if (!family || !LIST_KINDS.includes(kind)) return null;
  const lines = linesOf(state.doc, state.selection.main);
  // When the selection holds the list's own wrapper lines, they are the wrapper
  // and only the lines between them are items.
  const own = family === "latex" || family === "html" ? ownWrapper(family, lines) : null;
  const items = (own ? lines.slice(1, -1) : lines).filter((line) => line.text.trim() !== "");
  if (!items.length) return null;
  if (family === "latex" || family === "html") return wrappedList(state, family, kind, items, own);
  const on = !items.every((line) => KIND_MARKER[family][kind].test(where(line).body));
  const edits = [];
  items.forEach((line, index) => {
    const { at, body } = where(line);
    if (!on) {
      const old = KIND_MARKER[family][kind].exec(body);
      edits.push({ from: at, to: at + old[0].length, insert: "", bias: "start" });
      return;
    }
    const old = ANY_MARKER[family].exec(body);
    edits.push({ from: at, to: at + (old ? old[0].length : 0), insert: NEW_MARKER[family][kind](index + 1), bias: "end" });
  });
  return specOf(state, edits);
}

function wrappedList(state, family, kind, items, own) {
  const first = items[0];
  const last = items[items.length - 1];
  const wrap = own ?? wrapperAround(state.doc, first, last, family);
  const env = ENVIRONMENT[family][kind];
  const edits = [];

  // Taking the list off: the environment goes, and each item loses its marker.
  if (wrap && wrap.kind === kind) {
    edits.push({ from: wrap.begin.from, to: first.from, insert: "", bias: "start" });
    edits.push({ from: last.to, to: wrap.end.to, insert: "", bias: "start" });
    for (const line of items) {
      const { at, body } = where(line);
      if (family === "latex") {
        const item = /^\\item[ \t]*/.exec(body);
        if (item) edits.push({ from: at, to: at + item[0].length, insert: "", bias: "start" });
      } else {
        const open = /^<li>/.exec(body);
        if (open) edits.push({ from: at, to: at + open[0].length, insert: "", bias: "start" });
        const close = /<\/li>[ \t]*$/.exec(body);
        if (close) edits.push({ from: line.to - close[0].length, to: line.to, insert: "", bias: "start" });
      }
    }
    return specOf(state, edits);
  }

  // Making the list, or changing its kind: an environment already there is
  // renamed; otherwise one is opened above the first item and closed below the last.
  if (wrap) {
    edits.push({ from: wrap.begin.from, to: wrap.begin.to, insert: where(wrap.begin).indent + OPEN_TAG[family](env), bias: "start" });
    edits.push({ from: wrap.end.from, to: wrap.end.to, insert: where(wrap.end).indent + CLOSE_TAG[family](env), bias: "start" });
  }
  items.forEach((line) => {
    const { indent, at, body } = where(line);
    const item = family === "latex" ? "\\item " : "<li>";
    const old = family === "latex" ? /^\\item[ \t]*/.exec(body) : /^<li>/.exec(body);
    const opening = line === first && !wrap ? OPEN_TAG[family](env) + "\n" + indent + item : item;
    edits.push({ from: at, to: at + (old ? old[0].length : 0), insert: opening, bias: "end" });
    let suffix = "";
    if (family === "html" && !/<\/li>[ \t]*$/.test(body)) suffix += "</li>";
    if (line === last && !wrap) suffix += "\n" + indent + CLOSE_TAG[family](env);
    if (suffix) edits.push({ from: line.to, to: line.to, insert: suffix, bias: "start" });
  });
  return specOf(state, edits);
}

// The environment directly around the selected items, when there is one of
// the markup's own kind: a `\begin{...}` line above and its `\end{...}` below.
function wrapperAround(doc, first, last, family) {
  if (first.number === 1 || last.number >= doc.lines) return null;
  return wrapperBetween(family, doc.line(first.number - 1), doc.line(last.number + 1));
}

// The environment whose opening line and closing line are these two, or null.
function wrapperBetween(family, begin, end) {
  const open = WRAP_OPEN[family].exec(begin.text);
  const close = WRAP_CLOSE[family].exec(end.text);
  if (!open || !close || open[1] !== close[1]) return null;
  return { kind: ENVIRONMENT_KIND[family][open[1]], begin, end };
}

// The environment the selection itself spans: its first line opens it and its
// last line closes it.
function ownWrapper(family, lines) {
  if (lines.length < 2) return null;
  return wrapperBetween(family, lines[0], lines[lines.length - 1]);
}

/* ------------------------------------------------------------------ helpers */

// The lines a selection touches. A selection that ends at the very start of a
// line does not touch that line.
function linesOf(doc, range) {
  const first = doc.lineAt(range.from);
  let last = doc.lineAt(range.to);
  if (range.to > range.from && last.from === range.to && last.number > first.number) last = doc.line(last.number - 1);
  const lines = [];
  for (let number = first.number; number <= last.number; number += 1) lines.push(doc.line(number));
  return lines;
}

// A line split into its indentation, the offset where its text starts, and the
// text after the indentation.
function where(line) {
  const indent = /^[ \t]*/.exec(line.text)[0];
  return { indent, at: line.from + indent.length, body: line.text.slice(indent.length) };
}

// The transaction spec for a set of edits. The edits are written against the
// document as it is now, and the selection is carried through them.
function specOf(state, edits) {
  const sorted = [...edits].sort((a, b) => a.from - b.from || a.to - b.to);
  const range = state.selection.main;
  return {
    changes: sorted.map(({ from, to, insert }) => ({ from, to, insert })),
    selection: { anchor: place(sorted, range.anchor), head: place(sorted, range.head) },
    scrollIntoView: true,
    userEvent: "input.format",
  };
}

// Where a position in the old document lands in the new one. An edit that
// starts at the position takes it to the start or the end of what it inserts,
// by its bias: an inserted marker sits before the caret, so the caret goes
// after it, while an inserted closing tag goes after the caret.
function place(edits, at) {
  let shift = 0;
  for (const edit of edits) {
    const added = edit.insert.length;
    if (edit.to < at) shift += added - (edit.to - edit.from);
    else if (edit.from > at) break;
    else return edit.from + shift + (edit.bias === "end" ? added : 0);
  }
  return at + shift;
}
