/* Shared Insert menu vocabulary and format-aware source builders.
 * Builders are deliberately editor-agnostic: selection offsets are relative to
 * the returned text, so the caller can add its replacement offset.
 */
const FORMATS = new Set(["latex", "typst", "markdown", "quarto"]);
const string = (x, fallback = "") => x == null ? fallback : String(x);
const integer = (x, fallback, min = 1) => Math.max(min, Math.floor(Number.isFinite(+x) ? +x : fallback));
const esc = (x, format) => {
  x = string(x);
  if (format === "latex") return x.replace(/([\\{}%#$&_])/g, "\\$1").replace(/~/g, "\\textasciitilde{}").replace(/\^/g, "\\textasciicircum{}");
  if (format === "typst") return x.replace(/([\\#])/g, "\\$1");
  return x.replace(/([\\`*_{}\[\]()#+.!|>~-])/g, "\\$1");
};
const label = (x) => string(x, "label").trim().replace(/[^\p{L}\p{N}:._-]+/gu, "-").replace(/^-+|-+$/g, "") || "label";
const ctx = (context = {}) => ({ format: FORMATS.has(context.format) ? context.format : "markdown", text: string(context.text || context.mainText), selection: context.selection || { from: 0, to: 0, text: "" }, ...context });
const action = (id, label, group, dialog) => dialog ? { id, label, group, dialog } : { id, label, group };

export const INSERT_ACTIONS = [
  action("heading", "Heading / section", "Structure", "heading"), action("abstract", "Abstract", "Structure"), action("appendix", "Appendix", "Structure"), action("toc", "Table of contents", "Structure"),
  action("figure", "Image / figure", "Figures and tables", "figure"), action("table", "Table", "Figures and tables", "table"),
  action("citation", "Citation", "References", "citation"), action("bibliography", "Bibliography", "References", "bibliography"), action("cross-reference", "Cross-reference", "References", "cross-reference"), action("label", "Label / anchor", "References", "label"),
  action("inline-math", "Inline math", "Math"), action("display-math", "Displayed equation", "Math", "math"), action("aligned-math", "Aligned equations", "Math", "math"), action("gather-math", "Gathered equations", "Math", "math"), action("cases", "Cases", "Math", "matrix"), action("matrix", "Matrix", "Math", "matrix"),
  action("bulleted-list", "Bulleted list", "Lists"), action("numbered-list", "Numbered list", "Lists"), action("description-list", "Description list", "Lists"),
  action("quote", "Block quotation", "Text blocks"), action("quotation", "Quotation", "Text blocks"), action("code-block", "Code block", "Text blocks", "code"), action("footnote", "Footnote", "Text blocks"), action("link", "Link", "Text blocks", "link"),
  ...["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"].map((x) => action(x, x[0].toUpperCase() + x.slice(1), "Scholarly blocks", "environment")),
  action("page-break", "Page break", "Layout"), action("horizontal-rule", "Horizontal rule", "Layout"), action("columns", "Columns", "Layout", "columns"), action("custom-environment", "Custom environment", "Advanced", "environment"),
];

const unsupported = (format, id) => {
  if (format === "markdown" && ["abstract", "appendix", "toc", "aligned-math", "gather-math", "cases", "matrix", "theorem", "lemma", "proposition", "definition", "proof", "example", "remark", "page-break", "columns", "bibliography"].includes(id)) return "This insertion needs Quarto or a typesetting format.";
  return null;
};
export function insertionAvailability(id, context = {}) {
  const c = ctx(context), known = INSERT_ACTIONS.some((x) => x.id === id);
  if (!known) return { enabled: false, reason: "Unknown insertion" };
  const reason = unsupported(c.format, id);
  if (reason) return { enabled: false, reason };
  if (id === "citation" && !(c.bibliography?.length || c.files?.some((f) => /\.(bib|json|yaml|yml)$/.test(f.path || "")))) return { enabled: false, reason: "Add a bibliography file to insert citations." };
  if (id === "cross-reference" && !gatherInsertTargets(c).length) return { enabled: false, reason: "No headings, figures, tables, equations, or labels found." };
  return { enabled: true };
}
const block = (text, selection, before, after = "") => ({ text: before + text + after, selection: { anchor: before.length, head: before.length + text.length } });
const targetText = (c) => string(c.selection?.text || "");
const rows = (c, options, format) => {
  const n = integer(options.rows, 3), m = integer(options.columns, 3), header = options.header !== false;
  if (format === "latex") return `\\begin{tabular}{${"c".repeat(m)}}\n${Array.from({length:n}, (_, r) => Array.from({length:m}, (_, j) => r === 0 && header ? `Header ${j + 1}` : `Cell ${r + 1},${j + 1}`).join(" & ") + " \\\\").join("\n")}\n\\end{tabular}`;
  if (format === "typst") return `#table(\n  columns: ${m},\n  ${Array.from({length:n*m}, (_, i) => `[${i < m && header ? `Header ${i+1}` : `Cell ${Math.floor(i/m)+1},${i%m+1}`}]`).join(",\n  ")}\n)`;
  return [Array.from({length:m}, (_, j) => header ? `Header ${j+1}` : `Cell 1,${j+1}`).join(" | "), Array.from({length:m}, () => "---").join(" | "), ...Array.from({length:Math.max(0,n-1)}, (_, r) => Array.from({length:m}, (_, j) => `Cell ${r+2},${j+1}`).join(" | "))].join("\n");
};
const math = (id, c, options) => { const f = c.format, body = targetText(c) || "x = y"; if (id === "inline-math") return `$${body}$`; if (f === "latex") { const env = id === "aligned-math" ? "align" : id === "gather-math" ? "gather" : id === "cases" ? "cases" : id === "matrix" ? "pmatrix" : "equation"; return `\\begin{${env}}\n${id === "cases" ? "condition & value" : body}\n\\end{${env}}`; } if (f === "typst") { if (id === "matrix") { const r = integer(options.rows, 2), m = integer(options.columns, 2); return `$ mat(${Array.from({length:r}, () => `(${Array.from({length:m}, () => "x").join(", ")})`).join(", ")}) $`; } return ` $ ${body} $`; } return `$$\n${body}\n$$`; };

export function buildInsertion(id, options = {}, context = {}) {
  const c = ctx(context), available = insertionAvailability(id, c); if (!available.enabled) return { text: "", notes: [available.reason] };
  const f = c.format, selected = targetText(c), title = esc(options.title || "Untitled", f), notes = [];
  let result;
  if (id === "heading") { const level = integer(options.level, 1); result = f === "latex" ? `${"\\" + (level === 1 ? "section" : level === 2 ? "subsection" : "subsubsection")}${options.numbered === false ? "*" : ""}{${title}}` : f === "typst" ? `${"=".repeat(level)} ${title}` : `${"#".repeat(level)} ${title}`; if (options.numbered && f === "typst") notes.push("Numbering follows the document's heading numbering setting."); }
  else if (id === "abstract") result = f === "latex" ? `\\begin{abstract}\n${selected || "Abstract text."}\n\\end{abstract}` : f === "typst" ? `#block[\n*Abstract.* ${selected || "Abstract text."}\n]` : `::: {.abstract}\n${selected || "Abstract text."}\n:::`;
  else if (id === "appendix") result = f === "latex" ? "\\appendix\n\\section{Appendix}" : f === "typst" ? "= Appendix <appendix>" : "## Appendix {#appendix}";
  else if (id === "toc") result = f === "latex" ? "\\tableofcontents" : f === "typst" ? "#outline()" : "[TOC]";
  else if (id === "figure") { const src = esc(options.src || "path/to/image.png", f); result = f === "latex" ? `\\begin{figure}\n\\centering\n\\includegraphics[width=${options.width || "\\linewidth"}]{${src}}\n\\caption{${title}}\n\\label{fig:${label(options.label || title)}}\n\\end{figure}` : f === "typst" ? `#figure(image("${src}"), caption: [${title}]) <${label(options.label || title)}>` : `![${title}](${src})${options.label ? ` {#${label(options.label)}}` : ""}`; if (f === "latex") notes.push("Requires \\usepackage{graphicx} in the preamble."); }
  else if (id === "table") result = rows(c, options, f);
  else if (id === "citation") { const keys = options.keys || (c.bibliography || []).slice(0, 1).map((x) => x.key); if (!keys.length) return { text: "", notes: ["Choose at least one bibliography entry."] }; result = f === "latex" ? `\\cite{${keys.join(",")}}` : keys.map((x) => `@${x}`).join(" "); }
  else if (id === "bibliography") result = f === "latex" ? `\\bibliography{${options.file || "references"}}` : f === "typst" ? `#bibliography("${options.file || "references.bib"}")` : "::: {#refs}\n:::";
  else if (id === "cross-reference") { const ref = options.target || gatherInsertTargets(c)[0]?.id || "label"; result = f === "latex" ? `\\ref{${ref}}` : `@${ref}`; }
  else if (id === "label") result = f === "latex" ? `\\label{${label(options.label || title)}}` : `<${label(options.label || title)}>`;
  else if (["inline-math", "display-math", "aligned-math", "gather-math", "cases", "matrix"].includes(id)) result = math(id, c, options);
  else if (id === "bulleted-list" || id === "numbered-list" || id === "description-list") { const items = selected ? selected.split(/\r?\n/).filter(Boolean) : ["First item", "Second item"]; if (f === "latex") { const env = id === "bulleted-list" ? "itemize" : id === "numbered-list" ? "enumerate" : "description"; result = `\\begin{${env}}\n${items.map((x) => `\\item ${esc(x, f)}`).join("\n")}\n\\end{${env}}`; } else if (f === "typst") result = items.map((x) => `${id === "numbered-list" ? "+" : "-"} ${x}`).join("\n"); else result = items.map((x, i) => `${id === "numbered-list" ? `${i+1}.` : "-"} ${x}`).join("\n"); }
  else if (["quote", "quotation"].includes(id)) result = f === "latex" ? `\\begin{${id}}\n${selected || "Quote"}\n\\end{${id}}` : f === "typst" ? `#quote(block: true)[${selected || "Quote"}]` : `> ${selected || "Quote"}`;
  else if (id === "code-block") result = f === "latex" ? `\\begin{verbatim}\n${selected || "code"}\n\\end{verbatim}` : f === "typst" ? "```" + (options.language || "") + "\n" + (selected || "code") + "\n```" : "```" + (options.language || "") + "\n" + (selected || "code") + "\n```";
  else if (id === "footnote") result = f === "latex" ? `\\footnote{${selected || "Note"}}` : f === "typst" ? `#footnote[${selected || "Note"}]` : `^[${selected || "Note"}]`;
  else if (id === "link") result = f === "latex" ? `\\href{${options.url || "https://example.com"}}{${selected || "Link"}}` : f === "typst" ? `#link("${options.url || "https://example.com"}")[${selected || "Link"}]` : `[${selected || "Link"}](${options.url || "https://example.com"})`;
  else if (["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"].includes(id)) { result = f === "latex" ? `\\begin{${id}}${options.title ? `[${esc(options.title, f)}]` : ""}\n${selected || "Statement."}\n\\end{${id}}` : f === "typst" ? `#block[**${id[0].toUpperCase()+id.slice(1)}.** ${selected || "Statement."}]` : `::: {.${id}}\n${selected || "Statement."}\n:::`; if (f === "latex") notes.push(`Define the ${id} environment in the preamble if it is not already available.`); }
  else if (id === "page-break") result = f === "latex" ? "\\clearpage" : f === "typst" ? "#pagebreak()" : "\\newpage{}";
  else if (id === "horizontal-rule") result = f === "latex" ? "\\noindent\\rule{\\linewidth}{0.4pt}" : f === "typst" ? "#line(length: 100%)" : "---";
  else if (id === "columns") result = f === "latex" ? `\\begin{multicols}{${integer(options.columns, 2)}}\n${selected || "Column content."}\n\\end{multicols}` : f === "typst" ? `#columns(${integer(options.columns, 2)})[${selected || "Column content."}]` : `::: {.columns}\n${selected || "Column content."}\n:::`;
  else if (id === "custom-environment") { const env = options.environment || "environment"; result = f === "latex" ? `\\begin{${env}}\n${selected || "Content."}\n\\end{${env}}` : `::: {.${label(env)}}\n${selected || "Content."}\n:::`; }
  else result = "";
  if (id === "bibliography" && (f === "markdown" || f === "quarto")) notes.push(`Configure the document bibliography to use ${options.file || "references.bib"}; the refs div is the render target.`);
  const wrapped = selected && !["heading", "table", "figure", "citation", "label", "inline-math", "display-math", "aligned-math", "gather-math", "cases", "matrix", "link", "footnote"].includes(id) ? result : result;
  return { ...block(wrapped, c.selection, ""), notes };
}

export function gatherInsertTargets(context = {}) {
  const c = ctx(context), text = c.text || "", found = [], add = (id, label, kind) => found.push({ id, label, kind });
  if (c.format === "latex") { for (const m of text.matchAll(/\\(?:section|subsection|subsubsection)\*?\{([^}]*)\}/g)) add(m[1].replace(/\s+/g, "-"), m[1], "heading"); for (const m of text.matchAll(/\\label\{([^}]+)\}/g)) add(m[1], m[1], "label"); }
  else { for (const m of text.matchAll(/^#{1,6}\s+(.+?)(?:\s+\{#([^}]+)\})?$/gm)) add(m[2] || m[1].toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "-"), m[1], "heading"); for (const m of text.matchAll(/<([\w:.-]+)>/g)) add(m[1], m[1], "label"); }
  return found;
}
