import assert from "node:assert/strict";
import { build } from "vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-history-semantic-"));
const entry = join(temporary, "entry.js");
let server, page;
try {
  writeFileSync(entry, `import * as projection from ${JSON.stringify(join(root, "src/lib/diff-display.js"))}; window.contract=projection;`);
  await build({ configFile: false, root, logLevel: "error", build: { outDir: join(temporary, "build"), lib: { entry, formats: ["iife"], name: "HistoryContract", fileName: () => "contract.js" } } });
  let unexpectedRequests = 0;
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    if (path === "/") {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><body><script>window.messages=[];addEventListener("message",e=>messages.push(e.data));</script><script src="/contract.js"></script><script src="/agent.js?reader=*"></script>');
      return;
    }
    if (path.startsWith("/unexpected")) unexpectedRequests++;
    const file = path === "/contract.js" ? join(temporary, "build/contract.js") : join(root, "dist", path);
    if (!existsSync(file)) { response.statusCode = 404; response.end(); return; }
    response.setHeader("content-type", path.endsWith(".js") ? "text/javascript" : "application/octet-stream");
    response.end(readFileSync(file));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  page = await browser("chromium", join(temporary, "profile"), 22000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("history contract loaded", () => page.evaluate('Boolean(window.contract) && messages.some(m=>m.type==="ready")'), 10000);
  assert.equal(await page.evaluate(`(()=>{
    const a=contract.projectHtml('<p>inter<span>nation</span>al é 👩‍🔬</p>');
    const b=contract.projectHtml('<div><p>international é 👩‍🔬</p></div>');
    return contract.validateProjection(a) && a.tokens.some(t=>t.value==='international') && contract.projectionHunks(a,b).length===0;
  })()`), true, "inline and layout wrappers do not change words");
  assert.equal(await page.evaluate(`(()=>{
    const a=contract.projectHtml('<p>A   <em>word</em>.</p>');
    const b=contract.projectHtml('<p>A word.</p>');
    return contract.projectionHunks(a,b).length===0;
  })()`), true, "prose whitespace is normalized");
  assert.equal(await page.evaluate(`(()=>{
    const a=contract.projectHtml('<!doctype html><html><head><title>Page title</title><style>p{color:red}</style></head><body><main><h1>Heading</h1><p>A word.</p></main></body></html>');
    const b=contract.projectHtml('<main><h1>Heading</h1><p>A word.</p></main>');
    return a.text === b.text && contract.projectionHunks(a,b).length===0;
  })()`), true, "full-document head metadata is not reader-visible content");
  assert.equal(await page.evaluate(`(()=>{
    const html='<!doctype html><html><head><title>Code</title></head><body>\\n<pre>  x\\n  y</pre>\\n</body></html>';
    const parsed=new DOMParser().parseFromString(html,'text/html');
    const a=contract.projectHtml(html), b=contract.projectDom(parsed.body);
    return a.text === '  x\\n  y' && contract.projectionHunks(a,b).length===0;
  })()`), true, "block formatting is ignored without losing code whitespace");
  assert.equal(await page.evaluate(`(()=>{
    const a=contract.projectHtml('<math><mfrac><mi>a</mi><mi>b</mi></mfrac></math>');
    const b=contract.projectHtml('<math><mrow><mi>a</mi><mi>b</mi></mrow></math>');
    return a.complete && b.complete && contract.projectionHunks(a,b).length>0;
  })()`), true, "equation structure is not flattened away");
  await page.evaluate(`contract.projectHtml('<p>safe</p><img src="/unexpected-image"><iframe src="/unexpected-frame"></iframe><script>window.executed=true</script>')`);
  assert.equal(unexpectedRequests, 0, "baseline projection makes no resource requests");
  assert.equal(await page.evaluate("Boolean(window.executed)"), false);

  await page.evaluate(`(()=>{
    window.target='<!doctype html><html><head><title>Rendered document</title></head><body>\\n<p>Repeated word. Repeated <em>new</em> word.</p>\\n<table><tr><td>A</td></tr></table>\\n</body></html>';
    window.baseline='<p>Repeated word. Repeated old word.</p>';
    window.targetProjection=contract.projectHtml(target);
    window.hunks=contract.projectionHunks(contract.projectHtml(baseline),targetProjection);
    postMessage({librepaper:true,type:'preview',html:target,frameGeneration:1},'*');
  })()`);
  await until("target painted", () => page.evaluate('document.querySelector("em")?.textContent==="new"'), 5000);
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',version:1,generation:1,frameGeneration:1,targetProjection,hunks},'*')`);
  await until("semantic marks painted", () => page.evaluate('messages.some(m=>m.type==="semantic-redlines-painted" && m.generation===1)'), 5000);
  assert.equal(await page.evaluate('document.querySelector("em mark.librepaper-semantic-ins")?.textContent'), "new");
  assert.equal(await page.evaluate('document.querySelector("mark.librepaper-semantic-del")?.dataset.deleted'), "old");
  assert.equal(await page.evaluate('Boolean(document.querySelector("table[data-librepaper-semantic-in]"))'), true);
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',items:[]},'*')`);
  await until("marks cleared", () => page.evaluate('!document.querySelector("mark.librepaper-semantic-ins,mark.librepaper-semantic-del,[data-librepaper-semantic-in]")'), 5000);
  assert.equal(await page.evaluate('document.querySelector("p").textContent'), "Repeated word. Repeated new word.");
  await page.evaluate(`postMessage({librepaper:true,type:'highlight',ranges:[{id:'suggestion-one',start:0,end:8,motivation:'editing',proposed:'<img src=/unexpected-proposal>'}]},'*')`);
  await until("proposal painted as synthetic text", () => page.evaluate('Boolean(document.querySelector(".librepaper-suggestion-synthetic"))'), 5000);
  assert.equal(await page.evaluate('document.querySelector(".librepaper-suggestion-synthetic").textContent'), '<img src=/unexpected-proposal>');
  assert.equal(await page.evaluate('Boolean(document.querySelector(".librepaper-suggestion-synthetic img"))'), false);
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',version:1,generation:2,frameGeneration:1,targetProjection,hunks},'*')`);
  await until("history with suggestion painted", () => page.evaluate('messages.some(m=>m.type==="semantic-redlines-painted" && m.generation===2)'), 5000);
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',items:[]},'*')`);
  await until("history alone cleared", () => page.evaluate('!document.querySelector("mark.librepaper-semantic-ins")'), 5000);
  assert.equal(await page.evaluate('Boolean(document.querySelector(".librepaper-suggestion-synthetic"))'), true, "history toggle preserves pending suggestions");
  assert.equal(await page.evaluate('contract.projectDom(document.body).text === targetProjection.text'), true, "proposal text is excluded from document identity");
  await page.evaluate(`postMessage({librepaper:true,type:'redlines',version:1,generation:0,frameGeneration:0,targetProjection,hunks},'*')`);
  await until("stale target rejected", () => page.evaluate('messages.some(m=>m.type==="semantic-redlines-rejected" && m.reason==="stale-generation")'), 5000);
  console.log("history semantic browser: inline Unicode, whitespace, structural math, inert baseline, exact DOM mapping and stale frame rejection passed");
} finally {
  await page?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  rmSync(temporary, { recursive: true, force: true });
}
