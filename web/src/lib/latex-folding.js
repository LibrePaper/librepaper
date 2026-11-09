// Fold ranges for LaTeX sources, offered through CodeMirror's foldService.
//
// Two kinds of fold start on a line:
// - An environment: the first \begin{name} on the line folds from the end of
//   that line to the \end{name} that closes it (the fold stops just before
//   the \end). Same-name nesting is counted; other names are ignored.
// - A sectioning command at the start of a line (\part ... \subparagraph,
//   optionally starred) folds to the end of the line before the next heading
//   of the same or a higher level, \end{document}, or the end of the document.
//
// Comments (an unescaped % to the end of the line) are ignored. The bodies of
// verbatim-like environments are skipped, so \begin and \end inside them do not
// count. The scan starts at the line asked about, so a line that sits inside a
// verbatim body which began earlier is not recognised as such.

import { foldService } from "@codemirror/language";
import { VERBATIM_ENVS, LISTING_ENVS } from "./latex-language/tokens.js";

// The same environments the grammar treats as opaque, starred forms included.
const VERBATIM = new Set([...VERBATIM_ENVS, ...LISTING_ENVS].flatMap((name) => [name, `${name}*`]));
const SECTION_DEPTH = {
  part: 1,
  chapter: 2,
  section: 3,
  subsection: 4,
  subsubsection: 5,
  paragraph: 6,
  subparagraph: 7,
};
const SECTION = /^\s*\\(part|chapter|section|subsection|subsubsection|paragraph|subparagraph)\*?(?![A-Za-z])/;

/** Offset of the first unescaped % in a line, or -1. Same rule as completion/latex.js. */
function commentStart(line) {
  let slashes = 0;
  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    if (char === "%" && slashes % 2 === 0) return i;
    slashes = char === "\\" ? slashes + 1 : 0;
  }
  return -1;
}

/** @typedef {{ kind: "begin" | "end" | "section", name: string, depth?: number, line: number, from: number, to: number }} Token */

/**
 * Yield, in document order, the \begin and \end tokens and the sectioning
 * commands from startLine to the end of the document. Tokens are
 * { kind: "begin" | "end" | "section", name, depth, line, from, to }.
 * Comments and verbatim bodies produce no tokens, except the \end that closes
 * a verbatim body, which is yielded so the body's own fold can find it.
 * @param {import("@codemirror/state").Text} doc
 * @param {number} startLine
 * @returns {Generator<Token, undefined>}
 */
function* latexTokens(doc, startLine) {
  let verbatim = null;
  for (let n = startLine; n <= doc.lines; n++) {
    const line = doc.line(n);
    const raw = line.text;
    let from = 0;

    if (!verbatim) {
      const section = SECTION.exec(raw);
      if (section) {
        yield {
          kind: "section",
          name: section[1],
          depth: SECTION_DEPTH[section[1]],
          line: n,
          from: line.from,
          to: line.from + section[0].length,
        };
      }
    }

    while (true) {
      if (verbatim) {
        const tag = `\\end{${verbatim}}`;
        const at = raw.indexOf(tag, from);
        if (at < 0) break;
        yield { kind: "end", name: verbatim, line: n, from: line.from + at, to: line.from + at + tag.length };
        from = at + tag.length;
        verbatim = null;
      }

      // Only tokens before the comment marker count. The escape state starts
      // fresh at `from`, which is right once a verbatim body has closed.
      const comment = commentStart(raw.slice(from));
      const limit = comment < 0 ? raw.length : from + comment;
      const token = /\\(begin|end)\{([^}]*)\}/g;
      token.lastIndex = from;
      let match;
      let entered = false;
      while ((match = token.exec(raw)) && match.index + match[0].length <= limit) {
        const name = match[2].trim();
        const kind = /** @type {"begin" | "end"} */ (match[1]);
        yield {
          kind,
          name,
          line: n,
          from: line.from + match.index,
          to: line.from + match.index + match[0].length,
        };
        if (kind === "begin" && VERBATIM.has(name)) {
          verbatim = name;
          from = match.index + match[0].length;
          entered = true;
          break;
        }
      }
      if (!entered) break;
    }
  }
}

/** Fold from a line that starts with a sectioning command. */
function sectionFold(doc, line) {
  const tokens = latexTokens(doc, line.number);
  const heading = tokens.next().value;
  if (heading?.kind !== "section" || heading.line !== line.number) return null;

  let endLine = doc.lines;
  for (const token of tokens) {
    const stops = token.kind === "section"
      ? token.depth <= heading.depth
      : token.kind === "end" && token.name === "document";
    if (stops) {
      endLine = token.line - 1;
      break;
    }
  }
  // An empty or single-line body has nothing worth folding.
  if (endLine <= line.number) return null;
  return { from: line.to, to: doc.line(endLine).to };
}

/** Fold from a line that opens an environment, to the matching \end. */
function environmentFold(doc, line) {
  let open = null;
  let depth = 0;
  for (const token of latexTokens(doc, line.number)) {
    if (!open) {
      // The opening \begin must be on this line. Any other token ends the search.
      if (token.line !== line.number) return null;
      if (token.kind === "begin") open = token;
      continue;
    }
    if (token.name !== open.name) continue;
    if (token.kind === "begin") {
      depth++;
    } else if (depth > 0) {
      depth--;
    } else {
      // Closing on the same line is not a multi-line fold.
      return token.line > line.number ? { from: line.to, to: token.from } : null;
    }
  }
  return null;
}

/** Fold range for the line starting at lineStart, or null. */
function latexFoldRange(state, lineStart) {
  const line = state.doc.lineAt(lineStart);
  // The gutter asks about every visible line; most open nothing, and the scans
  // below would otherwise walk ahead to the next token for each of them.
  if (!line.text.includes("\\begin") && !SECTION.test(line.text)) return null;
  return sectionFold(state.doc, line) ?? environmentFold(state.doc, line);
}

/** CodeMirror extension that folds LaTeX environments and sections. */
export const latexFolding = foldService.of(latexFoldRange);
