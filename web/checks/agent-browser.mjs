// Exercise the actual Svelte live-agent panel in Chromium.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-agent-browser-"));
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
writeFileSync(harness, `
<script>
import Agent from ${JSON.stringify(join(root, "src/components/Agent.svelte"))};
let values=$state({slug:'paper',canShare:true,link:location.origin+'/docs/paper#k=secret',path:'paper.md',selection:{exact:'A passage'}});
export function updateProps(next){ values={...values,...next}; }
async function preview(request){ window.previewCalls.push(request); return {ok:true,diagnostics:[],output:'html'}; }
</script>
<Agent {...values} onpreview={preview} oncommenttask={item=>window.commentTask=item} ondiagnostictask={item=>window.diagnosticTask=item} />
`);
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
window.calls = [];
window.previewCalls = [];
window.sockets = [];
window.fetch = async (url, init) => {
  const pathname = new URL(url, location.href).pathname;
  if (pathname === '/api/documents/paper/agent/candidates/candidate-large') {
    window.candidateHeaders = {...init.headers};
    return Response.json({candidate_id:'candidate-large',base_revision:'base',revision:'candidate',main:'paper.md',files:{'paper.md':{kind:'text',id:'paper',sha:'source-sha',size:20000}}});
  }
  if (pathname === '/api/documents/paper/agent/candidates/candidate-large/source') {
    window.sourceHeaders = {...init.headers};
    return new Response('x'.repeat(20000), {headers:{'content-type':'text/plain'}});
  }
  if (new URL(url, location.href).pathname.endsWith('/share')) {
    if (init.method === 'POST') { window.createdAccess=JSON.parse(init.body); window.missingAccess=false; }
    if (window.missingAccess) return Response.json({links:{}});
    return Response.json({links:Object.fromEntries(['reader','commenter','editor'].map(role=>[role,{key:role,url:'/docs/paper#k='+role}]))});
  }
  if (new URL(url, location.href).pathname.endsWith('/assistant/capabilities')) {
    if(window.delayedCapabilities) return new Promise(resolve=>window.delayedCapabilities.push({key:init.headers['X-LibrePaper-Key'],resolve:value=>resolve(Response.json(value))}));
    const key=init.headers['X-LibrePaper-Key'];
    return Response.json({can_read:true,can_comment:key!=='reader',can_edit:key==='editor'});
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
    if (frame.type === 'join') queueMicrotask(()=>{ this.emit({type:'ready',browser:true,agent:true}); this.emit({type:'presence',browser:true,agent:true}); });
    if (frame.type === 'message') queueMicrotask(()=>{
      this.emit({type:'message',message:{id:frame.id,role:'user',text:frame.text,context:frame.context}});
      this.emit({type:'ack',id:frame.id});
      this.emit({type:'message',message:{id:'agent-reply',role:'agent',text:'<img src=x onerror="window.injected=true">'}});
    });
    if (frame.type === 'input') queueMicrotask(()=>this.emit({type:'ack',id:frame.id}));
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
  // Beyond the entry chunk and its stylesheet, a component reachable from
  // the panel can carry its own dynamic import -- dictation/service.js
  // reaches the toast store that way, so it never has to load Skeleton's
  // components for a check that never triggers a toast -- and that import
  // lands as its own file next to panel.js. Anything under the build
  // directory is served by name rather than special-cased one file at a
  // time, so a new chunk here does not mean a new line in this server.
  const CONTENT_TYPES = { ".js": "text/javascript", ".css": "text/css", ".map": "application/json" };
  server=createServer((request,response)=>{
    const path = join(temporary, "build", decodeURIComponent(request.url.split("?")[0]));
    if (request.url !== "/" && existsSync(path)) {
      const ext = path.slice(path.lastIndexOf("."));
      response.setHeader("content-type", CONTENT_TYPES[ext] || "application/octet-stream");
      response.end(readFileSync(path));
      return;
    }
    response.setHeader("content-type", "text/html");
    response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/librepaper-web.css"><style>body{display:flex;height:700px;width:360px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve,reject)=>{server.once("error",reject);server.listen(0,"127.0.0.1",resolve);});
  page=await browser("chromium",join(temporary,"profile"),22000+Math.floor(Math.random()*10000));
  await page.resize(800, 800);
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("live agent panel",()=>page.evaluate("Boolean(document.querySelector('.agent-panel textarea[aria-label=Message]'))"),10000);
  await until("runner connected",()=>page.evaluate('document.querySelector("[role=status]")?.textContent.includes("Waiting")'),10000);
  assert.equal(await page.evaluate('document.querySelector("#agent-pane-chat").hidden'), true);
  assert.deepEqual(await page.evaluate('Array.from(document.querySelectorAll(".access-buttons button")).map(b=>b.textContent)'), ["Reader", "Commenter", "Edit with track changes", "Edit directly"]);
  await page.evaluate('document.querySelector("#agent-tab-chat").click()');
  const beforeResize = await page.evaluate("document.querySelector('.chat-form textarea').getBoundingClientRect().toJSON()");
  const handle = await page.evaluate("document.querySelector('.resize-handle').getBoundingClientRect().toJSON()");
  await page.command("Input.dispatchMouseEvent", { type: "mousePressed", x: handle.x + 12, y: handle.y + 12, button: "left", clickCount: 1 });
  await page.command("Input.dispatchMouseEvent", { type: "mouseMoved", x: handle.x + 12, y: handle.y - 68, button: "left", buttons: 1 });
  await page.command("Input.dispatchMouseEvent", { type: "mouseReleased", x: handle.x + 12, y: handle.y - 68, button: "left", clickCount: 1 });
  const afterResize = await page.evaluate("document.querySelector('.chat-form textarea').getBoundingClientRect().toJSON()");
  assert.ok(afterResize.height > beforeResize.height + 60, `dragging the top-right handle upward expands the input: ${JSON.stringify({beforeResize, afterResize, handle})}`);
  assert.ok(Math.abs(afterResize.bottom - beforeResize.bottom) < 2, "resizing keeps the input bottom anchored");
  assert.equal(await page.evaluate("Boolean(document.querySelector('.chat-form label'))"), false, "the input needs no visible Message label");
  await page.evaluate("document.querySelector('.resize-handle').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}))");
  assert.ok(await page.evaluate("document.querySelector('.chat-form textarea').getBoundingClientRect().height") < afterResize.height, "keyboard resizing shrinks the input");
  assert.equal(await page.evaluate("(()=>{const panel=document.querySelector('.agent-panel');const form=document.querySelector('.chat-form').getBoundingClientRect();const bottom=panel.getBoundingClientRect().bottom-parseFloat(getComputedStyle(panel).paddingBottom);return Math.abs(form.bottom-bottom)<2;})()"), true, "composer stays at the bottom of the pane");
  assert.equal(await page.evaluate('window.sockets[0].url.includes("secret-token")'),false);
  assert.equal(await page.evaluate('new URL(window.sockets[0].url).searchParams.get("k")'),"secret");
  assert.deepEqual(await page.evaluate('window.sockets[0].sent[0]'),{type:"join",token:"secret-token",role:"user"});

  // Activity comes from task events, independently of connection presence.
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'activity',status:'working'})");
  await until("working indicator", () => page.evaluate('document.querySelector(".agent-actions [role=status]").textContent === "Working..."'), 1000);
  assert.equal(await page.evaluate('document.querySelectorAll(".working-dots > span").length'), 3);
  await page.evaluate("window.sockets[0].emit({type:'presence',browser:true,agent:true})");
  assert.equal(await page.evaluate('document.querySelector(".agent-actions [role=status]").textContent'), "Working...");
  await page.evaluate("window.sockets[0].emit({type:'presence',browser:true,agent:false})");
  await until("disconnected indicator", () => page.evaluate('document.querySelector(".agent-actions [role=status]").textContent === "Disconnected"'), 1000);
  await page.evaluate("window.sockets[0].emit({type:'presence',browser:true,agent:true}); window.sockets[0].emit({type:'task',task_id:'activity',status:'needs_input'})");
  await until("waiting for input", () => page.evaluate('document.querySelector(".agent-actions [role=status]").textContent === "Waiting"'), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'activity',status:'working'}); window.sockets[0].emit({type:'task',task_id:'activity',status:'completed'})");
  await until("waiting after completion", () => page.evaluate('document.querySelector(".agent-actions [role=status]").textContent === "Waiting" && !document.querySelector(".working-dots")'), 1000);

  await page.evaluate("window.setProps({diagnostics:[{severity:'warning',file:'librepaper.tex',line:12,message:'Reference undefined',revision:'render-sha',source:'source line'}]})");
  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='Explain this';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));})()`);
  await until("agent reply",()=>page.evaluate('document.querySelector("[role=log]")?.textContent.includes("<img")'),10000);
  assert.equal(await page.evaluate("Boolean(window.injected||document.querySelector('[role=log] img'))"),false);
  const posted=await page.evaluate('window.sockets[0].sent.find(frame=>frame.type==="message")');
  assert.equal(posted.context.file,"paper.md");
  assert.deepEqual(posted.context.diagnostics, [{severity:'warning',file:'librepaper.tex',line:12,message:'Reference undefined',revision:'render-sha',source:'source line'}]);
  assert.deepEqual(posted.context.selection,{path:"paper.md",exact:"A passage",prefix:"",suffix:"",position:null});

  await page.evaluate("window.sockets[0].emit({type:'preview_request',id:'preview-1',task_id:'task-1',base_revision:'base-revision',revision:'candidate-revision',candidate_id:'candidate-large',candidate_token:'candidate-token'})");
  await until("preview response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='preview_result')"), 1000);
  const preview = await page.evaluate("window.sockets[0].sent.find(frame=>frame.type==='preview_result')");
  assert.equal(preview.request_id, "preview-1");
  assert.equal(preview.task_id, "task-1");
  assert.equal(preview.base_revision, "base-revision");
  assert.equal(preview.revision, "candidate-revision");
  assert.equal(preview.ok, true);
  assert.equal(await page.evaluate("window.previewCalls[0].candidate.texts['paper.md'].length"), 20000);
  assert.equal(await page.evaluate("window.candidateHeaders['X-LibrePaper-Candidate-Token']"), 'candidate-token');
  assert.equal(await page.evaluate("window.sourceHeaders['X-LibrePaper-Key']"), 'secret');
  await page.evaluate("window.sockets[0].emit({type:'preview_request',id:'preview-1',task_id:'task-1',base_revision:'base-revision',revision:'candidate-revision',candidate_id:'candidate-large',candidate_token:'candidate-token'})");
  await until("preview retry", () => page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='preview_result').length===2"), 1000);
  assert.equal(await page.evaluate("window.previewCalls.length"), 1, "a retried preview reuses its cached browser verification");

  // Approval and multi-question requests remain visible until the runner
  // reports that it resumed. The browser sends explicit answers and never
  // silently treats a missing response as approval.
  await page.evaluate(`window.sockets[0].emit({type:'task',task_id:'input-task',status:'needs_input',context:{input:{request_id:'approval-1',kind:'approval',message:'Run the command?'}}})`);
  await until("approval request", () => page.evaluate('document.querySelector(".input-request")?.textContent.includes("Run the command?")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll(".input-request button")).find(b=>b.textContent==="Deny").click()');
  await until("approval response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='input'&&frame.response?.decision==='decline')"), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'input-task',status:'working',text:'Continuing'})");
  await until("approval cleared", () => page.evaluate('!document.querySelector(".input-request")'), 1000);
  await page.evaluate(`window.sockets[0].emit({type:'task',task_id:'question-task',status:'needs_input',context:{input:{request_id:'question-1',kind:'question',message:'Choose values',questions:[{id:'first',question:'First value'},{id:'second',question:'Second value'}]}}})`);
  await until("question request", () => page.evaluate('document.querySelectorAll(".input-request textarea").length===2'), 1000);
  await page.evaluate(`(()=>{const fields=document.querySelectorAll('.input-request textarea'); for (const [index,field] of fields.entries()) { field.value='answer-'+index; field.dispatchEvent(new Event('input',{bubbles:true})); }})()`);
  await page.evaluate('Array.from(document.querySelectorAll(".input-request button")).find(b=>b.textContent==="Send answer").click()');
  await until("question response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='input'&&frame.response?.answers?.first?.answers?.[0]==='answer-0'&&frame.response?.answers?.second?.answers?.[0]==='answer-1')"), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'question-task',status:'completed'})");
  await until("question cleared", () => page.evaluate('!document.querySelector(".input-request")'), 1000);

  // Opening a read-only comment prepares a response in the composer. It
  // never submits until the user explicitly sends it, and the whole thread
  // travels with that response for the external agent.
  const beforeComment = await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length");
  await page.evaluate("window.setProps({request:{id:'comment-request',comment:{id:'comment-1',body:'Please clarify this',source:{path:'paper.md',exact:'A passage',prefix:'',suffix:'',position:0},replies:[{body:'Could you expand?'}],revision:'rev-1'}}})");
  await until("comment response draft", () => page.evaluate('document.querySelector("textarea").value==="Address this comment."'), 1000);
  assert.equal(await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length"), beforeComment);
  await page.evaluate('document.querySelector(".chat-form").requestSubmit()');
  await until("comment response sent", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.context?.thread?.id==='comment-1')"), 1000);
  const response = await page.evaluate("window.sockets[0].sent.find(frame=>frame.context?.thread?.id==='comment-1')");
  assert.deepEqual(response.context.thread.replies, [{body:"Could you expand?"}]);

  await page.evaluate(`(()=>{const clipboard={writeText:value=>{window.copiedInstructions=value;return Promise.resolve();}};Object.defineProperty(navigator,'clipboard',{configurable:true,value:clipboard});document.querySelector('.access-buttons button:nth-child(2)').click();})()`);
  await until("instructions copied",()=>page.evaluate('typeof window.copiedInstructions==="string"'),1000);
  assert.match(await page.evaluate("window.copiedInstructions"),/librepaper agent connect/);
  assert.doesNotMatch(await page.evaluate("window.copiedInstructions"),/chat watch/);
  const setupPrompt = await page.evaluate("window.copiedInstructions");
  assert.doesNotMatch(setupPrompt, /export LIBREPAPER_|\$LIBREPAPER_/);
  assert.doesNotMatch(setupPrompt, /librepaper agent capabilities/);
  assert.match(setupPrompt, /LIBREPAPER_CHAT_TOKEN='secret-token' librepaper agent connect 'http[^\n]+#k=commenter' 'conversation-[^']+' --background/);

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='First';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea[placeholder]').value"),"First\n");

  // A runner going away stops sending, not drafting. Shift+Enter is a
  // newline and IME Enter cannot send the request prematurely.
  await page.evaluate("window.sockets[0].emit({type:'presence',agent:false,browser:true})");
  await until("draft while away", () => page.evaluate('!document.querySelector("textarea").disabled && document.querySelector(".chat-form").dataset.cansend==="false"'), 1000);
  await page.evaluate(`(()=>{const input=document.querySelector('textarea');input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',shiftKey:true,bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea').value"), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length"), 2);

  // Browser loss triggers automatic socket recovery; credentials, transcript
  // and locally drafted requests remain in the browser session.
  await page.evaluate("window.sockets[0].close()");
  await until("automatic recovery", () => page.evaluate('window.sockets.length===2'), 3000);
  await until("fresh recovered channel", () => page.evaluate("window.sockets.length===2"), 1000);
  assert.equal(await page.evaluate("document.querySelector('textarea').value"), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[1].sent.filter(frame=>frame.type==='message').length"), 2,
    "relay acknowledgements are replayed until a task event confirms runner admission");

  // Explicitly replacing a request discards its old draft and binds the new
  // source anchor; Remove must not immediately reattach it from props.
  await page.evaluate("window.setProps({selection:{exact:'same',source:{path:'a.md',exact:'same',prefix:'before ',suffix:' after',position:20},revision:'captured-a'},request:{id:'new-selection',selection:{exact:'same',source:{path:'a.md',exact:'same',prefix:'before ',suffix:' after',position:20}},revision:'captured-a'}})");
  await until("replacement choice", () => page.evaluate('document.body.innerText.includes("Replace draft and context")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Replace draft and context").click()');
  await until("replacement applied", () => page.evaluate('document.querySelector("textarea").value==="" && document.querySelector(".attachment").textContent.includes("a.md")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll(".task-chips button")).find(b=>b.textContent==="Tighten").click()');
  await until("tighten task", () => page.evaluate('document.querySelector("textarea").value==="Tighten the selected passage."'), 1000);
  await page.evaluate("window.setProps({path:'b.md'})");
  await page.evaluate('document.querySelector(".chat-form").requestSubmit()');
  await until("anchored request", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.task?.kind==='tighten')"), 1000);
  const anchored = await page.evaluate("window.sockets[1].sent.find(frame=>frame.task?.kind==='tighten')");
  assert.equal(anchored.context.file, "a.md");
  assert.equal(anchored.context.revision, "captured-a");
  assert.equal(anchored.context.selection.position, 20);
  await page.evaluate('Array.from(document.querySelectorAll(".attachment button")).find(b=>b.textContent==="Remove").click()');
  await until("removed attachment", () => page.evaluate('!document.querySelector(".attachment")'), 1000);
  await page.evaluate("window.setProps({request:{id:'diagnostic',diagnostic:{file:'error.typ',line:4,message:'Old error',source:'old source',revision:'old-revision'},revision:'old-revision'}})");
  await until("diagnostic request", () => page.evaluate('document.querySelector("textarea").value==="Fix this diagnostic."'), 1000);
  await page.evaluate('document.querySelector(".chat-form").requestSubmit()');
  await until("diagnostic sent", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.context?.diagnostic)"), 1000);
  const explained = await page.evaluate("window.sockets[1].sent.find(frame=>frame.context?.diagnostic)");
  assert.equal(explained.context.file, "error.typ");
  assert.equal(explained.context.revision, "old-revision");
  assert.equal(explained.context.selection, undefined);
  assert.equal(explained.context.diagnostic.source, "old source");

  // Every role copies a real bounded link, and reuses existing share links.
  for (const [index, role] of ["reader", "commenter", "editor"].entries()) {
    await page.evaluate('document.querySelector("#agent-tab-connection").click()');
    await page.evaluate(`document.querySelector('.access-buttons button[data-access=${role}]').click()`);
    await until(role + " prompt", () => page.evaluate(`window.copiedInstructions.includes('#k=${role}')`), 1000);
  }
  await page.evaluate("window.copiedInstructions=''; document.querySelector('[data-access=tracked]').click()");
  await until("tracked prompt", () => page.evaluate("window.copiedInstructions.includes('Use track changes for every edit')"), 1000);
  assert.match(await page.evaluate("window.copiedInstructions"), /#k=commenter/);
  assert.doesNotMatch(await page.evaluate("window.copiedInstructions"), /#k=editor/);
  await page.evaluate("window.missingAccess=true; window.copiedInstructions=''; document.querySelector('.access-buttons button').click()");
  await until("created reader access", () => page.evaluate("window.copiedInstructions.includes('#k=reader')"), 1000);
  assert.deepEqual(await page.evaluate("window.createdAccess"), {link:{role:"reader",until:"180d",label:"Agent"}});
  await page.evaluate('document.querySelector("#agent-tab-tasks").click()');
  assert.equal(await page.evaluate('document.querySelector("#agent-pane-chat").hidden'), true);
  await page.evaluate('Array.from(document.querySelectorAll(".task-chips button")).find(b=>b.textContent==="Explain").click()');
  await until("preset opens chat", () => page.evaluate('!document.querySelector("#agent-pane-chat").hidden'), 1000);

  await page.evaluate("window.setProps({comments:[{id:'task-comment',body:'Clarify this',replies:[]}],diagnostics:[{message:'Compile error',file:'error.typ',revision:'old-sha',source:'old source'}]})");
  await page.evaluate('document.querySelector("#agent-tab-connection").click(); document.querySelectorAll(".access-buttons button")[1].click()');
  await until("comment access", () => page.evaluate("window.copiedInstructions.includes('#k=commenter')"), 1000);
  await page.evaluate('document.querySelector("#agent-tab-tasks").click()');
  await page.evaluate('Array.from(document.querySelectorAll(".context-tasks button")).find(b=>b.textContent==="Address comment").click()');
  assert.equal(await page.evaluate("window.commentTask.id"), "task-comment");
  await page.evaluate('Array.from(document.querySelectorAll(".context-tasks button")).find(b=>b.textContent==="Fix diagnostic").click()');
  assert.equal(await page.evaluate("window.diagnosticTask.revision"), "old-sha");
  assert.equal(await page.evaluate("window.diagnosticTask.source"), "old source");
  await page.evaluate('document.querySelector("#agent-tab-chat").click()');

  // Short panels keep the composer visible while conversation content scrolls.
  await page.resize(240, 400);
  await page.evaluate("document.body.style.width='240px'; document.body.style.height='400px'; document.querySelector('textarea').scrollIntoView({block:'center'})");
  assert.equal(await page.evaluate("(()=>{const r=document.querySelector('textarea').getBoundingClientRect(); return r.top>=0 && r.bottom<=innerHeight;})()"), true);

  assert.equal(await page.evaluate("(()=>{const panel=document.querySelector('.agent-panel');const form=document.querySelector('.chat-form').getBoundingClientRect();const bottom=panel.getBoundingClientRect().bottom-parseFloat(getComputedStyle(panel).paddingBottom);return Math.abs(form.bottom-bottom)<2;})()"), true, "composer stays at the bottom of the pane");
  for (const tab of ["connection", "chat", "tasks"]) {
    await page.evaluate(`document.querySelector('#agent-tab-${tab}').click()`);
    assert.equal(await page.evaluate("document.querySelector('.agent-panel').scrollWidth <= document.querySelector('.agent-panel').clientWidth"), true, tab + " fits the narrow sidebar");
  }
  await page.evaluate('document.querySelector("#agent-tab-tasks").focus(); document.querySelector("#agent-tab-tasks").dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowRight",bubbles:true}))');
  // Keyboard navigation follows the selected tab (Tasks wraps to Connection).
  await until("keyboard tab navigation", () => page.evaluate('document.activeElement.id === "agent-tab-connection"'), 1000);

  await page.evaluate("window.remount()");
  await until("new live channel",()=>page.evaluate("window.sockets.length===3"),10000);
  assert.equal(await page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'),true);
  assert.equal(await page.evaluate("window.calls.filter(call=>call.suffix==='').length"),1);
  console.log("agent-browser: context, recovery, draft, keyboard, short layout and local session persistence passed");
} finally {
  await page?.close();
  if(server) await new Promise(resolve=>server.close(resolve));
  rmSync(temporary,{recursive:true,force:true});
}
