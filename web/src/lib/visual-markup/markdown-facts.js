// The facts of a Markdown or Quarto project that the visual layer draws from:
// reference numbers, citations and the citation style. One pass walks the main
// file in reading order and inlines each {{< include >}} where it stands, so
// the numbers match the order a reader meets the things in.

import { citationFacts } from "./facts.js";

const CROSS_REFERENCE = /^(?:fig|tbl|sec|eq|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|lst)-/i;
const THEOREMS = new Set(["thm", "lem", "cor", "prp", "cnj", "def", "exm", "exr", "sol", "rem"]);
const NUMERIC_CSL = /^(?:ieee|nature|vancouver|numeric)/i;

const FRONT_MATTER = /^---[ \t]*\n[\s\S]*?\n(?:---|\.\.\.)[ \t]*(?=\n|$)/;
const INLINE_CODE = /(`+)(?!`)[\s\S]*?[^`]\1(?!`)/g;
const DISPLAY_MATH = /\$\$[\s\S]*?\$\$(?:[ \t]*\{([^{}\n]*)\})?/g;
const INLINE_MATH = /(?<![\\$])\$(?![\s$])[^$\n]*?[^\s\\$]\$(?!\d)/g;
const HEADING = /^(#{1,6})[ \t]+(.+?)[ \t]*$/gm;
const IMAGE = /!\[[^\]\n]*\]\([^)\n]*\)(?:[ \t]*\{([^{}\n]*)\})?/g;
const DIV = /^:{3,}[ \t]*\{([^{}\n]*)\}/gm;
const CAPTION = /^:[ \t]+(.*)$/gm;
const CITATION = /(?<![\w\\])@([A-Za-z0-9_][A-Za-z0-9_:.#$%&+?<>~\/-]*)/g;
const INCLUDE = /\{\{<\s*include\s+"?([^"\s>]+)"?\s*>\}\}/g;

// Replaces a span by spaces, keeping its line breaks so that offsets still
// point into the source.
const blank = (text) => text.replace(/[^\n]/g, " ");

// The id of an attribute list such as "#fig-x width=50%", or null.
const idOf = (attributes) => /(?:^|\s)#([^\s"'}]+)/.exec(attributes ?? "")?.[1] ?? null;

// Joins and resolves . and .. so that paths compare as project paths.
function normalize(path) {
  const parts = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

// Blanks the fenced code, keeping the executable cells' `#| label:` options.
// Each fence line is blanked too.
function maskCode(source) {
  const labels = [];
  let fence = null;
  let at = 0;
  const lines = source.split("\n").map((line) => {
    const start = at;
    at += line.length + 1;
    if (fence) {
      const close = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(line);
      if (close && close[1][0] === fence.mark && close[1].length >= fence.length) fence = null;
      else if (fence.cell) {
        const option = /^#\|[ \t]*label:[ \t]*["']?([^\s"']+)/.exec(line);
        if (option) labels.push({ label: option[1], at: start });
      }
      return blank(line);
    }
    const open = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (open && !(open[1][0] === "`" && open[2].includes("`"))) {
      fence = { mark: open[1][0], length: open[1].length, cell: open[2].trimStart().startsWith("{") };
      return blank(line);
    }
    return line;
  });
  return { masked: lines.join("\n"), labels };
}

// The things the scan reacts to, in the order they occur in the masked text.
// Display math is read before it is blanked, so that its label counts; code
// and math are blanked, so that no @ inside them is a citation.
function eventsOf(source) {
  const { masked, labels } = maskCode(source);
  const noCode = masked.replace(INLINE_CODE, blank);
  const events = labels.map(({ label, at }) => ({ at, type: "label", id: label }));
  for (const match of noCode.matchAll(DISPLAY_MATH)) {
    events.push({ at: match.index, type: "label", id: idOf(match[1]) });
  }
  const text = noCode.replace(DISPLAY_MATH, blank).replace(INLINE_MATH, blank);
  for (const match of text.matchAll(HEADING)) {
    events.push({ at: match.index, type: "heading", level: match[1].length, attributes: /\{([^{}\n]*)\}[ \t]*$/.exec(match[2])?.[1] ?? "" });
  }
  for (const match of text.matchAll(IMAGE)) events.push({ at: match.index, type: "label", id: idOf(match[1]) });
  for (const match of text.matchAll(DIV)) events.push({ at: match.index, type: "label", id: idOf(match[1]) });
  for (const match of text.matchAll(CAPTION)) {
    events.push({ at: match.index, type: "label", id: idOf(/\{([^{}\n]*)\}[ \t]*$/.exec(match[1])?.[1]) });
  }
  for (const match of text.matchAll(CITATION)) {
    const key = match[1].replace(/[.:]+$/, "");
    if (!CROSS_REFERENCE.test(key)) events.push({ at: match.index, type: "cite", key });
  }
  for (const match of text.matchAll(INCLUDE)) events.push({ at: match.index, type: "include", name: match[1] });
  return events.sort((a, b) => a.at - b.at);
}

// Resolves an include against the folder of the including file. Returns a
// project path, or null when nothing exists there.
function resolveInclude(name, from, texts) {
  const target = name.trim();
  const folder = from.slice(0, from.lastIndexOf("/") + 1);
  const candidates = /\.\w+$/.test(target) ? [target] : [target, `${target}.qmd`, `${target}.md`];
  for (const candidate of candidates) {
    const path = normalize(folder + candidate);
    if (typeof texts[path] === "string") return path;
  }
  return null;
}

// Numbers the labels and records the citations of one file, then of the files
// it includes, in reading order. `stack` holds the files being read, which
// guards against an include cycle.
function scanFile(path, source, state, stack) {
  for (const event of eventsOf(source.replace(FRONT_MATTER, blank))) {
    if (event.type === "heading") {
      const { attributes, level } = event;
      const id = idOf(attributes);
      const unnumbered = /\.unnumbered\b/.test(attributes);
      if (!unnumbered) {
        state.sections[level - 1] += 1;
        state.sections.fill(0, level);
      }
      if (id?.startsWith("sec-")) {
        const number = state.numberSections && !unnumbered ? state.sections.slice(0, level).join(".") : "";
        state.labels.set(id, { number, kind: "section" });
      }
    } else if (event.type === "label") {
      const id = event.id;
      const prefix = /^([a-z]+)-/i.exec(id ?? "")?.[1].toLowerCase();
      if (prefix === "fig") state.labels.set(id, { number: String(++state.counts.figure), kind: "figure" });
      else if (prefix === "tbl") state.labels.set(id, { number: String(++state.counts.table), kind: "table" });
      else if (prefix === "eq") state.labels.set(id, { number: String(++state.counts.equation), kind: "equation" });
      else if (THEOREMS.has(prefix)) {
        const count = (state.counts.theorems.get(prefix) ?? 0) + 1;
        state.counts.theorems.set(prefix, count);
        state.labels.set(id, { number: String(count), kind: "theorem" });
      }
    } else if (event.type === "cite") {
      if (!state.order.has(event.key)) state.order.set(event.key, state.order.size + 1);
    } else if (event.type === "include") {
      const next = resolveInclude(event.name, path, state.texts);
      if (!next || stack.includes(next)) continue;
      stack.push(next);
      scanFile(next, state.texts[next], state, stack);
      stack.pop();
    }
  }
}

// The lines of the YAML front matter of a file, without the fences.
function frontMatter(text) {
  return /^---[ \t]*\n([\s\S]*?)\n(?:---|\.\.\.)[ \t]*(?:\n|$)/.exec(text)?.[1] ?? "";
}

// The value of a top-level YAML key, without quotes. Nested values are not read.
function yamlValue(yaml, key) {
  const match = new RegExp(`^${key}:[ \\t]*(.*)$`, "m").exec(yaml);
  return match ? match[1].trim().replace(/^["']|["']$/g, "") : "";
}

// Numeric when the csl names a numeric style, or when natbib or biblatex is
// set up to print numbers. Author-year otherwise.
function citeStyleOf(yaml) {
  const csl = yamlValue(yaml, "csl");
  if (csl && NUMERIC_CSL.test(csl.split("/").pop().replace(/\.csl$/, ""))) return "numeric";
  const method = yamlValue(yaml, "cite-method").toLowerCase();
  if ((method === "natbib" || method === "biblatex") && /\b(?:numbers|numeric)\b/.test(yaml)) return "numeric";
  return "author-year";
}

/// The facts of a Markdown project, in the shape documentFacts returns. `texts`
/// maps each project path to its source, `main` is the main file, and `entries`
/// the parsed bibliography entries. `open` is taken for parity with
/// documentFacts: Markdown numbers carry no offsets, so it is not read.
export function markdownFacts(texts, main, open, entries = []) {
  const root = normalize(main);
  const source = typeof texts[root] === "string" ? texts[root] : "";
  const yaml = frontMatter(source);
  const state = {
    texts,
    labels: new Map(),
    order: new Map(),
    numberSections: /^(?:true|yes)$/i.test(yamlValue(yaml, "number-sections")),
    sections: [0, 0, 0, 0, 0, 0],
    counts: { figure: 0, table: 0, equation: 0, theorems: new Map() },
  };
  if (source) scanFile(root, source, state, [root]);
  return {
    labels: state.labels,
    citations: citationFacts(entries, state.order),
    acronyms: new Map(),
    textMacros: new Map(),
    mathMacros: {},
    theorems: new Map(),
    environmentNumbers: new Map(),
    citeStyle: citeStyleOf(yaml),
  };
}
