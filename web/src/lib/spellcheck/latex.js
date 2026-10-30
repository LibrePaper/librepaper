// Markup that the spellchecker skips in LaTeX source. One left-to-right pass:
// each construct is consumed in a single jump, so the scan is linear and no
// character is read twice.

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

// A command name: letters and @, with an optional trailing star.
const COMMAND_NAME = /[A-Za-z@]+\*?/y;
// The {name} after \begin, with blanks allowed before it: spaces and tabs, then
// at most one newline. Nothing here nests, so it cannot backtrack badly.
const ENVIRONMENT_NAME = /[ \t]*(?:\n[ \t]*)?\{([^{}\n]*)\}/y;
// The blanks allowed before an argument, as a regular expression source.
const BLANK = "[ \\t]*(?:\\n[ \\t]*)?";
// The start of an \end tag, as a regular expression source.
const END_TAG = "\\\\end";
// An unbraced file name after \input, as in \input chapters/b.tex. It runs to
// a blank, a comment, a command or a group, as TeX reads it.
const FILE_NAME = /[^ \t\r\n\f\v%\\{}]+/y;

// Escapes a string so that it matches itself inside a regular expression.
function escapeRe(s) {
  return s.replace(/[\\^$.*+?()[\]{}|]/g, "\\$&");
}

/**
 * Ranges of a LaTeX source that are markup rather than prose, for the
 * spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16, as
 * JavaScript strings index). Any order; overlaps allowed.
 * @param {string} text
 * @returns {Array<[number, number]>}
 */
