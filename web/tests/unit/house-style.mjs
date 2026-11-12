import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { latexmlWithHouseStyle, quartoWithHouseStyle, typstWithHouseStyle } from "../../src/lib/house-style.js";

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
  assert.deepEqual(words(out), words(html));
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

// An older embedded stylesheet is replaced by the house one; a house page is left alone.
{
  const { pageWithHouseStyle } = await import("../../src/lib/house-style.js");
  const old = "<html><head><style>body{max-width:46rem}</style></head><body><p>x</p></body></html>";
  const out = pageWithHouseStyle(old, ":root{--lp-ink:#000}");
  assert.ok(out.includes("--lp-ink") && !out.includes("46rem"), out);
  assert.equal(pageWithHouseStyle(out, ":root{--lp-ink:#111}"), out);
}
