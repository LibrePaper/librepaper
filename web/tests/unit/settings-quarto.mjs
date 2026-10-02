import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registryText = await read("registry.js");
const { offered, search, CATEGORIES } = await import("../../src/components/settings/registry.js");
const build = await read("BuildSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
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
  assert.ok(rows.includes("render-local"), `${format} keeps local tools status visible`);
  assert.ok(rows.includes("render-latex-engine"), `${format} keeps LaTeX engine selection visible`);
  assert.ok(rows.includes("render-latex-files"), `${format} keeps browser-wide LaTeX cache controls visible`);
  assert.ok(rows.includes("render-typst-tool"), `${format} keeps Typst tool selection visible`);
  assert.ok(rows.includes("quarto-executable") && rows.includes("calepin-executable"), `${format} keeps both local integration sections visible`);
}
assert.ok(!renderRows({ format: "latex", mayEdit: false }).includes("render-folder"));
assert.ok(!renderRows({ format: "latex", mayEdit: false }).includes("render-output"));

// The dialog shows a fixed layout with sections for LaTeX, Typst and Calepin,
// and Markdown and Quarto. All sections stay visible regardless of the current
// document format.
for (const name of ["LaTeX", "Typst and Calepin", "Markdown and Quarto"]) {
  assert.ok(dialog.includes(`<h4 class="settings-subhead">${name}</h4>`), `Render always includes ${name}`);
}
assert.match(dialog, /<RenderingSettings \{userId\} \{onquartooptions\} \/>/);
assert.doesNotMatch(dialog, /h4 class="settings-group-title">/);
assert.match(dialog, /<span class="settings-scope">This browser<\/span>/);
assert.doesNotMatch(dialog, /quartoOptionsRelevant/);
assert.match(rendering, /disabled=\{controlsDisabled\}/);
assert.doesNotMatch(dialog, /quartoExecutionRelevant/);
assert.match(latexFiles, /id="render-latex-files"/);
assert.doesNotMatch(dialog, /render-folder/);

// Build settings are global and do not probe on mount. Selecting a local tool
// does probe. LaTeX engine, Typst tool, Markdown tool, and Quarto tool
// selections all present the correct options.
assert.doesNotMatch(build, /\$effect\([^)]*probe/);
assert.match(build, /void localBridge\.probe\(\{ force: true \}\)/);
assert.match(build, /id: "render-latex-engine"/);
assert.deepEqual([...build.matchAll(/value: "(\w+)", says/g)].map((match) => match[1]), [
  "automatic", "pdflatex", "xelatex",
  "browser", "calepin",
  "browser", "pandoc", "quarto",
  "browser", "quarto",
]);
assert.doesNotMatch(build, /lualatex/i);
for (const format of ["latex", "typst", "markdown", "quarto"]) assert.match(dialog, new RegExp(`<BuildSettings format="${format}"`));

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
