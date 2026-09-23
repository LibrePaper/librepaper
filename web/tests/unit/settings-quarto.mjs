import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registry = await read("registry.js");
const { offered, search } = await import("../../src/components/settings/registry.js");
const local = await read("LocalAppSettings.svelte");
const storage = await read("StorageSettings.svelte");
const rendering = await read("RenderingSettings.svelte");
const build = await read("BuildSettings.svelte");
const reader = await readFile(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");

// Local and remote connection information is available regardless of format
// or edit rights, so users can connect and understand their project anywhere.
for (const format of ["latex", "typst", "markdown", "quarto", "html", ""]) {
  for (const mayEdit of [true, false]) {
    const ids = offered({ format, mayEdit, signedIn: false }).map((item) => item.id);
    assert.ok(ids.includes("local"), `${format} (mayEdit=${mayEdit}) offers Local`);
    assert.ok(ids.includes("remote"), `${format} (mayEdit=${mayEdit}) offers Remote`);
  }
}
assert.match(registry, /id: "local", says: "Local", offered: local,/);
assert.match(registry, /id: "remote", says: "Remote", offered: remote,/);
for (const query of ["windows setup", "macos", "claude", "zotero", "backup", "server address"]) {
  const matches = search(query, { format: "html", mayEdit: false, signedIn: false }) || [];
  assert.ok(matches.length, `settings search finds ${query}`);
}
assert.match(registry, /id: "local-install-help"/);
assert.match(registry, /id: "remote-status"/);
assert.match(local, /import \* as localBridge from "\.\.\/\.\.\/lib\/companion\/client\.js"/);
// The panel shows the status rather than keeping its own copy of it: the
// shared module owns that state, and this panel is one of its watchers.
assert.match(local, /const local = \$derived\(companion\.status\);/);
assert.match(local, /\$effect\(\(\) => companion\.watch\(\)\);/);
assert.doesNotMatch(local, /latex\.local\.(status|address|connect|disconnect|retry|capabilities)/);
assert.match(local, /\["quarto", "typst", "markdown"\]\.includes\(sourceFormat\)/);
assert.match(local, /placeholder=\{quarto \? "main\.qmd" : sourceFormat === "typst" \? "main\.typ" : "main\.md"\}/);

// Engine and browser-cache controls remain specific to LaTeX. This catches
// accidentally exposing project-wide LaTeX choices in a Quarto document
// while still requiring the local service controls above.
assert.match(registry, /const latex = \(\{ format, mayEdit \}\) => format === "latex" && mayEdit;/);
assert.match(registry, /id: "build", says: "Build", offered: build,/);
assert.match(registry, /id: "storage-latex",.*offered: latex }/);
assert.match(storage, /id="storage-latex"/);
// The account's own storage is a section of the same category, gated on being
// signed in rather than on the format, and it lives in the settings dialog
// rather than behind the account menu.
assert.match(registry, /id: "storage-account",.*offered: account }/);
assert.doesNotMatch(await readFile(new URL("../../src/components/Nav.svelte", import.meta.url), "utf8"), /QuotaSettings|storage/);

// The Quarto page offers only what the live preview actually reads: a
// profile and parameters. There is no format picker (the preview is always
// the page Quarto renders) and no preview link (the app serves no URL).
assert.match(registry, /id: "rendering", says: "Quarto", offered: quarto,/);
assert.match(rendering, /id="rendering-profile"/);
assert.match(rendering, /id="rendering-parameters"/);
assert.doesNotMatch(rendering, /Render format|FORMATS/);
assert.doesNotMatch(rendering, /preview\.url/);

// No scope groups in the navigation: every category is a flat entry, and the
// two that are not this browser's alone say so in a note under their title.
assert.doesNotMatch(registry, /GROUPS|group:/);
assert.match(registry, /id: "build",[\s\S]*?note: "Only this browser and user\."/);
assert.match(registry, /id: "local",[\s\S]*?note: "The LibrePaper app running on this computer\."/);

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

console.log("settings-quarto: Quarto exposes the shared local pairing and doctor panel without LaTeX controls; the build pane and the local-execution item reach for the companion only when asked; storage, groups and render options are as expected");
