// Real Reader responsive smoke test. The room is the only module replaced: its
// doc-state is a genuine Loro directory, so Reader, Editor, Files and CSS mount.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-reader-responsive-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
writeFileSync(room, `
import { LoroDoc, LoroText, EphemeralStore } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const id = "main"; const text = new LoroText(); text.insert(0, Array.from({length: 180}, (_, i) => "line " + i + " source content").join("\\n"));
files.setContainer(id, text); paths.set(id, "main.html"); meta.set("main", id);
for (let i = 0; i < 70; i++) { const file = new LoroText(); file.insert(0, 'chapter ' + i); files.setContainer('chapter-' + i, file); paths.set('chapter-' + i, 'chapter-' + i + '.html'); }
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
const peers = new EphemeralStore(30000);
const remoteKeys = Array.from({length:5}, (_, i) => 'user:remote-tab-' + i);
const remotePeople = remoteKeys.map((key, i) => [key,{name:["Alex Collaborator","Jordan Editor","Sam Reviewer","Riley Author","Casey Reader"][i],tab:'remote-tab-' + i,color:["#5577bb","#bb5577","#55aa77","#aa7755","#7755aa"][i]}]);
export function openRoom(slug, {onMessage, onConnected}) {
  window.roomReceive = onMessage;
  window.roomSent = [];
  const publishPresence = () => {
    for (const [key, person] of remotePeople) {
      peers.set(key, person);
      onMessage({type:"doc-presence", update:encode(peers.encode(key))});
    }
  };
  window.refreshTestPresence = publishPresence;
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      window.roomSent.push(message);
      if (message.type === "doc-open") queueMicrotask(() => {
        onMessage({type:"doc-state", protocol:"librepaper.room.v3", vector, durableVector: vector, updates:[update]});
        publishPresence();
        onMessage({type:"doc-peers", count:6});
      });
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
// The agent panel shows its chat only once the local app is paired, so the
// harness seeds a pairing and answers the app's two probe routes.
localStorage.setItem('librepaper-local-connections', JSON.stringify({[location.origin]:{token:'pair-token',expires:Date.now()/1000+3600}}));
globalThis.fetch = async (url, init = {}) => {
  const path = String(url);
  if (path.startsWith('http://127.0.0.1:8763/')) {
    if (path.endsWith('/health')) return Response.json({service:'librepaper-local',protocol:[2],version:'test',instance:'one'});
    if (path.endsWith('/capabilities')) return Response.json({builders:[]});
    return Response.json({});
  }
  if (path.endsWith('/me')) return Response.json({name:'Very Long Tester Account Name',handle:'tester-handle',provider:'github',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({title:'A deliberately long responsive workspace title',created_at:'test',role:'editor',source_format:'html',docs_origin:location.origin,can_moderate:true,can_see_sharing:true});
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'chat', token:'private'});
  if (path.includes('/comments')) return { ok:true, json:async()=>({comments:[]}) };
  return { ok:false, json:async()=>({}) };
};
const { default: Reader } = await import(${JSON.stringify(join(root,"web/src/components/Reader.svelte"))});
const companion = await import(${JSON.stringify(join(root,"web/src/lib/companion/client.js"))});
companion.configure({ project: 'paper', origin: location.origin, active: true });
await companion.retry();
const { mount } = await import(${JSON.stringify(join(root,"web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);
let serverHttp, b;
try {
  await build({ configFile:false, root:join(root,"web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
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
    try { res.setHeader("content-type",contentType(file));res.end(readFileSync(file)); }
    catch { res.statusCode=404;res.end(); }
  });
  await new Promise((resolve,reject)=>{serverHttp.once("error",reject);serverHttp.listen(0,"127.0.0.1",resolve)});
  b=await browser("chromium",join(temp,"profile"),19000+Math.floor(Math.random()*1000));
  const url = `http://127.0.0.1:${serverHttp.address().port}/docs/paper`;
  await b.resize(1280,900);
  await b.navigate(url);
  await until("source and files",()=>b.evaluate("document.querySelector('.cm-editor') && document.querySelectorAll('.explorer-row').length > 60"), 10000);
  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const click = async (selector) => { await b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node.click(); })()`); await flush(); };
  const clickText = async (selector, label) => {
    await b.evaluate(`[...document.querySelectorAll(${JSON.stringify(selector)})].find(node => node.textContent.trim().startsWith(${JSON.stringify(label)})).click()`);
    await flush();
  };
  const panelId = (name) => ({ Files:'files', Outline:'outline', Agent:'agent', Collaboration:'collaboration', Changes:'changes', Share:'share', Diagnostics:'diagnostics', History:'history' })[name];
  const panelMenu = '.explorer-menu[data-state="open"]';
  const nav = async (name) => {
    await click('.compact-panels-trigger[aria-label="Panels"]');
    await until('Panels menu', () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`), 3000);
    await click(`${panelMenu} [data-panel-id="${panelId(name)}"]`);
  };
  // Narrow windows put the face switch beside the bottom menu; an adapted
  // split above the compact breakpoint keeps it in the top bar.
  const face = (name) => `.face-switch [aria-label="${name}"]`;
  const visible = (selector) => b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); return Boolean(node?.getClientRects().length && getComputedStyle(node).visibility !== 'hidden'); })()`);
  const bounded = async () => {
    const bounds = await b.evaluate(`(() => {
      const width = document.documentElement.clientWidth;
      const overflow = [...document.body.querySelectorAll('*')].flatMap(node => {
        const rect = node.getBoundingClientRect();
        if (!node.getClientRects().length || rect.width === 0 || (rect.left >= -1 && rect.right <= width + 1)) return [];
        const style = getComputedStyle(node);
        return [{ tag: node.tagName.toLowerCase(), id: node.id || undefined,
          cls: typeof node.className === 'string' ? node.className.slice(0, 100) : '',
          left: Math.round(rect.left), right: Math.round(rect.right), width: Math.round(rect.width),
          position: style.position, scrollWidth: node.scrollWidth, clientWidth: node.clientWidth }];
      }).sort((a, b) => (b.right - width) - (a.right - width)).slice(0, 12);
      return { width: innerWidth, height: innerHeight, scrollWidth: document.documentElement.scrollWidth,
        scrollHeight: document.documentElement.scrollHeight, overflow };
    })()`);
    assert.ok(bounds.scrollHeight <= bounds.height + 1 && bounds.scrollWidth <= bounds.width + 1, 'the outer page fits the viewport: ' + JSON.stringify(bounds));
    assert.equal(await b.evaluate('document.querySelector("main.reader").getBoundingClientRect().bottom <= innerHeight + 1'), true);
  };
  assert.equal(await visible('.editorpane'),true);
  assert.equal(await visible('.viewport'),true);
  await bounded();

  // Every panel at the narrowest column a reader can drag to. A panel that
  // asks to be wider than the column it is in does not get a wider column: it
  // gets a header scrolling sideways and a menu pushed out under the sidebar's
  // own clipping, which is why this measures each of them rather than trusting
  // one row of tabs to speak for all of them. The width is the panel's own:
  // the rail of icons is added to it, not taken out of it, so 240 here is 240
  // of panel. This runs before the test takes hold of the editor below, since
  // the History panel puts its own workspace where the source is.
  // Put back afterwards rather than removed: the Reader writes this variable
  // on the element, and taking it off is what every selector here looks for.
  const columnWas = await b.evaluate(`document.querySelector('[style*="--librepaper-sidebar"]').style.getPropertyValue("--librepaper-sidebar")`);
  const panelAt = async (label, width) => {
    await b.evaluate(`document.querySelector('[style*="--librepaper-sidebar"]').style.setProperty("--librepaper-sidebar", "${width}px")`);
    await flush();
    return b.evaluate(`(() => {
      const slot = [...document.querySelectorAll('.panel-slot')].find(node => !node.hidden);
      if (!slot) return { label: ${JSON.stringify(label)}, open: false };
      const box = slot.getBoundingClientRect();
      const wide = [...slot.querySelectorAll('*')].filter(node => {
        if (!node.getClientRects().length) return false;
        const style = getComputedStyle(node);
        if (style.position === 'fixed' || style.position === 'absolute') return false;
        return node.getBoundingClientRect().right > box.right + 1;
      }).map(node => node.tagName + '.' + String(node.className).slice(0, 40));
      return { label: ${JSON.stringify(label)}, open: true, width: Math.round(box.width),
        scroll: slot.scrollWidth - slot.clientWidth, wide: wide.slice(0, 5) };
    })()`);
  };
  for (const name of ['Outline', 'Collaboration', 'Changes', 'Agent', 'History', 'Diagnostics', 'Share']) {
    const button = `.sidebar-activity [aria-label^="${name}"]`;
    assert.equal(await b.evaluate(`Boolean(document.querySelector(${JSON.stringify(button)}))`), true, name + ' has an icon');
    await click(button);
    const panel = await panelAt(name, 240);
    assert.equal(panel.open, true, name + ' opens its panel: ' + JSON.stringify(panel));
    assert.ok(panel.width >= 239, 'the panel gets the width the column was dragged to: ' + JSON.stringify(panel));
    assert.ok(panel.scroll <= 1, name + ' fits the narrowest column: ' + JSON.stringify(panel));
    assert.deepEqual(panel.wide, [], name + ' keeps everything inside the column: ' + JSON.stringify(panel));
  }
  // A short window: the icons scroll rather than the workspace controls being
  // clipped off the bottom of the rail with no sign they were ever there.
  await b.resize(1280, 420); await flush();
  const rail = await b.evaluate(`(() => { const bar = document.querySelector('.sidebar-activity');
    const bottom = document.querySelector('.activity-bottom');
    return { fits: bottom.getBoundingClientRect().bottom <= bar.getBoundingClientRect().bottom + 1,
      scrolls: document.querySelector('.activity-sections').scrollHeight > document.querySelector('.activity-sections').clientHeight }; })()`);
  assert.equal(rail.fits, true, 'the workspace controls stay inside the rail: ' + JSON.stringify(rail));
  await b.resize(1280, 900); await flush();
  await click('.sidebar-activity [aria-label="Files"]');
  await b.evaluate(`document.querySelector('[style*="--librepaper-sidebar"]').style.setProperty("--librepaper-sidebar", ${JSON.stringify(columnWas)})`);
  await flush();
  await until('the source comes back', () => b.evaluate("Boolean(document.querySelector('.cm-editor'))"), 10000);

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
  // A menu panel paints its own surface. Skeleton owns the content element, so
  // the chrome is easy to lose to scoping: catch a see-through menu here.
  await clickText('.menubar-item', 'File');
  await until('file menu', () => b.evaluate('Boolean(document.querySelector(".explorer-menu[data-state=open]"))'), 3000);
  const menuChrome = await b.evaluate(`(() => { const style = getComputedStyle(document.querySelector('.explorer-menu[data-state=open]'));
    return {background:style.backgroundColor, shadow:style.boxShadow}; })()`);
  assert.notEqual(menuChrome.background, 'rgba(0, 0, 0, 0)', 'the menu is opaque: ' + JSON.stringify(menuChrome));
  assert.notEqual(menuChrome.shadow, 'none', 'the menu keeps its shadow');
  await click('body');
  await clickText('.menubar-item', 'View');
  await until('view menu', () => b.evaluate('Boolean(document.querySelector(".explorer-menu[data-state=open]"))'), 3000);
  await click('body');

  await click('.sidebar-activity [aria-label="Collaboration"]');
  const selectDiscussionTab = (label) => clickText('.collab-tabs [role="tab"]', label);
  const frameMessage = async (message) => {
    await b.evaluate(`window.dispatchEvent(new MessageEvent('message', { origin:location.origin,
      source:document.querySelector('.viewport iframe').contentWindow,
      data:{librepaper:true,...${JSON.stringify(message)}} }))`);
    await flush();
  };
  await until('collaboration tabs', () => b.evaluate('document.querySelectorAll(".collab-tabs [role=tab]").length === 3'), 3000);
  // The sidebar can be dragged down to PANES.sidebar.min. The tab row has to
  // stay a single row there rather than overflow or wrap, and a label that is
  // too wide to fit has to keep its full text reachable as a tooltip.
  const tabRow = async (width) => {
    await b.evaluate(`document.querySelector('[style*="--librepaper-sidebar"]').style.setProperty("--librepaper-sidebar", "${width}px")`);
    await flush();
    return b.evaluate(`(() => { const bar = document.querySelector(".collab-tabs");
      const tabs = [...bar.querySelectorAll('[role="tab"]')];
      return { overflow: bar.scrollWidth - bar.clientWidth, rows: new Set(tabs.map(t => t.offsetTop)).size,
        tabs: tabs.map(t => ({ label: t.textContent.trim(), title: t.title, clipped: t.scrollWidth > t.clientWidth })) }; })()`);
  };
  const narrowTabs = await tabRow(240);
  assert.ok(narrowTabs.overflow <= 1, 'the tab row fits the narrowest sidebar: ' + JSON.stringify(narrowTabs));
  assert.equal(narrowTabs.rows, 1, 'the tabs stay on one row: ' + JSON.stringify(narrowTabs));
  for (const tab of narrowTabs.tabs) {
    assert.equal(tab.title, tab.label, 'a truncated tab still says what it is: ' + JSON.stringify(tab));
  }
  await b.evaluate('document.querySelector(\'[style*="--librepaper-sidebar"]\').style.removeProperty("--librepaper-sidebar")');
  await flush();
  await b.evaluate(`window.roomReceive({type:'hello',comments:[
    {id:'passage',seq:1,motivation:'commenting',exact:'A long',position:0,suffix:' document',body:'Passage discussion',resolved:true,replies:[]},
    {id:'plain-highlight',seq:2,motivation:'highlighting',exact:'long',position:2,color:'#aabbcc',body:'',replies:[]},
    {id:'discussed-highlight',seq:3,motivation:'highlighting',exact:'document',position:7,color:'#ddeeff',body:'Discuss this highlight',replies:[]}
  ]})`);
  await frameMessage({type:'ready',text:'A long document'});
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("Passage discussion")'), true);
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("Discuss this highlight")'), true);
  assert.equal(await visible('.collaboration article[id$=plain-highlight]'), false);
  await selectDiscussionTab('Highlights');
  assert.equal(await b.evaluate('Array.from(document.querySelectorAll(".collaboration article")).filter(node=>node.getClientRects().length).length'), 2);
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("Passage discussion")'), false);
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
  // Mounted while its tab was hidden, the composer must still get a real
  // height once shown rather than the zero it measured in the background.
  await until('chat composer measured once shown', () => b.evaluate('document.querySelector(".collaboration .chat-form textarea").getBoundingClientRect().height > 30'), 1000);
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
  await frameMessage({type:'focus',id:'passage'});
  assert.equal(await b.evaluate('document.querySelector(".collab-tabs [aria-selected=true]").textContent.trim()'), 'Comments');
  assert.equal(await b.evaluate('document.querySelector(".collaboration article[id$=passage]").classList.contains("collapsed")'), false);
  assert.equal(await b.evaluate('Array.from(document.querySelectorAll("[id]")).map(node=>node.id).length === new Set(Array.from(document.querySelectorAll("[id]")).map(node=>node.id)).size'), true, 'tab instances never duplicate DOM IDs');

  // Every annotation starts the same way: select the words, choose the verb.
  // There is no mode to arm and nothing to put away afterwards.
  await frameMessage({type:'selection',selector:{exact:'A long',position:0,prefix:'',suffix:' document'},rect:{top:80,bottom:100,left:80,right:120}});
  assert.equal(await visible('#selectionbar'), true, 'a selection raises the bar over it');
  await click('#selectionbar .verb');
  // No dialog: the draft is a card in the column, in the place its note will
  // take, and the column it is in is the one that opens.
  assert.equal(await visible('.composer'), true, 'choosing a verb opens the draft in the column');
  assert.equal(await b.evaluate('document.querySelector(".collab-tabs [aria-selected=true]").textContent.trim()'), 'Comments');
  await b.evaluate(`(() => {
    const input=document.querySelector('.composer textarea');
    input.value='A new comment'; input.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  await flush();
  await b.evaluate(`Array.from(document.querySelectorAll('.composer button')).find(node=>node.textContent.trim()==='Comment').click()`);
  await flush();
  assert.equal(await visible('.composer'), false, 'sending closes the draft');
  const submitted = await b.evaluate('window.roomSent.filter(message=>message.type==="comment").at(-1)');
  assert.equal(submitted.position, 0);
  assert.equal(submitted.exact, 'A long');
  assert.equal(submitted.motivation, 'commenting');
  assert.equal(submitted.body, 'A new comment');
  await b.evaluate(`window.roomReceive({type:'submission-failed',temp_id:${JSON.stringify(submitted.temp_id)},message:'Offline'})`);
  await flush();
  assert.equal(await visible('.pending-recovery'), true);
  await b.evaluate(`(() => {
    const item=Array.from(document.querySelectorAll('.pending-recovery details')).find(node=>node.textContent.includes('A new comment'));
    item.open=true;
    Array.from(item.querySelectorAll('button')).find(node=>node.textContent==='Retry').click();
  })()`);
  assert.equal(await b.evaluate(`window.roomSent.filter(message=>message.temp_id===${JSON.stringify(submitted.temp_id)}).length`), 2, 'retry retains the annotation idempotency key');
  await b.evaluate(`(() => {
    const item=Array.from(document.querySelectorAll('.pending-recovery details')).find(node=>node.textContent.includes('A new comment'));
    Array.from(item.querySelectorAll('button')).find(node=>node.textContent==='Discard draft').click();
  })()`);
  await flush();
  assert.equal(await b.evaluate('document.querySelector(".collaboration").innerText.includes("A new comment")'), false);

  // Nothing is ever armed, so Escape has only the bar to put away.
  await frameMessage({type:'selection',selector:{exact:'long',position:2,prefix:'A ',suffix:' document'},rect:{top:80,bottom:100,left:80,right:120}});
  assert.equal(await visible('#selectionbar'), true);
  await b.evaluate("window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))");
  await flush();
  assert.equal(await visible('#selectionbar'), false, 'Escape puts the bar away');

  // Highlighting arms nothing either: select the words, choose the verb.
  // The swatches hang under Highlight, on the strip of ink below its icon,
  // and picking one is the highlight.
  await frameMessage({type:'selection',selector:{exact:'long',position:2,prefix:'A ',suffix:' document'},rect:{top:80,bottom:100,left:80,right:120}});
  assert.equal(await b.evaluate(`Array.from(document.querySelectorAll("#selectionbar .verb")).map(node=>node.getAttribute("aria-label")??node.querySelector("button").getAttribute("aria-label")).join("|")`),
    'Comment|Highlight', 'a passage can be commented on or highlighted, and nothing else');
  await click('#selectionbar .ink');
  await b.evaluate(`(() => {
    const picker=document.querySelector('#selectionbar input[type=color]');
    picker.value='#123abc';
    picker.dispatchEvent(new Event('input',{bubbles:true}));
    picker.dispatchEvent(new Event('change',{bubbles:true}));
  })()`);
  await flush();
  const submittedHighlight = await b.evaluate('window.roomSent.filter(message=>message.type==="comment").at(-1)');
  assert.equal(submittedHighlight.motivation, 'highlighting');
  assert.equal(submittedHighlight.color, '#123abc');
  if (process.env.LIBREPAPER_REVIEW_SCREENSHOT) {
    const screenshot = await b.command('Page.captureScreenshot', {format:'png'});
    writeFileSync(process.env.LIBREPAPER_REVIEW_SCREENSHOT, Buffer.from(screenshot.data,'base64'));
  }

  await click('.sidebar-activity [aria-label="Changes"]');
  assert.equal(await b.evaluate('document.querySelector(".sidebar").innerText.includes("Track changes")'), true);
  await click('.sidebar-activity [aria-label="Files"]');
  assert.equal(await b.evaluate('document.querySelector(".explorer-scroll").scrollTop'), filesTop);
  assert.equal(await b.evaluate('document.querySelector(".filelist .panel-actions").getBoundingClientRect().bottom <= document.querySelector(".explorer-scroll").getBoundingClientRect().top'), true);

  for (const width of [320,390,600,760]) {
    await b.evaluate('window.refreshTestPresence()');
    await flush();
    await b.resize(width,844); await flush();
    const header = await b.evaluate(`(() => {
      const toolbar=document.querySelector('nav');
      const menuItems=[...document.querySelectorAll('.menubar-item')];
      const account=toolbar?.querySelector('.account');
      const pills=[...toolbar?.querySelectorAll('.connection-pill')||[]];
      const presence=toolbar?.querySelector('.presence');
      const face=document.querySelector('.face-switch');
      const panels=document.querySelector('.compact-panels-trigger');
      const mobileNav=document.querySelector('.mobile-pane-nav');
      const rect=node=>{const r=node.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height}};
      const items=[panels,face,...pills,account,presence].filter(Boolean);
      const boxes=items.map(rect);
      const overlaps=boxes.some((a,i)=>boxes.slice(i+1).some(b=>a.left < b.right-1 && b.left < a.right-1 && a.top < b.bottom-1 && b.top < a.bottom-1));
      return {width:innerWidth,items:boxes,overlaps,presenceAvatars:presence?.querySelectorAll('.avatar').length||0,presenceMore:presence?.querySelector('.presence-more')?.textContent.trim()||'',
        menus:menuItems.map(n=>{const r=n.getBoundingClientRect();return {text:n.textContent.trim(),visible:!!n.getClientRects().length && getComputedStyle(n).display!=='none' && getComputedStyle(n).visibility!=='hidden' && r.width>0 && r.height>0 && r.left>=0 && r.right<=innerWidth};}),
        faceInTop:!!face?.closest('nav.reader-nav'), faceVisible:!!face?.getClientRects().length, mobileNavExists:!!mobileNav, panelsInTop:!!panels?.closest('nav.reader-nav')};
    })()`);
    assert.equal(header.overlaps,false,`toolbar controls do not overlap at ${width}px: ${JSON.stringify(header)}`);
    assert.ok(header.items.every(item=>item.width>0 && item.height>0 && item.left>=-1 && item.right<=width+1 && item.top>=0 && item.bottom<=844),`top bar controls stay visible and inside the viewport at ${width}px: ${JSON.stringify(header)}`);
    assert.equal(header.presenceAvatars,0,`collaborator avatars stay out of the compact bar at ${width}px: ${JSON.stringify(header)}`);
    assert.equal(header.mobileNavExists,false,`no bottom navigation bar at ${width}px: ${JSON.stringify(header)}`);
    for (const label of ['File','View']) assert.equal(header.menus.some(item=>item.visible&&item.text===label),false,`${label} menu text is hidden at ${width}px: ${JSON.stringify(header)}`);
    assert.equal(header.faceInTop,true,`source/document switch lives in the top bar at ${width}px: ${JSON.stringify(header)}`);
    assert.equal(header.faceVisible,true,`source/document switch is visible at ${width}px: ${JSON.stringify(header)}`);
    assert.equal(header.panelsInTop,true,`panels hamburger menu lives in the top bar at ${width}px: ${JSON.stringify(header)}`);
  }
  await b.resize(900,900); await flush();
  assert.equal(await b.evaluate('Boolean(document.querySelector(".face-switch")?.closest("nav"))'),true,'the source/document switch stays in the top toolbar above 760px');
  await b.resize(390,844); await flush();
  await click(face('Document'));
  const compactNav = await b.evaluate(`(() => {
    const navbar = document.querySelector('nav.reader-nav');
    const trigger = navbar?.querySelector('.compact-panels-trigger[aria-label="Panels"]');
    const faceSwitch = navbar?.querySelector('.face-switch');
    const triggerRect = trigger?.getBoundingClientRect();
    const faceRect = faceSwitch?.getBoundingClientRect();
    const controls = [trigger, ...[...(faceSwitch?.querySelectorAll('button') || [])]];
    const rects = controls.map(control => control?.getBoundingClientRect());
    const styles = controls.map(control => getComputedStyle(control));
    const icons = controls.map(control => control?.querySelector('svg'));
    return { triggers: navbar?.querySelectorAll('.compact-panels-trigger').length || 0,
      face: Boolean(navbar?.querySelector('.face-switch')),
      faceLabels: [...navbar?.querySelectorAll('.face-switch button')||[]].map(button=>button.getAttribute('aria-label')),
      height: triggerRect?.height || 0,
      sizes: rects.map(rect => [rect?.width || 0, rect?.height || 0]),
      radii: styles.map(style => style.borderRadius),
      borders: styles.map(style => [style.borderTopWidth, style.borderTopStyle]),
      iconSizes: icons.map(icon => { const style = icon && getComputedStyle(icon); return style ? [style.width, style.height] : null; }),
      gaps: [
        rects[1]?.left - rects[0]?.right,
        rects[2]?.left - rects[1]?.right,
      ],
      immediatelyLeft: Boolean(trigger && faceSwitch && trigger.compareDocumentPosition(faceSwitch) & Node.DOCUMENT_POSITION_FOLLOWING
        && triggerRect.right <= faceRect.left && faceRect.left - triggerRect.right <= parseFloat(getComputedStyle(trigger.parentElement).gap) + 1) };
  })()`);
  assert.equal(compactNav.triggers,1,'top bar has one Panels trigger');
  assert.equal(compactNav.face,true,'top bar includes the source/document switch beside Panels');
  assert.deepEqual(compactNav.faceLabels,['Document','Source'],'both mobile workspace faces remain available');
  assert.equal(compactNav.height,44,'Panels trigger is 44px tall');
  assert.deepEqual(compactNav.sizes,[[44,44],[44,44],[44,44]],'all three top bar controls are 44px square');
  assert.equal(new Set(compactNav.radii).size,1,'all three controls share a corner radius');
  assert.equal(new Set(compactNav.borders.map(border => border.join('|'))).size,1,'all three controls share a border');
  assert.equal(new Set(compactNav.iconSizes.map(size => size?.join('|'))).size,1,'all three controls share an icon size');
  assert.deepEqual(compactNav.gaps,[8,8],'the three controls are separated by equal 8px gaps');
  assert.equal(compactNav.immediatelyLeft,true,'the Panels trigger sits immediately left of the face switch');
  assert.equal(await b.evaluate('document.querySelector(".compact-panels-trigger[aria-label=Panels]").innerText.trim()'),'','the Panels trigger is icon only');
  // The menu bar is hidden on a phone; its menus open from the top of the
  // Panels menu instead, below the bar, with their own items.
  for (const [id, item] of [["file", "Settings…"], ["view", "Preview this file"]]) {
    await click('.compact-panels-trigger[aria-label="Panels"]');
    await until(`Panels menu before ${id}`, () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(`${panelMenu} [data-menu-id="${id}"]`)}))`), 3000);
    await click(`${panelMenu} [data-menu-id="${id}"]`);
    const opened = await until(`${id} menu from Panels`, () => b.evaluate(`(() => {
      const node=[...document.querySelectorAll(".explorer-menu[data-state=open] [role=menuitem]")].find(n => n.textContent.includes(${JSON.stringify(item)}));
      if (!node || !node.getClientRects().length) return null;
      return { top: node.closest(".explorer-menu").getBoundingClientRect().top, bar: document.querySelector("nav.reader-nav").getBoundingClientRect().bottom, panels: Boolean(document.querySelector(".explorer-menu[data-state=open] .compact-panels-items")) };
    })()`), 3000);
    assert.ok(opened.top >= opened.bar, `the ${id} menu opens below the top bar: ${JSON.stringify(opened)}`);
    assert.equal(opened.panels, false, `the Panels menu closes when ${id} opens`);
    await b.command("Input.dispatchKeyEvent", {type:"keyDown",key:"Escape",code:"Escape",windowsVirtualKeyCode:27});
    await b.command("Input.dispatchKeyEvent", {type:"keyUp",key:"Escape",code:"Escape",windowsVirtualKeyCode:27});
    await until(`${id} menu closed`, () => b.evaluate('!document.querySelector(".explorer-menu[data-state=open]")'), 3000);
  }
  await click('.compact-panels-trigger[aria-label="Panels"]');
  await until('compact Panels menu', () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`), 3000);
  // The Panels menu now starts with File/Edit/Insert/View menu items, then has a
  // separator, then the panel items.
  const menuItems = await b.evaluate(`(() => [...document.querySelectorAll(${JSON.stringify(`${panelMenu} [data-menu-id]`)})].map(node => ({id:node.dataset.menuId,text:node.textContent.trim()})))()`);
  assert.ok(menuItems.length >= 1, 'File menu item is present in Panels menu');
  assert.ok(menuItems.some(item => item.id === 'file' && item.text.includes('File')), 'File menu item has correct id and text');
  const menuPanels = await b.evaluate(`(() => [...document.querySelectorAll(${JSON.stringify(`${panelMenu} [data-panel-id]`)})].map(node => ({id:node.dataset.panelId,label:node.getAttribute('aria-label'),text:node.textContent.trim()})))()`);
  assert.deepEqual(menuPanels.map(item => [item.id,item.label,item.text]), [
    ['files','Files','Files'],['outline','Outline','Outline'],['collaboration','Collaboration','Collaboration'],
    ['changes','Changes','Changes'],['agent','Agent','Agent'],['history','History','History'],
    ['share','Share','Share'],['diagnostics','Diagnostics','Diagnostics']
  ], 'the Panels menu names every allowed workspace panel');
  const workspaceActions = await b.evaluate(`(() => {
    const menu=document.querySelector(${JSON.stringify(panelMenu)});
    return ['settings','home','docs'].map(action=>{const node=menu.querySelector('[data-workspace-action="'+action+'"]');return {action,tag:node?.tagName,role:node?.getAttribute('role'),href:node?.getAttribute('href'),label:node?.textContent.trim()}});
  })()`);
  assert.deepEqual(workspaceActions.map(item=>item.action),['settings','home','docs'],'Settings, Home and Docs follow the panel controls');
  assert.equal(workspaceActions[0].role,'menuitem','Settings keeps the menu item semantics');
  assert.ok(workspaceActions[1].tag==='A' && workspaceActions[1].href==='/','Home remains a native link');
  assert.ok(workspaceActions[2].tag==='A' && workspaceActions[2].href==='/documentation','Docs remains a native link');
  const workspaceOrder = await b.evaluate(`(() => [...document.querySelector(${JSON.stringify(panelMenu)}).querySelectorAll('[data-panel-id],[data-workspace-action]')].map(node=>node.dataset.panelId||node.dataset.workspaceAction))()`);
  assert.deepEqual(workspaceOrder.slice(-3),['settings','home','docs'],'workspace links are below the panels in order');
  assert.equal(await b.evaluate(`(() => {const menu=document.querySelector(${JSON.stringify(panelMenu)});const settings=menu.querySelector('[data-workspace-action=settings]');return settings?.previousElementSibling?.matches('[role=separator]')})()`),true,'a separator divides panel controls from workspace links');
  await click(`${panelMenu} [data-workspace-action="settings"]`);
  await until('Settings dialog opened from Panels',()=>b.evaluate('Boolean(document.querySelector("[role=dialog]"))'),3000);
  await b.evaluate('document.querySelector("[role=dialog]")?.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true}))');
  await flush();
  await click('.compact-panels-trigger[aria-label="Panels"]');
  await until('Panels reopened after Settings',()=>b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`),3000);
  await click(`${panelMenu} [data-panel-id="files"]`);
  await click('.compact-panels-trigger[aria-label="Panels"]');
  await until('reopened Panels menu', () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`), 3000);
  assert.equal(await b.evaluate(`document.querySelector(${JSON.stringify(`${panelMenu} [data-panel-id="files"]`)})?.getAttribute('aria-current')`), 'true', 'the selected panel is marked current');
  // Exercise the menu through actual keyboard input on the browser protocol.
  await b.evaluate('document.querySelector(\'.compact-panels-trigger[aria-label="Panels"]\').focus()');
  const key = async (key, code, virtual) => {
    await b.command('Input.dispatchKeyEvent', {type:'keyDown',key,code,windowsVirtualKeyCode:virtual});
    await b.command('Input.dispatchKeyEvent', {type:'keyUp',key,code,windowsVirtualKeyCode:virtual});
    await flush();
  };
  await key('Escape','Escape',27);
  await until('pre-keyboard menu closed', () => b.evaluate(`!document.querySelector(${JSON.stringify(panelMenu)})`), 3000);
  assert.equal(await b.evaluate('document.activeElement?.matches(\'.compact-panels-trigger[aria-label="Panels"]\')'), true, 'closing the menu restores focus to Panels');
  await key('Enter','Enter',13);
  await until('keyboard opened Panels menu', () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`), 3000);
  await key('ArrowDown','ArrowDown',40);
  // The File/Edit/Insert/View entries come first, so the second item is Edit.
  assert.equal(await b.evaluate('(() => { const items=[...document.querySelectorAll(".explorer-menu[data-state=open] [role=menuitem]")]; return items.findIndex(item => item.hasAttribute("data-highlighted")); })()'), 1, 'ArrowDown highlights the next item');
  await key('Escape','Escape',27);
  await until('keyboard closed Panels menu', () => b.evaluate(`!document.querySelector(${JSON.stringify(panelMenu)})`), 3000);
  assert.equal(await b.evaluate('document.activeElement?.matches(\'.compact-panels-trigger[aria-label="Panels"]\')'), true, 'Escape restores focus to Panels');
  await click('.compact-panels-trigger[aria-label="Panels"]');
  await until('Panels menu for active selection', () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`), 3000);
  await click(`${panelMenu} [data-panel-id="files"]`);
  assert.equal(await visible('.filelist'), true, 'choosing the active Files panel keeps it open');
  assert.equal(await b.evaluate(`!document.querySelector(${JSON.stringify(panelMenu)})`),true,'choosing a panel closes the menu');
  await click(face('Document'));
  assert.equal(await visible('.filelist'), false, 'the face switch returns from Files to the document');
  assert.equal(await visible('.viewport'), true, 'the document face is visible after leaving Files');
  for (const [width,height] of [[320,844],[760,844],[320,420],[390,844]]) {
    await b.resize(width, height); await flush();
    await click('.compact-panels-trigger[aria-label="Panels"]');
    await until(`Panels menu at ${width}px`, () => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(panelMenu)}))`), 3000);
    if (process.env.LIBREPAPER_COMPACT_SCREENSHOT) {
      const screenshot = await b.command('Page.captureScreenshot', {format:'png'});
      writeFileSync(process.env.LIBREPAPER_COMPACT_SCREENSHOT, Buffer.from(screenshot.data,'base64'));
    }
    const menuBounds = await b.evaluate(`(() => { const menu=document.querySelector(${JSON.stringify(panelMenu)}); const items=document.querySelector('.compact-panels-items'); const rect=menu.getBoundingClientRect();
      const topBar=document.querySelector('nav.reader-nav').getBoundingClientRect();
      return {left:rect.left,right:rect.right,top:rect.top,bottom:rect.bottom,width:innerWidth,height:innerHeight,topBarBottom:topBar?.bottom,
        itemScrollHeight:items?.scrollHeight||0,itemClientHeight:items?.clientHeight||0}; })()`);
    assert.ok(menuBounds.left >= -1 && menuBounds.right <= width + 1, `menu fits horizontally at ${width}px: ${JSON.stringify(menuBounds)}`);
    assert.ok(menuBounds.top >= -1 && menuBounds.bottom <= height + 1, `menu fits vertically at ${width}x${height}: ${JSON.stringify(menuBounds)}`);
    assert.ok(menuBounds.top >= menuBounds.topBarBottom && menuBounds.top-menuBounds.topBarBottom <= 16 && menuBounds.bottom <= menuBounds.height + 1,
      `menu opens below the top bar and fits within the viewport at ${width}x${height}: ${JSON.stringify(menuBounds)}`);
    if (height < 500) {
      assert.ok(menuBounds.itemScrollHeight > menuBounds.itemClientHeight, 'the menu items scroll in a short viewport: ' + JSON.stringify(menuBounds));
      await b.evaluate(`(() => {const items=document.querySelector('.compact-panels-items');items.scrollTop=0;const r=items.getBoundingClientRect();window.scrollTarget={x:r.left+r.width/2,y:r.top+r.height/2};})()`);
      const point=await b.evaluate('window.scrollTarget');
      await b.command('Input.dispatchMouseEvent',{type:'mouseWheel',x:point.x,y:point.y,deltaX:0,deltaY:500});
      await flush();
      assert.ok(await b.evaluate('document.querySelector(".compact-panels-items").scrollTop>0'),'a wheel gesture scrolls the menu items in a short viewport');
      await b.evaluate('document.querySelector(".compact-panels-items").scrollTop=0');
      await b.command('Emulation.setTouchEmulationEnabled',{enabled:true,maxTouchPoints:1});
      await b.command('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[{x:point.x,y:point.y}]});
      await b.command('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[{x:point.x,y:point.y-180}]});
      await b.command('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
      await flush();
      assert.ok(await b.evaluate('document.querySelector(".compact-panels-items").scrollTop>0'),'a touch gesture scrolls the menu items in a short viewport');
      await b.command('Emulation.setTouchEmulationEnabled',{enabled:false});
      await b.evaluate('document.querySelector(".compact-panels-items").scrollTop=0');
      await b.command('Input.dispatchMouseEvent',{type:'mouseWheel',x:point.x,y:point.y,deltaX:0,deltaY:5000});
      await flush();
      assert.ok(await b.evaluate(`(() => {const doc=[...document.querySelectorAll(${JSON.stringify(`${panelMenu} [data-workspace-action]`)})].find(node=>node.dataset.workspaceAction==='docs');const clip=document.querySelector('.compact-panels-items').getBoundingClientRect();const r=doc.getBoundingClientRect();const x=(r.left+r.right)/2,y=(r.top+r.bottom)/2;const hit=document.elementFromPoint(x,y);return r.top>=clip.top-1 && r.bottom<=clip.bottom+1 && (hit===doc || doc.contains(hit))})()`), 'Docs is visible inside the menu scrollport after a real scroll');
    }
    await click('body');
  }
  await b.resize(390,844); await flush();
  await click(face('Document'));
  assert.equal(await visible('.viewport'),true);
  assert.equal(await visible('.editorpane'),false);
  assert.equal(await visible('.sidebar'),false);
  await bounded();
  await b.evaluate('window.savedFrame.contentWindow.scrollTo(0, 800)');
  await click(face('Source'));
  assert.equal(await visible('.editorpane'),true);
  assert.equal(await visible('.viewport'),false);
  assert.equal(await b.evaluate('document.querySelector(".cm-editor") === window.savedEditor'), true);
  assert.ok(Math.abs(await b.evaluate('document.querySelector(".cm-scroller").scrollTop') - sourceTop) < 2);
  await nav('Files');
  assert.equal(await visible('.filelist'),true);
  assert.equal(await visible('.editorpane'),false);
  await click('.explorer-row[title="chapter-1.html"]');
  assert.equal(await visible('.editorpane'),true, 'choosing a file opens its source');
  await click(face('Document'));
  assert.equal(await b.evaluate('document.querySelector(".viewport iframe") === window.savedFrame'), true);
  assert.equal(await b.evaluate('window.savedFrame.contentWindow.scrollY'),800);

  await nav('Agent');
  await until('agent replies',()=>b.evaluate('document.querySelectorAll(".agent-panel .chat-message").length === 40'),10000);
  await b.evaluate('Array.from(document.querySelectorAll(".agent-tabs [role=tab]")).find(node=>node.textContent.trim()==="Chat").click()');
  await flush();
  await b.evaluate(`(() => {
    const input = document.querySelector('.agent-panel .chat-form textarea');
    input.value = 'An unsent thought'; input.dispatchEvent(new Event('input',{bubbles:true}));
    document.querySelector('.agent-panel .chat-transcript').scrollTop = 100;
  })()`);
  await flush();
  await click(face('Source'));
  await nav('Agent');
  assert.equal(await b.evaluate('document.querySelector(".agent-panel .chat-form textarea").value'), 'An unsent thought');
  assert.equal(await b.evaluate('document.querySelector(".agent-panel .chat-transcript").scrollTop'),100);
  await bounded();

  // The bar slides away while a pane scrolls down and returns on the way up,
  // at the top, and at desktop widths. The chat transcript is a pane that
  // really scrolls here; the open source file is one line long.
  const hidden = 'document.querySelector("body > nav").classList.contains("bar-hidden")';
  const scrollTo = async (top) => { await b.evaluate(`document.querySelector(".agent-panel .chat-transcript").scrollTop = ${top}`); await flush(); };
  await scrollTo(0); await scrollTo(200); await scrollTo(400);
  await until('bar hides on scroll down', () => b.evaluate(hidden), 5000);
  assert.equal(await b.evaluate('document.querySelector("main#main").classList.contains("bar-hidden")'), true);
  await scrollTo(300);
  await until('bar shows on scroll up', () => b.evaluate(`!${hidden}`), 5000);
  await scrollTo(400);
  await until('bar hides on scroll down again', () => b.evaluate(hidden), 5000);
  await b.resize(1280,900); await flush();
  assert.equal(await b.evaluate(hidden), false, 'the bar never hides at desktop widths');
  await b.resize(390,844); await flush();
  await scrollTo(0);
  await until('bar shows at the top', () => b.evaluate(`!${hidden}`), 5000);

  await b.resize(900,900); await flush();
  await click(face('Document'));
  assert.equal(await visible('.viewport'),true);
  assert.equal(await visible('.editorpane'),false);
  assert.equal(await visible('.sidebar'),true);
  await click(face('Source'));
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
    await click(face('Document')); assert.equal(await visible('.viewport'), true);
    await click(face('Source')); assert.equal(await visible('.editorpane'), true);
    await bounded();
  }
  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log('responsive-browser: panel minimum widths, rail overflow, collaboration tabs, comments, highlight discussions, custom colors, retry/discard, unread chat, drafts, viewport bounds, the hiding bar and saved layouts passed');
} finally { await b?.close(); serverHttp?.close(); rmSync(temp,{recursive:true,force:true}); }
