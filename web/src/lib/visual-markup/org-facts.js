// The facts of an Org project that the visual layer draws from: heading
// numbers, link targets, citations and the citation style. One pass walks the
// main file's tree in reading order and inlines each #+INCLUDE where it
// stands. The tree is read for structure; only one node's own text is
// interpreted (a keyword value, a property value, a link target).
//
// Label keys are written the way an Org internal link writes them:
//   #<id>        a headline with a :CUSTOM_ID: property       (kind "section")
//   *<Title>     any headline, by its title                   (kind "section")
//   <name>       #+NAME: before a table, an image link, a src block or a
//                numbered LaTeX environment      (table, figure, listing, equation)
//   <target>     a <<target>>                                 (kind "target")
// A heading's number is keyed by the offset of its Headline node (the first
// star), for the open file only.

import { citationFacts } from "./facts.js";
import { normalize } from "./markdown-facts.js";
import { orgTree } from "../source-trees.js";

const NUMERIC_CSL = /^(?:ieee|nature|vancouver|numeric)/i;
const NUMERIC_STYLE = /\b(?:numeric|numbers|ieee|unsrt|plain|abbrv|alpha)\b/i;
const IMAGE = /\.(?:png|jpe?g|gif|svg|webp|bmp|pdf|eps|tiff?)$/i;
const NUMBERED_ENVIRONMENTS = new Set(["equation", "align", "alignat", "gather", "multline", "flalign", "eqnarray"]);

const keyOf = (text, node) => text.slice(node.from, node.to);

// The name of a Keyword node, upper case, without its [option].
function keywordName(text, node) {
  const key = node.getChild("KeywordKey");
  return key ? keyOf(text, key).replace(/\[.*$/s, "").trim().toUpperCase() : "";
}

function keywordValue(text, node) {
  const value = node.getChild("KeywordValue");
  return value ? keyOf(text, value).trim() : "";
}

// The depth limit from `#+OPTIONS: num:...`: 0 for nil, Infinity for t, N for N.
function numLimit(text, tree) {
  let limit = Infinity;
  for (const node of tree.topNode.getChildren("Keyword")) {
    if (keywordName(text, node) !== "OPTIONS") continue;
    const match = /(?:^|\s)num:(\S+)/.exec(keywordValue(text, node));
    if (!match) continue;
    const value = match[1].toLowerCase();
    if (value === "nil") limit = 0;
    else if (value === "t") limit = Infinity;
    else if (/^\d+$/.test(value)) limit = Number(value);
  }
  return limit;
}

// The first quoted string or word of an #+INCLUDE value, unless the include is
// a verbatim one (src, example, export), which is not read as Org.
function includeTarget(value) {
  const quoted = /^"([^"]+)"(.*)$/.exec(value);
  const name = quoted ? quoted[1] : value.split(/\s+/)[0];
  const rest = quoted ? quoted[2] : value.slice(name.length);
  if (!name || /(?:^|\s)(?:src|example|export)(?:\s|$)/i.test(rest)) return null;
  return name;
}

function resolveInclude(name, from, texts) {
  const folder = from.slice(0, from.lastIndexOf("/") + 1);
  const path = normalize(folder + name);
  return typeof texts[path] === "string" ? path : null;
}

function blockName(text, node) {
  const name = node.getChild("BlockBegin")?.getChild("BlockName");
  return name ? keyOf(text, name).toLowerCase() : "";
}

// Whether a paragraph is only one link to an image.
function isFigure(text, node) {
  const link = node.getChild("Link");
  if (!link) return false;
  if (keyOf(text, node).trim() !== keyOf(text, link).trim()) return false;
  const target = link.getChild("LinkTarget");
  return Boolean(target && IMAGE.test(keyOf(text, target).trim().replace(/^file:/, "")));
}

