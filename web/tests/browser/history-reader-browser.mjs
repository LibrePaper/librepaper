// Exercise the real Reader, sidebar, history controls and CodeMirror together.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-history-reader-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");
const sources = ["<p>first red draft</p>", "<p>second blue draft</p>", "<p>third green draft</p>",
  "<p>routine fourth</p>", "<p>routine fifth</p>", "<p>routine sixth</p>"];
const current = "<p>current purple draft</p>";
const points = sources.map((text, i) => ({
  sha: `00000000-0000-4000-8000-${String(i + 1).padStart(12, "0")}`,
  at: `2026-09-10T09:0${i}:00Z`, seq: i + 1, main: "main.html", by: "Editor", why: i > 2 ? "quiet" : "label",
  label: ["Original", "Blah blah", "Latest checkpoint"][i] || "", changed: ["main.html"],
  texts: { "main.html": text }, files: { "main.html": { kind: "text" } },
}));
for (let i = 1; i < points.length; i++) points[i].parent = points[i - 1].sha;

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = value => Uint8Array.from(atob(value), char => char.charCodeAt(0));
const server = new LoroDoc();
const saved = localStorage.getItem('history-test-source');
if (saved) server.import(decode(saved));
let text = server.getMap('files').get('main');
if (!text) {
  text = new LoroText(); text.insert(0, ${JSON.stringify(current)});
  server.getMap('files').setContainer('main', text); server.getMap('paths').set('main', 'main.html');
  server.getMap('meta').set('main', 'main');
  server.commit();
}
const persist = () => localStorage.setItem('history-test-source', encode(server.export({ mode: "update" })));
persist();
let updateSub = server.subscribeLocalUpdates(() => persist());
export function openRoom(_slug, {onMessage, onConnected}) {
  // The commit is not a formality: updates leave on a commit, so an append
  // without one changes the mock and tells nobody.
  window.historyLive = { source: () => text.toString(), append: value => { text.insert(text.length, value); server.commit(); } };
  let updateSub2 = server.subscribeLocalUpdates(update => onMessage({type:'doc-update', update:encode(update)}));
  queueMicrotask(() => onConnected(true));
  return { send(message) {
    if (message.type === 'doc-open') setTimeout(() => onMessage({type:'doc-state',update:encode(server.export({ mode: "update" })),count:1}), location.search.includes('slowjoin') ? 350 : 0);
    if (message.type === 'doc-update') server.import(decode(message.update));
    return {ok:true};
  }, sendLive() { return {ok:true}; }, close() { updateSub2?.(); } };
}`);
writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
window.testErrors=[];
addEventListener('error', event => window.testErrors.push(event.error?.stack || event.message));
addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
const {default: Reader}=await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const {mount}=await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader,{target:document.body});
`);
let http, tab;
let delayedSha = "";
const delayed = [];
const failed = new Set();
const json = (response, value, status = 200) => {
  response.statusCode = status; response.setHeader("content-type", "application/json"); response.end(JSON.stringify(value));
};
try {
  await build({ configFile:false, root:join(root, "web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {
    name:"history-test-room", enforce:"pre", resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    },
  }], build:{outDir:out,emptyOutDir:true,minify:false,lib:{entry,formats:["es"],fileName:()=>"check.js",cssFileName:"check"}}, logLevel:"error" });
  http = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/me") return json(response, {name:"Editor",providers:[]});
    if (url.pathname === "/api/documents/paper") return json(response, {
      title:"History test", created_at:"history-test", role:"owner", source_format:"html",
      docs_origin:`http://${request.headers.host}`, can_see_sharing:true, can_moderate:true,
    });
    if (url.pathname === "/api/documents/paper/share") return json(response, {links:{}});
    if (url.pathname === "/api/documents/paper/publication") return json(response, {publication:null});
    if (url.pathname === "/api/documents/paper/history") {
      const list = points.map(({texts, files, ...point}) => point).reverse();
      return json(response, url.searchParams.has('after') ? {checkpoints:list.slice(-1)} : {checkpoints:list.slice(0, -1),next_cursor:2});
    }
    if (url.pathname.startsWith("/api/documents/paper/history/")) {
      const point = points.find(point => url.pathname.endsWith(point.sha));
      if (!point || failed.has(point.sha)) return json(response, {error:"Checkpoint unavailable"}, 404);
      // The real checkpoint endpoint omits parent; ancestry lives in the list.
      const {parent, seq, changed, ...payload} = point;
      if (point.sha === delayedSha) { delayed.push(() => json(response, payload)); return; }
      return json(response, payload);
    }
    if (url.pathname.startsWith("/raw/")) { response.setHeader("content-type", "text/html"); return response.end("<!doctype html><body></body>"); }
    if (url.pathname === "/docs/paper") {
      response.setHeader("content-type", "text/html");
      return response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/check.css"></head><body><script type="module" src="/check.js"></script></body></html>');
    }
    try {
      const file = join(out, url.pathname.slice(1));
      response.setHeader("content-type", contentType(file)); response.end(readFileSync(file));
    } catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { http.once("error", reject); http.listen(0, "127.0.0.1", resolve); });
  const origin = `http://127.0.0.1:${http.address().port}`;
  tab = await browser(process.argv.includes('firefox') ? 'firefox' : 'chromium', join(temp, "profile"), 35000 + Math.floor(Math.random() * 1000));
  await tab.resize(1400, 900); await tab.navigate(`${origin}/docs/paper`);
  await until("initial current source", () => tab.evaluate(`document.querySelector('.cm-content')?.textContent.includes('current purple')`), 10000);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="History"]').click()`);
  await until("checkpoint timeline", () => tab.evaluate(`Boolean(document.querySelector('[data-sha="${points[1].sha}"]'))`), 10000);
  const select = async index => {
    const sha = points[index].sha;
    const position = await tab.evaluate(`(() => {
      const row = document.querySelector('[data-sha="${sha}"] .timeline-point');
      row.scrollIntoView({block:'nearest'});
      const box = row.getBoundingClientRect(), x = box.left + box.width / 2, y = box.top + box.height / 2;
      if (document.elementFromPoint(x,y)?.closest('.timeline-point') !== row) throw new Error('Checkpoint row is obscured');
      return {x,y};
    })()`);
    if (tab.command) {
      await tab.command('Input.dispatchMouseEvent', {type:'mousePressed',...position,button:'left',clickCount:1});
      await tab.command('Input.dispatchMouseEvent', {type:'mouseReleased',...position,button:'left',clickCount:1});
    } else {
      await tab.evaluate(`document.elementFromPoint(${position.x},${position.y}).closest('.timeline-point').click()`);
    }
    await until(`selected ${points[index].label}`, () => tab.evaluate(`document.querySelector('[data-sha="${sha}"] .timeline-point')?.getAttribute('aria-current') === 'true'`), 3000);
  };
  const expectDiff = async (left, right) => {
    await until("expected source comparison", () => tab.evaluate(`(() => {
      const a = document.querySelector('.cm-merge-a .cm-content')?.textContent;
      const b = document.querySelector('.cm-merge-b .cm-content')?.textContent;
      return a === ${JSON.stringify(left)} && b === ${JSON.stringify(right)};
    })()`), 4000);
  };
  await select(1); await expectDiff(sources[1], current);
  await select(0); await expectDiff(sources[0], current);
  await select(2); await expectDiff(sources[2], current);
  await select(0); await expectDiff(sources[0], current);
  console.log("history reader: repeated checkpoint selection updates both source endpoints");
  await tab.evaluate(`document.querySelector('.timeline-now .timeline-point').click()`);
  await until('current source alone', () => tab.evaluate(`document.querySelectorAll('.cm-content').length === 1 && document.querySelector('.cm-content').textContent === ${JSON.stringify(current)}`), 4000);
  await select(1); await expectDiff(sources[1], current);
  console.log('history reader: the current row, and a version against it');
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="Files"]').click()`);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="History"]').click()`);
  await until('reopening history starts at the current source', () => tab.evaluate(`document.querySelectorAll('.cm-content').length === 1 && document.querySelector('.cm-content').textContent === ${JSON.stringify(current)}`), 4000);
  await select(5);
  await expectDiff(sources[5], current);
  console.log('history reader: every version is a row of its own');

  failed.add(points[0].sha);
  await select(0);
  await until('failed checkpoint is visible', () => tab.evaluate(`Boolean(document.querySelector('.history-workspace [role="alert"]'))`), 3000);
  assert.equal(await tab.evaluate(`document.querySelectorAll('.history-workspace .cm-content').length`), 0, 'a failed selection does not leave the previous checkpoint displayed');
  failed.delete(points[0].sha);
  await tab.evaluate(`document.querySelector('.history-workspace [role="alert"] button').click()`);
  await until('retry restores selected source', () => tab.evaluate(`document.querySelector('.history-workspace .cm-content')?.textContent === ${JSON.stringify(sources[0])}`), 3000);

  delayedSha = points[1].sha;
  await select(1);
  await until('delayed checkpoint request', async () => delayed.length > 0, 3000);
  await tab.evaluate(`document.querySelector('iframe[title="Document"]')?.contentWindow.eval("parent.postMessage({type:'ready'}, '*')")`);
  assert.equal(await tab.evaluate(`document.querySelectorAll('.history-workspace .cm-content').length`), 0, 'loading a checkpoint clears the old source');
  await select(2);
  delayedSha = ''; delayed.splice(0).forEach(release => release());
  await until('latest click wins', () => tab.evaluate(`document.querySelector('.history-workspace .cm-content')?.textContent === ${JSON.stringify(sources[2])}`), 3000);

  delayedSha = points[1].sha;
  await select(1);
  await until('request before leaving history', async () => delayed.length > 0, 3000);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="Files"]').click()`);
  delayedSha = ''; delayed.splice(0).forEach(release => release());
  await until('live editor after leaving history', () => tab.evaluate(`!document.querySelector('.history-workspace') && document.querySelector('.cm-content')?.textContent === ${JSON.stringify(current)}`), 3000);
  console.log('history reader: loading, errors, retry, out-of-order responses, and leaving history');

  await tab.navigate(`${origin}/docs/paper?at=${points[1].sha}&slowjoin=1`);
  await until('checkpoint URL loads source diff', () => tab.evaluate(`document.querySelector('.cm-merge-a .cm-content')?.textContent === ${JSON.stringify(sources[1])}`), 10000);
  await expectDiff(sources[1], current);
  assert.equal(await tab.evaluate(`document.querySelector('.reader')?.classList.contains('no-preview')`), true, 'history hides the preview');
  assert.equal(await tab.evaluate(`window.historyLive.source()`), current, 'a comparison never modifies the live source');
  assert.match(await tab.evaluate(`document.querySelector('[aria-label="History file"]').selectedOptions[0].textContent`), /main\.html · changed/, 'the file selector says what changed');
  const addition = '<p>live update</p>';
  await tab.evaluate(`window.historyLive.append(${JSON.stringify(addition)})`);
  assert.equal(await tab.evaluate(`document.querySelector('.cm-merge-b .cm-content')?.textContent`), current, 'the comparison is a stable snapshot of the current source');
  await tab.evaluate(`([...document.querySelectorAll('.history-workspace button')].find(button=>button.textContent==='Refresh comparison')).click()`);
  await expectDiff(sources[1], current + addition);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="Files"]').click()`);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="History"]').click()`);
  await tab.navigate(`${origin}/docs/paper?slowjoin=1`);
  await until('remembered history opens current source after connecting', () => tab.evaluate(`document.querySelector('.history-workspace .cm-content')?.textContent === ${JSON.stringify(current + addition)}`), 10000);
  console.log('history reader: paged history, checkpoint URLs, and preview isolation passed');
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
} catch (error) {
  if (tab) console.error(await tab.evaluate(`JSON.stringify({errors:window.testErrors,selected:[...document.querySelectorAll('.timeline-here')].map(node=>node.textContent),source:[...document.querySelectorAll('.cm-content')].map(node=>node.textContent)})`));
  throw error;
} finally {
  await tab?.close(); http?.close(); rmSync(temp, { recursive:true, force:true });
}
