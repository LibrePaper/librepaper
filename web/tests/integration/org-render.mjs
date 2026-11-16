// Renders Org through the real Org WebAssembly module and checks the page it
// returns. Skipped when the module has not been built, as the other renderer
// integration tests are.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { call, handOver } from '../../src/lib/renderer-wasm.js';

const source = String.raw`#+TITLE: Paper

* TODO [#A] Methods :draft:
SCHEDULED: <2026-11-12 Thu>
:LOGBOOK:
CLOCK: [2026-11-09 Mon 09:00]--[2026-11-09 Mon 10:30] =>  1:30
:END:

Text with *bold*, $x^2$, a note[fn:a] and [[https://example.org][a link]].

#+CAPTION: Results by $\tau$
| a | b |
|---+---|
| 1 | 2 |

[fn:a] The note.
`;

try {
  const binary = readFileSync(new URL('../../wasm/org.wasm', import.meta.url));
  const engine = new WebAssembly.Instance(new WebAssembly.Module(binary), {}).exports;

  // The worker asks for the title before any compile, with the source alone.
  assert.equal(call(engine, 'title_of', source).text, 'Paper');

  handOver(engine, { main: 'paper.org', texts: { 'paper.org': source }, assets: {}, urls: {} });
  const rendered = call(engine, 'compile', source, 'Paper');
  assert.equal(rendered.ok, true);
  assert.deepEqual(rendered.diagnostics, []);
  for (const fragment of [
    '<span class="todo">TODO</span>',
    '<span class="priority">[#A]</span>',
    'class="planning"',
    'class="logbook"',
    '<b>bold</b>',
    '<span data-math-style="inline">x^2</span>',
    '<a href="https://example.org">a link</a>',
    '<table><caption>Results by <span data-math-style="inline">\\tau</span></caption>',
    '<section class="footnotes">',
    '<li id="fn-1">The note.',
  ]) {
    assert.ok(rendered.text.includes(fragment), fragment);
  }
  assert.ok(!rendered.text.includes('<main>'));
  assert.ok(!rendered.text.includes('data-source'));
  console.log('org-render: Org module rendered the fixture page and title_of returned Paper');
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
}
