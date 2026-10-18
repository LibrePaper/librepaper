import assert from 'node:assert/strict';
import katex from 'katex';
import { CATEGORIES, SYMBOLS, searchSymbols, separated, symbolInsertion, symbolSpellings, symbolText } from '../../src/lib/symbols/index.js';
import { MATH_TEMPLATES, templateInsertion } from '../../src/lib/symbols/templates.js';
import { PICKER_SYMBOLS, entryAvailable, entryInsertion } from '../../src/lib/symbols/catalog.js';
import { RECENT_LIMIT, withFavorite, withRecent } from '../../src/lib/symbols/memory.js';

const MATH = new Set(['greek', 'operators', 'relations', 'arrows', 'logic', 'accents', 'delimiters']);
const PACKAGES = new Set(['amssymb', 'amsmath', 'emoji']);
const KINDS = new Set(['symbol', 'accent', 'delimiter', 'text', 'pair']);
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
assert.equal(symbolInsertion(byId('alpha'), { format: 'latex', text: '$x$', selection: { from: 1, to: 1, text: '' } }).text, '\\alpha ');
assert.equal(symbolInsertion(byId('alpha'), { format: 'typst', text: '$x$', selection: { from: 1, to: 1, text: '' } }).text, 'alpha ');
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

// Epsilon and phi mean the same letter in every format as their LaTeX command.
for (const [id, latex] of [['epsilon', '\\epsilon'], ['varepsilon', '\\varepsilon'], ['phi', '\\phi'], ['varphi', '\\varphi']]) {
  const drawn = katex.renderToString(latex, { output: 'html' }).match(/<span class="mord mathnormal">(.)<\/span>/);
  assert.ok(drawn, `KaTeX draws ${latex}`);
  assert.equal(byId(id).glyph, drawn[1], `${id} shows the glyph KaTeX draws for ${latex}`);
}
assert.equal(byId('epsilon').typst, 'epsilon.alt');
assert.equal(byId('varepsilon').typst, 'epsilon');
assert.equal(byId('epsilon').html, '&epsiv;');
assert.equal(byId('varphi').html, '&phi;');

// Separation from neighbouring text.
assert.equal(separated('\\alpha', 'latex', '', 'x'), '\\alpha ');
assert.equal(separated('\\alpha', 'latex', '', '$'), '\\alpha');
assert.equal(separated('alpha', 'typst', 'x', ''), ' alpha');
assert.equal(separated('alpha', 'typst', '', 'x'), 'alpha ');
assert.equal(separated('alpha', 'typst', 'x', 'y'), ' alpha ');
assert.equal(separated('&alpha;', 'html', 'a', 'b'), '&alpha;');
assert.equal(symbolInsertion(byId('alpha'), { format: 'latex', text: '$x$', selection: { from: 2, to: 2, text: '' } }).text, '\\alpha');
assert.equal(symbolInsertion(byId('alpha'), { format: 'typst', text: '$x$', selection: { from: 2, to: 2, text: '' } }).text, ' alpha');

// Packages for blackboard letters and amsmath operators.
assert.ok(symbolInsertion(byId('bb-r'), prose).additionalEdits.some(edit => edit.insert.includes('\\usepackage{amssymb}')), 'bb-r needs amssymb');
assert.ok(symbolInsertion(byId('iint'), prose).additionalEdits.some(edit => edit.insert.includes('\\usepackage{amsmath}')), 'iint needs amsmath');

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
assert.equal(aligned.text, '\\begin{align*}\nx &= y\n\\end{align*}\n');
assert.equal(templateInsertion(template('fraction'), {}, { format: 'html', text: '', selection: { from: 0, to: 0, text: '' } }).notes[0], 'Math templates need a math-capable format.');
// Template insertions are separated from their neighbours like symbols.
const fraction = { num: 'a', den: 'b' };
assert.equal(templateInsertion(template('fraction'), fraction, { format: 'typst', text: '$x$', selection: { from: 2, to: 2, text: '' }, files: [] }).text, ' frac(a, b)');
assert.equal(templateInsertion(template('fraction'), fraction, { format: 'typst', text: '$x$', selection: { from: 1, to: 1, text: '' }, files: [] }).text, 'frac(a, b)', 'a closing parenthesis needs no space before a letter');
assert.equal(templateInsertion(template('fraction'), fraction, { format: 'latex', text: '$x$', selection: { from: 1, to: 1, text: '' }, files: [] }).text, '\\frac{a}{b}');

// Scripts group a base of more than one token.
assert.equal(templateInsertion(template('superscript'), { base: 'x^2', sup: '2' }, inMath('latex')).text, '{x^2}^{2}');
assert.equal(templateInsertion(template('superscript'), { base: 'x', sup: '2' }, inMath('latex')).text, 'x^{2}');
assert.equal(templateInsertion(template('subscript'), { base: 'ab', sub: 'i' }, inMath('typst')).text, '(ab)_(i)');
assert.equal(templateInsertion(template('subsup'), { base: '\\alpha', sub: 'i', sup: '2' }, inMath('latex')).text, '\\alpha_{i}^{2}');
assert.equal(templateInsertion(template('superscript'), { sup: '2' }, inMath('latex')).text, '#{1:x}^{2}', 'a blank base is one field');

