// The facts of a Markdown or Quarto project that the visual layer draws from:
// reference numbers, citations and the citation style. One pass walks the main
// file in reading order and inlines each {{< include >}} where it stands, so
// the numbers match the order a reader meets the things in.

import { citationFacts } from "./facts.js";
import { markdownTree } from "../source-trees.js";

const CROSS_REFERENCE = /^(?:fig|tbl|sec|eq|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|lst)-/i;
const THEOREMS = new Set(["thm", "lem", "cor", "prp", "cnj", "def", "exm", "exr", "sol", "rem"]);
const NUMERIC_CSL = /^(?:ieee|nature|vancouver|numeric)/i;

// The `#| label:` option line of an executable cell, which the grammar leaves as code.
const CELL_LABEL = /^#\|[ \t]*label:[ \t]*["']?([^\s"']+)/;
// Quarto's include shortcode, which the grammar leaves as text.
const INCLUDE = /\{\{<\s*include\s+"?([^"\s>]+)"?\s*>\}\}/g;
// Nodes whose text is never prose: no include shortcode in them is an include.
const CODE = new Set(["FencedCode", "CodeBlock", "InlineCode", "InlineMath", "DisplayMath", "Comment", "CommentBlock"]);
const HEADING_NODE = /^(ATX|Setext)Heading([1-6])$/;

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

// `text` with the sorted, non-overlapping ranges blanked, keeping line breaks
// so that offsets still point into the source.
function blankRanges(text, ranges) {
  let out = "";
  let at = 0;
  for (const [from, to] of ranges) {
    if (from < at) continue;
    out += text.slice(at, from) + text.slice(from, to).replace(/[^\n]/g, " ");
    at = to;
  }
  return out + text.slice(at);
}

// The id of an Attributes node, or null.
const idOfAttributes = (body, attributes) => {
  const id = attributes.getChild("AttributeId");
  return id ? body.slice(id.from + 1, id.to) : null;
};

// Whether an Attributes node labels an element that Quarto numbers: a fenced
// div, display math, an image, or a table caption (a line that starts with ": ").
function labelsElement(body, attributes) {
  const parent = attributes.parent;
  if (parent?.name === "FencedDiv" || parent?.name === "DisplayMath") return true;
  const before = attributes.prevSibling?.name;
  if (before === "Image" || before === "DisplayMath") return true;
  if (parent?.name !== "Paragraph") return false;
  const line = body.lastIndexOf("\n", attributes.from - 1) + 1;
  return body[line] === ":" && (body[line + 1] === " " || body[line + 1] === "\t");
}

// The things the scan reacts to, in the order they occur in the tree: headings,
// labels of numbered elements, citations and includes. Code, math and comments
// are nodes of the tree, so no @ inside them is a citation.
function eventsOf(source, path) {
  const { tree, body } = markdownTree(source, path);
  /** @type {Array<{ at: number, type: string, id?: string | null, level?: number, unnumbered?: boolean, key?: string, name?: string }>} */
  const events = [];
  /** @type {Array<[number, number]>} */
  const code = [];
  tree.iterate({
    enter(ref) {
      const node = ref.node;
      const name = node.name;
      if (CODE.has(name)) code.push([node.from, node.to]);
      if (name === "FencedCode") {
        // Only executable cells, whose info string starts with a brace, have options.
        const info = node.getChild("CodeInfo");
        if (info && body.slice(info.from, info.to).trimStart().startsWith("{")) {
          for (let at = node.from; at < node.to; ) {
            const eol = body.indexOf("\n", at);
            const end = eol < 0 || eol > node.to ? node.to : eol;
            const option = CELL_LABEL.exec(body.slice(at, end));
            if (option) events.push({ at, type: "label", id: option[1] });
            at = end + 1;
          }
        }
        return false;
      }
      if (name === "CodeBlock" || name === "InlineCode" || name === "InlineMath" || name === "Comment" || name === "CommentBlock") return false;
      const heading = HEADING_NODE.exec(name);
      if (heading) {
        let last = node.lastChild;
        while (last && last.name === "HeaderMark") last = last.prevSibling;
        const attributes = last?.name === "Attributes" ? last : null;
        let at = node.from;
        if (heading[1] === "Setext") {
          // The text line over the underline, from its first character.
          const mark = node.lastChild?.name === "HeaderMark" ? node.lastChild.from : node.to;
          const text = body.slice(node.from, mark).replace(/\s+$/u, "");
          const line = text.slice(text.lastIndexOf("\n") + 1);
          at = node.from + text.length - line.length + (line.length - line.trimStart().length);
        }
        events.push({
          at,
          type: "heading",
          level: Number(heading[2]),
          id: attributes ? idOfAttributes(body, attributes) : null,
          unnumbered: attributes?.getChildren("AttributeClass").some((cls) => body.slice(cls.from, cls.to) === ".unnumbered") ?? false,
        });
        // A citation in the title still counts.
        return;
      }
      if (name === "Attributes") {
        if (labelsElement(body, node)) events.push({ at: node.from, type: "label", id: idOfAttributes(body, node) });
        return false;
      }
      if (name === "CitationKey") {
        const key = body.slice(node.from + 1, node.to);
        if (!CROSS_REFERENCE.test(key)) events.push({ at: node.from, type: "cite", key });
        return false;
      }
    },
  });
  // Includes are shortcodes in plain text, so they are read from the text that is not code.
  for (const match of blankRanges(body, code).matchAll(INCLUDE)) {
    events.push({ at: match.index, type: "include", name: match[1] });
  }
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
  for (const event of eventsOf(source, path)) {
    if (event.type === "heading") {
      const { id, level, unnumbered } = event;
      if (!unnumbered) {
        state.sections[level - 1] += 1;
        state.sections.fill(0, level);
      }
      const number = state.numberSections && !unnumbered ? state.sections.slice(0, level).join(".") : "";
      if (number && path === state.focus) state.headingNumbers.set(event.at, number);
      if (id?.startsWith("sec-")) state.labels.set(id, { number, kind: "section" });
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
  const range = markdownTree(text).frontMatter;
  return range ? text.slice(0, range[1]).split(/\r?\n/).slice(1, -1).join("\n") : "";
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
/// the parsed bibliography entries. `open` is the file shown in the editor: its
/// numbered headings are listed in `headingNumbers`, keyed by the offset of the
/// heading's first character (the # of an ATX heading, the text of a setext one).
export function markdownFacts(texts, main, open, entries = []) {
  const root = normalize(main);
  const source = typeof texts[root] === "string" ? texts[root] : "";
  const yaml = frontMatter(source);
  const state = {
    texts,
    focus: open == null ? null : normalize(open),
    headingNumbers: new Map(),
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
    headingNumbers: state.headingNumbers,
    citeStyle: citeStyleOf(yaml),
  };
}
