import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createTypstPaperRenderer } from "../../src/lib/paper/typst-renderer.js";

const typstWasm = new URL("../../wasm/typst.wasm", import.meta.url);

if (!existsSync(typstWasm)) {
  console.log("typst.wasm is missing; skipping the Typst frame tests");
} else {
  const renderer = await createTypstPaperRenderer(readFileSync(typstWasm));

  // Aligned content is kept, in a div that carries its horizontal side.
  {
    assert.equal(renderer.render("#align(center)[Centered text]"), '<div style="text-align: center">Centered text</div>');
    assert.ok(renderer.render("#align(right + bottom)[R]").includes("text-align: right"));
    assert.ok(renderer.render("#align(top)[T]").includes("text-align: start"));
  }

  // Placed content is kept in the flow, ahead of the block's other content.
  {
    const html = renderer.render('#place(top + center, float: true, scope: "parent")[Placed]\n\nBody.');
    assert.ok(html.includes("Placed"), html);
    assert.ok(html.includes("<p>Body.</p>"), html);
  }

  // The content of the other layout containers is kept too: a grid's cells and a stack's children come one after another.
  {
    assert.equal(renderer.render("#pad(x: 1em)[Padded]"), "<p>Padded</p>");
    assert.equal(renderer.render("#grid(columns: 2, grid.cell(colspan: 2)[One], [Two], grid.hline())"), "<p>One</p><p>Two</p>");
    assert.equal(renderer.render("#stack(spacing: 1em, [Up], 2em, [Down])"), "<p>Up</p><p>Down</p>");
    assert.ok(renderer.render("#rotate(5deg)[Turned]").includes("Turned"));
    // A table is not a grid, and stays a table.
    assert.ok(renderer.render("#table(columns: 2, [a], [b])").startsWith("<table>"));
  }

  // A source that does not compile still draws nothing.
  assert.equal(renderer.render("#unknown-function()"), "");

  // The frame of a template: what it writes before the body and after it.
  const template = `#let article(title: none, abstract: none, body) = {
  align(center, text(17pt, weight: "bold", title))
  block[*Abstract.* #abstract]
  body
  heading(numbering: none)[References]
}
#show: article.with(title: [A Title], abstract: [Short abstract.])
`;
  {
    const f = renderer.frame(template);
    assert.notEqual(f, null);
    assert.ok(f.before.includes("A Title"), f.before);
    assert.ok(f.before.includes("Short abstract."), f.before);
    assert.ok(f.after.includes("References"), f.after);
    assert.ok(!f.before.includes("lp-frame-mark"), f.before);
    assert.ok(!f.after.includes("lp-frame-mark"), f.after);
  }

  // A block compiled after the template carries the frame, and unframe takes it off.
  {
    const html = renderer.render(template + "\nFirst paragraph.\n");
    assert.ok(html.includes("Short abstract."), html);
    assert.equal(renderer.unframe(html, template), "<p>First paragraph.</p>");
  }

  // A prefix that writes nothing has an empty frame, and unframe changes nothing.
  {
    assert.deepEqual(renderer.frame("#set text(11pt)\n"), { before: "", after: "" });
    assert.equal(renderer.unframe("<p>x</p>", "#set text(11pt)\n"), "<p>x</p>");
  }

  // A prefix that does not compile has no frame, and unframe changes nothing.
  {
    assert.equal(renderer.frame("#unknown-function()\n"), null);
    assert.equal(renderer.unframe("<p>x</p>", "#unknown-function()\n"), "<p>x</p>");
  }

  // HTML the frame does not fit is returned as it is.
  assert.equal(renderer.unframe("<p>unrelated</p>", template), "<p>unrelated</p>");
}
