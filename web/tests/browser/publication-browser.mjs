// Browser acceptance for the real read-only publication Reader.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-publication-browser-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");
const dist = join(root, "web/dist");
assert.ok(existsSync(join(dist, "agent.js")), "run bun run build first");

writeFileSync(room, [
  "export function openRoom(slug,{onMessage,onConnected}) {",
  " window.roomSent=[];window.roomReceive=onMessage;",
  " queueMicrotask(()=>{onConnected(true);onMessage({type:'hello',comments:[]})});",
  " return {send(message){window.roomSent.push(message);return {ok:true}},sendLive(message){window.roomSent.push(message);return {ok:true}},close(){}}",
  "}",
].join("\n"));
writeFileSync(entry, [
  "import " + JSON.stringify(join(root, "web/src/styles/app.css")) + ";",
  "window.testErrors=[];",
  "addEventListener('error',event=>window.testErrors.push(event.error?.stack||event.message));",
  "addEventListener('unhandledrejection',event=>window.testErrors.push(event.reason?.stack||String(event.reason)));",
  "const nativeFetch=fetch.bind(globalThis);window.fetchRequests=[];",
  "globalThis.fetch=(url,init={})=>{window.fetchRequests.push({url:String(url),method:init.method||'GET'});return nativeFetch(url,init)};",
  "const {default:Reader}=await import(" + JSON.stringify(join(root, "web/src/components/Reader.svelte")) + ");",
  "const {mount}=await import(" + JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js")) + ");mount(Reader,{target:document.body});",
].join("\n"));

let shell, docs, tab, shellOrigin = "", docsOrigin = "", current = 1;
const shellRequests = [], docsRequests = [];
const publication = (id) => ({id: "pub-" + id, source_sha256: String(id).repeat(64),
  html_url: docsOrigin + "/published/paper/index.html?v=" + id});
