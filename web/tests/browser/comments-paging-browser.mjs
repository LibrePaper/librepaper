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

// What the comments panel says and offers when it is holding a prefix of the
// document's comments rather than all of them.
//
// The four states are not the same thing and the panel has to distinguish
// them: still arriving, arrived and empty, holding a prefix, holding the
// lot. The counts come from the server (`page.total`, `page.open`), never
// from the rows on screen, so a reader holding fifty of four hundred is told
// four hundred; a failed page keeps every card exactly where it is and
// offers the request again; and a long thread arrives with a preview and a
// button rather than with ten thousand replies.
const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-comments-paging-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Comments from ${JSON.stringify(join(root, "src/components/Comments.svelte"))};
window.asked=[];
const row = (id, body) => ({id,seq:0,motivation:'commenting',presentation:{rendered_exact:body},body,deletable:true,replies:[],reply_total:0});
let component=null;
window.show=(comments,page)=>{
  if (component) unmount(component);
  component=mount(Comments,{target:document.body,props:{
    comments,page,
    onloadmore:()=>window.asked.push("more"),
    onloadreplies:(c)=>window.asked.push("replies:"+c.id),
  }});
};
window.row=row;
window.meta=()=>document.querySelector(".panel-status,.panel-meta")?.textContent?.trim()||"";
window.panel=()=>document.querySelector(".comments-panel").innerText;
window.pressLoadMore=()=>{const b=document.querySelector(".load-more");if(!b)return false;b.click();return true;};
window.pressMoreReplies=()=>{const b=document.querySelector(".more-replies");if(!b)return false;b.click();return true;};
window.pressRetry=()=>{const b=Array.from(document.querySelectorAll(".page-error button")).at(0);if(!b)return false;b.click();return true;};
window.cards=()=>Array.from(document.querySelectorAll(".comments-list article")).map(node=>node.id);
`);
const state = (over = {}) => ({
  loading: false, total: 0, open: 0, replies: 0, revision: "r",
  cursor: null, complete: true, pages: 1, error: "", busy: false, ...over,
});
let server, page;
try {
  await build({ configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } } });
  server = createServer((request, response) => {
    const file = request.url === "/panel.js" ? "panel.js" : request.url === "/style.css" ? "librepaper-web.css" : null;
    response.setHeader("content-type", file?.endsWith("js") ? "text/javascript" : file ? "text/css" : "text/html");
    response.end(file ? readFileSync(join(temporary, "build", file)) : '<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/style.css"><style>body{display:flex;height:700px;width:380px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);

  // Still arriving.
  await page.evaluate(`window.show([], ${JSON.stringify(state({ loading: true }))})`);
  await until("loading", () => page.evaluate('window.panel().includes("Loading comments")'), 10000);
  assert.equal(await page.evaluate('window.panel().includes("No comments yet")'), false,
    "loading is not the same as empty");

  // Arrived and empty.
  await page.evaluate(`window.show([], ${JSON.stringify(state())})`);
  await until("empty", () => page.evaluate('window.panel().includes("No comments yet")'), 5000);

  // Holding a prefix: the counts are the document's, the button says how
  // much is missing, and nothing drains it on its own.
  await page.evaluate(
    `window.show([window.row("a","First"), window.row("b","Second")], ${JSON.stringify(
      state({ total: 412, open: 7, complete: false, cursor: "c1" }),
    )})`,
  );
  await until("prefix", () => page.evaluate('window.panel().includes("412 total")'), 5000);
  assert.ok(await page.evaluate('window.panel().includes("7 open")'));
  assert.ok(await page.evaluate('window.panel().includes("2 loaded")'),
    "the panel says how much of the document it is holding");
  assert.ok(await page.evaluate('window.panel().includes("Load more (410 not loaded)")'));
  assert.deepEqual(await page.evaluate("window.asked"), [], "nothing was asked for unprompted");
  assert.ok(await page.evaluate("window.pressLoadMore()"));
  await until("asked", () => page.evaluate('window.asked.includes("more")'), 5000);

  // A failed page: the cards stay, the reason is shown, and the request is
  // offered again.
  await page.evaluate(
    `window.show([window.row("a","First"), window.row("b","Second")], ${JSON.stringify(
      state({ total: 412, open: 7, complete: false, cursor: "c1", error: "the catalogue is unavailable" }),
    )})`,
  );
  await until("error", () => page.evaluate('window.panel().includes("the catalogue is unavailable")'), 5000);
  assert.deepEqual(await page.evaluate("window.cards()"), ["comment-a", "comment-b"],
    "a failed page leaves every visible comment where it was");
  await page.evaluate("window.asked=[]");
  assert.ok(await page.evaluate("window.pressRetry()"));
  await until("retried", () => page.evaluate('window.asked.includes("more")'), 5000);

  // Holding the lot.
  await page.evaluate(
    `window.show([window.row("a","First")], ${JSON.stringify(state({ total: 1, open: 1 }))})`,
  );
  await until("complete", () => page.evaluate('window.panel().includes("All 1 comments loaded")'), 5000);
  assert.equal(await page.evaluate('window.panel().includes("Load more")'), false);

  // A long thread: a preview and a button, never the thread.
  await page.evaluate(`(() => {
    const deep = window.row("deep","A busy thread");
    deep.replies = [{id:"r1",body:"one",creator:"A",created:"2026-01-01T00:00:00Z"}];
    deep.reply_total = 1204;
    window.show([deep], ${JSON.stringify(state({ total: 1, open: 1 }))});
  })()`);
  await until("thread", () => page.evaluate('window.panel().includes("Show 1203 more replies")'), 5000);
  await page.evaluate("window.asked=[]");
  assert.ok(await page.evaluate("window.pressMoreReplies()"));
  await until("replies asked", () => page.evaluate('window.asked.includes("replies:deep")'), 5000);

  console.log("comments-paging-browser: loading, empty, partial and complete states; authoritative counts; load more; a failed page keeping its cards; per-thread reply paging");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
