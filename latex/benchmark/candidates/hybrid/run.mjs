#!/usr/bin/env node
import { mkdirSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { browser, until } from "../../../../web/tools/browser-driver.mjs";
import { fixture, inputFiles, EXAMPLE, PHASES } from "../hybrid-validation/fixture.mjs";
import { inspectPDF } from "../../results.mjs";
import { treeDigest } from "../../corpus.mjs";
import { server } from "./server.mjs";
const ROOT=dirname(fileURLToPath(import.meta.url)), RESULTS=join(ROOT,"results"), base="http://127.0.0.1:8706", stem=EXAMPLE.main.replace(/\.tex$/,"");
const refs=join(ROOT,"../hybrid-validation/references"), enc=x=>x?Buffer.from(x,"base64"):null;
const arrays=t=>Object.fromEntries(Object.entries(inputFiles(t)).map(([k,v])=>[k,typeof v==="string"?v:[...v]])); let driver;
const report={date:new Date().toISOString(),runtime:{},cases:[{id:EXAMPLE.id,engine:EXAMPLE.engine,phases:[],verification:null}]};
const writeReport=()=>writeFileSync(join(RESULTS,"report.json"),JSON.stringify(report,null,2)+"\n");
async function browserRun(payload){
  await driver.evaluate(`globalThis.hybridJob={done:false};Promise.resolve(globalThis.hybridRun(${JSON.stringify(payload)})).then(value=>globalThis.hybridJob={done:true,value}).catch(error=>globalThis.hybridJob={done:true,error:String(error)});true`);
  let state; await until("hybrid compile",async()=>{state=await driver.evaluate("globalThis.hybridJob.done?globalThis.hybridJob:null");return state!==null},600000); if(state.error)throw Error(state.error); return state.value;
}
function save(phase,result){const dir=join(RESULTS,EXAMPLE.id,phase);mkdirSync(dir,{recursive:true});if(result.pdf)writeFileSync(join(dir,stem+".pdf"),enc(result.pdf));for(const [ext,v] of Object.entries(result.outputs||{}))if(v)writeFileSync(join(dir,stem+"."+ext),enc(v));if(result.log)writeFileSync(join(dir,stem+".log"),result.log);const p=result.pdf?inspectPDF(join(dir,stem+".pdf")):{pdf:false,pages:0};const r={phase,success:result.success,pdf:p.pdf,pages:p.pages,elapsedMs:result.elapsedMs,timings:result.timings||null,error:result.error||null,biber:result.biber||null};writeFileSync(join(dir,"result.json"),JSON.stringify(r,null,2)+"\n");return r}
try{
  await until("hybrid server",async()=>(await fetch(base+"/")).ok,10000);rmSync(join(ROOT,"profiles"),{recursive:true,force:true});mkdirSync(join(ROOT,"profiles"),{recursive:true});rmSync(join(RESULTS,EXAMPLE.id),{recursive:true,force:true});mkdirSync(RESULTS,{recursive:true});writeReport();driver=await browser("chromium",join(ROOT,"profiles/chromium"),9706);await driver.navigate(base+"/");
  await until("hybrid harness",async()=>{const s=await driver.evaluate("({ready:Boolean(globalThis.hybridReady),error:globalThis.hybridError})");if(s.error)throw Error(s.error);return s.ready},20000);report.runtime.browserMetadata=await driver.evaluate("globalThis.hybridMetadata||null");writeReport();
  for(const phase of PHASES){await fetch(base+"/__reset");const tree=fixture(phase),result=await browserRun({files:arrays(tree),mainFile:tree.main,engine:EXAMPLE.engine,assetBaseUrl:base+"/",texliveUrl:base+"/2025/",phase}),record=save(phase,result);record.network=await(await fetch(base+"/__bytes")).json();record.treeSha256=treeDigest(tree);record.native=JSON.parse(readFileSync(join(refs,phase,"reference.json")));report.cases[0].phases.push(record);writeReport();console.log(`${phase}: ${record.success?"success":"failed"}, ${(record.elapsedMs/1000).toFixed(2)}s${record.error?` (${record.error})`:""}`);if(!result.success||!record.pdf)throw Error(`${phase}: compile failed`);if(phase==="prose-edit"&&result.biber&&!result.biber.cached)throw Error("prose-edit unexpectedly invoked Biber");if((phase==="citation-addition"||phase==="title-edit")&&(!result.biber||result.biber.cached))throw Error(`${phase} did not invoke Biber`)}
  let verification={ok:true};try{const out=execFileSync(process.execPath,[join(ROOT,"../hybrid-validation/verify.mjs"),"--candidate",join(RESULTS,EXAMPLE.id),"--references",refs],{encoding:"utf8",stdio:["ignore","pipe","pipe"]});verification=JSON.parse(out)}catch(e){try{verification=JSON.parse(e.stdout)}catch{verification={ok:false,output:e.stdout||e.stderr||String(e)}}}report.cases[0].verification=verification;writeReport();if(!verification.ok){console.error(verification.output||verification.failures);process.exitCode=1}
  const broken=fixture("title-edit");broken.texts[broken.main]=broken.texts[broken.main].replace("\\end{document}","\\KomodocUndefinedCommand\\end{document}");
  const bad=await browserRun({files:arrays(broken),mainFile:broken.main,engine:EXAMPLE.engine,assetBaseUrl:base+"/",texliveUrl:base+"/2025/",phase:"invalid-tex"});
  report.invalidTexRejected=bad.success===false&&!bad.pdf;
  writeFileSync(join(RESULTS,"invalid-tex.log"),bad.log||bad.error||"");
  if(!report.invalidTexRejected)throw Error("Invalid TeX returned an old PDF");
  console.log("invalid TeX: rejected without stale PDF");
}catch(e){report.error=String(e);console.error(e);process.exitCode=1}finally{try{report.runtime.assets=await(await fetch(base+"/__metadata")).json();writeReport()}finally{await driver?.close();server.close()}}
