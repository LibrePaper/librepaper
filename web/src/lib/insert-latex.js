/* LaTeX adapter for the Insert menu.  Positions in additionalEdits are
 * absolute offsets in the source file named by edit.path. */
const s = (x, d = "") => x == null ? d : String(x);
const n = (x, d, min = 1, max = 100) => Math.min(max, Math.max(min, Number.isFinite(+x) ? Math.floor(+x) : d));
const tex = value => s(value).replace(/[\\{}%#$&_~^]/g, c => ({'\\':'\\textbackslash{}','~':'\\textasciitilde{}','^':'\\textasciicircum{}'}[c] || '\\'+c));
export const key = (x) => s(x, "label").trim().replace(/[^\p{L}\p{N}:._-]+/gu, "-").replace(/^-+|-+$/g, "") || "label";
const source = (c = {}) => s(c.mainText || c.text);
const includeTarget = (value) => s(value).trim().replace(/^\.\//, "").replace(/^\/+/, "").replace(/\.tex$/, "");
const includesPath = (text, path) => {
  const want = includeTarget(path), base = want.split("/").pop();
  for (const m of text.matchAll(/\\(?:input|include|subfile)\{([^}]+)\}|\\import\{([^}]*)\}\{([^}]+)\}/g)) {
    const target = m[1] != null ? includeTarget(m[1]) : includeTarget(`${m[2]}/${m[3]}`.replace(/\/+/g, "/"));
    if (target === want || target.split("/").pop() === base) return true;
  }
  return false;
};
const preambleAnchor = (c = {}, { afterClass = false } = {}) => {
  const current = c.path ? { path: c.path, text: s(c.text) } : null;
  const includer = c.path ? [...(Array.isArray(c.files) ? c.files : []).map(file => ({ path: file?.path, text: s(file?.text) })), { path: c.mainPath, text: s(c.mainText) }].find(file => file.path && file.text && file.path !== c.path && includesPath(file.text, c.path)) : null;
  const main = c.mainPath && c.mainPath !== c.path ? { path: c.mainPath, text: s(c.mainText) } : null;
  for (const { path, text } of [current, includer, main].filter(Boolean)) {
    if (!path || !text) continue;
    const classLine = text.match(/\\documentclass(?:\[[^\]]*\])?\{[^}]+\}[^\S\n]*(?:\n|$)/);
    const classEnd = classLine ? classLine.index + classLine[0].length : -1;
    if (afterClass && classEnd >= 0) return { path, at: classEnd, text };
    const begin = text.indexOf("\\begin{document}");
    if (begin >= 0) return { path, at: begin, text };
    if (classEnd >= 0) return { path, at: classEnd, text };
  }
  return null;
};
// The root is the file whose preamble an edit lands in: the current file when
// it has one, else the file that \input{}s it, else the project's main.
export const latexRoot = (c) => preambleAnchor(c)?.path || "";
const packageEdit = (name, c, notes, options = "") => {
  const anchor = preambleAnchor(c, { afterClass: name === "amsthm" }), bracket = options ? `[${options}]` : "";
  const text = [anchor?.text, c.text, c.mainText].map(value => s(value)).join("\n");
  if (new RegExp(`\\\\usepackage(?:\\[[^\\]]*\\])?\\{[^}]*\\b${name}\\b[^}]*\\}`).test(text)) return [];
  if (!anchor) { notes.push(`Add \\usepackage${bracket}{${name}} to the preamble.`); return []; }
  return [{ path: anchor.path, from: anchor.at, to: anchor.at, insert: `\\usepackage${bracket}{${name}}\n` }];
};
export const latexPackageEdits = (name, c, notes, options = "") => packageEdit(name, c, notes, options);
const preambleEdit = (insert, c, notes) => {
  const anchor = preambleAnchor(c);
  if (!anchor) { notes.push(`Add ${insert} to the preamble.`); return []; }
  return [{ path: anchor.path, from: anchor.at, to: anchor.at, insert: insert + "\n" }];
};
const result = (text, additionalEdits = [], notes = [], placeholder) => ({ text, additionalEdits, notes, ...(placeholder ? { placeholder } : {}) });
const selected = (c) => s(c.selection?.text, "");
const labelValue = (o, fallback) => o.label ? key(o.label) : fallback;

