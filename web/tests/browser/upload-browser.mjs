// Browser acceptance for making a project on the landing page. A project is a
// name and a format here and nothing else; the files it holds arrive later, in
// its own explorer, which `files-browser` checks.
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

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-upload-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
let server, tab;
try {
  const config = { max_document: 4 * 1024 * 1024, max_assets: 32 * 1024 * 1024, max_asset: 16 * 1024 * 1024,
    max_files: 200, max_path: 200, extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex"],
    text_extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex", ".bib", ".csl", ".yml", ".yaml"],
    asset_extensions: [".png", ".jpg", ".jpeg", ".gif", ".svg", ".pdf"] , derived_extensions: [] };
  const source = `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    // Creating a project leaves for it, so what the request carried and what
    // went wrong have to outlive the navigation that follows.
    const kept = (key, value) => sessionStorage.setItem(key, JSON.stringify(value));
    const held = (key) => JSON.parse(sessionStorage.getItem(key) || '[]');
    window.uploadEntries = [];
    kept('uploads', []);
    if (!sessionStorage.getItem('errors')) kept('errors', []);
    const blame = (message) => kept('errors', [...held('errors'), message]);
    addEventListener('error', event => blame(event.message));
    addEventListener('unhandledrejection', event => blame(String(event.reason)));
    const config = ${JSON.stringify(config)};
    const nativeFetch = fetch.bind(globalThis);
    globalThis.fetch = async (url, init = {}) => {
      const path = String(url);
      if (path === '/api/config') return new Response(JSON.stringify(config), { headers: {'content-type':'application/json'} });
      if (path === '/api/me') return new Response(JSON.stringify({name:'Tester', can_publish:true, providers:[]}));
      if (path === '/api/list') return new Response(JSON.stringify({documents:[]}));
      if (path === '/api/documents' && init.method === 'POST') {
        for (const [name, value] of init.body.entries()) window.uploadEntries.push({name, filename:value.name, bytes:value instanceof Blob ? await value.text() : value});
        kept('uploads', window.uploadEntries);
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
    // Where a created project opens. Nothing is mounted here; the page exists
    // so the navigation lands somewhere and what it carried can be read back.
    if (path.startsWith("/docs/")) { response.setHeader("content-type", "text/html"); response.end("<!doctype html><title>project</title>"); return; }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.slice(1)))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  tab = await browser("chromium", join(temp, "profile"), 25000 + Math.floor(Math.random() * 1000));
  await tab.resize(1100, 800);
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  const dialog = async () => {
    await until("new project button", () => tab.evaluate("Boolean([...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'New project'))"));
    await tab.evaluate("[...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'New project').click()");
    await until("naming dialog", () => tab.evaluate("Boolean(document.querySelector('#new-project input'))"));
  };
  const type = (value) => tab.evaluate(`(() => { const input = document.querySelector('#new-project input'); input.value = ${JSON.stringify(value)}; input.dispatchEvent(new Event('input', {bubbles:true})); })()`);
  const pick = (format) => tab.evaluate(`(() => { const select = document.querySelector('#new-project select'); select.value = ${JSON.stringify(format)}; select.dispatchEvent(new Event('change', {bubbles:true})); })()`);
  const create = () => tab.evaluate("document.querySelector('#new-project').requestSubmit()");
  const sent = async () => {
    await until("the project opens", () => tab.evaluate("location.pathname === '/docs/uploaded'"));
    return tab.evaluate("JSON.parse(sessionStorage.getItem('uploads'))");
  };
  const wentWrong = () => tab.evaluate("JSON.parse(sessionStorage.getItem('errors') || '[]')");

  // Nothing on the landing page takes a file any more: a project is made, then
  // filled in its own explorer.
  await dialog();
  assert.equal(await tab.evaluate("Boolean(document.querySelector('input[type=file]'))"), false);

  // A name and a format make the project: one file, named by the format, with
  // the name written into it as its title.
  await type("A Paper You Can Change");
  await pick("quarto");
  await create();
  const made = await sent();
  assert.deepEqual(made.map((entry) => entry.name), ["file", "title"]);
  assert.equal(made[0].filename, "main.qmd");
  assert.equal(made[1].bytes, "A Paper You Can Change");
  assert.match(made[0].bytes, /title: "A Paper You Can Change"/);
  assert.deepEqual(await wentWrong(), []);

  // The format is the whole of what decides the main file's name.
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await dialog();
  await type("Thesis");
  await pick("latex");
  await create();
  assert.equal((await sent())[0].filename, "main.tex");

  // An unnamed project is refused in the dialog, and nothing is sent.
  await tab.navigate(`http://127.0.0.1:${server.address().port}/`);
  await dialog();
  await type("   ");
  await create();
  await until("refusal shown", () => tab.evaluate("Boolean([...document.querySelectorAll('#new-project p')].find(p => p.textContent.includes('Give the project a name')))"));
  assert.equal(await tab.evaluate("window.uploadEntries.length"), 0, "nothing is sent for a project with no name");
  assert.equal(await tab.evaluate("location.pathname"), "/", "a refused creation stays on the list");
  assert.equal(await tab.evaluate("Boolean(document.querySelector('#new-project input'))"), true, "the dialog stays open on the name it refused");
  assert.deepEqual(await wentWrong(), []);

  console.log("upload-browser: naming, format, the main file it implies, and an empty name refused passed");
} finally {
  await tab?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  rmSync(temp, {recursive:true, force:true});
}
