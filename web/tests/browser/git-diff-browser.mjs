// The Git diff workspace over the real comparison: one file at a time, shown as
// a unified diff, chosen through the differ (select and show) as the Git panel
// does. It also checks the captions for one-sided files, the same-copy notice,
// refresh and errors, and that the workspace fits 1200px and 600px panels.
// sync.js is replaced, so nothing here talks to Git.
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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-git-diff-browser-"));
const output = join(temporary, "build");
const imp = (path) => JSON.stringify(join(root, path));
const entry = join(temporary, "entry.js");
const sync = join(temporary, "sync.js");

// compareRemote answers with the scene the test sets on window.
writeFileSync(sync, `
export async function compareRemote() { return window.diffScene; }
`);

// The blob texts, by id. intro.qmd differs in two places.
const blobs = {
  o1: "---\ntitle: Intro\n---\n\n# Introduction\n\nLibrePaper keeps the draft here.\n\nThe second paragraph is shared.\n",
  t1: "---\ntitle: Introduction\n---\n\n# Introduction\n\nThe remote says this instead.\n\nThe second paragraph is shared.\n",
  o2: "A chapter that only LibrePaper has.\n",
  t3: "@article{smith2020,\n  title = {A remote-only reference},\n  year = {2020}\n}\n",
  b1: "PNG\u0000\u0001binary",
  b2: "GIF\u0000\u0002binary",
};

const scene = {
  files: [
    { path: "intro.qmd", change: "different", ours: "o1", theirs: "t1" },
    { path: "chapters/methods.qmd", change: "local", ours: "o2", theirs: null },
    { path: "refs.bib", change: "remote", ours: null, theirs: "t3" },
    { path: "figs/plot.bin", change: "different", ours: "b1", theirs: "b2" },
  ],
  same: 5,
  unsynced: [{ path: "data/big.csv", reason: "Over the size limit." }],
};

writeFileSync(entry, `
window.__LIBREPAPER_DESKTOP__ = Object.freeze({ base: "lp-local://localhost/", version: "test", zoom: 1 });
import { tick } from ${imp("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${imp("node_modules/svelte/src/legacy/legacy-client.js")};
import GitDiffWorkspace from ${imp("src/components/reader/GitDiffWorkspace.svelte")};
import { createGitDiff } from ${imp("src/lib/reader/git-diff.svelte.js")};
window.testErrors=[];addEventListener("error",e=>window.testErrors.push(e.message));addEventListener("unhandledrejection",e=>window.testErrors.push(String(e.reason)));
window.blobs = ${JSON.stringify(blobs)};
const ctx = { git: { blobs: async (ids) => new Map(ids.map((id) => [id, new TextEncoder().encode(window.blobs[id])])) } };
const meta = { label: "owner/paper", branch: "main" };
const differ = createGitDiff();
window.differ = differ;
createClassComponent({ component: GitDiffWorkspace, target: document.getElementById("panel"), props: { differ } });
window.openDiff = async () => { await differ.open(ctx, meta); await tick(); };
// The Git panel calls these; they are not awaited by the page.
window.pick = (path) => { differ.select(path); };
window.reveal = (path) => { differ.show(ctx, meta, path); };
window.button = (label) => [...document.querySelectorAll(".git-diff button")].find((node) => node.textContent.trim() === label) ?? null;
window.click = (label) => window.button(label).click();
window.text = () => document.querySelector(".git-diff")?.textContent ?? "";
window.merged = () => document.querySelector(".git-merge")?.textContent ?? "";
window.fileName = () => document.querySelector(".git-diff-file")?.textContent ?? "";
window.caption = () => document.querySelector(".git-diff-caption")?.textContent.trim() ?? "";
window.side = () => document.querySelector(".git-diff-side")?.textContent ?? "";
window.overflows = () => { const node = document.querySelector(".git-diff"); return node.scrollWidth > node.clientWidth + 1; };
window.setWidth = (px) => document.documentElement.style.setProperty("--panel-width", px + "px");
window.ready = true;
`);

