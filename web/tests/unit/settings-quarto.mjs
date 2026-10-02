import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registry = await read("registry.js");
const { offered, search, CATEGORIES } = await import("../../src/components/settings/registry.js");
const local = await read("LocalAppSettings.svelte");
const latexFiles = await read("LatexFilesSettings.svelte");
const folder = await read("ProjectFolderSetting.svelte");
const rendering = await read("RenderingSettings.svelte");
const build = await read("BuildSettings.svelte");
const dialog = await read("SettingsDialog.svelte");
const reader = await readFile(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");

// Local, integrations, backups and account are offered always;
// storage and remote no longer exist as categories.
for (const format of ["latex", "typst", "markdown", "quarto", "html", ""]) {
  for (const mayEdit of [true, false]) {
    const ids = offered({ format, mayEdit, signedIn: false }).map((item) => item.id);
    assert.ok(ids.includes("local"), `${format} (mayEdit=${mayEdit}) offers Companion`);
    assert.ok(ids.includes("integrations"), `${format} (mayEdit=${mayEdit}) offers Integrations`);
    assert.ok(ids.includes("account"), `${format} (mayEdit=${mayEdit}) offers Account`);
    assert.ok(!ids.includes("storage"), `${format} (mayEdit=${mayEdit}) does not offer storage category`);
    assert.ok(!ids.includes("remote"), `${format} (mayEdit=${mayEdit}) does not offer remote category`);
  }
}
assert.match(registry, /id: "local", says: "Companion", offered: always,/);
assert.match(registry, /id: "integrations", says: "Integrations", offered: always,/);
for (const query of ["windows setup", "macos", "claude", "zotero", "server address", "companion address"]) {
  const matches = search(query, { format: "html", mayEdit: false, signedIn: false }) || [];
  assert.ok(matches.length, `settings search finds ${query}`);
}
assert.match(registry, /id: "local-address", says: "Companion address"/);
assert.match(registry, /id: "remote-status"/);
assert.match(local, /import \* as localBridge from "\.\.\/\.\.\/lib\/companion\/client\.js"/);
// The panel shows the status rather than keeping its own copy of it: the
// shared module owns that state, and this panel is one of its watchers.
assert.match(local, /const local = \$derived\(companion\.status\);/);
assert.match(local, /\$effect\(\(\) => companion\.watch\(\)\);/);
assert.doesNotMatch(local, /latex\.local\.(status|address|connect|disconnect|retry|capabilities)/);
// The project folder is a build question, not a local-app discovery question.
assert.doesNotMatch(local, /sourceFormat|chooseFolder/);
assert.match(folder, /id="render-folder"/);
assert.match(folder, /placeholder=\{quarto \? "main\.qmd" : sourceFormat === "typst" \? "main\.typ" : "main\.md"\}/);
assert.doesNotMatch(folder, /localBridge\.probe/);

// Engine and browser-cache controls remain specific to LaTeX. This catches
// accidentally exposing project-wide LaTeX choices in a Quarto document
// while still requiring the local service controls above.
assert.match(registry, /const latex = \(\{ format, mayEdit \}\) => format === "latex" && mayEdit;/);
assert.match(registry, /id: "render", says: "Render", offered: build,/);
assert.match(registry, /id: "render-latex-files",.*offered: latex }/);
assert.match(latexFiles, /id="render-latex-files"/);
// The account's own storage is a row of the Account page, gated on being
// signed in rather than on the format, and it lives in the settings dialog.
assert.match(registry, /id: "storage-account",.*offered: account }/);
assert.doesNotMatch(await readFile(new URL("../../src/components/Nav.svelte", import.meta.url), "utf8"), /QuotaSettings|storage/);

// The Quarto page offers only what the live preview actually reads: a
// profile and parameters. There is no format picker (the preview is always
// the page Quarto renders) and no preview link (the app serves no URL).
assert.match(registry, /id: "quarto-status", says: "Quarto status"/);
assert.match(registry, /id: "rendering-profile", says: "Quarto profile"/);
assert.match(registry, /id: "rendering-parameters", says: "Quarto parameters"/);
assert.match(rendering, /id="rendering-profile"/);
assert.match(rendering, /id="rendering-parameters"/);
assert.doesNotMatch(rendering, /Render format|FORMATS/);
assert.doesNotMatch(rendering, /preview\.url/);
assert.doesNotMatch(build, /render-profile|render-parameters|chooseProfile|chooseParameters/);
assert.match(dialog, /<RenderingSettings options=\{buildPreferences\} \{onapplyoptions\} \/>/);
// The profile and parameters have one editor and one store: buildPreferences.
assert.doesNotMatch(reader, /quartoOptions/);

