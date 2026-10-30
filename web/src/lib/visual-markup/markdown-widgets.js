// The widgets of the Markdown and Quarto language. They read the facts and the
// tokens that the language produces; the structural pieces (fences, markers,
// attribute blocks) are hidden by the language's styles, and these draw what is
// left in their place.
//
// This module is imported by index.js, and it imports index.js back for the
// shared helpers. Nothing here reads those imports at load time, only when a
// widget is built, so the cycle is safe.

import { WidgetType } from "@codemirror/view";
import "./markdown.css";
import { imageResolver, isExternal, registerWidget, resolveImagePath, revealOnMousedown, textOf } from "./markup.ts";
import { ElementWidget, factsOf, includeOpener, joinParts, nameList } from "./index.js";

const own = (object, key) => Object.hasOwn(object, key);

// A DOM element with a class and text, in one call.
function el(tag, className = "", text = "") {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}

// The text of a token's body, or "" when it has none. A body is a range in the
// source, or a string when the language has already read it.
function bodyText(token, source) {
  const body = token.body;
  if (body === undefined || body === null) return "";
  return typeof body === "string" ? body : textOf(source, body);
}

// The value of one attribute in a Pandoc attribute list, such as title="T" or width=50%.
function attribute(attrs, name) {
  const match = new RegExp(`(?:^|[\\s{])${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s}]+))`).exec(attrs);
  return match ? (match[1] ?? match[2] ?? match[3] ?? "") : "";
}

// Only plain CSS lengths are used for a figure width; anything else is dropped.
function sizeOf(value) {
  return /^\d+(\.\d+)?(%|px|em|rem)?$/.test(value) ? value : "";
}