let server;
let page;
try {
  await build({ configFile: false, root, plugins: [svelte()], logLevel: "error",
    resolve: { alias: [{ find: /^\.\.\/gitsync\/sync\.js$/, replacement: sync }] },
    build: { outDir: output, emptyOutDir: true, target: "es2022", lib: { entry, formats: ["es"], fileName: () => "git-diff-check.js", cssFileName: "style" } } });
  // The built shell's stylesheet carries the theme the workspace uses.
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
      + '<style>body{margin:0;background:var(--color-sidebar);height:100vh}#panel{width:var(--panel-width,1200px);height:100vh}</style></head>'
      + '<body><div id="panel"></div><script type="module" src="/git-diff-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.resize(1400, 800);
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("diff test loaded", () => page.evaluate("window.ready"), 10000);
  const evaluate = (code) => page.evaluate(code);
  const shoot = process.env.LIBREPAPER_SHOTS
    ? async (name) => {
      const { data } = await page.command("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(process.env.LIBREPAPER_SHOTS, `${name}.png`), Buffer.from(data, "base64"));
    }
    : async () => {};

  // The file list is gone: no list, no summary. Opening shows the first
  // different file, intro.qmd, as one unified diff with both texts.
  await evaluate(`window.diffScene = ${JSON.stringify(scene)}`);
  await evaluate("openDiff()");
  await until("merge view", () => evaluate('document.querySelector(".git-merge .cm-editor") !== null'), 10000);
  assert.equal(await evaluate('document.querySelector(".git-diff-files") === null'), true, "no file list");
  assert.equal(await evaluate('document.querySelector(".git-diff-summary") === null'), true, "no summary");
  assert.equal(await evaluate("differ.state.selected"), "intro.qmd");
  assert.equal(await evaluate("fileName()"), "intro.qmd");
  await until("unified legend", () => evaluate('document.querySelector(".git-merge-legend") !== null'), 5000);
  assert.equal(await evaluate('document.querySelectorAll(".git-merge .cm-editor").length'), 1, "one editor");
  assert.equal(await evaluate('document.querySelector(".git-merge .cm-changedLine, .git-merge .cm-deletedChunk") !== null'), true, "changes are marked");
  await until("both texts", () => evaluate('merged().includes("keeps the draft here") && merged().includes("says this instead")'), 5000);

  // A local-only file shows its text and the LibrePaper caption; a remote-only
  // file shows its text and the remote caption.
  await evaluate('pick("chapters/methods.qmd")');
  await until("local caption", () => evaluate('caption() === "Only in LibrePaper."'), 5000);
  assert.match(await evaluate("side()"), /chapter that only LibrePaper has/);
  await evaluate('pick("refs.bib")');
  await until("remote caption", () => evaluate('caption() === "Only on the remote."'), 5000);
  assert.match(await evaluate("side()"), /remote-only reference/);

  // A binary file says so instead of showing a diff.
  await evaluate('pick("figs/plot.bin")');
  await until("binary notice", () => evaluate('text().includes("This file is not text.")'), 5000);

  // A path that is not in the comparison is the same copy on both sides.
  await evaluate('pick("same.qmd")');
  await until("same copy", () => evaluate('text().includes("LibrePaper and the remote have the same copy of this file.")'), 5000);
  assert.equal(await evaluate("fileName()"), "same.qmd");

  // show() on the open comparison only changes the file.
  await evaluate('reveal("chapters/methods.qmd")');
  await until("show local file", () => evaluate('differ.state.selected === "chapters/methods.qmd" && caption() === "Only in LibrePaper."'), 5000);

  // Refresh keeps the file that is shown.
  await evaluate('reveal("refs.bib")');
  await until("show remote file", () => evaluate('differ.state.selected === "refs.bib"'), 5000);
  await evaluate('click("Refresh")');
  await until("refresh keeps file", () => evaluate('!differ.state.loading && differ.state.selected === "refs.bib" && caption() === "Only on the remote."'), 5000);

  // A refused comparison shows its reason.
  await evaluate('window.diffScene = { status: "refused", refusals: [{ reason: "The repository is locked." }] }');
  await evaluate('click("Refresh")');
  await until("refusal shown", () => evaluate('text().includes("The repository is locked.")'), 5000);
  await evaluate(`window.diffScene = ${JSON.stringify(scene)}`);
  await evaluate('click("Refresh")');
  // A refused comparison leaves no file selected, so Refresh picks the first different file again.
  await until("comparison back", () => evaluate('differ.state.selected === "intro.qmd" && document.querySelector(".git-merge-legend") !== null'), 5000);

  // Close ends the comparison; show() opens it again for the file it is given.
  await evaluate('click("Close")');
  assert.equal(await evaluate("differ.state.active"), false);
  await evaluate('reveal("intro.qmd")');
  await until("reopened by show", () => evaluate('differ.state.active && document.querySelector(".git-merge .cm-editor") !== null'), 10000);
  assert.equal(await evaluate("fileName()"), "intro.qmd");

  // The workspace fits 1200px and 600px panels, in light and dark.
  await evaluate("setWidth(1200)");
  assert.equal(await evaluate("overflows()"), false, "no horizontal overflow at 1200px");
  await shoot("diff-1200");
  await evaluate("setWidth(600)");
  assert.equal(await evaluate("overflows()"), false, "no horizontal overflow at 600px");
  await shoot("diff-600");
  await evaluate("setWidth(1200)");
  await evaluate('document.documentElement.dataset.mode = "dark"');
  assert.equal(await evaluate("overflows()"), false, "no horizontal overflow at 1200px in dark");
  await shoot("diff-1200-dark");
  await evaluate('delete document.documentElement.dataset.mode');

  assert.deepEqual(await evaluate("window.testErrors"), []);
  console.log("git diff browser ok");
} finally {
  await page?.close?.();
  server?.close();
  removeTemporary(temporary);
}
