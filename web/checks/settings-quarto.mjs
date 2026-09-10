import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/components/Settings.svelte", import.meta.url), "utf8");

// The local pairing surface is shared by the two formats. Keep these checks
// close to the component because a LaTeX-only gate silently makes Quarto's
// local renderer impossible to configure from the browser.
assert.match(source, /const showsQuarto = \$derived\(sourceFormat === "quarto" && mayEdit\)/);
assert.match(source, /const showsLocal = \$derived\(showsLatex \|\| showsQuarto\)/);
assert.match(source, /import \* as localBridge from "\.\.\/lib\/latex\/local\.js"/);
assert.match(source, /return localBridge\.subscribe\(\(status\) => \(local = status\)\)/);
assert.match(source, /\{showsQuarto \? "Local Quarto app" : "Local compilation"\}/);

// Engine and browser-cache controls remain specific to LaTeX. This catches
// accidentally exposing project-wide LaTeX choices in a Quarto panel while
// still requiring the local service controls above.
assert.match(source, /\{#if showsLatex\}[\s\S]*aria-labelledby="settings-latex-engine"/);
assert.match(source, /\{#if showsLatex\}[\s\S]*aria-labelledby="settings-cache"/);
assert.doesNotMatch(source, /latex\.local\.(status|address|connect|disconnect|retry|capabilities)/);

console.log("settings-quarto: Quarto exposes the shared local pairing and doctor panel without LaTeX controls");
