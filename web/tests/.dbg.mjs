import { readFileSync } from "node:fs";
import { loadOrgSyntax, orgSyntax } from "../src/lib/org-language/module.js";
await loadOrgSyntax(readFileSync("wasm/org.wasm"));
for (const src of process.argv.slice(2)) {
  const t = orgSyntax().parse(src.replace(/\\n/g, "\n")); let o = [];
  t.iterate({ enter(n) { if (n.name !== "Document") o.push(`${n.name}[${n.from},${n.to}]`); } }); console.log(JSON.stringify(src), "=>", o.join(" "));
}
