// The Run code switch: off whenever a document opens, off in a plain browser
// (which is told the page needs the desktop app), on in the desktop app, and
// never remembered across a reload or a stored build preference.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-reader-run-code-"));
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
      const desktop = new URL(req.url, "http://localhost").searchParams.get("desktop") === "1";
      const desktopScript = desktop
        ? '<script>window.__LIBREPAPER_DESKTOP__ = Object.freeze({ base: location.origin + "/__lp/", version: "test" });</script>'
        : "";
      res.setHeader("content-type","text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body>' + desktopScript + '<script type="module" src="/check.js"></script></body></html>');
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
  const plainUrl = `${origin}/docs/paper`;
  const desktopUrl = `${origin}/docs/paper?desktop=1`;
  await b.resize(1280,900);

  const runCodeState = () => b.evaluate(`document.querySelector('button[role=switch][aria-label="Run code"]')?.getAttribute("aria-checked") ?? null`);
  const runCodeSwitchExists = () => b.evaluate(`Boolean(document.querySelector('button[role=switch][aria-label="Run code"]'))`);
  const clickRunCode = () => b.evaluate("document.querySelector('button[role=switch][aria-label=\"Run code\"]').click()");
  const settle = () => new Promise((resolve) => setTimeout(resolve, 300));

  // Case 1: a plain browser. The switch starts off, and turning it on is refused.
  await b.navigate(plainUrl);
  await until('Run code switch in a browser',runCodeSwitchExists,10000);
  assert.equal(await runCodeState(),"false",'Run code starts off in a browser');
  await clickRunCode();
  await until('browser is told the page needs the desktop app',()=>b.evaluate('document.body.textContent.includes("needs the LibrePaper desktop app")'),5000);
  await settle();
  assert.equal(await runCodeState(),"false",'Run code stays off in a browser');

  // Case 2: the desktop app. The switch turns on, and a reload turns it off again.
  await b.navigate(desktopUrl);
  await until('Run code switch in the desktop app',runCodeSwitchExists,10000);
  assert.equal(await runCodeState(),"false",'Run code starts off in the desktop app');
  await clickRunCode();
  await until('Run code on in the desktop app',()=>b.evaluate('document.querySelector(\'button[role=switch][aria-label="Run code"]\')?.getAttribute("aria-checked") === "true"'),5000);
  await b.navigate(desktopUrl);
  await until('Run code switch after reload',runCodeSwitchExists,10000);
  assert.equal(await runCodeState(),"false",'Run code is never remembered across a reload');

  // Case 3: a stored build preference that points Quarto at the local tool
  // cannot start it. The page is still on this origin, so its storage is ours.
  await b.evaluate(`(() => {
    const anonymous = localStorage.getItem("librepaper-build-v3:anonymous-id");
    if (!anonymous) throw new Error("no anonymous build identity was stored");
    const scope = JSON.stringify([location.origin, anonymous, "quarto"]);
    const options = JSON.stringify([location.origin, anonymous, "quarto-options"]);
    localStorage.setItem("librepaper-build-v3:" + scope, JSON.stringify({ selection: "tool", backend: "local", tool: "quarto", format: "quarto" }));
    localStorage.setItem("librepaper-build-v3:" + options, JSON.stringify({ profile: null, parameters: {} }));
    return true;
  })()`);
  await b.navigate(desktopUrl);
  await until('Run code switch with a stored local preference',runCodeSwitchExists,10000);
  assert.equal(await runCodeState(),"false",'a stored preference cannot turn Run code on');

  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log("run-code-browser: off on open, browser stays off, desktop turns on, never remembered");
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
