// Exercise Preview.svelte itself across the app and documents origins. The
// docs page loads an ordinary third origin script to model document content;
// browser same-origin rules, the component receiver, and the iframe sandbox
// remain real throughout this harness.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { extname } from "node:path";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-frame-isolation-"));
const entry = join(temporary, "entry.js");
const viewerBuild = join(temporary, "viewer-build");
writeFileSync(entry, `
import { mount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Preview from ${JSON.stringify(join(root, "src/components/Preview.svelte"))};
window.messages = [];
window.preview = mount(Preview, { target: document.querySelector('#mount'), props: {
  src: 'http://localhost:' + new URL(location.href).port + '/docs',
  docsOrigin: 'http://localhost:' + new URL(location.href).port,
  onmessage: message => window.messages.push(message),
} });
window.previewFrameLoads = 0;
const previewFrame = document.querySelector('iframe[title=Document]');
previewFrame.addEventListener('load', () => window.previewFrameLoads++);
window.rawFrameEvents = [];
window.addEventListener('message', event => {
  if (event.data?.text === 'external-origin-doc') window.rawFrameEvents.push({
    origin: event.origin,
    fromPreviewFrame: event.source === previewFrame.contentWindow,
  });
});
`);

const types = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8" };
let appServer, scriptServer, page;
try {
  await build({ configFile: false, root, plugins: [svelte()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "preview.js" } } });
  await build({ configFile: false, root, logLevel: "error",
    build: { outDir: viewerBuild, rollupOptions: { input: { viewer: join(root, "pages/viewer.html") } } } });
  scriptServer = createServer((request, response) => {
    const scriptUrl = new URL(request.url, "http://127.0.0.1");
    if (scriptUrl.pathname === "/external") {
      response.setHeader("content-type", types[".html"]);
      response.end('<!doctype html><script>parent.postMessage({librepaper:true,type:"ready",text:"external-origin-doc"},"*")</script>');
      return;
    }
    if (scriptUrl.pathname !== "/attack.js") {
      response.writeHead(404).end();
      return;
    }
    const appOrigin = scriptUrl.searchParams.get("app") || "";
    response.setHeader("content-type", types[".js"]);
    response.end(`
      window.thirdScriptRan = true;
      window.addEventListener('message', event => {
        if (event.data?.isolateNavigate) location.href = ${JSON.stringify(`${scriptOrigin}/external`)};
      });
      if (sessionStorage.getItem('frameIsolationReloads') === '1') {
        try { window.parent.document.body.dataset.thirdScriptReadParent = 'yes'; }
        catch { window.thirdScriptParentDenied = true; }
        try { parent.localStorage.getItem('app-secret'); window.thirdScriptReadAppStorage = true; }
        catch { window.thirdScriptStorageDenied = true; }
        window.thirdScriptCookie = document.cookie;
        window.thirdScriptMessages = [];
        const send = data => parent.postMessage(data, '*');
        send({librepaper:true,type:'ready',text:'Document text'});
        send({librepaper:true,type:'selection',selector:{exact:'selected text',prefix:'',suffix:''}});
        send({librepaper:true,type:'admin-command',action:'read-session'});
        send({librepaper:true,type:'selection',selector:{exact:'x'.repeat(1000001),prefix:'',suffix:''}});
        send(null);
        window.thirdScriptMessages.push('sent');
        try { parent.location.href = ${JSON.stringify(`${appOrigin}/navigated`)}; }
        catch { window.thirdScriptNavigationDenied = true; }
      }
    `);
  });
  await new Promise((resolve, reject) => { scriptServer.once("error", reject); scriptServer.listen(0, resolve); });
  const scriptOrigin = `http://127.0.0.1:${scriptServer.address().port}`;

  appServer = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/") {
      response.setHeader("set-cookie", [
        "app_session=secret-session; HttpOnly; SameSite=Lax; Path=/",
        "app_canary=host-only; SameSite=Lax; Path=/",
      ]);
      response.setHeader("content-type", types[".html"]);
      response.end('<!doctype html><meta charset="utf-8"><body><div id="mount"></div><script>localStorage.setItem("app-secret","private");</script><script type="module" src="/preview.js"></script></body>');
      return;
    }
    if (url.pathname === "/docs") {
      response.setHeader("content-type", types[".html"]);
      response.end(`<!doctype html><meta charset="utf-8"><script>
        window.reloadCount=(Number(sessionStorage.getItem('frameIsolationReloads'))||0)+1;
        sessionStorage.setItem('frameIsolationReloads',String(window.reloadCount));
        localStorage.setItem('docs-secret','private-to-docs');
      </script><script src="${scriptOrigin}/attack.js?app=${encodeURIComponent(`http://127.0.0.1:${appServer.address().port}`)}"></script>`);
      return;
    }
    if (url.pathname === "/sibling") {
      response.setHeader("content-type", types[".html"]);
      response.end('<!doctype html><script>parent.postMessage({librepaper:true,type:"ready",text:"sibling"},"*")</script>');
      return;
    }
    if (url.pathname === "/wrong-origin") {
      response.setHeader("content-type", types[".html"]);
      response.end('<!doctype html><script>parent.postMessage({librepaper:true,type:"ready",text:"wrong-origin"},"*")</script>');
      return;
    }
    if (url.pathname === "/viewer-host") {
      const reader = url.searchParams.get("reader") || "";
      response.setHeader("content-type", types[".html"]);
      response.end(`<!doctype html><meta charset="utf-8"><script>
        window.viewerStates=[];
        window.viewerLoaded=false;
        addEventListener('message', event => { if (event.source === document.querySelector('#pdf').contentWindow) viewerStates.push(event.data); });
      </script><iframe id="pdf" src="http://localhost:${appServer.address().port}/viewer.html?reader=${encodeURIComponent(reader)}" onload="window.viewerLoaded=true;this.contentWindow.postMessage({librepaper:true,type:'reader-ready'},'*')"></iframe>`);
      return;
    }
    if (url.pathname === "/viewer.html") {
      const reader = url.searchParams.get("reader") || "";
      const builtPage = [join(viewerBuild, "viewer.html"), join(viewerBuild, "pages/viewer.html")].find(existsSync);
      const html = readFileSync(builtPage, "utf8").replace("</body>", `<script src="/frame.js?reader=${encodeURIComponent(reader)}"></script></body>`);
      response.setHeader("content-type", types[".html"]);
      response.end(html);
      return;
    }
    if (url.pathname === "/frame.js") {
      // The viewer reads the configured recipient from this exact production
      // injection URL. The PDF agent itself is outside this origin-pin test.
      response.setHeader("content-type", types[".js"]);
      response.end("");
      return;
    }
    if (url.pathname.startsWith("/assets/")) {
      const file = join(viewerBuild, url.pathname.slice(1));
      if (existsSync(file)) {
        response.setHeader("content-type", extname(file) === ".js" ? types[".js"] : "application/octet-stream");
        response.end(readFileSync(file));
        return;
      }
    }
    const asset = url.pathname === "/preview.js" ? "preview.js" : null;
    if (asset) {
      response.setHeader("content-type", types[".js"]);
      response.end(readFileSync(join(temporary, "build", asset)));
      return;
    }
    response.writeHead(404).end("not found");
  });
  await new Promise((resolve, reject) => { appServer.once("error", reject); appServer.listen(0, resolve); });
  const port = appServer.address().port;
  page = await browser(process.env.BROWSER || "chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${port}/`);
  await until("Preview mounted and external document script ran", () => page.frameEvaluate("window.thirdScriptRan === true"), 10000);

  const frame = await page.evaluate("document.querySelector('iframe[title=Document]') && ({src:document.querySelector('iframe[title=Document]').src,sandbox:document.querySelector('iframe[title=Document]').getAttribute('sandbox')})");
  assert.equal(new URL(frame.src).origin, `http://localhost:${port}`);
  assert.match(frame.sandbox, /allow-same-origin/);
  assert.match(frame.sandbox, /allow-scripts/);
  assert.doesNotMatch(frame.sandbox, /allow-downloads|allow-top-navigation/);

  const access = await page.frameEvaluate("({parentDenied:window.thirdScriptParentDenied,storageDenied:window.thirdScriptStorageDenied,appCookieVisible:document.cookie.includes('app_session=') || document.cookie.includes('app_canary='),docsStorage:localStorage.getItem('docs-secret')})");
  assert.equal(access.parentDenied, true, "the script executing in docs cannot read app DOM");
  assert.equal(access.storageDenied, true, "the script executing in docs cannot read app localStorage");
  assert.equal(access.appCookieVisible, false, "the host-only HttpOnly app cookie is unavailable from docs");
  assert.equal(access.docsStorage, "private-to-docs", "the script still runs with the document origin");
  await until("allowed ready and selection messages", () => page.evaluate("window.messages.some(m => m.type === 'ready' && m.text === 'Document text') && window.messages.some(m => m.type === 'selection')"));
  const received = await page.evaluate("window.messages.map(m => m.type)");
  assert.ok(received.includes("ready"));
  assert.ok(received.includes("selection"));
  assert.ok(!received.includes("admin-command"), "unknown privileged messages are dropped");
  assert.ok(received.length <= 2, `malformed and oversized messages are dropped; got ${received}`);
  assert.equal(await page.evaluate("location.pathname"), "/", "a sandboxed document cannot navigate the top page");

  // A separate same-origin window is not the embedded document and cannot
  // impersonate it. A third-origin window has both the wrong source and origin.
  await page.evaluate(`(() => { const other=document.createElement('iframe'); other.src='http://localhost:${port}/sibling'; document.body.append(other); })()`);
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.ok(!String(await page.evaluate("JSON.stringify(window.messages)")).includes("sibling"));
  await page.evaluate(`(() => { const other=document.createElement('iframe'); other.src='http://127.0.0.1:${port}/wrong-origin'; document.body.append(other); })()`);
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.ok(!String(await page.evaluate("JSON.stringify(window.messages)")).includes("wrong-origin"));

  // The expected frame can navigate itself to another origin; its messages
  // still fail the configured docs-origin check. Restore the docs URL before
  // the reload/budget scenario below.
  await page.evaluate(`document.querySelector('iframe[title=Document]').contentWindow.postMessage({isolateNavigate:true},'*')`);
  await until("current Preview frame posted from the external origin", () => page.evaluate(`window.rawFrameEvents.some(event => event.origin === ${JSON.stringify(scriptOrigin)} && event.fromPreviewFrame)`), 5000);
  await until("external-origin frame load handler ran", () => page.evaluate("window.previewFrameLoads >= 2"), 5000);
  assert.ok(!String(await page.evaluate("JSON.stringify(window.messages)")).includes("external-origin-doc"), "a message from the current frame at an untrusted origin is rejected");
  await page.evaluate(`document.querySelector('iframe[title=Document]').src='http://localhost:${port}/docs'`);
  await until("main document restored to docs origin", () => page.frameEvaluate(`location.origin === 'http://localhost:${port}' && window.reloadCount >= 2`), 5000);
  await until("restored docs frame load handler ran", () => page.evaluate("window.previewFrameLoads >= 3"), 5000);

  // Flood both message classes until their finite budgets are used, then put
  // an expensive repaint in the ready queue. The same-URL iframe load must
  // cancel that delayed repaint; selection cancellation is pinned by the
  // deterministic receiver test because its shorter refill window is 83ms.
  await page.evaluate("window.messages.length = 0");
  await page.frameEvaluate(`for (let i=0;i<64;i++) parent.postMessage({librepaper:true,type:'ready',text:'before-reload-'+i},'*'); for (let i=0;i<64;i++) parent.postMessage({librepaper:true,type:'selection',selector:{exact:'before-reload-'+i,prefix:'',suffix:''}},'*')`);
  await new Promise((resolve) => setTimeout(resolve, 100));
  const bounded = await page.evaluate("({ready:window.messages.filter(m => m.type === 'ready').length,selection:window.messages.filter(m => m.type === 'selection').length})");
  assert.ok(bounded.ready < 64 && bounded.selection < 64, `flood counts remain bounded: ${JSON.stringify(bounded)}`);
  await page.frameEvaluate("parent.postMessage({librepaper:true,type:'ready',text:'stale-ready-before-reload-'+'x'.repeat(15000000)},'*')");
  await page.frameEvaluate("location.reload(); true");
  await until("same docs document reloaded", () => page.frameEvaluate("window.reloadCount >= 3"), 5000);
  await until("reloaded docs frame load handler ran", () => page.evaluate("window.previewFrameLoads >= 4"), 5000);
  // Do not send a replacement repaint until the old 16-unit payload would
  // have become eligible. Otherwise coalescing could hide missing load-time
  // cancellation even if Preview stopped calling frameLoaded().
  await new Promise((resolve) => setTimeout(resolve, 8500));
  assert.ok(!String(await page.evaluate("JSON.stringify(window.messages)")).includes("stale-ready-before-reload"));

  // `frameLoaded()` drops queued messages while retaining the depleted token
  // buckets. A new flood remains bounded until tokens refill, after which a
  // legitimate repaint is delivered again.
  await page.evaluate("window.messages.length = 0");
  await page.frameEvaluate(`for (let i=0;i<32;i++) parent.postMessage({librepaper:true,type:'ready',text:'after-reload-flood-'+i},'*')`);
  await new Promise((resolve) => setTimeout(resolve, 100));
  const afterReloadFlood = await page.evaluate("window.messages.filter(m => m.type === 'ready').length");
  assert.ok(afterReloadFlood < 32, `reload must retain the depleted budget (accepted ${afterReloadFlood} of 32 immediately)`);
  await until("queued repaint after reload refill", () => page.evaluate("window.messages.some(m => m.text === 'after-reload-flood-31')"), 5000);
  await page.frameEvaluate("parent.postMessage({librepaper:true,type:'ready',text:'legitimate-after-reload-refill'},'*')");
  await until("legitimate message after reload refill", () => page.evaluate("window.messages.some(m => m.text === 'legitimate-after-reload-refill')"), 5000);

  // The production PDF viewer is built from pages/viewer.html and receives
  // its reader origin through the same frame.js injection that deployment
  // supplies. It must ignore the actual parent when the injected origin is
  // wrong, then report state when the injected origin matches that parent.
  await page.navigate(`http://127.0.0.1:${port}/viewer-host?reader=${encodeURIComponent(`http://localhost:${port}`)}`);
  await until("misconfigured PDF viewer loaded", () => page.evaluate("window.viewerLoaded === true"), 5000);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(await page.evaluate("window.viewerStates.some(message => message.type === 'viewer-state')"), false, "viewer rejects a parent whose origin differs from its configured reader");
  await page.navigate(`http://127.0.0.1:${port}/viewer-host?reader=${encodeURIComponent(`http://127.0.0.1:${port}`)}`);
  await until("PDF viewer state reaches its configured reader", () => page.evaluate("window.viewerStates.some(message => message.type === 'viewer-state')"), 5000);

  // This harness proves browser origin and frame-message boundaries. API and
  // WebSocket authentication require a live app and remain outside its scope.
  console.log("frame-isolation-browser: actual Preview receiver, PDF viewer origin pin, cross-origin DOM/storage/cookie boundary, source/origin checks, message filtering, flood bound, refill, and sandbox navigation passed (API/WS auth not exercised)");
} finally {
  await page?.close?.();
  if (appServer) await new Promise((resolve) => appServer.close(resolve));
  if (scriptServer) await new Promise((resolve) => scriptServer.close(resolve));
  removeTemporary(temporary);
}
