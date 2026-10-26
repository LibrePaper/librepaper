// Exercise the real Reader, sidebar, history controls and CodeMirror together.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-history-reader-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");
const sources = ["<p>first red draft</p>", "<p>second blue draft</p>", "<p>third green draft</p>",
  "<p>routine fourth</p>", "<p>routine fifth</p>", "<p>routine sixth</p>"];
const current = "<p>current purple draft</p>";
const points = sources.map((text, i) => ({
  sha: `00000000-0000-4000-8000-${String(i + 1).padStart(12, "0")}`,
  at: `2026-09-10T09:0${i}:00Z`, seq: i + 1, by: "Editor", why: i > 2 ? "quiet" : "label",
  label: ["Original", "Blah blah", "Latest label"][i] || "", changed: ["main.html"],
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
    if (message.type === 'doc-open') setTimeout(() => onMessage({type:'doc-state',protocol:'librepaper.room.v3',vector:encode(server.oplogVersion().encode()),durableVector:encode(server.oplogVersion().encode()),updates:[encode(server.export({ mode: "update" }))]}), location.search.includes('slowjoin') ? 350 : 0);
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
  await build({ configFile:false, define: KATEX_DEFINE, root:join(root, "web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {
    name:"history-test-room", enforce:"pre", resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    },
  }], build:{outDir:out,emptyOutDir:true,minify:false,lib:{entry,formats:["es"],fileName:()=>"check.js",cssFileName:"check"}}, logLevel:"error" });
  http = createServer((request, response) => {
    if (serveKatex(request, response)) return;
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/me") return json(response, {name:"Editor",providers:[]});
    if (url.pathname === "/api/documents/paper") return json(response, {
      title:"History test", created_at:"history-test", role:"owner", source_format:"html",
      docs_origin:`http://${request.headers.host}`, can_see_sharing:true, can_moderate:true,
    });
    if (url.pathname === "/api/documents/paper/share") return json(response, {links:[],tokens:[]});
    if (url.pathname === "/api/documents/paper/history") {
      // The manifest wire is `document_labels` now: `label_wire` in
      // crates/librepaper-server/src/server/history.rs names each row
      // `{sha, sequence, at, by, label, reason, tree_sha, frontier,
      // archive_status}` inside a `labels` array, not `checkpoints` (old name). There is
      // no per-row `changed` any more (a manifest is a moment, not a diff;
      // src/lib/history.js's own comment on `fromWire` says so).
      const wire = point => ({
        sha: point.sha, sequence: point.seq, at: point.at, by: point.by,
        label: point.label, reason: point.why, tree_sha: `tree-${point.sha}`,
        frontier: "", archive_status: "none",
      });
      const list = points.map(wire).reverse();
      const page = url.searchParams.has('after') ? {labels:list.slice(-1)} : {labels:list.slice(0, -1),next_cursor:2};
      return json(response, {slug:"paper", ...page});
    }
    if (url.pathname.startsWith("/api/documents/paper/history/")) {
      const point = points.find(point => url.pathname.endsWith(point.sha));
      if (!point || failed.has(point.sha)) return json(response, {error:"Label unavailable"}, 404);
      // The real label endpoint omits parent and seq; ancestry and
      // ordering live in the list, and it names the reason field `reason`,
      // not `why` (history.js normalizes on the way in, not the way out).
      const {parent, seq, changed, why, ...rest} = point;
      const payload = {...rest, reason: why, tree_sha: `tree-${point.sha}`, source_format: "html"};
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
  await until("label timeline", () => tab.evaluate(`Boolean(document.querySelector('[data-sha="${points[1].sha}"]'))`), 10000);
  // The first page holds every version but the oldest. The rest is read when
  // somebody asks for it.
  assert.equal(await tab.evaluate(`Boolean(document.querySelector('[data-sha="${points[0].sha}"]'))`), false, 'older pages wait to be asked for');
  await tab.evaluate(`document.querySelector('.hx-older button').click()`);
  await until("older page", () => tab.evaluate(`Boolean(document.querySelector('[data-sha="${points[0].sha}"]'))`), 10000);
  const row = (sha) => `document.querySelector('[data-sha="${sha}"] button[data-nav]')`;
  const select = async index => {
    const sha = points[index].sha;
    const position = await tab.evaluate(`(() => {
      const row = ${row(sha)};
      row.scrollIntoView({block:'nearest'});
      const box = row.getBoundingClientRect(), x = box.left + box.width / 2, y = box.top + box.height / 2;
      if (document.elementFromPoint(x,y)?.closest('button') !== row) throw new Error('Label row is obscured');
      return {x,y};
    })()`);
    if (tab.command) {
      await tab.command('Input.dispatchMouseEvent', {type:'mousePressed',...position,button:'left',clickCount:1});
      await tab.command('Input.dispatchMouseEvent', {type:'mouseReleased',...position,button:'left',clickCount:1});
    } else {
      await tab.evaluate(`document.elementFromPoint(${position.x},${position.y}).closest('button').click()`);
    }
    await until(`selected ${points[index].label}`, () => tab.evaluate(`${row(sha)}?.getAttribute('aria-current') === 'true'`), 3000);
  };
  const expectDiff = async (left, right) => {
    await until("expected source comparison", () => tab.evaluate(`(() => {
      const a = document.querySelector('.cm-merge-a .cm-content')?.textContent;
      const b = document.querySelector('.cm-merge-b .cm-content')?.textContent;
      return a === ${JSON.stringify(left)} && b === ${JSON.stringify(right)};
    })()`), 4000);
  };
  await select(1); await expectDiff(sources[1], current);
  // The host scrolls; an editor scroller would take the wheel and scroll nothing.
  assert.equal(await tab.evaluate(`getComputedStyle(document.querySelector(".cm-merge-b .cm-scroller")).overflowY`), "visible");
  await select(0); await expectDiff(sources[0], current);
  await select(2); await expectDiff(sources[2], current);
  await select(0); await expectDiff(sources[0], current);
  console.log("history reader: repeated label selection updates both source endpoints");
  await tab.evaluate(`document.querySelector('.hx-current button').click()`);
  await until('current source alone', () => tab.evaluate(`document.querySelectorAll('.cm-content').length === 1 && document.querySelector('.cm-content').textContent === ${JSON.stringify(current)}`), 4000);
  await select(1); await expectDiff(sources[1], current);
  console.log('history reader: the current row, and a version against it');
  // A base: a shift-click on another row, Swap, a shift-click that moves the
  // base, and back to the current source.
  const shiftClick = index => tab.evaluate(`${row(points[index].sha)}.dispatchEvent(new MouseEvent("click", {bubbles: true, shiftKey: true}))`);
  await shiftClick(0);
  await expectDiff(sources[0], sources[1]);
  assert.match(await tab.evaluate(`document.querySelector(".history-source-ends")?.textContent || ""`), /\u2192/);
  await tab.evaluate(`[...document.querySelectorAll(".history-source-bar button")].find(b => b.textContent.trim() === "Swap").click()`);
  await expectDiff(sources[1], sources[0]);
  await shiftClick(2);
  await expectDiff(sources[2], sources[0]);
  assert.equal(await tab.evaluate(`${row(points[2].sha)}.classList.contains("hx-base")`), true);
  await tab.evaluate(`[...document.querySelectorAll(".history-source-bar button")].find(b => b.textContent.trim() === "Compare with current").click()`);
  await expectDiff(sources[0], current);
  console.log("history reader: two versions against each other");
  // The bar names the comparison. Close, and then Escape, end it and give the
  // editor area back as it was.
  assert.match(await tab.evaluate(`document.querySelector('.comparison-bar')?.textContent || ""`), /^Comparing .* with /);
  await tab.evaluate(`[...document.querySelectorAll('.comparison-bar button')].find(b => b.textContent.trim() === 'Close').click()`);
  await until('closing a comparison restores the live source', () => tab.evaluate(`!document.querySelector('.history-workspace') && !document.querySelector('.comparison-bar') && document.querySelectorAll('.cm-content').length === 1 && document.querySelector('.cm-content').textContent === ${JSON.stringify(current)}`), 4000);
  await select(1); await expectDiff(sources[1], current);
  await tab.evaluate(`document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
  await until('Escape closes a comparison', () => tab.evaluate(`!document.querySelector('.comparison-bar') && !document.querySelector('.history-workspace')`), 4000);
  console.log("history reader: close and Escape restore the editor area");
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="Files"]').click()`);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="History"]').click()`);
  // Opening history shows its list and leaves the editor area alone: the live
  // source stays, and no comparison is started until a version is chosen.
  await until('reopening history leaves the live source alone', () => tab.evaluate(`!document.querySelector('.history-workspace') && !document.querySelector('.comparison-bar') && document.querySelectorAll('.cm-content').length === 1 && document.querySelector('.cm-content').textContent === ${JSON.stringify(current)}`), 4000);
  // The last three of these are autosaves in a row, which the panel gathers
  // into one editing session. Opening it is how the rest of them are reached.
  // That grouping is the panel's own and is checked in history-panel-browser.mjs.
  await tab.evaluate(`document.querySelector('[data-type="editing"] button[aria-expanded="false"]')?.click()`);
  await select(5);
  await expectDiff(sources[5], current);
  console.log('history reader: every version is reachable, gathered or not');

  failed.add(points[0].sha);
  await select(0);
  await until('failed label is visible', () => tab.evaluate(`Boolean(document.querySelector('.history-workspace [role="alert"]'))`), 3000);
  assert.equal(await tab.evaluate(`document.querySelectorAll('.history-workspace .cm-content').length`), 0, 'a failed selection does not leave the previous label displayed');
  failed.delete(points[0].sha);
  await tab.evaluate(`document.querySelector('.history-workspace [role="alert"] button').click()`);
  await until('retry restores selected source', () => tab.evaluate(`document.querySelector('.history-workspace .cm-content')?.textContent === ${JSON.stringify(sources[0])}`), 3000);

  delayedSha = points[1].sha;
  await select(1);
  await until('delayed label request', async () => delayed.length > 0, 3000);
  await tab.evaluate(`document.querySelector('iframe[title="Document"]')?.contentWindow.eval("parent.postMessage({type:'ready'}, '*')")`);
  assert.equal(await tab.evaluate(`document.querySelectorAll('.history-workspace .cm-content').length`), 0, 'loading a label clears the old source');
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

  await tab.navigate(`${origin}/docs/paper?slowjoin=1`);
  await until("initial current source after reload", () => tab.evaluate(`document.querySelector('.cm-content')?.textContent.includes('current purple')`), 10000);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="History"]').click()`);
  await until("label timeline after reload", () => tab.evaluate(`Boolean(document.querySelector('[data-sha="${points[1].sha}"]'))`), 10000);
  await select(1);
  await expectDiff(sources[1], current);
  assert.equal(await tab.evaluate(`document.querySelector('.reader')?.classList.contains('no-preview')`), true, 'history hides the preview');
  assert.equal(await tab.evaluate(`window.historyLive.source()`), current, 'a comparison never modifies the live source');
  assert.match(await tab.evaluate(`document.querySelector('[aria-label="History file"]').selectedOptions[0].textContent`), /main\.html · changed/, 'the file selector says what changed');
  const addition = '<p>live update</p>';
  await tab.evaluate(`window.historyLive.append(${JSON.stringify(addition)})`);
  assert.equal(await tab.evaluate(`document.querySelector('.cm-merge-b .cm-content')?.textContent`), current, 'the comparison is a stable snapshot of the current source');
  await tab.evaluate(`([...document.querySelectorAll('.history-workspace button')].find(button=>button.textContent==='Refresh')).click()`);
  await expectDiff(sources[1], current + addition);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="Files"]').click()`);
  await tab.evaluate(`document.querySelector('.sidebar-activity [aria-label="History"]').click()`);
  await tab.navigate(`${origin}/docs/paper?slowjoin=1`);
  // A reload lands on the files and the live source, never back in history.
  await until('reload opens the files and the current source', () => tab.evaluate(`!document.querySelector('.history-workspace') && document.getElementById('sidebar-panel-files')?.hidden === false && document.querySelector('.cm-content')?.textContent === ${JSON.stringify(current + addition)}`), 10000);
  console.log('history reader: paged history and preview isolation passed');
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
} catch (error) {
  if (tab) console.error(await tab.evaluate(`JSON.stringify({errors:window.testErrors,selected:[...document.querySelectorAll('.timeline-here')].map(node=>node.textContent),source:[...document.querySelectorAll('.cm-content')].map(node=>node.textContent)})`));
  throw error;
} finally {
  await tab?.close(); http?.close(); removeTemporary(temp);
}
