// The Run button of a Quarto document: not offered in a plain browser,
// offered in the desktop app, and nothing runs until it is pressed. Pressing
// it without a Run directory asks for one instead of running.
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
const account = {name:'Tester',providers:[]};
const params = new URLSearchParams(location.search);
const format = params.get('format') === 'typst' ? 'typst' : 'quarto';
globalThis.fetch = async (url) => {
  const path = String(url);
  if (path.includes('/__lp/')) { (window.lpCalls ||= []).push(path); return Response.json({}); }
  if (path.endsWith('/me')) return Response.json(account);
  if (path.endsWith('/config')) return Response.json({ extensions: ['.qmd', '.md', '.typ'], text_extensions: ['.qmd', '.md', '.typ'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({title:'Run code',created_at:'test',role:'owner',source_format:format,docs_origin:location.origin});
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'chat',token:'private'});
  if (path.includes('/comments')) return {ok:true,json:async()=>({comments:[]})};
  return {ok:false,json:async()=>({})};
};
const { update, buildPreferenceUser } = await import(${JSON.stringify(join(root,"web/src/lib/build-preferences.js"))});
// This person's stored Typst preference: this computer renders Typst (Run code is offered).
if (params.get('local') === '1') update({origin: location.origin, user: buildPreferenceUser(account)}, 'typst', {backend:'local'});
const { default: Reader } = await import(${JSON.stringify(join(root,"web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root,"web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);
let serverHttp, b;
try {
  await build({ configFile:false, define: KATEX_DEFINE, root:join(root,"web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
  serverHttp=createServer((req,res)=>{
    if (serveKatex(req, res)) return;
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

  const runButton = `[role=group][aria-label="Run"] button[data-tip]`;
  const runButtonExists = () => b.evaluate(`Boolean(document.querySelector('${runButton}'))`);
  const switchExists = () => b.evaluate(`Boolean(document.querySelector('button[role=switch][aria-label="Run code"]'))`);
  const renderCalls = () => b.evaluate(`(window.lpCalls || []).filter((path) => path.includes("/jobs")).length`);
  const settle = () => new Promise((resolve) => setTimeout(resolve, 300));

  // Case 1: a plain browser has no computer to run code on, so Run is not offered.
  await b.navigate(plainUrl);
  await until('the page joined its room',()=>b.evaluate('typeof window.roomConnected === "function"'),10000);
  await settle();
  assert.equal(await runButtonExists(),false,'Run is not offered in a browser');

  // Case 2: the desktop app offers Run in place of the switch, and opening the
  // document runs nothing.
  await b.navigate(desktopUrl);
  await until('Run button in the desktop app',runButtonExists,10000);
  assert.equal(await switchExists(),false,'a Quarto document has no Run code switch');
  await settle();
  assert.equal(await renderCalls(),0,'opening a Quarto document runs nothing on this computer');

  // Case 3: Run without a Run directory asks for one and runs nothing.
  await b.evaluate(`document.querySelector('${runButton}').click()`);
  await until('Run directory dialog',()=>b.evaluate(`[...document.querySelectorAll('[role=dialog]')].some((d) => d.textContent.includes("Choose a Run directory"))`),5000);
  assert.equal(await renderCalls(),0,'Run without a folder runs nothing');

  // Case 4: a Typst document left to the browser is not offered Run, so the
  // desktop app shows neither the Run button nor the switch, and nothing runs.
  // This runs before case 5 because the stored preference persists in this
  // browser profile.
  await b.navigate(`${origin}/docs/paper?desktop=1&format=typst`);
  await until('the Typst page joined its room',()=>b.evaluate('typeof window.roomConnected === "function"'),10000);
  await settle();
  assert.equal(await runButtonExists(),false,'a Typst document rendered in the browser is not offered Run');
  assert.equal(await switchExists(),false,'a Typst document rendered in the browser has no Run code switch');
  assert.equal(await renderCalls(),0,'opening a Typst document rendered in the browser runs nothing');

  // Case 5: this computer renders Typst (stored preference backend "local"),
  // so the Run button replaces the switch, and opening the document runs nothing.
  await b.navigate(`${origin}/docs/paper?desktop=1&format=typst&local=1`);
  await until('Run button for a Typst document on this computer',runButtonExists,10000);
  assert.equal(await switchExists(),false,'a Typst document has no Run code switch');
  await settle();
  assert.equal(await renderCalls(),0,'opening a Typst document runs nothing on this computer');

  // Case 6: Run without a Run directory asks for one and runs nothing, as for Quarto.
  await b.evaluate(`document.querySelector('${runButton}').click()`);
  await until('Run directory dialog for Typst',()=>b.evaluate(`[...document.querySelectorAll('[role=dialog]')].some((d) => d.textContent.includes("Choose a Run directory"))`),5000);
  assert.equal(await renderCalls(),0,'Run on a Typst document without a folder runs nothing');

  assert.deepEqual(await b.evaluate('window.testErrors'),[]);
  console.log("run-code-browser: not offered in a browser, offered in the desktop app, nothing runs until Run, Run asks for a folder; Typst is offered Run only when this computer renders it");
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
