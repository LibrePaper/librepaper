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

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-assistant-review-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Comments from ${JSON.stringify(join(root, "src/components/Comments.svelte"))};
import Diagnostics from ${JSON.stringify(join(root, "src/components/reader/Diagnostics.svelte"))};
window.rejected=[];
const comments=['one','two'].map((id,seq)=>({id,seq,motivation:'editing',pass:'pass-1',exact:'Original text',proposed:'Better text',body:'Tighten',source:{path:'a.md',exact:'Original text'},replies:[]}));
let component=mount(Comments,{target:document.body,props:{comments,canModerate:true,onrejectconfirmed:async item=>{window.rejected.push(item.id);if(item.id==='one')throw Error('Access changed.');}}});
window.diagnostics=async()=>{
  await unmount(component);
  component=mount(Diagnostics,{target:document.body,props:{diagnostics:[{message:'An error',file:'a.md',line:2,revision:'old-sha',source:'old source'}]}});
};
`);
let server, page;
try {
  await build({ configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } } });
  server = createServer((request, response) => {
    const file = request.url === "/panel.js" ? "panel.js" : request.url === "/style.css" ? "librepaper-web.css" : null;
    response.setHeader("content-type", file?.endsWith("js") ? "text/javascript" : file ? "text/css" : "text/html");
    response.end(file ? readFileSync(join(temporary, "build", file)) : '<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/style.css"><style>body{display:flex;height:500px;width:360px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("pass grouping", () => page.evaluate('document.body.innerText.includes("2 pending")'), 10000);
  await page.evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Review next").click()');
  await until("review focus", () => page.evaluate('Boolean(document.activeElement.closest("#comment-one"))'), 1000);
  await page.evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Reject all").click()');
  await until("partial rejection", () => page.evaluate('document.body.innerText.includes("1 rejected; 1 could not be rejected: Access changed.")'), 1000);
  assert.deepEqual(await page.evaluate("window.rejected"), ["one", "two"]);
  await page.evaluate("window.diagnostics()");
  await until("diagnostic content", () => page.evaluate('document.body.innerText.includes("An error")'), 1000);
  assert.equal(await page.evaluate('document.body.innerText.includes("Fix with assistant")'), false);
  console.log("assistant-review-browser: grouped review, partial rejection and diagnostics without AI shortcuts passed");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
