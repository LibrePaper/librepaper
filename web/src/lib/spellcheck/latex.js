// Markup that the spellchecker skips in LaTeX source. The ranges come from the
// parse tree of our own LaTeX grammar, so the editor, the document index and
// the spellchecker agree on what is a command, a group, math, verbatim text or
// a comment. One walk over the tree, so the scan is linear.
import { latexTree } from "../source-trees.js";

// Commands whose first braced argument names a key, file or package, not words.
const NAME_ARGUMENT = new Set([
  "label", "input", "include", "includeonly", "subfile", "subfileinclude",
  "includegraphics", "usepackage",
  "RequirePackage", "documentclass", "bibliography", "bibliographystyle",
  "addbibresource", "url", "href", "begin", "end", "newcommand",
  "renewcommand", "providecommand", "newenvironment", "renewenvironment",
  "setlength", "addtolength", "setcounter", "addtocounter", "newcounter",
  "pagestyle", "thispagestyle", "hypersetup", "graphicspath",
  "lstinputlisting", "inputminted", "usetikzlibrary", "definecolor", "color",
  "textcolor", "author",
]);

// Definition commands are followed by an optional count and a body, as in
// \newcommand{\R}[1]{...}. The body is markup too.
const DEFINITIONS = new Set([
  "newcommand", "renewcommand", "providecommand",
  "newenvironment", "renewenvironment",
]);

// Link and cite commands whose braced argument is visible text, not a key.
const PROSE_REFERENCES = new Set(["hyperref", "citetext"]);

// Commands whose optional [...] argument holds words the reader sees, so only
// the command name is skipped and the option text stays prose.
const PROSE_OPTIONS = new Set([
  "item", "section", "subsection", "subsubsection", "chapter", "part",
  "paragraph", "subparagraph", "caption", "footnote", "title",
]);

// Environments whose body is not prose. The math ones also come starred.
const MATH_ENVIRONMENTS = [
  "equation", "align", "alignat", "gather", "multline", "flalign",
  "eqnarray", "math", "displaymath",
];
const SKIPPED_ENVIRONMENTS = new Set([
  ...MATH_ENVIRONMENTS,
  ...MATH_ENVIRONMENTS.map((name) => `${name}*`),
  "verbatim", "Verbatim", "Verbatim*", "BVerbatim", "LVerbatim", "lstlisting",
  "minted", "comment", "tikzpicture",
  // Code chunks from literate and knitr documents: Journal of Statistical
  // Software, Sweave, knitr, and the code output that Pandoc writes.
  "CodeChunk", "CodeInput", "CodeOutput", "Schunk", "Sinput", "Soutput",
  "Scode", "Highlighting", "Shaded", "kframe", "knitrout",
  // Python and Lua code embedded in a document.
  "pycode", "luacode", "luacode*",
  // Files written out by the document itself.
  "filecontents", "filecontents*",
]);

// Commands whose leading braced arguments are lengths or specs, not words. The
// number is how many braced groups are markup. Optional groups before each of
// them are markup too, and the first braced group after the last one is prose.
const MARKUP_ARGUMENTS = new Map([
  ["vspace", 1], ["hspace", 1], ["addvspace", 1], ["linespread", 1],
  ["scalebox", 1], ["parbox", 1], ["raisebox", 1], ["cline", 1],
  ["cmidrule", 1], ["rotatebox", 1], ["enlargethispage", 1],
  ["rule", 2], ["fontsize", 2], ["resizebox", 2], ["multicolumn", 2],
  ["multirow", 2],
]);

// Table and box environments whose leading arguments are specs or widths. The
// number is how many braced groups follow \begin{NAME} as markup. The body is
// prose, so only the begin tag and these groups are skipped.
const SPEC_ENVIRONMENTS = new Map([
  ["tabular", 1], ["longtable", 1], ["array", 1], ["tblr", 1], ["longtblr", 1],
  ["talltblr", 1], ["minipage", 1], ["multicols", 1], ["multicols*", 1],
  ["supertabular", 1], ["xtabular", 1],
  ["tabular*", 2], ["tabularx", 2], ["tabulary", 2], ["wrapfigure", 2],
  ["wraptable", 2],
]);

// An unbraced file name after \input, as in \input chapters/b.tex. It runs to
// a blank, a comment, a command or a group, as TeX reads it.
const FILE_NAME = /[^ \t\r\n\f\v%\\{}]+/y;
// A blank line: the line end that starts a paragraph break.
const BLANK_LINE = /\n[ \t]*\n/g;

// Nodes whose whole span is markup: comments, math and \verb come from the
// grammar's own tokens, so nothing inside them is read again.
const WHOLE_NODES = new Set([
  "Comment", "DisplayMath", "MathEnvironment", "Verb", "EndEnv",
]);
// The braced nodes that can be the argument of a command.
const GROUPS = new Set(["Group", "MathGroup", "UrlArg"]);

/**
 * Ranges of a LaTeX source that are markup rather than prose, for the
 * spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16, as
 * JavaScript strings index). Any order; overlaps allowed.
 * @param {string} text
 * @returns {Array<[number, number]>}
 */
