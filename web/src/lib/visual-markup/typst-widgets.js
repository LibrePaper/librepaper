// The widgets for Typst-only tokens: references and citations, labels, includes,
// the title block and colour swatches. index.js imports this file, so nothing here
// reads an index.js export at load time; the factories and toDOM are where they are used.
import { WidgetType } from "@codemirror/view";
import { chipWidget, ElementWidget, factsOf, includeOpener, nameList } from "./index.js";
import { registerWidget, revealOnMousedown, textOf } from "./markup.ts";
import { TYPST_FUNCTIONS } from "./languages/typst/catalog.ts";
import "./typst.css";

// ---- Text helpers ----------------------------------------------------------

// The trimmed source text of a range, or "" when the range is missing.
function rangeText(source, range) {
  return range ? textOf(source, range).trim() : "";
}

// A string literal's contents, or the text itself when it is not quoted.
function unquote(text) {
  const match = /^"((?:[^"\\]|\\.)*)"$/s.exec(text.trim());
  return match ? match[1].replace(/\\(.)/g, "$1") : text.trim();
}

// Typst markup as plain text: escapes and emphasis markers go, the words stay.
function plainMarkup(text) {
  return text.replace(/\\(.)/g, "$1").replace(/[*_]/g, "").replace(/\s+/g, " ").trim();
}

// A named argument's value as text: a string, or the inside of a content block.
function valueText(value) {
  if (!value) return "";
  if (value.startsWith('"')) return unquote(value);
  if (value.startsWith("[") && value.endsWith("]")) return plainMarkup(value.slice(1, -1));
  return "";
}

// An array argument such as ("A", "B") as its texts; a single value as a list of one.
function valueList(value) {
  if (value?.startsWith("(") && value.endsWith(")")) {
    return splitTopLevel(value.slice(1, -1)).map(valueText).filter(Boolean);
  }
  const one = valueText(value);
  return one ? [one] : [];
}

