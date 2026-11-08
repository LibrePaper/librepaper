// LaTeX completion. The trigger is read from the line before the cursor, and
// the project is read only after a trigger matches, so ordinary typing costs
// nothing here. Commands and environments come from LaTeX-Workshop's lists
// (see latex-workshop.json) and from latex-packages.js; everything the
// project defines or loads is read from its own sources.
import { snippet, snippetCompletion } from "@codemirror/autocomplete";
import workshop from "./latex-workshop.json" with { type: "json" };
import { CORE, PACKAGES } from "./latex-packages.js";
import { pathOptions } from "./paths.js";
import { indexTexts, labelList } from "../document-index/labels.js";
import { inMath } from "../latex-language/index.js";
import { SYMBOLS } from "../symbols/index.js";
import { m } from "../paraglide/messages.js";

const REFERENCE = /\\(?:ref|eqref|pageref|autoref|cref|Cref|nameref)\*?\{([^{}]*)$/;
const INCLUDE = /\\(input|include|subfile)\{([^{}]*)$/;
const GRAPHIC = /\\includegraphics(?:\[[^\]]*\])?\{([^{}]*)$/;
const BIBLIOGRAPHY = /\\(bibliography|addbibresource)\{([^{}]*)$/;
const PACKAGE = /\\usepackage(?:\[[^\]]*\])?\{([^{}]*)$/;
const ENVIRONMENT = /\\begin\{([A-Za-z*]*)$/;
const COMMAND = /(?<!\\)\\([A-Za-z]*)$/;

const DEFINED_COMMAND = /\\(?:re)?newcommand\*?\s*(?:\{\\([A-Za-z]+)\}|\\([A-Za-z]+))\s*(?:\[(\d)\](\s*\[)?)?/g;
const DEFINED_OPERATOR = /\\DeclareMathOperator\*?\s*\{\\([A-Za-z]+)\}/g;
const DEFINED_DEF = /\\def\\([A-Za-z]+)/g;
const DEFINED_ENVIRONMENT = /\\(?:re)?newenvironment\*?\{([A-Za-z]+\*?)\}/g;
const USED_PACKAGE = /\\usepackage\s*(?:\[[^\]]*\])?\s*\{([^{}]*)\}/g;

const GRAPHIC_EXTENSIONS = ["png", "jpg", "jpeg", "pdf", "eps", "svg"];
const FALLBACK_BODY = "\n\t${}";

// Index of the first % that starts a comment, or -1. A % after an odd number
// of backslashes is escaped.
function commentStart(line) {
  let slashes = 0;
  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    if (char === "%" && slashes % 2 === 0) return i;
    slashes = char === "\\" ? slashes + 1 : 0;
  }
  return -1;
}

function stripComment(line) {
  const at = commentStart(line);
  return at < 0 ? line : line.slice(0, at);
}

function codeOf(text) {
  return String(text).split("\n").map(stripComment).join("\n");
}

// Names the project defines or loads, read from every text.
function definitions(texts) {
  const commands = [], environments = [], packages = [];
  for (const text of Object.values(texts)) {
    const code = codeOf(text);
    for (const match of code.matchAll(DEFINED_COMMAND)) {
      // With a default, the first argument is optional and is left out.
      const arity = Number(match[3] || 0) - (match[4] ? 1 : 0);
      commands.push({ name: match[1] || match[2], arity: Math.max(0, arity) });
    }
    for (const match of code.matchAll(DEFINED_OPERATOR)) commands.push({ name: match[1], arity: 0 });
    for (const match of code.matchAll(DEFINED_DEF)) commands.push({ name: match[1], arity: 0 });
    for (const match of code.matchAll(DEFINED_ENVIRONMENT)) environments.push(match[1]);
    for (const match of code.matchAll(USED_PACKAGE)) {
      for (const name of match[1].split(",")) {
        const trimmed = name.trim();
        if (trimmed) packages.push(trimmed);
      }
    }
  }
  return { commands, environments, packages };
}

function loadedPackages(names) {
  return [...new Set(names)].filter((name) => Object.hasOwn(PACKAGES, name));
}

// One completion per name, the first one wins. This keeps project definitions
// ahead of the built-in lists.
function unique(entries) {
  const seen = new Set();
  const out = [];
  for (const entry of entries) {
    if (seen.has(entry.name)) continue;
    seen.add(entry.name);
    out.push(entry);
  }
  return out;
}

// Math or text mode. A command is offered by where the cursor is, by one rule
// (built-in names only; commands the project defines are always offered):
//  - Math-only commands are hidden outside math, where they are errors, and
//    ranked first inside it. They are every command the symbol catalog files
//    under Greek letters, operators, relations, arrows, logic, accents and
//    delimiters (a text symbol like \dag is in the catalog's text group and is
//    not math-only), plus MATH_STRUCTURE below for what the catalog has no tile for.
//  - Text-only commands (document structure, floats, lists, page layout) are
//    hidden inside math. Font commands such as \textbf, \text and \textrm stay
//    available in math, because they work there.
//  - Everything else is offered in both modes.
const MATH_CATEGORIES = new Set(["greek", "operators", "relations", "arrows", "logic", "accents", "delimiters"]);
// \underline and \overline also work in text mode.
const MATH_CATALOG_EXEMPT = new Set(["underline", "overline"]);
const MATH_STRUCTURE = [
  "frac", "dfrac", "tfrac", "cfrac", "binom", "dbinom", "tbinom", "sqrt", "left", "right", "middle",
  "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr", "biggl", "biggr", "Biggl", "Biggr",
  "mathbb", "mathcal", "mathfrak", "mathscr", "mathbf", "mathrm", "mathit", "mathsf", "mathtt",
  "mathnormal", "boldsymbol", "bm", "overbrace", "underbrace", "overset", "underset", "stackrel",
  "substack", "limits", "nolimits", "displaystyle", "textstyle", "scriptstyle", "scriptscriptstyle",
  "operatorname", "cdots", "ddots", "vdots", "to", "gets", "iff", "implies", "impliedby", "lvert", "rvert",
  "lVert", "rVert", "langle", "rangle", "lfloor", "rfloor", "lceil", "rceil", "tag", "nonumber", "notag",
  "mathop", "mathbin", "mathrel", "mathord", "mathopen", "mathclose", "mathpunct", "mathstrut",
  "widehat", "widetilde", "overrightarrow", "overleftarrow", "xrightarrow", "xleftarrow",
];
const TEXT_ONLY = new Set([
  "documentclass", "usepackage", "part", "chapter", "section", "section*", "subsection", "subsection*",
  "subsubsection", "subsubsection*", "paragraph", "subparagraph", "title", "author", "date", "maketitle",
  "tableofcontents", "listoffigures", "listoftables", "appendix", "appendixname", "bibliography",
  "bibliographystyle", "bibitem", "caption", "centering", "raggedright", "raggedleft", "itshape",
  "ttfamily", "item", "newpage", "clearpage", "cleardoublepage", "pagebreak", "nopagebreak", "linebreak",
  "nolinebreak", "newline", "noindent", "par", "vfill", "thispagestyle", "pagename", "today", "glossary",
  "makeglossary", "makeindex", "footnotemark", "inputlineno", "kill", "hlinefill",
]);
const MATH_ONLY = new Set(MATH_STRUCTURE);
for (const symbol of SYMBOLS) {
  if (!MATH_CATEGORIES.has(symbol.category)) continue;
  for (const match of symbol.latex.matchAll(/\\([A-Za-z]+)/g)) {
    if (!MATH_CATALOG_EXEMPT.has(match[1])) MATH_ONLY.add(match[1]);
  }
}

// Environments that only exist inside math. Inside math only these are offered.
const MATH_INNER_ENVIRONMENTS = new Set([
  "aligned", "alignedat", "gathered", "split", "cases", "dcases", "rcases", "drcases", "array", "subarray",
  "matrix", "pmatrix", "bmatrix", "Bmatrix", "vmatrix", "Vmatrix", "smallmatrix", "psmallmatrix",
  "bsmallmatrix", "Bsmallmatrix", "vsmallmatrix", "Vsmallmatrix",
]);

function commandEntry(name, snippetText, detail, project = false) {
  const completion = { label: name, type: "function", detail };
  return { name, project, option: snippetText === undefined ? completion : snippetCompletion(snippetText, completion) };
}

// The commands that fit the mode, math-only ones ranked first inside math.
function forMode(entries, math) {
  const out = [];
  for (const entry of entries) {
    if (entry.project) { out.push(entry.option); continue; }
    if (math) {
      if (TEXT_ONLY.has(entry.name)) continue;
      out.push(MATH_ONLY.has(entry.name) ? { ...entry.option, boost: 10 } : entry.option);
    } else if (!MATH_ONLY.has(entry.name)) {
      out.push(entry.option);
    }
  }
  return out;
}

function environmentEntry(name, body, detail, project = false) {
  return { name, body, detail, project };
}

function arityTemplate(arity) {
  return Array.from({ length: arity }, () => "{${}}").join("");
}

// Built once at module load: the LaTeX-Workshop lists and each package's list.
const WORKSHOP_COMMANDS = workshop.commands.map((item) => commandEntry(item.name, item.snippet, item.detail));
const WORKSHOP_ENVIRONMENTS = workshop.environments.map((item) => environmentEntry(item.name, item.snippet, "environment"));
const CORE_COMMANDS = CORE.commands.map((item) => commandEntry(item.name, item.snippet, item.detail ?? "LaTeX"));
const CORE_ENVIRONMENTS = CORE.environments.map((item) => environmentEntry(item.name, item.snippet, "environment"));
const PACKAGE_COMMANDS = Object.fromEntries(Object.entries(PACKAGES).map(([name, entry]) => [
  name,
  entry.commands.map((item) => commandEntry(item.name, item.snippet, item.detail ?? name)),
]));
const PACKAGE_ENVIRONMENTS = Object.fromEntries(Object.entries(PACKAGES).map(([name, entry]) => [
  name,
  entry.environments.map((item) => environmentEntry(item.name, item.snippet, name)),
]));
const PACKAGE_OPTIONS = Object.keys(PACKAGES).map((label) => ({ label, type: "namespace", detail: m.latex_package_detail() }));

// Completes \begin{name} with its closing brace, the body, and the matching
// \end unless the next line already has one. closeBrackets() may have
// put a "}" after the cursor, so that brace is replaced too.
function environmentOption(entry, closed) {
  const tail = closed ? "" : `\n\\end{${entry.name}}`;
  const template = `${entry.name}}${entry.body ?? FALLBACK_BODY}${tail}`;
  const option = snippetCompletion(template, { label: entry.name, type: "keyword", detail: entry.detail });
  const insert = snippet(template);
  return {
    ...option,
    apply(view, completion, from, to) {
      const extend = view.state.doc.sliceString(to, to + 1) === "}";
      insert(view, completion, from, extend ? to + 1 : to);
    },
  };
}

// The text of a comma list after the last comma, without leading spaces.
function listToken(inner) {
  return inner.slice(inner.lastIndexOf(",") + 1).replace(/^\s+/, "");
}

function referenceResult(context, p, inner) {
  const token = listToken(inner);
  const formats = new Set(["latex"]);
  const texts = { ...p.texts, [p.path]: context.state.doc.toString() };
  // The editor's live index already holds the edited document: it refreshes
  // before answering.
  const labels = p.labels
    ? p.labels({ first: p.path, formats })
    : labelList(indexTexts(texts), texts, { first: p.path, formats });
  const options = labels.map((label) => ({
    label: label.id,
    type: "variable",
    detail: `${label.path}:${label.line}`,
    ...(label.title ? { info: label.title } : {}),
  }));
  return { from: context.pos - token.length, options, validFor: /^[^{},]*$/ };
}

/**
 * Completion source for LaTeX. Returns null for any other format.
 * @param {() => { path: string, format: string, texts: Record<string, string>, files: string[] }} project
 */
export function latexCompletion(project) {
  return (context) => {
    const p = project();
    if (p.format !== "latex") return null;
    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, context.pos - line.from);
    if (commentStart(before) >= 0) return null;

    let match = REFERENCE.exec(before);
    if (match) return referenceResult(context, p, match[1]);

    match = INCLUDE.exec(before);
    if (match) {
      return {
        from: context.pos - match[2].length,
        options: pathOptions(p.files, "", { extensions: ["tex"], dropExtension: true, exclude: p.path }),
        validFor: /^[^{}]*$/,
      };
    }

    match = GRAPHIC.exec(before);
    if (match) {
      return {
        from: context.pos - match[1].length,
        options: pathOptions(p.files, "", { extensions: GRAPHIC_EXTENSIONS }),
        validFor: /^[^{}]*$/,
      };
    }

    match = BIBLIOGRAPHY.exec(before);
    if (match) {
      const token = listToken(match[2]);
      return {
        from: context.pos - token.length,
        options: pathOptions(p.files, "", { extensions: ["bib"], dropExtension: match[1] === "bibliography" }),
        validFor: /^[^{},]*$/,
      };
    }

    match = PACKAGE.exec(before);
    if (match) {
      const token = listToken(match[1]);
      return { from: context.pos - token.length, options: PACKAGE_OPTIONS, validFor: /^[^{},]*$/ };
    }

    match = ENVIRONMENT.exec(before);
    if (match) {
      const texts = { ...p.texts, [p.path]: context.state.doc.toString() };
      const found = definitions(texts);
      const packages = loadedPackages(found.packages);
      const entries = unique([
        ...found.environments.map((name) => environmentEntry(name, undefined, m.latex_defined_in_project(), true)),
        ...CORE_ENVIRONMENTS,
        ...WORKSHOP_ENVIRONMENTS,
        ...packages.flatMap((name) => PACKAGE_ENVIRONMENTS[name]),
      ]);
      const math = inMath(context.state, context.pos);
      const shown = math ? entries.filter((entry) => entry.project || MATH_INNER_ENVIRONMENTS.has(entry.name)) : entries;
      const next = line.number < context.state.doc.lines ? context.state.doc.line(line.number + 1).text : "";
      return {
        from: context.pos - match[1].length,
        options: shown.map((entry) => environmentOption(entry, next.includes(`\\end{${entry.name}}`))),
        validFor: /^[A-Za-z*]*$/,
      };
    }

    match = COMMAND.exec(before);
    if (match) {
      const texts = { ...p.texts, [p.path]: context.state.doc.toString() };
      const found = definitions(texts);
      const packages = loadedPackages(found.packages);
      const entries = unique([
        ...found.commands.map((item) => commandEntry(
          item.name,
          item.arity ? `${item.name}${arityTemplate(item.arity)}` : undefined,
          m.latex_defined_in_project(),
          true,
        )),
        ...CORE_COMMANDS,
        ...WORKSHOP_COMMANDS,
        ...packages.flatMap((name) => PACKAGE_COMMANDS[name]),
      ]);
      return {
        from: context.pos - match[1].length,
        options: forMode(entries, inMath(context.state, context.pos)),
        validFor: /^[A-Za-z]*$/,
      };
    }
    return null;
  };
}
