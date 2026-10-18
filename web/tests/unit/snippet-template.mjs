import assert from 'node:assert/strict';
import { snippet } from '@codemirror/autocomplete';
import { EditorState } from '@codemirror/state';
import { snippetTemplate } from '../../src/lib/snippet-template.js';

// Run the template through CodeMirror's own parser. The dispatched value is a
// transaction, so the text and the main range come from its new state.
const apply = (template) => {
  const state = EditorState.create({ doc: '' });
  let tr = null;
  snippet(template)({ state, dispatch: (t) => { tr = t; } }, null, 0, 0);
  const main = tr.state.selection.main;
  return { text: tr.state.doc.toString(), from: main.from, to: main.to };
};

assert.equal(snippetTemplate('\\left\\{ #{1:x} \\right\\}'), '\\left\\\\{ #{1:x} \\right\\\\}');
assert.equal(snippetTemplate('\\frac{#{1:a}}{#{2:b}}'), '\\frac{#{1:a}}{#{2:b}}', 'bare braces are left for the parser');
assert.equal(snippetTemplate('plain text'), 'plain text');
assert.equal(snippetTemplate('${#{1:x}}$'), '$\\{#{1:x}}$', 'a literal ${ is escaped');

assert.deepEqual(apply(snippetTemplate('\\left\\{ #{1:x} \\right\\}')), { text: '\\left\\{ x \\right\\}', from: 8, to: 9 });
assert.deepEqual(apply(snippetTemplate('\\frac{#{1:a}}{#{2:b}}')), { text: '\\frac{a}{b}', from: 6, to: 7 });
assert.deepEqual(apply(snippetTemplate('${#{1:x}}$')), { text: '${x}$', from: 2, to: 3 });

// The parser removes the markers, then unescapes braces. The result must match
// the text with its markers replaced by their names.
const unescape = (s) => s.replace(/#\{\d+:([^{}]*)\}/g, '$1').replace(/\\([{}])/g, '$1');
const strip = (t) => t.replace(/#\{\d+:([^{}]*)\}/g, '$1');

for (const t of ['\\begin{cases}\n#{1:1} & #{2:x > 0}\n\\end{cases}', '$\\left\\{ #{1:x} \\right\\}$']) {
  assert.equal(unescape(snippetTemplate(t)), strip(t));
}

console.log('snippet-template: brace escaping and round-trip checks passed');
