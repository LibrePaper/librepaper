import {createServer} from 'node:http';
import {readFileSync,writeFileSync,realpathSync,mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,relative,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {browser,until} from '../../../../web/tools/browser-driver.mjs';
const evidenceDir=dirname(fileURLToPath(import.meta.url));
const root=join(evidenceDir,'../wasmtex/source/lib');
const manifest=await (await fetch('https://corca-ai.github.io/wasmtex/wasmtex/2026/manifest.json')).json();
if(manifest.releaseId!=='2026-8b7946970153c52e')throw Error('Upstream engine release changed; review and repin before rerunning');
const pins=new Map(manifest.files.map(x=>[x.name,x]));
const cache=new Map(), receipts={};
let bytes=0,requests=0;
const server=createServer(async(req,res)=>{
 try {
  const path=new URL(req.url,'http://localhost').pathname;
  if(path==='/'){res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>Engine check</title>');return;}
  if(path.startsWith('/lib/')){
   const file=realpathSync(join(root,path.slice(5)));
   if(relative(root,file).startsWith('..'))throw Error('Invalid path');
   res.setHeader('Content-Type','text/javascript');res.end(readFileSync(file));return;
  }
  const engine=path.startsWith('/wasmtex/2026/');
  if(!engine&&!path.startsWith('/2026/')){res.writeHead(404);res.end();return;}
  const url=engine?'https://corca-ai.github.io/wasmtex'+path:'https://texlive.corca.ai/snapshots/2026-ba38749b8714505a'+path;
  if(!cache.has(url))cache.set(url,(async()=>{
   const r=await fetch(url,{signal:AbortSignal.timeout(30000)});
   const data=Buffer.from(await r.arrayBuffer());
   const sha256=createHash('sha256').update(data).digest('hex');
   if(engine&&r.ok){
    const pin=pins.get(path.slice('/wasmtex/2026/'.length));
    if(!pin||pin.bytes!==data.length||pin.sha256!==sha256)throw Error('Asset digest mismatch');
   }
   receipts[path]={status:r.status,bytes:data.length,sha256};
   return {data,status:r.status,fileid:r.headers.get('fileid')};
  })());
  const r=await cache.get(url);
  bytes+=r.data.length;requests++;
  res.statusCode=r.status;
  if(r.fileid)res.setHeader('fileid',r.fileid);
  res.setHeader('Content-Type',path.endsWith('.js')?'text/javascript':path.endsWith('.wasm')?'application/wasm':'application/octet-stream');
  res.end(r.data);
 }catch(e){res.statusCode=500;res.end(String(e));}
});
await new Promise(resolve=>server.listen(8709,'127.0.0.1',resolve));
async function smoke(){
 const {WasmTexPdftexEngine}=await import('/lib/engine/wasmtex-engine.js');
 const {BibtexEngine}=await import('/lib/engine/bibtex-engine.js');
 const opts={assetBaseUrl:location.origin+'/',texliveVersion:'2026',texliveUrl:location.origin+'/2026/',disablePreambleSnapshot:true,persistentCache:false};
 const tex=new WasmTexPdftexEngine(opts),bib=new BibtexEngine(opts), phases=[];
 const doc='\\documentclass{article}\n\\usepackage{siunitx}\n\\begin{document}\nEngine comparison: \\qty{12}{\\metre}. Reference \\cite{knuth}.\n\\bibliographystyle{plain}\\bibliography{refs}\n\\end{document}\n';
 const data='@book{knuth, author={Donald E. Knuth}, title={The TeXbook}, publisher={Addison-Wesley}, year={1984}}\n';
 const start=performance.now();
 try{
  await tex.init();await tex.writeFile('main.tex',doc);tex.setMainFile('main.tex');
  await tex.writeFile('refs.bib',data);
  const first=await tex.compile();phases.push({stage:'tex1',success:first.success,log:first.log});
  if(!first.success) return {phases,elapsedMs:performance.now()-start};
  const aux=await tex.readFile('main.aux');
  await bib.init();await bib.writeFile('main.aux',aux);await bib.writeFile('refs.bib',data);
  const br=await bib.compile('main');phases.push({stage:'bibtex',...br});
  const bbl=await bib.readFile('main.bbl');
  if(!br.success||!bbl)return {phases,bbl,elapsedMs:performance.now()-start};
  await tex.writeFile('main.bbl',bbl);
  const second=await tex.compile();phases.push({stage:'tex2',success:second.success,log:second.log});
  const third=await tex.compile();phases.push({stage:'tex3',success:third.success,log:third.log});
  const pdf=third.pdf?btoa(Array.from(third.pdf,b=>String.fromCharCode(b)).join('')):null;
  const elapsedMs=performance.now()-start;
  const editStart=performance.now();
  await tex.writeFile('main.tex',doc.replace('Engine comparison:','Edited comparison:'));
  const edit=await tex.compile();
  phases.push({stage:'prose-edit',success:edit.success,log:edit.log,elapsedMs:performance.now()-editStart});
  return {phases,bbl,pdf,elapsedMs,doc,data};
 }finally{tex.terminate();bib.terminate();}
}
let driver;
const profile=mkdtempSync(join(tmpdir(),'komodoc-wasmtex-2026-'));
try{
 driver=await browser('chromium',profile,9711);
 await driver.navigate('http://127.0.0.1:8709/');
 await until('page',()=>driver.evaluate('document.readyState === "complete"'),10000);
 await driver.evaluate('globalThis.job={};('+smoke.toString()+')().then(result=>globalThis.job={done:true,result},e=>globalThis.job={done:true,error:String(e)});true');
 await until('compile',()=>driver.evaluate('globalThis.job.done'),180000);
 const job=await driver.evaluate('globalThis.job');
 if(job.error)throw Error(job.error);
 const report={release:manifest.releaseId,...job.result,bytes,requests,receipts};
 if(report.pdf)writeFileSync(join(evidenceDir,'wasmtex-2026.pdf'),Buffer.from(report.pdf,'base64'));
 if(report.doc)writeFileSync(join(evidenceDir,'wasmtex-2026.tex'),report.doc);
 writeFileSync(join(evidenceDir,'wasmtex-2026-result.json'),JSON.stringify({...report,pdf:report.pdf?'saved separately':null},null,2));
 console.log(JSON.stringify({release:report.release,phases:report.phases.map(p=>({stage:p.stage,success:p.success,log:p.log.slice(0,450),tail:p.log.slice(-450),elapsedMs:p.elapsedMs})),bbl:report.bbl,pdfBytes:report.pdf?Buffer.from(report.pdf,'base64').length:0,elapsedMs:report.elapsedMs,bytes,requests}));
}finally{await driver?.close();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));rmSync(profile,{recursive:true,force:true});}
