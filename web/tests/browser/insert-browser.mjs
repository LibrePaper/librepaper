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
import { browser, until } from "../../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
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
window.testErrors=[];addEventListener("error",e=>window.testErrors.push(e.message));addEventListener("unhandledrejection",e=>window.testErrors.push(String(e.reason)));
const session = joinSession({send: () => {}, mayEdit: true});
const entries = [{key:'smith2020',authors:['Jane Smith'],title:'A Study of Rivers',year:'2020'}, {key:'jones2021',authors:['Alice Jones'],title:'Mountains',year:'2021'}];
let component, menu, file;
window.setupInsert = async (format, source = null) => {
  menu?.$destroy(); component?.$destroy();
  source ??= format === 'latex' ? ${JSON.stringify('\\documentclass{article}\n\\begin{document}\nBefore AFTER\n\\end{document}')} : 'Before AFTER';
  const extension = {latex:'tex',typst:'typ',markdown:'md',quarto:'qmd'}[format];
  file = session.addText('paper-' + Date.now() + '.' + extension, source);
  session.setMain(file);
  component = createClassComponent({component:Editor,target:document.getElementById('editor'),props:{session,format,file,analyze:async()=>({entries,diagnostics:[]})}});
  await tick();
  menu = createClassComponent({component:InsertMenu,target:document.getElementById('menu'),props:{
    getContext:()=>({...component.getInsertContext(),bibliography:entries,files:[{path:'refs.bib',text:'@article{smith2020,title={Rivers},author={Smith, Jane},year={2020}}'},{path:'images/river.png',url:'data:image/png;base64,preview'}]}),
    oninsert:(result,context)=>{ const ok=component.applyInsertResult(result,context); if(!ok) throw Error('Insertion target changed'); return ok; },
    onupload:async()=> 'images/upload.png',onfocus:()=>component.focus(),oncancel:context=>component.releaseInsertContext(context)
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
window.atomicSetupCheck=async()=>{
  const main=session.addText('setup-main.tex','\\\\documentclass{article}\\n\\\\begin{document}\\n\\\\input{chapter}\\n\\\\end{document}');
  const chapter=session.addText('chapter.tex','BEFORE AFTER');session.setMain(main);
  component.$set({file:chapter,format:'latex'});await tick();
  view().dispatch({selection:{anchor:7}});
  const c=component.getInsertContext();const oldMain=session.textOf(main).toString();
  const at=oldMain.indexOf('\\\\begin{document}');
  const ok=component.applyInsertResult({text:'INSERTED ',selection:{anchor:0,head:8},additionalEdits:[{path:'setup-main.tex',from:at,to:at,insert:'\\\\usepackage{graphicx}\\n'}]},c);
  const applied=ok&&session.textOf(chapter).toString()==='BEFORE INSERTED AFTER'&&session.textOf(main).toString().includes('graphicx');
  window.undoInsert();await tick();
  const undone=session.textOf(chapter).toString()==='BEFORE AFTER'&&session.textOf(main).toString()===oldMain;
  view().dispatch({selection:{anchor:0,head:6}});const selectionContext=component.getInsertContext();
  session.doc.transact(()=>session.textOf(chapter).insert(3,'PEER'),'remote');await tick();
  const conflict=component.applyInsertResult({text:'BAD'},selectionContext);
  return {applied,undone,conflict,text:session.textOf(chapter).toString()};
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
  const click=async(selector)=>{ await until(selector,()=>evaluate(`Boolean(document.querySelector(${JSON.stringify(selector)}))`),5000); await evaluate(`document.querySelector(${JSON.stringify(selector)}).dispatchEvent(new PointerEvent("pointermove",{bubbles:true,pointerType:"mouse"}));document.querySelector(${JSON.stringify(selector)}).dispatchEvent(new PointerEvent("pointerdown",{bubbles:true,pointerType:"mouse"}));document.querySelector(${JSON.stringify(selector)}).click()`); };
  const choose=async(id)=>{ await click('[data-scope="menu"][data-part="trigger"]'); await until("menu ready",()=>evaluate(`document.querySelector('[data-scope="menu"][data-part="trigger"]')?.getAttribute('data-state')==='open'`),5000); const state=await evaluate(`(()=>{const x=document.querySelector('[data-scope="menu"][data-part="item"][data-value=${JSON.stringify(id)}]');return {disabled:x?.getAttribute('data-disabled'),text:x?.textContent}})()`);assert.equal(state.disabled,null,`${id} insertion is enabled: ${state.text}`); await click(`[data-scope="menu"][data-part="item"][data-value="${id}"]`); };
  const dialogTitle=async()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim() || ''`);
  const confirm=async()=>{ await evaluate(`[...document.querySelectorAll('[role="dialog"][data-state="open"] button')].find(x=>x.textContent.trim()==='Insert').click()`); await until("dialog closed",()=>evaluate(`!document.querySelector('[role="dialog"][data-state="open"]')`),5000); };
  const cancel=async()=>{ await evaluate(`[...document.querySelectorAll('[role="dialog"][data-state="open"] button')].find(x=>x.textContent.trim()==='Cancel').click()`); await until("dialog cancelled",()=>evaluate(`!document.querySelector('[role="dialog"][data-state="open"]')`),5000); };
  const field=async(label,value)=>evaluate(`(()=>{const dialog=document.querySelector('[role="dialog"][data-state="open"]');const wanted=${JSON.stringify(label.toLowerCase())};const parent=[...dialog.querySelectorAll('label')].find(x=>x.textContent.trim().toLowerCase().startsWith(wanted));const input=parent?.querySelector('input,select') || [...dialog.querySelectorAll('input,select')].find(x=>x.getAttribute('aria-label')?.toLowerCase().includes(wanted));if(!input)throw Error('Missing field '+${JSON.stringify(label)}+' labels='+[...dialog.querySelectorAll('label')].map(x=>x.textContent.trim()).join('|')+' html='+dialog.innerHTML.slice(0,1000));input.value=${JSON.stringify(String(value))};input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  const checkCitation=async(key)=>evaluate(`(()=>{const input=document.querySelector('[role="dialog"][data-state="open"] input[type="checkbox"][value=${JSON.stringify(key)}]');if(!input)throw Error('Missing citation '+${JSON.stringify(key)});input.click();})()`);

  for (const [format,expected] of [["latex",/\\begin\{tabular\}/],["typst",/#table\(/],["markdown",/\|/],["quarto",/\|/]]) {
    await evaluate(`setupInsert(${JSON.stringify(format)})`);
    await evaluate("selectInsert(insertState().text.indexOf('Before') + 7)");
    await choose("table");
    await until("table dialog",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim()==='Table'`),5000);
    await field("Rows",2);await field("Columns",2);
    await confirm();
    assert.match((await evaluate("insertState()")).text,expected,format+" generates its own table syntax");
  }
  await evaluate("setupInsert('markdown')");await evaluate("selectInsert(7)");
  await choose("table");await until("dialog visible",()=>evaluate(`Boolean(document.querySelector('[role="dialog"][data-state="open"]'))`),5000);
  // The dialog is portalled to <body>: mounted under a navbar with a backdrop filter, a `fixed` dialog would otherwise be confined to the bar. (This harness has no stylesheet, so only the mount point is checked here.)
  assert.equal(await evaluate(`document.querySelector('[role="dialog"][data-state="open"]').closest("#menu") === null`),true,"dialog renders outside the menu mount");
  await evaluate("remoteInsert()");await confirm();
  assert.match((await evaluate("insertState()")).text,/^REMOTE Before [\s\S]*AFTER$/, "dialog inserts at peer-adjusted caret");
  await evaluate("undoInsert()");
  assert.equal((await evaluate("insertState()")).text,"REMOTE Before AFTER","one undo removes insertion and preserves peer edit");
  // Cancellation must release the captured target without mutating source.
  await evaluate("setupInsert('markdown','Before AFTER')");await evaluate("selectInsert(7)");
  await choose("table");
  await until("cancellable table dialog",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim()==='Table'`),5000);
  await cancel();
  assert.equal((await evaluate("insertState()" )).text,"Before AFTER","cancelling an insertion leaves source unchanged");

  // Citation search supports multiple selections and narrative style.
  await evaluate("setupInsert('markdown','---\\nbibliography: refs.bib\\n---\\n\\nBefore AFTER')");await evaluate("selectInsert(insertState().text.indexOf('Before') + 7)");
  await choose("citation");await until("citation dialog",()=>evaluate(`Boolean(document.querySelector('[role="dialog"][data-state="open"]'))`),5000);assert.equal(await dialogTitle(),"Citation","citation action opens the citation dialog");
  await field("Search bibliography","Rivers");await checkCitation("smith2020");
  await field("Search bibliography","");await checkCitation("jones2021");await field("Citation style","narrative");await confirm();
  assert.match((await evaluate("insertState()")).text,/\[?@smith2020; @jones2021\]?/,"citation inserts both selected references in narrative form");

  // Cross-reference and label dialogs use actual gathered target ids.
  await evaluate(`setupInsert('markdown',${JSON.stringify("# Intro\n\nBefore AFTER")})`);await evaluate("selectInsert(15)");
  await choose("cross-reference");await until("cross-reference dialog",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim()==='Cross-reference'`),5000);
  const target=await evaluate(`document.querySelector('[role="dialog"][data-state="open"] select')?.options[1]?.value`);assert.ok(target,"cross-reference target is available");await field("Reference",target);await confirm();
  assert.match((await evaluate("insertState()" )).text,/\[[^\]]+\]\(#.+\)/,"cross-reference inserts a link");
  await evaluate("setupInsert('markdown','Before AFTER')");await evaluate("selectInsert(7)");await choose("label");await until("label dialog",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim()==='Label / anchor'`),5000);await field("Label","sec-intro");await confirm();assert.match((await evaluate("insertState()" )).text,/sec-intro/);

  // Invalid dimensions are reported in the dialog and do not close it.
  await evaluate("setupInsert('markdown','Before AFTER')");await evaluate("selectInsert(7)");await choose("table");await until("invalid table dialog",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim()==='Table'`),5000);await field("Rows",0);await evaluate(`[...document.querySelectorAll('[role="dialog"][data-state="open"] button')].find(x=>x.textContent.trim()==='Insert').click()`);await until("dimension error",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [role="alert"], .insert-error')?.textContent.includes('Rows')`),5000);assert.equal(await dialogTitle(),"Table");await cancel();

  // Project image selection provides a preview and inserts the chosen source.
  await evaluate("setupInsert('markdown','Before AFTER')");await evaluate("selectInsert(7)");await choose("figure");await until("figure dialog",()=>evaluate(`document.querySelector('[role="dialog"][data-state="open"] [data-part="title"]')?.textContent.trim()==='Image / figure'`),5000);await field("Project image","images/river.png");assert.equal(await evaluate(`Boolean(document.querySelector('[role="dialog"][data-state="open"] img.insert-image-preview'))`),true);await confirm();assert.match((await evaluate("insertState()" )).text,/images\/river\.png/);
  await evaluate("setupInsert('markdown','Before AFTER')");await evaluate("selectInsert(7)");await choose('figure');
  await until('upload dialog',()=>evaluate('Boolean(document.querySelector(\'[role="dialog"][data-state="open"]\'))'),5000);
  await evaluate('(()=>{const input=document.querySelector(\'[role="dialog"][data-state="open"] input[type=file]\');const data=new DataTransfer();data.items.add(new File(["bytes"],"upload.png",{type:"image/png"}));input.files=data.files;input.dispatchEvent(new Event("change",{bubbles:true}));})()');
  await until('upload path',()=>evaluate('document.querySelector(\'[role="dialog"][data-state="open"] select\')?.value === "images/upload.png"'),5000);
  await confirm();assert.match((await evaluate('insertState()')).text,/images\/upload\.png/);
  await evaluate("setupInsert('markdown','alpha\\nbeta')");await evaluate("selectInsert(0,10)");
  await choose("bulleted-list");
  await until("selection converted to list",async()=>/^\s*- alpha\n- beta/.test((await evaluate("insertState()")).text),5000);
  assert.deepEqual(await evaluate("targetChecks()"),{switched:false,readonly:false});
  await evaluate("setupInsert('markdown','Before')");await evaluate("selectInsert(6)");await choose("footnote");await until("footnote inserted",()=>evaluate("insertState().text.includes('[^note-1]')"),5000);assert.match((await evaluate("insertState()")).text,/^Before\[\^note-1\]\n\n\[\^note-1\]: Note\./);
  assert.deepEqual(await evaluate("atomicSetupCheck()"),{applied:true,undone:true,conflict:false,text:"BEFPEERORE AFTER"});
  await evaluate("setupInsert('quarto')");await evaluate("selectInsert(7)");await choose("toc");await until("Quarto TOC",()=>evaluate(`Boolean(document.querySelector('[role="dialog"][data-state="open"]'))`),5000);await confirm();assert.match((await evaluate("insertState()")).text,/toc: true/);
  console.log("insert-browser: Skeleton table dialogs in all four formats, selection wrapping, remote anchors, undo, file switch and readonly guards passed");
} catch(error) {console.error(await page?.evaluate("({errors:window.testErrors,dialogs:[...document.querySelectorAll('[role=dialog]')].map(x=>({title:x.textContent,state:x.dataset.state})),documentText:document.querySelector('.cm-content')?.textContent})"));throw error;} finally {await page?.close();server?.close();rmSync(temporary,{recursive:true,force:true});}
