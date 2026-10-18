// Math structures for the math palette. Each template writes a LaTeX and a
// Typst form from named slots. An empty slot becomes a snippet field, so the
// caret lands in the first blank and Tab moves through the rest.
import { insertSyntaxContext, uniqueLabel } from "../insert.js";
import { key as latexKey, latexPackageEdits } from "../insert-latex.js";
import { identifier as typstIdentifier } from "../insert-typst.js";
import { separated } from "./index.js";

const slot = (id, label, placeholder, extra = {}) => ({ id, label, placeholder, ...extra });

// The rows of a grid inside one LaTeX environment. A label line, when there is
// one, goes first inside it.
const rowsIn = (environment, rows, labelLine = "") => `\\begin{${environment}}\n${labelLine}${rows}\n\\end{${environment}}`;

const BRACKETS_LATEX = {
  parentheses: ["(", ")"],
  square: ["[", "]"],
  braces: ["\\{", "\\}"],
  angle: ["\\langle", "\\rangle"],
  bars: ["\\lvert", "\\rvert"],
  norm: ["\\lVert", "\\rVert"],
  floor: ["\\lfloor", "\\rfloor"],
  ceil: ["\\lceil", "\\rceil"],
};

const MATRIX_ENV = { parentheses: "pmatrix", square: "bmatrix", braces: "Bmatrix", bars: "vmatrix", "double-bars": "Vmatrix", none: "matrix" };
// Typst takes the delimiter as a string of exactly one character, so the
// double bar is the single character U+2016. "none" is the Typst none value.
const MATRIX_DELIM = { parentheses: '"("', square: '"["', braces: '"{"', bars: '"|"', "double-bars": '"‖"', none: "#none" };

