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
import { browser, until } from "../../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-agent-browser-"));
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
writeFileSync(harness, `
<script>
import Agent from ${JSON.stringify(join(root, "src/components/reader/Agent.svelte"))};
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
import * as localBridge from ${JSON.stringify(join(root, "src/lib/companion/client.js"))};
// The Reader scopes the local client to the document it loaded; this harness
// mounts the panel without a Reader, so it stands in for that one step. The
// panel itself deliberately does not do it: which document the local app is
// asked about is page-level state, not the panel's.
localBridge.configure({ project: "paper", origin: location.origin });
window.calls = [];
window.previewCalls = [];
window.sockets = [];
// A stand-in for the local LibrePaper app on loopback. The connection flow
// is now a conversation between the browser and this computer, so the panel
// cannot be exercised at all without one.
window.localCalls = [];
window.localAgents = [
  {id:'claude',label:'Claude Code',path:'/usr/bin/claude',configurable:true,assistant:true,assistant_fetches:false,assistant_blocked:'',assistant_note:'',restart:'Restart Claude Code'},
  {id:'pi',label:'Pi',path:'/usr/bin/pi',configurable:false,assistant:true,assistant_fetches:true,assistant_blocked:'',assistant_note:"Pi's ACP adapter has incomplete MCP support, so the document tools may not reach it",restart:'Restart Pi'},
];
window.fetch = async (url, init) => {
  const pathname = new URL(url, location.href).pathname;
  if (new URL(url, location.href).origin === 'http://127.0.0.1:8763') {
    const route = pathname.replace('/librepaper/local/v1/', '');
    const body = init?.body ? JSON.parse(init.body) : {};
    window.localCalls.push({route, body});
    if (route === 'health') return Response.json({service:'librepaper-local',protocol:[2],version:'test',instance:'one'});
    if (route === 'capabilities') return Response.json({builders:[]});
    if (route === 'agents') return Response.json({agents:window.localAgents,connections:[]});
    if (route === 'connections') return Response.json({connection:'paper'}, {status:201});
    if (route === 'connect') {
      // The real app rejects a null project as a malformed body, which is
      // what an unconfigured client sends. Fail loudly here rather than
      // letting the panel look like it paired.
      window.connectBodies = [...(window.connectBodies || []), body];
      if (typeof body.project !== 'string' || !body.project) return Response.json({error:'bad JSON body'}, {status:400});
      return window.pairingAccepted
        ? Response.json({token:'pair-token',expires:Date.now()/1000+3600,instance:'one'})
        : Response.json({error:'invalid pairing code'}, {status:403});
    }
    if (route === 'assistant') return Response.json({running:true,agent:body.agent});
    if (route === 'assistant/stop') return Response.json({stopped:true});
    throw new Error('unexpected local app request: ' + route);
  }
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
// The pairing the panel reads is the one the local compiler already uses, so
// the harness seeds it exactly as a previously paired browser would hold it,
// under the real key: origin and the document's own project. A pairing stored
// under any other key is not this document's.
// Setting window.unpaired drops it, to exercise the panel before pairing.
window.pairingKey = location.origin + '|paper';
if (!window.unpaired) localStorage.setItem('librepaper-local-pairings', JSON.stringify({[window.pairingKey]:{token:'pair-token',expires:Date.now()/1000+3600}}));
let component;
window.remount = async () => {
  if (component) await unmount(component);
  component = mount(Harness, {target:document.body});
  window.setProps=next=>component.updateProps(next);
};
window.remount();
`);

// Both settings are selects now, so a test chooses a value the way a person
// does: set it and let the change handler run.
const choose = (label, value) => page.evaluate(
  `(()=>{const s=document.querySelector('select[aria-label=${label}]');s.value=${JSON.stringify(value)};s.dispatchEvent(new Event('change',{bubbles:true}));})()`);

