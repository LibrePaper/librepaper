// The Reader's phone shell: header with its tabs, mode switch, More sheet, panels as full-screen
// views, command lists, browser Back and the keyboard (simulated). The room is the only module
// replaced, as in responsive-browser.mjs.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-phone-nav-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
writeFileSync(room, `
import { LoroDoc, LoroText, EphemeralStore } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths");
const id = "main"; const text = new LoroText(); text.insert(0, Array.from({length: 180}, (_, i) => "line " + i + " source content").join("\\n"));
// The project opens on the first file by name, so the long one sorts first.
files.setContainer(id, text); paths.set(id, "a-main.html");
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
// The agent panel shows its chat only inside the desktop app, so the
// harness defines the desktop global.
window.__LIBREPAPER_DESKTOP__ = { base: 'http://lp-local.localhost/', version: 'test' };
globalThis.fetch = async (url, init = {}) => {
  const path = String(url);
  if (path.startsWith('http://lp-local.localhost/')) {
    if (path.endsWith('/capabilities')) return Response.json({builders:[]});
    return Response.json({});
  }
  if (path.endsWith('/me')) return Response.json({name:'Very Long Tester Account Name',handle:'tester-handle',provider:'github',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({title:'A deliberately long responsive workspace title',created_at:'test',role:new URLSearchParams(location.search).get('role')||'editor',source_format:'html',docs_origin:location.origin,can_moderate:true,can_see_sharing:true});
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'chat', token:'private'});
  if (path.includes('/comments')) return { ok:true, json:async()=>({comments:[]}) };
  return { ok:false, json:async()=>({}) };
};
const { default: Reader } = await import(${JSON.stringify(join(root,"web/src/components/Reader.svelte"))});
const desktopClient = await import(${JSON.stringify(join(root,"web/src/lib/desktop/client.js"))});
desktopClient.configure({ project: 'paper', origin: location.origin });
const { mount } = await import(${JSON.stringify(join(root,"web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);
let serverHttp, b;
try {
  await build({ configFile:false, define: KATEX_DEFINE, root:join(root,"web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
  serverHttp=createServer((req,res)=>{
    if (serveKatex(req, res)) return;
    if(req.url.split("?")[0]==="/docs/paper") {
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
  const wait = (ms) => new Promise(resolve => setTimeout(resolve, ms));
  const click = async (selector) => { await b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node.click(); })()`); await flush(); };
  const visible = (selector) => b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); return Boolean(node?.getClientRects().length && getComputedStyle(node).visibility !== 'hidden'); })()`);
  const has = (selector) => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(selector)}))`);
  const key = async (key, code, virtual) => {
    await b.command('Input.dispatchKeyEvent', {type:'keyDown',key,code,windowsVirtualKeyCode:virtual});
    await b.command('Input.dispatchKeyEvent', {type:'keyUp',key,code,windowsVirtualKeyCode:virtual});
    await flush();
  };
  const backBtn = 'nav.phone-shell button.mobile-header-button[aria-label="Back"]';
  const modeOf = () => b.evaluate(`['mobile-document','mobile-source','mobile-sidebar'].find(name => document.querySelector('main.reader').classList.contains(name))`);
  const title = () => b.evaluate(`document.querySelector('nav.phone-shell #docTitle').textContent.trim()`);
  const openMore = async () => { await click('.mobile-more-trigger'); await until('More sheet', () => visible('.mobile-sheet'), 3000); };
  const sheetGone = () => until('More sheet closed', async () => !(await visible('.mobile-sheet')), 3000);
  const rows = () => b.evaluate(`[...document.querySelectorAll('.mobile-sheet [data-row]')].map(node => node.dataset.row)`);
  // The workspace animates its height for 200ms when the chrome around it changes.
  const resize = async (width, height = 844) => { await b.resize(width, height); await flush(); await wait(260); };
  const toMode = async (mode) => {
    if (await visible(backBtn)) await click(backBtn);
    await click(`.mobile-mode-switch [data-mode="${mode}"]`);
    await until(`main is mobile-${mode}`, async () => (await modeOf()) === `mobile-${mode}`, 3000);
    await wait(260);
  };
  const box = (selector) => b.evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)})?.getBoundingClientRect(); return r && {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height}; })()`);
  const reload = async (suffix = '') => { await b.resize(1280, 900); await b.navigate(url + suffix); await until('reader mounted', () => b.evaluate(suffix ? "Boolean(document.querySelector('main.reader .sidebar-activity'))" : "Boolean(document.querySelector('.cm-editor')) && document.querySelectorAll('.explorer-row').length > 60"), 10000); await flush(); };

  await resize(390);
  await until('phone shell', () => has('.mobile-header-tabs'), 10000);
  await b.evaluate('window.keepEditor = document.querySelector(".cm-editor")');

  // a. Every phone width: nothing spills, and the controls are touch targets.
  for (const width of [320, 375, 390, 430, 767]) {
    await resize(width);
    await toMode('document');
    const m = await b.evaluate(`(() => {
      const rect = node => { const r = node.getBoundingClientRect(); return [r.width, r.height]; };
      const main = document.querySelector('main.reader').getBoundingClientRect();
      const tabs = document.querySelector('.mobile-header-tabs');
      return { scrollWidth: document.documentElement.scrollWidth, innerWidth, mainLeft: main.left, mainWidth: main.width, mainBottom: main.bottom, innerHeight,
        tabButtons: [...tabs.querySelectorAll('button')].map(node => { const r = node.getBoundingClientRect(); return [r.width, r.height, r.left, r.right, r.top, r.bottom]; }),
        more: [...document.querySelectorAll('.mobile-more-trigger')].map(rect), moreInHeader: Boolean(document.querySelector('nav.phone-shell .mobile-more-trigger')),
        header: document.querySelector('nav.phone-shell').getBoundingClientRect().height,
        headerButtons: [...document.querySelectorAll('nav.phone-shell .mobile-header-button')].map(rect),
        mode: Boolean(document.querySelector('.mobile-mode-switch')?.getClientRects().length) };
    })()`);
    const info = `${width}px: ${JSON.stringify(m)}`;
    assert.ok(m.scrollWidth <= m.innerWidth + 1, 'no horizontal overflow at ' + info);
    assert.ok(Math.abs(m.mainLeft) <= 1 && Math.abs(m.mainWidth - m.innerWidth) <= 1, 'main spans the width at ' + info);
    assert.equal(m.tabButtons.length, 3, 'the header has three tabs at ' + info);
    assert.ok(m.tabButtons.every(([w, h]) => h >= 44 && w > 0), 'header tabs are at least 44px tall at ' + info);
    assert.ok(m.tabButtons.every(([w, h, left, right, top, bottom]) => left >= 0 && right <= m.innerWidth && top >= 0 && bottom <= m.innerHeight), 'header tabs are inside the viewport at ' + info);
    assert.equal(m.more.length, 1, 'exactly one More trigger at ' + info);
    assert.ok(m.more[0][0] >= 44 && m.more[0][0] <= 45 && m.more[0][1] >= 44 && m.more[0][1] <= 45, 'the More trigger is 44x44 at ' + info);
    assert.equal(m.moreInHeader, true, 'More is in the header at ' + info);
    assert.ok(m.header <= 52, 'the header is at most 52px at ' + info);
    assert.equal(await b.evaluate(`document.querySelectorAll('.mobile-more-trigger').length`), 1, 'exactly one More trigger at ' + info);
    assert.equal(await has('#docTitle'), false, 'the document screen has no file name in the header at ' + info);
    assert.ok(m.headerButtons.length > 0 && m.headerButtons.every(([w, h]) => w >= 44 && h >= 44), 'header buttons are 44px at ' + info);
    assert.equal(m.mode, true, 'the mode switch shows at ' + info);
    // The longest labels the catalogues give these tabs must fit the narrowest phone.
    const fit = await b.evaluate(`(() => { const nodes = [...document.querySelectorAll('.mobile-header-tabs .label')]; const was = nodes.map(node => node.textContent);
      ['Documento', 'Arquivos', 'Commentaires'].forEach((text, i) => { nodes[i].textContent = text; });
      const clipped = nodes.filter(node => node.scrollWidth > node.clientWidth).map(node => node.textContent);
      nodes.forEach((node, i) => { node.textContent = was[i]; }); return clipped; })()`);
    assert.deepEqual(fit, [], 'no tab label is cut off at ' + info);
    assert.ok(Math.abs(m.mainBottom - m.innerHeight) <= 1, 'the workspace ends at the viewport bottom at ' + info);
  }

  // b. The phone shell is gone at the breakpoint.
  await resize(768);
  await until('desktop shell at 768', async () => !(await has('.mobile-header-tabs')), 3000);
  assert.equal(await has('.mobile-mode-switch'), false, 'no mode switch at 768');
  assert.equal(await visible('.sidebar-activity'), true, 'the rail shows at 768');
  assert.equal(await visible('.menubar'), true, 'the menubar shows at 768');

  // c. The More sheet.
  await resize(390);
  await toMode('document');
  await openMore();
  assert.equal(await b.evaluate('Boolean(document.activeElement?.closest(".mobile-sheet"))'), true, 'opening the sheet focuses inside it');
  assert.equal(await b.evaluate('document.querySelector(".mobile-sheet").getAttribute("role")'), 'dialog');
  assert.equal(await b.evaluate('document.querySelectorAll(".mobile-sheet").length'), 1, 'one sheet at a time');
  await until('sheet settled', () => b.evaluate('document.querySelector(".mobile-sheet").getBoundingClientRect().bottom <= innerHeight + 1'), 3000);
  assert.equal(await b.evaluate('document.querySelector(".mobile-sheet").getBoundingClientRect().height <= innerHeight * 0.85 + 1'), true, 'the sheet is at most 85% of the screen');
  assert.equal(await b.evaluate('[...document.querySelectorAll(".mobile-sheet-row")].every(node => node.getBoundingClientRect().height >= 44)'), true, 'sheet rows are 44px');
  await key('Escape', 'Escape', 27);
  await sheetGone();
  assert.equal(await b.evaluate('document.activeElement?.matches(".mobile-more-trigger")'), true, 'Escape returns focus to More');
  await openMore();
  // A real press above the sheet: the dialog listens for it on the document.
  for (const type of ['mousePressed', 'mouseReleased']) await b.command('Input.dispatchMouseEvent', {type, x: 195, y: 40, button: 'left', clickCount: 1});
  await sheetGone();
  await openMore();
  // A finger swipe down on the handle, which only a touch event can express.
  await b.evaluate(`(() => {
    const handle = document.querySelector('.mobile-sheet .sheet-handle');
    const touch = y => new Touch({ identifier: 1, target: handle, clientX: 100, clientY: y });
    const fire = (type, y) => handle.dispatchEvent(new TouchEvent(type, { bubbles: true, cancelable: true,
      touches: type === 'touchend' ? [] : [touch(y)], changedTouches: [touch(y)] }));
    fire('touchstart', 500); fire('touchmove', 560); fire('touchmove', 620); fire('touchend', 620);
  })()`);
  await sheetGone();

  // d. Every row an editor has opens its panel full-screen and Back undoes it.
  await resize(320);
  await toMode('document');
  const panelRows = ['outline', 'history', 'changes', 'agent', 'git', 'diagnostics', 'share'];
  const visitFiles = async (mode) => {
    await toMode(mode);
    await openMore();
    assert.equal((await rows()).includes('files'), false, 'Files is a tab, not a sheet row');
    await key('Escape', 'Escape', 27); await sheetGone();
    await click('.mobile-header-tabs [data-destination="files"]');
    await until('files is open', async () => (await modeOf()) === 'mobile-sidebar', 3000);
    assert.equal(await b.evaluate('document.querySelector(\'.mobile-header-tabs [data-destination="files"]\').getAttribute("aria-current")'), 'page', 'the Files tab is current');
    assert.equal(await visible('.filelist'), true, 'the file list shows');
    assert.equal(await has('#docTitle'), false, 'the tabs replace the title over Files');
    await click(backBtn);
    await until('back from files', async () => (await modeOf()) === `mobile-${mode}`, 3000);
  };
  const visit = async (id, mode) => {
    await toMode(mode);
    await openMore();
    const present = await rows();
    assert.ok(present.includes(id), `an editor is offered ${id}: ${present.join(',')}`);
    const label = await b.evaluate(`document.querySelector('.mobile-sheet [data-row="${id}"] .row-label').textContent.trim()`);
    await click(`.mobile-sheet [data-row="${id}"]`);
    await sheetGone();
    await until(`${id} is open`, async () => (await modeOf()) === 'mobile-sidebar', 3000);
    assert.equal(await title(), label, `the header names ${id}`);
    assert.equal(await b.evaluate('document.querySelector(".cm-editor") === window.keepEditor'), true, `${id} keeps the very same editor`);
    const slots = await b.evaluate(`(() => [...document.querySelectorAll('.panel-slot:not([hidden])')].filter(node => node.getClientRects().length).map(node => ({ width: node.getBoundingClientRect().width, scroll: node.scrollWidth - node.clientWidth })))()`);
    assert.equal(slots.length, 1, `${id}: one panel at a time: ${JSON.stringify(slots)}`);
    assert.ok(slots[0].width >= 319 && slots[0].scroll <= 1, `${id} fills 320px without sideways scroll: ${JSON.stringify(slots)}`);
    await click(backBtn);
    await until(`back from ${id}`, async () => (await modeOf()) === `mobile-${mode}`, 3000);
    assert.equal(await visible('.mobile-sheet'), false);
  };
  await b.evaluate('window.keepEditor = document.querySelector(".cm-editor")');
  for (const id of panelRows) await visit(id, 'document');
  await visitFiles('document');
  await visit('outline', 'source');
  await visitFiles('source');
  // Collaborators is a row of the Document group, and opens the collaboration panel.
  await toMode('document');
  await openMore();
  assert.ok((await rows()).includes('collaborators'), 'an editor is offered collaborators');
  await click('.mobile-sheet [data-row="collaborators"]');
  await sheetGone();
  await until('collaborators open', () => visible('.collaboration'), 3000);
  await click(backBtn);
  await until('back from collaborators', async () => (await modeOf()) === 'mobile-document', 3000);

  // e. The header tabs.
  await toMode('document');
  const tab = (name) => `.mobile-header-tabs [data-destination="${name}"]`;
  const current = (name) => b.evaluate(`document.querySelector(${JSON.stringify(tab(name))}).getAttribute("aria-current")`);
  await click(tab('comments'));
  await until('comments open', () => visible('.collaboration'), 3000);
  assert.equal(await current('comments'), 'page');
  assert.equal(await current('document'), null);
  await click(tab('files'));
  await until('files open', () => visible('.filelist'), 3000);
  assert.equal(await current('files'), 'page');
  assert.equal(await current('comments'), null);
  await click(tab('document'));
  await until('document again', async () => (await modeOf()) === 'mobile-document', 3000);
  assert.equal(await current('document'), 'page');
  assert.equal(await visible('.collaboration'), false);
  await openMore();
  assert.equal(await b.evaluate('document.querySelector(".mobile-more-trigger").getAttribute("aria-expanded")'), 'true', 'More is expanded while its sheet is open');
  await key('Escape', 'Escape', 27); await sheetGone();

  // f. The browser's Back button. History entries settle for 50ms before they are pushed.
  await reload();
  await resize(390);
  await click('.mobile-header-tabs [data-destination="files"]');
  await until('files open', async () => (await modeOf()) === 'mobile-sidebar', 3000);
  await wait(150);
  await b.evaluate('history.back()');
  await until('Back leaves Files', async () => (await modeOf()) === 'mobile-document', 3000);
  await click('.mobile-header-tabs [data-destination="comments"]');
  await wait(150);
  await click('.mobile-more-trigger');
  await until('sheet over Comments', () => visible('.mobile-sheet'), 3000);
  await wait(150);
  await b.evaluate('history.back()');
  await sheetGone();
  assert.equal(await modeOf(), 'mobile-sidebar', 'Back closes the sheet and Comments stays');
  assert.equal(await visible('.collaboration'), true);
  await b.evaluate('history.back()');
  await until('Back leaves Comments', async () => (await modeOf()) === 'mobile-document', 3000);

  // g. The mode switch keeps the source where it was.
  await toMode('source');
  await b.evaluate('document.querySelector(".cm-scroller").scrollTop = 600');
  await flush();
  const top = await b.evaluate('document.querySelector(".cm-scroller").scrollTop');
  assert.ok(top > 0);
  await click('.mobile-mode-switch [data-mode="document"]');
  assert.equal(await b.evaluate('document.querySelectorAll(".mobile-mode-switch [aria-pressed=true]").length'), 1);
  await click('.mobile-mode-switch [data-mode="source"]');
  assert.equal(await b.evaluate('document.querySelectorAll(".mobile-mode-switch [aria-pressed=true]").length'), 1);
  assert.ok(Math.abs(await b.evaluate('document.querySelector(".cm-scroller").scrollTop') - top) < 2, 'the source keeps its scroll');

  // h. File, Edit, View and Insert are lists, not menus hanging from a trigger.
  for (const id of ['file', 'edit', 'view', 'insert']) {
    await toMode('source');
    await openMore();
    await click(`.mobile-sheet [data-row="command:${id}"]`);
    await until(`${id} list`, () => visible('.mobile-command-view'), 3000);
    assert.equal(await has('.explorer-menu[data-state=open]'), false, `${id} hangs no menu`);
    assert.equal(await visible('.menubar'), false, 'the menubar stays hidden');
    await click(backBtn);
    await until(`${id} list closed`, async () => !(await visible('.mobile-command-view')), 3000);
  }

  // i. The keyboard. This is a simulation: a real one is not available here.
  await toMode('source');
  const keyboard = 300;
  await b.evaluate(`(() => {
    Object.defineProperty(window.visualViewport, 'height', { configurable: true, get: () => innerHeight - ${keyboard} });
    document.querySelector('.cm-content').focus();
    window.visualViewport.dispatchEvent(new Event('resize'));
  })()`);
  await until('mode switch steps aside', async () => !(await visible('.mobile-mode-switch')), 3000);
  assert.equal(await has('.mobile-header-tabs'), true, 'the header tabs stay while the keyboard is open');
  await until('keyboard bar', () => visible('.mobile-keyboard-bar'), 3000);
  await until('the workspace ends above the keyboard bar', () => b.evaluate(`document.querySelector('main.reader').getBoundingClientRect().bottom <= innerHeight - ${keyboard} - 44 + 1`), 3000);
  await b.evaluate(`(() => { delete window.visualViewport.height; document.activeElement.blur(); window.visualViewport.dispatchEvent(new Event('resize')); })()`);
  await until('mode switch returns', () => visible('.mobile-mode-switch'), 3000);

  // j. Widening drops every phone layer.
  const layers = {
    panel: async () => { await toMode('document'); await click('.mobile-header-tabs [data-destination="files"]'); await until('files', () => visible('.filelist'), 3000); },
    sheet: async () => { await toMode('document'); await openMore(); },
    command: async () => { await toMode('document'); await openMore(); await click('.mobile-sheet [data-row="command:file"]'); await until('list', () => visible('.mobile-command-view'), 3000); },
  };
  for (const [name, open] of Object.entries(layers)) {
    await resize(390);
    await open();
    await resize(1024);
    await until(`${name}: desktop`, async () => !(await has('.mobile-header-tabs')) && !(await visible('.mobile-sheet')) && !(await has('.mobile-command-view')), 3000);
    assert.equal(await visible('.sidebar-activity'), true, `${name}: the rail is back`);
    await resize(390);
    await until(`${name}: phone again`, () => has('nav.phone-shell'), 3000);
    assert.equal(await visible('.mobile-sheet'), false);
  }

  // k. The header tabs are painted in both themes.
  await resize(390);
  await toMode('document');
  const paint = async (mode) => {
    await b.evaluate(`document.documentElement.dataset.mode = ${JSON.stringify(mode)}`);
    await flush();
    return b.evaluate(`(() => { const nav = getComputedStyle(document.querySelector('nav.phone-shell')); const button = getComputedStyle(document.querySelector('nav.phone-shell .mobile-header-tabs [aria-current="page"]'));
      return { background: nav.backgroundColor, color: button.color }; })()`);
  };
  const dark = await paint('dark');
  const light = await paint('light');
  assert.notEqual(dark.background, light.background, 'the header background follows the theme');
  assert.notEqual(dark.color, light.color, 'the active tab colour follows the theme');
  for (const paintOf of [dark, light]) assert.notEqual(paintOf.background, 'rgba(0, 0, 0, 0)', 'the header is opaque');

  // l. A reader sees less.
  await reload('?role=reader');
  await resize(390);
  await until('reader shell', () => has('.mobile-header-tabs'), 10000);
  assert.equal(await has('.mobile-header-tabs [data-destination="comments"]'), false, 'a reader has no Comments');
  assert.equal(await has('.mobile-header-tabs [data-destination="files"]'), true, 'a reader has Files');
  await openMore();
  const readerRows = await rows();
  assert.equal(readerRows.includes('git'), false, 'a reader has no Git');

  assert.deepEqual(await b.evaluate('window.testErrors'), []);
  console.log('phone-nav-browser: widths, the breakpoint, the More sheet, panels, the header tabs, browser Back, the mode switch, commands, the simulated keyboard, resizing, themes and the reader role passed');
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
