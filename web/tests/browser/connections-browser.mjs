// The Reader's presence group and its offline state, plus its settings pages and narrow-window bounds.
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
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

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
  await build({ configFile:false, define: KATEX_DEFINE, root:join(root,"web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
  serverHttp=createServer((req,res)=>{
    if (serveKatex(req, res)) return;
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
  await until('Reader is ready',()=>b.evaluate(`typeof window.roomConnected === "function" && Boolean(document.querySelector('nav .presence'))`),10000);
  assert.equal(await b.evaluate('Boolean(document.querySelector(".connection-pill") || document.querySelector(\'[aria-label="Connection settings"]\'))'),false,'no connection pills in the top bar');
  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const click = async (selector) => { await b.evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`); await flush(); };
  const offlineLabel = () => b.evaluate(`document.querySelector('nav .presence .presence-offline')?.textContent.trim() ?? null`);
  const openSettingsAt = async (name) => {
    await b.evaluate('window.dispatchEvent(new CustomEvent("librepaper:open-settings"))'); await flush();
    await b.evaluate(`[...document.querySelectorAll(".settings-nav-item")].find(item => item.textContent.trim() === ${JSON.stringify(name)}).click()`); await flush();
  };
  await until('connected presence',()=>b.evaluate('document.querySelector("nav .presence .presence-offline") === null'),10000);

  await b.evaluate('window.roomConnected(false)'); await flush();
  assert.equal(await offlineLabel(),'Offline','the presence group says Offline while disconnected');

  await openSettingsAt('Account');
  await until('Account settings',()=>b.evaluate('document.querySelector(".settings-category")?.textContent.trim() === "Account"'),3000);
  assert.equal(await b.evaluate('Boolean(document.querySelector(".local-pill"))'),false,'no desktop app status pill in the bar');
  assert.equal(await b.evaluate('document.querySelector("#remote-status [role=status]")?.getAttribute("aria-label")'),"Remote connection disconnected",
    'Remote settings exposes its disconnected state');
  await click('[aria-label="Close"]');

  await openSettingsAt('Desktop app');
  await until('Desktop app settings',()=>b.evaluate('document.querySelector(".settings-category")?.textContent.trim() === "Desktop app"'),3000);
  const desktopActions = await b.evaluate(`(() => {
    const root=document.querySelector('.desktop-only');
    const visible=element => Boolean(element?.getClientRects().length && getComputedStyle(element).visibility !== 'hidden');
    const install=root?.querySelector('a[href="https://librepaper.org/desktop-app.html"]');
    const open=root?.querySelector('a[href^="librepaper://open?url="]');
    return { message:root?.querySelector('.desktop-only-message')?.textContent.trim(), install:install?.textContent.trim(), open:open?.textContent.trim(), installVisible:visible(install), openVisible:visible(open) };
  })()`);
  assert.equal(desktopActions.message,'This needs the LibrePaper desktop app.','a browser is told the page needs the desktop app');
  assert.equal(desktopActions.install,'Get the desktop app');
  assert.equal(desktopActions.open,'Open in the desktop app');
  assert.equal(desktopActions.installVisible,true,'Desktop app settings shows a visible install link');
  assert.equal(desktopActions.openVisible,true,'Desktop app settings shows a visible open link');
  await click('[aria-label="Close"]');

  await openSettingsAt('Backups');
  await until('Backup settings',()=>b.evaluate('document.querySelector(".backups-intro") && document.querySelector(".settings-category")?.textContent.trim() === "Backups"'),3000);
  assert.equal(await b.evaluate('document.querySelector(".desktop-only .desktop-only-message")?.textContent.trim() || ""'),'This needs the LibrePaper desktop app.',
    'Backup settings in a browser say they need the desktop app');
  await click('[aria-label="Close"]');

  await b.evaluate('window.roomConnected(true)'); await flush();
  assert.equal(await offlineLabel(),null,'the Offline label goes away when reconnected');
  await openSettingsAt('Account');
  await until('Account settings again',()=>b.evaluate('document.querySelector(".settings-category")?.textContent.trim() === "Account"'),3000);
  assert.equal(await b.evaluate('document.querySelector("#remote-status [role=status]")?.getAttribute("aria-label")'),"Remote connection connected",
    'Remote settings exposes its connected state');
  await click('[aria-label="Close"]');

  // Compact widths draw no presence group, so nothing connection-related is in the bar.
  await b.resize(390,844); await flush();
  assert.equal(await b.evaluate('document.querySelectorAll(".connection-pill").length'),0,'no connection pills in a narrow window');
  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log('connections-browser: presence offline state, settings pages, desktop app links, and narrow-window bounds passed');
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
