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
const harness = join(temporary, "Harness.svelte");
writeFileSync(harness, `
<script>
import Agent from ${JSON.stringify(join(root, "src/components/Agent.svelte"))};
let values=$state({slug:'paper',link:location.origin+'/docs/paper#k=secret',path:'paper.md',selection:{exact:'A passage'}});
export function updateProps(next){ values={...values,...next}; }
</script>
<Agent {...values} />
`);
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
window.calls = [];
window.sockets = [];
window.fetch = async (url, init) => {
  if (new URL(url, location.href).pathname.endsWith('/assistant/capabilities')) {
    if(window.delayedCapabilities) return new Promise(resolve=>window.delayedCapabilities.push({key:init.headers['X-Komodoc-Key'],resolve:value=>resolve(Response.json(value))}));
    return Response.json({can_read:true,can_comment:true,can_edit:false});
  }
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
  component = mount(Harness, {target:document.body});
  window.setProps=next=>component.updateProps(next);
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
  await until("agent listening",()=>page.evaluate('document.querySelector("[role=status]")?.textContent.includes("Ready for your message")'),10000);
  assert.equal(await page.evaluate('window.sockets[0].url.includes("secret-token")'),false);
  assert.equal(await page.evaluate('new URL(window.sockets[0].url).searchParams.get("k")'),"secret");
  assert.deepEqual(await page.evaluate('window.sockets[0].sent[0]'),{type:"join",token:"secret-token",role:"user"});

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='Explain this';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));})()`);
  await until("agent reply",()=>page.evaluate('document.querySelector("[role=log]")?.textContent.includes("<img")'),10000);
  assert.equal(await page.evaluate("Boolean(window.injected||document.querySelector('[role=log] img'))"),false);
  const posted=await page.evaluate('window.sockets[0].sent.find(frame=>frame.type==="message")');
  assert.equal(posted.context.file,"paper.md");
  assert.deepEqual(posted.context.selection,{path:"paper.md",exact:"A passage",prefix:"",suffix:"",position:null});

  await page.evaluate(`(()=>{const clipboard={writeText:value=>{window.copiedInstructions=value;return Promise.resolve();}};Object.defineProperty(navigator,'clipboard',{configurable:true,value:clipboard});document.querySelector('.agent-actions button').click();})()`);
  await until("instructions copied",()=>page.evaluate('typeof window.copiedInstructions==="string"'),1000);
  assert.match(await page.evaluate("window.copiedInstructions"),/komodoc agent chat watch/);
  assert.doesNotMatch(await page.evaluate("window.copiedInstructions"),/--after/);

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='First';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea[placeholder]').value"),"First\n");

  // A listener going away disables Send, not drafting. Shift+Enter is a
  // newline and IME Enter cannot send the request prematurely.
  await page.evaluate("window.sockets[0].emit({type:'presence',listening:false,browser:true})");
  await until("draft while away", () => page.evaluate('!document.querySelector("textarea").disabled && document.querySelector(".chat-form button").disabled'), 1000);
  await page.evaluate(`(()=>{const input=document.querySelector('textarea');input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',shiftKey:true,bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea').value"), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length"), 1);

  // Browser loss revokes credentials; both setup buttons must refuse to copy
  // the old prompt, and recovery must keep a locally drafted request.
  await page.evaluate("window.sockets[0].close()");
  await until("ended", () => page.evaluate('document.querySelector("[role=status]").textContent.includes("Connection ended")'), 1000);
  assert.equal(await page.evaluate('Array.from(document.querySelectorAll("button")).filter(b=>/Copy setup prompt|Copied/.test(b.textContent)).every(b=>b.disabled)'), true);
  await page.evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Reconnect agent").click()');
  await until("fresh recovered channel", () => page.evaluate("window.sockets.length===2"), 1000);
  assert.equal(await page.evaluate("document.querySelector('textarea').value"), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[1].sent.filter(frame=>frame.type==='message').length"), 0);

  // Explicitly replacing a request discards its old draft and binds the new
  // source anchor; Remove must not immediately reattach it from props.
  await page.evaluate("window.setProps({selection:{exact:'same',source:{path:'a.md',exact:'same',prefix:'before ',suffix:' after',position:20},revision:'captured-a'},request:{id:'new-selection',selection:{exact:'same',source:{path:'a.md',exact:'same',prefix:'before ',suffix:' after',position:20}},revision:'captured-a'}})");
  await until("replacement choice", () => page.evaluate('document.body.innerText.includes("Replace draft and context")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Replace draft and context").click()');
  await until("replacement applied", () => page.evaluate('document.querySelector("textarea").value==="" && document.querySelector(".attachment").textContent.includes("a.md")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll(".task-chips button")).find(b=>b.textContent==="Tighten").click()');
  await until("tighten task", () => page.evaluate('document.querySelector("textarea").value==="Tighten the selected passage."'), 1000);
  await page.evaluate("window.setProps({path:'b.md'})");
  await page.evaluate('document.querySelector(".chat-form button").click()');
  await until("anchored request", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.task?.kind==='tighten')"), 1000);
  const anchored = await page.evaluate("window.sockets[1].sent.find(frame=>frame.task?.kind==='tighten')");
  assert.equal(anchored.context.file, "a.md");
  assert.equal(anchored.context.revision, "captured-a");
  assert.equal(anchored.context.selection.position, 20);
  await page.evaluate('Array.from(document.querySelectorAll(".attachment button")).find(b=>b.textContent==="Remove").click()');
  await until("removed attachment", () => page.evaluate('!document.querySelector(".attachment")'), 1000);
  await page.evaluate("window.setProps({request:{id:'diagnostic',diagnostic:{file:'error.typ',line:4,message:'Old error',source:'old source',revision:'old-revision'},revision:'old-revision'}})");
  await until("diagnostic request", () => page.evaluate('document.querySelector("textarea").value==="Explain this diagnostic."'), 1000);
  await page.evaluate('document.querySelector(".chat-form button").click()');
  await until("diagnostic sent", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.context?.diagnostic)"), 1000);
  const explained = await page.evaluate("window.sockets[1].sent.find(frame=>frame.context?.diagnostic)");
  assert.equal(explained.context.file, "error.typ");
  assert.equal(explained.context.revision, "old-revision");
  assert.equal(explained.context.selection, undefined);
  assert.equal(explained.context.diagnostic.source, "old source");

  // Permission checks are tied to the newly entered link. An older response
  // cannot restore permissions after a newer, weaker link has been checked.
  await page.evaluate("window.delayedCapabilities=[]");
  await page.evaluate(`(()=>{const input=document.querySelector('input[type=url]');input.value=location.origin+'/docs/paper#k=old-key';input.dispatchEvent(new Event('input',{bubbles:true}));input.value=location.origin+'/docs/paper#k=read-key';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await until("two access checks", () => page.evaluate("window.delayedCapabilities.length===2"), 1000);
  assert.deepEqual(await page.evaluate("window.delayedCapabilities.map(call=>call.key)"), ["old-key", "read-key"]);
  assert.equal(await page.evaluate('document.querySelector(".access-line").textContent.includes("Access not checked")'), true);
  await page.evaluate("window.delayedCapabilities[1].resolve({can_read:true,can_comment:false,can_edit:false})");
  await until("read only", () => page.evaluate('document.querySelector(".access-line").textContent.includes("Read only")'), 1000);
  await page.evaluate("window.delayedCapabilities[0].resolve({can_read:true,can_comment:true,can_edit:true})");
  assert.equal(await page.evaluate('document.querySelector(".access-line").textContent.includes("Read only")'), true);
  await page.evaluate("window.delayedCapabilities=null");

  // Short panels retain a scroll path to the composer instead of clipping it.
  await page.resize(240, 400);
  await page.evaluate("document.body.style.width='240px'; document.body.style.height='400px'; document.querySelector('textarea').scrollIntoView({block:'center'})");
  assert.equal(await page.evaluate("(()=>{const r=document.querySelector('textarea').getBoundingClientRect(); return r.top>=0 && r.bottom<=innerHeight;})()"), true);

  await page.evaluate("window.remount()");
  await until("new live channel",()=>page.evaluate("window.sockets.length===3"),10000);
  assert.equal(await page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'),false);
  assert.equal(await page.evaluate("window.calls.filter(call=>call.suffix==='').length"),3);
  console.log("agent-browser: context, recovery, draft replacement, keyboard, short layout and no replay passed");
} finally {
  await page?.close();
  if(server) await new Promise(resolve=>server.close(resolve));
  rmSync(temporary,{recursive:true,force:true});
}