function scanFile(path, source, state, stack) {
  const tree = orgTree(source, path);
  if (!tree) return;
  const limit = path === state.root ? (state.limit = numLimit(source, tree)) : state.limit;
  /** @type {{ name?: string, caption?: string, has: boolean } | null} */
  let pending = null;
  /** @type {Array<{ to: number, unnumbered: boolean }>} */
  const sectionStack = [];
  let heading = ""; // the number of the headline being read
  let element = null; // { to, number } of the numbered element being read

  const take = () => {
    const taken = pending;
    pending = null;
    return taken;
  };
  const count = (kind) => String(++state.counts[kind]);
  const register = (name, label) => {
    if (name) state.labels.set(name, label);
  };

  tree.iterate({
    enter(ref) {
      const node = ref.node;
      const name = ref.name;
      if (name === "Section") {
        const headline = node.getChild("Headline");
        if (!headline) return;
        const tags = headline.getChild("Tags")?.getChildren("Tag").map((tag) => keyOf(source, tag).toLowerCase()) ?? [];
        if (tags.includes("noexport")) return false;
        while (sectionStack.length && sectionStack.at(-1).to <= node.from) sectionStack.pop();
        const properties = new Map();
        for (const property of node.getChild("PropertyDrawer")?.getChildren("Property") ?? []) {
          const key = property.getChild("PropertyKey");
          const value = property.getChild("PropertyValue");
          if (key) properties.set(keyOf(source, key).toUpperCase(), value ? keyOf(source, value).trim() : "");
        }
        const own = properties.has("UNNUMBERED") && properties.get("UNNUMBERED").toLowerCase() !== "nil";
        const unnumbered = own || (sectionStack.at(-1)?.unnumbered ?? false);
        sectionStack.push({ to: node.to, unnumbered });
        const level = headline.getChild("Stars").to - headline.getChild("Stars").from;
        let number = "";
        if (!unnumbered && level <= limit) {
          state.sections[level - 1] += 1;
          state.sections.fill(0, level);
          number = state.sections.slice(0, level).join(".");
        }
        heading = number;
        pending = null;
        if (number && path === state.focus) state.headingNumbers.set(headline.from, number);
        const label = { kind: "section", ...(number ? { number } : {}) };
        const title = headline.getChild("Title");
        const titleText = title ? keyOf(source, title).trim() : "";
        if (titleText) {
          state.labels.set(`*${titleText}`, { ...label });
          const id = properties.get("CUSTOM_ID");
          if (id) state.labels.set(`#${id}`, { ...label, title: titleText });
        } else if (properties.get("CUSTOM_ID")) {
          state.labels.set(`#${properties.get("CUSTOM_ID")}`, label);
        }
        return;
      }
      if (name === "Keyword") {
        const key = keywordName(source, node);
        const value = keywordValue(source, node);
        if (key === "NAME") pending = { ...(pending ?? { has: false }), name: value };
        else if (key === "CAPTION") pending = { ...(pending ?? {}), caption: value, has: true };
        else if (key === "CITE_EXPORT") {
          if (!state.citeExport) state.citeExport = value;
        } else if (key === "INCLUDE") {
          const target = includeTarget(value);
          const next = target && resolveInclude(target, path, state.texts);
          if (next && !stack.includes(next)) {
            stack.push(next);
            scanFile(next, state.texts[next], state, stack);
            stack.pop();
          }
        }
        // A caption can hold a citation, so the value is still read.
        return key === "CAPTION" || key === "TITLE" ? undefined : false;
      }
      if (name === "Table") {
        const taken = take();
        const number = taken?.caption !== undefined ? count("table") : "";
        register(taken?.name, { kind: "table", ...(number ? { number } : {}), ...(taken?.caption ? { title: taken.caption } : {}) });
        if (number) element = { to: node.to, number };
        return;
      }
      if (name === "Paragraph") {
        const taken = take();
        if (taken && isFigure(source, node)) {
          const number = taken.caption !== undefined ? count("figure") : "";
          register(taken.name, { kind: "figure", ...(number ? { number } : {}), ...(taken.caption ? { title: taken.caption } : {}) });
          if (number) element = { to: node.to, number };
        }
        return;
      }
      if (name === "Block") {
        const taken = take();
        if (blockName(source, node) === "src") {
          const number = taken?.caption !== undefined ? count("listing") : "";
          register(taken?.name, { kind: "listing", ...(number ? { number } : {}), ...(taken?.caption ? { title: taken.caption } : {}) });
        }
        return;
      }
      if (name === "LatexEnvironment") {
        const taken = take();
        const env = node.getChild("LatexEnvBegin")?.getChild("LatexEnvName");
        if (env && NUMBERED_ENVIRONMENTS.has(keyOf(source, env).trim())) {
          const number = count("equation");
          register(taken?.name, { kind: "equation", number });
        }
        return false;
      }
      if (name === "Target") {
        const target = keyOf(source, node).replace(/^<<|>>$/g, "").trim();
        const number = element && node.from < element.to ? element.number : heading;
        if (target) state.labels.set(target, { kind: "target", ...(number ? { number } : {}) });
        return;
      }
      if (name === "CitationKey") {
        const key = keyOf(source, node);
        if (key && !state.order.has(key)) state.order.set(key, state.order.size + 1);
        return false;
      }
    },
  });
}

// Numeric when `#+CITE_EXPORT:` names a numeric style. Author-year otherwise.
function citeStyleOf(value) {
  const words = (value ?? "").trim().split(/\s+/).filter(Boolean);
  if (!words.length) return "author-year";
  if (words[0].toLowerCase() === "csl") {
    const csl = (words[1] ?? "").split("/").pop().replace(/\.csl$/, "");
    return NUMERIC_CSL.test(csl) ? "numeric" : "author-year";
  }
  return NUMERIC_STYLE.test(words.slice(1).join(" ")) ? "numeric" : "author-year";
}

/// The facts of an Org project, in the shape documentFacts returns. `texts`
/// maps each project path to its source, `main` is the main file, `open` the
/// file shown in the editor (its numbered headlines are in `headingNumbers`,
/// keyed by the offset of the Headline node, i.e. of its first star), and
/// `entries` the parsed bibliography entries.
export function orgFacts(texts, main, open, entries = []) {
  const root = normalize(main);
  const source = typeof texts[root] === "string" ? texts[root] : "";
  const state = {
    texts,
    root,
    focus: open == null ? null : normalize(open),
    limit: Infinity,
    headingNumbers: new Map(),
    labels: new Map(),
    order: new Map(),
    citeExport: "",
    sections: [0, 0, 0, 0, 0, 0, 0, 0, 0],
    counts: { table: 0, figure: 0, listing: 0, equation: 0 },
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
    citeStyle: citeStyleOf(state.citeExport),
  };
}
