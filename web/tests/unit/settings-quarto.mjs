import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registryText = await read("registry.js");
const { offered, search, CATEGORIES } = await import("../../src/components/settings/registry.js");
const build = await read("BuildSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
const latexFiles = await read("LatexFilesSettings.svelte");
const command = await read("ToolCommand.svelte");
const toolsSettings = await read("ToolsSettings.svelte");
const toolList = await readFile(new URL("../../src/lib/desktop/tools.js", import.meta.url), "utf8");
const renderer = await read("RendererSettings.svelte");
const remote = await read("RemoteSettings.svelte");
const statusPill = await read("StatusPill.svelte");

// Rendering, Tools, Agents, Backups, Account and Diagnostics are offered the
// same way on the landing page and the project page, to a visitor and to a
// signed-in browser alike.
for (const signedIn of [true, false]) {
  const ids = offered({ signedIn }).map((item) => item.id);
  for (const id of ["rendering", "tools", "agents", "backups", "account", "diagnostics"]) {
    assert.ok(ids.includes(id), `signedIn=${signedIn} offers ${id}`);
  }
}
assert.doesNotMatch(registryText, /id: "desktop"/);
for (const query of ["rescan", "zotero", "server address", "remote connection", "disconnected sync"]) {
  assert.ok(search(query, { signedIn: false })?.length, `settings search finds ${query}`);
}

const rendering = CATEGORIES.find((category) => category.id === "rendering");
const renderingRows = (context) => rendering.entries.filter((entry) => !entry.offered || entry.offered(context)).map((entry) => entry.id);
// The rows do not depend on the document's format, so one check covers every format.
const rows = renderingRows({ signedIn: false });
assert.ok(rows.includes("render-latex-engine"), "Rendering keeps LaTeX engine selection visible");
assert.ok(rows.includes("render-latex-files"), "Rendering keeps browser-wide LaTeX cache controls visible");
for (const format of ["latex", "typst", "markdown"]) {
  assert.ok(rows.includes(`render-${format}-where`), `Rendering says where ${format} renders`);
  assert.match(dialog, new RegExp(`<RendererSettings format="${format}"`));
}
assert.match(dialog, /<RendererSettings format="quarto"/);
assert.ok(rows.includes("render-quarto-html"), "Rendering lists the Quarto command");
// The Quarto profile and parameters are part of the command now.
for (const gone of ["rendering-profile", "rendering-parameters", "render-markdown-tool", "tools-quarto", "tools-calepin"]) {
  assert.ok(!registryText.includes(`id: "${gone}"`), `${gone} is gone from the settings map`);
}
assert.doesNotMatch(dialog, /RenderingSettings|onquartooptions/);
for (const format of ["latex", "typst", "markdown", "quarto"]) {
  assert.ok(rows.includes(`render-${format}-preview`), `Rendering keeps the ${format} default preview visible`);
  assert.match(dialog, new RegExp(`<DefaultPreviewSetting format="${format}"`));
}

// The rendering page shows one section per format.
for (const name of ["LaTeX", "Typst", "Markdown", "Quarto"]) {
  assert.ok(dialog.includes(`<h4 class="settings-subhead">${name}</h4>`), `Rendering always includes ${name}`);
}
assert.doesNotMatch(dialog, /h4 class="settings-group-title">/);
assert.doesNotMatch(dialog, /quartoOptionsRelevant/);
assert.doesNotMatch(dialog, /quartoExecutionRelevant/);
assert.match(latexFiles, /id="render-latex-files"/);
assert.doesNotMatch(dialog, /render-folder/);

// Build settings are global. The LaTeX engine is chosen once for every
// document, and where a format renders is a separate row for each format.
assert.doesNotMatch(build, /desktop\/client/);
assert.match(build, /id: "render-latex-engine"/);
assert.deepEqual([...build.matchAll(/value: "(\w+)", says/g)].map((match) => match[1]), [
  "automatic", "pdflatex", "xelatex",
]);
assert.doesNotMatch(build, /lualatex/i);
assert.match(dialog, /<BuildSettings format="latex"/);
for (const format of ["typst", "markdown", "quarto"]) assert.doesNotMatch(dialog, new RegExp(`<BuildSettings format="${format}"`));
// This computer is offered only in the desktop app, and Quarto has no browser
// engine to choose.
assert.match(renderer, /\{#if hasDesktop\}/);
assert.match(renderer, /\{#if format !== "quarto"\}/);
assert.match(renderer, /<option value="browser">Browser<\/option>[\s\S]*?<option value="local">This computer<\/option>/);
assert.match(renderer, /The tool may download packages on its own\./);
assert.match(renderer, /LOCAL_OUTPUTS\[format\]/);

// A render command's fields need the desktop app and fresh settings before
// edits apply, and a response for another output is dropped. The whole
// command is one line, saved by key, and the program is listed with the others.
assert.match(command, /const canEdit = \$derived\(isConnected && record !== null && !pendingDialogAction\);/);
assert.match(command, /requestId !== loadId \|\| requestedKey !== key/);
assert.match(command, /desktopClient\.setRenderer\(requestedKey, \{ command: splitArgs\(editingCommand\)/);
assert.match(command, /desktopClient\.resetRenderer\(requestedKey\)/);
assert.match(command, /Reset to default/);
assert.doesNotMatch(toolsSettings, /ToolCommand|CONFIGURABLE/);
assert.match(toolList, /"Zotero"/);
assert.match(toolList, /capabilities\?\.renderers/);
// Rendering says nothing about the desktop app's connection; the programs live under Tools.
assert.doesNotMatch(dialog, /ConnectionRow/);

// The account page calls the server Remote connection and shows its state as
// a compact, accessible pill beside the server address.
assert.match(registryText, /id: "remote-status", says: "Remote connection"/);
assert.match(remote, /<SettingRow id="remote-status" title="Remote connection"/);
assert.match(remote, /StatusPill/);
assert.match(statusPill, /tone = "neutral"/);
assert.match(statusPill, /role="status" aria-label=\{accessibleLabel \|\| label\}/);
assert.match(statusPill, /aria-hidden="true"/);

console.log("settings-quarto: Rendering page shows one section per format; where it renders and its commands sit in each");
