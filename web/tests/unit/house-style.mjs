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
  assert.equal(body(out).replace(/<script id="quarto-html-after-body"[\s\S]*?<\/script>\s*/, ""), body(html).replace(/<script id="quarto-html-after-body"[\s\S]*?<\/script>\s*/, ""));
  // No stylesheet to apply: the page is untouched.
  assert.equal(quartoWithHouseStyle(html, ""), html);
}
