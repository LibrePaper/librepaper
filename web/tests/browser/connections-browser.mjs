// Connection pills, their settings pages, and their compact navbar layout.
// Reader deliberately mounts without a document-state reply: these controls
// are independent of source hydration and collaboration's document content.
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

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-reader-connections-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
writeFileSync(room, `
export function openRoom(_slug, {onConnected}) {
  window.roomConnected = onConnected;
  queueMicrotask(() => onConnected(true));
  return { send() { return {ok:true}; }, sendLive() { return {ok:true}; }, close() {} };
}
`);
writeFileSync(entry, `
import ${JSON.stringify(join(root,"web/src/styles/app.css"))};
window.testErrors = [];
window.addEventListener('error', event => window.testErrors.push(event.message));
window.addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
globalThis.WebSocket = class {
  constructor(url) {
    if (!String(url).includes('/chat/')) throw new Error('Unexpected socket: ' + url);
    queueMicrotask(() => this.onopen?.());
  }
  send(raw) {
    if (JSON.parse(raw).type === 'join') queueMicrotask(() => this.onmessage?.({data:JSON.stringify({type:'ready',agent:true})}));
  }
  close() { this.onclose?.(); }
};
globalThis.fetch = async (url) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({title:'Connections',created_at:'test',role:'reader',source_format:'html',docs_origin:location.origin});
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'chat',token:'private'});
  if (path.includes('/comments')) return {ok:true,json:async()=>({comments:[]})};
  return {ok:false,json:async()=>({})};
};
const { default: Reader } = await import(${JSON.stringify(join(root,"web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root,"web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);
let serverHttp, b;
try {
  await build({ configFile:false, root:join(root,"web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
  serverHttp=createServer((req,res)=>{
    if(req.url === "/docs/paper") {
      res.setHeader("content-type","text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    if(req.url.startsWith('/raw/')) { res.setHeader('content-type','text/html'); res.end('<body>Preview</body>'); return; }
    const file=join(out,req.url.split("?")[0].slice(1));
    try { res.setHeader("content-type",contentType(file));res.end(readFileSync(file)); }
    catch { res.statusCode=404;res.end(); }
  });
  await new Promise((resolve,reject)=>{serverHttp.once("error",reject);serverHttp.listen(0,"127.0.0.1",resolve)});
  b=await browser("chromium",join(temp,"profile"),20000+Math.floor(Math.random()*1000));
  const url = `http://127.0.0.1:${serverHttp.address().port}/docs/paper`;
  await b.resize(1280,900);
  await b.navigate(url);
  await until('connection controls',()=>b.evaluate(`(() => {
    const group=document.querySelector('[aria-label="Connection settings"]');
    return group?.querySelectorAll('.connection-pill').length === 3
      && Boolean(group.querySelector('.local-pill'))
      && Boolean([...group.querySelectorAll('.connection-pill')].find(pill => pill.getAttribute('aria-label')?.includes('Remote server')))
      && Boolean([...group.querySelectorAll('.connection-pill')].find(pill => pill.getAttribute('aria-label')?.includes('open backup settings')));
  })()`),10000);
  await until('connected remote state',()=>b.evaluate('typeof window.roomConnected === "function" && document.querySelector(\'.connection-pill[aria-label*="Remote"] .connection-dot\')?.classList.contains("offline") === false'),10000);
  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const click = async (selector) => { await b.evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`); await flush(); };
  const clickPill = (name) => click(`.connection-pill[aria-label*="${name}"]`);
  const connectionState = (name) => b.evaluate(`(() => {
    const pill=document.querySelector(${JSON.stringify(`.connection-pill[aria-label*="${name}"]`)});
    return { visible:Boolean(pill?.getClientRects().length && getComputedStyle(pill).visibility !== 'hidden'),
      offline:pill?.querySelector('.connection-dot')?.classList.contains('offline') };
  })()`);

  const remote = await connectionState('Remote');
  assert.equal(remote.visible,true,'Remote pill is visible');
  assert.equal(remote.offline,false,'Remote starts connected');
  await b.evaluate('window.roomConnected(false)'); await flush();
  assert.equal((await connectionState('Remote')).offline,true,'Remote pill turns red while disconnected');
  await clickPill('Local');
  await until('Companion settings',()=>b.evaluate('document.querySelector(".settings-category")?.textContent.trim() === "Companion"'),3000);
  assert.equal(await b.evaluate('document.querySelector("#settings-companion .companion-status-label")?.textContent.trim() === "Companion" && Boolean(document.querySelector("#settings-companion .setting-status-pill")?.textContent.trim())'),true,
    'the settings sidebar shows the Companion status with its pill');
  assert.equal((await connectionState('Local')).offline,true,'Local pill is red while its app is unavailable');
  const localActions = await b.evaluate(`(() => {
    const root=document.querySelector('#companion-connection');
    const visible=element => Boolean(element?.getClientRects().length && getComputedStyle(element).visibility !== 'hidden');
    const install=root?.querySelector('a[href="https://librepaper.org/local-app.html"]');
    const connect=[...(root?.querySelectorAll('button') || [])].find(button => button.textContent.trim() === 'Connect');
    return { installVisible:visible(install), connectVisible:visible(connect), connectEnabled:Boolean(connect && !connect.disabled) };
  })()`);
  assert.equal(localActions.installVisible,true,'Companion settings shows a visible companion Install link');
  assert.equal(localActions.connectVisible,true,'Companion settings shows a visible Connect button');
  assert.equal(localActions.connectEnabled,true,'Companion settings Connect button is enabled');

  await click('[aria-label="Close"]');
  const backup = await connectionState('open backup settings');
  assert.equal(backup.visible,true,'Backup pill is visible');
  assert.equal(backup.offline,true,'Backup pill reports unavailable account backups');
  await clickPill('open backup settings');
  await until('Backup settings',()=>b.evaluate('document.querySelector(".backups-intro") && document.querySelector(".settings-category")?.textContent.trim() === "Backups"'),3000);
  assert.match(await b.evaluate('document.querySelector("#backup-connection .setting-description")?.innerText || ""'),/Sign in to load account backup settings/,
    'Backup settings explains that account backups require sign-in');
  await click('[aria-label="Close"]');
  await clickPill('Remote');
  await until('Account settings',()=>b.evaluate('document.querySelector(".settings-category")?.textContent.trim() === "Account"'),3000);
  assert.equal(await b.evaluate('document.querySelector("#remote-status [role=status]")?.getAttribute("aria-label")'),"Remote connection disconnected",
    'Remote settings exposes its disconnected state');
  await b.evaluate('window.roomConnected(true)'); await flush();
  assert.equal((await connectionState('Remote')).offline,false,'Remote pill turns green when connected');
  assert.equal(await b.evaluate('document.querySelector("#remote-status [role=status]")?.getAttribute("aria-label")'),"Remote connection connected",
    'Remote settings exposes its connected state');
  await click('[aria-label="Close"]');

  // Phones leave the connection lights out of the bar; Settings still has them.
  await b.resize(390,844); await flush();
  assert.equal(await b.evaluate('document.querySelectorAll(".connection-pill").length'),0,'no connection lights in a narrow window');
  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log('connections-browser: status pills, settings pages, local install links, and narrow-window bounds passed');
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
