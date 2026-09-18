// Browser acceptance for the real read-only bundle Reader.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-bundle-browser-"));
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
const bundle = (id) => ({id: "pub-" + id, source_sha256: String(id).repeat(64),
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
    if (url.pathname === "/api/me") { response.end(JSON.stringify({name:"Bundle tester",providers:[]}));return; }
    if (url.pathname === "/api/documents/paper") {
      response.end(JSON.stringify({title:"Published paper",created_at:"bundle-test",role:"commenter",source_format:"html",docs_origin:docsOrigin,commenting_as:"Bundle tester"}));return;
    }
    if (url.pathname === "/api/documents/paper/bundle") { response.end(JSON.stringify({bundle:bundle(current)}));return; }
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
  const draft=async(text)=>{await tab.evaluate("(()=>{const field=document.querySelector('#composer textarea');field.value="+JSON.stringify(text)+";field.dispatchEvent(new Event('input',{bubbles:true}))})()");await flush();};

  await until("published iframe",()=>tab.evaluate("Boolean(document.querySelector('.viewport iframe')&&window.roomReceive)"),10000);
  await until("published frame agent",()=>Promise.resolve(docsRequests.includes("/agent.js")),10000);
  assert.equal(await tab.evaluate("document.querySelector('.viewport iframe').src.startsWith("+JSON.stringify(docsOrigin)+")"),true,"published HTML is on its separate documents origin");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('.editorpane'))"),false,"commenter has no source pane");
  assert.equal(await tab.evaluate("document.body.innerText.includes('Download project')"),false,"commenter has no project download");
  assert.equal(await tab.evaluate("window.roomSent.some(message=>message.type==='doc-open')"),false,"commenter does not open source state");
  assert.ok(shellRequests.every(path=>!/\/(?:source|snapshot|state|history)$/.test(path)), "restricted requests: "+shellRequests);

  await select();await until("selection bar",()=>tab.evaluate("Boolean(document.querySelector('#selectionbar'))"),3000);
  await tab.evaluate("document.querySelector('#selectionbar button').click()");
  await until("comment composer",()=>tab.evaluate("Boolean(document.querySelector('#composer textarea'))"),3000);
  await draft("Comment on the first bundle");
  await tab.evaluate("document.querySelector('#composer [data-send]').click()");await flush();
  const first=await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').at(-1)");
  assert.equal(first.bundle_id,"pub-1");assert.equal(first.body,"Comment on the first bundle");

  await select();await tab.evaluate("document.querySelector('#selectionbar button').click()");
  await until("second comment composer",()=>tab.evaluate("Boolean(document.querySelector('#composer textarea'))"),3000);
  await draft("Draft retained for the new bundle");
  const bundleFetches=await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/bundle')).length");
  const oldFrame=await tab.evaluate("document.querySelector('.viewport iframe').src");
  current=2;await tab.evaluate("window.roomReceive({type:'bundle-updated',bundle_id:'pub-2'})");await flush();
  assert.equal(await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/bundle')).length"),bundleFetches,"notice does not refetch HTML");
  assert.equal(await tab.evaluate("document.querySelector('.viewport iframe').src"),oldFrame,"notice does not navigate");
  assert.equal(await tab.evaluate("document.querySelector('#composer textarea').value"),"Draft retained for the new bundle");
  const sent=await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').length");
  await tab.evaluate("document.querySelector('#composer [data-send]').click()");await flush();
  assert.equal(await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').length"),sent,"stale selection is refused");
  assert.equal(await tab.evaluate("document.querySelector('#composer textarea').value"),"Draft retained for the new bundle","stale submit keeps draft");

  await tab.evaluate("[...document.querySelectorAll('button')].find(button=>button.textContent.includes('Refresh')).click()");
  await until("refreshed bundle",()=>tab.evaluate("document.querySelector('.viewport iframe').src.includes('v=2')"),5000);
  await select();await tab.evaluate("document.querySelector('#composer [data-send]').click()");await flush();
  const refreshed=await tab.evaluate("window.roomSent.filter(message=>message.type==='comment').at(-1)");
  assert.equal(refreshed.bundle_id,"pub-2");assert.equal(refreshed.body,"Draft retained for the new bundle");
  assert.equal(await tab.evaluate("window.fetchRequests.some(request => /\\/bundle\\/(?:prepare|activate|objects\\/)/.test(request.url))"),false,
    "reading, annotating, and a bundle notice never start a bundle upload");
  assert.deepEqual(await tab.evaluate("window.testErrors"),[]);
  console.log("bundle-browser: real commenter Reader bundle and draft behavior passed");
} finally {
  await tab?.close();
  await new Promise(resolve=>shell ? shell.close(resolve) : resolve());
  await new Promise(resolve=>docs ? docs.close(resolve) : resolve());
  rmSync(temp,{recursive:true,force:true});
}