let server, page;
try {
  await build({ configFile:false, root, plugins:[svelte(),tailwindcss()], logLevel:"error",
    build:{ outDir:join(temporary,"build"), lib:{entry,formats:["es"],fileName:()=>"panel.js"} } });
  // Beyond the entry chunk and its stylesheet, a component reachable from
  // the panel can carry its own dynamic import, and that import lands as its
  // own file next to panel.js. Anything under the build directory is served
  // by name rather than special-cased one file at a time, so a new chunk
  // here does not mean a new line in this server.
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
  assert.deepEqual(await page.evaluate(`Array.from(document.querySelectorAll("select[aria-label=Access] option")).slice(1).map(o=>o.textContent.split(":")[0])`), ["Comment", "Track changes", "Edit"]);
  // The copy action appears only once an access level is chosen.
  assert.equal(await page.evaluate("Boolean(document.querySelector('.access-detail'))"), false);
  if (process.env.AGENT_SCREENSHOT) {
    await choose("Access", "tracked");
    const shot = await page.command("Page.captureScreenshot", { format: "png" });
    writeFileSync(process.env.AGENT_SCREENSHOT, Buffer.from(shot.data, "base64"));
  }
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
  // The pane carries the padding, not the panel: the tab strip is flush to
  // the top of the panel, as the collaboration panel's is.
  assert.equal(await page.evaluate("(()=>{const pane=document.querySelector('#agent-pane-chat');const form=document.querySelector('.chat-form').getBoundingClientRect();const bottom=pane.getBoundingClientRect().bottom-parseFloat(getComputedStyle(pane).paddingBottom);return Math.abs(form.bottom-bottom)<2;})()"), true, "composer stays at the bottom of the pane");
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

  // A permission request stays visible until the runner reports that it
  // resumed. The sidebar offers exactly the options the agent named, and
  // never treats a missing response as consent.
  await page.evaluate(`window.sockets[0].emit({type:'task',task_id:'input-task',status:'needs_input',context:{input:{request_id:'approval-1',kind:'permission',message:'Run the command?',options:[{id:'allow-once',label:'Allow once'},{id:'reject',label:'Reject'}]}}})`);
  await until("permission request", () => page.evaluate('document.querySelector(".input-request")?.textContent.includes("Run the command?")'), 1000);
  assert.equal(
    await page.evaluate(`Array.from(document.querySelectorAll('.input-request button')).map(b=>b.textContent.trim()).join('|')`),
    "Allow once|Reject|Cancel",
    "the sidebar shows the agent's own options and nothing it invented",
  );
  await page.evaluate('Array.from(document.querySelectorAll(".input-request button")).find(b=>b.textContent.trim()==="Reject").click()');
  await until("permission response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='input'&&frame.response?.option==='reject')"), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'input-task',status:'working',text:'Continuing'})");
  await until("permission cleared", () => page.evaluate('!document.querySelector(".input-request")'), 1000);

  // Cancelling is distinct from choosing an option: the agent must be able to
  // tell "the user declined to answer" from "the user rejected this".
  await page.evaluate(`window.sockets[0].emit({type:'task',task_id:'cancel-task',status:'needs_input',context:{input:{request_id:'approval-2',kind:'permission',message:'Write the file?',options:[{id:'allow-once',label:'Allow once'}]}}})`);
  await until("second permission request", () => page.evaluate('document.querySelector(".input-request")?.textContent.includes("Write the file?")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll(".input-request button")).find(b=>b.textContent.trim()==="Cancel").click()');
  await until("cancel response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='input'&&frame.response?.cancelled===true)"), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'cancel-task',status:'completed'})");
  await until("second permission cleared", () => page.evaluate('!document.querySelector(".input-request")'), 1000);

  // Opening a read-only comment prepares a response in the composer. It
  // never submits until the user explicitly sends it, and the whole thread
  // travels with that response for the external agent.
  const beforeComment = await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length");
  await page.evaluate("window.setProps({request:{id:'comment-request',comment:{id:'comment-1',body:'Please clarify this',source:{path:'paper.md',exact:'A passage',prefix:'',suffix:'',position:0},replies:[{body:'Could you expand?'}],revision:'rev-1'}}})");
  await until("comment response draft", () => page.evaluate('document.querySelector(".chat-form textarea").value==="Address this comment."'), 1000);
  assert.equal(await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length"), beforeComment);
  await page.evaluate('document.querySelector(".chat-form").requestSubmit()');
  await until("comment response sent", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.context?.thread?.id==='comment-1')"), 1000);
  const response = await page.evaluate("window.sockets[0].sent.find(frame=>frame.context?.thread?.id==='comment-1')");
  assert.deepEqual(response.context.thread.replies, [{body:"Could you expand?"}]);

  // The connection flow is a choice of access and a click, with no prompt to
  // paste anywhere. The panel offers the agents the local app actually found.
  await until("local app paired", () => page.evaluate(`document.querySelector('.setup-requirement')?.dataset.state==='ready'`), 3000);
  await until("agents listed", () => page.evaluate(`Boolean(document.querySelector('select[aria-label=Agent] option[value=claude]'))`), 2000);

  const panel = await page.evaluate("document.body.textContent");
  for (const gone of ["librepaper agent connect", "Copy connection instructions", "LIBREPAPER_CHAT_TOKEN", "--background", "npx skills add"]) {
    assert.doesNotMatch(panel, new RegExp(gone.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
      `the sidebar no longer hands out "${gone}"`);
  }

  const startWith = async (role) => {
    await choose("Access", role);
    await until(role + " chosen", () => page.evaluate(`Boolean(document.querySelector('.access-detail[data-access=${role}] button'))`), 1000);
    await page.evaluate(`document.querySelector('.access-detail[data-access=${role}] button').click()`);
  };
  await startWith("commenter");
  await until("assistant started", () => page.evaluate("window.localCalls.some(call=>call.route==='assistant')"), 2000);
  const started = await page.evaluate("window.localCalls.find(call=>call.route==='assistant')");
  assert.equal(started.body.agent, "claude");
  assert.equal(started.body.connection, "paper");
  assert.equal(started.body.chat_token, "secret-token");
  // The link crosses loopback exactly once, to register the connection.
  // Everything afterwards refers to the document by name.
  const registration = await page.evaluate("window.localCalls.find(call=>call.route==='connections')");
  assert.match(registration.body.link, /#k=commenter/);
  assert.equal(registration.body.access, "commenter");
  // The work happens in the chat, so starting goes there rather than leaving
  // the reader on a settings pane with nothing more to say.
  assert.equal(await page.evaluate(`document.querySelector("#agent-pane-chat").hidden`), false);
  await page.evaluate('document.querySelector("#agent-tab-connection").click()');

  await choose("Agent", "pi");
  // A known limitation of an agent's ACP route is stated next to the agent,
  // not left to be inferred from a task that silently does nothing.
  await until("pi chosen", () => page.evaluate(`Boolean(document.querySelector('[data-agent-note=pi]'))`), 1000);
  assert.match(
    await page.evaluate(`document.querySelector('[data-agent-note=pi]').textContent`),
    /incomplete MCP support/,
  );
  // Starting it would fetch its adapter, so the panel says so beforehand,
  // and says it beside the button rather than inside a label that changes.
  assert.match(
    await page.evaluate(`document.querySelector(".access-detail").textContent`),
    /downloads its adapter the first time/,
  );

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='First';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea[placeholder]').value"),"First\n");

  // A runner going away stops sending, not drafting. Shift+Enter is a
  // newline and IME Enter cannot send the request prematurely.
  await page.evaluate("window.sockets[0].emit({type:'presence',agent:false,browser:true})");
  await until("draft while away", () => page.evaluate('!document.querySelector(".chat-form textarea").disabled && document.querySelector(".chat-form").dataset.cansend==="false"'), 1000);
  await page.evaluate(`(()=>{const input=document.querySelector(".chat-form textarea");input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',shiftKey:true,bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));})()`);
  assert.equal(await page.evaluate(`document.querySelector(".chat-form textarea").value`), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length"), 2);

  // Browser loss triggers automatic socket recovery; credentials, transcript
  // and locally drafted requests remain in the browser session.
  await page.evaluate("window.sockets[0].close()");
  await until("automatic recovery", () => page.evaluate('window.sockets.length===2'), 3000);
  await until("fresh recovered channel", () => page.evaluate("window.sockets.length===2"), 1000);
  assert.equal(await page.evaluate(`document.querySelector(".chat-form textarea").value`), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[1].sent.filter(frame=>frame.type==='message').length"), 2,
    "relay acknowledgements are replayed until a task event confirms runner admission");

  // Explicitly replacing a request discards its old draft and binds the new
  // source anchor; Remove must not immediately reattach it from props.
  await page.evaluate("window.setProps({selection:{exact:'same',source:{path:'a.md',exact:'same',prefix:'before ',suffix:' after',position:20},revision:'captured-a'},request:{id:'new-selection',selection:{exact:'same',source:{path:'a.md',exact:'same',prefix:'before ',suffix:' after',position:20}},revision:'captured-a'}})");
  await until("replacement choice", () => page.evaluate('document.body.innerText.includes("Replace draft and context")'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Replace draft and context").click()');
  await until("replacement applied", () => page.evaluate('document.querySelector(".chat-form textarea").value==="" && document.querySelector(".context-summary").textContent.includes("a.md")'), 1000);
  // The launcher is a catalog: search narrows it, a row opens a preparation
  // view, and only that view writes the draft.
  await page.evaluate('document.querySelector("#agent-tab-tasks").click()');
  await page.evaluate(`(()=>{const search=document.querySelector('.task-search');search.value='tight';search.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await until("search narrows", () => page.evaluate('document.querySelectorAll(".task-catalog .task-row").length===1'), 1000);
  await page.evaluate('document.querySelector(".task-catalog .task-row").click()');
  await until("preparation view", () => page.evaluate('!!document.querySelector(".task-prepare") && document.querySelector(".task-prepare .attachment").textContent.includes("a.md")'), 1000);
  assert.equal(await page.evaluate('document.querySelector(".chat-form textarea").value'), "", "opening a task does not write the draft");
  assert.equal(await page.evaluate('document.querySelector(".task-scope select").value'), "selection", "scope is inferred from the attached passage");
  await page.evaluate(`(()=>{const notes=document.querySelector('.task-prepare textarea');notes.value='Keep the citations.';notes.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await page.evaluate('Array.from(document.querySelectorAll(".task-prepare button")).find(b=>b.textContent==="Send to agent").click()');
  // Picking the task is the decision. It goes to the agent there and then,
  // and the panel moves to where the answer will arrive.
  await until("tighten sent", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.task?.kind==='tighten')"), 2000);
  assert.equal(await page.evaluate('document.querySelector("#agent-pane-chat").hidden'), false, "sending a task opens the chat");
  assert.equal(await page.evaluate('document.querySelector(".chat-form textarea").value'), "", "the task was sent, not left as a draft");
  await page.evaluate("window.setProps({path:'b.md'})");
  await until("anchored request", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.task?.kind==='tighten')"), 1000);
  const anchored = await page.evaluate("window.sockets[1].sent.find(frame=>frame.task?.kind==='tighten')");
  assert.equal(anchored.context.file, "a.md");
  assert.equal(anchored.context.revision, "captured-a");
  assert.equal(anchored.context.selection.position, 20);
  await page.evaluate('Array.from(document.querySelectorAll(".context-summary button")).find(b=>b.textContent==="Remove passage").click()');
  await until("removed attachment", () => page.evaluate('!document.body.innerText.includes("Selected passage")'), 1000);
  await page.evaluate("window.setProps({request:{id:'diagnostic',diagnostic:{file:'error.typ',line:4,message:'Old error',source:'old source',revision:'old-revision'},revision:'old-revision'}})");
  await until("diagnostic request", () => page.evaluate('document.querySelector(".chat-form textarea").value==="Fix this diagnostic."'), 1000);
  await page.evaluate('document.querySelector(".chat-form").requestSubmit()');
  await until("diagnostic sent", () => page.evaluate("window.sockets[1].sent.some(frame=>frame.context?.diagnostic)"), 1000);
  const explained = await page.evaluate("window.sockets[1].sent.find(frame=>frame.context?.diagnostic)");
  assert.equal(explained.context.file, "error.typ");
  assert.equal(explained.context.revision, "old-revision");
  assert.equal(explained.context.selection, undefined);
  assert.equal(explained.context.diagnostic.source, "old source");

  // Every role registers a real bounded link, and reuses existing share
  // links. The connection is what carries the access, so getting the key
  // right here is what actually bounds the agent.
  const linkFor = (role) => page.evaluate(`window.localCalls.filter(call=>call.route==='connections').at(-1).body.link.includes('#k=${role}')`);
  const connectAs = async (role) => {
    await page.evaluate('document.querySelector("#agent-tab-connection").click()');
    // Starting the combination already running is correctly refused, so stop
    // first. Re-selecting a value already in force also fires no change event,
    // exactly as it would not for a person, so move away from it.
    await page.evaluate(`Array.from(document.querySelectorAll(".access-detail button")).find(b=>b.textContent.trim().startsWith("Stop"))?.click()`);
    if (await page.evaluate(`document.querySelector("select[aria-label=Access]").value`) === role) {
      await choose("Access", role === "editor" ? "commenter" : "editor");
    }
    await choose("Agent", "claude");
    await choose("Access", role);
    await until(role + " chosen", () => page.evaluate(`Boolean(document.querySelector('.access-detail[data-access=${role}] button'))`), 1000);
    await page.evaluate("window.localCalls=[]");
    await page.evaluate(`document.querySelector('.access-detail[data-access=${role}] button').click()`);
    await until(role + " registered", () => page.evaluate("window.localCalls.some(call=>call.route==='connections')"), 2000);
  };
  // No read-only level is offered: the document's MCP surface admits a
  // commenter at minimum, so an assistant on a read link gets no tools at all.
  assert.equal(
    await page.evaluate(`Boolean(document.querySelector("select[aria-label=Access] option[value=reader]"))`),
    false,
    "a level the assistant cannot use is not offered",
  );
  for (const role of ["commenter", "editor"]) {
    await connectAs(role);
    assert.equal(await linkFor(role), true, `the ${role} connection carries the ${role} key`);
  }
  // Track changes deliberately hands out the commenter key: the access level
  // is what prevents direct source edits, not an instruction asking nicely.
  await connectAs("tracked");
  assert.equal(await linkFor("commenter"), true);
  assert.equal(await linkFor("editor"), false);
  // A level with no existing share link mints one rather than failing.
  await page.evaluate("window.missingAccess=true");
  await connectAs("editor");
  assert.equal(await linkFor("editor"), true);
  assert.deepEqual(await page.evaluate("window.createdAccess"), {link:{role:"editor",until:"180d",label:"Agent"}});
  await page.evaluate('document.querySelector("#agent-tab-tasks").click()');
  assert.equal(await page.evaluate('document.querySelector("#agent-pane-chat").hidden'), true);
  await page.evaluate(`(()=>{const search=document.querySelector('.task-search');search.value='';search.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await until("catalog groups", () => page.evaluate('document.querySelectorAll(".task-catalog .task-group").length>=3'), 1000);
  // The runner went away earlier in this file, and a task cannot be sent to
  // an assistant that is not there. Bring it back first: the panel refusing
  // is the correct behaviour, not something to click through.
  await page.evaluate("window.sockets.at(-1).emit({type:'presence',agent:true,browser:true})");
  await until("runner back", () => page.evaluate(`!document.querySelector(".task-prepare button")?.disabled`), 2000);
  await page.evaluate('Array.from(document.querySelectorAll(".task-catalog .task-row span")).find(s=>s.textContent==="Explain").closest("button").click()');
  await page.evaluate('Array.from(document.querySelectorAll(".task-prepare button")).find(b=>b.textContent==="Send to agent").click()');
  await until("preset opens chat", () => page.evaluate('!document.querySelector("#agent-pane-chat").hidden'), 1000);

  await page.evaluate("window.setProps({comments:[{id:'task-comment',body:'Clarify this',replies:[]}],diagnostics:[{message:'Compile error',file:'error.typ',revision:'old-sha',source:'old source'}]})");
  await page.evaluate('document.querySelector("#agent-tab-connection").click()');
  await connectAs("commenter");
  assert.equal(await linkFor("commenter"), true);
  await page.evaluate('document.querySelector("#agent-tab-tasks").click()');
  await page.evaluate('Array.from(document.querySelectorAll(".context-tasks .task-row span")).find(s=>s.textContent==="Address comment").closest("button").click()');
  assert.equal(await page.evaluate("window.commentTask.id"), "task-comment");
  await page.evaluate('document.querySelector("#agent-tab-tasks").click()');
  // The launcher names the diagnostic; the compiler's prose stays in Diagnostics.
  assert.match(await page.evaluate('document.querySelector(".context-tasks").textContent'), /Error · error\.typ/);
  assert.ok(!(await page.evaluate('document.querySelector(".context-tasks").textContent')).includes("Compile error"));
  await page.evaluate('Array.from(document.querySelectorAll(".context-tasks .task-row span")).find(s=>s.textContent==="Fix diagnostic").closest("button").click()');
  assert.equal(await page.evaluate("window.diagnosticTask.revision"), "old-sha");
  assert.equal(await page.evaluate("window.diagnosticTask.source"), "old source");
  await page.evaluate('document.querySelector("#agent-tab-chat").click()');

  // Short panels keep the composer visible while conversation content scrolls.
  await page.resize(240, 400);
  await page.evaluate(`document.body.style.width="240px"; document.body.style.height="400px"; document.querySelector(".chat-form textarea").scrollIntoView({block:"center"})`);
  assert.equal(await page.evaluate(`(()=>{const r=document.querySelector(".chat-form textarea").getBoundingClientRect(); return r.top>=0 && r.bottom<=innerHeight;})()`), true);

  // The pane carries the padding, not the panel: the tab strip is flush to
  // the top of the panel, as the collaboration panel's is.
  assert.equal(await page.evaluate("(()=>{const pane=document.querySelector('#agent-pane-chat');const form=document.querySelector('.chat-form').getBoundingClientRect();const bottom=pane.getBoundingClientRect().bottom-parseFloat(getComputedStyle(pane).paddingBottom);return Math.abs(form.bottom-bottom)<2;})()"), true, "composer stays at the bottom of the pane");
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
  // Pairing, from an unpaired browser. A refused code must say so beside the
  // button that was pressed: the panel's shared error line is below three
  // tabs of content, so an error reported only there reads as a dead button.
  await page.evaluate("window.unpaired=true; localStorage.removeItem('librepaper-local-pairings'); window.pairingAccepted=false");
  await page.evaluate("window.remount()");
  await page.evaluate('document.querySelector("#agent-tab-connection").click()');
  await until("pairing prompt", () => page.evaluate(`Boolean(document.querySelector('.setup-code'))`), 3000);
  await page.evaluate(`(()=>{const field=document.querySelector('.setup-code');field.value='424242';field.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await page.evaluate(`Array.from(document.querySelectorAll('.setup-requirement button')).find(b=>b.textContent.trim()==='Pair').click()`);
  await until("refused code is reported in place", () => page.evaluate(
    `document.querySelector('.setup-requirement [role=alert]')?.textContent.includes('different one')`), 3000);

  // A local app on a non-default port is recoverable from this panel, not
  // only from another settings screen.
  assert.match(
    await page.evaluate(`document.querySelector('.setup-address summary').textContent`),
    /another port/,
  );

  // The pairing must name this document, whatever its format. The Reader only
  // scopes the local client for locally renderable formats, so a panel that
  // relied on that would send a null project and be refused outright.
  assert.equal(
    await page.evaluate("window.connectBodies.at(-1).project"), "paper",
    "pairing is scoped to this document, not to whatever configured the client last",
  );

  await page.evaluate("window.pairingAccepted=true");
  await page.evaluate(`Array.from(document.querySelectorAll('.setup-requirement button')).find(b=>b.textContent.trim()==='Pair').click()`);
  await until("pairing succeeds", () => page.evaluate(
    `document.querySelector('.setup-requirement')?.dataset.state==='ready'`), 3000);

  // The agents listed right after pairing must be selectable. A background
  // refresh that shares the panel's busy flag leaves every row disabled, and
  // a list you cannot click is worse than no list.
  await until("agents listed after pairing", () => page.evaluate(
    `document.querySelectorAll('select[aria-label=Agent] option').length > 0`), 3000);
  assert.equal(
    await page.evaluate(`document.querySelector('select[aria-label=Agent]').disabled`),
    false,
    "the agent list is usable as soon as it appears",
  );
  await choose("Agent", "claude");
  assert.equal(
    await page.evaluate(`document.querySelector('select[aria-label=Agent]').value`),
    "claude",
    "the chosen agent is what the control shows",
  );

  console.log("agent-browser: context, recovery, draft, keyboard, short layout, pairing and local session persistence passed");
} finally {
  await page?.close();
  if(server) await new Promise(resolve=>server.close(resolve));
  rmSync(temporary,{recursive:true,force:true});
}
