import assert from 'node:assert/strict';
import katex from 'katex';
import { CATEGORIES, SYMBOLS, searchSymbols, symbolInsertion, symbolText } from '../../src/lib/symbols/index.js';
import { MATH_TEMPLATES, templateInsertion } from '../../src/lib/symbols/templates.js';

const MATH = new Set(['greek', 'operators', 'relations', 'arrows', 'logic', 'accents', 'delimiters']);
const PACKAGES = new Set(['amssymb', 'amsmath', 'emoji']);
const KINDS = new Set(['symbol', 'accent', 'delimiter', 'text']);
const CATEGORY_IDS = new Set(CATEGORIES.map(category => category.id));

const ids = SYMBOLS.map(symbol => symbol.id);
assert.equal(new Set(ids).size, ids.length, 'ids are unique');
for (const symbol of SYMBOLS) {
  for (const key of ['id', 'glyph', 'name', 'category', 'kind', 'latex', 'typst']) {
    assert.equal(typeof symbol[key], 'string', `${symbol.id} has a string ${key}`);
    assert.notEqual(symbol[key], '', `${symbol.id} has a non-empty ${key}`);
  }
  assert.ok(Array.isArray(symbol.aliases), `${symbol.id} has aliases`);
  assert.match(symbol.id, /^[a-z0-9-]+$/, `${symbol.id} is a lowercase id`);
  assert.ok(CATEGORY_IDS.has(symbol.category), `${symbol.id} has a known category`);
  assert.ok(KINDS.has(symbol.kind), `${symbol.id} has a known kind`);
  if (symbol.package !== undefined) assert.ok(PACKAGES.has(symbol.package), `${symbol.id} has a known package`);
  if (symbol.pair) assert.equal(symbol.pair.latex.length, 2, `${symbol.id} pairs two latex delimiters`);
}

for (const symbol of SYMBOLS.filter(entry => MATH.has(entry.category))) {
  const spelling = symbol.latex.replaceAll('#1', 'x');
  assert.doesNotThrow(() => katex.renderToString(spelling, { throwOnError: true }), `KaTeX renders ${symbol.latex}`);
}

// Search: exact and prefix matches rank above substrings, and ties keep order.
const eps = searchSymbols('eps', SYMBOLS).map(symbol => symbol.id);
assert.deepEqual(eps.slice(0, 2), ['epsilon', 'varepsilon']);
assert.ok(searchSymbols('implies', SYMBOLS).some(symbol => symbol.id === 'double-rightarrow'));
assert.ok(searchSymbols('=>', SYMBOLS).some(symbol => symbol.id === 'double-rightarrow'));
assert.equal(searchSymbols('\u03B1', SYMBOLS)[0].id, 'alpha');
assert.equal(searchSymbols('', SYMBOLS), SYMBOLS);

const byId = id => {
  const symbol = SYMBOLS.find(entry => entry.id === id);
  assert.ok(symbol, `symbol ${id} exists`);
  return symbol;
};
const at = (text, offset) => ({ format: 'latex', path: 'main.tex', text, selection: { from: offset, to: offset, text: '' }, files: [] });
const inMath = (format, body = '$x$') => ({ format, text: body, selection: { from: 1, to: 1, text: '' }, files: [] });
const latexDocument = '\\documentclass{article}\n\\begin{document}\n\nHello\n\\end{document}';
const prose = at(latexDocument, latexDocument.indexOf('Hello'));