// No scope groups in the navigation: every category is a flat entry, and the
// two that are not this browser's alone say so in a note under their title.
assert.doesNotMatch(registry, /GROUPS|group:/);
assert.match(registry, /id: "render",[\s\S]*?note: "Only this browser and user\."/);
assert.match(registry, /id: "local",[\s\S]*?note: "LibrePaper on this computer"/);

// Choosing a build tool is mostly a browser question -- which engine, which
// output -- and opening this pane must not make the browser ask to allow the
// site "access to other apps and services". Nothing here probes on mount;
// picking a local tool is what looks, and until somebody has looked the local
// rows are offered rather than greyed out as unavailable.
assert.doesNotMatch(build, /\$effect\([^)]*localBridge\.probe/);
assert.match(build, /if \(backend === "local"\) void localBridge\.probe\(\{ force: true \}\);/);
assert.match(build, /if \(local\?\.state === "unknown"\) return false;/);
assert.match(build, /unknown: "The local companion has not been looked for yet\."/);

// "Execute code locally" is offered only where a local tool actually runs the
// document. On LaTeX or HTML it warned about arbitrary code execution and then
// did nothing, which is a local-app question asked of somebody who never posed
// one.
assert.match(reader, /const localExecutionRelevant = \$derived\(\["quarto", "typst"\]\.includes\(sourceFormat\) && mayEdit\);/);
assert.match(reader, /\{#if localExecutionRelevant\}[\s\S]{0,900}?Execute code locally/);

// Behavior checks on offered rows per context.
const render = CATEGORIES.find((category) => category.id === "render");
const renderRows = (context) => render.entries.filter((entry) => !entry.offered || entry.offered(context)).map((entry) => entry.id);
for (const format of ["quarto", "markdown", "typst", "latex"]) {
  const context = { format, mayEdit: true };
  const rows = renderRows(context);
  if (format === "quarto") {
    assert.ok(rows.includes("quarto-status"), "Quarto format includes quarto-status");
    assert.ok(rows.includes("quarto-executable"), "Quarto format includes quarto-executable");
    assert.ok(rows.includes("rendering-profile"), "Quarto format includes rendering-profile");
    assert.ok(rows.includes("quarto-execution"), "Quarto format includes quarto-execution");
    assert.ok(rows.includes("render-folder"), "Quarto format includes render-folder");
    assert.ok(!rows.includes("calepin-status"), "Quarto format does not include calepin-status");
    assert.ok(!rows.includes("render-latex-files"), "Quarto format does not include render-latex-files");
  }
  if (format === "markdown") {
    assert.ok(rows.includes("quarto-status"), "Markdown format includes quarto-status");
    assert.ok(!rows.includes("rendering-profile"), "Markdown format without tool does not include rendering-profile");
    assert.ok(!rows.includes("quarto-execution"), "Markdown format does not include quarto-execution");
  }
  if (format === "typst") {
    assert.ok(rows.includes("calepin-status"), "Typst format includes calepin-status");
    assert.ok(rows.includes("render-folder"), "Typst format includes render-folder");
    assert.ok(!rows.includes("quarto-status"), "Typst format does not include quarto-status");
  }
  if (format === "latex") {
    assert.ok(rows.includes("render-latex-files"), "LaTeX format includes render-latex-files");
    assert.ok(!rows.includes("quarto-status"), "LaTeX format does not include quarto-status");
    assert.ok(!rows.includes("calepin-status"), "LaTeX format does not include calepin-status");
    assert.ok(!rows.includes("render-folder"), "LaTeX format does not include render-folder");
  }
}
// With tool specified for markdown, includes rendering options.
const mdWithQuarto = renderRows({ format: "markdown", mayEdit: true, tool: "quarto" });
assert.ok(mdWithQuarto.includes("rendering-profile"), "Markdown with Quarto tool includes rendering-profile");
const integrations = CATEGORIES.find((category) => category.id === "integrations");
assert.deepEqual(integrations.entries.map((e) => e.id), ["zotero-status"], "Integrations category contains only zotero-status");

console.log("settings-quarto: Quarto exposes the shared local pairing and doctor panel without LaTeX controls; the build pane and the local-execution item reach for the companion only when asked; a tool that builds the document is configured under Render, whole");
