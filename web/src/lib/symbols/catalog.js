// The picker's view of the catalog: the symbols and the math templates, with
// the recent and favorite groups in front. Pairs and templates reuse the math
// templates, so their syntax is the one already checked against the Typst engine.
import { CATEGORIES, SYMBOLS, symbolInsertion } from "./index.js";
import { MATH_TEMPLATES, templateInsertion } from "./templates.js";

export { symbolSpellings as entrySpellings } from "./index.js";

export const PICKER_CATEGORIES = [
  { id: "recent", label: "Recent" },
  { id: "favorites", label: "Favorites" },
  ...CATEGORIES.flatMap(category => (category.id === "delimiters" ? [category, { id: "templates", label: "Templates" }] : [category])),
];

// Each template tile names the template it writes, and its spellings are the
// template's own output for placeholder values.
const TEMPLATE_TILES = [
  { template: "fraction", name: "Fraction", aliases: ["frac", "divide", "over"], preview: "\\frac{a}{b}", typst: "frac(a, b)" },
  { template: "root", name: "Square root", aliases: ["sqrt", "radical"], preview: "\\sqrt{x}", typst: "sqrt(x)" },
  { template: "superscript", name: "Superscript", aliases: ["power", "exponent"], preview: "x^{2}", typst: "x^(2)" },
  { template: "subscript", name: "Subscript", aliases: ["sub", "index"], preview: "x_{i}", typst: "x_(i)" },
  { template: "sum", name: "Sum with limits", aliases: ["summation", "sigma"], preview: "\\sum_{i=1}^{n} x_i", typst: "sum_(i=1)^(n) x_i" },
  { template: "integral", name: "Integral with limits", aliases: ["int", "integration"], preview: "\\int_a^b f \\, dx", typst: "integral_(a)^(b) f dif x" },
  { template: "matrix", name: "Matrix", aliases: ["pmatrix", "array"], preview: "\\begin{pmatrix} a & b \\end{pmatrix}", typst: "mat(a, b; c, d)" },
];

export const TEMPLATE_ENTRIES = TEMPLATE_TILES.map(tile => ({
  id: `template-${tile.template}`,
  template: tile.template,
  glyph: "",
  name: tile.name,
  aliases: tile.aliases,
  category: "templates",
  kind: "template",
  latex: tile.preview,
  typst: tile.typst,
  preview: tile.preview,
}));

export const PICKER_SYMBOLS = [...SYMBOLS, ...TEMPLATE_ENTRIES];

/**
 * Templates need a math-capable format, so they are hidden in HTML.
 * @param {object} entry
 * @param {string} format
 * @returns {boolean}
 */
export function entryAvailable(entry, format) {
  return !(entry.kind === "template" && format === "html");
}

/**
 * Turns a click on a picker entry into the text to insert at the caret.
 * @param {object} entry one of PICKER_SYMBOLS
 * @param {object} context the editor's insert context (see insert.js)
 * @returns {{text: string, additionalEdits: object[], notes: string[], snippet: boolean, selection?: {anchor: number, head: number}}}
 */
export function entryInsertion(entry, context = {}) {
  const selected = context.selection?.text ?? "";

  if (entry.kind === "template") {
    if (context.format === "html") {
      return { text: "", additionalEdits: [], notes: ["Templates need LaTeX, Typst, Markdown or Quarto."], snippet: false };
    }
    const template = MATH_TEMPLATES.find(item => item.id === entry.template);
    const values = { style: "inline" };
    if (selected && entry.template !== "matrix") values[template.slots[0].id] = selected;
    return templateInsertion(template, values, context);
  }

  if (entry.kind === "pair") {
    if (context.format === "html") {
      const [open, close] = entry.glyphs;
      return {
        text: open + selected + close,
        additionalEdits: [],
        notes: [],
        snippet: false,
        selection: { anchor: open.length, head: open.length + selected.length },
      };
    }
    const brackets = MATH_TEMPLATES.find(item => item.id === "brackets");
    return templateInsertion(brackets, { kind: entry.bracket, style: "inline", ...(selected ? { body: selected } : {}) }, context);
  }

  return symbolInsertion(entry, context);
}
