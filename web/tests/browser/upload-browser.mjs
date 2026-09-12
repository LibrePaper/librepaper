// Browser acceptance for the landing page's ZIP project upload.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { zip } from "../../src/lib/zip.js";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-upload-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
let server, tab;
try {
  const config = { max_document: 4 * 1024 * 1024, max_assets: 32 * 1024 * 1024, max_asset: 16 * 1024 * 1024,
    max_files: 200, max_path: 200, extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex"],
    text_extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex", ".bib", ".csl", ".yml", ".yaml"],
    asset_extensions: [".png", ".jpg", ".jpeg", ".gif", ".svg", ".pdf"] , derived_extensions: [] };
  const archive = new Uint8Array(await zip({ "project/main.md": "# Main", "project/other.md": "# Other", "project/fig.png": new Uint8Array([1, 2, 3]) }).arrayBuffer());
  const encoded = Buffer.from(archive).toString("base64");
  const source = `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    window.uploadEntries = [];
    window.testErrors = [];
    addEventListener('error', event => window.testErrors.push(event.message));
    addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
    const config = ${JSON.stringify(config)};
    const nativeFetch = fetch.bind(globalThis);
    globalThis.fetch = async (url, init = {}) => {
      const path = String(url);
      if (path === '/api/config') return new Response(JSON.stringify(config), { headers: {'content-type':'application/json'} });
      if (path === '/api/me') return new Response(JSON.stringify({can_publish:true, providers:[]}));
      if (path === '/api/list') return new Response(JSON.stringify({documents:[]}));
      if (path === '/api/documents' && init.method === 'POST') {
        for (const [name, value] of init.body.entries()) window.uploadEntries.push({name, filename:value.name, bytes:value instanceof Blob ? [...new Uint8Array(await value.arrayBuffer())] : value});
        return new Response(JSON.stringify({url:'/docs/uploaded'}), {status:201,headers:{'content-type':'application/json'}});
      }
      if (path.startsWith('/api/documents/')) return new Response(JSON.stringify({comment_count:0,files:[]}));
      return nativeFetch(url, init);
    };
    mount(Landing, {target: document.body});
  `;
  writeFileSync(entry, source);
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss(), svelte()], build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js" } }, logLevel: "error" });
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    if (path === "/") { response.setHeader("content-type", "text/html"); response.end('<!doctype html><script type="module" src="/check.js"></script>'); return; }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.slice(1)))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  tab = await browser("chromium", join(temp, "profile"), 25000 + Math.floor(Math.random() * 1000));
  await tab.resize(1100, 800);
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("landing upload form", () => tab.evaluate("Boolean(document.querySelector('input[type=file]'))"));
  await tab.evaluate(`(() => { const raw = Uint8Array.from(atob(${JSON.stringify(encoded)}), c => c.charCodeAt(0)); const input = document.querySelector('input[type=file]'); const transfer = new DataTransfer(); transfer.items.add(new File([raw], 'project.zip', {type:'application/zip'})); input.files = transfer.files; input.dispatchEvent(new Event('change', {bubbles:true})); })()`);
  await until("archive main selector", () => tab.evaluate("Boolean(document.querySelector('select option[value=\"main.md\"]'))"));
  assert.equal(await tab.evaluate("document.querySelector('select').value"), "main.md");
  await tab.evaluate("document.querySelector('select').value='other.md'; document.querySelector('select').dispatchEvent(new Event('change',{bubbles:true}))");
  await tab.evaluate("document.querySelector('form').requestSubmit()");
  await until("multipart upload", () => tab.evaluate("window.uploadEntries.length === 5"));
  const uploaded = await tab.evaluate("window.uploadEntries");
  assert.deepEqual(uploaded.map((entry) => entry.name), ["file", "file", "file", "main", "title"]);
  assert.equal(uploaded.find((entry) => entry.name === "main").bytes, "other.md");
  assert.deepEqual(uploaded.find((entry) => entry.filename === "fig.png").bytes, [1, 2, 3]);
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);

  // Without an obvious main file, creation waits for an explicit choice.
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("fresh upload form", () => tab.evaluate("Boolean(document.querySelector('input[type=file]'))"));
  const ambiguous = Buffer.from(await zip({ "first.md": "# First", "second.md": "# Second", "refs.bib": "@book{ref,title={Reference}}" }).arrayBuffer()).toString("base64");
  await tab.evaluate(`(() => { const input = document.querySelector('input[type=file]'); const transfer = new DataTransfer(); transfer.items.add(new File([Uint8Array.from(atob(${JSON.stringify(ambiguous)}), c => c.charCodeAt(0))], 'ambiguous.zip')); input.files = transfer.files; input.dispatchEvent(new Event('change', {bubbles:true})); })()`);
  await until("ambiguous selector", () => tab.evaluate("Boolean(document.querySelector('select option[value=\"second.md\"]'))"));
  assert.equal(await tab.evaluate("document.querySelector('select').value"), "");
  assert.equal(await tab.evaluate("document.querySelector('button[type=submit]').disabled"), true);
  assert.equal(await tab.evaluate("Boolean(document.querySelector('select option[value=\"refs.bib\"]'))"), false);
  await tab.evaluate("document.querySelector('select').value='second.md'; document.querySelector('select').dispatchEvent(new Event('change',{bubbles:true}))");
  await tab.evaluate("document.querySelector('form').requestSubmit()");
  await until("chosen main uploaded", () => tab.evaluate("window.uploadEntries.some(entry => entry.name === 'main' && entry.bytes === 'second.md')"));
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);

  // The original single-file upload still sends the file without a main field.
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("single file form", () => tab.evaluate("Boolean(document.querySelector('input[type=file]'))"));
  await tab.evaluate("(()=>{const input=document.querySelector('input[type=file]');const transfer=new DataTransfer();transfer.items.add(new File(['# Single document'],'single.md'));input.files=transfer.files;input.dispatchEvent(new Event('change',{bubbles:true}));})()");
  await until("single file ready", () => tab.evaluate("Boolean(document.querySelector('button[type=submit]') && !document.querySelector('button[type=submit]').disabled)"));
  await tab.evaluate("document.querySelector('form').requestSubmit()");
  await until("single file uploaded", () => tab.evaluate("window.uploadEntries.length === 2"));
  assert.deepEqual(await tab.evaluate("window.uploadEntries.map(entry=>entry.name)"), ["file", "title"]);
  assert.equal(await tab.evaluate("window.uploadEntries[0].filename"), "single.md");
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
  console.log("upload-browser: ZIP selection, ambiguous main, multipart project upload, and single-file regression passed");
} finally {
  await tab?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  rmSync(temp, {recursive:true, force:true});
}
