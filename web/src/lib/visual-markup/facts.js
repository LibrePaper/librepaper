// The facts of a LaTeX project that the visual layer draws from: reference
// numbers, citations, acronyms, macros and the citation style. The editor
// recomputes them on each edit pause, so each part is a pass over the text, and
// the numbering pass is shared by the labels, the theorems and the citations.

import { rankEntries } from "../bibliography.js";
import { numberLabels, stripComments, theoremDefinitions } from "./labels.js";

const NEWACRONYM = /\\newacronym\s*(?:\[[^\]]*\]\s*)?\{([^{}]*)\}\s*\{((?:[^{}]|\{[^{}]*\})*)\}\s*\{((?:[^{}]|\{[^{}]*\})*)\}/g;
const ACRO = /\\acro\s*\{([^{}]*)\}\s*(?:\[([^\]]*)\])?\s*\{((?:[^{}]|\{[^{}]*\})*)\}/g;
const DECLARE_ACRONYM = /\\DeclareAcronym\s*\{([^{}]*)\}\s*\{((?:[^{}]|\{[^{}]*\})*)\}/g;
const GLOSSARY_ENTRY = /\\newglossaryentry\s*\{([^{}]*)\}\s*\{((?:[^{}]|\{[^{}]*\})*)\}/g;

// A definition head: \newcommand, \renewcommand, \providecommand (group 1), the
// xparse forms (group 2), \def (group 3) and \DeclareMathOperator (group 4, the star).
const DEFINE = /\\(?:(new|renew|provide)command\*?|(New|Renew|Provide)DocumentCommand|(def)(?=\\)|DeclareMathOperator(\*?))/g;
const MACRO_NAME = /\s*(?:\{\s*\\([A-Za-z@]+)\s*\}|\\([A-Za-z@]+))/y;

const CITE_STYLE_NUMERIC = /^(?:numeric|ieee|nature|vancouver|science|alphabetic)/;
const CITE_STYLE_AUTHOR_YEAR = /^(?:authoryear|authordate|authortitle|apa|chicago|harvard|mla)/;
const BIBSTYLE = new Map([
  ["plain", "numeric"],
  ["unsrt", "numeric"],
  ["ieeetr", "numeric"],
  ["abbrv", "numeric"],
  ["alpha", "numeric"],
  ["siam", "numeric"],
  ["acm", "numeric"],
  ["plainnat", "author-year"],
  ["apalike", "author-year"],
  ["chicago", "author-year"],
  ["apa", "author-year"],
  ["agsm", "author-year"],
  ["authordate", "author-year"],
]);

/// The facts of a project, as the object the visual layer reads. `texts` maps
/// each project path to its source, `main` is the main file, `open` the file
/// shown in the editor (its environment offsets are the ones reported), and
/// `entries` the parsed bibliography entries.
export function documentFacts(texts, main, open, entries = []) {
  const plain = {};
  for (const path of Object.keys(texts)) plain[path] = stripComments(texts[path] ?? "");
  const theorems = theoremDefinitions(texts);
  const environmentNumbers = new Map();
  const order = new Map();
  const labels = numberLabels(texts, main, {
    theorems,
    open,
    environments: environmentNumbers,
    cited: (key) => {
      if (!order.has(key)) order.set(key, order.size + 1);
    },
  });
  const { textMacros, mathMacros } = macroFacts(plain);
  return {
    labels,
    citations: citationFacts(entries, order),
    acronyms: acronymFacts(plain),
    textMacros,
    mathMacros,
    theorems,
    environmentNumbers,
    citeStyle: citeStyleOf(plain),
  };
}

// Citations keep the bibliography's own normalization. A query of "" keeps every
// entry in order, so rankEntries serves here as the normalizer. Uncited entries
// have number 0.
function citationFacts(entries, order) {
  const citations = new Map();
  for (const entry of rankEntries(entries)) {
    if (!entry.key || citations.has(entry.key)) continue;
    citations.set(entry.key, {
      authors: surnamesOf(entry),
      year: entry.year,
      title: entry.title,
      number: order.get(entry.key) ?? 0,
    });
  }
  return citations;
}

// The same surname rule as the bibliography helper of that name, which is not
// exported from bibliography.js.
function surnamesOf(entry) {
  return entry.authors.map((author) => {
    const value = author.trim();
    return (value.includes(",") ? value.split(",", 1)[0] : value.split(/\s+/).at(-1) || value).replace(/^[{(]+|[})]+$/g, "");
  });
}

// The value of `name = value` in a key-value list. A braced value is read
// without its braces.
function field(list, name) {
  const match = new RegExp(`(?:^|[,\\s])${name}\\s*=\\s*(?:\\{((?:[^{}]|\\{[^{}]*\\})*)\\}|([^,{}]*))`).exec(list);
  return (match?.[1] ?? match?.[2] ?? "").trim();
}

function acronymFacts(texts) {
  const acronyms = new Map();
  for (const path of Object.keys(texts)) {
    const text = texts[path];
    for (const match of text.matchAll(NEWACRONYM)) {
      acronyms.set(match[1].trim(), { short: match[2].trim(), long: match[3].trim() });
    }
    for (const match of text.matchAll(ACRO)) {
      const key = match[1].trim();
      acronyms.set(key, { short: match[2]?.trim() || key, long: match[3].trim() });
    }
    for (const match of text.matchAll(DECLARE_ACRONYM)) {
      acronyms.set(match[1].trim(), { short: field(match[2], "short"), long: field(match[2], "long") });
    }
    for (const match of text.matchAll(GLOSSARY_ENTRY)) {
      acronyms.set(match[1].trim(), { short: field(match[2], "name"), long: field(match[2], "description") });
    }
  }
  return acronyms;
}

