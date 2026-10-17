// LaTeX completion. The trigger is read from the line before the cursor, and
// the project is read only after a trigger matches, so ordinary typing costs
// nothing here. Commands and environments come from LaTeX-Workshop's lists
// (see latex-workshop.json) and from latex-packages.js; everything the
// project defines or loads is read from its own sources.
import { snippet, snippetCompletion } from "@codemirror/autocomplete";
import workshop from "./latex-workshop.json" with { type: "json" };
import { CORE, PACKAGES } from "./latex-packages.js";
import { pathOptions } from "./paths.js";

const REFERENCE = /\\(?:ref|eqref|pageref|autoref|cref|Cref|nameref)\*?\{([^{}]*)$/;
const INCLUDE = /\\(input|include|subfile)\{([^{}]*)$/;
const GRAPHIC = /\\includegraphics(?:\[[^\]]*\])?\{([^{}]*)$/;
const BIBLIOGRAPHY = /\\(bibliography|addbibresource)\{([^{}]*)$/;
const PACKAGE = /\\usepackage(?:\[[^\]]*\])?\{([^{}]*)$/;
const ENVIRONMENT = /\\begin\{([A-Za-z*]*)$/;
const COMMAND = /(?<!\\)\\([A-Za-z]*)$/;

const DEFINED_COMMAND = /\\(?:re)?newcommand\*?\s*(?:\{\\([A-Za-z]+)\}|\\([A-Za-z]+))\s*(?:\[(\d)\])?/g;
const DEFINED_OPERATOR = /\\DeclareMathOperator\*?\s*\{\\([A-Za-z]+)\}/g;
const DEFINED_DEF = /\\def\\([A-Za-z]+)/g;
const DEFINED_ENVIRONMENT = /\\(?:re)?newenvironment\*?\{([A-Za-z]+\*?)\}/g;
const USED_PACKAGE = /\\usepackage\s*(?:\[[^\]]*\])?\s*\{([^{}]*)\}/g;
const LABEL = /\\label\{([^{}]+)\}/g;

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

/**
 * Labels in every .tex and .ltx text, comments ignored. The kind is the part
 * of the label before a colon (fig, tab, eq, sec), or "label" without one.
 * @param {Record<string, string>} texts
 * @returns {Array<{ id: string, kind: string, path: string, line: number }>}
 */
export function latexLabels(texts) {
  const labels = [];
  for (const path of Object.keys(texts).sort()) {
    if (!/\.(tex|ltx)$/.test(path)) continue;
    String(texts[path]).split("\n").forEach((raw, index) => {
      for (const match of stripComment(raw).matchAll(LABEL)) {
        const id = match[1];
        const colon = id.indexOf(":");
        labels.push({ id, kind: colon > 0 ? id.slice(0, colon) : "label", path, line: index + 1 });
      }
    });
  }
  return labels;
}

// Names the project defines or loads, read from every text.
function definitions(texts) {
  const commands = [], environments = [], packages = [];
  for (const text of Object.values(texts)) {
    const code = codeOf(text);
    for (const match of code.matchAll(DEFINED_COMMAND)) {
      commands.push({ name: match[1] || match[2], arity: Number(match[3] || 0) });
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

function commandEntry(name, snippetText, detail) {
  const completion = { label: name, type: "function", detail };
  return { name, option: snippetText === undefined ? completion : snippetCompletion(snippetText, completion) };
}

function environmentEntry(name, body, detail) {
  return { name, body, detail };
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
const PACKAGE_OPTIONS = Object.keys(PACKAGES).map((label) => ({ label, type: "namespace", detail: "package" }));

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
  const seen = new Set();
  const options = [];
  const labels = [
    ...latexLabels({ [p.path]: context.state.doc.toString() }),
    ...latexLabels(p.texts),
  ];
  for (const label of labels) {
    if (seen.has(label.id)) continue;
    seen.add(label.id);
    options.push({ label: label.id, type: "variable", detail: `${label.path}:${label.line}` });
  }
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
        ...found.environments.map((name) => environmentEntry(name, undefined, "defined in project")),
        ...CORE_ENVIRONMENTS,
        ...WORKSHOP_ENVIRONMENTS,
        ...packages.flatMap((name) => PACKAGE_ENVIRONMENTS[name]),
      ]);
      const next = line.number < context.state.doc.lines ? context.state.doc.line(line.number + 1).text : "";
      return {
        from: context.pos - match[1].length,
        options: entries.map((entry) => environmentOption(entry, next.includes(`\\end{${entry.name}}`))),
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
          "defined in project",
        )),
        ...CORE_COMMANDS,
        ...WORKSHOP_COMMANDS,
        ...packages.flatMap((name) => PACKAGE_COMMANDS[name]),
      ]);
      return {
        from: context.pos - match[1].length,
        options: entries.map((entry) => entry.option),
        validFor: /^[A-Za-z]*$/,
      };
    }
    return null;
  };
}