// Markdown inline markup, reduced to the text it shows. Table cells, captions
// and notes are drawn as plain text.
function plainMarkup(text) {
  return text
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/__(.+?)__/g, "$1")
    .replace(/~~(.+?)~~/g, "$1")
    .replace(/==(.+?)==/g, "$1")
    .replace(/\*(\S(?:.*?\S)?)\*/g, "$1")
    .replace(/(^|\W)_(\S(?:.*?\S)?)_(?!\w)/g, "$1$2")
    .replace(/\\([\\*_`|[\]])/g, "$1")
    .trim();
}

// A widget whose DOM is built by DRAW[name]. Two widgets are equal when they
// draw the same data with the same callbacks and resolver.
class MarkdownWidget extends WidgetType {
  constructor(name, data, { opener = null, resolver = null } = {}) {
    super();
    this.name = name;
    this.data = data;
    this.opener = opener;
    this.resolver = resolver;
    this.signature = JSON.stringify([name, data]);
  }

  eq(other) {
    return (
      other instanceof MarkdownWidget &&
      other.signature === this.signature &&
      other.opener === this.opener &&
      other.resolver === this.resolver
    );
  }

  toDOM(view) {
    const element = DRAW[this.name](this.data, this);
    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

// ---- Front matter ----------------------------------------------------------

// The YAML that front matter uses: mappings, block and flow sequences, and
// scalars. Anything else is read as a plain string, and a parse error gives no
// keys rather than an error.
const YAML_KEY = /^("[^"]*"|'[^']*'|[^\s:][^:]*?):(?:\s+(.*))?$/;

function isItem(text) {
  return text === "-" || text.startsWith("- ");
}

// Each non-blank, non-comment line with its indentation.
function yamlLines(text) {
  return text.split(/\r?\n/).flatMap((line) => {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) return [];
    return [{ indent: line.length - line.trimStart().length, text: trimmed }];
  });
}

// Splits a flow sequence at its commas, leaving quoted commas alone.
function splitFlow(text) {
  const parts = [];
  let current = "";
  let quote = "";
  for (const character of text) {
    if (quote) {
      if (character === quote) quote = "";
      current += character;
    } else if (character === '"' || character === "'") {
      quote = character;
      current += character;
    } else if (character === ",") {
      parts.push(current);
      current = "";
    } else {
      current += character;
    }
  }
  parts.push(current);
  return parts.map((part) => part.trim()).filter((part) => part !== "");
}

function yamlScalar(text) {
  const value = text.trim();
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
    return value.slice(1, -1).replace(/\\(["\\])/g, "$1");
  }
  if (value.length >= 2 && value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1).replace(/''/g, "'");
  }
  if (value.startsWith("[") && value.endsWith("]")) return splitFlow(value.slice(1, -1)).map(yamlScalar);
  if (value === "~" || value === "null") return null;
  return value;
}

// The value under a key: the rest of its line, or the indented lines below it.
function yamlValue(rest, children) {
  // Block scalars keep their lines; | keeps the breaks and > joins them.
  if (/^[|>][-+]?$/.test(rest)) return children.map((child) => child.text).join(rest.startsWith("|") ? "\n" : " ");
  if (rest) return yamlScalar(rest);
  return yamlNode(children);
}

function yamlMapping(entries) {
  const map = {};
  const base = entries[0].indent;
  let i = 0;
  while (i < entries.length) {
    const entry = entries[i++];
    const match = YAML_KEY.exec(entry.text);
    if (!match || entry.indent !== base) continue;
    const children = [];
    while (
      i < entries.length &&
      (entries[i].indent > base || (entries[i].indent === base && isItem(entries[i].text)))
    ) {
      children.push(entries[i++]);
    }
    map[String(yamlScalar(match[1]))] = yamlValue(match[2] ?? "", children);
  }
  return map;
}

function yamlSequence(entries) {
  const list = [];
  const base = entries[0].indent;
  let i = 0;
  while (i < entries.length) {
    const entry = entries[i++];
    if (!isItem(entry.text)) continue;
    const children = [];
    while (i < entries.length && entries[i].indent > base) children.push(entries[i++]);
    const rest = entry.text.slice(1).trim();
    if (!rest) {
      list.push(yamlNode(children));
    } else if (YAML_KEY.test(rest) && !/^["']/.test(rest)) {
      // "- name: x" opens a mapping whose other keys sit under "name".
      list.push(yamlMapping([{ indent: base + 2, text: rest }, ...children]));
    } else {
      list.push(yamlScalar(rest));
    }
  }
  return list;
}

function yamlNode(entries) {
  if (!entries.length) return null;
  return isItem(entries[0].text) ? yamlSequence(entries) : yamlMapping(entries);
}

function plainValue(value) {
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean" ? String(value) : "";
}

// Authors are a string, a list of strings, or a list of maps with a name.
function authorNames(value) {
  const list = Array.isArray(value) ? value : value === undefined || value === null ? [] : [value];
  return list
    .map((item) => (typeof item === "object" && item !== null ? plainValue(item.name) : plainValue(item)))
    .filter(Boolean);
}

function frontmatterData(yaml) {
  let data = {};
  try {
    data = yamlNode(yamlLines(yaml)) ?? {};
  } catch {
    data = {};
  }
  if (typeof data !== "object" || Array.isArray(data)) data = {};
  return {
    count: Object.keys(data).length,
    title: plainValue(data.title),
    subtitle: plainValue(data.subtitle),
    authors: authorNames(data.author),
    date: plainValue(data.date),
  };
}

function drawFrontmatter(data) {
  const block = el("div", "cm-lp-title cm-lp-md-frontmatter");
  if (data.title) block.append(el("div", "cm-lp-title-main", data.title));
  if (data.subtitle) block.append(el("div", "cm-lp-title-affil", data.subtitle));
  if (data.authors.length) block.append(el("div", "cm-lp-title-author", data.authors.join(", ")));
  if (data.date) block.append(el("div", "cm-lp-title-date", data.date));
  block.append(el("div", "cm-lp-md-keys", `Front matter: ${data.count} ${data.count === 1 ? "key" : "keys"}`));
  return block;
}

registerWidget("frontmatter", ({ token, source }) => {
  const yaml = bodyText(token, source) || textOf(source, token);
  return new MarkdownWidget("frontmatter", frontmatterData(yaml));
});

// ---- Executable cells ------------------------------------------------------

const CELL_ENGINES = { r: "R", python: "Python", julia: "Julia", ojs: "OJS" };

function drawCellhead(data) {
  const head = el("div", "cm-lp-cellhead");
  head.append(el("span", "cm-lp-cellhead-engine", data.engine));
  if (data.label) head.append(el("span", "cm-lp-cellhead-label", data.label));
  if (data.caption) head.append(el("span", "cm-lp-cellhead-caption", data.caption));
  for (const [key, value] of data.options) {
    head.append(el("span", "cm-lp-cellhead-option", `${key}: ${value}`));
  }
  return head;
}

registerWidget("cellhead", ({ token }) => {
  const lang = token.meta?.lang ?? "";
  const engine = own(CELL_ENGINES, lang.toLowerCase()) ? CELL_ENGINES[lang.toLowerCase()] : lang;
  return new MarkdownWidget("cellhead", {
    engine,
    label: token.meta?.label ?? "",
    caption: token.meta?.caption ?? "",
    options: JSON.parse(token.meta?.options ?? "[]"),
  });
});

// ---- Rules -----------------------------------------------------------------

function drawRule() {
  const rule = el("div", "cm-lp-hr");
  rule.append(el("hr"));
  return rule;
}

registerWidget("hr", () => new MarkdownWidget("hr", {}));

// ---- Figures ---------------------------------------------------------------

// The image is read through the same resolver the LaTeX and Typst images use.
function loadImage(image, figure, src, resolver) {
  if (!src) {
    figure.classList.add("cm-lv-image-missing");
    return;
  }
  if (isExternal(src) || !resolver) {
    image.src = src;
    return;
  }
  const path = resolveImagePath(resolver.currentPath(), src);
  Promise.resolve(resolver.resolve(path, src))
    .then((url) => {
      if (url) image.src = url;
      else figure.classList.add("cm-lv-image-missing");
    })
    .catch(() => {});
}

function drawFigure(data, { resolver }) {
  const figure = el("figure", "cm-lp-md-figure");
  const image = el("img");
  image.decoding = "async";
  image.loading = "lazy";
  image.alt = data.alt;
  image.title = data.src;
  if (data.width) image.style.width = data.width;
  figure.append(image);
  if (data.caption) figure.append(el("figcaption", "cm-lp-md-caption", data.caption));
  loadImage(image, figure, data.src, resolver);
  return figure;
}

registerWidget("mdfigure", ({ token, state }) => {
  const id = token.meta?.id ?? "";
  const number = id.startsWith("fig-") ? factsOf(state).labels.get(id)?.number : undefined;
  const alt = plainMarkup(token.meta?.alt ?? "");
  const hasNumber = number !== undefined && number !== "";
  const caption = hasNumber ? (alt ? `Figure ${number}: ${alt}` : `Figure ${number}`) : alt;
  const width = sizeOf(attribute(token.meta?.attrs ?? "", "width"));
  return new MarkdownWidget(
    "mdfigure",
    { src: token.meta?.src ?? "", alt, caption, width },
    { resolver: state.facet(imageResolver) },
  );
});

// ---- Citations -------------------------------------------------------------

// One item of a Pandoc citation: an optional note before, the suppress-author
// flag (-@key), the key, and a note after.
const CITE_ITEM = /^\s*(.*?)(-?)@([\w:.#$%&+?<>~\/-]*\w)(.*)$/s;

function parseCiteItem(piece) {
  const match = CITE_ITEM.exec(piece);
  if (!match) return null;
  return {
    prefix: match[1].trim(),
    suppress: match[2] === "-",
    key: match[3],
    suffix: match[4].replace(/^\s*,?\s*/, "").trim(),
  };
}

function renderCiteItem(item, bracketed, numeric, facts) {
  const entry = facts.citations.get(item.key);
  if (!entry) return [{ text: `?${item.key}`, muted: true }];

  const number = String(entry.number ?? item.key);
  const lead = item.prefix ? `${item.prefix} ` : "";
  const tail = item.suffix ? `, ${item.suffix}` : "";
  if (numeric) return bracketed ? [`${lead}${number}${tail}`] : [`[${number}]`];

  const names = nameList(entry.authors ?? []) || entry.title || item.key;
  const year = entry.year || "n.d.";
  if (!bracketed) return [`${names} (${year})`];
  return [`${lead}${item.suppress ? year : `${names} ${year}`}${tail}`];
}

// `[@a; @b]` reads "(Smith 2020; Jones 2019)"; a bare `@a` reads "Smith (2020)".
function citationParts(text, bracketed, facts) {
  const numeric = facts.citeStyle === "numeric";
  const items = text
    .split(";")
    .map(parseCiteItem)
    .filter(Boolean);
  if (!items.length) return [];

  const rendered = items.map((item) => renderCiteItem(item, bracketed, numeric, facts));
  if (!bracketed) return rendered[0];
  const [open, close] = numeric ? ["[", "]"] : ["(", ")"];
  return [open, ...joinParts(rendered, numeric ? ", " : "; "), close];
}

registerWidget("mdcite", ({ token, state }) => {
  const facts = factsOf(state);
  const text = token.meta?.text ?? "";
  const bracketed = token.meta?.bracketed === "true";
  const parts = citationParts(text, bracketed, facts);
  if (!parts.length) return null;
  const keys = text
    .split(";")
    .map(parseCiteItem)
    .filter(Boolean)
    .map((item) => item.key);
  return new ElementWidget({ className: "cm-lp-cite", content: parts, title: keys.join(", ") });
});

// ---- Cross-references ------------------------------------------------------

// The words Quarto writes before a number, by the prefix of the label.
const XREF_WORDS = {
  fig: "Figure",
  tbl: "Table",
  sec: "Section",
  eq: "Equation",
  thm: "Theorem",
  lem: "Lemma",
  cor: "Corollary",
  prp: "Proposition",
  cnj: "Conjecture",
  def: "Definition",
  exm: "Example",
  exr: "Exercise",
  sol: "Solution",
  rem: "Remark",
  lst: "Listing",
};

registerWidget("xref", ({ token, state }) => {
  const key = token.meta?.key ?? "";
  const prefix = token.meta?.prefix ?? key.split("-")[0];
  const found = factsOf(state).labels.get(key);
  if (!found) return new ElementWidget({ className: "cm-lp-ref", content: [{ text: `?@${key}`, muted: true }] });

  const number = found.number === undefined ? "" : String(found.number);
  const word = own(XREF_WORDS, prefix) ? XREF_WORDS[prefix] : "";
  let text;
  if (token.meta?.bare === "true") text = number || key;
  else if (!number) text = word || key;
  else if (prefix === "eq") text = `Equation (${number})`;
  else text = `${word} ${number}`;
  return new ElementWidget({ className: "cm-lp-ref", content: [text], title: key });
});

// ---- Blocks: callouts and theorems -----------------------------------------

const CALLOUT_TITLES = {
  note: "Note",
  tip: "Tip",
  warning: "Warning",
  important: "Important",
  caution: "Caution",
};

const THEOREM_WORDS = {
  thm: "Theorem",
  lem: "Lemma",
  cor: "Corollary",
  prp: "Proposition",
  cnj: "Conjecture",
  def: "Definition",
  exm: "Example",
  exr: "Exercise",
  sol: "Solution",
  rem: "Remark",
};

// A callout gets its title line; a theorem-like div gets "Theorem n (Name)."
// Other divs (columns, panels) keep their fence lines hidden with no header.
registerWidget("divhead", ({ token, state }) => {
  const attrs = token.meta?.attrs ?? "";
  const classes = (token.meta?.classes ?? "").split(/\s+/);

  const callout = classes.map((name) => /^callout-(\w+)$/.exec(name)).find(Boolean);
  if (callout && own(CALLOUT_TITLES, callout[1])) {
    const title = attribute(attrs, "title") || CALLOUT_TITLES[callout[1]];
    return new ElementWidget({ className: "cm-lp-callout-head", content: [title], tag: "div" });
  }

  const id = token.meta?.id ?? "";
  const prefix = /^([a-z]+)-/.exec(id)?.[1];
  if (prefix && own(THEOREM_WORDS, prefix)) {
    const number = factsOf(state).labels.get(id)?.number;
    const word = [THEOREM_WORDS[prefix], number === undefined ? "" : String(number)].filter(Boolean).join(" ");
    const name = attribute(attrs, "name");
    const title = `${name ? `${word} (${name})` : word}.`;
    return new ElementWidget({ className: "cm-lp-envhead cm-lp-envhead-theorem", content: [title], tag: "div" });
  }

  return null;
});

// ---- Footnotes -------------------------------------------------------------

// The number of a note is its place among the references before it, as Pandoc
// numbers them.
function noteNumber(source, from) {
  const before = source.slice(0, from);
  return (before.match(/\[\^[^\]\s]+\](?!:)|\^\[/g) ?? []).length + 1;
}

// The text of a labelled note, from its definition `[^label]: text` in the source.
function definitionText(source, label) {
  const escaped = label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(`^\\[\\^${escaped}\\]:\\s*(.*)$`, "m").exec(source);
  return match ? plainMarkup(match[1]) : "";
}

registerWidget("mdnote", ({ token, source }) => {
  const label = token.meta?.label ?? "";
  const note = label ? definitionText(source, label) : plainMarkup(bodyText(token, source));
  return new ElementWidget({
    className: "cm-lp-mdnote",
    tag: "sup",
    content: [String(noteNumber(source, token.from))],
    title: note,
  });
});

// ---- Shortcodes ------------------------------------------------------------

const SHORTCODE_CHIPS = {
  embed: "Embed",
  video: "Video",
  meta: "Meta",
  var: "Var",
  env: "Env",
};

// An include is a chip with the file's name and an Open button, as LaTeX's are.
function drawInclude(data, { opener }) {
  const element = el("span", "cm-lp-include", `Include: ${data.file}`);
  if (opener) {
    const button = el("button", "cm-lp-include-open", "Open");
    button.type = "button";
    // The button stops its own mousedown, so the reveal does not see it.
    button.addEventListener("mousedown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      opener(data.file);
    });
    element.append(button);
  }
  return element;
}

registerWidget("shortcode", ({ token, state }) => {
  const name = token.meta?.name ?? "";
  const first = (token.meta?.args ?? "").trim().split(/\s+/)[0].replace(/^["']|["']$/g, "");

  if (name === "include") {
    return new MarkdownWidget("include", { file: first }, { opener: state.facet(includeOpener) });
  }
  if (own(SHORTCODE_CHIPS, name)) {
    const text = first ? `${SHORTCODE_CHIPS[name]}: ${first}` : SHORTCODE_CHIPS[name];
    return new ElementWidget({ className: "cm-lp-chip", content: [text] });
  }
  if (name === "pagebreak") {
    return new ElementWidget({ className: "cm-lp-chip cm-lp-chip-block", content: ["Page break"] });
  }
  return new ElementWidget({ className: "cm-lp-chip", content: [`Shortcode: ${name}`] });
});

// The draw functions by widget name, for MarkdownWidget.
const DRAW = {
  frontmatter: drawFrontmatter,
  cellhead: drawCellhead,
  hr: drawRule,
  mdfigure: drawFigure,
  include: drawInclude,
};
