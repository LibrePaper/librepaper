// Real Reader responsive smoke test. The room is the only module replaced: its
// y-state is a genuine Yjs directory, so Reader, Editor, Files and CSS mount.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-reader-responsive-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
const yjs = join(root, "web/node_modules/yjs/dist/yjs.mjs");
writeFileSync(room, `
import * as Y from ${JSON.stringify(yjs)};
const encode = b => btoa(String.fromCharCode(...b));
const server = new Y.Doc();
const files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const id = "main"; const text = new Y.Text(); text.insert(0, Array.from({length: 180}, (_, i) => "line " + i + " source content").join("\\n"));
files.set(id, text); paths.set(id, "main.html"); meta.set("main", id);
for (let i = 0; i < 70; i++) { const file = new Y.Text(); file.insert(0, 'chapter ' + i); files.set('chapter-' + i, file); paths.set('chapter-' + i, 'chapter-' + i + '.html'); }
const update = encode(Y.encodeStateAsUpdate(server));
export function openRoom(slug, {onMessage, onConnected}) {
  window.roomReceive = onMessage;
  window.roomSent = [];
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      window.roomSent.push(message);
      if (message.type === "y-open") queueMicrotask(() => onMessage({type:"y-state", update, count:1}));
      return {ok:true};
    },
    sendLive(message) { window.roomSent.push(message); return {ok:true}; },
    close() {}
  };
}
`);
writeFileSync(entry, `
import ${JSON.stringify(join(root,"web/src/styles/app.css"))};
window.testErrors = [];
window.addEventListener('error', event => window.testErrors.push(event.message));
window.addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
// Agent chat is delivered by its private socket, rather than HTTP polling.
globalThis.WebSocket = class {
  constructor(url) {
    if (!String(url).includes('/chat/')) throw new Error('Unexpected socket: ' + url);
    queueMicrotask(() => this.onopen?.());
  }
  send(raw) {
    if (JSON.parse(raw).type !== 'join') return;
    queueMicrotask(() => {
      this.onmessage?.({data:JSON.stringify({type:'ready',agent:true})});
      for (let i = 0; i < 40; i++) this.onmessage?.({data:JSON.stringify({type:'message',message:{id:'reply-'+i,role:'agent',text:('Reply '+i+' ').repeat(30)}})});
    });
  }
  close() { this.onclose?.(); }
};
globalThis.fetch = async (url, init = {}) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({});
  if (path === '/api/documents/paper') return Response.json({title:'Responsive',created_at:'test',role:'editor',source_format:'html',docs_origin:location.origin,can_moderate:true,can_see_sharing:true});
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'chat', token:'private'});
  if (path.includes('/comments')) return { ok:true, json:async()=>({comments:[]}) };
  return { ok:false, json:async()=>({}) };
};
const { default: Reader } = await import(${JSON.stringify(join(root,"web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root,"web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);
let serverHttp, b;
try {
  await build({ configFile:false, root:join(root,"web"), plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
  serverHttp=createServer((req,res)=>{
    if(req.url==="/docs/paper") {
      res.setHeader("content-type","text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    if(req.url.startsWith('/raw/')) {
      res.setHeader('content-type','text/html');
      res.end('<body style="height:4000px">A long document</body>');
      return;
    }
    const file=join(out,req.url.split("?")[0].slice(1));
    try { res.setHeader("content-type",file.endsWith(".css")?"text/css":"text/javascript");res.end(readFileSync(file)); }
    catch { res.statusCode=404;res.end(); }
  });
  await new Promise((resolve,reject)=>{serverHttp.once("error",reject);serverHttp.listen(0,"127.0.0.1",resolve)});
  b=await browser("chromium",join(temp,"profile"),19000+Math.floor(Math.random()*1000));
  const url = `http://127.0.0.1:${serverHttp.address().port}/docs/paper`;
  await b.resize(1280,900);
  await b.navigate(url);
  await until("source and files",()=>b.evaluate("document.querySelector('.cm-editor') && document.querySelectorAll('.explorer-row').length > 60"), 10000);
  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const click = async (selector) => { await b.evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`); await flush(); };
  const nav = (name) => `.mobile-pane-nav [aria-label="${name}"]`;
  const visible = (selector) => b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); return Boolean(node?.getClientRects().length && getComputedStyle(node).visibility !== 'hidden'); })()`);
  const bounded = async () => {
    const bounds = await b.evaluate(`({width:innerWidth,height:innerHeight,scrollWidth:document.documentElement.scrollWidth,scrollHeight:document.documentElement.scrollHeight,children:[...document.body.children].map(n=>({tag:n.tagName,cls:n.className,top:n.getBoundingClientRect().top,bottom:n.getBoundingClientRect().bottom}))})`);
    assert.ok(bounds.scrollHeight <= bounds.height + 1 && bounds.scrollWidth <= bounds.width + 1, 'the outer page fits the viewport: ' + JSON.stringify(bounds));
    assert.equal(await b.evaluate('document.querySelector("main.reader").getBoundingClientRect().bottom <= innerHeight + 1'), true);
  };
  assert.equal(await visible('.editorpane'),true);
  assert.equal(await visible('.viewport'),true);
  await bounded();
  await b.evaluate(`(() => {
    window.savedEditor = document.querySelector('.cm-editor');
    window.savedFrame = document.querySelector('.viewport iframe');
    document.querySelector('.cm-scroller').scrollTop = 700;
    document.querySelector('.explorer-scroll').scrollTop = 300;
  })()`);
  await flush();
  const sourceTop = await b.evaluate('document.querySelector(".cm-scroller").scrollTop');
  assert.ok(sourceTop > 0);
  const filesTop = await b.evaluate('document.querySelector(".explorer-scroll").scrollTop');
  assert.ok(filesTop > 0);
  await click('.sidebar-activity [aria-label="Collaboration"]');
  const selectDiscussionTab = async (label) => {
    await b.evaluate(`Array.from(document.querySelectorAll('.collab-tabs [role="tab"]')).find(node => node.textContent.trim().startsWith(${JSON.stringify(label)})).click()`);
    await flush();
  };
  const frameMessage = async (message) => {
    await b.evaluate(`window.dispatchEvent(new MessageEvent('message', { origin:location.origin,
      source:document.querySelector('.viewport iframe').contentWindow,
      data:{librepaper:true,...${JSON.stringify(message)}} }))`);
    await flush();
  };
  await until('collaboration tabs', () => b.evaluate('document.querySelectorAll(".collab-tabs [role=tab]").length === 3'), 3000);
  await b.evaluate(`window.roomReceive({type:'hello',comments:[
    {id:'point',seq:1,motivation:'commenting',exact:'',point:true,position:0,suffix:'A long document',body:'Point discussion',resolved:true,replies:[]},
    {id:'plain-highlight',seq:2,motivation:'highlighting',exact:'long',position:2,color:'#aabbcc',body:'',replies:[]},
    {id:'discussed-highlight',seq:3,motivation:'highlighting',exact:'document',position:7,color:'#ddeeff',body:'Discuss this highlight',replies:[]}
  ]})`);
  await frameMessage({type:'ready',text:'A long document',images:[]});
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("Point discussion")'), true);
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("Discuss this highlight")'), true);
  assert.equal(await visible('.collaboration article[id$=plain-highlight]'), false);
  await selectDiscussionTab('Highlights');
  assert.equal(await b.evaluate('Array.from(document.querySelectorAll(".collaboration article")).filter(node=>node.getClientRects().length).length'), 2);
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("Point discussion")'), false);
  await click('#collaboration-highlight-plain-highlight [aria-label="Reply"]');
  await b.evaluate(`(() => {
    const field=document.querySelector('#collaboration-highlight-plain-highlight textarea');
    field.value='Discussion attached to a highlight'; field.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  await flush();
  await b.evaluate('document.querySelector("#collaboration-highlight-plain-highlight form").requestSubmit()');
  await flush();
  assert.equal(await b.evaluate('window.roomSent.filter(message=>message.type==="reply").at(-1).comment_id'), 'plain-highlight');
  await selectDiscussionTab('Comments');
  assert.equal(await visible('#collaboration-comment-plain-highlight'), true, 'adding a discussion shows the same highlight in Comments');
  assert.equal(await b.evaluate('document.querySelector("#collaboration-comment-plain-highlight").innerText.includes("Discussion attached to a highlight")'), true);
  await selectDiscussionTab('Chat');
  await b.evaluate(`(() => {
    const input=document.querySelector('.collaboration .chat-form textarea');
    input.value='Draft for co-authors'; input.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  await selectDiscussionTab('Highlights');
  await b.evaluate("window.roomReceive({type:'chat',id:'incoming',creator:'Coauthor',text:'A new message',created:'2026-09-09T12:00:00Z'})");
  await flush();
  assert.equal(await b.evaluate('Boolean(document.querySelector(".collab-tabs .unread"))'), true);
  await selectDiscussionTab('Chat');
  assert.equal(await b.evaluate('document.querySelector(".collaboration .chat-form textarea").value'), 'Draft for co-authors');
  assert.equal(await b.evaluate('Boolean(document.querySelector(".collab-tabs .unread"))'), false);
  await frameMessage({type:'focus',id:'point'});
  assert.equal(await b.evaluate('document.querySelector(".collab-tabs [aria-selected=true]").textContent.trim()'), 'Comments');
  assert.equal(await b.evaluate('document.querySelector(".collaboration article[id$=point]").classList.contains("collapsed")'), false);
  assert.equal(await b.evaluate('Array.from(document.querySelectorAll("[id]")).map(node=>node.id).length === new Set(Array.from(document.querySelectorAll("[id]")).map(node=>node.id)).size'), true, 'tab instances never duplicate DOM IDs');

  await click('.collaboration [aria-label="Point comment"]');
  await frameMessage({type:'selection',selector:{exact:'',point:true,position:0,prefix:'',suffix:'A long document'},rect:{top:80,bottom:80,left:80,right:80}});
  await click('#selectionbar > button');
  await b.evaluate(`(() => {
    const input=document.querySelector('#commentForm textarea');
    input.value='A new point comment'; input.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  await flush();
  await b.evaluate('document.querySelector("#commentForm").requestSubmit()');
  await flush();
  const submittedPoint = await b.evaluate('window.roomSent.filter(message=>message.type==="comment").at(-1)');
  assert.equal(submittedPoint.point, true);
  assert.equal(submittedPoint.position, 0);
  assert.equal(submittedPoint.exact, '');
  assert.equal(submittedPoint.motivation, 'commenting');
  assert.equal(submittedPoint.body, 'A new point comment');
  await b.evaluate(`window.roomReceive({type:'submission-failed',temp_id:${JSON.stringify(submittedPoint.temp_id)},message:'Offline'})`);
  await flush();
  assert.equal(await visible('.pending-recovery'), true);
  await b.evaluate(`(() => {
    const item=Array.from(document.querySelectorAll('.pending-recovery details')).find(node=>node.textContent.includes('A new point comment'));
    item.open=true;
    Array.from(item.querySelectorAll('button')).find(node=>node.textContent==='Retry').click();
  })()`);
  assert.equal(await b.evaluate(`window.roomSent.filter(message=>message.temp_id===${JSON.stringify(submittedPoint.temp_id)}).length`), 2, 'retry retains the annotation idempotency key');
  await b.evaluate(`(() => {
    const item=Array.from(document.querySelectorAll('.pending-recovery details')).find(node=>node.textContent.includes('A new point comment'));
    Array.from(item.querySelectorAll('button')).find(node=>node.textContent==='Discard draft').click();
  })()`);
  await flush();
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("A new point comment")'), false);

  await selectDiscussionTab('Highlights');
  await click('.collaboration [aria-label="Highlight"]');
  await frameMessage({type:'selection',selector:{exact:'long',position:2,prefix:'A ',suffix:' document'},rect:{top:80,bottom:100,left:80,right:120}});
  await b.evaluate(`(() => {
    const picker=document.querySelector('#selectionbar input[type=color]');
    picker.value='#123abc'; picker.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  await flush();
  await click('#selectionbar > button');
  const submittedHighlight = await b.evaluate('window.roomSent.filter(message=>message.type==="comment").at(-1)');
  assert.equal(submittedHighlight.motivation, 'highlighting');
  assert.equal(submittedHighlight.color, '#123abc');
  assert.equal(submittedHighlight.point, undefined);
  if (process.env.LIBREPAPER_REVIEW_SCREENSHOT) {
    const screenshot = await b.command('Page.captureScreenshot', {format:'png'});
    writeFileSync(process.env.LIBREPAPER_REVIEW_SCREENSHOT, Buffer.from(screenshot.data,'base64'));
  }

  await click('.sidebar-activity [aria-label="Changes"]');
  assert.equal(await b.evaluate('document.querySelector(".sidebar").innerText.includes("Suggest a change")'), true);
  await click('.sidebar-activity [aria-label="Files"]');
  assert.equal(await b.evaluate('document.querySelector(".explorer-scroll").scrollTop'), filesTop);
  assert.equal(await b.evaluate('document.querySelector(".filelist .panel-actions").getBoundingClientRect().bottom <= document.querySelector(".explorer-scroll").getBoundingClientRect().top'), true);

  await b.resize(390,844); await flush();
  await click(nav('Document'));
  assert.equal(await visible('.viewport'),true);
  assert.equal(await visible('.editorpane'),false);
  assert.equal(await visible('.sidebar'),false);
  await bounded();
  await b.evaluate('window.savedFrame.contentWindow.scrollTo(0, 800)');
  await click(nav('Source'));
  assert.equal(await visible('.editorpane'),true);
  assert.equal(await visible('.viewport'),false);
  assert.equal(await b.evaluate('document.querySelector(".cm-editor") === window.savedEditor'), true);
  assert.ok(Math.abs(await b.evaluate('document.querySelector(".cm-scroller").scrollTop') - sourceTop) < 2);
  await click(nav('Files'));
  assert.equal(await visible('.filelist'),true);
  assert.equal(await visible('.editorpane'),false);
  await click('.explorer-row[title="chapter-1.html"]');
  assert.equal(await visible('.editorpane'),true, 'choosing a file opens its source');
  await click(nav('Document'));
  assert.equal(await b.evaluate('document.querySelector(".viewport iframe") === window.savedFrame'), true);
  assert.equal(await b.evaluate('window.savedFrame.contentWindow.scrollY'),800);

  await click(nav('Agent'));
  await until('agent replies',()=>b.evaluate('document.querySelectorAll(".agent-panel .chat-message").length === 40'),10000);
  await b.evaluate('Array.from(document.querySelectorAll(".agent-tabs [role=tab]")).find(node=>node.textContent.trim()==="Chat").click()');
  await flush();
  await b.evaluate(`(() => {
    const input = document.querySelector('.agent-panel .chat-form textarea');
    input.value = 'An unsent thought'; input.dispatchEvent(new Event('input',{bubbles:true}));
    document.querySelector('.agent-panel .chat-transcript').scrollTop = 100;
  })()`);
  await flush();
  await click(nav('Source'));
  await click(nav('Agent'));
  assert.equal(await b.evaluate('document.querySelector(".agent-panel .chat-form textarea").value'), 'An unsent thought');
  assert.equal(await b.evaluate('document.querySelector(".agent-panel .chat-transcript").scrollTop'),100);
  await bounded();

  await b.resize(900,900); await flush();
  await click(nav('Document'));
  assert.equal(await visible('.viewport'),true);
  assert.equal(await visible('.editorpane'),false);
  assert.equal(await visible('.sidebar'),true);
  await click(nav('Source'));
  assert.equal(await visible('.editorpane'),true);
  assert.equal(await visible('.viewport'),false);
  await bounded();
  await b.resize(1280,900); await flush();
  assert.equal(await visible('.viewport'),true);
  assert.equal(await visible('.editorpane'),true, 'widening restores the split');
  assert.equal(await b.evaluate('document.querySelector(".cm-editor") === window.savedEditor'), true);

  for (const saved of ['source','document']) {
    await b.resize(390,844);
    await b.evaluate(`localStorage.setItem('librepaper-layout', JSON.stringify(${JSON.stringify(saved)}))`);
    await b.navigate(url);
    await until('reader remount',()=>b.evaluate('document.querySelector(".cm-editor") !== null'),10000);
    await click(nav('Document')); assert.equal(await visible('.viewport'), true);
    await click(nav('Source')); assert.equal(await visible('.editorpane'), true);
    await bounded();
  }
  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log('responsive-browser: collaboration tabs, point comments, highlight discussions, custom colors, retry/discard, unread chat, drafts, viewport bounds and saved layouts passed');
} finally { await b?.close(); serverHttp?.close(); rmSync(temp,{recursive:true,force:true}); }
