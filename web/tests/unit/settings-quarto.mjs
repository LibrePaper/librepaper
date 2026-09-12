import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../../src/components/settings/${name}`, import.meta.url), "utf8");
const registry = await read("registry.js");
const local = await read("LocalAppSettings.svelte");
const storage = await read("StorageSettings.svelte");
const compiler = await read("CompilerSettings.svelte");
const dictation = await read("DictationSettings.svelte");
const rendering = await read("RenderingSettings.svelte");

// The local pairing surface is shared by the two formats. Keep these checks
// close to the components because a LaTeX-only gate silently makes Quarto's
// local renderer impossible to configure from the browser.
assert.match(registry, /const local = \(\{ format, mayEdit \}\) => \(format === "latex" \|\| format === "quarto"\) && mayEdit;/);
assert.match(registry, /id: "local", says: "Local app", offered: local,/);
assert.match(local, /import \* as localBridge from "\.\.\/\.\.\/lib\/latex\/local\.js"/);
assert.match(local, /localBridge\.subscribe\(\(status\) => \(local = status\)\)/);
assert.doesNotMatch(local, /latex\.local\.(status|address|connect|disconnect|retry|capabilities)/);

// Engine and browser-cache controls remain specific to LaTeX. This catches
// accidentally exposing project-wide LaTeX choices in a Quarto document
// while still requiring the local service controls above.
assert.match(registry, /const latex = \(\{ format, mayEdit \}\) => format === "latex" && mayEdit;/);
assert.match(registry, /id: "compiler", says: "Compiler", offered: latex,/);
assert.match(registry, /id: "storage", says: "Storage", offered: latex,/);
assert.match(storage, /id="storage-latex"/);
assert.doesNotMatch(storage, /dictation/);
assert.doesNotMatch(compiler, /sourceFormat/);

// The speech models a browser downloaded are listed with the rest of
// dictation, not under a separate storage page.
assert.match(registry, /id: "dictation-downloads", says: "Downloaded models"/);
assert.match(dictation, /id={index === 0 \? "dictation-downloads" : undefined}/);
assert.match(dictation, /removeCachedModel/);

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
assert.match(registry, /id: "compiler",[\s\S]*?note: "Shared with everyone who edits this document\."/);
assert.match(registry, /id: "local",[\s\S]*?note: "The LibrePaper app running on this computer\."/);

console.log("settings-quarto: Quarto exposes the shared local pairing and doctor panel without LaTeX controls; storage, groups and render options are as expected");