// Splits the inside of a call at its top-level commas, leaving strings and brackets alone.
function splitTopLevel(text) {
  const parts = [];
  let depth = 0;
  let quoted = false;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const character = text[i];
    if (quoted) {
      if (character === "\\") i++;
      else if (character === '"') quoted = false;
    } else if (character === '"') {
      quoted = true;
    } else if (character === "\\") {
      i++;
    } else if ("([{".includes(character)) {
      depth++;
    } else if (")]}".includes(character)) {
      depth--;
    } else if (character === "," && depth === 0) {
      parts.push(text.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(text.slice(start));
  return parts.map((part) => part.trim()).filter(Boolean);
}

// The text inside a call's parentheses, from just after the opening one to its close.
function callInside(text, start) {
  let depth = 0;
  let quoted = false;
  for (let i = start; i < text.length; i++) {
    const character = text[i];
    if (quoted) {
      if (character === "\\") i++;
      else if (character === '"') quoted = false;
    } else if (character === '"') {
      quoted = true;
    } else if (character === "\\") {
      i++;
    } else if ("([{".includes(character)) {
      depth++;
    } else if (")]}".includes(character)) {
      if (depth === 0) return text.slice(start, i);
      depth--;
    }
  }
  return text.slice(start);
}

// The named arguments of a call's inside, as raw value text by name.
function namedArgs(inside) {
  const named = {};
  for (const part of splitTopLevel(inside)) {
    const match = /^([a-z-]+)\s*:\s*([\s\S]*)$/.exec(part);
    if (match) named[match[1]] = match[2].trim();
  }
  return named;
}

// The named arguments of the last #set document(...) in the text, or none.
function documentArgs(text) {
  let named = {};
  for (const match of text.matchAll(/#set\s+document\s*\(/g)) {
    named = namedArgs(callInside(text, match.index + match[0].length));
  }
  return named;
}

// The key of a label or reference: the lexer's meta.key, or the <...> argument of a call.
function keyOf(token, source) {
  if (token.meta?.key) return token.meta.key;
  return rangeText(source, token.args?.[0]).replace(/^<|>$/g, "").trim();
}

// ---- References and citations ----------------------------------------------

// The words a reference shows before its number, by the kind of label it names.
const TYPST_WORDS = { section: "Section", figure: "Figure", table: "Table", equation: "Equation", listing: "Listing" };

// A label's reference text. A supplement replaces the word, as in Typst.
function labelWords(found, supplement) {
  const word = supplement || TYPST_WORDS[found.kind] || "";
  const number = found.kind === "equation" ? `(${found.number})` : String(found.number);
  return word ? `${word} ${number}` : number;
}

// Joins arrays of content parts with a separator between them.
function joined(items, separator) {
  return items.flatMap((item, index) => (index ? [separator, ...item] : item));
}

// The content of a citation. The form argument picks the reading: "prose" writes
// the author with the year in brackets, "year" and "author" write one part only.
function citeContent(keys, form, facts) {
  const numeric = facts.citeStyle === "numeric";
  const items = keys.map((key) => {
    const entry = facts.citations.get(key);
    if (!entry) return [{ text: `${key}?`, muted: true }];
    const names = nameList(entry.authors ?? []) || entry.title || key;
    const year = entry.year || "n.d.";
    const number = String(entry.number);
    switch (form) {
      case "prose":
        return [numeric ? `${names} [${number}]` : `${names} (${year})`];
      case "year":
        return [year];
      case "author":
        return [names];
      default:
        return [numeric ? number : `${names} ${year}`];
    }
  });
  // Numeric citations in the default form share one pair of brackets.
  if (numeric && form !== "prose" && form !== "year" && form !== "author") {
    return ["[", ...joined(items, ", "), "]"];
  }
  return joined(items, "; ");
}

// #ref, #cite and @key. A label wins for references; a citation is the fallback,
// and a #cite only looks at citations. Anything else is shown as its key, muted.
registerWidget("typref", ({ token, source, state }) => {
  const facts = factsOf(state);
  const key = keyOf(token, source);
  const isCite = token.name === "cite";
  const found = isCite ? undefined : facts.labels.get(key);
  const supplement = token.options?.[0]
    ? plainMarkup(textOf(source, token.options[0]))
    : valueText(rangeText(source, token.named?.supplement));

  if (found) {
    return new ElementWidget({ className: "cm-lp-ref", content: [labelWords(found, supplement)], title: key });
  }
  if (facts.citations.has(key)) {
    const form = unquote(rangeText(source, token.named?.form)) || "normal";
    return new ElementWidget({ className: "cm-lp-cite", content: citeContent([key], form, facts), title: key });
  }
  const shown = token.meta?.key ? `@${key}` : key;
  return new ElementWidget({
    className: isCite ? "cm-lp-cite" : "cm-lp-ref",
    content: [{ text: `${shown}?`, muted: true }],
    title: key,
  });
});

// ---- Labels ----------------------------------------------------------------

// A <label> in the text, shown as the tag a LaTeX label gets.
registerWidget("typlabel", ({ token, source }) => {
  return new ElementWidget({ className: "cm-lp-label", content: [keyOf(token, source)], title: "Label" });
});

// ---- Includes --------------------------------------------------------------

// A #include or #import of a file, shown as a chip with the file's name and an Open button.
class TypIncludeWidget extends WidgetType {
  constructor({ token, source, state }) {
    super();
    this.name = unquote(token.meta?.path ?? rangeText(source, token.args?.[0]));
    this.opener = state.facet(includeOpener);
  }

  eq(other) {
    return other instanceof TypIncludeWidget && other.name === this.name && other.opener === this.opener;
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

registerWidget("typinclude", (context) => new TypIncludeWidget(context));

// ---- Title -----------------------------------------------------------------

// The title block: the title, the authors and the date, in the classes the LaTeX
// title block uses.
class TypTitleWidget extends WidgetType {
  constructor({ title, authors, date }) {
    super();
    this.title = title;
    this.authors = authors;
    this.date = date;
    this.signature = JSON.stringify([title, authors, date]);
  }

  eq(other) {
    return other instanceof TypTitleWidget && other.signature === this.signature;
  }

  toDOM(view) {
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

    if (this.authors.length) {
      const author = document.createElement("div");
      author.className = "cm-lp-title-author";
      author.textContent = this.authors.join(", ");
      block.appendChild(author);
    }

    if (this.date) {
      const date = document.createElement("div");
      date.className = "cm-lp-title-date";
      date.textContent = this.date;
      block.appendChild(date);
    }

    revealOnMousedown(view, block);
    return block;
  }

  ignoreEvent() {
    return true;
  }
}

// A #title[...] call, or a #set document(...) rule. The author and date come from
// the last #set document(...) before a #title call.
registerWidget("typtitle", ({ token, source }) => {
  const isSet = token.name === "set";
  const named = isSet ? documentArgs(textOf(source, token)) : documentArgs(source.slice(0, token.from));
  const bodyText = token.body
    ? plainMarkup(textOf(source, token.body))
    : plainMarkup(rangeText(source, token.args?.[0]));
  const title = isSet ? valueText(named.title) : bodyText;
  return new TypTitleWidget({
    title,
    authors: valueList(named.author),
    date: valueText(named.date),
  });
});

// ---- Colour swatches -------------------------------------------------------

const HEX_DIGITS = /^[0-9a-f]{3,8}$/i;
const CSS_NUMBER = /^[\d.]+(?:%|deg)?$/;

// The CSS colour of a colour call, or null when the call's form is not one this reads.
// Only validated values reach the page, and CSS.supports checks the result.
function cssColour(name, values) {
  let css = null;
  if (name === "rgb" && values.length === 1) {
    const hex = values[0].replace(/^#/, "");
    css = HEX_DIGITS.test(hex) ? `#${hex}` : null;
  } else if (name === "rgb" && values.length >= 3 && values.slice(0, 3).every((value) => /^\d+$/.test(value))) {
    css = `${name}(${values.slice(0, 3).join(", ")})`;
  } else if (name === "hsl" && values.length === 3 && values.every((value) => CSS_NUMBER.test(value))) {
    css = `${name}(${values.join(", ")})`;
  }
  return css && typeof CSS !== "undefined" && CSS.supports("color", css) ? css : null;
}

// A colour call drawn as a small square in the colour, followed by the call's text.
class SwatchWidget extends WidgetType {
  constructor({ token, source }) {
    super();
    const raw = (token.args ?? []).map((range) => rangeText(source, range));
    this.text = `${token.name}(${raw.join(", ")})`;
    this.color = cssColour(token.name, raw.map(unquote));
  }

  eq(other) {
    return other instanceof SwatchWidget && other.text === this.text && other.color === this.color;
  }

  toDOM(view) {
    const element = document.createElement("span");
    element.className = "cm-lp-swatch";
    element.title = this.text;

    const chip = document.createElement("span");
    chip.className = "cm-lp-swatch-chip";
    if (this.color) {
      chip.style.background = this.color;
    } else {
      chip.classList.add("cm-lp-swatch-unknown");
    }

    element.append(chip, this.text);
    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

registerWidget("swatch", (context) => new SwatchWidget(context));

// A chip from the Typst catalog (page break, outline, bibliography, placeholder
// text, drawings), with its first argument when the catalog asks for it.
registerWidget("typchip", ({ token, source }) => {
  const name = token.name ?? "";
  const render = (TYPST_FUNCTIONS.get(name) ?? TYPST_FUNCTIONS.get(name.split(".")[0]))?.render;
  if (!render || render.kind !== "chip") return null;
  let text = render.text;
  const first = token.args?.[0] ? textOf(source, token.args[0]).replace(/^"|"$/g, "").trim() : "";
  if (render.argument && first) text += `: ${first}`;
  return chipWidget(text, render.block);
});
