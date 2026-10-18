// Compiles every Typst spelling of the math palette entries with the Typst
// WASM engine, the same way typst-pdf.mjs drives it. Needs the built engine
// in web/wasm; see typst-pdf.mjs for how it is fetched.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { call, handOver } from '../../src/lib/renderer-wasm.js';
import { SYMBOLS } from '../../src/lib/symbols/index.js';
import { MATH_TEMPLATES, templateInsertion } from '../../src/lib/symbols/templates.js';

const MATH = new Set(['greek', 'operators', 'relations', 'arrows', 'logic', 'accents', 'delimiters']);
const spellings = SYMBOLS
  .filter(symbol => MATH.has(symbol.category))
  .map(symbol => symbol.typst.replaceAll('#1', 'x'));
// Every template, filled so no snippet field remains, in each of its options.
const filled = template => {
  if (template.id === 'aligned') return { rows: [{ left: 'x', right: 'y' }] };
  if (template.id === 'gathered') return { rows: [{ value: 'x = y' }, { value: 'z = w' }] };
  if (template.id === 'cases') return { rows: [{ value: '1', condition: 'x > 0' }] };
  if (template.id === 'matrix') return { cells: [['a', 'b'], ['c', 'd']], rows: 2, columns: 2 };
  return Object.fromEntries(template.slots.map(slot => [slot.id, 'x']));
};
const OPTIONS = {
  brackets: ['kind', ['parentheses', 'square', 'braces', 'angle', 'bars', 'norm', 'floor', 'ceil']],
  matrix: ['brackets', ['parentheses', 'square', 'braces', 'bars', 'double-bars', 'none']],
};
const typstContext = { format: 'typst', text: '$x$', selection: { from: 1, to: 1, text: '' }, files: [] };
const templateSpellings = [];
for (const template of MATH_TEMPLATES) {
  const [key, choices] = OPTIONS[template.id] ?? [null, [null]];
  for (const choice of choices) {
    const values = key ? { ...filled(template), [key]: choice } : filled(template);
    templateSpellings.push(templateInsertion(template, values, typstContext).text);
  }
}
// A numbered display is already a block with its own math, so it is not wrapped
// in dollars. The context is markup, outside math, so the display is written.
const typstMarkup = { format: 'typst', text: '', selection: { from: 0, to: 0, text: '' }, files: [] };
const numbered = templateInsertion(MATH_TEMPLATES.find(template => template.id === 'fraction'), { num: 'a', den: 'b', numbered: true, label: 'eq:x' }, typstMarkup).text;
assert.equal(numbered, '#math.equation(block: true, numbering: "(1)")[$ frac(a, b) $] <eq:x>');
const source = ['#set page(width: 300pt, height: auto)', ...spellings.map(spelling => `$${spelling}$`), ...templateSpellings.map(spelling => `$ ${spelling} $`), numbered].join('\n\n');

const { instance } = await WebAssembly.instantiate(readFileSync(new URL('../../wasm/typst.wasm', import.meta.url)), {});
const wasm = instance.exports;
handOver(wasm, { main: 'symbols.typ', texts: { 'symbols.typ': source }, assets: {} });
const result = call(wasm, 'compile', source, 'PDF fixture');
const errors = result.diagnostics.filter(item => item.severity === 'error');
assert.equal(errors.length, 0, JSON.stringify(errors));
assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
assert.ok(result.bytes.length > 0, 'the document compiles to a PDF');

const emoji = JSON.parse(readFileSync(new URL('../../src/lib/symbols/emoji.json', import.meta.url), 'utf8'));
assert.ok(emoji.emoji.length > 1000, `emoji count ${emoji.emoji.length}`);
assert.ok(emoji.groups.length >= 8, `group count ${emoji.groups.length}`);

console.log(`symbols-typst: ${spellings.length} Typst spellings and ${templateSpellings.length} template forms compiled; ${emoji.emoji.length} emoji in ${emoji.groups.length} groups`);
