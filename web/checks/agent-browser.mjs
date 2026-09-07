// Exercise the actual Svelte live-agent panel in Chromium.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "komodoc-agent-browser-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Agent from ${JSON.stringify(join(root, "src/components/Agent.svelte"))};
window.calls = [];
window.sockets = [];
window.fetch = async (url, init) => {
  const suffix = new URL(url, location.href).pathname.split('/chat')[1];
  window.calls.push({ suffix, body:init.body ? JSON.parse(init.body) : {}, headers:{...init.headers} });
  if (suffix === '') return Response.json({id:'conversation-'+window.calls.length,token:'secret-token'});
  if (init.method === 'DELETE') return Response.json({deleted:true});
  throw new Error('unexpected live chat request');
};
window.WebSocket = class {
  static OPEN = 1;
  readyState = 1;
  constructor(url) { this.url=url; this.sent=[]; window.sockets.push(this); queueMicrotask(()=>this.onopen?.()); }
  send(raw) {
    const frame=JSON.parse(raw); this.sent.push(frame);
    if (frame.type === 'join') queueMicrotask(()=>{ this.emit({type:'ready',browser:true,listening:false}); this.emit({type:'presence',browser:true,listening:true}); });
    if (frame.type === 'message') queueMicrotask(()=>{
      this.emit({type:'message',message:{id:frame.id,role:'user',text:frame.text,context:frame.context}});
      this.emit({type:'ack',id:frame.id});
      this.emit({type:'message',message:{id:'agent-reply',role:'agent',text:'<img src=x onerror="window.injected=true">'}});
    });
  }
  emit(frame) { this.onmessage?.({data:JSON.stringify(frame)}); }
  close() { this.readyState=3; this.onclose?.(); }
};
let component;
window.remount = async () => {
  if (component) await unmount(component);
  component = mount(Agent, {target:document.body,props:{slug:'paper',link:location.origin+'/docs/paper#k=secret',path:'paper.md',selection:{exact:'A passage'}}});
};
window.remount();
`);

let server, page;
try {
  await build({ configFile:false, root, plugins:[svelte(),tailwindcss()], logLevel:"error",
    build:{ outDir:join(temporary,"build"), lib:{entry,formats:["es"],fileName:()=>"panel.js"} } });
  server=createServer((request,response)=>{
    if(request.url==='/komodoc-web.css'){response.setHeader('content-type','text/css');response.end(readFileSync(join(temporary,'build/komodoc-web.css')));return;}
    response.setHeader("content-type",request.url==="/panel.js"?"text/javascript":"text/html");
    response.end(request.url==="/panel.js"?readFileSync(join(temporary,"build/panel.js")):'<!doctype html><html data-theme="komodoc"><head><link rel="stylesheet" href="/komodoc-web.css"><style>body{display:flex;height:700px;width:360px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve,reject)=>{server.once("error",reject);server.listen(0,"127.0.0.1",resolve);});
  page=await browser("chromium",join(temporary,"profile"),22000+Math.floor(Math.random()*10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("live agent panel",()=>page.evaluate("Boolean(document.querySelector('.agent-panel textarea[aria-label=Message]'))"),10000);
  await until("agent listening",()=>page.evaluate('document.querySelector("[role=status]")?.textContent==="Listening"'),10000);
  assert.equal(await page.evaluate('window.sockets[0].url.includes("secret-token")'),false);
  assert.equal(await page.evaluate('new URL(window.sockets[0].url).searchParams.get("k")'),"secret");
  assert.deepEqual(await page.evaluate('window.sockets[0].sent[0]'),{type:"join",token:"secret-token",role:"user"});

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='Explain this';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));})()`);
  await until("agent reply",()=>page.evaluate('document.querySelector("[role=log]")?.textContent.includes("<img")'),10000);
  assert.equal(await page.evaluate("Boolean(window.injected||document.querySelector('[role=log] img'))"),false);
  const posted=await page.evaluate('window.sockets[0].sent.find(frame=>frame.type==="message")');
  assert.equal(posted.context.file,"paper.md");
  assert.equal(posted.context.selection,"A passage");

  await page.evaluate(`(()=>{const clipboard={writeText:value=>{window.copiedInstructions=value;return Promise.resolve();}};Object.defineProperty(navigator,'clipboard',{configurable:true,value:clipboard});document.querySelector('.agent-actions button').click();})()`);
  await until("instructions copied",()=>page.evaluate('typeof window.copiedInstructions==="string"'),1000);
  assert.match(await page.evaluate("window.copiedInstructions"),/komodoc agent chat watch/);
  assert.doesNotMatch(await page.evaluate("window.copiedInstructions"),/--after/);

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='First';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea[placeholder]').value"),"First\n");

  await page.evaluate("window.remount()");
  await until("new live channel",()=>page.evaluate("window.sockets.length===2"),10000);
  assert.equal(await page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'),false);
  assert.equal(await page.evaluate("window.calls.filter(call=>call.suffix==='').length"),2);
  console.log("agent-browser: live socket, context, escaped text, CLI handoff and no remount replay passed");
} finally {
  await page?.close();
  if(server) await new Promise(resolve=>server.close(resolve));
  rmSync(temporary,{recursive:true,force:true});
}
