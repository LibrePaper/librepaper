import {readFileSync, globSync, existsSync} from "node:fs";
import {dirname, resolve, relative} from "node:path";
process.chdir("/home/vincent/repos/librepaper/librepaper-site");
const README="/home/vincent/repos/librepaper/librepaper/README.md";
let fail=0; const bad=(m)=>{console.log("  ✗ "+m); fail++;};
const EXPECTED=["site/start/index.md","site/start/sandbox.md","site/authoring/index.md",
 "site/authoring/latex.md","site/authoring/typst.md","site/authoring/quarto.md",
 "site/collaborate/share.md","site/collaborate/edit.md","site/collaborate/history.md",
 "site/collaborate/review.md","site/agents.md","site/cli/index.md","site/cli/publish.md",
 "site/host/index.md","site/host/access.md","site/host/storage.md","site/internals.md"];

console.log("== 1. pages present ==");
for(const p of EXPECTED) if(!existsSync(p)) bad(`missing ${p}`);
const stray=globSync("site/**/*.md").filter(p=>!EXPECTED.includes(p));
for(const p of stray) bad(`unexpected page ${p}`);
if(!fail) console.log("  ok — all 17, no strays");

const pages=EXPECTED.filter(existsSync);
const txt=Object.fromEntries(pages.map(p=>[p,readFileSync(p,"utf8")]));
const all=Object.values(txt).join("\n");

console.log("\n== 2. code blocks vs README ==");
const blocks=t=>{const o=[];let i=false,c=[];for(const l of t.split("\n")){
  if(l.startsWith("```")){if(i){o.push(c.join("\n"));c=[];i=false}else i=true;continue}
  if(i)c.push(l)}return o};
const rb=blocks(readFileSync(README,"utf8")), sb=new Set(blocks(all).map(b=>b.trim()));
const miss=rb.filter(b=>!sb.has(b.trim()));
console.log(`  README ${rb.length} blocks, site ${blocks(all).length}`);
for(const b of miss) bad("code block lost: "+b.trim().slice(0,110).replace(/\n/g," | "));
if(!miss.length) console.log("  ok — every README block present byte-identical");

console.log("\n== 3. flags / env vars exist in the code ==");
// Checked against the Rust source, not the README: the README is a stub now,
// and what the manual has to agree with is the binary. clap declares a flag as
// long = "document-expire-after", without the dashes, so the bare name is what
// is looked for. Placeholders the docs use to show the naming convention are
// not flags and are skipped.
// The Rust source plus the shell installer, which owns its own variables.
// clap derives a long flag from the field name when it is not given one, so a
// documented --no-local is the field no_local: both spellings count.
const code = [...globSync("crates/librepaper/src/**/*.rs"), ...globSync("deploy/*.sh")]
  .map((f)=>readFileSync(f,"utf8")).join("\n");
const PLACEHOLDER = /foo.bar/i;
const CLAP_BUILTIN = new Set(["--help","--version"]);
for(const p2 of pages){
  for(const m of txt[p2].match(/--[a-z][a-z0-9-]{2,}/g)||[]){
    if(CLAP_BUILTIN.has(m)||PLACEHOLDER.test(m)) continue;
    const bare=m.slice(2), snake=bare.replace(/-/g,"_");
    if(!code.includes(m) && !code.includes(`"${bare}"`) && !new RegExp(`\\b${snake}\\b`).test(code))
      bad(`flag not in the source: ${m}  (${p2})`);
  }
  for(const m of txt[p2].match(/\bLIBREPAPER_[A-Z0-9_]+\b/g)||[]){
    if(PLACEHOLDER.test(m)) continue;
    if(!code.includes(m)) bad(`env var not in the source: ${m}  (${p2})`);
  }
}
if(!fail) console.log("  ok");

console.log("\n== 4. Quarto leftovers ==");
for(const p of pages){
  if(/:::\s*\{\.callout/.test(txt[p])) bad(`Quarto callout left in ${p}`);
  for(const m of txt[p].matchAll(/\]\(([^)]*\.qmd[^)]*)\)/g)) bad(`.qmd link in ${p}: ${m[1]}`);
}
if(!fail) console.log("  ok");

console.log("\n== 5. internal links resolve ==");
for(const p of pages)
  for(const m of txt[p].matchAll(/\]\((?!https?:|#|mailto:)([^)#]+)(#[^)]*)?\)/g)){
    const target=resolve(dirname(p),m[1]);
    if(!existsSync(target)) bad(`dead link in ${p}: ${m[1]} -> ${relative(process.cwd(),target)}`);
  }
if(!fail) console.log("  ok");

console.log("\n== 6. flagship sample block intact ==");
const rev=txt["site/collaborate/review.md"]||"";
for(const [what,s] of [["curly open","“with 95% probability"],["curly close",'the interval”'],
  ["sample heading","## Reviewer: annegrandchamp"],["Then/Now","**Now:** no longer in the document."]])
  if(!rev.includes(s)) bad(`review.md lost ${what}`);
if(!fail) console.log("  ok");

console.log("\n== 7. page sizes ==");
for(const p of pages){const n=txt[p].split("\n").length; if(n<25) bad(`${p} only ${n} lines`);}
if(!fail) console.log("  ok — none under 25 lines");

console.log(`\n${fail?`${fail} PROBLEM(S)`:"ALL CHECKS PASSED"}`);
