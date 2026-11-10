import { Rule } from '../../core/language';
import { Token } from '../../core/tokens';
import { STATE_OF, TASK_LINE } from '../../../tasks/syntax.js';
import { atBlockStart, atLineHead, lineEndAt, lineStartAt, parseAttributes } from './scan';

const CAPTION = /^[ \t]*(?:Table:|:)[ \t]+(.*?)[ \t]*$/;
const DELIMITER_ROW = /^[ \t]*\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;
const GRID_BORDER = /^[ \t]*\+[-=:+]+\+[ \t]*$/;
const GRID_ROW = /^[ \t]*[|+]/;
const FENCE_OPEN = /^(`{3,}|~{3,})(.*)$/;
const FENCE_CLOSE = /^[ \t]*(`{3,}|~{3,})[ \t]*$/;
const OPTION_LINE = /^[ \t]*#\|[ \t]*([\w-]+)[ \t]*:[ \t]*(.*?)[ \t]*$/;
export const TASK = /[ \t]+\[([ xX\/-])\](?=[ \t\n]|$)/y;
const FOOTNOTE_DEF = /\[\^([^\]\s^]+)\]:[ \t]*/y;
const LINK_DEF = /\[([^\]\n]+)\]:[ \t]*\S/y;
function unquote(value: string): string {
  return value.replace(/^"(.*)"$/, '$1');
}

// Start of the line after the one ending at `end` (its newline), or the end of the source.
function nextLineStart(source: string, end: number): number {
  return Math.min(end + 1, source.length);
}

