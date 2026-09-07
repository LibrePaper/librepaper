// Exercise the actual Svelte mailbox panel in Chromium. The mock is a server
// mailbox: there is no local bridge, pairing code, adapter, or model setting.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "komodoc-agent-browser-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Agent from ${JSON.stringify(join(root, "src/components/Agent.svelte"))};
let messages = [], cursor = 0, nextId = 0;
window.calls = [];
window.fetch = async (url, init) => {
  const parsed = new URL(url, location.href);
  const suffix = parsed.pathname.split('/chat')[1];
  const body = init.body ? JSON.parse(init.body) : {};
  window.calls.push({ suffix, body, headers: {...init.headers} });
  if (suffix === '') return Response.json({id:'conversation',token:'secret-token'});
  if (suffix === '/conversation' && init.method === 'GET') return Response.json({messages, next_cursor:cursor, listening:true});
  if (suffix === '/conversation' && init.method === 'POST') {
    const message = {...body, cursor:++cursor};
    messages.push(message);
    if (body.role === 'user') messages.push({id:'agent-'+(++nextId),cursor:++cursor,role:'agent',text:'<img src=x onerror="window.injected=true">'});
    return Response.json({message, next_cursor:cursor});
  }
  if (suffix === '/conversation' && init.method === 'DELETE') return Response.json({deleted:true});
  throw new Error('unexpected mailbox request');
};
let component;
window.remount = async () => {
  if (component) await unmount(component);
  component = mount(Agent, {target:document.body,props:{slug:'paper',link:location.origin+'/docs/paper#k=secret',path:'paper.md',selection:{exact:'A passage'}}});
};
window.remount();
`);

let server, page;
try {
  await build({ configFile: false, root, plugins: [svelte()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } } });
  server = createServer((request, response) => {
    response.setHeader("content-type", request.url === "/panel.js" ? "text/javascript" : "text/html");
    response.end(request.url === "/panel.js" ? readFileSync(join(temporary, "build/panel.js")) : '<body><script type="module" src="/panel.js"></script>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 22000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("panel mailbox", () => page.evaluate("Boolean(document.querySelector('.agent-panel textarea[aria-label]'))"), 10000);
  await until("agent listening", () => page.evaluate('document.querySelector("[role=status]")?.textContent === "Listening"'), 10000);
  const instructions = await page.evaluate("document.querySelector('.agent-panel textarea[aria-label]').value");
  assert.match(instructions, /komodoc agent chat watch/);
  assert.match(instructions, /--after 0 --timeout 25/);
  assert.match(instructions, /raw\.githubusercontent\.com.*SKILL\.md/);
  assert.match(instructions, /secret-token/);
  assert.doesNotMatch(await page.evaluate('window.calls.map(c=>c.suffix).join("\\n")'), /secret-token/);

  await page.evaluate(`(() => { const input=document.querySelector('textarea[placeholder]'); input.value='Explain this'; input.dispatchEvent(new Event('input',{bubbles:true})); input.form.requestSubmit(); })()`);
  await until("agent reply", () => page.evaluate('document.querySelector("[role=log]")?.textContent.includes("<img")'), 10000);
  assert.equal(await page.evaluate('Boolean(window.injected || document.querySelector("[role=log] img"))'), false);
  const posted = await page.evaluate('window.calls.find(c=>c.body.role==="user").body');
  assert.equal(posted.context.file, "paper.md");
  assert.equal(posted.context.selection, "A passage");
  assert.equal(await page.evaluate('window.calls.find(c=>c.body.role==="user").headers["X-Komodoc-Key"]'), "secret");
  assert.equal(await page.evaluate('window.calls.find(c=>c.body.role==="user").headers["X-Komodoc-Chat-Token"]'), "secret-token");

  await page.evaluate('window.remount()');
  await until("replayed mailbox", () => page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'), 10000);
  assert.equal(await page.evaluate('window.calls.filter(c=>c.suffix==="/conversation" && c.body.role==="user").length'), 1);
  console.log("agent-browser: mailbox creation, CLI handoff, private headers, context, escaped text and remount replay passed");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
