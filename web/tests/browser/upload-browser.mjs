// Browser acceptance for the New project page: its three tabs (Blank, From
// Template, Upload Files), the name they share, and what each one sends. Blank
// and template projects go through fetch, mocked in the page; the Upload tab
// uses XMLHttpRequest, so a node server reads that multipart body for real.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-upload-browser-"));
const entry = join(temp, "entry.js"), out = join(temp, "build");
let server, tab;
try {
  const config = { log_quota_bytes: 4 * 1024 * 1024, storage: { per_owner: 32 * 1024 * 1024 },
    max_files: 200, max_path: 200, extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex"],
    text_extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex", ".bib", ".csl", ".yml", ".yaml"],
    asset_extensions: [".png", ".jpg", ".jpeg", ".gif", ".svg", ".pdf"] , derived_extensions: [] };
  const source = `
    import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
    import Landing from ${JSON.stringify(join(root, "web/src/components/Landing.svelte"))};
    import { zip } from ${JSON.stringify(join(root, "web/src/lib/zip.js"))};
    import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
    window.makeZip = zip;
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
      const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: {'content-type':'application/json'} });
      if (path === '/api/config') return json(config);
      if (path === '/api/me') return json({name:'Tester', can_publish:true, providers:[]});
      if (path === '/api/list') return json({documents:[{ slug: 'x', title: 'Untitled project', updated_at: '2026-09-01T10:00:00Z', role: 'owner' }]});
      if (path.startsWith('/api/templates')) return json({ templates: [{ slug: 'mine', title: 'Lab report', source_format: 'markdown', url: '/docs/mine' }] });
      if (path === '/api/documents' && init.method === 'POST') {
        for (const [name, value] of init.body.entries()) window.uploadEntries.push({name, filename:value.name, bytes:value instanceof Blob ? await value.text() : value});
        kept('uploads', window.uploadEntries);
        return json({url:'/docs/uploaded'}, 201);
      }
      if (path.startsWith('/api/documents/')) return json({comment_count:0,files:[]});
      return nativeFetch(url, init);
    };
    mount(Landing, {target: document.body});
  `;
  writeFileSync(entry, source);
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss(), svelte()], build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } }, logLevel: "error" });

  // What the Upload tab posted, as the node server read it.
  const posted = [];
  server = createServer(async (request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    if (request.method === "POST" && path === "/api/documents") {
      try {
        const form = await new Request("http://x" + request.url, { method: "POST", headers: request.headers, body: request, duplex: "half" }).formData();
        const entries = [];
        for (const [name, value] of form.entries()) {
          entries.push(typeof value === "string" ? { name, filename: undefined, bytes: value } : { name, filename: value.name, bytes: await value.text() });
        }
        posted.push(entries);
        const refused = form.get("title") === "Refuse me";
        response.statusCode = refused ? 400 : 201;
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(refused ? { error: "that name is taken" } : { url: "/docs/uploaded" }));
      } catch (failure) {
        response.statusCode = 500;
        response.end(String(failure));
      }
      return;
    }
    if (path === "/" ) { response.setHeader("content-type", "text/html"); response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/check.css"></head><body><script type="module" src="/check.js"></script></body></html>'); return; }
    // Where a created project opens. Nothing is mounted here; the page exists
    // so the navigation lands somewhere and what it carried can be read back.
    if (path.startsWith("/docs/")) { response.setHeader("content-type", "text/html"); response.end("<!doctype html><title>project</title>"); return; }
    try { response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(join(out, path.slice(1)))); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}/`;
  tab = await browser("chromium", join(temp, "profile"), 25000 + Math.floor(Math.random() * 1000));
  await tab.resize(1100, 800);

  // The templates as the page should list them, read from the same directories.
  const FORMAT_IDS = ["markdown", "quarto", "latex", "typst", "html"];
  const templateDir = join(root, "web/src/templates");
  const builtIn = readdirSync(templateDir).map((id) => ({
    id,
    name: JSON.parse(readFileSync(join(templateDir, id, "template.json"), "utf8")).name,
    formats: FORMAT_IDS.filter((format) => existsSync(join(templateDir, id, format))),
  }));
  const catalog = [...builtIn, { id: "mine", name: "Lab report", formats: ["markdown"] }]
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.id.localeCompare(b.id));

  const q = (expression) => tab.evaluate(expression);
  const exists = (selector) => q(`Boolean(document.querySelector(${JSON.stringify(selector)}))`);
  const textOf = (selector) => q(`document.querySelector(${JSON.stringify(selector)})?.textContent.trim() ?? null`);
  const valueOf = (selector) => q(`document.querySelector(${JSON.stringify(selector)})?.value ?? null`);
  const click = (selector) => q(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const clickText = (label) => q(`[...document.querySelectorAll('button')].find(button => button.textContent.trim() === ${JSON.stringify(label)}).click()`);
  const set = (selector, value, type = "input") => q(`(() => { const field = document.querySelector(${JSON.stringify(selector)}); field.value = ${JSON.stringify(value)}; field.dispatchEvent(new Event(${JSON.stringify(type)}, {bubbles:true})); })()`);
  const submit = () => q("document.querySelector('#new-project-form').requestSubmit()");
  const rows = () => q("[...document.querySelectorAll('tr[data-template]')].map(row => row.dataset.template)");
  const names = () => q("[...document.querySelectorAll('tr[data-template] .col-name')].map(cell => cell.textContent.trim())");
  const wentWrong = () => q("JSON.parse(sessionStorage.getItem('errors') || '[]')");
  const sent = async () => {
    await until("the project opens", () => q("location.pathname === '/docs/uploaded'"));
    return q("JSON.parse(sessionStorage.getItem('uploads'))");
  };
  // A fresh page at the project list, or straight on a tab of the new page.
  const fresh = async (query = "") => {
    await tab.navigate(base + query);
    await until("the page is up", () => exists(query ? "#new-project" : ".nav-new"));
  };
  const openNew = async () => {
    await fresh();
    await click(".nav-new");
    await until("the new project page", () => exists("#new-project"));
  };
  const openTemplates = async () => {
    await fresh("?new=template");
    await until("your template is listed", () => exists('tr[data-template="mine"]'));
  };
  // Stage files as the file picker would: a change event on the hidden input.
  const stageFiles = async (specs) => {
    await q(`(() => {
      const data = new DataTransfer();
      for (const spec of ${JSON.stringify(specs)}) {
        const body = spec.zip
          ? window.makeZip(Object.fromEntries(Object.entries(spec.zip).map(([path, value]) => [path, typeof value === 'string' ? value : new Uint8Array(value)])))
          : spec.text;
        data.items.add(new File([body], spec.name));
      }
      const input = document.querySelector('#upload-files');
      input.files = data.files;
      input.dispatchEvent(new Event('change', {bubbles:true}));
    })()`);
  };
  const tree = () => textOf(".upload-tree");

  // a. The button opens the Blank tab, with a name that is not taken.
  await fresh();
  await click(".nav-new");
  await until("the new project page", () => exists("#new-project"));
  assert.equal(await q("location.search"), "?new=blank");
  assert.deepEqual(await q("[...document.querySelectorAll('#new-project [role=tab]')].map(node => node.textContent.trim())"), ["Blank", "From Template", "Upload Files"]);
  await until("the suggested name", async () => (await valueOf("#new-project-name")) === "Untitled project 2");

  // b. A name alone makes the project: nothing is sent but the title, and the
  // project starts with no files and no format.
  assert.equal(await exists("#new-project-format"), false, "blank asks for no format");
  await set("#new-project-name", "A Paper You Can Change");
  await submit();
  const made = await sent();
  assert.deepEqual(made.map((one) => one.name), ["title"]);
  assert.equal(made[0].bytes, "A Paper You Can Change");
  assert.deepEqual(await wentWrong(), []);

  // c. An unnamed project is refused on the page, and nothing is sent.
  await openNew();
  await set("#new-project-name", "   ");
  await submit();
  await until("refusal shown", async () => (await textOf("p.new-project-error")) === "Give the project a name.");
  assert.equal(await q("window.uploadEntries.length"), 0, "nothing is sent for a project with no name");
  assert.equal(await q("location.pathname"), "/", "a refused creation stays on the page");
  assert.equal(await exists("#new-project-form"), true);
  assert.deepEqual(await wentWrong(), []);

  // d. What was typed survives a visit to another tab.
  await openNew();
  await set("#new-project-name", "My typed name");
  await click("#new-project-tab-template");
  await until("template tab", () => exists("#template-search"));
  await click("#new-project-tab-blank");
  await until("blank tab", () => exists("#new-project-name"));
  assert.equal(await valueOf("#new-project-name"), "My typed name");

  // e. The template tab: listing, search, filters, sort, selection.
  await openTemplates();
  assert.equal(await exists('tr[data-template="blank"]'), true, "the empty document is a template");
  assert.deepEqual(await rows(), catalog.map((one) => one.id));
  await set("#template-search", "letter");
  await until("search narrows", async () => JSON.stringify(await rows()) === '["letter"]');
  assert.equal(await textOf("p.template-count"), "1 template");

  await set("#template-search", "");
  await set("#template-format", "html", "change");
  await until("format filter", async () => (await rows()).length === catalog.filter((one) => one.formats.includes("html")).length);
  assert.deepEqual(await rows(), catalog.filter((one) => one.formats.includes("html")).map((one) => one.id));
  await set("#template-format", "", "change");
  await until("filter cleared", async () => (await rows()).length === catalog.length);

  await set("#template-sort", "name-desc", "change");
  await until("sort reversed", async () => JSON.stringify(await names()) === JSON.stringify(catalog.map((one) => one.name).reverse()));
  await set("#template-sort", "name-asc", "change");
  await until("sort restored", async () => JSON.stringify(await names()) === JSON.stringify(catalog.map((one) => one.name)));

  await click('tr[data-template="article"]');
  await until("article chosen", () => exists("#new-project-format"));
  assert.equal(await q("document.querySelector('tr[data-template=\"article\"] input[type=radio]').checked"), true);
  const article = catalog.find((one) => one.id === "article");
  assert.deepEqual(await q("[...document.querySelectorAll('#new-project-format option')].map(option => option.value)"), article.formats);

  // A search that hides the choice takes the create form with it.
  await set("#template-search", "letter");
  await until("choice dropped", async () => !(await exists("#new-project-form")));
  await set("#template-search", "");
  await until("all listed", async () => (await rows()).length === catalog.length);
  await click('tr[data-template="article"]');
  await until("article chosen again", () => exists("#new-project-form"));
  await set("#new-project-format", "latex", "change");
  await submit();
  const fromTemplate = await sent();
  const templatePaths = fromTemplate.filter((one) => one.name === "file").map((one) => one.filename);
  assert.ok(templatePaths.includes("main.tex") && templatePaths.includes("references.bib"), `article files: ${templatePaths}`);
  assert.deepEqual(await wentWrong(), []);

  // f. One file is a project named for it, sent through the real request.
  await openNew();
  await click("#new-project-tab-upload");
  await until("upload tab", () => exists("#upload-files"));
  posted.length = 0;
  await stageFiles([{ name: "paper.tex", text: "\\documentclass{article}" }]);
  await until("the file is listed", async () => ((await tree()) ?? "").includes("paper.tex"));
  await until("the name follows the file", async () => (await valueOf("#new-project-name")) === "paper");
  await submit();
  await until("the project opens", () => q("location.pathname === '/docs/uploaded'"));
  assert.equal(posted.length, 1);
  const single = posted[0];
  assert.equal(single.find((one) => one.name === "file").filename, "paper.tex");
  assert.equal(single.find((one) => one.name === "file").bytes, "\\documentclass{article}");
  assert.equal(single.find((one) => one.name === "title").bytes, "paper");
  assert.ok(!single.some((one) => one.name === "main"), "an upload names no main file");

  // g. A ZIP with a top folder loses it; the folders inside stay.
  await openNew();
  await click("#new-project-tab-upload");
  await until("upload tab", () => exists("#upload-files"));
  posted.length = 0;
  await stageFiles([{ name: "thesis.zip", zip: {
    "thesis/main.tex": "\\documentclass{book}",
    "thesis/chapters/one.tex": "One",
    "thesis/figures/plot.png": [137, 80, 78, 71, 13, 10, 26, 10],
  } }]);
  await until("the note", async () => (await q("document.body.textContent")).includes("The archive's top folder thesis was removed."));
  await until("the name follows the archive", async () => (await valueOf("#new-project-name")) === "thesis");
  await submit();
  await until("the project opens", () => q("location.pathname === '/docs/uploaded'"));
  const archive = posted[0];
  assert.deepEqual(archive.filter((one) => one.name === "file").map((one) => one.filename).sort(), ["chapters/one.tex", "figures/plot.png", "main.tex"]);
  assert.ok(!archive.some((one) => one.name === "main"), "an upload names no main file");
  assert.equal(archive.find((one) => one.name === "title").bytes, "thesis");

  // h. A second paper.tex is a conflict the person settles.
  await openNew();
  await click("#new-project-tab-upload");
  await until("upload tab", () => exists("#upload-files"));
  await stageFiles([{ name: "paper.tex", text: "one" }]);
  await until("the file is listed", async () => ((await tree()) ?? "").includes("paper.tex"));
  await stageFiles([{ name: "paper.tex", text: "two" }]);
  await until("the conflict", async () => (await q("document.body.textContent")).includes("already staged with the same path: paper.tex"));
  await clickText("Keep both");
  await until("both are listed", async () => ((await tree()) ?? "").includes("paper (2).tex"));

  // i. A refusal from the server is shown, and the staged files stay.
  await openNew();
  await click("#new-project-tab-upload");
  await until("upload tab", () => exists("#upload-files"));
  await stageFiles([{ name: "paper.tex", text: "one" }]);
  await until("the file is listed", async () => ((await tree()) ?? "").includes("paper.tex"));
  await set("#new-project-name", "Refuse me");
  await submit();
  await until("server refusal shown", async () => (await textOf("p.new-project-error")) === "that name is taken");
  assert.equal(await q("location.pathname"), "/");
  assert.ok(((await tree()) ?? "").includes("paper.tex"), "the staged file is still listed");

  // j. Cancel goes back to the project list.
  await openNew();
  await click(".new-project-cancel");
  await until("the list is back", async () => !(await exists("#new-project")) && (await exists(".nav-new")));
  assert.equal(await q("location.search"), "");
  assert.deepEqual(await wentWrong(), []);

  // k. On a phone the template table is a list that fits the screen.
  await tab.navigate(base + "?new=template");
  await tab.resize(390, 844);
  await until("your template is listed", () => exists('tr[data-template="mine"]'));
  await until("the phone layout applies", async () => (await q("innerWidth")) === 390);
  assert.equal(await q("getComputedStyle(document.querySelector('.template-table thead')).display"), "none");
  assert.equal(await q("document.documentElement.scrollWidth <= window.innerWidth"), true, "no sideways scroll");

  console.log("upload-browser: the three tabs, names, templates, uploads, archives, conflicts, refusals, cancel and phone width passed");
} finally {
  await tab?.close();
  await new Promise((resolve) => server ? server.close(resolve) : resolve());
  removeTemporary(temp);
}