// Fenced code. Quarto executable cells (```{r}) take their options from `#|` lines,
// which the header widget replaces together with the opening fence.
export const fence: Rule = (source, pos) => {
  const marker = source[pos];
  if ((marker !== '`' && marker !== '~') || !atBlockStart(source, pos)) return null;

  const headEnd = lineEndAt(source, pos);
  const open = FENCE_OPEN.exec(source.slice(pos, headEnd));
  if (!open) return null;
  const fenceText = open[1];
  if (marker === '`' && open[2].includes('`')) return null;
  const info = open[2].trim();

  let closeStart = source.length;
  let closeEnd = source.length;
  let closed = false;
  let cursor = headEnd;
  while (cursor < source.length) {
    const lineStart = cursor + 1;
    const lineEnd = lineEndAt(source, lineStart);
    const match = FENCE_CLOSE.exec(source.slice(lineStart, lineEnd));
    if (match && match[1][0] === marker && match[1].length >= fenceText.length) {
      closeStart = lineStart;
      closeEnd = lineEnd;
      closed = true;
      break;
    }
    cursor = lineEnd;
  }

  const bodyFrom = nextLineStart(source, headEnd);
  const cell = info.startsWith('{');
  const meta: Record<string, string> = {};
  let contentFrom = bodyFrom;

  if (cell) {
    meta.cell = 'true';
    meta.lang = /^\{\s*([A-Za-z][\w-]*)/.exec(info)?.[1] ?? '';
    // Inline options, `{r setup, include=FALSE}`: a bare first word is the chunk label.
    const options: [string, string][] = [];
    const inlineText = /^\{\s*[A-Za-z][\w-]*([^}]*)/.exec(info)?.[1] ?? '';
    const parts = inlineText.split(',').map((part) => part.trim()).filter(Boolean);
    for (const [index, part] of parts.entries()) {
      const eq = part.indexOf('=');
      if (eq < 0) {
        if (index === 0 && !meta.label) meta.label = part;
        continue;
      }
      const key = part.slice(0, eq).trim();
      const value = part.slice(eq + 1).trim().replace(/^(["'])(.*)\1$/, '$2');
      if (!key) continue;
      if (key === 'label') meta.label = value;
      else if (key === 'fig-cap' || key === 'tbl-cap' || key === 'fig.cap') meta.caption = value;
      else options.push([key, value]);
    }

    let line = bodyFrom;
    while (line < closeStart) {
      const lineEnd = lineEndAt(source, line);
      const option = OPTION_LINE.exec(source.slice(line, lineEnd));
      if (!option) break;
      const key = option[1];
      const value = unquote(option[2]);
      if (key === 'label') meta.label = value;
      else if (key === 'fig-cap' || key === 'tbl-cap') meta.caption = value;
      else options.push([key, value]);
      line = nextLineStart(source, lineEnd);
      contentFrom = Math.min(line, source.length);
    }
    if (options.length) meta.options = JSON.stringify(options);
  } else {
    meta.lang = info.split(/\s+/)[0] ?? '';
  }

  const bodyTo = !closed ? source.length : closeStart - 1 >= contentFrom ? closeStart - 1 : contentFrom;
  return {
    kind: 'container',
    name: 'codeblock',
    from: pos,
    to: closed ? closeEnd : source.length,
    body: { from: contentFrom, to: bodyTo },
    meta
  };
};

// Number of cells in a pipe table row: pipes at the ends are optional, escaped pipes do not split.
function cellCount(line: string): number {
  let text = line.trim();
  if (text.startsWith('|')) text = text.slice(1);
  if (text.endsWith('|') && !text.endsWith('\\|')) text = text.slice(0, -1);
  return text.split(/(?<!\\)\|/).length;
}

// End of the table that starts at `start`, or -1. Pipe tables need a delimiter row with
// as many cells as the header; grid tables are a run of border and row lines.
function tableEnd(source: string, start: number): number {
  const headEnd = lineEndAt(source, start);
  const head = source.slice(start, headEnd);

  if (GRID_BORDER.test(head)) {
    let end = headEnd;
    let cursor = headEnd;
    while (cursor < source.length) {
      const next = cursor + 1;
      const lineEnd = lineEndAt(source, next);
      if (!GRID_ROW.test(source.slice(next, lineEnd))) break;
      end = lineEnd;
      cursor = lineEnd;
    }
    return end;
  }

  if (!head.includes('|') || headEnd >= source.length) return -1;
  const delimStart = headEnd + 1;
  const delimEnd = lineEndAt(source, delimStart);
  const delimiter = source.slice(delimStart, delimEnd);
  if (!delimiter.includes('|') || !DELIMITER_ROW.test(delimiter) || cellCount(head) !== cellCount(delimiter)) return -1;

  let end = delimEnd;
  let cursor = delimEnd;
  while (cursor < source.length) {
    const next = cursor + 1;
    const lineEnd = lineEndAt(source, next);
    if (source.slice(next, lineEnd).trim() === '') break;
    end = lineEnd;
    cursor = lineEnd;
  }
  return end;
}

function tableToken(from: number, to: number, caption?: string): Token {
  const meta: Record<string, string> = {};
  if (caption !== undefined) {
    let text = caption;
    const attr = /\{([^{}\n]*)\}$/.exec(caption);
    if (attr) {
      const attributes = parseAttributes(attr[1]);
      if (attributes.id !== undefined) meta.id = attributes.id;
      text = caption.slice(0, attr.index).trim();
    }
    if (text) meta.caption = text;
  }
  return { kind: 'container', name: 'mdtable', from, to, meta };
}

// Pipe and grid tables, drawn whole. The caption line (`: Caption {#tbl-x}`) may come
// before the table or after it, and then belongs to the same token.
export const table: Rule = (source, pos) => {
  if (source[pos] === ' ' || source[pos] === '\t' || !atLineHead(source, pos)) return null;
  const lineEnd = lineEndAt(source, pos);
  const above = CAPTION.exec(source.slice(pos, lineEnd));
  if (above) {
    if (lineEnd >= source.length) return null;
    const end = tableEnd(source, lineEnd + 1);
    if (end === -1) return null;
    return tableToken(pos, end, above[1]);
  }

  const end = tableEnd(source, pos);
  if (end === -1) return null;

  // A caption below the table comes directly after it, or after one blank line.
  let after = end + 1;
  if (after < source.length && source.slice(after, lineEndAt(source, after)).trim() === '') {
    after = lineEndAt(source, after) + 1;
  }
  if (after < source.length) {
    const afterEnd = lineEndAt(source, after);
    const below = CAPTION.exec(source.slice(after, afterEnd));
    if (below) return tableToken(pos, afterEnd, below[1]);
  }
  return tableToken(pos, end);
};

// ATX heading. A trailing `{#id .class}` is hidden by the default concealment (it sits
// after the body) and read into meta.
export const heading: Rule = (source, pos, ctx) => {
  if (source[pos] !== '#' || !atBlockStart(source, pos)) return null;
  let level = 0;
  while (source[pos + level] === '#') level++;
  if (level > 6) return null;
  const after = source[pos + level];
  if (after !== undefined && after !== ' ' && after !== '\t' && after !== '\n') return null;

  const lineEnd = lineEndAt(source, pos);
  let from = pos + level;
  while (from < lineEnd && (source[from] === ' ' || source[from] === '\t')) from++;
  let to = lineEnd;

  const closing = /(^|[ \t]+)#+[ \t]*$/.exec(source.slice(from, to));
  if (closing) to = from + closing.index;
  while (to > from && (source[to - 1] === ' ' || source[to - 1] === '\t')) to--;

  const meta: Record<string, string> = {};
  const attr = /\{([^{}\n]*)\}$/.exec(source.slice(from, to));
  if (attr && /[#.=]/.test(attr[1])) {
    const attributes = parseAttributes(attr[1]);
    if (attributes.id !== undefined) meta.id = attributes.id;
    if (attributes.classes.length) meta.classes = attributes.classes.join(' ');
    to = from + attr.index;
    while (to > from && (source[to - 1] === ' ' || source[to - 1] === '\t')) to--;
  }

  return {
    kind: 'heading',
    level,
    from: pos,
    to: lineEnd,
    body: { from, to },
    children: ctx.parse(from, to),
    meta: Object.keys(meta).length ? meta : undefined
  };
};

// Horizontal rule: three or more of one marker, spaces allowed, alone on its line.
export const hr: Rule = (source, pos) => {
  const marker = source[pos];
  if ((marker !== '-' && marker !== '*' && marker !== '_') || !atLineHead(source, pos)) return null;
  const line = source.slice(pos, lineEndAt(source, pos)).replace(/[ \t]/g, '');
  if (line.length < 3 || line.split('').some(char => char !== marker)) return null;
  return { kind: 'command', name: 'hr', from: pos, to: lineEndAt(source, pos) };
};

// "done/total" over the task descendants of the item at pos, or null when it has none. Following
// lines belong to the item while they are blank or indented past its marker. Cancelled tasks are
// left out of both numbers.
export function taskProgress(source: string, pos: number): string | null {
  const column = pos - lineStartAt(source, pos);
  let done = 0;
  let total = 0;
  let lineFrom = lineEndAt(source, pos) + 1;
  while (lineFrom <= source.length) {
    const lineTo = lineEndAt(source, lineFrom);
    const line = source.slice(lineFrom, lineTo);
    // Quote markers do not count as indentation, so a quoted list reads the same as a plain one.
    const content = line.replace(/^[ \t>]*/, '');
    if (content.trim() !== '') {
      const indent = line.length - content.length;
      if (indent <= column) break;
      const child = TASK_LINE.exec(line);
      if (child && child[1].length > column) {
        const state = (STATE_OF as Record<string, string>)[child[2]];
        if (state !== 'cancelled') {
          total++;
          if (state === 'done') done++;
        }
      }
    }
    lineFrom = lineTo + 1;
  }
  return total > 0 ? `${done}/${total}` : null;
}

// Footnote definition `[^1]: text`, with indented continuation lines.
export const footnoteDefinition: Rule = (source, pos, ctx) => {
  if (source[pos] !== '[' || source[pos + 1] !== '^' || !atBlockStart(source, pos)) return null;
  FOOTNOTE_DEF.lastIndex = pos;
  const match = FOOTNOTE_DEF.exec(source);
  if (!match) return null;

  const bodyFrom = pos + match[0].length;
  let end = lineEndAt(source, pos);
  let cursor = end;
  while (cursor < source.length) {
    const next = cursor + 1;
    const lineEnd = lineEndAt(source, next);
    const text = source.slice(next, lineEnd);
    if (text.trim() === '') {
      cursor = lineEnd;
      continue;
    }
    if (!/^( {4}|\t)/.test(text)) break;
    end = lineEnd;
    cursor = lineEnd;
  }

  return {
    kind: 'container',
    name: 'fndef',
    from: pos,
    to: end,
    body: { from: bodyFrom, to: end },
    children: ctx.parse(bodyFrom, end),
    meta: { label: match[1] }
  };
};

// Link reference definition `[ref]: url`. It is source only, hidden from the view.
export const linkDefinition: Rule = (source, pos) => {
  if (source[pos] !== '[' || source[pos + 1] === '^' || !atLineHead(source, pos)) return null;
  LINK_DEF.lastIndex = pos;
  const match = LINK_DEF.exec(source);
  if (!match) return null;
  return { kind: 'raw', name: 'linkdef', from: pos, to: lineEndAt(source, pos), meta: { label: match[1] } };
};
