import assert from 'node:assert/strict';
// Compile every shared Insert action that has a Typst representation. This
// catches syntax drift in the generator instead of only checking its strings.
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { INSERT_ACTIONS, buildInsertion } from "../src/lib/insert.js";

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
    count: 2, gap: "1em", title: "Example", environment: "boxed", arguments: ["80%"],
  };
  let source='#let boxed(width, body) = block(width: width, body)\n#set page(width: 20cm)\n= Existing <existing>\n';
  const actions=[...INSERT_ACTIONS].sort((a,b)=>a.id==='bibliography'?-1:b.id==='bibliography'?1:0);
  for(const {id} of actions) {
    const current={...context,text:source,mainText:source,selection:{from:source.length,to:source.length,text:''}};
    const generated=buildInsertion(id,options,current);
    assert.ok(generated.text,id+' must generate source');
    assert.deepEqual(generated.additionalEdits,[],id+' uses native Typst constructs');
    source+=generated.text+'\n\n';
  }
  await writeFile(join(dir,'main.typ'),source);
  await run("typst", ["compile", "main.typ", "out.pdf"], { cwd: dir });
  console.log(`insert Typst render: ${actions.length} generated actions compiled`);
} finally {
  await rm(dir, { recursive: true, force: true });
}