export function buildLatex(id, options = {}, context = {}) {
  const c = context || {}, text = source(c), body = selected(c), notes = [], additionalEdits = [];
  const title = tex(options.title || body || (id === "appendix" ? "Appendix" : "Heading")), rowCount = n(options.rows, 3), colCount = n(options.columns, 3);
  const add = (pkg, opts) => additionalEdits.push(...packageEdit(pkg, c, notes, opts));
  if (id === "heading") { const level = n(options.level, 1, 1, 5), cmd = ["section", "subsection", "subsubsection", "paragraph", "subparagraph"][level - 1], lab = options.label ? `\\label{${key(options.label)}}` : ""; return result(`\\${cmd}${options.numbered === false ? "*" : ""}{${title}}${lab}`, [], notes, title); }
  if (id === "abstract") return result(`\\begin{abstract}\n${body || "Abstract text."}\n\\end{abstract}`, [], notes, "Abstract text.");
  if (id === "appendix") return result(`\\appendix\n\\section${options.numbered === false ? "*" : ""}{${title}}`, [], notes, title);
  if (id === "toc") return result("\\tableofcontents", [], notes);
  if (id === "figure") {
    add("graphicx");
    if (!options.src) throw Error("Choose an image first.");
    const src=s(options.src).replace(/[{}%#]/g,c=>'\\'+c),rawWidth=options.width;
    const width=/%$/.test(rawWidth||'')?(parseFloat(rawWidth)/100)+'\\linewidth':rawWidth||'\\linewidth';
    const caption=options.caption||options.label?'\\caption{'+tex(options.caption||'Figure')+'}\n':'';
    const anchor=options.label?'\\label{'+key(options.label)+'}\n':'';
    return result('\\begin{figure}[htbp]\n\\centering\n\\includegraphics[width='+width+']{'+src+'}\n'+caption+anchor+'\\end{figure}',additionalEdits,notes);
  }
  if (id === "table") { const header = options.header !== false, alignment = { left: "l", center: "c", right: "r" }[options.alignment] || (options.alignment === "default" ? "c" : s(options.alignment, "c").slice(0, 1)), align = alignment.repeat(colCount); const cells = []; for (let r = 0; r < rowCount; r++) cells.push(Array.from({ length: colCount }, (_, j) => r === 0 && header ? tex(options.headers?.[j] || `Header ${j + 1}`) : tex(options.cells?.[r]?.[j] || `Cell ${r + 1},${j + 1}`)).join(" & ")); const tabular = `\\begin{tabular}{${align}}\n${header ? "\\toprule\n" : ""}${cells.join(" \\\\\n")}${header ? " \\\\\n\\bottomrule" : ""}\n\\end{tabular}`; if (header) add("booktabs"); const caption = options.caption || options.label ? `\\caption{${tex(options.caption || "Table")}}\n` : ""; const lab = options.label ? `\\label{${key(options.label)}}\n` : ""; return result(`\\begin{table}[${options.placement || "htbp"}]\n\\centering\n${caption}${lab}${tabular}\n\\end{table}`, additionalEdits, notes, header ? "Header 1" : "Cell 1,1"); }
  if (id === "citation") { const keys = options.keys || []; if (!keys.length) throw Error("Choose at least one reference."); if (keys.some(k => !/^[\p{L}\p{N}_.:+-]+$/u.test(k))) throw Error("Unsupported citation key."); const loc = options.locator ? `[${tex(options.locator)}]` : ""; const biblatex = /usepackage(?:\[[^\]]*\])?\{biblatex\}/.test(text) || c.biblatex; const natbib = /usepackage(?:\[[^\]]*\])?\{natbib\}/.test(text); const style = options.style || "parenthetical"; if (!biblatex && !natbib && style === "narrative") {
      additionalEdits.push(...packageEdit("natbib", c, notes));
      const existing = text.match(/\\bibliographystyle\{plain\}/);
      if (existing) additionalEdits.push({path:c.mainPath || c.path,from:existing.index,to:existing.index+existing[0].length,insert:'\\bibliographystyle{plainnat}'});
    } const command = biblatex ? (style === "narrative" ? "\\textcite" : "\\parencite") : (natbib || style === "narrative") ? (style === "narrative" ? "\\citet" : "\\citep") : "\\cite"; return result(`${command}${loc}{${keys.join(",")}}`, additionalEdits, notes); }
  if (id === "bibliography") { const file = tex(options.file || "references"), bibfile = file.endsWith(".bib") ? file : file + ".bib", hasBiblatex = /usepackage(?:\[[^\]]*\])?\{biblatex\}/.test(text) || c.biblatex; if (options.biblatex || hasBiblatex) { if (!/usepackage(?:\[[^\]]*\])?\{biblatex\}/.test(text)) additionalEdits.push(...preambleEdit(`\\usepackage[backend=biber]{biblatex}`, c, notes)); if (!new RegExp(`\\\\addbibresource\{${file.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\}`).test(text)) additionalEdits.push(...preambleEdit(`\\addbibresource{${bibfile}}`, c, notes)); return result("\\printbibliography", additionalEdits, notes); } return result(`\\bibliographystyle{${tex(options.style || (/\\usepackage(?:\[[^\]]*\])?\{natbib\}/.test(text) ? "plainnat" : "plain"))}}\n\\bibliography{${file.replace(/\.bib$/, "")}}`, [], notes); }
  if (id === "cross-reference") return result(`\\ref{${key(options.target || "label")}}`, [], notes, options.target || "label");
  if (id === "label") return result(`\\label{${labelValue(options, "label")}}`, [], notes, labelValue(options, "label"));
  if (id === "inline-math") return result(`$${body || "x = y"}$`, [], notes, "x = y");
  if (id === "display-math") { add("amsmath"); const formula = body || "x = y", labelText = options.label ? `\\label{${key(options.label)}}\n` : "", star = options.numbered === false ? "*" : ""; if (options.inMath) return result(`\\begin{aligned}\n${formula}\n\\end{aligned}`, additionalEdits, notes, formula); return result(`\\begin{equation${star}}\n${labelText}${formula}\n\\end{equation${star}}`, additionalEdits, notes, formula); }
  if (["bulleted-list", "numbered-list", "description-list"].includes(id)) { const items = (body ? body.split(/\r?\n/).filter(Boolean) : ["First item", "Second item"]); const env = id === "bulleted-list" ? "itemize" : id === "numbered-list" ? "enumerate" : "description"; const content = id === "description-list" ? items.map((x, i) => `\\item[${tex(options.terms?.[i] || `Term ${i + 1}`)}] ${x}`).join("\n") : items.map((x) => `\\item ${x}`).join("\n"); return result(`\\begin{${env}}\n${content}\n\\end{${env}}`, [], notes, "First item"); }
  if (["quote", "quotation"].includes(id)) return result(`\\begin{${id}}\n${body || "Quote"}\n\\end{${id}}`, [], notes, "Quote");
  if (id === "code-block") { const language = s(options.language).toLowerCase(), known = { python: "Python", r: "R", javascript: "JavaScript", js: "JavaScript", bash: "bash", sh: "bash", rust: "Rust" }; if (!language) return result(`\\begin{verbatim}\n${body || "code"}\n\\end{verbatim}`, [], notes, "code"); if (!known[language]) throw new Error(`Unsupported listings language: ${language}`); add("listings");
    if (["JavaScript","Rust"].includes(known[language]) && !text.includes('\\lstdefinelanguage{'+known[language]+'}')) additionalEdits.push(...preambleEdit('\\lstdefinelanguage{'+known[language]+'}{morekeywords={function,let,const,var,return,if,else,for,while,fn,impl,struct,pub,use,match},sensitive=true,morecomment=[l]{//},morestring=[b]"}',c,notes));
    return result(`\\begin{lstlisting}[language=${known[language]}]\n${body || "code"}\n\\end{lstlisting}`, additionalEdits, notes, "code"); }
  if (id === "footnote") return result(`\\footnote{${body || "Note"}}`, [], notes, "Note");
  if (id === "link") { add("hyperref"); return result(`\\href{${s(options.url || "https://example.com").replace(/[{}%#&]/g, c => "\\"+c)}}{${body || tex(options.title || "Link")}}`, additionalEdits, notes, "Link"); }
  if (["theorem", "lemma", "proposition", "definition", "proof", "example", "remark"].includes(id)) { add("amsthm"); const declared = [...text.matchAll(/\\(?:newtheorem|newenvironment|renewenvironment)\*?\s*\{([^}]+)\}/g)].some(match => match[1] === id); if (!declared && id !== "proof") additionalEdits.push(...preambleEdit(`\\newtheorem{${id}}{${id[0].toUpperCase() + id.slice(1)}}`, c, notes)); const lab = options.label ? `\\label{${key(options.label)}}\n` : ""; return result(`\\begin{${id}}${options.title ? `[${tex(options.title)}]` : ""}\n${lab}${body || "Statement."}\n\\end{${id}}`, additionalEdits, notes, "Statement."); }
  if (id === "page-break") return result(options.strong ? "\\clearpage" : "\\newpage", [], notes);
  if (id === "horizontal-rule") return result("\\noindent\\rule{\\linewidth}{0.4pt}", [], notes);
  if (id === "columns") { add("multicol"); const gap = options.gap ? `\n\\setlength{\\columnsep}{${s(options.gap)}}` : ""; return result(`${gap}\\begin{multicols}{${n(options.columns, 2)}}\n${body || "Column content."}\n\\end{multicols}`, additionalEdits, notes, "Column content."); }
  if (id === "custom-environment") {
    const env=key(options.environment||'environment');
    const all=[text,...(c.files||[]).map(file=>file.text)].filter(Boolean).join('\n');
    if(!new RegExp('\\\\(?:newenvironment|renewenvironment|newtheorem)\\s*\\{'+env+'\\}').test(all))throw Error("LaTeX environment '"+env+"' is not defined in the document.");
    const signature=all.match(new RegExp('\\\\(?:newenvironment|renewenvironment)\\s*\\{'+env+'\\}\\s*\\[(\\d+)\\](?:\\s*\\[([^\\]]*)\\])?'));
    const count=Number(signature?.[1])||0;let args='';
    for(let i=0;i<count;i++){
      const value=options.arguments?.[i] || (i===0?signature?.[2]:'');
      if(!value)throw Error('Enter argument '+(i+1)+' for '+env+'.');
      args+=i===0&&signature?.[2]!=null?'['+value+']':'{'+value+'}';
    }
    return result('\\begin{'+env+'}'+args+'\n'+(body||'Content.')+'\n\\end{'+env+'}',[],notes,body||'Content.');
  }
  return result("", [], [`Unknown LaTeX insertion: ${id}`]);
}
