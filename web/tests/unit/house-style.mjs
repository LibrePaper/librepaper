import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { houseStyleCss, latexmlWithHouseStyle, quartoWithHouseStyle, typstWithHouseStyle, withHouseStyle } from "../../src/lib/house-style.js";

const css = "body { color: var(--lp-ink) }";

// LaTeXML's styles are replaced by one house style, in place of the first.
{
  const html = '<html><head><style data-latexml-css="a.css">.ltx_page{}</style><style>.keep{}</style><style data-latexml-css="b.css">@media (prefers-color-scheme: dark){}</style></head><body><p>x</p></body></html>';
  const out = latexmlWithHouseStyle(html, css);
  assert.ok(!out.includes("data-latexml-css"));
  assert.equal(out.match(/data-lp-house/g).length, 1);
  assert.ok(out.includes(`<style data-lp-house>${css}</style><style>.keep{}</style></head>`));
}

// With no LaTeXML style, the house style goes at the end of the head.
assert.equal(latexmlWithHouseStyle("<head></head><p>x</p>", css), `<head><style data-lp-house>${css}</style></head><p>x</p>`);
// No stylesheet to apply: the page is untouched.
assert.equal(latexmlWithHouseStyle("<head><style data-latexml-css>a{}</style></head>", ""), "<head><style data-latexml-css>a{}</style></head>");

// Typst: its own styles go, the MathML rules stay.
{
  const html = "<html><head><style>body{margin:0} h1{}</style><style>math { display: inline } mrow {}</style></head><body></body></html>";
  const out = typstWithHouseStyle(html, css);
  assert.ok(!out.includes("body{margin:0}"));
  assert.ok(out.includes("math { display: inline }"));
  assert.ok(out.includes(`<style data-lp-house>${css}</style>`));
}

// Quarto: Bootstrap and the scripts that serve it go, the house style takes
// their place, the root is marked, and the body is untouched.
{
  const html = readFileSync(new URL("../fixtures/quarto-page.html", import.meta.url), "utf8");
  const out = quartoWithHouseStyle(html, css);
  assert.equal(out.match(/data-lp-house/g).length, 1);
  assert.ok(out.includes(`<style data-lp-house>${css}</style>`));
  assert.match(out, /<html [^>]*class="lp-quarto"/);
  assert.ok(!/bootstrap|tippy|clipboard\.js|AnchorJS|popperjs|Polyfill service|tabsets/i.test(out.slice(0, out.indexOf("<body"))));
  assert.ok(!out.includes("data:text/css"));
  assert.ok(!out.includes("quarto-html-after-body"));
  // Pandoc's own small rules and the math loader stay.
  assert.ok(out.includes("span.smallcaps"));
  assert.ok(out.includes("mathjax@3/es5/tex-chtml-full.js"));
  assert.ok(out.includes("window.Quarto"));
  const body = (page) => page.slice(page.indexOf("<body"), page.indexOf("</body>"));
  // No content is lost: the same words, whatever the order (the contents move
  // after the title block) and punctuation (caption labels).
  const words = (page) => body(page).replace(/<script id="quarto-html-after-body"[\s\S]*?<\/script>/, "")
    .replace(/<[^>]+>/g, " ").replace(/[.:\u00a0]/g, " ").split(/\s+/).filter(Boolean).sort();
  assert.deepEqual(words(out), words(html.replace(">Table of contents<", ">Contents<")));
  // No stylesheet to apply: the page is untouched.
  assert.equal(quartoWithHouseStyle(html, ""), html);
}

// The contents follow the title block, and Quarto's caption number becomes the house label.
{
  const { quartoWithHouseStyle } = await import("../../src/lib/house-style.js");
  const page = '<html><head><style>.quarto-x{}</style></head><body><nav id="TOC"><ul></ul></nav><header id="title-block-header"><h1>T</h1></header><figure><figcaption class="quarto-float-caption quarto-float-fig"> Figure\u00a01: A plot </figcaption></figure></body></html>';
  const out = quartoWithHouseStyle(page, ":root{--lp-ink:#000}");
  assert.ok(out.indexOf('id="title-block-header"') < out.indexOf('id="TOC"'), out);
  assert.ok(out.includes('<span class="caption-label">Figure 1.</span> A plot'), out);
}

// The default contents title takes the house wording; an author's own title stays.
{
  const { quartoWithHouseStyle } = await import("../../src/lib/house-style.js");
  const page = (title) => `<html><head><style>.quarto-x{}</style></head><body><nav id="TOC"><h2 id="toc-title">${title}</h2></nav></body></html>`;
  assert.ok(quartoWithHouseStyle(page("Table of contents"), ":root{--lp-ink:#000}").includes('id="toc-title">Contents</h2>'));
  assert.ok(quartoWithHouseStyle(page("Inhalt"), ":root{--lp-ink:#000}").includes('id="toc-title">Inhalt</h2>'));
}

// The bundled stylesheet is the app's only copy.
{
  const bundled = await houseStyleCss();
  assert.ok(bundled.includes("--lp-ink"));
  assert.ok(bundled.includes("canonical copy"));
}

// A page from a wasm renderer gets the house style as the first stylesheet of its head.
{
  const tag = `<style data-lp-house>${css}</style>`;
  // an old module's embedded copy is replaced
  const old = withHouseStyle('<html><head><meta charset="utf-8"><style>:root{--lp-ink:#111}</style><style>.own{}</style></head><body>x</body></html>', css);
  assert.equal(old, `<html><head><meta charset="utf-8">${tag}<style>.own{}</style></head><body>x</body></html>`);
  // no style: inserted at the end of the head
  assert.equal(withHouseStyle("<html><head><title>t</title></head><body>x</body></html>", css), `<html><head><title>t</title>${tag}</head><body>x</body></html>`);
  // a renderer style, and a linked sheet, come after the house style
  assert.equal(withHouseStyle("<head><style>.own{}</style></head>", css), `<head>${tag}<style>.own{}</style></head>`);
  assert.equal(withHouseStyle('<head><title>t</title><link rel="stylesheet" href="a.css"></head>', css), `<head><title>t</title>${tag}<link rel="stylesheet" href="a.css"></head>`);
  // a page that has it is unchanged
  const had = `<html><head>${tag}<style>.own{}</style></head></html>`;
  assert.equal(withHouseStyle(had, css), had);
  // no head: one is made
  assert.equal(withHouseStyle("<html><body>x</body></html>", css), `<html><head>${tag}</head><body>x</body></html>`);
  assert.equal(withHouseStyle("<p>x</p>", css), `<head>${tag}</head><p>x</p>`);
  // a style in the body is not mistaken for a head style
  assert.equal(withHouseStyle("<head></head><body><style>.b{}</style></body>", css), `<head>${tag}</head><body><style>.b{}</style></body>`);
  // the bundled stylesheet is the default
  assert.ok(withHouseStyle("<head></head>").includes("--lp-ink"));
}
