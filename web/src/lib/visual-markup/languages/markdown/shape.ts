import type { SyntaxNode } from '@lezer/common';

// Small tables and functions that give meaning to nodes of the Pandoc Markdown tree. Each takes the
// text of one node (or a few offsets); none searches the source for syntax.

export interface Attributes {
  id?: string;
  classes: string[];
}

const XREF_PREFIX = /^(fig|tbl|sec|eq|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|lst)-/i;
const BRACKET_CITE = /^(-?)@([A-Za-z0-9_](?:[A-Za-z0-9_:.\-]*[A-Za-z0-9_])?)$/;
const OPTION_LINE = /^[ \t]*#\|[ \t]*([\w-]+)[ \t]*:[ \t]*(.*?)[ \t]*$/;

export function lineStartAt(source: string, pos: number): number {
  return pos === 0 ? 0 : source.lastIndexOf('\n', pos - 1) + 1;
}

export function lineEndAt(source: string, pos: number): number {
  const end = source.indexOf('\n', pos);
  return end === -1 ? source.length : end;
}

// True when only white space precedes pos on its line. A block widget cannot start in the middle of
// a line, so a block inside a quote or after a list marker is drawn as plain text.
export function startsLine(source: string, pos: number): boolean {
  return source.slice(lineStartAt(source, pos), pos).trim() === '';
}

// True when nothing but white space shares the line with the range.
export function aloneOnLine(source: string, from: number, to: number): boolean {
  return startsLine(source, from) && source.slice(to, lineEndAt(source, to)).trim() === '';
}

export function skipBlanks(source: string, pos: number, end: number): number {
  while (pos < end && (source[pos] === ' ' || source[pos] === '\t')) pos++;
  return pos;
}

export function trimEnd(source: string, from: number, to: number): number {
  while (to > from && (source[to - 1] === ' ' || source[to - 1] === '\t')) to--;
  return to;
}

// The id and classes of an Attributes node.
export function attributesOf(source: string, node: SyntaxNode | null): Attributes {
  const attributes: Attributes = { classes: [] };
  if (!node) return attributes;
  const id = node.getChild('AttributeId');
  if (id) attributes.id = source.slice(id.from + 1, id.to);
  for (const cls of node.getChildren('AttributeClass')) {
    const text = source.slice(cls.from, cls.to);
    attributes.classes.push(text.startsWith('.') ? text.slice(1) : text);
  }
  return attributes;
}

// A cross-reference key's kind and the key with that kind lower-cased, or null for an ordinary citation key.
export function crossReference(key: string): { kind: string; key: string; capital: boolean } | null {
  const prefix = XREF_PREFIX.exec(key);
  if (!prefix) return null;
  const kind = prefix[1].toLowerCase();
  return { kind, key: kind + key.slice(prefix[1].length), capital: key[0] !== key[0].toLowerCase() };
}

// The token of a bracketed citation [@a; @b] or a lone cross-reference [-@fig-x], from the text between the brackets.
export function bracketCitation(content: string): { name: 'xref' | 'mdcite'; meta: Record<string, string> } {
  const single = BRACKET_CITE.exec(content.trim());
  const reference = single ? crossReference(single[2]) : null;
  if (single && reference) {
    const meta: Record<string, string> = { key: reference.key, prefix: reference.kind };
    if (single[1] === '-') meta.bare = 'true';
    if (reference.capital) meta.capital = 'true';
    return { name: 'xref', meta };
  }
  return { name: 'mdcite', meta: { text: content, bracketed: 'true' } };
}

// The token of an author-in-text key `@key`, from the key without its @.
export function bareCitation(key: string): { name: 'xref' | 'mdcite'; meta: Record<string, string> } {
  const reference = crossReference(key);
  if (reference) {
    const meta: Record<string, string> = { key: reference.key, prefix: reference.kind };
    if (reference.capital) meta.capital = 'true';
    return { name: 'xref', meta };
  }
  return { name: 'mdcite', meta: { text: `@${key}`, bracketed: 'false' } };
}

// A URL node's text without the angle brackets of `<url>`.
export function unangle(text: string): { skip: number; text: string } {
  return text.startsWith('<') && text.endsWith('>') && text.length >= 2 ? { skip: 1, text: text.slice(1, -1) } : { skip: 0, text };
}

// A bare address as the sentence sees it: trailing punctuation and an unbalanced parenthesis are the sentence's.
export function bareUrlText(text: string): string | null {
  if (!/^https?:\/\//.test(text)) return null;
  let url = text.replace(/[.,;:!?'"*_~]+$/, '');
  while (url.endsWith(')') && url.split('(').length < url.split(')').length) url = url.slice(0, -1);
  return url.length > 'https://'.length ? url : null;
}

function unquote(value: string): string {
  return value.replace(/^"(.*)"$/, '$1');
}

// A Quarto `#| key: value` line inside a code cell.
export function optionLine(line: string): [string, string] | null {
  const match = OPTION_LINE.exec(line);
  return match ? [match[1], unquote(match[2])] : null;
}

function storeOption(meta: Record<string, string>, options: [string, string][], key: string, value: string, captions: string[]): void {
  if (key === 'label') meta.label = value;
  else if (captions.includes(key)) meta.caption = value;
  else options.push([key, value]);
}

// The meta of a fenced code block from its info string, and of a cell from its `#|` lines (in `lines`).
// Cells, whose info starts with a brace, take their options from both.
export function codeMeta(info: string, lines: string[]): Record<string, string> {
  const meta: Record<string, string> = {};
  if (!info.startsWith('{')) {
    meta.lang = info.split(/\s+/)[0] ?? '';
    return meta;
  }
  meta.cell = 'true';
  meta.lang = /^\{\s*([A-Za-z][\w-]*)/.exec(info)?.[1] ?? '';
  // Inline options, `{r setup, include=FALSE}`: a bare first word is the chunk label.
  const options: [string, string][] = [];
  const inlineText = /^\{\s*[A-Za-z][\w-]*([^}]*)/.exec(info)?.[1] ?? '';
  const parts = inlineText.split(',').map(part => part.trim()).filter(Boolean);
  for (const [index, part] of parts.entries()) {
    const eq = part.indexOf('=');
    if (eq < 0) {
      if (index === 0 && !meta.label) meta.label = part;
      continue;
    }
    const key = part.slice(0, eq).trim();
    const value = part.slice(eq + 1).trim().replace(/^(["'])(.*)\1$/, '$2');
    if (key) storeOption(meta, options, key, value, ['fig-cap', 'tbl-cap', 'fig.cap']);
  }
  for (const line of lines) {
    const option = optionLine(line);
    if (option) storeOption(meta, options, option[0], option[1], ['fig-cap', 'tbl-cap']);
  }
  if (options.length) meta.options = JSON.stringify(options);
  return meta;
}
