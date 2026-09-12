import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

// The panel's bulk verbs act only on what the cards would let this caller do
// one at a time: "Resolve all" skips suggestions (their verbs are accept and
// reject) and what is already resolved; "Clear resolved" and "Delete all" hand
// the page one list, so it can confirm once, and only ever name comments the
// caller may delete.
const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-comments-bulk-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Comments from ${JSON.stringify(join(root, "src/components/Comments.svelte"))};
window.resolved=[];window.deleted=[];
const comments=[
  {id:'open',seq:0,motivation:'commenting',exact:'One',body:'Open',deletable:true,replies:[]},
  {id:'theirs',seq:1,motivation:'commenting',exact:'Two',body:'Somebody else, open',deletable:false,replies:[]},
  {id:'done',seq:2,motivation:'commenting',exact:'Three',body:'Resolved',resolved:true,deletable:true,replies:[]},
  {id:'locked',seq:3,motivation:'commenting',exact:'Four',body:'Somebody else, resolved',resolved:true,deletable:false,replies:[]},
  {id:'accepted',seq:4,motivation:'editing',exact:'Five',proposed:'Six',resolved:true,outcome:'accepted',deletable:true,source:{path:'a.md',exact:'Five'},replies:[]},
  {id:'proposed',seq:5,motivation:'editing',exact:'Seven',proposed:'Eight',deletable:true,source:{path:'a.md',exact:'Seven'},replies:[]},
];
const handlers={onresolve:c=>window.resolved.push(c.id),ondeletemany:list=>window.deleted.push(list.map(c=>c.id))};
let component=mount(Comments,{target:document.body,props:{comments,canModerate:false,...handlers}});
window.asModerator=async()=>{await unmount(component);window.deleted=[];component=mount(Comments,{target:document.body,props:{comments,canModerate:true,...handlers}});};
window.click=label=>{const b=Array.from(document.querySelectorAll(".bulk button")).find(b=>b.textContent===label);if(!b)return false;b.click();return true;};
window.labels=()=>Array.from(document.querySelectorAll(".bulk button")).map(b=>b.textContent);
`);
let server, page;
try {
  await build({ configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } } });
  server = createServer((request, response) => {
    const file = request.url === "/panel.js" ? "panel.js" : request.url === "/style.css" ? "librepaper-web.css" : null;
    response.setHeader("content-type", file?.endsWith("js") ? "text/javascript" : file ? "text/css" : "text/html");
    response.end(file ? readFileSync(join(temporary, "build", file)) : '<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/style.css"><style>body{display:flex;height:600px;width:360px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("panel", () => page.evaluate('document.body.innerText.includes("3 open · 6 total")'), 10000);

  // A commenter: no "Delete all", since they cannot delete what is not theirs.
  assert.deepEqual(await page.evaluate("window.labels()"), ["Resolve all", "Clear resolved"]);
  assert.ok(await page.evaluate('window.click("Resolve all")'));
  assert.deepEqual(await page.evaluate("window.resolved"), ["open", "theirs"]);
  assert.ok(await page.evaluate('window.click("Clear resolved")'));
  assert.deepEqual(await page.evaluate("window.deleted"), [["done", "accepted"]]);

  // An owner clears everything, decided suggestions included, in one list.
  await page.evaluate("window.asModerator()");
  await until("moderator panel", () => page.evaluate('window.labels().includes("Delete all")'), 2000);
  assert.ok(await page.evaluate('window.click("Clear resolved")'));
  assert.ok(await page.evaluate('window.click("Delete all")'));
  assert.deepEqual(await page.evaluate("window.deleted"), [
    ["done", "locked", "accepted"],
    ["open", "theirs", "done", "locked", "accepted", "proposed"],
  ]);
  console.log("comments-bulk-browser: resolve all, clear resolved and delete all respect each caller's reach");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
