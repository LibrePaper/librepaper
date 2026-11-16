// Source-level checks for the reader and PDF viewer fixes of librepaper#k5xf.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../../src/${path}`, import.meta.url), "utf8");
const reader = read("components/Reader.svelte");
const editor = read("components/Editor.svelte");
const render = read("lib/pdf/render.js");
const find = read("lib/pdf/find.js");
const frame = read("agent/frame.js");
const resources = read("lib/latex/resources.js");

// The offline copy is marked ready only when every lazy chunk loaded.
assert.match(reader, /return results\.every\(\(result\) => result\.status === "fulfilled"\)/);
assert.match(reader, /if \(await prefetchLazyPanels\(\) && await cacheCurrentShell\(\)\) await markShellReady/);
// A failed palette or settings load says so and does not open the palette.
assert.match(reader, /try \{ await lazy\.commandPalette\.load\(\); \} catch \{ lazyFailed\(\); return; \} commandPalette = true/);
assert.match(reader, /try \{ await lazy\.settings\.load\(\); \} catch \{ lazyFailed\(\); return; \}/);

// A plain preview click moves the source caret only when the two are linked.
assert.match(reader, /function followPreviewClick\(offset, pdfPoint = null\) \{\n    if \(!linked \|\|/);

// Revealing a collaborator does not take the focus.
assert.match(editor, /function show\(id, \{ focus = true \} = \{\}\)/);
assert.match(editor, /if \(focus\) view\.focus\(\)/);
assert.match(editor, /show\(id, \{ focus: false \}\);\n    const length/);

// The page break is put against the layer, and the old document's builder
// outlives a new render until the swap.
assert.match(render, /layer\.prepend\(gap\("\\n\\n"\)\)/);
assert.doesNotMatch(render, /first\.before\(gap/);
assert.match(render, /const gone = \(\) => stopped;/);

// Find reveals an unbuilt page by scrolling to the frame and finishes after it builds.
assert.match(find, /revealAfterBuild = true/);
assert.match(find, /if \(revealAfterBuild && index >= 0\) reveal\(\)/);

// Lazy page builds do not republish the whole document.
assert.match(frame, /new MutationObserver\(onMutations\)/);

// The bundle index is verified against the release itself, not a copy with another digest.
assert.doesNotMatch(resources, /digest: release\.id/);