export function latexSkips(text) {
  if (typeof text !== "string") return [];
  const n = text.length;
  const skips = [];
  // Everything before this offset is already skipped, so the walk does not
  // enter nodes that end there. A node that straddles it is entered, so what
  // follows an unclosed group is still read.
  let skipTo = 0;
  // The last blank-line search. No blank line starts from `blankFrom` up to
  // `blankAt`, which is the one found there, or the text end.
  let blankFrom = -1;
  let blankAt = -1;

  function push(from, to) {
    if (to > n) to = n;
    if (to > from) skips.push([from, to]);
  }

  // Skips a range and keeps the walk from re-reading it.
  function skip(from, to) {
    push(from, to);
    if (to > skipTo) skipTo = to;
  }

  // Index of the line end that starts the first blank line at or after `from`,
  // or the text end. Memoized, so unclosed groups never rescan the same text.
  function blankLine(from) {
    if (from >= blankFrom && from <= blankAt) return blankAt;
    BLANK_LINE.lastIndex = from;
    const hit = BLANK_LINE.exec(text);
    blankFrom = from;
    blankAt = hit ? hit.index : n;
    return blankAt;
  }

  function lineEnd(from) {
    const at = text.indexOf("\n", from);
    return at < 0 ? n : at;
  }

  // A node closed by a real closing token. A parser that recovers from a
  // missing closer leaves a zero-length one, which does not count.
  function closedBy(node, closer) {
    const last = node.lastChild;
    return last !== null && last.name === closer && last.to > last.from;
  }

  // Index of the first character after the blanks at `at`: spaces and tabs,
  // then at most one newline, then spaces and tabs.
  function argumentStart(at) {
    let k = at;
    while (text[k] === " " || text[k] === "\t") k++;
    if (text[k] === "\n") {
      k++;
      while (text[k] === " " || text[k] === "\t") k++;
    }
    return k;
  }

  // Like argumentStart. With `comments`, a "%" comment and its line end also
  // count as blanks, so a table spec may run over lines of comments.
  function gapEnd(at, comments) {
    let k = argumentStart(at);
    while (comments && text[k] === "%") k = argumentStart(lineEnd(k));
    return k;
  }

  // A lookup of the node that covers an offset, for the siblings after
  // `start`. Offsets only grow, so the lookup moves forward and each sibling is
  // passed once. A command's optional argument is its child, so the lookup
  // leaves the command for its next sibling when the children run out.
  function siblings(start) {
    let node = start;
    return (k) => {
      while (node && node.to <= k) {
        node = node.nextSibling ?? (node.parent?.name === "Command" ? node.parent.nextSibling : null);
      }
      return node;
    };
  }

  // Index just past the [...] group that opens at `open`, or -1 when none does.
  // Directly after a command or \begin tag the grammar made it an optional
  // argument. After blanks or a star it is plain text, and then the "]" must be
  // in the same text node. A "[" with no closing "]" is prose.
  function optionEnd(open, at) {
    if (text[open] !== "[") return -1;
    const node = at(open);
    if (!node || node.from > open) return -1;
    if (node.name === "OptionalArgument") {
      return node.from === open && closedBy(node, "OptClose") ? node.to : -1;
    }
    if (node.name !== "Text") return -1;
    for (let k = open + 1; k < node.to; k++) if (text[k] === "]") return k + 1;
    return -1;
  }

  // Index just past the [...] groups that follow `from`. With `spaced`, blanks
  // may come before each group, as in \cite [note] {key}.
  function skipOptions(from, spaced, at) {
    let k = from;
    for (;;) {
      const end = optionEnd(spaced ? argumentStart(k) : k, at);
      if (end < 0) return k;
      k = end;
    }
  }

  // Index just past the braced group that opens at `open`. A group the grammar
  // closed ends at its brace. One it did not close, or a half-typed \cite{
  // whose brace is only found paragraphs later, ends at the next blank line,
  // so it hides one paragraph at most.
  function groupEnd(open, at) {
    const node = at(open);
    let end = n;
    if (node && node.from === open && GROUPS.has(node.name)) {
      if (node.name === "UrlArg" || closedBy(node, "CloseBrace")) end = node.to;
    }
    const blank = blankLine(open);
    return blank < end ? blank : end;
  }

  // Index just past the leading groups of a command or environment whose
  // arguments are markup. Each of the first `count` braced groups may have
  // optional [...] groups before it. With `comments`, blanks and comments may
  // separate the groups. A missing group ends the run at the last group taken.
  function markupArguments(from, count, comments, at) {
    let k = from;
    for (let g = 0; g < count; g++) {
      let open = gapEnd(k, comments);
      for (let end = optionEnd(open, at); end >= 0; end = optionEnd(open, at)) {
        open = gapEnd(end, comments);
      }
      if (text[open] !== "{") break;
      k = groupEnd(open, at);
    }
    return k;
  }

  // Index just past a "(...)" trim spec at `from` whose ")" is on the same line
  // and in the same text node, as \cmidrule(lr) has. Without one, `from`.
  function trimSpec(from, at) {
    if (text[from] !== "(") return from;
    const node = at(from);
    if (!node || node.name !== "Text" || node.from > from) return from;
    for (let k = from + 1; k < node.to && text[k] !== "\n"; k++) {
      if (text[k] === ")") return k + 1;
    }
    return from;
  }

  // Index just past an unbraced file name at `from`, after spaces and tabs.
  // Without a name, the index is just past the blanks, which are not words
  // either.
  function fileName(from) {
    let k = from;
    while (text[k] === " " || text[k] === "\t") k++;
    FILE_NAME.lastIndex = k;
    const match = FILE_NAME.exec(text);
    return match ? k + match[0].length : k;
  }

  function takesName(name) {
    if (PROSE_REFERENCES.has(name)) return false;
    return NAME_ARGUMENT.has(name) || name.includes("cite") || name.endsWith("ref");
  }

  // Index just past a command with name-like arguments. Optional groups and the
  // first braced group are skipped, and a definition's body after them. Blanks
  // may separate the command from each argument. When no argument follows, the
  // blanks are left in the prose. An unbraced \input name ends the command, so
  // a bracket after the file name is prose.
  function argumentCommand(name, from, at) {
    let k = skipOptions(from, true, at);
    const brace = argumentStart(k);
    if (text[brace] === "{") k = groupEnd(brace, at);
    else if (name === "input") return fileName(k);
    k = skipOptions(k, true, at);
    const body = argumentStart(k);
    if (DEFINITIONS.has(name) && text[body] === "{") k = groupEnd(body, at);
    return k;
  }

  // A control word or sectioning word with the arguments that are markup.
  // Returns after recording the skip; prose arguments are left to the walk.
  function command(node, first) {
    const star = first.name === "ControlSequence" && text[first.to] === "*";
    const nameEnd = star ? first.to + 1 : first.to;
    // A sectioning word's token already holds its star.
    const name = text.slice(first.from + 1, first.to).replace(/\*$/, "");
    const at = siblings(first.nextSibling ?? node.nextSibling);

    const markup = MARKUP_ARGUMENTS.get(name);
    if (markup !== undefined) {
      const from = name === "cmidrule" ? trimSpec(nameEnd, at) : nameEnd;
      return skip(first.from, markupArguments(from, markup, false, at));
    }
    if (takesName(name)) return skip(first.from, argumentCommand(name, nameEnd, at));
    // The words of these options are prose: only the command name is markup.
    if (PROSE_OPTIONS.has(name)) return skip(first.from, nameEnd);
    skip(first.from, skipOptions(nameEnd, false, at));
  }

  // A \begin tag: it is markup with its optional argument, and a table or box
  // environment's spec groups too. The body after them stays prose.
  function beginTag(node) {
    const name = node.getChild("EnvName");
    const env = name ? text.slice(name.from, name.to).trim() : "";
    const count = SPEC_ENVIRONMENTS.get(env);
    const at = siblings(node.nextSibling);
    if (count !== undefined) return skip(node.from, markupArguments(node.to, count, true, at));
    skip(node.from, skipOptions(node.to, false, at));
  }

  // Math that was never closed. A missing "$" or "\)" ends at the line end, so
  // one stray dollar cannot hide the rest of the document.
  function inlineMath(node) {
    const closer = node.lastChild?.name;
    const closed = (closer === "DollarClose" || closer === "ParenClose") && node.lastChild.to > node.lastChild.from;
    skip(node.from, closed ? node.to : Math.min(node.to, lineEnd(node.from)));
  }

  latexTree(text).iterate({
    enter(ref) {
      if (ref.to <= skipTo) return false;
      const name = ref.name;
      if (WHOLE_NODES.has(name)) {
        skip(ref.from, ref.to);
        return false;
      }
      switch (name) {
        case "InlineMath":
          inlineMath(ref.node);
          return false;
        case "VerbatimEnvironment": {
          // An unclosed one runs to the end of the text.
          const node = ref.node;
          skip(ref.from, closedBy(node, "EndEnv") ? ref.to : n);
          return false;
        }
        case "Environment": {
          const node = ref.node;
          const tag = node.firstChild?.getChild("EnvName");
          if (tag && SKIPPED_ENVIRONMENTS.has(text.slice(tag.from, tag.to).trim())) {
            skip(ref.from, ref.to);
            return false;
          }
          return true;
        }
        case "BeginEnv":
          beginTag(ref.node);
          return false;
        case "Command": {
          const node = ref.node;
          const first = node.firstChild;
          if (first && first.name !== "ControlSymbol") {
            command(node, first);
            return true;
          }
          // A control symbol such as \\, \% or \$ is skipped, with the
          // [...] options that directly follow, as in \\[2pt].
          if (first) skip(first.from, skipOptions(first.to, false, siblings(node.nextSibling)));
          return false;
        }
        default:
          return true;
      }
    },
  });
  return skips;
}