// Bar brackets bring amsmath, round ones need nothing.
assert.ok(templateInsertion(template('brackets'), { body: 'x', kind: 'bars' }, prose).additionalEdits.some(edit => edit.insert.includes('\\usepackage{amsmath}')), 'bars need amsmath');
assert.equal(templateInsertion(template('brackets'), { body: 'x', kind: 'parentheses' }, prose).additionalEdits.length, 0);

// The boundary is judged on the expanded text, not on the field marker.
const beforeA = { format: 'typst', text: '$a$', selection: { from: 1, to: 1, text: '' }, files: [] };
assert.equal(templateInsertion(template('integral'), { from: 'a', to: 'b', body: 'f' }, beforeA).text, 'integral_(a)^(b) f dif x ', 'the default variable runs into the a after the caret');
assert.equal(templateInsertion(template('sum'), { from: 'i=1', to: 'n' }, beforeA).text, 'sum_(i=1)^(n) #{1:x_i} ', 'a blank body is judged by its placeholder');

// Aligned equations use the align environment only in LaTeX prose, in display style.
const alignedRows = [{ left: 'x', right: 'y' }];
const markdownProse = { format: 'markdown', text: 'Hello', selection: { from: 5, to: 5, text: '' }, files: [] };
assert.equal(templateInsertion(template('aligned'), { rows: alignedRows, style: 'inline' }, markdownProse).text, '$\\begin{aligned}\nx &= y\n\\end{aligned}$');
assert.equal(templateInsertion(template('aligned'), { rows: alignedRows, style: 'display' }, markdownProse).text, '\n$$\\begin{aligned}\nx &= y\n\\end{aligned}$$');
assert.equal(templateInsertion(template('aligned'), { rows: alignedRows, style: 'display' }, prose).text, '\\begin{align*}\nx &= y\n\\end{align*}\n');
assert.equal(templateInsertion(template('aligned'), { rows: alignedRows, style: 'inline' }, prose).text, '$\\begin{aligned}\nx &= y\n\\end{aligned}$');

// A display goes on its own line, with one newline and not a blank line.
assert.equal(templateInsertion(template('fraction'), fraction, at('Some text', 4)).text, '\n\\[\n\\frac{a}{b}\n\\]\n', 'mid-line, the display gets a line of its own');
assert.equal(templateInsertion(template('fraction'), { ...fraction, style: 'inline' }, at('Some text', 4)).text, '$\\frac{a}{b}$', 'inline math stays in the line');

// Gathered equations follow the same rules with a gather environment.
const gatheredRows = [{ value: 'x = y' }, { value: 'z = w' }];
assert.equal(templateInsertion(template('gathered'), { rows: gatheredRows }, prose).text, '\\begin{gather*}\nx = y \\\\\nz = w\n\\end{gather*}\n');
assert.equal(templateInsertion(template('gathered'), { rows: [{ value: 'x' }], style: 'inline' }, prose).text, '$\\begin{gathered}\nx\n\\end{gathered}$');
assert.equal(templateInsertion(template('gathered'), { rows: gatheredRows, numbered: true, label: 'eq:g' }, prose).text, '\\begin{gather}\n\\label{eq:g}\nx = y \\\\\nz = w\n\\end{gather}\n');
assert.equal(templateInsertion(template('gathered'), { rows: [{ value: 'x' }] }, markdownProse).text, '\n$$\\begin{gathered}\nx\n\\end{gathered}$$');

// Numbering applies to display insertions outside math, and only the label is literal.
const numberedFraction = { num: 'a', den: 'b', numbered: true, label: 'eq:x' };
// Empty documents put nothing around the insertion, so the text is exact.
const typstEmpty = { format: 'typst', text: '', selection: { from: 0, to: 0, text: '' }, files: [] };
const quartoEmpty = { format: 'quarto', path: 'paper.qmd', text: '', selection: { from: 0, to: 0, text: '' }, files: [] };
assert.equal(templateInsertion(template('fraction'), numberedFraction, prose).text, '\\begin{equation}\n\\label{eq:x}\n\\frac{a}{b}\n\\end{equation}\n');
assert.equal(templateInsertion(template('fraction'), { num: 'a', den: 'b', numbered: true }, prose).text, '\\begin{equation}\n\\frac{a}{b}\n\\end{equation}\n', 'a number without a label has no \\label');
assert.equal(templateInsertion(template('fraction'), { ...numberedFraction, style: 'inline' }, prose).text, '$\\frac{a}{b}$', 'inline style ignores the number');
assert.equal(templateInsertion(template('aligned'), { rows: alignedRows, numbered: true, label: 'eq:x' }, prose).text, '\\begin{align}\n\\label{eq:x}\nx &= y\n\\end{align}\n');
assert.equal(templateInsertion(template('aligned'), { rows: alignedRows, numbered: true, label: 'eq x' }, prose).text, '\\begin{align}\n\\label{eq-x}\nx &= y\n\\end{align}\n', 'the LaTeX label is sanitized');
assert.equal(templateInsertion(template('fraction'), numberedFraction, typstEmpty).text, '#math.equation(block: true, numbering: "(1)")[$ frac(a, b) $] <eq:x>');
assert.equal(templateInsertion(template('fraction'), { num: 'a', den: 'b', numbered: true }, typstEmpty).text, '#math.equation(block: true, numbering: "(1)")[$ frac(a, b) $]');
assert.equal(templateInsertion(template('fraction'), { num: 'a', den: 'b', numbered: true }, quartoEmpty).text, '$$\\frac{a}{b}$$ {#eq-equation}', 'quarto numbers an unlabelled equation with a generated label');
assert.equal(templateInsertion(template('fraction'), { num: 'a', den: 'b', numbered: true, label: 'frac' }, quartoEmpty).text, '$$\\frac{a}{b}$$ {#eq-frac}', 'quarto prefixes the label once');
assert.equal(templateInsertion(template('fraction'), numberedFraction, markdownProse).text, '\n$$\\frac{a}{b}$$', 'markdown ignores the number');

