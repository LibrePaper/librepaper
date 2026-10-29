import { Facet, StateEffect, StateField } from "@codemirror/state";
import { WidgetType } from "@codemirror/view";
import { typst2tex } from "tex2typst";
import "./markup.css";
import "./visual-markup.css";
import { preambleFold } from "./preamble.js";
import { stripComments } from "./labels.js";
import {
  createImageResolver,
  createTheme,
  DARK_THEME_COLORS,
  DEFAULT_CONFIG,
  imageResolver,
  LIGHT_THEME_COLORS,
  refreshVisual,
  registerWidget,
  textOf,
  visualDefaults,
  visualExtension,
} from "./markup.ts";
import { loadKatex } from "../katex.js";

/// The formats the visual layer draws.
export const VISUAL_FORMATS = new Set(["latex", "typst"]);

// LaTeX's \includegraphics often leaves the extension off, so these are tried in turn.
const IMAGE_EXTENSIONS = [".png", ".jpg", ".jpeg", ".svg", ".pdf"];

/// Replaces the numbers the references show. Takes the Map that numberLabels returns.
export const setLabels = StateEffect.define();

// The latest label numbers for the open LaTeX file, read by refs and display maths.
const labelsField = StateField.define({
  create: () => new Map(),
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setLabels)) value = effect.value;
    }
    return value;
  },
});

/// The effects that give the open file new label numbers and redraw the widgets.
export function labelsUpdate(labels) {
  return [setLabels.of(labels), refreshVisual.of(null)];
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
    extensions.push(preambleFold(), labelsField);
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

// Typesets a math token with KaTeX. The raw source shows until KaTeX loads,
// and stays if it fails.
class KatexMath extends WidgetType {
  constructor(context) {
    super();
    const { token, source, language, state } = context;
    this.text = textOf(source, token);
    this.tex = token.body ? textOf(source, token.body) : this.text;
    this.environment = token.name;
    this.display = Boolean(token.display);
    this.language = language.id;
    // The final TeX is compared by eq(), so a renumbering redraws the formula.
    this.formula =
      this.language === "typst"
        ? typstTex(this.tex, this.display, this.text)
        : latexMath(
            this.tex,
            this.environment,
            this.display ? state.field(labelsField, false) : null,
          );
  }

  eq(other) {
    return (
      other instanceof KatexMath &&
      other.text === this.text &&
      other.formula === this.formula &&
      other.display === this.display &&
      other.language === this.language
    );
  }

  toDOM() {
    const element = document.createElement(this.display ? "div" : "span");
    element.className = `cm-lv-widget cm-lv-math ${this.display ? "cm-lv-math-display" : "cm-lv-math-inline"}`;
    element.textContent = this.text;

    loadKatex()
      .then((katex) => {
        if (!element.isConnected) return;
        katex.render(this.formula, element, {
          displayMode: this.display,
          throwOnError: false,
        });
      })
      .catch(() => {});

    return element;
  }

  // A click places the caret, which reveals the source.
  ignoreEvent() {
    return false;
  }
}

// Runs after the markup.ts import, which registers the MathLive math widget; this
// overwrites it.
registerWidget("math", (context) => new KatexMath(context));

// The words a reference shows, by the kind of thing it points at.
const REF_PREFIX = { section: "Section", equation: "Equation", figure: "Figure", table: "Table" };

// The text of one key in a reference. An unknown key shows as ??.
function describeRef(name, key, labels) {
  const found = labels.get(key);
  if (!found) return "??";
  const { number, kind } = found;
  if (name === "eqref") return `(${number})`;
  if (name === "autoref" || name === "cref" || name === "Cref") {
    if (kind === "equation") return name === "autoref" ? `Equation (${number})` : `Eq. (${number})`;
    return `${REF_PREFIX[kind]} ${number}`;
  }
  return number;
}

// A \ref-style command, shown as the number of the label it names.
class RefWidget extends WidgetType {
  constructor(context) {
    super();
    const { token, source, state } = context;
    const labels = state.field(labelsField, false) ?? new Map();
    this.raw = token.args?.[0] ? source.slice(token.args[0].from, token.args[0].to).trim() : "";
    this.text = this.raw
      .split(",")
      .map((key) => describeRef(token.name, key.trim(), labels))
      .join(", ");
  }

  eq(other) {
    return other instanceof RefWidget && other.text === this.text && other.raw === this.raw;
  }

  toDOM() {
    const element = document.createElement("span");
    element.className = "cm-lp-ref";
    element.title = this.raw;
    element.textContent = this.text;
    return element;
  }

  // A click reveals the source.
  ignoreEvent() {
    return false;
  }
}

registerWidget("ref", (context) => new RefWidget(context));

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

  toDOM() {
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
      button.addEventListener("mousedown", (event) => {
        event.preventDefault();
        event.stopPropagation();
        opener(name);
      });
      element.appendChild(button);
    }

    return element;
  }

  // The button handles its own clicks; anywhere else on the chip reveals the source.
  ignoreEvent(event) {
    const target = event.target;
    const element = target instanceof Element ? target : target?.parentElement;
    return Boolean(element?.closest?.(".cm-lp-include-open"));
  }
}

