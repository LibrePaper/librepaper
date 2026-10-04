import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registryText = await read("registry.js");
const { offered, search, CATEGORIES } = await import("../../src/components/settings/registry.js");
const build = await read("BuildSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
const latexFiles = await read("LatexFilesSettings.svelte");
const command = await read("ToolCommand.svelte");
const tools = await read("ToolsSettings.svelte");
const renderingComponent = await read("RenderingSettings.svelte");
const remote = await read("RemoteSettings.svelte");
const statusPill = await read("StatusPill.svelte");

// Rendering, Tools, Agents, Diagnostics, Backups and Account remain navigable
// for every document format and editing state.
for (const format of ["latex", "typst", "markdown", "quarto", "html", ""]) {
  for (const mayEdit of [true, false]) {
    const ids = offered({ format, mayEdit, signedIn: false }).map((item) => item.id);
    for (const id of ["rendering", "tools", "agents", "diagnostics", "backups", "account"]) {
      assert.ok(ids.includes(id), `${format} (mayEdit=${mayEdit}) offers ${id}`);
    }
  }
}
for (const query of ["windows setup", "zotero", "server address", "remote connection", "disconnected sync", "companion address"]) {
  assert.ok(search(query, { format: "html", mayEdit: false, signedIn: false })?.length, `settings search finds ${query}`);
}

const rendering = CATEGORIES.find((category) => category.id === "rendering");
const renderingRows = (context) => rendering.entries.filter((entry) => !entry.offered || entry.offered(context)).map((entry) => entry.id);
for (const format of ["latex", "typst", "markdown", "quarto", "html"]) {
  const rows = renderingRows({ format, mayEdit: false });
  assert.ok(rows.includes("render-latex-engine"), `${format} keeps LaTeX engine selection visible`);
  assert.ok(rows.includes("render-latex-files"), `${format} keeps browser-wide LaTeX cache controls visible`);
  assert.ok(rows.includes("render-markdown-tool"), `${format} keeps Markdown renderer selection visible`);
  assert.ok(rows.includes("rendering-profile") && rows.includes("rendering-parameters"), `${format} keeps Quarto profile and parameters visible`);
}

// The rendering page shows sections for LaTeX and Markdown/Quarto.
for (const name of ["LaTeX", "Markdown and Quarto"]) {
  assert.ok(dialog.includes(`<h4 class="settings-subhead">${name}</h4>`), `Rendering always includes ${name}`);
}
assert.match(dialog, /<RenderingSettings \{userId\} \{onquartooptions\} \/>/);
assert.doesNotMatch(dialog, /h4 class="settings-group-title">/);
assert.doesNotMatch(dialog, /quartoOptionsRelevant/);
assert.match(renderingComponent, /disabled=\{controlsDisabled\}/);
assert.doesNotMatch(dialog, /quartoExecutionRelevant/);
assert.match(latexFiles, /id="render-latex-files"/);
assert.doesNotMatch(dialog, /render-folder/);

// Build settings are global and do not probe on mount. Selecting a local tool
// does probe. LaTeX engine and Markdown tool
// selections both present the correct options.
assert.doesNotMatch(build, /\$effect\([^)]*probe/);
assert.match(build, /void localBridge\.probe\(\{ force: true \}\)/);
assert.match(build, /id: "render-latex-engine"/);
assert.deepEqual([...build.matchAll(/value: "(\w+)", says/g)].map((match) => match[1]), [
  "automatic", "pdflatex", "xelatex",
  "browser", "pandoc", "quarto",
]);
assert.doesNotMatch(build, /lualatex/i);
for (const format of ["latex", "markdown"]) assert.match(dialog, new RegExp(`<BuildSettings format="${format}"`));
// Quarto and Calepin run the document, so whether they do is asked per
// document in the View menu, never chosen once for every document here.
for (const format of ["typst", "quarto"]) assert.doesNotMatch(dialog, new RegExp(`<BuildSettings format="${format}"`));

// A tool's command fields need the companion and fresh settings before edits
// apply, and responses are scoped to the connection. Only Quarto and Calepin
// have commands; Zotero is listed with the other programs.
assert.match(command, /const canEdit = \$derived\(isConnected && settingsLoaded && !pendingDialogAction\);/);
assert.match(command, /requestId !== loadId[\s\S]{0,260}local\?\.state !== "connected"/);
assert.match(tools, /const CONFIGURABLE = \["quarto", "calepin"\];/);
assert.match(tools, /"Zotero"/);
// Rendering says nothing about the companion; the programs live under Tools.
assert.doesNotMatch(dialog, /ConnectionRow|ToolCommand/);

// The account page calls the server Remote connection and shows its state as
// a compact, accessible pill beside the server address.
assert.match(registryText, /id: "remote-status", says: "Remote connection"/);
assert.match(remote, /<SettingRow id="remote-status" title="Remote connection"/);
assert.match(remote, /StatusPill/);
assert.match(statusPill, /tone = "neutral"/);
assert.match(statusPill, /role="status" aria-label=\{accessibleLabel \|\| label\}/);
assert.match(statusPill, /aria-hidden="true"/);

console.log("settings-quarto: Rendering page shows LaTeX and Markdown/Quarto sections; format-specific controls stay available");
