// Applies live patches to a LaTeXML-shaped page in a real browser, with the
// real KaTeX, and checks that only the intended nodes change.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const katexDir = join(root, "node_modules/katex/dist");
if (!existsSync(join(katexDir, "katex.min.js"))) throw new Error(`KaTeX not found in ${katexDir}; run bun install in web/`);
const temporary = mkdtempSync(join(tmpdir(), "librepaper-live-patch-"));

const page_html = `<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/katex.min.css">
<body><main id="root">
<p class="ltx_p" id="p1" data-sourcepos="0:12:2-0:12:2">We have <math class="ltx_Math" alttext="x^{2}" display="inline" data-sourcepos="0:12:9-0:12:9"><mi>x</mi></math> and more text<span class="ltx_note"><span class="ltx_note_outer"><span class="ltx_note_content">A footnote mentioning zebra crossing here.</span></span></span> and some unique wording.</p>
<table class="ltx_equation" data-sourcepos="0:15:1-0:15:1"><tbody><tr><td><math class="ltx_Math" alttext="E = mc^{2}" display="block"><mi>E</mi></math></td></tr></tbody></table>
<p class="ltx_p" id="p2" data-sourcepos="0:16:2-0:16:2">Repeated phrase here. Another Repeated phrase here.</p>
<p class="ltx_p" id="p3" data-sourcepos="0:20:2-0:20:2">Twin <math class="ltx_Math" alttext="y" display="inline"><mi>y</mi></math> and <math class="ltx_Math" alttext="y" display="inline"><mi>y</mi></math> here.</p>
</main>
<script src="/katex.min.js"></script>
<script type="module">
import { applyLivePatch } from "/live-patch.js";
const ctx = { sources: ["main.tex", null], katex: window.katex };
window.apply = (patch) => applyLivePatch(document, ctx, patch);
window.textNodes = () => { const out = []; const walk = (n) => { for (const c of n.childNodes) { if (c.nodeType === 3) out.push(c); else walk(c); } }; walk(document.body); return out; };
window.mark = () => window.textNodes().forEach((n, i) => { n.lpMark = i; n.lpData = n.data; });
window.changed = () => window.textNodes().filter((n) => n.lpMark === undefined || n.data !== n.lpData).map((n) => n.data);
window.ready = true;
</script></body>`;