// A base of more than one token is grouped before a script is attached, so
// the script applies to all of it and a base that already carries a script
// does not get a second one on the same side.
const grouped = (base, typst) => {
  const text = String(base);
  const single = /^(?:.|\\[A-Za-z]+|\{[^{}]*\}|\([^()]*\)|#\{\d+:[^{}]*\})$/u.test(text);
  if (single) return text;
  return typst ? `(${text})` : `{${text}}`;
};

export const MATH_TEMPLATES = [
  {
    id: "fraction", label: "Fraction", preview: "\\frac{a}{b}",
    slots: [slot("num", "Numerator", "a"), slot("den", "Denominator", "b")],
    latex: v => `\\frac{${v.num}}{${v.den}}`,
    typst: v => `frac(${v.num}, ${v.den})`,
  },
  {
    id: "root", label: "Root", preview: "\\sqrt[n]{x}",
    slots: [slot("radicand", "Radicand", "x"), slot("index", "Index", "n", { optional: true })],
    latex: v => (v.index ? `\\sqrt[${v.index}]{${v.radicand}}` : `\\sqrt{${v.radicand}}`),
    typst: v => (v.index ? `root(${v.index}, ${v.radicand})` : `sqrt(${v.radicand})`),
  },
  {
    id: "subscript", label: "Subscript", preview: "x_{i}",
    slots: [slot("base", "Base", "x"), slot("sub", "Subscript", "i")],
    latex: v => `${grouped(v.base, false)}_{${v.sub}}`,
    typst: v => `${grouped(v.base, true)}_(${v.sub})`,
  },
  {
    id: "superscript", label: "Superscript", preview: "x^{2}",
    slots: [slot("base", "Base", "x"), slot("sup", "Superscript", "2")],
    latex: v => `${grouped(v.base, false)}^{${v.sup}}`,
    typst: v => `${grouped(v.base, true)}^(${v.sup})`,
  },
  {
    id: "subsup", label: "Subscript and superscript", preview: "x_{i}^{2}",
    slots: [slot("base", "Base", "x"), slot("sub", "Subscript", "i"), slot("sup", "Superscript", "2")],
    latex: v => `${grouped(v.base, false)}_{${v.sub}}^{${v.sup}}`,
    typst: v => `${grouped(v.base, true)}_(${v.sub})^(${v.sup})`,
  },
  {
    id: "sum", label: "Sum", preview: "\\sum_{i=1}^{n} x_i",
    slots: [slot("from", "From", "i=1"), slot("to", "To", "n"), slot("body", "Body", "x_i")],
    latex: v => `\\sum_{${v.from}}^{${v.to}} ${v.body}`,
    typst: v => `sum_(${v.from})^(${v.to}) ${v.body}`,
  },
  {
    id: "product", label: "Product", preview: "\\prod_{i=1}^{n} x_i",
    slots: [slot("from", "From", "i=1"), slot("to", "To", "n"), slot("body", "Body", "x_i")],
    latex: v => `\\prod_{${v.from}}^{${v.to}} ${v.body}`,
    typst: v => `product_(${v.from})^(${v.to}) ${v.body}`,
  },
  {
    id: "integral", label: "Integral", preview: "\\int_a^b f \\, dx",
    slots: [
      slot("from", "Lower limit", "a"), slot("to", "Upper limit", "b"), slot("body", "Integrand", "f"),
      slot("variable", "Variable", "x", { default: "x" }),
    ],
    latex: v => `\\int_{${v.from}}^{${v.to}} ${v.body} \\, d${v.variable}`,
    typst: v => `integral_(${v.from})^(${v.to}) ${v.body} dif ${v.variable}`,
  },
  {
    id: "limit", label: "Limit", preview: "\\lim_{x \\to a} f",
    slots: [slot("variable", "Variable", "x"), slot("to", "Approaches", "a"), slot("body", "Body", "f")],
    latex: v => `\\lim_{${v.variable} \\to ${v.to}} ${v.body}`,
    typst: v => `lim_(${v.variable} -> ${v.to}) ${v.body}`,
  },
  {
    id: "binomial", label: "Binomial", preview: "\\binom{n}{k}", package: "amsmath",
    slots: [slot("top", "Top", "n"), slot("bottom", "Bottom", "k")],
    latex: v => `\\binom{${v.top}}{${v.bottom}}`,
    typst: v => `binom(${v.top}, ${v.bottom})`,
  },
  {
    id: "overbrace", label: "Overbrace", preview: "\\overbrace{x}^{n}",
    slots: [slot("body", "Body", "x"), slot("note", "Label", "label")],
    latex: v => `\\overbrace{${v.body}}^{${v.note}}`,
    typst: v => `overbrace(${v.body}, ${v.note})`,
  },
  {
    id: "underbrace", label: "Underbrace", preview: "\\underbrace{x}_{n}",
    slots: [slot("body", "Body", "x"), slot("note", "Label", "label")],
    latex: v => `\\underbrace{${v.body}}_{${v.note}}`,
    typst: v => `underbrace(${v.body}, ${v.note})`,
  },
  {
    id: "brackets", label: "Brackets", preview: "\\left( x \\right)",
    // The bar and norm delimiters are amsmath commands.
    package: v => (v.kind === "bars" || v.kind === "norm" ? "amsmath" : null),
    slots: [slot("body", "Body", "x")],
    latex: v => {
      const [open, close] = BRACKETS_LATEX[v.kind ?? "parentheses"] ?? BRACKETS_LATEX.parentheses;
      return `\\left${open} ${v.body} \\right${close}`;
    },
    typst: v => {
      switch (v.kind) {
        case "square": return `[${v.body}]`;
        case "braces": return `{${v.body}}`;
        case "angle": return `chevron.l ${v.body} chevron.r`;
        case "bars": return `abs(${v.body})`;
        case "norm": return `norm(${v.body})`;
        case "floor": return `floor(${v.body})`;
        case "ceil": return `ceil(${v.body})`;
        default: return `(${v.body})`;
      }
    },
  },
  {
    // In LaTeX display, templateInsertion sets the environment, so the star
    // and the number live there. Everywhere else, and in the preview, it is
    // aligned, which KaTeX and LaTeX accept inline and in display.
    id: "aligned", label: "Aligned equations", preview: "\\begin{aligned} x &= y \\end{aligned}", package: "amsmath",
    grid: true, selfDisplay: true, displayEnvironment: "align",
    slots: [slot("rows", "Equations", "x")],
    latex: v => rowsIn(v.environment ?? "aligned", v.rows.map(row => `${row.left} &= ${row.right}`).join(" \\\\\n"), v.labelLine),
    typst: v => v.rows.map(row => `${row.left} &= ${row.right}`).join(" \\ "),
  },
  {
    // Same rules as aligned, with one value per row and gather for display.
    id: "gathered", label: "Gathered equations", preview: "\\begin{gathered} x = y \\\\ z = w \\end{gathered}", package: "amsmath",
    grid: true, selfDisplay: true, displayEnvironment: "gather",
    slots: [slot("rows", "Equations", "x")],
    latex: v => rowsIn(v.environment ?? "gathered", v.rows.map(row => row.value).join(" \\\\\n"), v.labelLine),
    typst: v => v.rows.map(row => row.value).join(" \\ "),
  },
  {
    id: "cases", label: "Cases", preview: "\\begin{cases} 1 & x > 0 \\end{cases}", package: "amsmath",
    grid: true,
    slots: [slot("rows", "Cases", "1")],
    latex: v => `\\begin{cases}\n${v.rows.map(row => `${row.value} & ${row.condition}`).join(" \\\\\n")}\n\\end{cases}`,
    typst: v => `cases(${v.rows.map(row => `${row.value} "if" ${row.condition}`).join(", ")})`,
  },
  {
    id: "matrix", label: "Matrix", preview: "\\begin{pmatrix} a & b \\end{pmatrix}", package: "amsmath",
    grid: true,
    slots: [slot("cells", "Cells", "0")],
    latex: v => {
      const env = MATRIX_ENV[v.brackets ?? "parentheses"] ?? "pmatrix";
      const rows = v.cells.map(row => row.join(" & ")).join(" \\\\\n");
      return `\\begin{${env}}\n${rows}\n\\end{${env}}`;
    },
    typst: v => {
      const delim = MATRIX_DELIM[v.brackets ?? "parentheses"] ?? '"("';
      return `mat(delim: ${delim}, ${v.cells.map(row => row.join(", ")).join("; ")})`;
    },
  },
];

// Grid slots start with two blank rows, or a 2 by 2 blank matrix, when the
// caller gives none.
const GRID_DEFAULTS = {
  aligned: () => [{ left: "", right: "" }, { left: "", right: "" }],
  gathered: () => [{ value: "" }, { value: "" }],
  cases: () => [{ value: "", condition: "" }, { value: "", condition: "" }],
  matrix: values => Array.from({ length: Number(values.rows) || 2 }, () => Array.from({ length: Number(values.columns) || 2 }, () => "")),
};

/**
 * @param {object} template one entry of MATH_TEMPLATES
 * @param {object} [values] slot values, plus the template's options
 * @param {object} [context] the editor's insert context (see insert.js)
 * @returns {{text: string, additionalEdits: object[], notes: string[], snippet: boolean}}
 */
export function templateInsertion(template, values = {}, context = {}) {
  const format = context.format;
  if (format === "html") return { text: "", notes: ["Math templates need a math-capable format."], additionalEdits: [], snippet: false };

  const inMath = insertSyntaxContext(context) === "math";
  const inline = values.style === "inline";
  // Numbers apply to a display outside math. Markdown has no numbering, so it
  // ignores them.
  const numbered = !inMath && !inline && values.numbered === true;
  const label = String(values.label ?? "").trim();
  const latexDisplay = format === "latex" && !inMath && !inline;
  const notes = [], additionalEdits = [];
  let fields = 0;
  // Fields number from left to right across the whole template, including
  // the cells of a grid, so the caret visits them in reading order.
  const fill = (value, placeholder) => {
    if (Array.isArray(value)) return value.map(item => fill(item, placeholder));
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, fill(item, placeholder)]));
    const text = value == null ? "" : String(value);
    if (text !== "") return text;
    fields += 1;
    return `#{${fields}:${placeholder}}`;
  };

  const v = { ...values, inMath, format, inline };
  for (const item of template.slots) {
    const raw = values[item.id];
    if (raw === undefined && item.default !== undefined) { v[item.id] = item.default; continue; }
    if (item.optional && (raw == null || raw === "")) { v[item.id] = ""; continue; }
    const value = raw ?? GRID_DEFAULTS[template.id]?.(values);
    v[item.id] = fill(value, item.placeholder ?? item.label);
  }

  // The label is literal text, so it is never a snippet field. A LaTeX label
  // is only written with a number, because an unnumbered one has nothing to
  // refer to.
  const labelLine = format === "latex" && numbered && label ? `\\label{${latexKey(label)}}\n` : "";
  const typstLabel = numbered && label ? ` <${typstIdentifier(label)}>` : "";
  v.labelLine = labelLine;
  // A self-displaying template in LaTeX display brings its own environment,
  // starred when unnumbered. The label sits inside it.
  if (latexDisplay && template.selfDisplay) v.environment = `${template.displayEnvironment}${numbered ? "" : "*"}`;

  let body = format === "typst" ? template.typst(v) : template.latex(v);
  // A numbered LaTeX display that is not self-displaying gets an equation environment.
  if (latexDisplay && numbered && !template.selfDisplay) body = rowsIn("equation", body, labelLine);
  let text = body;
  // The LaTeX display environment is already display math, so it is not wrapped again.
  const latexEnvironment = latexDisplay && (template.selfDisplay || numbered);
  if (!inMath) {
    if (format === "typst") {
      if (inline) text = `$${body}$`;
      else if (numbered) text = `#math.equation(block: true, numbering: "(1)")[$ ${body} $]${typstLabel}`;
      else text = `$ ${body} $`;
    } else if (format === "markdown" || format === "quarto") {
      if (inline) text = `$${body}$`;
      // Quarto numbers only a labelled equation, so an unnamed one gets a generated label.
      else if (numbered && format === "quarto") text = `$$${body}$$ {#${uniqueLabel(label || "equation", context, "eq")}}`;
      else text = `$$${body}$$`;
    } else if (!latexEnvironment) text = inline ? `$${body}$` : `\\[\n${body}\n\\]`;
  }
  // Add a space where the text would run into the characters beside it.
  const from = context.selection?.from, to = context.selection?.to;
  const before = typeof context.text === "string" ? context.text[from - 1] ?? "" : "";
  const after = typeof context.text === "string" ? context.text[to] ?? "" : "";
  // The boundary is judged on the text as it will read once the fields are
  // filled with their placeholders, and the marked text is padded to match.
  const expanded = text.replace(/#\{\d+:([^{}]*)\}/g, "$1");
  const padded = separated(expanded, format, before, after);
  text = (padded.startsWith(" ") && !expanded.startsWith(" ") ? " " : "") + text + (padded.endsWith(" ") && !expanded.endsWith(" ") ? " " : "");
  const pkg = typeof template.package === "function" ? template.package(v) : template.package;
  if (format === "latex" && pkg) additionalEdits.push(...latexPackageEdits(pkg, context, notes));
  return { text, additionalEdits, notes, snippet: fields > 0 };
}
