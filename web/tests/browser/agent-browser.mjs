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
<Agent {...values} onpreview={preview} oncommenttask={item=>window.commentTask=item} ondiagnostictask={item=>window.diagnosticTask=item} onsettings={()=>window.settingsCalls=(window.settingsCalls||0)+1} />
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
window.activeRunner = null;
window.delayedAssistantStatuses = [];
window.localAgents = [
  {id:'claude',label:'Claude Code',path:'/usr/bin/claude',configurable:true,assistant:true,assistant_fetches:false,assistant_blocked:'',assistant_note:'',restart:'Restart Claude Code'},
  {id:'pi',label:'Pi',path:'/usr/bin/pi',configurable:false,assistant:true,assistant_fetches:true,assistant_blocked:'',assistant_note:"Pi's ACP adapter has incomplete MCP support, so the document tools may not reach it",restart:'Restart Pi'},
];
window.fetch = async (url, init) => {
  const pathname = new URL(url, location.href).pathname;
  if (new URL(url, location.href).origin === 'http://127.0.0.1:8763') {
    const route = pathname.replace('/librepaper/local/', '');
    const body = init?.body ? JSON.parse(init.body) : {};
    window.localCalls.push({route, body});
    if (route === 'health') return Response.json({service:'librepaper-local',protocol:[2],version:'test',instance:'one'});
    if (route === 'capabilities') return Response.json({builders:[]});
    if (route === 'settings') return Response.json({version:'test',standalone:false,startup:null,presets:[],grants:[]});
    if (route === 'agents') return Response.json({agents:window.localAgents});
    if (route === 'pair/request') {
      // Pairing request: return 202 to indicate it's pending approval.
      // In headless mode, the companion would print an approval code to stderr.
      window.pairRequestBodies = [...(window.pairRequestBodies || []), body];
      return Response.json({}, {status:202});
    }
    if (route === 'connect/claim') {
      // Claim phase: check if pairing was accepted and return token.
      // The real app would have approved or rejected based on the approve command.
      window.connectClaimBodies = [...(window.connectClaimBodies || []), body];
      if (typeof body.origin !== 'string' || !body.origin) return Response.json({error:'bad JSON body'}, {status:400});
      return window.pairingAccepted
        ? Response.json({token:'pair-token',expires:Date.now()/1000+3600,instance:'one'})
        : Response.json({error:'approval was denied'}, {status:403});
    }
    if (route === 'assistant') {
      // Runners are located by server, document and conversation, not by a
      // separate connection name, so the mock keys the same way.
      // Pi's adapter download is refused, so choosing Pi restarts the running
      // agent into a stopped one and its pre-start notes stay on screen.
      if (body.agent==='pi') return Response.json({running:false,error:'adapter download refused'});
      window.activeRunner={link:body.link,conversation:body.conversation,agent:body.agent};
      return Response.json({running:true});
    }
    if (route === 'assistant/status') {
      if (window.delayAssistantStatus) {
        window.delayAssistantStatus=false;
        return new Promise(resolve=>window.delayedAssistantStatuses.push({body,resolve}));
      }
      const runner=window.activeRunner;
      return Response.json(runner && runner.conversation===body.conversation
        ? {running:true,state:'ready'}
        : {running:false});
    }
    if (route === 'assistant/stop') { window.activeRunner=null; return Response.json({stopped:true}); }
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
    return Response.json({can_read:true,can_comment:key!=='reader',can_edit:key==='editor',can_suggest:key==='editor',can_reply:key!=='reader'});
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
    if (frame.type === 'join') queueMicrotask(()=>{
      this.emit({type:'ready',browser:true,agent:true,session_id:'runtime-session-1'});
      this.emit({type:'capabilities',session_id:'runtime-session-1',capabilities:{can_read:true,can_comment:true,can_edit:true}});
      this.emit({type:'presence',browser:true,agent:true,session_id:'runtime-session-1'});
    });
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
// under the real key: the origin. A pairing stored under any other key is
// not this origin's.
// Setting window.unpaired drops it, to exercise the panel before pairing.
window.pairingKey = location.origin;
if (!window.unpaired) localStorage.setItem('librepaper-local-connections', JSON.stringify({[window.pairingKey]:{token:'pair-token',expires:Date.now()/1000+3600}}));
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
  // A previously saved pairing is data, not permission to contact the local
  // companion. Wait for both the hosted conversation and the local Connect
  // prompt to settle before checking the request log.
  await until("saved-pair Connect prompt", () => page.evaluate(`Boolean(document.querySelector('.connect-required button'))`), 3000);
  // There is no longer a status line that reads "Waiting" once the runner is
  // ready with nothing in flight; the composer's own sendability is the
  // observable stand-in for "the runner is here and this can be sent".
  await until("runner connected",()=>page.evaluate('document.querySelector(".chat-form")?.dataset.cansend === "true"'),10000);
  assert.equal(await page.evaluate("window.localCalls.length"), 0,
    "mounting with a saved pairing makes no local companion requests before Connect");
  // A full page reload must preserve the same opt-in boundary. localStorage
  // still contains the valid saved token, but the new page session waits for
  // another explicit Connect click.
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("reloaded live agent panel", () => page.evaluate("Boolean(document.querySelector('.agent-panel textarea[aria-label=Message]'))"), 10000);
  await until("reloaded saved-pair Connect prompt", () => page.evaluate(`Boolean(document.querySelector('.connect-required button'))`), 3000);
  await until("reloaded hosted conversation", () => page.evaluate('document.querySelector(".chat-form")?.dataset.cansend === "true"'), 10000);
  assert.equal(await page.evaluate("window.localCalls.length"), 0,
    "a full page reload with a saved pairing makes no local requests before Connect");
  await page.evaluate(`Array.from(document.querySelectorAll('.connect-required button')).find(b=>b.textContent.trim()==='Connect').click()`);
  await until("saved pairing connected by explicit choice", () => page.evaluate(
    `!document.querySelector('.connect-required') && Boolean(document.querySelector('.agent-settings'))`), 3000);
  assert.equal(await page.evaluate("window.localCalls.some(call=>call.route==='pair/request')"), false,
    "Connect reuses a valid saved pairing without requesting a new pairing");
  assert.equal(await page.evaluate("window.localCalls.some(call=>call.route==='connect/claim')"), false,
    "Connect reuses a valid saved pairing without claiming a new pairing");
  assert.ok(await page.evaluate("window.localCalls.some(call=>call.route==='health')"),
    "the companion is contacted only after the explicit Connect click");
  // The Chat tab is the default: the agent/access/start controls live there
  // now, so there is no separate Connection tab to land on first.
  assert.equal(await page.evaluate('document.querySelector("#agent-pane-chat").hidden'), false);
  assert.deepEqual(await page.evaluate(`Array.from(document.querySelectorAll("select[aria-label=Access] option")).map(o=>o.textContent.split(":")[0])`), ["Comment", "Edit"]);
  // The agent settings are always visible when paired.
  assert.equal(await page.evaluate("Boolean(document.querySelector('.agent-settings'))"), true);
  if (process.env.AGENT_SCREENSHOT) {
    await choose("Access", "commenter");
    const shot = await page.command("Page.captureScreenshot", { format: "png" });
    writeFileSync(process.env.AGENT_SCREENSHOT, Buffer.from(shot.data, "base64"));
  }
  await page.evaluate('document.querySelector("#agent-tab-chat").click()');
  // The resize handle is gone; the textarea auto-grows with its content
  // instead, up to a viewport-relative cap, and shrinks back when the text
  // is removed.
  assert.equal(await page.evaluate("Boolean(document.querySelector('.resize-handle'))"), false,
    "the manual resize handle is gone");
  const oneLineHeight = await page.evaluate("document.querySelector('.chat-form textarea').getBoundingClientRect().height");
  await page.evaluate(`(()=>{const field=document.querySelector('.chat-form textarea');field.value=Array.from({length:10},(_,i)=>'line '+i).join('\\n');field.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await until("textarea grows with multiple lines",
    () => page.evaluate(`document.querySelector('.chat-form textarea').getBoundingClientRect().height > ${oneLineHeight} + 40`), 1000);
  await page.evaluate(`(()=>{const field=document.querySelector('.chat-form textarea');field.value='';field.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await until("textarea shrinks back once the draft is cleared",
    () => page.evaluate(`document.querySelector('.chat-form textarea').getBoundingClientRect().height <= ${oneLineHeight} + 4`), 1000);
  assert.equal(await page.evaluate("Boolean(document.querySelector('.chat-form label'))"), false, "the input needs no visible Message label");
  // The pane carries the padding, not the panel: the tab strip is flush to
  // the top of the panel, as the collaboration panel's is.
  await until("composer stays at the bottom of the pane", () => page.evaluate("(()=>{const pane=document.querySelector('#agent-pane-chat');const form=document.querySelector('.chat-form').getBoundingClientRect();const bottom=pane.getBoundingClientRect().bottom-parseFloat(getComputedStyle(pane).paddingBottom);return Math.abs(form.bottom-bottom)<2;})()"), 1000);
  assert.equal(await page.evaluate('window.sockets[0].url.includes("secret-token")'),false);
  assert.equal(await page.evaluate('new URL(window.sockets[0].url).searchParams.get("k")'),"secret");
  assert.deepEqual(await page.evaluate('window.sockets[0].sent[0]'),{type:"join",token:"secret-token",role:"user"});

  // Activity comes from task events, independently of connection presence.
  // The status is the last line of the transcript, never beside the composer's
  // buttons, so they do not move as it changes; there is no more "Working"/"Disconnected"/"Waiting"/"Interrupted"
  // line, and no working-dots animation to count.
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'activity',status:'working'})");
  await until("working indicator", () => page.evaluate('document.querySelector(".agent-progress")?.textContent.includes("Working")'), 1000);
  assert.equal(await page.evaluate('Boolean(document.querySelector(".chat-form [role=status]"))'), false, "no status sits among the composer's buttons");
  // Working tasks are not listed anywhere. Queued tasks render as .queued-request
  // rows. Stopping is the composer's job, not the activity disclosure's.
  assert.equal(await page.evaluate('!document.querySelector(".queued-request[data-task-id=\\"activity\\"]")'),
    true, "a working task has no queued-request row");
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'queued-task',status:'queued'})");
  await until("queued task row", () => page.evaluate('document.querySelector(".queued-request[data-task-id=\\"queued-task\\"]")'), 1000);
  assert.ok(
    await page.evaluate('document.querySelector(".queued-request[data-task-id=\\"queued-task\\"] .queued-label")?.textContent.includes("Queued")'),
    "a queued task row shows Queued label");
  assert.ok(
    await page.evaluate('Array.from(document.querySelectorAll(".queued-request[data-task-id=\\"queued-task\\"] button")).some(b=>b.textContent.trim()==="Cancel")'),
    "a queued task offers Cancel button");
  await page.evaluate("window.sockets[0].emit({type:'presence',browser:true,agent:true})");
  assert.match(await page.evaluate('document.querySelector(".agent-progress")?.textContent'), /Working/);
  // With the runner away the composer still sends: sending is what starts
  // the chosen agent.
  await page.evaluate("window.sockets[0].emit({type:'presence',browser:true,agent:false})");
  await until("composer sendable while the runner is away", () => page.evaluate('document.querySelector(".chat-form")?.dataset.cansend === "true"'), 1000);
  assert.ok(await page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Send a message to start the agent.")'),
    "the empty transcript names the missing runner, not the old badge text");
  await page.evaluate("window.sockets[0].emit({type:'presence',browser:true,agent:true}); window.sockets[0].emit({type:'task',task_id:'activity',status:'needs_input'})");
  await until("waiting for input", () => page.evaluate('document.querySelector(".agent-progress")?.textContent === "Waiting for permission"'), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'activity',status:'working'}); window.sockets[0].emit({type:'task',task_id:'activity',status:'completed'})");
  await until("no status once idle", () => page.evaluate('!document.querySelector(".agent-progress")'), 1000);
  await page.evaluate("window.setProps({diagnostics:[{severity:'warning',file:'librepaper.tex',line:12,message:'Reference undefined',revision:'render-sha',source:'source line'}]})");
  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='Explain this';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));})()`);
  await until("agent reply",()=>page.evaluate('document.querySelector("[role=log]")?.textContent.includes("<img")'),10000);
  assert.equal(await page.evaluate("Boolean(window.injected||document.querySelector('[role=log] img'))"),false);
  // The quiet transcript drops the visible "You"/"Agent" labels; they are
  // sr-only now, not gone from the accessibility tree.
  assert.equal(await page.evaluate('Boolean(document.querySelector(".chat-author"))'), false,
    "author names are visually hidden in the quiet transcript");
  assert.ok(await page.evaluate('Boolean(document.querySelector("[role=log] .sr-only"))'),
    "author names stay in the accessibility tree as sr-only text");
  // The quiet gutter is narrower, so the avatar must shrink with it rather
  // than spill over the message beside it.
  assert.ok(await page.evaluate(`Array.from(document.querySelectorAll("[role=log] .chat-message.starts")).every((row) => row.querySelector(".avatar").getBoundingClientRect().right <= row.querySelector(".chat-bubble").getBoundingClientRect().left)`),
    "avatars stay clear of their messages");
  const posted=await page.evaluate('window.sockets[0].sent.find(frame=>frame.type==="message")');
  assert.equal(posted.context.file,"paper.md");
  assert.deepEqual(posted.context.diagnostics, [{severity:'warning',file:'librepaper.tex',line:12,message:'Reference undefined',revision:'render-sha',source:'source line'}]);
  assert.deepEqual(posted.context.selection,{path:"paper.md",exact:"A passage",prefix:"",suffix:"",position:null});
  assert.equal(await page.evaluate(`document.querySelector('.chat-form button[type=submit]')?.textContent.trim()`), "Send",
    "the composer exposes a visible Send button");
  await page.evaluate(`(()=>{const field=document.querySelector('.chat-form textarea');field.value='Send button request';field.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await until("draft enables Send", () => page.evaluate(`!document.querySelector('.chat-form button[type=submit]').disabled`), 1000);
  await page.evaluate(`document.querySelector('.chat-form button[type=submit]').click()`);
  await until("Send button submits the draft", () => page.evaluate(`window.sockets[0].sent.some(frame=>frame.type==='message'&&frame.text==='Send button request')`), 1000);

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
  // resumed. The card offers exactly the options the agent named, labelled
  // by kind, and never treats a missing response as consent.
  await page.evaluate(`window.sockets[0].emit({type:'task',task_id:'input-task',status:'needs_input',context:{input:{request_id:'approval-1',kind:'permission',message:'mcp__librepaper__document_read',options:[{id:'allow-once',label:'Allow this run',kind:'allow_once'},{id:'reject',label:'Reject this',kind:'reject_once'}],details:{command:'git status',files:['paper.md'],diff:'-old\\n+new'}}}})`);
  await until("permission request", () => page.evaluate('document.querySelector(".permission-card")?.textContent.includes("Permission required")'), 1000);
  assert.match(await page.evaluate('document.querySelector(".permission-card p")?.textContent'), /Read the current document/,
    "the card humanizes the agent's own mcp tool name rather than showing the wire name");
  assert.equal(
    await page.evaluate(`Array.from(document.querySelectorAll('.permission-card .setup-actions button')).map(b=>b.textContent.trim()).join('|')`),
    "Allow|Reject",
    "the card labels the agent's own options by kind, in kind order, and invents nothing extra",
  );
  // Command/files/diff sit behind a collapsed "Details" disclosure now,
  // rather than spread across the card by default.
  assert.equal(await page.evaluate('document.querySelector(".permission-info")?.open'), false,
    "the permission details start collapsed");
  await page.evaluate('document.querySelector(".permission-info summary").click()');
  assert.match(await page.evaluate('document.querySelector(".permission-details")?.textContent'), /git status.*paper\.md/s);
  assert.match(await page.evaluate('document.querySelector(".permission-details pre")?.textContent'), /-old.*\+new/s);
  await page.evaluate('Array.from(document.querySelectorAll(".permission-card .setup-actions button")).find(b=>b.textContent.trim()==="Reject").click()');
  await until("permission response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='input'&&frame.response?.option==='reject')"), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'input-task',status:'working',text:'Continuing'})");
  await until("permission cleared", () => page.evaluate('!document.querySelector(".permission-card")'), 1000);

  // An option kind the card does not recognise keeps the agent's own label
  // instead of guessing one for it, and there is no invented Cancel button
  // beside the agent's own choices (the old .input-request card had one;
  // the .permission-card renders only the options the agent actually sent).
  await page.evaluate(`window.sockets[0].emit({type:'task',task_id:'other-kind-task',status:'needs_input',context:{input:{request_id:'approval-2',kind:'permission',message:'Write the file?',options:[{id:'allow-once',label:'Sure, go ahead',kind:'weird'}]}}})`);
  await until("unknown-kind permission request", () => page.evaluate('document.querySelector(".permission-card")?.textContent.includes("Write the file?")'), 1000);
  assert.equal(
    await page.evaluate(`Array.from(document.querySelectorAll('.permission-card .setup-actions button')).map(b=>b.textContent.trim()).join('|')`),
    "Sure, go ahead",
    "an unrecognised option kind keeps the agent's own label, and no extra Cancel option is invented",
  );
  await page.evaluate('document.querySelector(".permission-card .setup-actions button").click()');
  await until("unknown-kind response", () => page.evaluate("window.sockets[0].sent.some(frame=>frame.type==='input'&&frame.response?.option==='allow-once')"), 1000);
  await page.evaluate("window.sockets[0].emit({type:'task',task_id:'other-kind-task',status:'completed'})");
  await until("unknown-kind permission cleared", () => page.evaluate('!document.querySelector(".permission-card")'), 1000);

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
  await until("local app paired", () => page.evaluate(`!document.querySelector('.connect-required') && Boolean(document.querySelector('.agent-settings'))`), 3000);
  await until("agents listed", () => page.evaluate(`Boolean(document.querySelector('select[aria-label=Agent] option[value=claude]'))`), 2000);

  // The "Companion settings" text button is gone from the settings line; each
  // pane now carries its own icon button to the same page in its header, and
  // only the Chat pane also carries Clear conversation.
  assert.ok(await page.evaluate(`Boolean(document.querySelector('#agent-pane-chat button[aria-label="Local companion settings"]'))`),
    "the Chat pane header offers a way to the companion settings page");
  assert.ok(await page.evaluate(`Boolean(document.querySelector('#agent-pane-chat button[aria-label="Clear conversation"]'))`),
    "the Chat pane header offers Clear conversation");
  assert.ok(await page.evaluate(`Boolean(document.querySelector('#agent-pane-tasks button[aria-label="Local companion settings"]'))`),
    "the Tasks pane header also offers a way to the companion settings page");
  assert.equal(await page.evaluate(`Boolean(document.querySelector('#agent-pane-tasks button[aria-label="Clear conversation"]'))`), false,
    "Clear conversation is only in the Chat pane");

  const panel = await page.evaluate("document.body.textContent");
  for (const gone of ["librepaper agent connect", "Copy connection instructions", "LIBREPAPER_CHAT_TOKEN", "--background", "npx skills add"]) {
    assert.doesNotMatch(panel, new RegExp(gone.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
      `the sidebar no longer hands out "${gone}"`);
  }

  const startWith = async (role) => {
    await choose("Access", role);
    await choose("Agent", "claude");
    await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='test';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await page.evaluate(`document.querySelector('.chat-form button[type=submit]').click()`);
    await until(role + " started", () => page.evaluate("window.localCalls.some(call=>call.route==='assistant')"), 2000);
    await until(role + " settled", () => page.evaluate("document.querySelector('select[aria-label=Access]').disabled === false"), 2000);
  };
  // The session below exercises editing tasks (tighten, fix), which only an
  // editor may run: a suggestion is an editor's track change.
  await startWith("editor");
  const started = await page.evaluate("window.localCalls.find(call=>call.route==='assistant')");
  assert.equal(started.body.agent, "claude");
  assert.equal(started.body.chat_token, "secret-token");
  assert.equal(started.body.connection, undefined, "the assistant call carries no separate connection name");
  // The link crosses loopback exactly once, in the assistant call itself.
  // Everything afterwards refers to the document by conversation and link.
  assert.match(started.body.link, /#k=editor/);
  // There is no separate route to register a connection; the sidebar never
  // calls one.
  assert.equal(await page.evaluate("window.localCalls.some(call=>call.route==='connections')"), false);
  // The agent settings stay visible at all times when paired, and nothing in
  // the pane header stops the agent: sending, changing a select or clearing the
  // conversation is how its state changes.
  assert.equal(await page.evaluate(`document.querySelector("#agent-pane-chat").hidden`), false);
  assert.ok(await page.evaluate(`Boolean(document.querySelector(".agent-settings"))`),
    "the agent settings stay visible when an assistant is running");
  assert.equal(await page.evaluate('Boolean(document.querySelector(".pane-header [aria-label^=\\"Stop\\"]"))'), false,
    "the pane header has no Stop button");
  // The model picker appears only while the runner announces a model option,
  // and choosing in it asks the runner to change in place, with no restart.
  assert.equal(await page.evaluate("Boolean(document.querySelector('select[aria-label=Model]'))"), false,
    "no model picker before the runner announces a model");
  const modelOptions = [{id:'model',name:'Model',category:'model',current:'sonnet',
    choices:[{value:'sonnet',name:'Sonnet'},{value:'opus',name:'Opus'}]}];
  await page.evaluate(`window.sockets.at(-1).emit({type:'options',options:${JSON.stringify(modelOptions)}})`);
  await until("model picker shown", () => page.evaluate("Boolean(document.querySelector('select[aria-label=Model]'))"), 1000);
  assert.deepEqual(
    await page.evaluate(`Array.from(document.querySelector('select[aria-label=Model]').options).map(o=>[o.value,o.textContent])`),
    [['sonnet','Sonnet'],['opus','Opus']]);
  assert.equal(await page.evaluate("document.querySelector('select[aria-label=Model]').value"), 'sonnet');
  await page.evaluate("window.localCalls=[]");
  await choose("Model", "opus");
  await until("set_option sent", () => page.evaluate(`window.sockets.at(-1).sent.some(frame=>frame.type==='set_option'&&frame.option==='model'&&frame.value==='opus'&&Boolean(frame.id))`), 1000);
  assert.equal(await page.evaluate("window.localCalls.length"), 0, "changing the model does not restart the agent");
  assert.equal(await page.evaluate("localStorage.getItem('librepaper.agent.model.claude')"), 'opus');
  // The runner answers with a fresh options frame; the picker follows it.
  await page.evaluate(`window.sockets.at(-1).emit({type:'options',options:[{...${JSON.stringify(modelOptions[0])},current:'opus'}]})`);
  await until("model picker follows the runner", () => page.evaluate("document.querySelector('select[aria-label=Model]').value==='opus'"), 1000);
  // The model name joins the composer status while a task works.
  await page.evaluate("window.sockets.at(-1).emit({type:'task',task_id:'model-status',status:'working'})");
  await until("model in composer status", () => page.evaluate('document.querySelector(".agent-progress")?.textContent.includes("Opus")'), 1000);
  await page.evaluate("window.sockets.at(-1).emit({type:'task',task_id:'model-status',status:'completed'})");
  // An empty list means no model choice.
  await page.evaluate("window.sockets.at(-1).emit({type:'options',options:[]})");
  await until("model picker hidden", () => page.evaluate("!document.querySelector('select[aria-label=Model]')"), 1000);
  // The agent leaving the conversation.
  await page.evaluate("window.sockets.at(-1).emit({type:'presence',browser:true,agent:false})");

  await choose("Agent", "pi");
  // A known limitation of an agent's ACP route is stated next to the agent,
  // not left to be inferred from a task that silently does nothing.
  await until("pi note shown", () => page.evaluate(`Boolean(document.querySelector('[data-agent-note=pi]'))`), 1000);
  assert.match(
    await page.evaluate(`document.querySelector('[data-agent-note=pi]').textContent`),
    /incomplete MCP support/,
  );
  // Starting it would fetch its adapter, so the panel says so beforehand.
  assert.match(
    await page.evaluate(`document.querySelector("[data-adapter-fetch]")?.textContent`),
    /downloads its adapter the first time/,
  );

  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='First';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate("document.querySelector('textarea[placeholder]').value"),"First\n");

  // A runner going away leaves drafting alone, and sending would start one.
  // Shift+Enter is a newline and IME Enter cannot send the request
  // prematurely.
  await page.evaluate("window.sockets[0].emit({type:'presence',agent:false,browser:true})");
  const sentBefore = await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length");
  await until("draft while away", () => page.evaluate('!document.querySelector(".chat-form textarea").disabled && document.querySelector(".chat-form").dataset.cansend==="true"'), 1000);
  await page.evaluate(`(()=>{const input=document.querySelector(".chat-form textarea");input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',shiftKey:true,bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true}));})()`);
  assert.equal(await page.evaluate(`document.querySelector(".chat-form textarea").value`), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[0].sent.filter(frame=>frame.type==='message').length"), sentBefore);

  // Browser loss triggers automatic socket recovery; credentials, transcript
  // and locally drafted requests remain in the browser session.
  await page.evaluate("window.sockets[0].close()");
  await until("automatic recovery", () => page.evaluate('window.sockets.length===2'), 3000);
  await until("fresh recovered channel", () => page.evaluate("window.sockets.length===2"), 1000);
  assert.equal(await page.evaluate(`document.querySelector(".chat-form textarea").value`), "First\n\n");
  assert.equal(await page.evaluate("window.sockets[1].sent.filter(frame=>frame.type==='message').length"), sentBefore,
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

  // A real rendered quote has no source coordinates or source revision.
  await page.evaluate("window.setProps({request:{id:'rendered-quote',task:{kind:'tighten',scope:'selection'},selection:{exact:'Rendered passage',render_digest:'rendered-tree',position:null}}})");
  await until("rendered quote attached",()=>page.evaluate("document.querySelector('.chat-form textarea').value.includes('Tighten')"),1000);
  await page.evaluate("document.querySelector('.chat-form').requestSubmit()");
  await until("rendered quote submitted",()=>page.evaluate("window.sockets[1].sent.some(frame=>frame.context?.render_digest==='rendered-tree')"),1000);
  const rendered = await page.evaluate("window.sockets[1].sent.find(frame=>frame.context?.render_digest==='rendered-tree')");
  assert.equal(rendered.context.selection.exact,"Rendered passage");
  assert.equal(rendered.context.revision,undefined);
  await page.evaluate("window.sockets[1].emit({type:'task',task_id:'partial',status:'cancelled',context:{results:{suggestions:['kept'],confirmed:[{kind:'application'}],refused:120,unresolved:3}}})");
  await until("partial effects visible",()=>page.evaluate("document.querySelector('[role=log]').textContent.includes('Source changes applied.') && document.querySelector('[role=log]').textContent.includes('120 document operations were refused')"),1000);
  await page.evaluate("window.sockets[1].emit({type:'capabilities',session_id:'replacement-session',capabilities:{cancel:true}})");
  await until("session boundary visible",()=>page.evaluate("document.querySelector('[role=log]').textContent.includes('does not remember earlier messages')"),1000);

  // Every role starts with a real bounded link, and reuses existing share
  // links. The link itself carries the access, so getting the key right in
  // the `assistant` call body is what actually bounds the agent.
  const linkFor = (role) => page.evaluate(`window.localCalls.filter(call=>call.route==='assistant').at(-1).body.link.includes('#k=${role}')`);
  const connectAs = async (role) => {
    await page.evaluate('document.querySelector("#agent-tab-chat").click()');
    // Changing the agent or access while one is running restarts it, so the
    // selects alone are the whole path; with nothing running, sending starts it.
    if (await page.evaluate(`document.querySelector("select[aria-label=Access]").value`) === role) {
      await choose("Access", role === "editor" ? "commenter" : "editor");
    }
    await choose("Agent", "claude");
    const running = await page.evaluate(`window.activeRunner !== null`);
    await page.evaluate("window.localCalls=[]");
    await choose("Access", role);
    if (!running) {
      await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='test';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
      await page.evaluate(`document.querySelector('.chat-form button[type=submit]').click()`);
    }
    await until(role + " started", () => page.evaluate("window.localCalls.some(call=>call.route==='assistant')"), 2000);
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
  // A level with no existing share link mints one rather than failing.
  // Leave Edit first: every change of access restarts, and so mints.
  await page.evaluate("window.localCalls=[]");
  await choose("Access", "commenter");
  await until("commenter restart", () => page.evaluate("window.localCalls.some(call=>call.route==='assistant')"), 2000);
  await until("commenter restart settled", () => page.evaluate("document.querySelector('select[aria-label=Access]').disabled === false"), 2000);
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
  await until("composer stays at the bottom of the pane", () => page.evaluate("(()=>{const pane=document.querySelector('#agent-pane-chat');const form=document.querySelector('.chat-form').getBoundingClientRect();const bottom=pane.getBoundingClientRect().bottom-parseFloat(getComputedStyle(pane).paddingBottom);return Math.abs(form.bottom-bottom)<2;})()"), 1000);
  for (const tab of ["chat", "tasks"]) {
    await page.evaluate(`document.querySelector('#agent-tab-${tab}').click()`);
    assert.equal(await page.evaluate("document.querySelector('.agent-panel').scrollWidth <= document.querySelector('.agent-panel').clientWidth"), true, tab + " fits the narrow sidebar");
  }
  await page.evaluate('document.querySelector("#agent-tab-tasks").focus(); document.querySelector("#agent-tab-tasks").dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowRight",bubbles:true}))');
  // Keyboard navigation follows the selected tab (Tasks wraps to Chat, the
  // only other tab now that Connection is gone).
  await until("keyboard tab navigation", () => page.evaluate('document.activeElement.id === "agent-tab-chat"'), 1000);

  const beforeNewConversation = await page.evaluate("({sockets:window.sockets.length,creates:window.calls.filter(call=>call.suffix==='').length})");
  // Clear conversation is a fresh runner boundary: it stops the local assistant
  // and clears the transcript/context. The agent settings remain visible.
  // A socket reconnect keeps the existing conversation and transcript instead.
  await page.evaluate('document.querySelector("#agent-tab-chat").click(); document.querySelector(\'button[aria-label="Clear conversation"]\').click()');
  await until("new conversation cleared", () => page.evaluate('document.querySelector("#agent-tab-chat").getAttribute("aria-selected")==="true"'), 2000);
  assert.equal(await page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'), false);
  assert.equal(await page.evaluate('window.localCalls.some(call => call.route === "assistant/stop")'), true);
  await until("new channel created",()=>page.evaluate(`window.sockets.length === ${beforeNewConversation.sockets + 1}`),10000);
  await page.evaluate("window.remount()");
  await until("new live channel",()=>page.evaluate(`window.sockets.length === ${beforeNewConversation.sockets + 2}`),10000);
  assert.equal(await page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'),false);
  assert.equal(await page.evaluate("window.calls.filter(call=>call.suffix==='').length"),beforeNewConversation.creates + 1);
  // Pairing, from an unpaired browser. The new flow uses pair/request which
  // is triggered by the Connect button, not a manual code entry.
  await page.evaluate("window.unpaired=true; localStorage.removeItem('librepaper-local-connections'); window.pairingAccepted=false");
  await page.evaluate("window.remount()");
  await page.evaluate('document.querySelector("#agent-tab-chat").click()');
  await until("pairing prompt shown", () => page.evaluate(`Boolean(document.querySelector('.connect-required button'))`), 3000);
  await until("unpaired hosted conversation settled", () => page.evaluate('document.querySelector(".chat-form")?.dataset.cansend === "true"'), 3000);
  assert.equal(await page.evaluate("window.localCalls.length"), 0,
    "mounting without a pairing makes no local companion requests before Connect");
  // The Connect button initiates the pairing flow. The way to the companion
  // settings page is no longer a text button inside .connect-required; it is
  // the pane header's icon button, present whether or not the app is paired.
  const requirementButtons = await page.evaluate(`Array.from(document.querySelectorAll('.connect-required button')).map(b=>b.textContent.trim())`);
  assert.ok(requirementButtons.includes("Connect"), `no Connect button: ${requirementButtons}`);
  assert.ok(await page.evaluate(`Boolean(document.querySelector('#agent-pane-chat button[aria-label="Local companion settings"]'))`),
    "the pane header still offers a way to the companion settings page while unpaired");
  await page.evaluate(`document.querySelector('#agent-pane-chat button[aria-label="Local companion settings"]').click()`);
  assert.equal(await page.evaluate("window.settingsCalls"), 1, "the not-paired warning offers a way to the companion settings page");
  // Try Connect when pairing is not yet accepted to trigger a refused response.
  await page.evaluate(`Array.from(document.querySelectorAll('.connect-required button')).find(b=>b.textContent.trim()==='Connect').click()`);
  await until("refused pairing is reported in place", () => page.evaluate(
    `document.querySelector('.connect-required [role=alert]')?.textContent.includes('approval was denied')`), 3000);
  assert.equal(await page.evaluate("window.localCalls.filter(call=>call.route==='pair/request').length"), 1,
    "an unpaired browser requests pairing only after Connect");

  // Pairing is scoped to the page's origin, not to a document: the request
  // names the origin and carries no project.
  assert.equal(await page.evaluate("window.pairRequestBodies?.at(-1)?.origin === location.origin"), true, "pairing is scoped to this origin");
  assert.equal(await page.evaluate("'project' in (window.pairRequestBodies?.at(-1) || {})"), false, "the pairing request carries no project");

  await page.evaluate("window.pairingAccepted=true");
  await page.evaluate(`Array.from(document.querySelectorAll('.connect-required button')).find(b=>b.textContent.trim()==='Connect').click()`);
  await until("pairing succeeds", () => page.evaluate(
    `!document.querySelector('.connect-required') && Boolean(document.querySelector('.agent-settings'))`), 3000);
  assert.equal(await page.evaluate("window.localCalls.filter(call=>call.route==='pair/request').length"), 2,
    "a successful Connect creates the missing pairing after the declined attempt");

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

  // A remounted panel reads the local status for its saved connection and
  // restores the agent and access that are actually running.
  await choose("Access", "commenter");
  // Nothing is attached yet, so sending is what starts the assistant.
  await page.evaluate("window.sockets.at(-1).emit({type:'presence',browser:true,agent:false})");
  await page.evaluate(`(()=>{const input=document.querySelector('textarea[placeholder]');input.value='test';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await page.evaluate(`document.querySelector('.chat-form button[type=submit]').click()`);
  await until("restoration test assistant started", () => page.evaluate("window.activeRunner !== null"), 2000);
  await page.evaluate("window.remount()");
  // The agent settings stay visible at all times. A remounted panel asks the
  // local app whether its assistant is running, and the selects show the
  // configuration that was running.
  assert.ok(await page.evaluate(`Boolean(document.querySelector(".agent-settings"))`),
    "the agent settings stay visible when an assistant is restored");
  await until("running configuration restored", () => page.evaluate(`document.querySelector('select[aria-label=Agent]')?.value==='claude' && document.querySelector('select[aria-label=Access]')?.value==='commenter'`), 3000);

  // A delayed status response from the old conversation must not overwrite a
  // newer conversation after its component has remounted again.
  await page.evaluate("window.delayAssistantStatus=true; window.remount()");
  await until("delayed assistant status is pending", () => page.evaluate("window.delayedAssistantStatuses.length===1"), 2000);
  await page.evaluate("window.conversationCreatesBeforePending=window.calls.filter(call=>call.suffix==='').length");
  await page.evaluate('document.querySelector("#agent-tab-chat").click(); document.querySelector(\'button[aria-label="Clear conversation"]\').click()');
  await until("conversation changed while status was pending", () => page.evaluate("window.calls.filter(call=>call.suffix==='').length === window.conversationCreatesBeforePending+1"), 3000);
  await page.evaluate("window.delayedAssistantStatuses[0].resolve(Response.json({running:true,state:'ready'}))");
  await new Promise(resolve=>setTimeout(resolve,50));
  assert.notEqual(await page.evaluate(`document.querySelector('select[aria-label=Agent]')?.value`), "pi",
    "an older runner status cannot replace the configuration of a newer conversation");
  assert.notEqual(await page.evaluate(`document.querySelector('select[aria-label=Access]')?.value`), "editor",
    "an older runner status cannot replace the access choice of a newer conversation");

  console.log("agent-browser: context, recovery, draft, keyboard, short layout, pairing and local session persistence passed");
} finally {
  await page?.close();
  if(server) await new Promise(resolve=>server.close(resolve));
  rmSync(temporary,{recursive:true,force:true});
}
