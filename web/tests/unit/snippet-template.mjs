import assert from 'node:assert/strict';
import { snippetTemplate } from '../../src/lib/snippet-template.js';

assert.equal(snippetTemplate('\\left\\{ #{1:x} \\right\\}'), '\\left\\\\{ #{1:x} \\right\\\\}');
assert.equal(snippetTemplate('\\frac{#{1:a}}{#{2:b}}'), '\\frac{#{1:a}}{#{2:b}}', 'bare braces are left for the parser');
assert.equal(snippetTemplate('plain text'), 'plain text');

// The parser removes the markers, then unescapes braces. The result must match
// the text with its markers replaced by their names.
const unescape = (s) => s.replace(/#\{\d+:([^{}]*)\}/g, '$1').replace(/\\([{}])/g, '$1');
const strip = (t) => t.replace(/#\{\d+:([^{}]*)\}/g, '$1');

for (const t of ['\\begin{cases}\n#{1:1} & #{2:x > 0}\n\\end{cases}', '$\\left\\{ #{1:x} \\right\\}$']) {
  assert.equal(unescape(snippetTemplate(t)), strip(t));
}

console.log('snippet-template: brace escaping and round-trip checks passed');
