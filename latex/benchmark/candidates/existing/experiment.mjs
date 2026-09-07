#!/usr/bin/env node
// Controlled browser experiments for existing adapters. This file never edits
// the shared mirror or web source; it only changes the in-memory project tree.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { browser, until } from "../../../../web/tools/browser-driver.mjs";
import { treeOf } from "../../corpus.mjs";

const ROOT = join(import.meta.dirname, "../../../../");
const serve = spawn(process.execPath, [join(ROOT, "latex/tools/serve.mjs"), "--port", "8703"], { stdio: "ignore" });
const base = "http://localhost:8703";
let driver;
const work = join(import.meta.dirname, "tmp-experiment");
rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });

const evalPage = async (source) => JSON.parse(await driver.evaluate(`(async () => JSON.stringify(await (async () => { ${source} })()))()`));
const start = async () => {
  await until("mirror", async () => (await fetch(`${base}/__bytes`)).ok, 10000);
  driver = await browser("chromium", join(work, "profile"), 9703);
  await driver.navigate(`${base}/`);
  await until("harness", () => evalPage("return Boolean(globalThis.latexReady);"));
};
const choose = async (name) => {
  await evalPage("globalThis.pending = null; return true;");
  await evalPage(`latex.at(${JSON.stringify(`${base}/mirror/`)}); latex.choose(${JSON.stringify(name)}).then(() => globalThis.pending={ok:true}, e => globalThis.pending={error:String(e)}); return true;`);
  await until("compiler", () => evalPage("return globalThis.pending !== null;"), 120000);
  const ready = await evalPage("return globalThis.pending;");
  if (ready.error) throw new Error(ready.error);
};
const compile = async (tree) => {
  await evalPage("globalThis.pending = null; return true;");
  await evalPage(`latex.compile(${JSON.stringify(tree)}).then(r => { const enc=x=>{ if(!x) return null; const a=new Uint8Array(x); let s=''; for(let i=0;i<a.length;i+=8192) s+=String.fromCharCode(...a.subarray(i,i+8192)); return btoa(s); }; globalThis.pending={pdf:enc(r.pdf),log:r.log||'',diagnostics:r.diagnostics||[]}; }, e => globalThis.pending={error:String(e),log:''}); return true;`);
  await until("compile", () => evalPage("return globalThis.pending !== null;"), 120000);
  return evalPage("return globalThis.pending;");
};
const acm = treeOf({ id: "acm-conference", source: "acmart", main: "sigconf.tex", engine: "pdflatex" });
const packageText = (path) => readFileSync(join(ROOT, "latex/mirror", path), "utf8");
const xkeyval = packageText("packages/pdftex/ad/adec9c8e36d791b1-xkeyval.tex");
const xkvutils = packageText("packages/pdftex/a4/a49b792fa91181b8-xkvutils.tex");
const xkvtxhdr = packageText("packages/pdftex/20/209aadb5e1ab10e1-xkvtxhdr.tex");
const keyval = packageText("packages/pdftex/8e/8e19ce03ea9b83fe-keyval.tex");
const tests = [];
try {
  await start();
  await choose("texlyre-busytex");
  for (const [label, tree] of [
    ["acm-original", acm],
    ["acm-project-xkeyval-tex", { ...acm, texts: { ...acm.texts, "xkeyval.tex": xkeyval } }],
    ["acm-project-xkeyval-deps", { ...acm, texts: { ...acm.texts, "xkeyval.tex": xkeyval, "xkvutils.tex": xkvutils, "xkvtxhdr.tex": xkvtxhdr, "keyval.tex": keyval } }],
  ]) {
    const result = await compile(tree);
    const errors = (result.diagnostics || []).filter(d => d.severity === "error").map(d => d.message);
    tests.push({ label, pdf: Boolean(result.pdf), errors, log: result.log });
  }
} finally {
  await driver?.close();
  serve.kill();
  rmSync(join(work, "profile"), { recursive: true, force: true });
}
writeFileSync(join(import.meta.dirname, "experiment-results.json"), JSON.stringify(tests, null, 2) + "\n");
for (const t of tests) console.log(`${t.label}: ${t.pdf ? "pdf" : "failed"} ${t.errors.join(" | ")}`);
