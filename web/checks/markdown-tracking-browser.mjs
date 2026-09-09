// Track changes over HTML compiled by the real Markdown renderer.
import { call } from "../src/lib/renderer-wasm.js";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const dist = join(root, "dist");
assert.ok(existsSync(join(dist, "agent.js")), "run `bun run build` first");
const temporary = mkdtempSync(join(tmpdir(), "librepaper-markdown-tracking-"));

const types = { js: "text/javascript", css: "text/css", woff2: "font/woff2", html: "text/html" };
// The page is its own parent: the agent takes messages from `parent`, which
// at the top level is the window itself, so the check plays the sidebar.
const shell = `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script>window.published=[];addEventListener("message",e=>{if(e.data&&e.data.librepaper&&e.data.type==="ready")window.published.push(e.data.text)});</script>
<script src="/agent.js?reader=*"></script>
</body></html>`;

let server, page;
try {
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    if (path === "/") {
      response.setHeader("content-type", types.html);
      response.end(shell);
      return;
    }
    const file = join(dist, path);
    if (!existsSync(file)) {
      response.statusCode = 404;
      response.end("not found");
      return;
    }
    response.setHeader("content-type", types[path.split(".").pop()] || "application/octet-stream");
    response.end(readFileSync(file));
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 22000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("agent loaded", () => page.evaluate("window.published.length > 0"), 10000);

  const preview = (html) => page.evaluate(`postMessage({librepaper:true,type:"preview",html:${JSON.stringify(html)}},"*")`);

  const wasm = new WebAssembly.Instance(new WebAssembly.Module(readFileSync(join(dist, "wasm/markdown.wasm"))), {}).exports;
  const rendered = call(wasm, "compile", "# Review\n\nThe **quick** *brown* [fox](https://example.org) jumps.\n\nA second paragraph.", "Review");
  assert.equal(rendered.ok, true);
  await preview(rendered.text);
  await until("Markdown published", () => page.evaluate('window.published.at(-1).includes("quick brown fox")'), 5000);
  const original = await page.evaluate('document.body.textContent');
  await page.evaluate(`
    const start = window.published.at(-1).indexOf('quick brown fox');
    window.trackedStart = start;
    postMessage({librepaper:true,type:'highlight',ranges:[
      {id:'suggestion',start,end:start+15,motivation:'editing',proposed:'slow red dog'},
      {id:'comment',start:start+6,end:start+11,motivation:'commenting'}
    ]},'*');
  `);
  await until("suggestion painted once", () => page.evaluate('document.querySelectorAll("mark[data-proposed]").length === 1'), 5000);
  assert.equal(await page.evaluate('[...document.querySelectorAll("mark[data-librepaper~=suggestion]")].map(m=>m.textContent).join("")'), 'quick brown fox');
  assert.equal(await page.evaluate('document.querySelectorAll("p strong, p em, p a[href]").length'), 3);
  assert.equal(await page.evaluate('getComputedStyle(document.querySelector("mark[data-proposed]"),"::after").content'), '"slow red dog"');
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',items:[
    {kind:'insert',start:window.trackedStart,end:window.trackedStart+15,who:'Editor'},
    {kind:'delete',at:window.trackedStart,text:'old wording',who:'Editor'}
  ]},'*')`);
  await until("redlines painted", () => page.evaluate('Boolean(document.querySelector("mark.librepaper-del"))'), 5000);
  assert.equal(await page.evaluate('[...document.querySelectorAll("mark.librepaper-ins")].map(m=>m.textContent).join("")'), 'quick brown fox');
  assert.equal(await page.evaluate('getComputedStyle(document.querySelector("mark.librepaper-del"),"::before").content'), '"old wording"');
  assert.equal(await page.evaluate('document.body.textContent'), original);
  assert.equal(await page.evaluate('document.querySelectorAll("mark[data-proposed]").length'), 1);
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',items:[]},'*')`);
  await until("redlines cleared", () => page.evaluate('!document.querySelector("mark.librepaper-ins, mark.librepaper-del")'), 5000);
  assert.equal(await page.evaluate('document.querySelectorAll("mark[data-librepaper~=comment]").length'), 1);
  assert.equal(await page.evaluate('document.body.textContent'), original);
  await page.evaluate(`postMessage({librepaper:true,type:'highlight',ranges:[]},'*')`);
  await until("suggestions cleared", () => page.evaluate('!document.querySelector("mark[data-librepaper]")'), 5000);
  assert.equal(await page.evaluate('document.body.textContent'), original);
  console.log("markdown-tracking: formatted suggestions, one proposal, overlapping comments, inline redlines, stable text and clearing passed");
} finally {
  await page?.close?.();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
