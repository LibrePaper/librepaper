// The Git conflict workspace over a real resolver: the side by side view
// highlights what differs, an arrow copies the repository's lines into the
// Result, a whole-file choice replaces the Result, and Apply waits until
// every file has a choice. sync.js is replaced, so nothing here talks to Git.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-git-resolver-browser-"));
const output = join(temporary, "build");
const imp = (path) => JSON.stringify(join(root, path));
const entry = join(temporary, "entry.js");
const sync = join(temporary, "sync.js");

writeFileSync(sync, `
export const calls = { saved: [], applied: [] };
window.syncCalls = calls;
export async function saveResolutions(ctx, session, resolutions) {
  calls.saved.push(JSON.parse(JSON.stringify(resolutions)));
  return { ...session, resolutions };
}
export async function applyResolution(ctx, session, resolutions) {
  calls.applied.push(JSON.parse(JSON.stringify(resolutions)));
  return { status: "pulled", files: Object.keys(resolutions) };
}
`);

writeFileSync(entry, `
import { tick } from ${imp("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${imp("node_modules/svelte/src/legacy/legacy-client.js")};
import GitConflictWorkspace from ${imp("src/components/reader/GitConflictWorkspace.svelte")};
import { createGitResolver } from ${imp("src/lib/reader/git-resolver.svelte.js")};
window.testErrors=[];addEventListener("error",e=>window.testErrors.push(e.message));addEventListener("unhandledrejection",e=>window.testErrors.push(String(e.reason)));
const encode = (text) => new TextEncoder().encode(text);
const base = "# Title\\n\\nThe first paragraph.\\n\\nThe second paragraph.\\n";
const blobs = new Map([
  ["base", encode(base)],
  ["ours", encode(base.replace("first paragraph", "first paragraph, edited here"))],
  ["theirs", encode(base.replace("first paragraph", "first paragraph, edited in Git"))],
  ["png-ours", new Uint8Array([137, 80, 78, 71, 0, 1])],
  ["png-theirs", new Uint8Array([137, 80, 78, 71, 0, 2])],
]);
const ctx = { git: { blobs: async (ids) => new Map(ids.map((id) => [id, blobs.get(id)])) } };
const session = { id: "s1", conflicts: [
  { path: "paper.md", kind: "text", base: "base", ours: "ours", theirs: "theirs" },
  { path: "figure.png", kind: "asset", base: null, ours: "png-ours", theirs: "png-theirs" },
] };
const resolver = createGitResolver();
window.results = [];
createClassComponent({ component: GitConflictWorkspace, target: document.getElementById("workspace"), props: { resolver } });
window.openResolver = async () => { await resolver.open(ctx, session, (result) => window.results.push(result)); await tick(); };
window.state = () => JSON.parse(JSON.stringify(resolver.state));
window.resultDoc = () => document.querySelector('.cm-mergeView [aria-label="Result"]')?.textContent ?? null;
window.changedText = () => [...document.querySelectorAll(".cm-mergeView .cm-changedText")].map((node) => node.textContent);
window.button = (label) => [...document.querySelectorAll(".git-conflicts button")].find((node) => node.textContent.trim() === label);
window.click = (label) => window.button(label).click();
window.revert = () => document.querySelector(".cm-merge-revert button")?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
window.selectFile = (path) => { const select = document.querySelector(".git-conflicts select"); select.value = path; select.dispatchEvent(new Event("change", { bubbles: true })); };
window.ready = true;
`);

let server;
let page;
try {
  await build({ configFile: false, root, plugins: [svelte()], logLevel: "error",
    resolve: { alias: [{ find: /^\.\.\/gitsync\/sync\.js$/, replacement: sync }] },
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "git-resolver-check.js", cssFileName: "style" } } });
  // The built shell's stylesheet carries the theme the buttons and panes use.
  const built = join(root, "dist/assets");
  const theme = existsSync(built) ? readdirSync(built)
    .filter((name) => name.endsWith(".css"))
    .map((name) => readFileSync(join(built, name), "utf8"))
    .filter((text) => text.includes("--color-sidebar"))
    .join("\n") : "";
  server = createServer((request, response) => {
    if (request.url === "/theme.css") {
      response.setHeader("Content-Type", "text/css");
      return response.end(theme);
    }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", request.url.endsWith(".css") ? "text/css" : "text/javascript");
      return response.end(readFileSync(file));
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><html data-theme="librepaper"><head><meta charset="utf-8">'
      + '<link rel="stylesheet" href="/theme.css"><link rel="stylesheet" href="/style.css">'
      + '<style>body{margin:0}#workspace{height:100vh;width:100vw}</style></head>'
      + '<body><div id="workspace"></div><script type="module" src="/git-resolver-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.resize(1400, 800);
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("resolver test loaded", () => page.evaluate("window.ready"), 10000);
  const evaluate = (code) => page.evaluate(code);

  // The first file that needs a choice opens side by side, with the words
  // that differ highlighted.
  await evaluate("openResolver()");
  await until("merge view", () => evaluate("resultDoc() !== null"), 10000);
  assert.equal((await evaluate("state()")).selected, "paper.md");
  assert.match(await evaluate("resultDoc()"), /edited here/);
  const changed = await evaluate("changedText()");
  assert.ok(changed.some((text) => /in Git/.test(text)), `highlighted: ${JSON.stringify(changed)}`);
  assert.equal(await evaluate('button("Apply").disabled'), true);

  // The arrow beside a difference copies the repository's lines into the
  // Result, which counts as typed text.
  await evaluate("revert()");
  await until("reverted", () => evaluate("/edited in Git/.test(resultDoc())"), 5000);
  await until("typed resolution", () => evaluate('state().resolutions["paper.md"]?.text !== undefined'), 5000);

  // A whole-file choice replaces the Result.
  await evaluate('click("Use LibrePaper\'s")');
  await until("ours restored", () => evaluate("/edited here/.test(resultDoc())"), 5000);
  assert.deepEqual((await evaluate("state()")).resolutions["paper.md"], { keep: "ours" });

  // A binary file has no comparison, only a choice of side.
  await evaluate('selectFile("figure.png")');
  await until("binary note", () => evaluate('document.querySelector(".git-conflicts-body").textContent.includes("not text")'), 5000);
  await evaluate('click("Use the repository\'s")');

  // With every file chosen, Apply hands the choices over and closes.
  await until("apply enabled", () => evaluate('button("Apply").disabled === false'), 5000);
  await evaluate('click("Apply")');
  await until("applied", () => evaluate("window.results.length === 1"), 5000);
  const applied = await evaluate("window.syncCalls.applied[0]");
  assert.deepEqual(applied, { "paper.md": { keep: "ours" }, "figure.png": { keep: "theirs" } });
  assert.equal((await evaluate("state()")).active, false);
  assert.deepEqual(await evaluate("window.testErrors"), []);
  console.log("git resolver browser ok");
} finally {
  await page?.close?.();
  server?.close();
  removeTemporary(temporary);
}
