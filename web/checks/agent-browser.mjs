// Exercise the actual Svelte mailbox panel in Chromium. The mock is a server
// mailbox: there is no local bridge, pairing code, adapter, or model setting.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
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
import ${JSON.stringify(join(root, "src/styles/app.css"))};
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
  await build({ configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } } });
  server = createServer((request, response) => {
    if (request.url === '/komodoc-web.css') {
      response.setHeader('content-type', 'text/css');
      response.end(readFileSync(join(temporary, 'build/komodoc-web.css')));
      return;
    }
    response.setHeader("content-type", request.url === "/panel.js" ? "text/javascript" : "text/html");
    response.end(request.url === "/panel.js" ? readFileSync(join(temporary, "build/panel.js")) : '<!doctype html><html data-theme="komodoc"><head><link rel="stylesheet" href="/komodoc-web.css"><style>body { display:flex; height:700px; width:360px; overflow:hidden; }</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 22000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("panel mailbox", () => page.evaluate("Boolean(document.querySelector('.agent-panel textarea[aria-label=Message]'))"), 10000);
  await until("agent listening", () => page.evaluate('document.querySelector("[role=status]")?.textContent === "Listening"'), 10000);
  assert.equal(await page.evaluate("Boolean(document.querySelector('[aria-label=\"Copyable CLI instructions\"]'))"), false);
  assert.equal(await page.evaluate("document.querySelector('.agent-actions button').textContent"), "Copy instructions");
  assert.doesNotMatch(await page.evaluate('window.calls.map(c=>c.suffix).join("\\n")'), /secret-token/);

  await page.evaluate(`(() => { const input=document.querySelector('textarea[placeholder]'); input.value='Explain this'; input.dispatchEvent(new Event('input',{bubbles:true})); input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true})); })()`);
  await until("agent reply", () => page.evaluate('document.querySelector("[role=log]")?.textContent.includes("<img")'), 10000);
  assert.equal(await page.evaluate("Boolean(document.querySelector('.agent-setup'))"), false);
  assert.equal(await page.evaluate('Boolean(window.injected || document.querySelector("[role=log] img"))'), false);
  const posted = await page.evaluate('window.calls.find(c=>c.body.role==="user").body');
  assert.equal(posted.context.file, "paper.md");
  assert.equal(posted.context.selection, "A passage");
  assert.equal(await page.evaluate('window.calls.find(c=>c.body.role==="user").headers["X-Komodoc-Key"]'), "secret");
  assert.equal(await page.evaluate('window.calls.find(c=>c.body.role==="user").headers["X-Komodoc-Chat-Token"]'), "secret-token");
  assert.equal(await page.evaluate('document.querySelector("[role=log]").scrollTop + document.querySelector("[role=log]").clientHeight >= document.querySelector("[role=log]").scrollHeight - 1'), true);

  await page.evaluate(`(() => {
    const clipboard = { writeText: value => { window.copiedInstructions = value; return Promise.resolve(); } };
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: clipboard });
    document.querySelector('.agent-actions button').click();
  })()`);
  await until("instructions copied", () => page.evaluate('typeof window.copiedInstructions === "string"'), 1000);
  assert.match(await page.evaluate('window.copiedInstructions'), /komodoc agent chat watch/);
  assert.equal(await page.evaluate('document.querySelector("[aria-label=Conversation]").textContent.includes("komodoc agent chat")'), false);

  await page.evaluate(`(async () => {
    for (let i = 0; i < 24; i++) {
      await fetch('/api/documents/paper/chat/conversation', {
        method: 'POST', body: JSON.stringify({ id: 'history-' + i, role: 'agent', text: ('History message ' + i + ' ').repeat(12) }),
      });
    }
  })()`);
  await page.evaluate('window.remount()');
  await until("history replay", () => page.evaluate('document.querySelector("[role=log]")?.textContent.includes("History message 23")'), 10000);
  await until("transcript overflow", () => page.evaluate(`(() => {
    const log = document.querySelector('[role=log]');
    return log && log.querySelectorAll('.agent-message').length >= 20 && log.scrollHeight > log.clientHeight && getComputedStyle(log).overflowY === 'auto';
  })()`), 10000);
  await page.evaluate(`(() => {
    const log = document.querySelector('[role=log]');
    log.scrollTop = 0;
    log.dispatchEvent(new Event('scroll'));
  })()`);
  await page.evaluate(`fetch('/api/documents/paper/chat/conversation', {
    method: 'POST', body: JSON.stringify({ id: 'history-late', role: 'agent', text: 'A reply while reading history' }),
  })`);
  await until("new messages affordance", () => page.evaluate('Boolean(document.querySelector(".new-messages"))'), 10000);
  assert.equal(await page.evaluate('document.querySelector("[role=log]").scrollTop'), 0);
  await page.evaluate('document.querySelector(".new-messages").click()');
  assert.equal(await page.evaluate(`(() => {
    const log = document.querySelector('[role=log]');
    return log.scrollTop + log.clientHeight >= log.scrollHeight - 1;
  })()`), true);

  await page.evaluate(`(() => { const input=document.querySelector('textarea[placeholder]'); input.value='First'; input.dispatchEvent(new Event('input',{bubbles:true})); input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true})); })()`);
  assert.equal(await page.evaluate("document.querySelector('textarea[placeholder]').value"), "First\n");

  await page.evaluate('window.remount()');
  await until("replayed mailbox", () => page.evaluate('document.querySelector("[role=log]")?.textContent.includes("Explain this")'), 10000);
  assert.equal(await page.evaluate('window.calls.filter(c=>c.suffix==="/conversation" && c.body.role==="user").length'), 1);
  console.log("agent-browser: mailbox creation, CLI handoff, private headers, context, escaped text and remount replay passed");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
