import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { buildInsertion } from "../src/lib/insert.js";

const run = promisify(execFile), dir = await mkdtemp(join(tmpdir(), "librepaper-insert-latex-"));
try {
  const mainPath = join(dir, "main.tex");
  let source = String.raw`\documentclass{article}
\newenvironment{customenv}[1]{}{}
\newtheorem{theorem}{Theorem}
\newtheorem{lemma}{Lemma}
\newtheorem{proposition}{Proposition}
\newtheorem{definition}{Definition}
\newtheorem{example}{Example}
\newtheorem{remark}{Remark}
\begin{document}
`;
  await writeFile(join(dir, "pixel.png"), Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64"));
  await writeFile(join(dir, "refs.bib"), "@article{smith2020, author={Smith, Sam}, title={Testing}, year={2020}}\n");
  const snippets = [
    ["heading", { title: "Methods", level: 2, numbered: false, label: "methods" }], ["abstract", {}], ["appendix", { title: "Supplement" }], ["toc", {}],
    ["figure", { src: "pixel.png", caption: "A figure", label: "fig-pixel", width: "80%" }], ["table", { rows: 2, columns: 3, caption: "Data", label: "tbl-data", alignment: "left" }],
    ["bibliography", { file: "refs" }], ["citation", { keys: ["smith2020"], style: "narrative" }], ["cross-reference", { target: "fig-pixel" }], ["label", { label: "anchor" }],
    ["inline-math", {}], ["display-math", { numbered: false, label: "eq-one" }], ["aligned-math", {}], ["gather-math", {}], ["cases", { rows: 2 }], ["matrix", { rows: 2, columns: 3, brackets: "bmatrix" }],
    ["bulleted-list", {}], ["numbered-list", {}], ["description-list", { terms: ["One", "Two"] }], ["quote", {}], ["quotation", {}], ["code-block", { language: "python" }], ["footnote", {}], ["link", { url: "https://example.com" }],
    ["theorem", {}], ["lemma", {}], ["proposition", {}], ["definition", {}], ["proof", {}], ["example", {}], ["remark", {}], ["page-break", {}], ["horizontal-rule", {}], ["columns", { columns: 2, gap: "12pt" }], ["custom-environment", { environment: "customenv", arguments:["Title"] }],
  ];
  for (const [id, options] of snippets) {
    const context = { format: "latex", path: "main.tex", mainPath: "main.tex", text: source, mainText: source, selection: { from: source.length, to: source.length, text: "" }, bibliography: [{ key: "smith2020" }] };
    const built = buildInsertion(id, options, context);
    assert.ok(built.text, `${id} generated no text`);
    for (const edit of [...built.additionalEdits].sort((a, b) => b.from - a.from)) {
      assert.equal(edit.path, "main.tex", `${id} dependency edit targeted the main source`);
      source = source.slice(0, edit.from) + edit.insert + source.slice(edit.to);
    }
    source += built.text + "\n";
  }
  source += "\\end{document}\n";
  await writeFile(mainPath, source);
  await run("pdflatex", ["-interaction=nonstopmode", "-halt-on-error", "main.tex"], { cwd: dir });
  await run("bibtex", ["main"], {cwd:dir});
  await run("pdflatex", ["-interaction=nonstopmode", "-halt-on-error", "main.tex"], { cwd: dir });
  assert.match(source, /\\usepackage\{amsmath\}/);
  assert.match(source, /\\usepackage\{listings\}/);
  assert.match(source, /\\usepackage\{natbib\}/);
  assert.match(source, /\\setlength\{\\columnsep\}\{12pt\}/);
  console.log("insert-latex-render: buildInsertion dependencies and all 35 actions compiled twice");
} finally { await rm(dir, { recursive: true, force: true }); }