registerWidget("include", (context) => new IncludeWidget(context));

// The argument of the last \command{...} in the source, without its braces, or null.
function lastArgument(source, command) {
  let found = null;
  const pattern = new RegExp(`\\\\${command}\\s*\\{`, "g");
  let match;
  while ((match = pattern.exec(source))) {
    const start = match.index + match[0].length;
    const close = closingBrace(source, start);
    if (close !== -1) found = source.slice(start, close);
  }
  return found;
}

// The index of the brace that closes one opened just before `from`, or -1.
function closingBrace(source, from) {
  let depth = 1;
  for (let i = from; i < source.length; i++) {
    const character = source[i];
    if (character === "\\") i++;
    else if (character === "{") depth++;
    else if (character === "}" && --depth === 0) return i;
  }
  return -1;
}

// The text without every \command{...} of this name, braces and contents included.
function withoutCommand(text, command) {
  const pattern = new RegExp(`\\\\${command}\\s*\\{`, "g");
  let out = "";
  let kept = 0;
  let match;
  while ((match = pattern.exec(text))) {
    const close = closingBrace(text, match.index + match[0].length);
    if (close === -1) break;
    out += text.slice(kept, match.index);
    kept = close + 1;
    pattern.lastIndex = kept;
  }
  return out + text.slice(kept);
}

// Today's date in the reader's locale, in long form.
function longToday() {
  return new Date().toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
}

// Turns the source of a \title, \author or \date into plain text for the title block.
function cleanTitle(text) {
  let out = withoutCommand(text, "thanks");
  out = out.replace(/\\\\\*?(?:\[[^\]]*\])?/g, "\n");
  out = out.replace(/\\and(?![a-zA-Z])/g, ", ");
  out = out.replace(/\\today(?![a-zA-Z])/g, longToday());

  // Formatting wrappers can nest, so they are unwrapped until none is left.
  const formatting = /\\(?:textbf|textit|emph|textsc|textrm|textsf|texttt|textsl|underline|mbox)\s*\{([^{}]*)\}/g;
  let previous;
  do {
    previous = out;
    out = out.replace(formatting, "$1");
  } while (out !== previous);

  out = out.replace(/\\([&%$#_{}])/g, "$1");
  out = out.replace(/\\[a-zA-Z]+\*?/g, "");
  out = out.replace(/[{}]/g, "");
  out = out.replace(/~/g, " ");
  return out
    .split("\n")
    .map((line) => line.replace(/\s+/g, " ").trim())
    .join("\n")
    .trim();
}

// The title block that \maketitle draws, from the document's \title, \author and \date.
class TitleWidget extends WidgetType {
  constructor(context) {
    super();
    const source = stripComments(context.source);
    this.title = cleanTitle(lastArgument(source, "title") ?? "");
    this.author = cleanTitle(lastArgument(source, "author") ?? "");
    // LaTeX prints today's date when there is no \date; an empty \date prints nothing.
    const date = lastArgument(source, "date");
    this.date = date === null ? longToday() : cleanTitle(date);
  }

  eq(other) {
    return (
      other instanceof TitleWidget &&
      other.title === this.title &&
      other.author === this.author &&
      other.date === this.date
    );
  }

  toDOM() {
    const block = document.createElement("div");
    block.className = "cm-lp-title";

    const main = document.createElement("div");
    main.className = "cm-lp-title-main";
    if (this.title) {
      main.textContent = this.title;
    } else {
      main.textContent = "Untitled";
      main.classList.add("cm-lp-title-muted");
    }
    block.appendChild(main);

    if (this.author) {
      const author = document.createElement("div");
      author.className = "cm-lp-title-author";
      author.textContent = this.author;
      block.appendChild(author);
    }

    if (this.date) {
      const date = document.createElement("div");
      date.className = "cm-lp-title-date";
      date.textContent = this.date;
      block.appendChild(date);
    }

    return block;
  }

  // A click reveals the source.
  ignoreEvent() {
    return false;
  }
}

registerWidget("title", (context) => new TitleWidget(context));
