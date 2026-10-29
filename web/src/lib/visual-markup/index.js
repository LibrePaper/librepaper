import { Facet, StateEffect, StateField } from "@codemirror/state";
import { WidgetType } from "@codemirror/view";
import { typst2tex } from "tex2typst";
import "./markup.css";
import "./visual-markup.css";
import { preambleFold } from "./preamble.js";
import { cleanTitle, longToday, titleParts } from "./metadata.js";
import {
  createImageResolver,
  createTheme,
  DARK_THEME_COLORS,
  DEFAULT_CONFIG,
  imageResolver,
  LIGHT_THEME_COLORS,
  refreshVisual,
  registerWidget,
  revealOnMousedown,
  textOf,
  visualDefaults,
  visualExtension,
} from "./markup.ts";
import { COMMANDS, ENVIRONMENTS } from "./languages/latex/catalog.ts";
import { loadKatex } from "../katex.js";
import { VISUAL_FORMATS } from "./formats.js";

export { VISUAL_FORMATS } from "./formats.js";

// LaTeX's \includegraphics often leaves the extension off, so these are tried in turn.
const IMAGE_EXTENSIONS = [".png", ".jpg", ".jpeg", ".svg", ".pdf"];

// The facts about the open LaTeX project that the widgets read: labels, citations,
// acronyms, macros, theorems and the citation style. Every field starts empty.
function emptyFacts() {
  return {
    labels: new Map(),
    citations: new Map(),
    acronyms: new Map(),
    textMacros: new Map(),
    mathMacros: {},
    theorems: new Map(),
    environmentNumbers: new Map(),
    citeStyle: "author-year",
  };
}

const NO_FACTS = emptyFacts();

/// Replaces the facts the widgets read. Takes the object that the editor computes.
export const setFacts = StateEffect.define();

// The latest facts for the open LaTeX file.
const factsField = StateField.define({
  create: () => emptyFacts(),
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setFacts)) value = effect.value;
    }
    return value;
  },
});

// The facts in force for a state; a state without the field gets empty ones.
function factsOf(state) {
  return state.field(factsField, false) ?? NO_FACTS;
}

/// The effects that give the open file new facts and redraw the widgets.
export function factsUpdate(facts) {
  return [setFacts.of(facts), refreshVisual.of(null)];
}

// The callback that opens an included file, given its raw name. The first one wins.
const includeOpener = Facet.define({ combine: (values) => values[0] ?? null });

/// The visual layer for one file, or nothing for a format it does not draw.
/// `path` returns the open file's project path (no leading slash), for resolving relative image paths.
/// `imageUrl(path)` resolves a project path (no leading slash) to a URL an <img> can load, or null.
/// `dark` picks the dark colours.
/// `openInclude(name)` opens the file an \input, \include or \subfile names; without it the chip has no button.
export function visualMarkup(format, { path = () => "", imageUrl = null, dark = false, openInclude = null } = {}) {
  if (!VISUAL_FORMATS.has(format)) return [];

  const extensions = [
    visualExtension(),
    visualDefaults.of({ enabled: true, language: format }),
    createTheme({
      ...DEFAULT_CONFIG,
      theme: dark ? "dark" : "light",
      styles: {
        ...DEFAULT_CONFIG.styles,
        colors: dark ? DARK_THEME_COLORS : LIGHT_THEME_COLORS,
      },
    }),
  ];

  if (imageUrl) {
    // Upstream resolves against leading-slash paths; LibrePaper's have none.
    extensions.push(
      imageResolver.of(
        createImageResolver(
          () => "/" + path(),
          (resolved) => resolveImage(imageUrl, resolved.replace(/^\/+/, "")),
        ),
      ),
    );
  }

  if (format === "latex") {
    extensions.push(preambleFold(), factsField);
    if (openInclude) extensions.push(includeOpener.of(openInclude));
  }

  return extensions;
}

async function resolveImage(imageUrl, file) {
  const url = await imageUrl(file);
  if (url || /\.[^/]+$/.test(file)) return url;
  for (const extension of IMAGE_EXTENSIONS) {
    const found = await imageUrl(file + extension);
    if (found) return found;
  }
  return null;
}

