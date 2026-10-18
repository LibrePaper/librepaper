// Compiles every Typst spelling of the math palette entries with the Typst
// WASM engine, the same way typst-pdf.mjs drives it. Needs the built engine
// in web/wasm; see typst-pdf.mjs for how it is fetched.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { call, handOver } from '../../src/lib/renderer-wasm.js';
import { SYMBOLS } from '../../src/lib/symbols/index.js';

const MATH = new Set(['greek', 'operators', 'relations', 'arrows', 'logic', 'accents', 'delimiters']);
const spellings = SYMBOLS
  .filter(symbol => MATH.has(symbol.category))
  .map(symbol => symbol.typst.replaceAll('#1', 'x'));
const source = ['#set page(width: 300pt, height: auto)', ...spellings.map(spelling => `$${spelling}$`)].join('\n\n');

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

console.log(`symbols-typst: ${spellings.length} Typst spellings compiled; ${emoji.emoji.length} emoji in ${emoji.groups.length} groups`);
