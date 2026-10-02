import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registryText = await read("registry.js");
const { offered, search, CATEGORIES } = await import("../../src/components/settings/registry.js");
const build = await read("BuildSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
const folder = await read("ProjectFolderSetting.svelte");
const latexFiles = await read("LatexFilesSettings.svelte");
const integration = await read("IntegrationSettings.svelte");
const rendering = await read("RenderingSettings.svelte");
const remote = await read("RemoteSettings.svelte");
const statusPill = await read("StatusPill.svelte");

// Render, Integrations, Companion, Backups and Account remain navigable for
// every document format and editing state.
for (const format of ["latex", "typst", "markdown", "quarto", "html", ""]) {
  for (const mayEdit of [true, false]) {
    const ids = offered({ format, mayEdit, signedIn: false }).map((item) => item.id);
    for (const id of ["render", "integrations", "local", "backups", "account"]) {
      assert.ok(ids.includes(id), `${format} (mayEdit=${mayEdit}) offers ${id}`);
    }
  }
}
for (const query of ["windows setup", "zotero", "server address", "remote connection", "disconnected sync", "companion address"]) {
  assert.ok(search(query, { format: "html", mayEdit: false, signedIn: false })?.length, `settings search finds ${query}`);
}

const render = CATEGORIES.find((category) => category.id === "render");
const renderRows = (context) => render.entries.filter((entry) => !entry.offered || entry.offered(context)).map((entry) => entry.id);
for (const format of ["latex", "typst", "markdown", "quarto", "html"]) {
  const rows = renderRows({ format, mayEdit: false });
  assert.ok(rows.includes("render-latex-files"), `${format} keeps browser-wide LaTeX cache controls visible`);
  assert.ok(rows.includes("typst-status"), `${format} keeps Typst status visible`);
  assert.ok(rows.includes("quarto-executable") && rows.includes("calepin-executable"), `${format} keeps both local integration sections visible`);
}
for (const format of ["typst", "markdown", "quarto"]) {
  const rows = renderRows({ format, mayEdit: false });
  assert.ok(rows.includes("render-folder") && rows.includes("render-output"), `${format} offers its applicable folder and output controls`);
}
assert.ok(!renderRows({ format: "latex", mayEdit: false }).includes("render-folder"));
assert.ok(!renderRows({ format: "html", mayEdit: false }).includes("render-output"));

// The dialog owns the fixed four-section layout, including for HTML and
// read-only documents. Quarto preferences stay visible but become disabled
// outside Quarto or Markdown using Quarto.
for (const name of ["LaTeX", "Typst", "Quarto", "Calepin"]) {
  assert.ok(dialog.includes(`<h4 class="settings-subhead">${name}</h4>`), `Render always includes ${name}`);
}
assert.match(dialog, /<RenderingSettings options=\{buildPreferences\} \{onapplyoptions\} scopeKey=\{`\$\{documentId\}\\u0000\$\{sourceFormat\}`\} disabled=\{!quartoOptionsRelevant\} \/>/);
assert.match(dialog, /<h4 class="settings-group-title">Current document<\/h4>/);
assert.match(dialog, /<span class="settings-scope">This browser<\/span>/);
assert.match(dialog, /<span class="settings-scope">This computer<\/span>/);
assert.match(dialog, /const quartoOptionsRelevant = \$derived\(sourceFormat === "quarto" \|\| \(sourceFormat === "markdown" && buildPreferences\.tool === "quarto"\)\);/);
assert.match(rendering, /disabled=\{controlsDisabled\}/);
assert.match(dialog, /disabled=\{!quartoExecutionRelevant\}/);
assert.match(latexFiles, /id="render-latex-files"/);
assert.match(folder, /id="render-folder"/);
assert.match(folder, /disabled=\{!canChoose\}/);
assert.doesNotMatch(folder, /localBridge\.probe/);

// Local build choices do not probe on mount, browser choices stay available,
// and stale local capabilities cannot enable outputs after disconnect.
assert.doesNotMatch(build, /\$effect\([^)]*localBridge\.probe/);
assert.match(build, /if \(backend === "local"\) void localBridge\.probe\(\{ force: true \}\);/);
assert.match(build, /if \(local\?\.state !== "connected"\) return true;/);
assert.match(build, /disabled=\{preferences\.backend === "local" && outputChoices\.every\(disabledOutput\)\}/);

// Integration command fields stay rendered; the companion and fresh settings
// are required before edits apply, and responses are scoped to the connection.
assert.match(integration, /const showFields = \$derived\(name !== "zotero"\);/);
assert.match(integration, /const canEdit = \$derived\(isConnected && settingsLoaded && !pendingDialogAction\);/);
assert.match(integration, /requestId !== loadId[\s\S]{0,260}local\?\.state !== "connected"/);
assert.match(integration, /if \(requestedName === "zotero"\) settingsLoaded = true;/);
assert.match(integration, /Zotero library/);
assert.match(integration, /local API/);

// The account page calls the server Remote connection and shows its state as
// a compact, accessible pill beside the server address.
assert.match(registryText, /id: "remote-status", says: "Remote connection"/);
assert.match(remote, /<SettingRow id="remote-status" title="Remote connection"/);
assert.match(remote, /StatusPill/);
assert.match(statusPill, /tone = "neutral"/);
assert.match(statusPill, /role="status" aria-label=\{accessibleLabel \|\| label\}/);
assert.match(statusPill, /aria-hidden="true"/);

console.log("settings-quarto: Render sections and format-specific controls stay available across document types; offline local controls are unavailable while browser preferences remain usable");