// KaTeX lacks some math environments and has no \label. The environment is
// rebuilt in a form KaTeX accepts. Display labels with a number become a \tag
// of that number; the rest of the labels are removed.
function latexMath(body, environment, numbers) {
  let tex;
  if (!environment || environment === "equation" || environment === "equation*") {
    tex = body;
  } else if (environment === "multline" || environment === "multline*") {
    tex = `\\begin{gather*}${body}\\end{gather*}`;
  } else if (environment === "eqnarray" || environment === "eqnarray*") {
    tex = `\\begin{array}{rcl}${body}\\end{array}`;
  } else {
    // align, align*, gather and gather* are KaTeX's own environments.
    tex = `\\begin{${environment}}${body}\\end{${environment}}`;
  }
  // A formula with its own \tag keeps it; KaTeX rejects a second one.
  if (numbers && !/\\tag\s*\{/.test(tex)) {
    tex = tex.replace(/\\label\s*\{([^{}]*)\}/g, (match, key) => {
      const found = numbers.get(key.trim());
      return found ? `\\tag{${found.number}}` : "";
    });
  }
  return tex.replace(/\\label\s*\{[^{}]*\}/g, "");
}

// Typst source is converted to TeX, and on failure the raw text stays.
function typstTex(tex, display, text) {
  try {
    return typst2tex(tex, { blockMathMode: display });
  } catch {
    return text;
  }
}

// The macros are compared by their JSON once per facts object, not once per formula.
const macroKeys = new WeakMap();

function macroKey(macros) {
  let key = macroKeys.get(macros);
  if (key === undefined) {
    key = JSON.stringify(macros);
    macroKeys.set(macros, key);
  }
  return key;
}

// Typesets a math token with KaTeX. The raw source shows until KaTeX loads,
// and stays if it fails.
class KatexMath extends WidgetType {
  constructor(context) {
    super();
    const { token, source, language, state } = context;
    const facts = factsOf(state);
    this.text = textOf(source, token);
    this.tex = token.body ? textOf(source, token.body) : this.text;
    this.environment = token.name;
    this.display = Boolean(token.display);
    this.language = language.id;
    // KaTeX mutates the macros object it is given, so each render gets a copy.
    this.macros = { ...(facts.mathMacros ?? {}) };
    this.macroKey = macroKey(facts.mathMacros ?? {});
    // The final TeX is compared by eq(), so a renumbering redraws the formula.
    this.formula =
      this.language === "typst"
        ? typstTex(this.tex, this.display, this.text)
        : latexMath(this.tex, this.environment, this.display ? facts.labels : null);
  }

  eq(other) {
    return (
      other instanceof KatexMath &&
      other.text === this.text &&
      other.formula === this.formula &&
      other.macroKey === this.macroKey &&
      other.display === this.display &&
      other.language === this.language
    );
  }

  toDOM(view) {
    const element = document.createElement(this.display ? "div" : "span");
    element.className = `cm-lv-widget cm-lv-math ${this.display ? "cm-lv-math-display" : "cm-lv-math-inline"}`;
    element.textContent = this.text;

    loadKatex()
      .then((katex) => {
        if (!element.isConnected) return;
        katex.render(this.formula, element, {
          displayMode: this.display,
          throwOnError: false,
          macros: { ...this.macros },
        });
      })
      .catch(() => {});

    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

// Runs after the markup.ts import, which registers the MathLive math widget; this
// overwrites it.
registerWidget("math", (context) => new KatexMath(context));

// The words a reference shows before its number, by the kind of thing it points
// at. Each package has its own: cleveref lowercases them in \cref and capitalises
// them in \Cref, and hyperref's \autoref writes an equation without parentheses.
const REF_WORDS = {
  cref: { section: "section", equation: "eq.", figure: "fig.", table: "table" },
  Cref: { section: "Section", equation: "Eq.", figure: "Fig.", table: "Table" },
  autoref: { section: "section", equation: "Equation", figure: "Figure", table: "Table" },
};

// The text of one key in a reference. An unknown key shows as ??. \ref, \pageref
// and \nameref show the bare number, as do kinds with no words of their own.
function describeRef(name, key, labels) {
  const found = labels.get(key);
  if (!found) return "??";
  const { number, kind } = found;
  if (name === "eqref") return `(${number})`;
  const word = REF_WORDS[name]?.[kind];
  if (!word) return number;
  if (kind === "equation" && name !== "autoref") return `${word} (${number})`;
  return `${word} ${number}`;
}

// A \ref-style command, shown as the number of the label it names.
class RefWidget extends WidgetType {
  constructor(context) {
    super();
    const { token, source, state } = context;
    const labels = factsOf(state).labels;
    this.raw = token.args?.[0] ? source.slice(token.args[0].from, token.args[0].to).trim() : "";
    this.text = this.raw
      .split(",")
      .map((key) => describeRef(token.name, key.trim(), labels))
      .join(", ");
  }

  eq(other) {
    return other instanceof RefWidget && other.text === this.text && other.raw === this.raw;
  }

  toDOM(view) {
    const element = document.createElement("span");
    element.className = "cm-lp-ref";
    element.title = this.raw;
    element.textContent = this.text;
    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

registerWidget("ref", (context) => new RefWidget(context));

// A \label outside maths, shown as a small tag at the right edge of its line.
class LabelWidget extends WidgetType {
  constructor(context) {
    super();
    const { token, source } = context;
    this.key = token.args?.[0] ? source.slice(token.args[0].from, token.args[0].to).trim() : "";
  }

  eq(other) {
    return other instanceof LabelWidget && other.key === this.key;
  }

  toDOM(view) {
    const element = document.createElement("span");
    element.className = "cm-lp-label";
    element.title = "Label";
    element.textContent = this.key;
    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

registerWidget("label", (context) => new LabelWidget(context));

// An \input, \include or \subfile, shown as a chip with the file's name.
class IncludeWidget extends WidgetType {
  constructor(context) {
    super();
    const { token, source, state } = context;
    this.name = token.args?.[0] ? source.slice(token.args[0].from, token.args[0].to).trim() : "";
    this.opener = state.facet(includeOpener);
  }

  eq(other) {
    return other instanceof IncludeWidget && other.name === this.name && other.opener === this.opener;
  }

  toDOM(view) {
    const element = document.createElement("span");
    element.className = "cm-lp-include";
    element.append(this.name);

    if (this.opener) {
      const opener = this.opener;
      const name = this.name;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "cm-lp-include-open";
      button.textContent = "Open";
      // The button stops its own mousedown, so the reveal below does not see it.
      button.addEventListener("mousedown", (event) => {
        event.preventDefault();
        event.stopPropagation();
        opener(name);
      });
      element.appendChild(button);
    }

    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

registerWidget("include", (context) => new IncludeWidget(context));

// Makes one part of the title block open its command in the source. The listener
// sits on the part, a child of the block, so it runs before the block's own reveal,
// and stopping it there keeps that reveal from moving the caret to \maketitle.
function linkPart(view, block, element, part) {
  if (part.from === null) return;
  element.dataset.part = part.kind;
  element.dataset.index = String(part.index);
  element.title = "Edit in source";
  element.addEventListener("mousedown", (event) => {
    // The offset from build time may be stale, so the part is found again in the current text.
    const current = titleParts(view.state.doc.toString(), view.posAtDOM(block)).find(
      (candidate) => candidate.kind === part.kind && candidate.index === part.index,
    );
    if (!current || current.from === null) return;
    event.preventDefault();
    event.stopPropagation();
    view.dispatch({ selection: { anchor: current.from }, scrollIntoView: true });
    view.focus();
  });
}

// The title block that \maketitle draws, from the document's \title, \author, \date
// and affiliation lines. Each part is a click target for its command's argument.
class TitleWidget extends WidgetType {
  constructor(context) {
    super();
    this.parts = titleParts(context.source, context.token.from);
    // The parts' display strings and targets, which is what the DOM is built from.
    this.signature = JSON.stringify(this.parts.map(({ kind, index, text, from }) => [kind, index, text, from !== null]));
  }

  eq(other) {
    return other instanceof TitleWidget && other.signature === this.signature;
  }

  toDOM(view) {
    const block = document.createElement("div");
    block.className = "cm-lp-title";
    const of = (kind) => this.parts.filter((part) => part.kind === kind);

    const [title] = of("title");
    const main = document.createElement("div");
    main.className = "cm-lp-title-main";
    if (title.text) {
      main.textContent = title.text;
    } else {
      main.textContent = "Untitled";
      main.classList.add("cm-lp-title-muted");
    }
    linkPart(view, block, main, title);
    block.appendChild(main);

    const authors = of("author");
    if (authors.length) {
      const author = document.createElement("div");
      author.className = "cm-lp-title-author";
      authors.forEach((part, index) => {
        if (index) author.append(", ");
        const span = document.createElement("span");
        span.textContent = part.text;
        linkPart(view, block, span, part);
        author.appendChild(span);
      });
      block.appendChild(author);
    }

    for (const part of of("affil")) {
      const affil = document.createElement("div");
      affil.className = "cm-lp-title-affil";
      affil.textContent = part.text;
      linkPart(view, block, affil, part);
      block.appendChild(affil);
    }

    const [date] = of("date");
    if (date.text) {
      const element = document.createElement("div");
      element.className = "cm-lp-title-date";
      element.textContent = date.text;
      linkPart(view, block, element, date);
      block.appendChild(element);
    }

    revealOnMousedown(view, block);
    return block;
  }

  ignoreEvent() {
    return true;
  }
}

registerWidget("title", (context) => new TitleWidget(context));

// ---------------------------------------------------------------------------
// The widgets for the catalog's commands and environments. Most of them draw a
// line of text, so they share one element widget.
// ---------------------------------------------------------------------------

// Writes content parts into an element. A part is a string, or { text, sup?, muted? }.
function appendParts(element, parts) {
  for (const part of parts) {
    if (typeof part === "string") {
      element.append(part);
      continue;
    }
    const child = document.createElement(part.sup ? "sup" : "span");
    if (part.muted) child.className = "cm-lp-unknown";
    child.textContent = part.text;
    element.append(child);
  }
}

// An element with text drawn in it. A click reveals the source; a Ctrl or Cmd click
// opens `openUrl` in a new tab when it is given.
class ElementWidget extends WidgetType {
  constructor({ className, content, title = "", tag = "span", openUrl = "" }) {
    super();
    this.className = className;
    this.content = Array.isArray(content) ? content : [content];
    this.title = title;
    this.tag = tag;
    this.openUrl = openUrl;
    this.signature = JSON.stringify([className, this.content, title, tag, openUrl]);
  }

  eq(other) {
    return other instanceof ElementWidget && other.signature === this.signature;
  }

  toDOM(view) {
    const element = document.createElement(this.tag);
    element.className = this.className;
    if (this.title) element.title = this.title;
    appendParts(element, this.content);

    if (this.openUrl) {
      const url = this.openUrl;
      element.addEventListener("mousedown", (event) => {
        if (!event.ctrlKey && !event.metaKey) return;
        // Registered before the reveal, so stopping it keeps the caret where it is.
        event.preventDefault();
        event.stopImmediatePropagation();
        window.open(url, "_blank", "noopener");
      });
    }

    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

// The trimmed source text of a range, or "" when the argument is missing.
function argText(source, range) {
  return range ? textOf(source, range).trim() : "";
}

// The trimmed text of each mandatory argument of a token.
function argsOf(source, token) {
  return (token.args ?? []).map((range) => argText(source, range));
}

// Joins arrays of content parts with a separator between them.
function joinParts(items, separator) {
  return items.flatMap((item, index) => (index ? [separator, ...item] : item));
}

// ---- Citations -------------------------------------------------------------

// How each citation command reads. Commands not listed here are parenthetical:
// the whole citation in brackets, as \citep and \parencite write it.
const CITE_MODES = {
  citet: "textual",
  Citet: "textual",
  textcite: "textual",
  textcites: "textual",
  citealt: "bare",
  citealp: "bare",
  citeauthor: "author",
  citename: "author",
  citelist: "author",
  citefield: "author",
  citeyear: "year",
  citeyearpar: "yearpar",
  footcite: "footnote",
  footcitetext: "footnote",
  footfullcite: "footnote",
  supercite: "footnote",
  nocite: "hidden",
};

function citeMode(name) {
  return Object.hasOwn(CITE_MODES, name) ? CITE_MODES[name] : "parenthetical";
}

// Two authors read "Smith and Jones"; three or more read "Smith et al.".
function nameList(authors) {
  if (authors.length === 0) return "";
  if (authors.length === 1) return authors[0];
  if (authors.length === 2) return `${authors[0]} and ${authors[1]}`;
  return `${authors[0]} et al.`;
}

// The content of one \cite-style command. `pre` and `post` are the notes that
// come before and after the citation, such as "see" and "p. 3".
function citeParts(name, keys, { pre, post }, facts) {
  const mode = citeMode(name);
  if (mode === "hidden") return [];

  const numeric = facts.citeStyle === "numeric";
  const single = keys.length === 1;
  const entries = keys.map((key) => ({ key, entry: facts.citations.get(key) }));

  // In numeric styles a footnote citation is a superscript number, and it has no brackets.
  if (mode === "footnote" && numeric) {
    const numbers = entries.map(({ key, entry }) => (entry ? String(entry.number) : `${key}?`));
    return [{ text: numbers.join(","), sup: true }];
  }

  // The notes sit inside the citation's own text when it is textual and alone.
  const noted = (text) => [pre ? `${pre} ` : "", text, post ? `, ${post}` : ""].join("");

  const items = entries.map(({ key, entry }) => {
    if (!entry) return [{ text: `${key}?`, muted: true }];
    const names = nameList(entry.authors ?? []) || entry.title || key;
    const year = entry.year || "n.d.";
    const number = String(entry.number);
    switch (mode) {
      case "author":
        return [names];
      case "year":
        return [year];
      case "yearpar":
        return [`(${year})`];
      case "bare":
        return [`${names} ${year}`];
      case "textual":
        if (numeric) return [`${names} [${single ? noted(number) : number}]`];
        return [`${names} (${single ? noted(year) : year})`];
      default:
        return [numeric ? number : `${names} ${year}`];
    }
  });

  const numericList = numeric && (mode === "parenthetical" || mode === "footnote");
  const body = joinParts(items, numericList ? ", " : "; ");

  if (mode === "parenthetical" || mode === "footnote") {
    // Notes wrap the whole list when it is in brackets.
    const [open, close] = numeric ? ["[", "]"] : ["(", ")"];
    return [open, pre ? `${pre} ` : "", ...body, post ? `, ${post}` : "", close];
  }
  return body;
}

// The keys of a citation, in order, from every mandatory argument.
function citeKeys(source, token) {
  return (token.args ?? [])
    .flatMap((range) => textOf(source, range).split(","))
    .map((key) => key.trim())
    .filter(Boolean);
}

registerWidget("cite", ({ token, source, state }) => {
  const facts = factsOf(state);
  const keys = citeKeys(source, token);
  // Options are [pre][post], or a single post note.
  const options = (token.options ?? []).map((range) => cleanTitle(textOf(source, range)));
  const [pre, post] = options.length >= 2 ? options : [null, options[0] ?? null];
  const hidden = citeMode(token.name) === "hidden";
  return new ElementWidget({
    className: hidden ? "cm-lp-cite cm-lp-cite-hidden" : "cm-lp-cite",
    content: citeParts(token.name, keys, { pre, post }, facts),
    title: keys.join(", "),
  });
});

// ---- Links -----------------------------------------------------------------

// Only web and mail links are opened; anything else in a link is shown but not followed.
function isWebUrl(url) {
  return /^(https?:|mailto:)/i.test(url);
}

registerWidget("link", ({ token, source }) => {
  const args = argsOf(source, token);
  const name = token.name;
  let text;
  let url;
  if (name === "href") {
    url = args[0] ?? "";
    text = cleanTitle(args[1] ?? "") || url;
  } else if (name === "hyperlink") {
    url = `#${args[0] ?? ""}`;
    text = cleanTitle(args[1] ?? "");
  } else if (name === "hyperref") {
    const label = token.options?.[0] ? argText(source, token.options[0]) : "";
    url = `#${label}`;
    text = cleanTitle(args[0] ?? "");
  } else {
    // \url, \nolinkurl and \path show the address itself.
    url = args[0] ?? "";
    text = url;
  }
  return new ElementWidget({
    className: "cm-lp-link",
    content: [text],
    title: url,
    openUrl: isWebUrl(url) ? url : "",
  });
});

// ---- Footnotes -------------------------------------------------------------

// Notes are marked with a star for now; the note itself is the marker's title.
registerWidget("footnote", ({ token, source }) => {
  const note = token.body ? cleanTitle(argText(source, token.body)) : "";
  return new ElementWidget({
    className: "cm-lp-footnote",
    content: ["*"],
    title: note,
    tag: "sup",
  });
});

// ---- Verbatim code ---------------------------------------------------------

registerWidget("verb", ({ token, source }) => {
  const code = token.body ? textOf(source, token.body) : "";
  return new ElementWidget({ className: "cm-lp-verb", content: [code] });
});

// ---- Chips -----------------------------------------------------------------

// A catalog chip for a command, or for an environment that folds into a block.
// The first argument follows the chip's text when the catalog asks for it.
registerWidget("chip", ({ token, source }) => {
  const command = COMMANDS.get(token.name)?.render;
  const environment = ENVIRONMENTS.get(token.name)?.render;
  const render =
    command?.kind === "chip"
      ? command
      : environment?.kind === "opaque"
        ? { text: environment.text, block: true, argument: false }
        : null;
  if (!render) return null;

  let text = render.text;
  const first = cleanTitle(argText(source, token.args?.[0]));
  if (render.argument && first) text += `: ${first}`;
  return new ElementWidget({
    className: render.block ? "cm-lp-chip cm-lp-chip-block" : "cm-lp-chip",
    content: [text],
  });
});

// ---- Units (siunitx) -------------------------------------------------------

const SI_PREFIXES = {
  kilo: "k",
  mega: "M",
  giga: "G",
  milli: "m",
  micro: "µ",
  nano: "n",
  pico: "p",
  centi: "c",
  deci: "d",
};

const SI_UNITS = {
  metre: "m",
  meter: "m",
  second: "s",
  gram: "g",
  kelvin: "K",
  ampere: "A",
  mole: "mol",
  candela: "cd",
  hertz: "Hz",
  newton: "N",
  joule: "J",
  watt: "W",
  volt: "V",
  pascal: "Pa",
  percent: "%",
  degree: "°",
  celsius: "°C",
  litre: "L",
  liter: "L",
  hour: "h",
  minute: "min",
  ohm: "Ω",
};

// Commands that write nothing of their own; their braces hold the text.
const UNIT_WRAPPERS = new Set(["text", "textrm", "mbox", "mathrm", "si", "unit"]);

// A siunitx unit, such as \meter\per\second, in its written form: "m/s".
function unitText(tex) {
  const tokens = [];
  for (const match of tex.matchAll(/\\([a-zA-Z]+)|\\([\s\S])|([^\\]+)/g)) {
    if (match[1]) tokens.push({ name: match[1] });
    else if (match[2]) {
      if (/[,;:! ]/.test(match[2])) tokens.push({ space: true });
    } else tokens.push({ text: match[3] });
  }

  const units = [];
  let per = false;
  let exponent = false;
  const add = (text) => {
    units.push({ text, per });
    per = false;
  };

  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i];
    if (token.space) continue;

    if (token.text !== undefined) {
      const text = token.text.replace(/[{}~]/g, " ").trim();
      if (exponent && units.length) {
        units[units.length - 1].text += `^${text}`;
        exponent = false;
      } else if (text === "/") {
        per = true;
      } else if (text) {
        add(text);
      }
      continue;
    }

    const name = token.name;
    if (name === "per") {
      per = true;
    } else if (name === "squared" || name === "cubed") {
      if (units.length) units[units.length - 1].text += name === "squared" ? "²" : "³";
    } else if (name === "tothe") {
      exponent = true;
    } else if (UNIT_WRAPPERS.has(name)) {
      continue;
    } else if (Object.hasOwn(SI_PREFIXES, name) && Object.hasOwn(SI_UNITS, tokens[i + 1]?.name)) {
      add(SI_PREFIXES[name] + SI_UNITS[tokens[i + 1].name]);
      i++;
    } else {
      add(Object.hasOwn(SI_UNITS, name) ? SI_UNITS[name] : name);
    }
  }

  let out = "";
  units.forEach((unit, index) => {
    if (index === 0) out = (unit.per ? "/" : "") + unit.text;
    else out += (unit.per ? "/" : " ") + unit.text;
  });
  return out;
}

// A number with thin spaces in its thousands, as siunitx groups four or more digits.
function numberText(text) {
  const match = /^([+-]?)(\d*)(\.\d+)?(.*)$/s.exec(text.trim());
  const [, sign, integer, fraction = "", rest] = match;
  const grouped = integer.length >= 4 ? integer.replace(/\B(?=(\d{3})+(?!\d))/g, " ") : integer;
  const power = /^\s*e\s*([+-]?\d+)\s*$/.exec(rest);
  const tail = power ? `×10^${power[1]}` : cleanTitle(rest.replace(/\\times/g, "×"));
  return sign + grouped + fraction + tail;
}

// \ang{30;15;4}: degrees, then minutes and seconds.
function angleText(text) {
  const [degrees, minutes, seconds] = text.split(";");
  return (
    numberText(degrees ?? "") +
    "°" +
    (minutes ? numberText(minutes) + "′" : "") +
    (seconds ? numberText(seconds) + "″" : "")
  );
}

// A number and its unit with a space between them; the unit may be absent.
function joinedText(number, unit) {
  return unit ? `${number} ${unit}` : number;
}

function siText(name, args) {
  const [first = "", second = "", third = ""] = args;
  switch (name) {
    case "num":
      return numberText(first);
    case "ang":
      return angleText(first);
    case "si":
    case "unit":
      return unitText(first);
    case "SI":
    case "qty":
      return joinedText(numberText(first), unitText(second));
    case "numrange":
      return `${numberText(first)} to ${numberText(second)}`;
    case "SIrange":
    case "qtyrange":
      return joinedText(`${numberText(first)} to ${numberText(second)}`, unitText(third));
    default:
      return "";
  }
}

registerWidget("unit", ({ token, source }) => {
  const text = siText(token.name, argsOf(source, token));
  return new ElementWidget({ className: "cm-lp-unit", content: [text] });
});

// ---- Glossary and acronyms --------------------------------------------------

// Which form each glossary command shows. "full" is "Long (SHORT)".
const GLOSS_FORMS = {
  gls: { form: "short" },
  glsentrytext: { form: "short" },
  ac: { form: "short" },
  acs: { form: "short" },
  Gls: { form: "short", capital: true },
  glspl: { form: "short", plural: true },
  acp: { form: "short", plural: true },
  Glspl: { form: "short", plural: true, capital: true },
  glsfirst: { form: "full" },
  acf: { form: "full" },
  acl: { form: "long" },
  glsentrylong: { form: "long" },
};

function pluralise(word) {
  return word ? `${word}s` : word;
}

function glossText(name, entry) {
  const { form, plural, capital } = Object.hasOwn(GLOSS_FORMS, name) ? GLOSS_FORMS[name] : GLOSS_FORMS.gls;
  const short = plural ? pluralise(entry.short) : entry.short ?? "";
  const long = plural ? pluralise(entry.long) : entry.long ?? "";
  let text;
  if (form === "short") text = short || long;
  else if (form === "long") text = long || short;
  else text = long && short ? `${long} (${short})` : long || short;
  return capital ? text.charAt(0).toUpperCase() + text.slice(1) : text;
}

registerWidget("gloss", ({ token, source, state }) => {
  const key = argText(source, token.args?.[0]);
  const entry = factsOf(state).acronyms.get(key);
  if (!entry) {
    return new ElementWidget({
      className: "cm-lp-gloss cm-lp-unknown",
      content: [key],
      title: `No acronym named ${key}`,
    });
  }
  return new ElementWidget({
    className: "cm-lp-gloss",
    content: [glossText(token.name, entry)],
    title: entry.long || entry.short || "",
  });
});

// ---- Quotations ------------------------------------------------------------

// \enquote with a star, and the star form of the others, use single marks.
registerWidget("quote", ({ token, source }) => {
  const text = cleanTitle(argText(source, token.body));
  const [open, close] = token.star ? ["‘", "’"] : ["“", "”"];
  return new ElementWidget({ className: "cm-lp-quote-inline", content: [`${open}${text}${close}`] });
});

// ---- Environment headers ---------------------------------------------------

// The header of a theorem-like, proof, abstract or keywords environment, or null
// for any other environment, whose header then stays hidden.
registerWidget("envhead", ({ token, source, state }) => {
  const facts = factsOf(state);
  const catalog = ENVIRONMENTS.get(token.name)?.render;
  const option = token.options?.[0] ? cleanTitle(argText(source, token.options[0])) : "";
  const shared = facts.theorems.get(token.name);

  const known = catalog?.kind === "theorem" ? catalog : null;
  if (shared || known) {
    const title = shared ? shared.title : known.title;
    const numbered = shared ? shared.numbered : known.numbered;
    const number = numbered ? facts.environmentNumbers.get(token.from) : undefined;
    let text = [title, number ? String(number) : ""].filter(Boolean).join(" ");
    if (option) text += ` (${option})`;
    return new ElementWidget({ className: "cm-lp-envhead cm-lp-envhead-theorem", content: [`${text}.`] });
  }

  if (catalog?.kind === "proof") {
    const text = option ? (option.endsWith(".") ? option : `${option}.`) : "Proof.";
    return new ElementWidget({ className: "cm-lp-envhead cm-lp-envhead-proof", content: [text] });
  }
  if (catalog?.kind === "abstract") {
    return new ElementWidget({ className: "cm-lp-envhead cm-lp-envhead-abstract", content: ["Abstract"] });
  }
  if (catalog?.kind === "keywords") {
    return new ElementWidget({ className: "cm-lp-envhead cm-lp-envhead-keywords", content: ["Keywords"] });
  }
  return null;
});

// ---- User macros -----------------------------------------------------------

// Commands that keep their formatting when a macro's text is shown as plain text.
const FORMATTING_COMMANDS = "textbf|textit|emph|textrm|textsf|texttt|textsc|textsl|underline|mbox";

// The text of a use of a user text macro, with its arguments filled in, or null
// when the body needs more than plain text can show.
function macroText(name, token, source, facts) {
  const macro = facts.textMacros.get(name);
  if (!macro) return null;
  if (new RegExp(`\\\\(?!(?:${FORMATTING_COMMANDS})\\b)[a-zA-Z]`).test(macro.body)) return null;

  const args = argsOf(source, token);
  const values = [];
  let mandatory = macro.args;
  // An optional first argument takes its default when the use leaves it out.
  if (macro.optional !== null) {
    values.push(token.options?.[0] ? argText(source, token.options[0]) : macro.optional);
    mandatory -= 1;
  }
  if (args.length < mandatory) return null;
  values.push(...args.slice(0, mandatory));

  const body = macro.body.replace(/#([1-9])/g, (match, digit) => (values[digit - 1] ?? match));
  if (/#[1-9]/.test(body)) return null;
  return cleanTitle(body);
}

registerWidget("macro", ({ token, source, state }) => {
  const text = macroText(token.name, token, source, factsOf(state));
  if (text === null) return null;
  return new ElementWidget({ className: "cm-lp-macro", content: [text], title: `\\${token.name}` });
});

// ---- Bibliography items ----------------------------------------------------

// The marker of a \bibitem: its optional label, or its number. The number is the
// citation's order when the key is cited, and the item's place otherwise.
registerWidget("bibitem", ({ token, source, state }) => {
  const facts = factsOf(state);
  const label = token.options?.[0] ? cleanTitle(argText(source, token.options[0])) : "";
  const key = argText(source, token.args?.[0]);
  let marker = label;
  if (!marker) {
    const entry = facts.citations.get(key);
    const before = source.slice(0, token.from).match(/\\bibitem\b/g)?.length ?? 0;
    marker = String(entry ? entry.number : before + 1);
  }
  return new ElementWidget({ className: "cm-lp-bibitem", content: [`[${marker}]`] });
});

// ---- Symbols ---------------------------------------------------------------

const SYMBOL_TEXT = {
  LaTeX: "LaTeX",
  TeX: "TeX",
  LaTeXe: "LaTeX2ε",
};

registerWidget("symbol", ({ token }) => {
  if (token.name === "today") {
    return new ElementWidget({ className: "cm-lp-symbol", content: [longToday()] });
  }
  if (!Object.hasOwn(SYMBOL_TEXT, token.name)) return null;
  return new ElementWidget({
    className: "cm-lp-symbol cm-lp-logo",
    content: [SYMBOL_TEXT[token.name]],
  });
});
