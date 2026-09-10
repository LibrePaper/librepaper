import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (name) => readFile(new URL(`../src/components/settings/${name}`, import.meta.url), "utf8");
const registry = await read("registry.js");
const local = await read("LocalAppSettings.svelte");
const storage = await read("StorageSettings.svelte");
const compiler = await read("CompilerSettings.svelte");

// The local pairing surface is shared by the two formats. Keep these checks
// close to the components because a LaTeX-only gate silently makes Quarto's
// local renderer impossible to configure from the browser.
assert.match(registry, /const local = \(\{ format, mayEdit \}\) => \(format === "latex" \|\| format === "quarto"\) && mayEdit;/);
assert.match(registry, /id: "local", group: "computer", says: "Local app", offered: local,/);
assert.match(local, /import \* as localBridge from "\.\.\/\.\.\/lib\/latex\/local\.js"/);
assert.match(local, /localBridge\.subscribe\(\(status\) => \(local = status\)\)/);
assert.doesNotMatch(local, /latex\.local\.(status|address|connect|disconnect|retry|capabilities)/);

// Engine and browser-cache controls remain specific to LaTeX. This catches
// accidentally exposing project-wide LaTeX choices in a Quarto document
// while still requiring the local service controls above.
assert.match(registry, /const latex = \(\{ format, mayEdit \}\) => format === "latex" && mayEdit;/);
assert.match(registry, /id: "compiler", group: "document", says: "Compiler", offered: latex,/);
assert.match(registry, /id: "storage-latex",[^\n]*offered: latex/);
assert.match(storage, /\{#if showsLatex\}[\s\S]*id="storage-latex"/);
assert.doesNotMatch(compiler, /sourceFormat/);

console.log("settings-quarto: Quarto exposes the shared local pairing and doctor panel without LaTeX controls");
