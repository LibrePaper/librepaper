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
<script>window.published=[];window.events=[];addEventListener("message",e=>{if(!e.data||!e.data.librepaper)return;if(e.data.type==="ready")window.published.push(e.data.text);if(["selection","focus"].includes(e.data.type))window.events.push(e.data)});</script>
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
  const readyBeforePoint = await page.evaluate('window.published.length');
  await page.evaluate(`postMessage({librepaper:true,type:'highlight',ranges:[
    {id:'point',start:window.trackedStart+2,end:window.trackedStart+2,point:true,motivation:'commenting'},
    {id:'second-point',start:window.trackedStart+4,end:window.trackedStart+4,point:true,motivation:'commenting'},
    {id:'custom',start:window.trackedStart,end:window.trackedStart+5,motivation:'commenting',color:'#ff8800'}
  ]},'*')`);
  await until("point bubble painted", () => page.evaluate('Boolean(document.querySelector(".librepaper-point-bubble"))'), 5000);
  assert.equal(await page.evaluate('document.body.textContent'), original, "point marker has no text");
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(await page.evaluate('window.published.length'), readyBeforePoint, "annotation painting does not republish ready");
  assert.match(await page.evaluate('getComputedStyle(document.querySelector("mark[data-librepaper~=custom]")).backgroundColor'), /color\(srgb 1 0\.53333\d* 0 \/ 0\.42\)|rgba\(255, 136, 0, 0\.42\)/);
  const pointOffsets = () => page.evaluate(`['point','second-point'].map(id=>{
    const marker=document.querySelector('.librepaper-point-marker[data-librepaper="'+id+'"]');
    const range=document.createRange(); range.selectNodeContents(document.body); range.setEndBefore(marker);
    return range.toString().length-window.trackedStart;
  })`);
  assert.deepEqual(await pointOffsets(), [2,4], 'multiple points survive text-node splitting inside a highlight');
  await page.resize(390, 844);
  assert.deepEqual(await pointOffsets(), [2,4], 'point markers remain anchored after reflow');
  assert.equal(await page.evaluate('document.querySelector(".librepaper-point-bubble").getBoundingClientRect().width > 0'), true);
  await page.evaluate(`postMessage({librepaper:true,type:'reveal',id:'point'},'*'); document.querySelector('.librepaper-point-bubble').click()`);
  await until("point bubble focuses thread", () => page.evaluate('window.events.some((e) => e.type === "focus" && e.id === "point")'), 2000);
  await page.evaluate(`postMessage({librepaper:true,type:'tool',tool:'point'},'*')`);
  const clickPoint = await page.evaluate(`(() => {
    const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);
    let textNode; while ((textNode=walker.nextNode())) { if(textNode.data.includes('A second paragraph')) break; }
    const range=document.createRange(); range.setStart(textNode,2); range.setEnd(textNode,3);
    const rect=range.getBoundingClientRect(); return {x:rect.left+1,y:rect.top+rect.height/2};
  })()`);
  await page.command('Input.dispatchMouseEvent',{type:'mousePressed',...clickPoint,button:'left',clickCount:1});
  await page.command('Input.dispatchMouseEvent',{type:'mouseReleased',...clickPoint,button:'left',clickCount:1});
  await until("point click selection", () => page.evaluate('window.events.some((e) => e.type === "selection" && e.selector?.point === true)'), 2000);
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.equal(await page.evaluate('window.events.filter(e=>e.type==="selection").at(-1).selector?.point'), true, 'collapsed selection events do not erase the point draft');
  await page.evaluate(`postMessage({librepaper:true,type:'highlight',ranges:[]},'*')`);
  await until("suggestions cleared", () => page.evaluate('!document.querySelector("mark[data-librepaper]")'), 5000);
  assert.equal(await page.evaluate('document.body.textContent'), original);
  assert.equal(await page.evaluate('document.querySelectorAll(".librepaper-point-marker").length'), 0);
  console.log("markdown-tracking: formatted suggestions, one proposal, overlapping comments, inline redlines, stable text and clearing passed");
} finally {
  await page?.close?.();
  server?.close();
  rmSync(temporary, { recursive: true, force: true });
}
