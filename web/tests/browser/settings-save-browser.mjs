// Regression check for Quarto settings saves: an options echo is part of a
// successful save, while a document scope change cancels its late feedback.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-settings-save-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

const source = `
import RenderingSettings from ${JSON.stringify(join(root, "web/src/components/settings/RenderingSettings.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
let resolveSave, component;
let mode = "echo";
const initial = { profile: "", parameters: {} };
component = createClassComponent({ component: RenderingSettings, target: document.body, props: {
  options: initial, scopeKey: "doc-a:quarto",
  onapplyoptions: (next) => {
    if (mode === "failure") return Promise.reject(new Error("Save failed"));
    if (mode === "scope") return new Promise((resolve) => { resolveSave = resolve; });
    component.$set({ options: next });
    return new Promise((resolve) => { resolveSave = resolve; });
  },
} });
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 35)); await tick(); };
window.settingsSaveCheck = async () => {
  const text = (node) => node?.textContent.replace(/\\s+/g, " ").trim() ?? "";
  const field = () => document.querySelector('[aria-label="Quarto parameters"]');
  const save = () => document.querySelector("#rendering-save");
  const edit = (value) => { field().value = value; field().dispatchEvent(new Event("input", { bubbles: true })); };
  const clickSave = async () => { await flush(); save().click(); await flush(); };

  edit('{"first": 1}'); await clickSave();
  if (text(save()) !== "Saving…") throw new Error("save did not enter Saving state");
  await flush(); // let the parent echo its new options before resolving
  resolveSave(); await flush();
  if (!document.querySelector('.setting-feedback[role="status"]') || !text(document.querySelector('.setting-feedback')).includes("Saved"))
    throw new Error("successful echoed options did not show Saved");
  if (field().disabled || text(save()) === "Saving…") throw new Error("successful save left controls pending");
  edit('{"first": 2}'); await flush();
  if (save().disabled) throw new Error("editing after save did not enable Save");
  if (document.querySelector('.setting-feedback[role="status"]')) throw new Error("editing did not clear Saved");

  mode = "failure"; await clickSave(); await flush();
  if (!document.querySelector('[role="alert"]') || document.querySelector('.setting-feedback[role="status"]'))
    throw new Error("failed save did not show only an error");

  mode = "scope"; edit('{"first": 3}'); await clickSave();
  component.$set({ scopeKey: "doc-b:quarto", options: initial }); await flush();
  resolveSave(); await flush();
  if (document.querySelector('.setting-feedback[role="status"]')) throw new Error("late save showed success in a new scope");
  if (!save().disabled) throw new Error("scope change did not discard the old draft");
  component.$destroy();
  return true;
};
`;

async function freePort() {
  const probe = createServer();
  await new Promise((resolve, reject) => { probe.once("error", reject); probe.listen(0, "127.0.0.1", resolve); });
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "settings-save-check.js" } } });
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", request.url.endsWith(".css") ? "text/css" : "text/javascript");
      response.end(readFileSync(file)); return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><html><head><link rel="stylesheet" href="/style.css"></head>'
      + '<body><script type="module" src="/settings-save-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, await freePort());
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("settings save component", () => tab.evaluate("Boolean(window.settingsSaveCheck)"));
  assert.equal(await tab.evaluate("window.settingsSaveCheck()"), true);
  console.log("settings save: echo, failure, edit reset and scope cancellation");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