// Search matches spellings, words and substrings, and keeps the best rank first.
assert.equal(searchSymbols('\\alpha', SYMBOLS)[0].id, 'alpha');
assert.equal(searchSymbols('alpha', SYMBOLS)[0].id, 'alpha');
assert.ok(searchSymbols('\\al', SYMBOLS).some(symbol => symbol.id === 'alpha'));
assert.ok(searchSymbols('infinity', SYMBOLS).some(symbol => symbol.id === 'infty'));
assert.ok(searchSymbols('less equal', SYMBOLS).some(symbol => symbol.id === 'leq'));
assert.ok(searchSymbols('right arrow', SYMBOLS).some(symbol => symbol.id === 'rightarrow'));
assert.ok(searchSymbols('arrow', SYMBOLS).filter(symbol => symbol.category === 'arrows').length >= 5);

// Spellings for the details panel: LaTeX and Typst, then the code point.
assert.deepEqual(symbolSpellings(byId('sum'), 'latex'), [
  { label: 'LaTeX', code: '\\sum' }, { label: 'Typst', code: 'sum' }, { label: 'Unicode', code: 'U+2211' },
]);
assert.equal(symbolSpellings(byId('sum'), 'html')[0].label, 'HTML');

// Recent and favorite lists: dedupe, move to the front, cap, and star or unstar.
assert.deepEqual(withRecent(['a', 'b', 'c'], 'b'), ['b', 'a', 'c']);
const manyRecent = Array.from({ length: RECENT_LIMIT + 5 }, (_, index) => `s${index}`);
const recentAfter = withRecent(manyRecent, 'new');
assert.equal(recentAfter.length, RECENT_LIMIT);
assert.equal(recentAfter[0], 'new');
assert.deepEqual(withFavorite(['a'], 'b', true), ['a', 'b']);
assert.deepEqual(withFavorite(['a', 'b'], 'a', false), ['b']);
assert.deepEqual(withFavorite(['a'], 'a', true), ['a']);

// Picker entries: pairs and templates go through the math templates.
const pickerEntry = id => {
  const entry = PICKER_SYMBOLS.find(item => item.id === id);
  assert.ok(entry, `picker entry ${id} exists`);
  return entry;
};
const latexAb = latexDocument.replace('Hello', 'ab');
const abStart = latexAb.indexOf('ab');
const selectedAb = { format: 'latex', path: 'main.tex', text: latexAb, selection: { from: abStart, to: abStart + 2, text: 'ab' }, files: [] };
const parentheses = pickerEntry('pair-parentheses');
assert.deepEqual(entryInsertion(parentheses, prose).text, '$\\left( #{1:x} \\right)$');
assert.equal(entryInsertion(parentheses, prose).snippet, true);
assert.equal(entryInsertion(parentheses, selectedAb).text, '$\\left( ab \\right)$');
assert.equal(entryInsertion(parentheses, selectedAb).snippet, false);
const htmlAb = { format: 'html', text: 'ab', selection: { from: 0, to: 2, text: 'ab' }, files: [] };
assert.deepEqual(entryInsertion(parentheses, htmlAb), { text: '(ab)', additionalEdits: [], notes: [], snippet: false, selection: { anchor: 1, head: 3 } });
const fractionTile = pickerEntry('template-fraction');
assert.equal(entryInsertion(fractionTile, prose).text, '$\\frac{#{1:a}}{#{2:b}}$');
assert.equal(entryInsertion(fractionTile, prose).snippet, true);
const htmlFraction = entryInsertion(fractionTile, { format: 'html', text: '', selection: { from: 0, to: 0, text: '' }, files: [] });
assert.equal(htmlFraction.text, '');
assert.equal(htmlFraction.notes.length, 1);
assert.equal(entryAvailable(fractionTile, 'html'), false);
assert.equal(entryAvailable(fractionTile, 'latex'), true);

console.log('symbols: data, search, insertion and template checks passed');
