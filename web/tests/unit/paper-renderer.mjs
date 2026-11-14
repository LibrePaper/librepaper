import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  createPaperRenderer,
  currentPaperRenderer,
  loadPaperRenderer,
} from "../../src/lib/paper/renderer.js";
import { paperStylesheet } from "../../src/lib/paper/style.js";

// The renderer is the preview's own module, so it is the one the build ships.
const bytes = readFileSync(new URL("../../wasm/markdown.wasm", import.meta.url));
const renderer = await createPaperRenderer(bytes);

// A heading renders as an HTML heading, with no page wrapper around it.
{
  const html = renderer.render("# Hi");
  assert.match(html, /<h1/);
  assert.doesNotMatch(html, /<body|<html/);
}

// The footnotes list is taken out of the block, but the reference stays in the text.
{
  const html = renderer.render("Para[^1].\n\n[^1]: note");
  assert.match(html, /<sup/);
  assert.doesNotMatch(html, /footnotes/);
}

// The Org module renders the same way: its sections come out as HTML, and no
// empty section is left in the block.
{
  const orgRenderer = await createPaperRenderer(readFileSync(new URL("../../wasm/org.wasm", import.meta.url)));
  const html = orgRenderer.render("* Heading\n\nSome *bold* text.");
  assert.match(html, /<h1/);
  assert.match(html, /<b>bold<\/b>/);
  assert.doesNotMatch(html, /<section><\/section>/);
}

// A repeat render of the same source gives the same HTML.
{
  assert.equal(renderer.render("# Same"), renderer.render("# Same"));
}

// Before anything has loaded, there is no renderer to hand out, and loading
// with no module URL configured rejects.
{
  assert.equal(currentPaperRenderer(), null);
  delete globalThis.LIBREPAPER_MODULES;
  await assert.rejects(loadPaperRenderer());
}

// The stylesheet is scoped to the block container and its page layout is
// undone. The page's :root variables carry over onto the scope root, and the
// dark theme class (on the editor, outside the scope) reaches them.
{
  const css = paperStylesheet("body { color: red; max-width: 46rem }\n:root { --ink: #000 }\n.lp-dark { --ink: #fff }\n.lp-dark img { background: #fff }\nh1 { margin-top: 1em }");
  assert.ok(css.includes("@scope (.cm-lp-paper)"));
  assert.ok(css.includes(":scope {"));
  assert.doesNotMatch(css, /:root/);
  assert.doesNotMatch(css, /body\s*\{/);
  assert.match(css, /:scope \{ --ink: #000 \}/);
  assert.match(css, /\.lp-dark :scope \{ --ink: #fff \}/);
  assert.match(css, /\.lp-dark :scope img \{/);
  assert.doesNotMatch(css, /cm-lp-paper-dark/);
  const both = paperStylesheet(":root.lp-dark { --ink: #fff }\n.lp-dark :root { --a: 1 }");
  // Both spellings of the dark root are rewritten (the stylesheet also adds
  // its own `.lp-dark :scope` editor-colour rule).
  assert.match(both, /\.lp-dark :scope \{ --ink: #fff \}/);
  assert.match(both, /\.lp-dark :scope \{ --a: 1 \}/);
}