try {
  await build({configFile:false,root:join(root,"web"),plugins:[
    tailwindcss(),svelte(),{name:"mock-room",enforce:"pre",resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    }}],build:{outDir:out,emptyOutDir:true,minify:false,sourcemap:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}},logLevel:"error"});

  docs = createServer((request,response) => {
    const url = new URL(request.url,"http://127.0.0.1"); docsRequests.push(url.pathname);
    if (url.pathname === "/published/paper/index.html") {
      response.setHeader("content-type","text/html");
      response.end('<!doctype html><body><p id="passage">Visible passage</p><script src="/agent.js?reader=' + encodeURIComponent(shellOrigin) + '"></script></body>');
      return;
    }
    const file = join(dist,url.pathname.slice(1));
    try { response.setHeader("content-type",file.endsWith(".css")?"text/css":"text/javascript");response.end(readFileSync(file)); }
    catch { response.statusCode=404;response.end(); }
  });
  await new Promise((resolve,reject)=>{docs.once("error",reject);docs.listen(0,"127.0.0.1",resolve)});
  docsOrigin = "http://127.0.0.1:" + docs.address().port;

  shell = createServer((request,response) => {
    const url = new URL(request.url,"http://127.0.0.1"); shellRequests.push(url.pathname);
    response.setHeader("content-type","application/json");
    if (url.pathname === "/api/me") { response.end(JSON.stringify({name:"Publication tester",providers:[]}));return; }
    if (url.pathname === "/api/documents/paper") {
      response.end(JSON.stringify({title:"Published paper",created_at:"publication-test",role:"commenter",source_format:"html",docs_origin:docsOrigin,commenting_as:"Publication tester"}));return;
    }
    if (url.pathname === "/api/documents/paper/publication") { response.end(JSON.stringify({publication:publication(current)}));return; }
    if (url.pathname.startsWith("/docs/")) {
      response.setHeader("content-type","text/html");
      response.end('<!doctype html><html data-theme="librepaper"><body><script type="module" src="/check.js"></script></body></html>');return;
    }
    const file=join(out,url.pathname.slice(1));
    try { response.setHeader("content-type",file.endsWith(".css")?"text/css":"text/javascript");response.end(readFileSync(file)); }
    catch { response.statusCode=404;response.end(); }
  });
  await new Promise((resolve,reject)=>{shell.once("error",reject);shell.listen(0,"127.0.0.1",resolve)});
  shellOrigin = "http://127.0.0.1:" + shell.address().port;

  tab=await browser("chromium",join(temp,"profile"),24000+Math.floor(Math.random()*1000));
  await tab.resize(1280,900);await tab.navigate(shellOrigin+"/docs/paper");
  const flush=()=>tab.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  const frameMessage=async(message)=> {
    await tab.evaluate("window.dispatchEvent(new MessageEvent('message',{origin:" + JSON.stringify(docsOrigin) + ",source:document.querySelector('.viewport iframe').contentWindow,data:{librepaper:true,..." + JSON.stringify(message) + "}}))");await flush();
  };
  const select=()=>frameMessage({type:"selection",selector:{exact:"Visible passage",prefix:"",suffix:"",position:0},rect:{top:30,bottom:50,left:30,right:130}});
  const draft=async(text)=>{await tab.evaluate("(()=>{const field=document.querySelector('#commentForm textarea');field.value="+JSON.stringify(text)+";field.dispatchEvent(new Event('input',{bubbles:true}))})()");await flush();};

  await until("published iframe",()=>tab.evaluate("Boolean(document.querySelector('.viewport iframe')&&window.roomReceive)"),10000);
  await until("published frame agent",()=>Promise.resolve(docsRequests.includes("/agent.js")),10000);
  assert.equal(await tab.evaluate("document.querySelector('.viewport iframe').src.startsWith("+JSON.stringify(docsOrigin)+")"),true,"published HTML is on its separate documents origin");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('.editorpane'))"),false,"commenter has no source pane");
  assert.equal(await tab.evaluate("document.body.innerText.includes('Download project')"),false,"commenter has no project download");
  assert.equal(await tab.evaluate("window.roomSent.some(message=>message.type==='y-open')"),false,"commenter does not open source state");
  assert.ok(shellRequests.every(path=>!/\/(?:source|snapshot|state|history)$/.test(path)), "restricted requests: "+shellRequests);

  // Commenters can configure dictation from both menu layouts without
  // gaining editor settings. Changing a model persists on this browser.
  let chosenModel;
  for (const width of [1280, 390]) {
    await tab.resize(width,900);await flush();
    const menu = width === 1280 ? "Tools" : "Menu";
    await tab.evaluate("[...document.querySelectorAll('.menubar-item')].find(button=>button.textContent.trim()==="+JSON.stringify(menu)+").click()");
    await until("settings menu item",()=>tab.evaluate("[...document.querySelectorAll('[role=menuitem]')].some(item=>item.checkVisibility()&&item.textContent.trim()==='Settings…')"),3000);
    assert.equal(await tab.evaluate("[...document.querySelectorAll('[role=menuitem]')].some(item=>item.textContent.includes('Compile now'))"),false);
    await tab.evaluate("[...document.querySelectorAll('[role=menuitem]')].find(item=>item.checkVisibility()&&item.textContent.trim()==='Settings…').dispatchEvent(new PointerEvent('pointermove',{bubbles:true,pointerType:'mouse'}))");await flush();
    await tab.evaluate("[...document.querySelectorAll('[role=menuitem]')].find(item=>item.checkVisibility()&&item.textContent.trim()==='Settings…').click()");
    await until("dictation settings",()=>tab.evaluate("Boolean(document.querySelector('[role=dialog][data-state=open] select[aria-label=\"Dictation model\"]'))"),3000);
    assert.deepEqual(await tab.evaluate("[...document.querySelectorAll('.settings-nav-item')].map(item=>item.textContent.trim())"),["Dictation"]);
    if (chosenModel) {
      assert.equal(await tab.evaluate("document.querySelector('select[aria-label=\"Dictation model\"]').value"),chosenModel);
    }
    chosenModel=await tab.evaluate("(()=>{const select=document.querySelector('select[aria-label=\"Dictation model\"]');select.value=[...select.options].find(option=>option.value!==select.value).value;select.dispatchEvent(new Event('change',{bubbles:true}));return select.value})()");
    await flush();
    assert.equal(await tab.evaluate("localStorage.getItem('librepaper-dictation-model')"),chosenModel);
    await tab.evaluate("document.querySelector('[role=dialog][data-state=open] button[aria-label=Close]').click()");
    await until("settings closed",()=>tab.evaluate("!document.querySelector('[role=dialog][data-state=open]')"),3000);
  }
  await tab.resize(1280,900);await flush();

  await select();await until("selection bar",()=>tab.evaluate("Boolean(document.querySelector('#selectionbar'))"),3000);
  await tab.evaluate("document.querySelector('#selectionbar button').click()");
  await until("comment form",()=>tab.evaluate("Boolean(document.querySelector('#commentForm textarea'))"),3000);
  await draft("Comment on the first publication");
  await tab.evaluate("document.querySelector('#commentForm').requestSubmit()");await flush();
  const first=await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').at(-1)");
  assert.equal(first.publication_id,"pub-1");assert.equal(first.body,"Comment on the first publication");

  await select();await tab.evaluate("document.querySelector('#selectionbar button').click()");
  await until("second comment form",()=>tab.evaluate("Boolean(document.querySelector('#commentForm textarea'))"),3000);
  await draft("Draft retained for the new publication");
  const publicationFetches=await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/publication')).length");
  const oldFrame=await tab.evaluate("document.querySelector('.viewport iframe').src");
  current=2;await tab.evaluate("window.roomReceive({type:'publication-updated',publication_id:'pub-2'})");await flush();
  assert.equal(await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/publication')).length"),publicationFetches,"notice does not refetch HTML");
  assert.equal(await tab.evaluate("document.querySelector('.viewport iframe').src"),oldFrame,"notice does not navigate");
  assert.equal(await tab.evaluate("document.querySelector('#commentForm textarea').value"),"Draft retained for the new publication");
  const sent=await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').length");
  await tab.evaluate("document.querySelector('#commentForm').requestSubmit()");await flush();
  assert.equal(await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').length"),sent,"stale selection is refused");
  assert.equal(await tab.evaluate("document.querySelector('#commentForm textarea').value"),"Draft retained for the new publication","stale submit keeps draft");

  await tab.evaluate("[...document.querySelectorAll('button')].find(button=>button.textContent.includes('Refresh')).click()");
  await until("refreshed publication",()=>tab.evaluate("document.querySelector('.viewport iframe').src.includes('v=2')"),5000);
  await select();await tab.evaluate("document.querySelector('#commentForm').requestSubmit()");await flush();
  const refreshed=await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').at(-1)");
  assert.equal(refreshed.publication_id,"pub-2");assert.equal(refreshed.body,"Draft retained for the new publication");
  assert.equal(await tab.evaluate("window.fetchRequests.some(request => /\\/publication\\/(?:prepare|activate|objects\\/)/.test(request.url))"),false,
    "reading, annotating, and a publication notice never start a publication upload");
  assert.deepEqual(await tab.evaluate("window.testErrors"),[]);
  console.log("publication-browser: real commenter Reader publication and draft behavior passed");
} finally {
  await tab?.close();
  await new Promise(resolve=>shell ? shell.close(resolve) : resolve());
  await new Promise(resolve=>docs ? docs.close(resolve) : resolve());
  rmSync(temp,{recursive:true,force:true});
}
