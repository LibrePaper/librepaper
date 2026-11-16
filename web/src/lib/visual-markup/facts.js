// The facts of a LaTeX project that the visual layer draws from: reference
// numbers, citations, acronyms, macros and the citation style. The editor
// recomputes them on each edit pause, so each part is a pass over the text, and
// the numbering pass is shared by the labels, the theorems and the citations.

import { rankEntries } from "../bibliography.js";
import { bracketed, braced, groupAfter, latexCommands, keyValues, readArguments, skipSpace } from "../latex-language/args.js";
import { numberLabels, theoremDefinitions } from "./labels.js";

// Definition heads: newcommand, renewcommand, providecommand (starred forms
// too), the xparse forms, def and DeclareMathOperator.
const COMMAND_DEFINITIONS = new Map([
  ["newcommand", "new"],
  ["renewcommand", "new"],
  ["providecommand", "provide"],
  ["NewDocumentCommand", "document"],
  ["RenewDocumentCommand", "document"],
  ["ProvideDocumentCommand", "provide-document"],
]);

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
  const theorems = theoremDefinitions(texts);
  const environmentNumbers = new Map();
  const headingNumbers = new Map();
  const captionNumbers = new Map();
  const order = new Map();
  const labels = numberLabels(texts, main, {
    theorems,
    open,
    environments: environmentNumbers,
    headings: headingNumbers,
    captions: captionNumbers,
    cited: (key) => {
      if (!order.has(key)) order.set(key, order.size + 1);
    },
  });
  const { textMacros, mathMacros } = macroFacts(texts);
  return {
    labels,
    citations: citationFacts(entries, order),
    acronyms: acronymFacts(texts),
    textMacros,
    mathMacros,
    theorems,
    environmentNumbers,
    headingNumbers,
    captionNumbers,
    citeStyle: citeStyleOf(texts),
  };
}

// Citations keep the bibliography's own normalization. A query of "" keeps every
// entry in order, so rankEntries serves here as the normalizer. Uncited entries
// have number 0.
export function citationFacts(entries, order) {
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

// The commands of each project text, with comments blanked.
function lexed(texts) {
  return Object.keys(texts).map((path) => latexCommands(texts[path] ?? "", path));
}

const group = braced;
const bracket = bracketed;
const skip = skipSpace;

function acronymFacts(texts) {
  const acronyms = new Map();
  for (const { code, commands } of lexed(texts)) {
    const calls = (name) => commands.filter((command) => command.type === "command" && command.name === name);
    for (const command of calls("newacronym")) {
      // \newacronym[options]{key}{short}{long}
      const args = readArguments(code, command.to).filter((arg) => arg.kind === "group");
      if (args.length >= 3) {
        acronyms.set(args[0].value.trim(), { short: args[1].value.trim(), long: args[2].value.trim() });
      }
    }
    for (const command of calls("acro")) {
      // \acro{key}[short]{long}
      const args = readArguments(code, command.to);
      if (args[0]?.kind !== "group") continue;
      const key = args[0].value.trim();
      const short = args[1]?.kind === "optional" ? args[1] : null;
      const long = args[short ? 2 : 1];
      if (long?.kind !== "group") continue;
      acronyms.set(key, { short: short?.value.trim() || key, long: long.value.trim() });
    }
    for (const command of calls("DeclareAcronym")) {
      const args = readArguments(code, command.to);
      if (args[0]?.kind !== "group" || args[1]?.kind !== "group") continue;
      const fields = keyValues(args[1].value);
      acronyms.set(args[0].value.trim(), { short: fields.get("short") ?? "", long: fields.get("long") ?? "" });
    }
    for (const command of calls("newglossaryentry")) {
      const args = readArguments(code, command.to);
      if (args[0]?.kind !== "group" || args[1]?.kind !== "group") continue;
      const fields = keyValues(args[1].value);
      acronyms.set(args[0].value.trim(), { short: fields.get("name") ?? "", long: fields.get("description") ?? "" });
    }
  }
  return acronyms;
}

// The name a definition defines, braced or bare, as \name.
function macroName(text, at) {
  let i = skip(text, at);
  if (text[i] === "{") {
    const inner = group(text, i);
    const match = inner && /^\s*\\([A-Za-z@]+)\s*$/.exec(inner.value);
    return match ? { name: match[1], next: inner.next } : null;
  }
  const match = /\\([A-Za-z@]+)/y;
  match.lastIndex = i;
  const found = match.exec(text);
  return found ? { name: found[1], next: match.lastIndex } : null;
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
  for (const { code, commands } of lexed(texts)) {
    for (const command of commands) {
      if (command.type !== "command") continue;
      const kind = command.name === "def" ? "def" : command.name === "DeclareMathOperator" ? "operator" : COMMAND_DEFINITIONS.get(command.name);
      if (!kind) continue;
      // \def defines a bare control word, so the backslash must follow it.
      if (kind === "def" && code[command.to] !== "\\") continue;
      // The star belongs to \newcommand* and \DeclareMathOperator*.
      const starred = command.star && (kind === "new" || kind === "provide" || kind === "operator");
      const at = starred ? command.to + 1 : command.to;
      const name = macroName(code, at);
      if (!name) continue;
      if (kind === "operator") {
        const body = group(code, skip(code, name.next));
        if (body) operators[`\\${name.name}`] = `\\operatorname${command.star ? "*" : ""}{${body.value}}`;
        continue;
      }
      // Text macros are keyed by bare name, as command tokens carry them.
      const key = name.name;
      let definition = null;
      if (kind === "def") {
        const body = group(code, skip(code, name.next));
        if (body) definition = { args: 0, optional: null, body: body.value };
      } else if (kind === "document" || kind === "provide-document") {
        definition = documentCommand(code, name.next);
      } else {
        definition = newCommand(code, name.next);
      }
      if (!definition) continue;
      // \providecommand leaves an existing definition alone.
      if ((kind === "provide" || kind === "provide-document") && textMacros.has(key)) continue;
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
  for (const { code, commands } of lexed(texts)) {
    for (const command of commands) {
      if (command.type !== "command") continue;
      if (command.name === "usepackage") {
        const args = readArguments(code, command.to);
        const optional = args[0]?.kind === "optional" ? args[0] : null;
        const names = args[optional ? 1 : 0];
        if (names?.kind !== "group") continue;
        const packages = names.value.split(",").map((name) => name.trim());
        const options = (optional?.value ?? "").split(",").map((option) => option.trim());
        if (packages.includes("biblatex")) {
          const style = keyValues(optional?.value ?? "").get("style");
          if (style && CITE_STYLE_NUMERIC.test(style)) biblatex = "numeric";
          else if (style && CITE_STYLE_AUTHOR_YEAR.test(style)) biblatex = "author-year";
        }
        if (packages.includes("natbib")) natbib = options.includes("numbers") ? "numeric" : "author-year";
      } else if (command.name === "bibliographystyle") {
        const style = groupAfter(code, command.to);
        if (style) bibliography = BIBSTYLE.get(style.value.trim()) ?? bibliography;
      }
    }
  }
  return biblatex ?? natbib ?? bibliography ?? "author-year";
}
