import { EditorSelection } from "@codemirror/state";

/// Open and close markup for each format and kind. Quarto writes Markdown, so
/// its entries match Markdown's.
export const MARKUP = {
  latex: {
    bold: ["\\textbf{", "}"],
    italic: ["\\emph{", "}"],
    mathBold: ["\\mathbf{", "}"],
    mathItalic: ["\\mathit{", "}"],
  },
  typst: { bold: ["*", "*"], italic: ["_", "_"] },
  markdown: { bold: ["**", "**"], italic: ["*", "*"] },
  quarto: { bold: ["**", "**"], italic: ["*", "*"] },
  html: { bold: ["<strong>", "</strong>"], italic: ["<em>", "</em>"] },
};

// Environments whose body is math.
const MATH_ENVIRONMENTS = new Set([
  "equation", "equation*", "align", "align*", "gather", "gather*",
  "multline", "multline*", "displaymath", "math", "eqnarray", "eqnarray*",
]);

// A \begin{...} or \end{...} at the scan position. Sticky, so it only matches
// where the scan is.
const ENVIRONMENT = /\\(begin|end)\{([^}]*)\}/y;

/// Whether offset `at` in LaTeX `text` is inside math. The text is read once
/// from the start, so comments and escapes before `at` are honored.
export function inLatexMath(text, at) {
  let inline = false; // $...$
  let display = false; // $$...$$
  let paren = false; // \(...\)
  let bracket = false; // \[...\]
  let env = null; // the open math environment, if any
  let i = 0;
  while (i < at) {
    const ch = text[i];
    if (ch === "%") {
      const end = text.indexOf("\n", i);
      i = end === -1 ? text.length : end + 1;
      continue;
    }
    if (ch === "\\") {
      ENVIRONMENT.lastIndex = i;
      const match = ENVIRONMENT.exec(text);
      if (match) {
        const [whole, kind, name] = match;
        if (kind === "begin" && env === null && MATH_ENVIRONMENTS.has(name)) env = name;
        else if (kind === "end" && name === env) env = null;
        i += whole.length;
        continue;
      }
      const next = text[i + 1];
      const plain = !inline && !display && !paren && !bracket && env === null;
      if (next === "(" && plain) paren = true;
      else if (next === ")" && paren) paren = false;
      else if (next === "[" && plain) bracket = true;
      else if (next === "]" && bracket) bracket = false;
      i += 2;
      continue;
    }
    if (ch === "$") {
      // Inside \( \), \[ \] or an environment, a dollar sign is just a character.
      if (paren || bracket || env !== null) {
        i += 1;
      } else if (display) {
        if (text[i + 1] === "$") {
          display = false;
          i += 2;
        } else {
          i += 1;
        }
      } else if (inline) {
        inline = false;
        i += 1;
      } else if (text[i + 1] === "$") {
        display = true;
        i += 2;
      } else {
        inline = true;
        i += 1;
      }
      continue;
    }
    i += 1;
  }
  return inline || display || paren || bracket || env !== null;
}

/// The transaction that toggles bold or italic around each selection. `kind`
/// is "bold" or "italic" and `format` is a key of MARKUP. Returns null for an
/// unknown format or kind.
export function toggleEmphasis(state, kind, format) {
  const table = Object.hasOwn(MARKUP, format) ? MARKUP[format] : null;
  if (!table || (kind !== "bold" && kind !== "italic")) return null;
  const doc = state.doc;
  const source = format === "latex" ? doc.toString() : "";
  const starred = format === "markdown" || format === "quarto";
  return {
    ...state.changeByRange((range) => {
      if (starred) return starEdit(doc, range, table[kind], kind);
      let pair = table[kind];
      if (format === "latex" && inLatexMath(source, range.from)) {
        pair = table[kind === "bold" ? "mathBold" : "mathItalic"];
      }
      return wrapEdit(doc, range, pair);
    }),
    scrollIntoView: true,
    userEvent: "input.format",
  };
}

/// A command that toggles `kind` in the format `formatFor()` returns. It does
/// nothing in a read-only editor or for a format with no markup.
export function emphasisCommand(kind, formatFor) {
  return (view) => {
    if (view.state.readOnly) return false;
    const spec = toggleEmphasis(view.state, kind, formatFor());
    if (!spec) return false;
    view.dispatch(spec);
    return true;
  };
}

// Text between two positions, or "" when the positions fall outside the doc.
function slice(doc, from, to) {
  return from >= 0 && to <= doc.length ? doc.sliceString(from, to) : "";
}

// Wrapping for formats whose markers differ from each other, such as LaTeX
// or HTML. A selection that already has the markers around it is unwrapped.
function wrapEdit(doc, range, [open, close]) {
  const { from, to } = range;
  const text = slice(doc, from, to);
  // The markers are inside the selection.
  if (text.length >= open.length + close.length && text.startsWith(open) && text.endsWith(close)) {
    return {
      changes: [
        { from, to: from + open.length, insert: "" },
        { from: to - close.length, to, insert: "" },
      ],
      range: EditorSelection.range(from, to - open.length - close.length),
    };
  }
  // The markers are just outside the selection.
  if (slice(doc, from - open.length, from) === open && slice(doc, to, to + close.length) === close) {
    return {
      changes: [
        { from: from - open.length, to: from, insert: "" },
        { from: to, to: to + close.length, insert: "" },
      ],
      range: EditorSelection.range(from - open.length, to - open.length),
    };
  }
  return wrapSelection(from, to, open, close);
}

// Markdown stars. `*` is a prefix of `**`, so the stars on each side are
// counted as a run and the run decides what is already there.
function starEdit(doc, range, [open, close], kind) {
  const { from, to } = range;
  const n = open.length;
  const text = slice(doc, from, to);
  const lead = text.match(/^\**/)[0].length;
  const trail = text.match(/\**$/)[0].length;
  // The stars are inside the selection, with some text between them.
  if (lead > 0 && trail > 0 && lead + trail < text.length && starsSay(kind, lead, trail)) {
    return {
      changes: [
        { from, to: from + n, insert: "" },
        { from: to - n, to, insert: "" },
      ],
      range: EditorSelection.range(from, to - 2 * n),
    };
  }
  // The stars are just outside the selection.
  if (starsSay(kind, starsBefore(doc, from), starsAfter(doc, to))) {
    return {
      changes: [
        { from: from - n, to: from, insert: "" },
        { from: to, to: to + n, insert: "" },
      ],
      range: EditorSelection.range(from - n, to - n),
    };
  }
  return wrapSelection(from, to, open, close);
}

// Whether a run of stars on each side means the emphasis is already there.
// Bold is two stars or more, italic is an odd count.
function starsSay(kind, left, right) {
  if (kind === "bold") return left >= 2 && right >= 2;
  return left % 2 === 1 && right % 2 === 1;
}

function starsBefore(doc, pos) {
  let count = 0;
  while (slice(doc, pos - count - 1, pos - count) === "*") count += 1;
  return count;
}

function starsAfter(doc, pos) {
  let count = 0;
  while (slice(doc, pos + count, pos + count + 1) === "*") count += 1;
  return count;
}

// Inserts the markers. The inner text stays selected, or the cursor sits
// between the markers when nothing was selected.
function wrapSelection(from, to, open, close) {
  return {
    changes: [
      { from, insert: open },
      { from: to, insert: close },
    ],
    range: EditorSelection.range(from + open.length, to + open.length),
  };
}
