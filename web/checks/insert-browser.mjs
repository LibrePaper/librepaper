// Real Skeleton controls and CodeMirror/Yjs transactions, including a peer
// changing the document while the insertion dialog owns keyboard focus.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join, extname } from "node:path";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-insert-browser-"));
const output = join(temporary, "build");
const imp = (path) => JSON.stringify(join(root, path));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import { tick } from ${imp("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${imp("node_modules/svelte/src/legacy/legacy-client.js")};
import { EditorView } from ${imp("node_modules/@codemirror/view/dist/index.js")};
import { yUndoManagerKeymap } from ${imp("node_modules/y-codemirror.next/src/y-undomanager.js")};
import Editor from ${imp("src/components/Editor.svelte")};
import InsertMenu from ${imp("src/components/InsertMenu.svelte")};
import { join as joinSession } from ${imp("src/lib/collab.js")};
const session = joinSession({send: () => {}, mayEdit: true});
const entries = [{key:'smith2020',author:['Jane Smith'],title:'A Study of Rivers',year:'2020'}, {key:'jones2021',author:['Alice Jones'],title:'Mountains',year:'2021'}];
let component, menu, file;
window.setupInsert = async (format, source = 'Before AFTER') => {
  menu?.$destroy(); component?.$destroy();
  const extension = {latex:'tex',typst:'typ',markdown:'md',quarto:'qmd'}[format];
  file = session.addText('paper-' + Date.now() + '.' + extension, source);
  session.setMain(file);
  component = createClassComponent({component:Editor,target:document.getElementById('editor'),props:{session,format,file,analyze:async()=>({entries,diagnostics:[]})}});
  await tick();
  menu = createClassComponent({component:InsertMenu,target:document.getElementById('menu'),props:{
    getContext:()=>({...component.getInsertContext(),bibliography:entries,files:[{path:'refs.bib',text:'@article{smith2020,title={Rivers},author={Smith, Jane},year={2020}}'}]}),
    oninsert:(result,context)=>{ const ok=component.applyInsertResult(result,context); if(!ok) throw Error('Insertion target changed'); return ok; },
    onupload:async()=> 'images/upload.png'
  }});
  await tick();
};
const view=()=>EditorView.findFromDOM(document.querySelector('.cm-editor'));
window.insertState=()=>({text:view().state.doc.toString(),selection:view().state.sliceDoc(view().state.selection.main.from,view().state.selection.main.to),focused:view().hasFocus});
window.selectInsert=(from,to=from)=>{view().dispatch({selection:{anchor:from,head:to}});view().focus();};
window.remoteInsert=async()=>{ session.doc.transact(()=>session.textOf(file).insert(0,'REMOTE '),'remote');await tick(); };
window.undoInsert=()=>yUndoManagerKeymap[0].run(view());
window.targetChecks=async()=>{
  const context=component.getInsertContext();
  const another=session.addText('other.md','Other document');
  component.$set({file:another});await tick();
  const switched=component.applyInsertResult({text:'BAD'},context);
  component.$set({file});await tick();
  component.$set({editable:false});await tick();
  const readonly=component.applyInsertResult({text:'BAD'},context);
  component.$set({editable:true});await tick();
  return {switched,readonly};
};
window.insertReady=true;
`);
let server, page;
try {
  await build({configFile:false,root,plugins:[svelte()],logLevel:"error",build:{outDir:output,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"insert-check.js"}}});
  server=createServer((request,response)=>{
    const pathname = new URL(request.url,"http://localhost").pathname;
    const file=join(output,pathname);
    if(pathname!=="/" && existsSync(file)) {
      response.setHeader("Content-Type",extname(file)===".css"?"text/css":"text/javascript");response.end(readFileSync(file));
    } else response.end('<!doctype html><html><head><meta charset="utf-8"></head><body><div id="menu"></div><div id="editor"></div><script type="module" src="/insert-check.js"></script></body></html>');
  });
  await new Promise((resolve,reject)=>{server.once("error",reject);server.listen(0,"127.0.0.1",resolve);});
  page=await browser("chromium",join(temporary,"profile"),24000+Math.floor(Math.random()*10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("Insert test loaded",()=>page.evaluate("window.insertReady"),10000);
  const evaluate=(code)=>page.evaluate(code);
  const click=async(selector)=>{ await until(selector,()=>evaluate(`Boolean(document.querySelector(${JSON.stringify(selector)}))`),5000); await evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`); };
  const choose=async(id)=>{ await click('[data-scope="menu"][data-part="trigger"]'); await click(`[data-scope="menu"][data-part="item"][data-value="${id}"]`); };
  const confirm=async()=>{ await evaluate(`[...document.querySelectorAll('[role="dialog"] button')].find(x=>x.textContent.trim()==='Insert').click()`); await until("dialog closed",()=>evaluate(`!document.querySelector('[role="dialog"]')`),5000); };
  const field=async(label,value)=>evaluate(`(()=>{const parent=[...document.querySelectorAll('[role="dialog"] label')].find(x=>x.textContent.trim().startsWith(${JSON.stringify(label)}));const input=parent?.querySelector('input,select');if(!input)throw Error('Missing field '+${JSON.stringify(label)});input.value=${JSON.stringify(String(value))};input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));})()`);

  for (const [format,expected] of [["latex",/\\begin\{tabular\}/],["typst",/#table\(/],["markdown",/\|/],["quarto",/\|/]]) {
    await evaluate(`setupInsert(${JSON.stringify(format)})`);
    await evaluate("selectInsert(7)");
    await choose("table");
    await until("table dialog",()=>evaluate(`Boolean(document.querySelector('[role="dialog"]'))`),5000);
    await field("Rows",2);await field("Columns",2);
    await confirm();
    assert.match((await evaluate("insertState()")).text,expected,format+" generates its own table syntax");
  }
  await evaluate("setupInsert('markdown')");await evaluate("selectInsert(7)");
  await choose("table");await until("dialog visible",()=>evaluate(`Boolean(document.querySelector('[role="dialog"]'))`),5000);
  await evaluate("remoteInsert()");await confirm();
  assert.match((await evaluate("insertState()")).text,/^REMOTE Before [\s\S]*AFTER$/, "dialog inserts at peer-adjusted caret");
  await evaluate("undoInsert()");
  assert.equal((await evaluate("insertState()")).text,"REMOTE Before AFTER","one undo removes insertion and preserves peer edit");
  await evaluate("setupInsert('markdown','alpha\\nbeta')");await evaluate("selectInsert(0,10)");
  await choose("bulleted-list");
  await until("selection converted to list",async()=>/^\s*- alpha\n- beta/.test((await evaluate("insertState()")).text),5000);
  assert.deepEqual(await evaluate("targetChecks()"),{switched:false,readonly:false});
  console.log("insert-browser: Skeleton table dialogs in all four formats, selection wrapping, remote anchors, undo, file switch and readonly guards passed");
} finally {await page?.close();server?.close();rmSync(temporary,{recursive:true,force:true});}