const files = {
  "/": ["text/html; charset=utf-8", () => page_html],
  "/live-patch.js": ["text/javascript; charset=utf-8", () => readFileSync(join(root, "src/agent/live-patch.js"))],
  "/katex.min.js": ["text/javascript; charset=utf-8", () => readFileSync(join(katexDir, "katex.min.js"))],
  "/katex.min.css": ["text/css; charset=utf-8", () => readFileSync(join(katexDir, "katex.min.css"))],
};
let server, page;
try {
  server = createServer((request, response) => {
    const entry = files[new URL(request.url, "http://x").pathname];
    if (!entry) return response.writeHead(404).end();
    response.setHeader("content-type", entry[0]);
    response.end(entry[1]());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  page = await browser(process.env.BROWSER || "chromium", join(temporary, "profile"), 24000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${port}/`);
  await until("page ready", () => page.evaluate("window.ready === true"), 10000);

  const call = async (patch) => JSON.parse(await page.evaluate(`JSON.stringify(window.apply(${JSON.stringify(patch)}))`));
  const changed = async () => JSON.parse(await page.evaluate("JSON.stringify(window.changed())"));
  const html = () => page.evaluate("document.getElementById('root').innerHTML");
  const prose = (over) => ({ kind: "prose", file: "main.tex", line: 14, before: "", deleted: "", inserted: "", after: "", ...over });
  // Alttext is normalized TeX, so the patch's oldTex is written the way the source is.
  const math = (over) => ({ kind: "math", file: "main.tex", line: 12, oldTex: "x^2", tex: "x^{3}", display: false, ...over });
  const rejected = async (label, patch) => {
    const before = await html();
    assert.equal(await call(patch), false, `${label}: returns false`);
    assert.equal(await html(), before, `${label}: page unchanged`);
  };

  await page.evaluate("window.mark()");

  // Typing a letter mid-word touches exactly one text node.
  assert.equal(await call(prose({ before: "and some unique wo", inserted: "x", after: "rding." })), true);
  assert.deepEqual(await changed(), [" and some unique woxrding."]);
  // Replacement inside one node.
  assert.equal(await call(prose({ before: "some ", deleted: "unique", inserted: "special", after: " woxrding" })), true);
  assert.deepEqual(await changed(), [" and some special woxrding."]);
  // Deletion.
  assert.equal(await call(prose({ before: "special wo", deleted: "x", after: "rding" })), true);
  assert.deepEqual(await changed(), [" and some special wording."]);
  // Whitespace in the context is collapsed on both sides.
  assert.equal(await call(prose({ before: "some  special", inserted: "!", after: " wording" })), true);

  await rejected("context twice", prose({ line: 16, before: "Repeated phrase", inserted: "x", after: " here" }));
  await rejected("footnote only", prose({ line: 12, before: "mentioning zebra", inserted: "x", after: " crossing" }));
  await rejected("math content only", prose({ line: 12, before: "have x", inserted: "q", after: " and" }));
  await rejected("wrong file", prose({ file: "other.tex", before: "some special", inserted: "x", after: " wording" }));
  await rejected("unknown file", prose({ file: "nope.tex", before: "some special", inserted: "x", after: " wording" }));
  await rejected("line past the last element", prose({ line: 30, before: "some special", inserted: "x", after: " wording" }));
  await rejected("deleted spans text nodes", prose({ line: 12, before: "more ", deleted: "text and", inserted: "z", after: " some" }));
  await rejected("no context match", prose({ before: "absent phrase", inserted: "x", after: " nowhere" }));
  await rejected("empty edit", prose({ before: "some special", after: " wording" }));

  // Second line of a multi-line paragraph (p1 spans lines 12-14; it starts at 12).
  assert.equal(await call(prose({ line: 13, before: "special! wo", deleted: "rding", inserted: "rd", after: "." })), true);
  assert.deepEqual(await changed(), [" and some special! word."]);
  // First line of the next paragraph (p2 starts at 16): the edit on line 17 is
  // looked up in p2, not in p1.
  assert.equal(await call(prose({ line: 17, before: "Another ", deleted: "Repeated", inserted: "New", after: " phrase" })), true);
  assert.deepEqual(await changed(), [" and some special! word.", "Repeated phrase here. Another New phrase here."]);

  // Math.
  assert.equal(await call(math()), true);
  assert.equal(await page.evaluate("document.querySelectorAll('#p1 math[alttext]').length"), 0, "old math gone");
  const span = JSON.parse(await page.evaluate(`JSON.stringify((() => { const s = document.querySelector('#p1 .lp-live-math'); return s && { tex: s.dataset.lpTex, prov: s.hasAttribute('data-lp-provisional'), pos: s.getAttribute('data-sourcepos'), katex: !!s.querySelector('.katex'), block: s.style.display }; })())`));
  assert.deepEqual(span, { tex: "x^{3}", prov: true, pos: "0:12:9-0:12:9", katex: true, block: "" });
  // A second patch, on a line after the math starts, finds the live span through data-lp-tex.
  assert.equal(await call(math({ line: 13, oldTex: "x ^ {3}", tex: "x^{4}" })), true);
  assert.equal(await page.evaluate("document.querySelectorAll('#p1 .lp-live-math').length"), 1);
  assert.equal(await page.evaluate("document.querySelector('#p1 .lp-live-math').dataset.lpTex"), "x^{4}");
  // Display equation starts at line 15; the patch may name the line before or the line itself.
  await rejected("display equation too early", math({ line: 11, oldTex: "E=mc^2", tex: "E=mc^{3}", display: true }));
  await rejected("display equation too late", math({ line: 19, oldTex: "E=mc^2", tex: "E=mc^{3}", display: true }));
  assert.equal(await call(math({ line: 14, oldTex: "E=mc^2", tex: "E=mc^{3}", display: true })), true);
  assert.equal(await page.evaluate("document.querySelector('.ltx_equation .lp-live-math').style.display"), "block");
  assert.equal(await page.evaluate("document.querySelector('.ltx_equation .lp-live-math').style.textAlign"), "center");

  await rejected("invalid TeX", math({ oldTex: "x^{4}", tex: "\\frac{" }));
  await rejected("ambiguous math", math({ line: 20, oldTex: "y", tex: "z" }));
  await rejected("no text match", math({ oldTex: "w^{9}", tex: "w" }));
  await rejected("wrong line", math({ line: 30, oldTex: "x^{4}", tex: "x" }));
  await rejected("wrong file (math)", math({ file: "other.tex", oldTex: "x^{4}", tex: "x" }));

  // Paragraph patches, on elements added after the other cases.
  await page.evaluate(`document.getElementById('root').insertAdjacentHTML('beforeend', '<section id="s1"><h2 class="ltx_title ltx_title_section" id="h1" data-sourcepos="0:40:1-0:40:1">Heading</h2><div class="ltx_para" id="w1"><p class="ltx_p" data-sourcepos="0:42:2-0:42:2">Wrapped paragraph.</p></div></section>')`);
  const para = (over) => ({ kind: "paragraph", file: "main.tex", anchorLine: 42, text: "New", ...over });
  const provisional = () => page.evaluate("document.querySelectorAll('[data-lp-para]').length");
  assert.equal(await call(para()), true);
  assert.equal(await page.evaluate("document.getElementById('w1').nextElementSibling.getAttribute('data-lp-para')"), "0:42");
  assert.equal(await page.evaluate("document.getElementById('w1').nextElementSibling.querySelector('p.ltx_p').textContent"), "New");
  assert.equal(await call(para({ text: "New words" })), true);
  assert.equal(await provisional(), 1, "same anchor updates in place");
  assert.equal(await page.evaluate("document.querySelector('[data-lp-para] p').textContent"), "New words");
  assert.equal(await call(para({ text: "New words" })), false, "equal text changes nothing");
  assert.equal(await call(para({ text: "" })), true);
  assert.equal(await provisional(), 0, "empty text removes it");
  assert.equal(await call(para({ anchorLine: 40 })), true);
  assert.equal(await page.evaluate("document.getElementById('h1').nextElementSibling.getAttribute('data-lp-para')"), "0:40", "after the heading");
  await rejected("paragraph for an unknown file", para({ file: "nope.tex", anchorLine: 41 }));

  console.log("live-patch-browser: prose and math patches apply to exactly one node and refuse every ambiguous case");
} finally {
  await page?.close?.();
  if (server) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