// Symbol insertion.
assert.equal(symbolInsertion(byId('alpha'), prose).text, '$\\alpha$');
assert.equal(symbolInsertion(byId('alpha'), { format: 'latex', text: '$x$', selection: { from: 1, to: 1, text: '' } }).text, '\\alpha');
assert.equal(symbolInsertion(byId('alpha'), { format: 'typst', text: '$x$', selection: { from: 1, to: 1, text: '' } }).text, 'alpha');
assert.equal(symbolInsertion(byId('alpha'), { format: 'typst', text: 'Hello', selection: { from: 5, to: 5, text: '' } }).text, '$alpha$');
assert.equal(symbolInsertion(byId('leq'), { format: 'html', text: 'Hello', selection: { from: 0, to: 0, text: '' } }).text, '&le;');
assert.equal(symbolInsertion(byId('alpha'), { format: 'markdown', text: 'Hello', selection: { from: 0, to: 0, text: '' } }).additionalEdits.length, 0);
const nexists = symbolInsertion(byId('nexists'), prose);
assert.ok(nexists.additionalEdits.some(edit => edit.insert.includes('\\usepackage{amssymb}')), 'amssymb edit');
assert.equal(symbolInsertion(byId('hat'), inMath('latex')).text, '\\hat{#{1:x}}', 'accent without selection is a snippet');
assert.equal(symbolInsertion(byId('hat'), { format: 'latex', text: '$x$', selection: { from: 1, to: 2, text: 'x' } }).text, '\\hat{x}');
assert.equal(symbolInsertion(byId('langle'), { format: 'latex', text: '$x$', selection: { from: 1, to: 2, text: 'x' } }).text, '\\left\\langle x \\right\\rangle');
const thumbsUp = {
  id: 'emoji-thumbs-up', glyph: '\u{1F44D}', name: 'thumbs up', aliases: ['+1'], category: 'emoji', kind: 'symbol',
  group: 'people-and-body', latex: '\\emoji{thumbs-up}', package: 'emoji', typst: '\u{1F44D}', html: '\u{1F44D}',
};
const emoji = symbolInsertion(thumbsUp, prose);
assert.equal(emoji.text, '\\emoji{thumbs-up}');
assert.ok(emoji.additionalEdits.some(edit => edit.insert.includes('\\usepackage{emoji}')), 'emoji package edit');
assert.ok(emoji.notes.includes('The emoji package needs LuaLaTeX.'));
assert.equal(symbolInsertion(byId('text-endash'), prose).text, '--');
assert.equal(symbolInsertion(byId('text-endash'), { format: 'typst', text: 'Hi', selection: { from: 0, to: 0, text: '' } }).text, '\u2013');
assert.equal(symbolText(byId('alpha'), 'typst'), 'alpha');
assert.equal(symbolText(byId('leq'), 'html'), '&le;');
assert.equal(symbolText(byId('text-endash'), 'latex', { inMath: false }), '\u2013');

// Templates.
const template = id => MATH_TEMPLATES.find(entry => entry.id === id);
assert.equal(templateInsertion(template('fraction'), { num: 'a', den: 'b' }, inMath('latex')).text, '\\frac{a}{b}');
assert.equal(templateInsertion(template('fraction'), { num: 'a', den: 'b' }, inMath('typst')).text, 'frac(a, b)');
const blank = templateInsertion(template('fraction'), {}, inMath('latex'));
assert.equal(blank.text, '\\frac{#{1:a}}{#{2:b}}');
assert.equal(blank.snippet, true);
const matrix = templateInsertion(template('matrix'), { brackets: 'square', rows: 2, columns: 2, cells: [['a', 'b'], ['c', 'd']] }, inMath('latex'));
assert.equal(matrix.text, '\\begin{bmatrix}\na & b \\\\\nc & d\n\\end{bmatrix}');
assert.equal(templateInsertion(template('matrix'), { brackets: 'square', cells: [['a', 'b'], ['c', 'd']] }, inMath('typst')).text, 'mat(delim: "[", a, b; c, d)');
const cases = templateInsertion(template('cases'), { rows: [{ value: '1', condition: 'x > 0' }, { value: '0', condition: 'x \\le 0' }] }, inMath('latex'));
assert.equal(cases.text, '\\begin{cases}\n1 & x > 0 \\\\\n0 & x \\le 0\n\\end{cases}');
const aligned = templateInsertion(template('aligned'), { rows: [{ left: 'x', right: 'y' }] }, at('Hello', 0));
assert.equal(aligned.text, '\\begin{align}\nx &= y\n\\end{align}');
assert.equal(templateInsertion(template('fraction'), {}, { format: 'html', text: '', selection: { from: 0, to: 0, text: '' } }).notes[0], 'Math templates need a math-capable format.');

console.log('symbols: data, search, insertion and template checks passed');
