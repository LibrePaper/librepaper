// The desktop app's Settings shortcut: the shell dispatches librepaper:open-settings
// on window (Ctrl or Cmd+, or the Settings menu item), and the Reader opens its
// Settings page.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-open-settings-"));
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
  if (path.includes('/__lp/')) return Response.json({});
  if (path.endsWith('/me')) return Response.json({name:'Tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.qmd', '.md'], text_extensions: ['.qmd', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({title:'Run code',created_at:'test',role:'owner',source_format:'quarto',docs_origin:location.origin});
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
    const path = req.url.split("?")[0];
    if(path === "/docs/paper") {
      res.setHeader("content-type","text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    if(req.url.startsWith('/raw/')) { res.setHeader('content-type','text/html'); res.end('<body>Preview</body>'); return; }
    const file=join(out,path.slice(1));
    try { res.setHeader("content-type",contentType(file));res.end(readFileSync(file)); }
    catch { res.statusCode=404;res.end(); }
  });
  await new Promise((resolve,reject)=>{serverHttp.once("error",reject);serverHttp.listen(0,"127.0.0.1",resolve)});
  b=await browser("chromium",join(temp,"profile"),20000+Math.floor(Math.random()*1000));
  const origin = `http://127.0.0.1:${serverHttp.address().port}`;
  await b.resize(1280,900);

  // The page is ready once the Run code switch or the connection pill is drawn.
  await b.navigate(`${origin}/docs/paper`);
  await until('Reader is ready',()=>b.evaluate(`Boolean(document.querySelector('button[role=switch][aria-label="Run code"]') || document.querySelector('.connection-pill'))`),10000);

  // The shell's shortcut: Settings is not open yet, and the event opens it.
  // The Settings page stays mounted while closed, so only a visible one counts.
  const settingsOpen = () => b.evaluate(`Boolean(document.querySelector('.settings-nav')?.checkVisibility())`);
  assert.equal(await settingsOpen(), false, 'Settings starts closed');
  await b.evaluate('window.dispatchEvent(new CustomEvent("librepaper:open-settings"))');
  await until('Settings page is shown',settingsOpen,5000);

  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log("open-settings-browser: the desktop app's Settings shortcut opens Settings");
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