export function latexSkips(text) {
  const n = text.length;
  const skips = [];
  // The last line-end search. No newline lies from `eolFrom` up to `eolAt`,
  // which is the newline found there, or the text end.
  let eolFrom = -1;
  let eolAt = -1;

  function push(from, to) {
    skips.push([from, to]);
    return to;
  }

  function lineEnd(from) {
    if (from >= eolFrom && from <= eolAt) return eolAt;
    const at = text.indexOf("\n", from);
    eolFrom = from;
    eolAt = at < 0 ? n : at;
    return eolAt;
  }

  // A search for one pattern that remembers its last result. A match at
  // `hit.index` answers every call from `start` up to that index, and a miss
  // answers every later call, because no match starts in between. A call that
  // moves backward searches again.
  function matcher(source) {
    const re = new RegExp(source, "g");
    let start = -1;
    let hit = null;
    return (from) => {
      if (start >= 0 && from >= start && (hit === null || from <= hit.index)) return hit;
      start = from;
      re.lastIndex = from;
      hit = re.exec(text);
      return hit;
    };
  }

  // The first "]" at or after a position. Without the memo, every unclosed "["
  // would rescan to the end of the text.
  const nextClose = matcher("\\]");
  // The first ")" at or after a position, memoized the same way.
  const nextParen = matcher("\\)");

  // One matcher per verbatim delimiter and per skipped environment, so each
  // search over a stretch of text happens at most once per pattern.
  const verbClosers = new Map();
  const envClosers = new Map();

  function verbCloser(ch) {
    if (!verbClosers.has(ch)) verbClosers.set(ch, matcher(escapeRe(ch)));
    return verbClosers.get(ch);
  }

  function envCloser(name) {
    if (!envClosers.has(name)) {
      envClosers.set(name, matcher(`${END_TAG}${BLANK}\\{${escapeRe(name)}\\}`));
    }
    return envClosers.get(name);
  }

  // Index of the first character after the blanks at `at`: spaces and tabs,
  // then at most one newline, then spaces and tabs. The caller decides whether
  // those blanks are consumed.
  function argumentStart(at) {
    let k = at;
    while (text[k] === " " || text[k] === "\t") k++;
    if (text[k] === "\n") {
      k++;
      while (text[k] === " " || text[k] === "\t") k++;
    }
    return k;
  }

  // Index of the first character after the blanks at `at`. With `comments`, a
  // "%" comment and its line end also count as blanks, so a table spec may
  // run over lines of comments.
  function gapEnd(at, comments) {
    let k = argumentStart(at);
    while (comments && text[k] === "%") k = argumentStart(lineEnd(k));
    return k;
  }

  // Index just past an unbraced file name at `at`, after spaces and tabs. Without
  // a name, the index is just past the blanks, which are not words either.
  function fileName(at) {
    let k = at;
    while (text[k] === " " || text[k] === "\t") k++;
    FILE_NAME.lastIndex = k;
    const match = FILE_NAME.exec(text);
    return match ? k + match[0].length : k;
  }

  // Index just past the [...] groups that directly follow `at`. A "[" with no
  // closing "]" is prose and is left alone. With `spaced`, blanks may come
  // before each group, as in \cite [note] {key}.
  function skipOptions(at, spaced) {
    let k = at;
    for (;;) {
      const open = spaced ? argumentStart(k) : k;
      if (text[open] !== "[") break;
      const hit = nextClose(open + 1);
      if (!hit) break;
      k = hit.index + 1;
    }
    return k;
  }

  // Index just past the balanced {...} group that opens at `at`. An unclosed
  // group ends at the next blank line, so a half-typed \cite{ hides one
  // paragraph at most. Without a blank line it runs to the end of the text.
  function skipGroup(at) {
    let depth = 0;
    for (let k = at; k < n; k++) {
      const ch = text[k];
      if (ch === "\\") k++;
      else if (ch === "{") depth++;
      else if (ch === "}" && --depth === 0) return k + 1;
      else if (ch === "\n") {
        let j = k + 1;
        while (text[j] === " " || text[j] === "\t") j++;
        if (text[j] === "\n") return k;
      }
    }
    return n;
  }

  // Index just past the closer of a math run whose body starts at `at`. Inline
  // runs end at the line end when unclosed, so one missing "$" cannot hide the
  // rest of the document. Display runs go to the end of the text.
  function mathEnd(at, closer, display) {
    let k = at;
    while (k < n) {
      if (text.startsWith(closer, k)) return k + closer.length;
      const ch = text[k];
      if (ch === "\\") k += !display && text[k + 1] === "\n" ? 1 : 2;
      else if (ch === "\n" && !display) return k;
      else k++;
    }
    return n;
  }

  // Index just past a \verb or \verb* whose delimiter sits at `at`. A closing
  // delimiter at or past the line end counts as unclosed, so the \verb ends at
  // its line end.
  function verb(at) {
    const eol = lineEnd(at);
    if (at >= eol) return at;
    const hit = verbCloser(text[at])(at + 1);
    return hit && hit.index < eol ? hit.index + 1 : eol;
  }

  // Index just past the leading groups of a command or environment whose
  // arguments are markup. Each of the first `count` braced groups may have
  // optional [...] groups before it. With `comments`, blanks and comments may
  // separate the groups. A missing group ends the run at the last group taken.
  function markupArguments(at, count, comments) {
    let k = at;
    for (let g = 0; g < count; g++) {
      let open = gapEnd(k, comments);
      while (text[open] === "[") {
        const hit = nextClose(open + 1);
        if (!hit) break;
        open = gapEnd(hit.index + 1, comments);
      }
      if (text[open] !== "{") break;
      k = skipGroup(open);
    }
    return k;
  }

  // Index just past a "(...)" trim spec at `at` whose ")" is on the same line,
  // as \cmidrule(lr) has. Without one, the index is `at`.
  function trimSpec(at) {
    if (text[at] !== "(") return at;
    const hit = nextParen(at + 1);
    return hit && hit.index < lineEnd(at) ? hit.index + 1 : at;
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
  function argumentCommand(name, at) {
    let k = skipOptions(at, true);
    const brace = argumentStart(k);
    if (text[brace] === "{") k = skipGroup(brace);
    else if (name === "input") return fileName(k);
    k = skipOptions(k, true);
    const body = argumentStart(k);
    if (DEFINITIONS.has(name) && text[body] === "{") k = skipGroup(body);
    return k;
  }

  // Handles the backslash at `i`. The caller guarantees a character follows it.
  function command(i) {
    COMMAND_NAME.lastIndex = i + 1;
    const match = COMMAND_NAME.exec(text);
    if (!match) {
      // A control symbol such as \\, \%, \$ or \{ is one non-letter character.
      // \( and \[ open math, and \$ must not open it, so they are checked here.
      const symbol = text[i + 1];
      if (symbol === "(") return push(i, mathEnd(i + 2, "\\)", false));
      if (symbol === "[") return push(i, mathEnd(i + 2, "\\]", true));
      const width = text.codePointAt(i + 1) > 0xffff ? 3 : 2;
      return push(i, skipOptions(i + width, false));
    }
    const token = match[0];
    const name = token.endsWith("*") ? token.slice(0, -1) : token;
    const at = i + 1 + token.length;
    if (name === "verb") return push(i, verb(at));
    if (name === "begin" || name === "end") {
      ENVIRONMENT_NAME.lastIndex = at;
      const env = ENVIRONMENT_NAME.exec(text);
      if (name === "begin" && env && SKIPPED_ENVIRONMENTS.has(env[1])) {
        // The body is skipped whole, so nothing inside it is scanned. The
        // closing tag may have blanks before its name, as in \end {equation}.
        const hit = envCloser(env[1])(at + env[0].length);
        return push(i, hit ? hit.index + hit[0].length : n);
      }
      if (name === "begin" && env && SPEC_ENVIRONMENTS.has(env[1])) {
        // The spec groups are markup, and the body after them stays prose.
        const count = SPEC_ENVIRONMENTS.get(env[1]);
        return push(i, markupArguments(at + env[0].length, count, true));
      }
    }
    const markup = MARKUP_ARGUMENTS.get(name);
    if (markup !== undefined) {
      const from = name === "cmidrule" ? trimSpec(at) : at;
      return push(i, markupArguments(from, markup, false));
    }
    if (takesName(name)) return push(i, argumentCommand(name, at));
    if (PROSE_OPTIONS.has(name)) return push(i, at);
    return push(i, skipOptions(at, false));
  }

  let i = 0;
  while (i < n) {
    const ch = text[i];
    if (ch === "%") {
      i = push(i, lineEnd(i));
    } else if (ch === "$") {
      i = push(i, text[i + 1] === "$" ? mathEnd(i + 2, "$$", true) : mathEnd(i + 1, "$", false));
    } else if (ch === "\\" && i + 1 < n) {
      i = command(i);
    } else {
      i++;
    }
  }
  return skips;
}
