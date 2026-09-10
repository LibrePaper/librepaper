// Compile every shared Insert action that has a Typst representation. This
// catches syntax drift in the generator instead of only checking its strings.
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { buildTypst } from "../src/lib/insert-typst.js";

const run = promisify(execFile);
const dir = await mkdtemp(join(tmpdir(), "librepaper-insert-typst-"));
try {
  await writeFile(join(dir, "x.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"10\" height=\"10\"/></svg>");
  await writeFile(join(dir, "references.bib"), "@article{smith2020, author={Smith, Ada}, title={A Test}, year={2020}}\n");
  const context = {
    format: "typst", path: "main.typ", text: "= Existing\n", mainPath: "main.typ",
    mainText: "= Existing\n", selection: { from: 0, to: 0, text: "" },
    files: [{ path: "x.svg", text: "" }, { path: "references.bib", text: "" }],
    bibliography: [{ key: "smith2020", author: "Smith, Ada", title: "A Test", year: 2020 }],
  };
  const options = {
    level: 2, numbered: true, src: "x.svg", caption: "A figure", label: "fig:test", width: "80%",
    rows: 2, columns: 2, header: true, alignment: "center", keys: ["smith2020"], locator: "p. 3",
    file: "references.bib", target: "existing", brackets: "brackets", language: "text", url: "https://example.com",
    count: 2, gap: "1em", title: "Example", environment: "boxed",
  };
  const ids = ["heading", "abstract", "appendix", "toc", "figure", "table", "citation", "bibliography", "cross-reference", "label", "inline-math", "display-math", "aligned-math", "gather-math", "cases", "matrix", "bulleted-list", "numbered-list", "description-list", "quote", "quotation", "code-block", "footnote", "link", "theorem", "lemma", "proposition", "definition", "proof", "example", "remark", "page-break", "horizontal-rule", "columns"];
  const snippets = ids.map((id) => buildTypst(id, options, { ...context, selection: { from: 0, to: 1, text: "x" } }).text).filter(Boolean);
  snippets.push(buildTypst("custom-environment", options, { ...context, text: "#let boxed(body) = block(body)" }).text);
  await writeFile(join(dir, "main.typ"), `#let boxed(body) = block(body)\n#set page(width: 20cm)\n#set heading(numbering: "1.")\n= Existing <existing>\n${snippets.join("\n\n")}\n`);
  await run("typst", ["compile", "main.typ", "out.pdf"], { cwd: dir });
  console.log(`insert Typst render: ${ids.length + 1} generated actions compiled`);
} finally {
  await rm(dir, { recursive: true, force: true });
}
