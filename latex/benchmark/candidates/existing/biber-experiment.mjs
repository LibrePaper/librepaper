#!/usr/bin/env node
// Compare the existing TeXlyre adapter with a candidate-only export fix.
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { browser, until } from "../../../../web/tools/browser-driver.mjs";
import { treeOf } from "../../corpus.mjs";
const root = join(import.meta.dirname, "../../../../");
const server = spawn(process.execPath, [join(import.meta.dirname, "candidate-server.mjs")], { stdio: "ignore" });
const base = "http://localhost:8703";
const out = join(import.meta.dirname, "biber-results.json");
const work = join(import.meta.dirname, "tmp-biber");
rmSync(work, { recursive: true, force: true }); mkdirSync(work, { recursive: true });
let driver;
const ev = async (s) => JSON.parse(await driver.evaluate(`(async()=>JSON.stringify(await(async()=>{${s}})()))()`));
const tree = treeOf({ id: "biber-sorting", source: "biblatex", main: "91-sorting-schemes.tex", engine: "xelatex" });
const distribution = JSON.parse(readFileSync(join(root, "latex/mirror/manifest.json"), "utf8")).distributions["texlyre-busytex"];
const runOne = async (label, moduleUrl) => {
  await driver.navigate(`${base}/`);
  await until("candidate module", () => ev("return Boolean(globalThis.candidateReady);"));
  await ev(`globalThis.result=null; globalThis.worker=new Worker('/candidate/lib/worker.js',{type:'module'}); globalThis.worker.onmessage=e=>{if(e.data.ready||e.data.error)globalThis.result=e.data;}; globalThis.worker.postMessage({cmd:'choose',base:${JSON.stringify(`${base}/mirror/`)},distribution:${JSON.stringify(distribution)}}); return true;`);
  await until("candidate init", () => ev("return globalThis.result !== null;"), 120000);
  const init = await ev("return globalThis.result;"); if (init.error) return { label, error: init.error };
  await ev(`globalThis.result=null; globalThis.worker.onmessage=e=>{const r=e.data.result||e.data; if(r.pdf){const a=new Uint8Array(r.pdf);let s='';for(let i=0;i<a.length;i+=8192)s+=String.fromCharCode(...a.subarray(i,i+8192));r.pdf=btoa(s)} globalThis.result=r}; globalThis.worker.postMessage({cmd:'compile',tree:${JSON.stringify(tree)}}); return true;`);
  await until("candidate compile", () => ev("return globalThis.result !== null;"), 120000);
  const result = await ev("return globalThis.result.result || globalThis.result;");
  const pdf = Boolean(result.pdf);
  return { label, pdf, pdfBase64: result.pdf || null, log: result.log || '', error: result.error || null };
};
try {
  await until("server", async () => (await fetch(`${base}/`)).ok, 10000);
  driver = await browser("chromium", join(work, "profile"), 9703);
  const results = [];
  // Existing adapter (loaded from web source) is measured through the normal harness.
  results.push(await runOne("candidate-biber-export", "/candidate/lib/latex/texlyre.js"));
  writeFileSync(out, JSON.stringify(results, null, 2) + "\n");
  for (const r of results) {
    if (r.pdfBase64) writeFileSync(join(import.meta.dirname, "biber-candidate.pdf"), Buffer.from(r.pdfBase64, "base64"));
    delete r.pdfBase64;
    console.log(`${r.label}: ${r.pdf ? "pdf" : "failed"} ${r.error || ""} ${/No biber module factory/.test(r.log||"") ? "no-biber-factory" : ""}`);
  }
} finally { await driver?.close(); server.kill(); rmSync(join(work, "profile"), { recursive: true, force: true }); }