// The balanced {...} group that opens at `at`: its inner text, and the index
// after its close. Null when there is no group there.
function group(text, at) {
  if (text[at] !== "{") return null;
  let depth = 0;
  for (let i = at; i < text.length; i++) {
    if (text[i] === "\\") i++;
    else if (text[i] === "{") depth++;
    else if (text[i] === "}" && --depth === 0) return { value: text.slice(at + 1, i), next: i + 1 };
  }
  return null;
}

// A [...] argument that opens at `at`, without nesting.
function bracket(text, at) {
  if (text[at] !== "[") return null;
  const close = text.indexOf("]", at);
  return close === -1 ? null : { value: text.slice(at + 1, close), next: close + 1 };
}

function skip(text, at) {
  let i = at;
  while (i < text.length && /\s/.test(text[i])) i++;
  return i;
}

// The name a definition defines, braced or bare, as \name.
function macroName(text, at) {
  MACRO_NAME.lastIndex = at;
  const match = MACRO_NAME.exec(text);
  return match ? { name: match[1] ?? match[2], next: MACRO_NAME.lastIndex } : null;
}

// \newcommand, \renewcommand, \providecommand: the argument count, a
// bracketed default for the first argument, and the body. Null when the body
// is missing.
function newCommand(text, at) {
  let i = skip(text, at);
  let args = 0;
  let optional = null;
  const count = bracket(text, i);
  if (count) {
    args = Number(count.value);
    if (!Number.isInteger(args) || args < 0) return null;
    i = skip(text, count.next);
    const first = bracket(text, i);
    if (first) {
      optional = first.value;
      i = skip(text, first.next);
    }
  }
  const body = group(text, i);
  return body ? { args, optional, body: body.value } : null;
}

// \NewDocumentCommand with only m and an O{default} first: any other argument
// kind is left out.
function documentCommand(text, at) {
  const spec = group(text, skip(text, at));
  if (!spec) return null;
  const value = spec.value;
  let args = 0;
  let optional = null;
  let k = skip(value, 0);
  if (value[k] === "O") {
    const fallback = group(value, skip(value, k + 1));
    if (!fallback) return null;
    optional = fallback.value;
    args += 1;
    k = skip(value, fallback.next);
  }
  for (; k < value.length; k++) {
    if (value[k] === "m") args += 1;
    else if (!/\s/.test(value[k])) return null;
  }
  const body = group(text, skip(text, spec.next));
  return body ? { args, optional, body: body.value } : null;
}

// Text macros and the KaTeX macros. KaTeX takes a macro as its body string, so
// a macro with an optional argument, which KaTeX cannot express, is left out
// of mathMacros.
function macroFacts(texts) {
  const textMacros = new Map();
  const operators = {};
  for (const path of Object.keys(texts)) {
    const text = texts[path];
    for (const match of text.matchAll(DEFINE)) {
      const at = match.index + match[0].length;
      if (match[4] !== undefined) {
        const name = macroName(text, at);
        const body = name && group(text, skip(text, name.next));
        if (body) operators[`\\${name.name}`] = `\\operatorname${match[4]}{${body.value}}`;
        continue;
      }
      const name = macroName(text, at);
      if (!name) continue;
      // Text macros are keyed by bare name, as command tokens carry them.
      const key = name.name;
      let definition = null;
      if (match[3]) {
        const body = group(text, skip(text, name.next));
        if (body) definition = { args: 0, optional: null, body: body.value };
      } else if (match[2]) {
        definition = documentCommand(text, name.next);
      } else {
        definition = newCommand(text, name.next);
      }
      if (!definition) continue;
      // \providecommand leaves an existing definition alone.
      if ((match[1] === "provide" || match[2] === "Provide") && textMacros.has(key)) continue;
      textMacros.set(key, definition);
    }
  }
  const mathMacros = {};
  for (const [key, macro] of textMacros) {
    if (macro.optional === null) mathMacros[`\\${key}`] = macro.body;
  }
  return { textMacros, mathMacros: { ...mathMacros, ...operators } };
}

// natbib and biblatex options, and \bibliographystyle. The package options
// win over the style, since they set how \cite prints, and the default is
// author-year.
function citeStyleOf(texts) {
  let biblatex = null;
  let natbib = null;
  let bibliography = null;
  for (const path of Object.keys(texts)) {
    const text = texts[path];
    for (const match of text.matchAll(/\\usepackage\s*(?:\[([^\]]*)\])?\s*\{([^}]*)\}/g)) {
      const packages = match[2].split(",").map((name) => name.trim());
      const options = (match[1] ?? "").split(",").map((option) => option.trim());
      if (packages.includes("biblatex")) {
        const style = /\bstyle\s*=\s*([\w-]+)/.exec(match[1] ?? "")?.[1];
        if (style && CITE_STYLE_NUMERIC.test(style)) biblatex = "numeric";
        else if (style && CITE_STYLE_AUTHOR_YEAR.test(style)) biblatex = "author-year";
      }
      if (packages.includes("natbib")) natbib = options.includes("numbers") ? "numeric" : "author-year";
    }
    for (const match of text.matchAll(/\\bibliographystyle\s*\{([^}]*)\}/g)) {
      bibliography = BIBSTYLE.get(match[1].trim()) ?? bibliography;
    }
  }
  return biblatex ?? natbib ?? bibliography ?? "author-year";
}
