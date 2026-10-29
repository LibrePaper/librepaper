import { cssColor } from '../../core/color';
import { registerLanguage, Language, LanguageCommands, TokenStyle } from '../../core/language';
import { Token } from '../../core/tokens';
import type { Render } from '../latex/catalog';
import { TYPST_FUNCTIONS } from './catalog';
import { TYPST_EMOJI, TYPST_SYMBOLS } from './symbols';
import { rules } from './rules';
import { table } from './table';
import { figure, typstImageStyle } from './figure';

const commands: LanguageCommands = {
  wrap: {
    bold: ['*', '*'],
    italic: ['_', '_'],
    underline: ['#underline[', ']'],
    mono: ['`', '`'],
    smallcaps: ['#smallcaps[', ']'],
    inlineMath: ['$', '$'],
    displayMath: ['$ ', ' $'],
    quote: ['#quote[', ']']
  },
  heading(level, text) {
    return `${'='.repeat(Math.max(1, level))} ${text}`;
  },
  list(kind, items) {
    const marker = kind === 'number' ? '+' : '-';
    return items.map(entry => `${marker} ${entry}`).join('\n');
  },
  table(rows, cols) {
    const cells = Array.from({ length: rows * cols }, (_, index) => `[Cell ${index + 1}]`);
    const lines: string[] = [];
    for (let row = 0; row < rows; row++) {
      lines.push(`  ${cells.slice(row * cols, row * cols + cols).join(', ')},`);
    }
    return `#table(\n  columns: ${cols},\n${lines.join('\n')}\n)`;
  },
  color(kind, color, text) {
    const value = /^#[0-9a-f]{6}$/i.test(color) ? `rgb("${color}")` : color;
    return kind === 'text' ? `#text(fill: ${value})[${text}]` : `#highlight(fill: ${value})[${text}]`;
  }
};

// Typst's predefined colours, as Typst defines them (not the CSS keywords of the same name).
const TYPST_COLORS = new Map<string, string>([
  ['black', '#000000'], ['gray', '#aaaaaa'], ['silver', '#dddddd'], ['white', '#ffffff'],
  ['navy', '#001f3f'], ['blue', '#0074d9'], ['aqua', '#7fdbff'], ['teal', '#39cccc'],
  ['eastern', '#239dad'], ['purple', '#b10dc9'], ['fuchsia', '#f012be'], ['maroon', '#85144b'],
  ['red', '#ff4136'], ['orange', '#ff851b'], ['yellow', '#ffdc00'], ['olive', '#3d9970'],
  ['green', '#2ecc40'], ['lime', '#01ff70']
]);

const TYPST_WEIGHTS = new Map<string, number>([
  ['thin', 100], ['extralight', 200], ['light', 300], ['regular', 400], ['medium', 500],
  ['semibold', 600], ['bold', 700], ['extrabold', 800], ['black', 900]
]);

// Statements that draw nothing and fold into the hidden run's setup chip.
const HIDDEN_STATEMENTS = new Set(['let', 'set', 'show', 'import']);

const UNKNOWN: TokenStyle = { class: 'cm-lv-cmd-unknown', keepSyntax: true };

// Removes the quotes of a Typst string literal; other values come back trimmed.
function unquote(value: string): string {
  const match = /^\s*["']([\s\S]*)["']\s*$/.exec(value);
  return (match ? match[1] : value).trim();
}

// A CSS colour for a Typst colour argument, or null when it is not a simple literal.
function typstColor(raw?: string): string | null {
  if (!raw) return null;
  const value = raw.trim();
  const named = TYPST_COLORS.get(value);
  if (named) return named;

  const hex = /^#((?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8}))$/i.exec(value);
  if (hex) return `#${hex[1]}`;

  const quoted = /^rgb\(\s*["']#((?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8}))["']\s*\)$/i.exec(value);
  if (quoted) return `#${quoted[1]}`;

  const channels = /^rgb\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*\)$/.exec(value);
  if (channels) {
    const parts = channels.slice(1).map(Number);
    if (parts.every(part => part <= 255)) return `rgb(${parts.join(', ')})`;
  }
  return null;
}

// Keeps the value only when it is a plain length (14pt, 1.2em), which CSS reads the same way.
function typstLength(raw?: string): string | null {
  if (!raw) return null;
  const match = /^(\d*\.?\d+)(pt|em|in|cm|mm|%)$/.exec(unquote(raw));
  return match ? `${match[1]}${match[2]}` : null;
}

function typstWeight(raw?: string): string | null {
  if (!raw) return null;
  const value = unquote(raw);
  const named = TYPST_WEIGHTS.get(value);
  if (named !== undefined) return String(named);
  return /^\d{3}$/.test(value) && Number(value) >= 100 && Number(value) <= 900 ? value : null;
}

function typstFontStyle(raw?: string): string | null {
  if (!raw) return null;
  const value = unquote(raw);
  return value === 'italic' || value === 'oblique' || value === 'normal' ? value : null;
}

// The first font name, restricted to characters that are safe inside a style attribute.
function typstFamily(raw?: string): string | null {
  if (!raw) return null;
  const match = /["']([^"']+)["']/.exec(raw);
  return match && /^[A-Za-z0-9 ._-]+$/.test(match[1]) ? `"${match[1].trim()}"` : null;
}

// CSS declarations for the named arguments of #text(...), or undefined when there are none.
function textAttributes(meta: Record<string, string> | undefined): Record<string, string> | undefined {
  const declarations: string[] = [];
  const color = typstColor(meta?.['arg:fill']) ?? cssColor(meta?.color);
  if (color) declarations.push(`color:${color}`);
  const size = typstLength(meta?.['arg:size']);
  if (size) declarations.push(`font-size:${size}`);
  const weight = typstWeight(meta?.['arg:weight']);
  if (weight) declarations.push(`font-weight:${weight}`);
  const fontStyle = typstFontStyle(meta?.['arg:style']);
  if (fontStyle) declarations.push(`font-style:${fontStyle}`);
  const family = typstFamily(meta?.['arg:font']);
  if (family) declarations.push(`font-family:${family}`);
  return declarations.length ? { style: declarations.join(';') } : undefined;
}

// The catalog entry for a dotted name, then for its first segment (table.cell uses table).
function catalogRender(name: string): Render | null {
  const entry = TYPST_FUNCTIONS.get(name) ?? TYPST_FUNCTIONS.get(name.split('.')[0]);
  return entry?.render ?? null;
}

function renderStyle(render: Render): TokenStyle {
  switch (render.kind) {
    case 'mark':
      return { class: render.class };
    case 'declaration':
      return { hidden: true, declaration: render.class };
    case 'hide':
      return { hidden: true };
    case 'symbol':
      return { replaceWith: render.text };
    case 'chip':
      return { widget: 'typchip', block: render.block };
    case 'widget':
      return { widget: render.widget, block: render.block };
    case 'source':
      return UNKNOWN;
  }
}

function headingLevel(meta: Record<string, string> | undefined): number {
  const level = Number(meta?.['arg:level']);
  return Number.isInteger(level) && level >= 1 ? Math.min(level, 3) : 1;
}

function style(token: Token): TokenStyle | null {
  switch (token.kind) {
    case 'comment':
      return { class: 'cm-lv-comment' };
    case 'math':
      return { widget: 'math', block: token.display };
    case 'heading':
      return { class: `cm-lv-heading cm-lv-h${Math.min(token.level ?? 1, 3)}`, block: true, before: 'headnum' };
    case 'item':
      return { replaceWith: token.name === 'number' ? '1.' : '•' };
    case 'raw':
      if (token.name === 'argument' || token.name === 'separator') return { hidden: true };
      if (token.name === 'string') return { class: 'cm-lv-string' };
      // An escaped character shows without its backslash; the lexer puts the character in meta.text.
      if (token.name === 'escape') {
        const text = token.meta?.text;
        return text === undefined ? null : { replaceWith: text };
      }
      return token.name ? { class: 'cm-lv-mono' } : null;
    case 'container':
      // A raw block draws like a Markdown code block: its language line becomes the cellhead
      // header and its closing fence collapses through the default concealment.
      if (token.name === 'codeblock') {
        return token.meta?.lang
          ? { class: 'cm-lp-code', block: true, header: 'cellhead' }
          : { class: 'cm-lp-code', block: true };
      }
      if (token.meta?.figureCaption === 'true') return { class: 'cm-lv-caption' };
      return { class: 'cm-lv-content' };
    case 'table':
      return { class: 'cm-lv-table', block: true };
    case 'tie':
      return { replaceWith: ' ' };
    case 'ligature':
    case 'accent': {
      const text = token.meta?.text;
      return text === undefined ? null : { replaceWith: text };
    }
    case 'linebreak':
      return { replaceWith: '↵' };
    // Brackets are hidden by the default concealment of a body; the content stays visible.
    case 'group':
      return { class: '' };
    case 'command': {
      const name = token.name ?? '';
      if (name === 'image') return { widget: 'image' };
      if (token.meta?.figureComposite === 'true') return { widget: 'figure', block: true, keepSyntax: true };
      if (token.meta?.figure === 'true') {
        const classes = ['cm-lv-figure', 'cm-lv-figure-simple'];
        if (token.meta.figureWide === 'true') classes.push('cm-lv-figure-wide');
        if (token.meta.figureCaptionTop === 'true') classes.push('cm-lv-figure-caption-top');
        return { class: classes.join(' '), block: true };
      }
      if (token.meta?.statement) {
        if (HIDDEN_STATEMENTS.has(name)) return { hidden: true };
        if (name === 'include') return { widget: 'typinclude' };
        // if, for, while, context, code blocks and expressions show as code.
        return { class: 'cm-lp-code-inline', keepSyntax: true };
      }
      // A call without a content block, such as #pagebreak(), #lorem(5) or #rgb("#f00"),
      // is still a chip or widget when the catalog says so; other such calls show their arguments.
      const callRender = (TYPST_FUNCTIONS.get(name) ?? TYPST_FUNCTIONS.get(name.split('.')[0]))?.render;
      if (token.meta?.args === 'code' && callRender?.kind === 'chip') return { widget: 'typchip', block: callRender.block };
      if (token.meta?.args === 'code' && callRender?.kind === 'widget') return { widget: callRender.widget, block: callRender.block };
      if (token.meta?.args === 'code') return {};
      if (name === 'sym' || name === 'emoji') {
        const glyph = (name === 'sym' ? TYPST_SYMBOLS : TYPST_EMOJI).get(token.meta?.path ?? '');
        return glyph === undefined ? UNKNOWN : { replaceWith: glyph };
      }
      if (name === 'label') return { widget: 'typlabel' };
      if (name === 'ref' || name === 'cite') return { widget: 'typref' };
      if (name === 'url') return { widget: 'link' };
      if (name === 'text') return { class: 'cm-lp-text', attributes: textAttributes(token.meta) };
      if (name === 'heading') return { class: `cm-lv-heading cm-lv-h${headingLevel(token.meta)}`, block: true, before: 'headnum' };
      if (name === 'highlight') {
        const color = cssColor(token.meta?.color);
        if (color) return { class: 'cm-lv-colorbox', attributes: { style: `background-color:${color}` } };
      }
      const render = catalogRender(name);
      return render ? renderStyle(render) : UNKNOWN;
    }
    default:
      return null;
  }
}

// Names a fully hidden run of source (set, show, import and let statements) for its chip.
function hiddenSummary(text: string): string | null {
  if (/^\s*#pagebreak\s*\(\s*\)\s*$/.test(text)) return 'Page break';

  const count = (name: string): number => (text.match(new RegExp(`#${name}\\b`, 'g')) ?? []).length;
  const template = /#show\s*:\s*([A-Za-z_][\w.]*)\s*(?:\.with\s*\(|$)/m.exec(text);
  const sets = count('set');
  const shows = count('show') - (template ? 1 : 0);
  const imports = count('import');
  const definitions = count('let');

  const setup: string[] = [];
  if (sets) setup.push(`${sets} set ${sets === 1 ? 'rule' : 'rules'}`);
  if (shows) setup.push(`${shows} show ${shows === 1 ? 'rule' : 'rules'}`);
  if (imports) setup.push(`${imports} ${imports === 1 ? 'import' : 'imports'}`);
  if (definitions) setup.push(`${definitions} ${definitions === 1 ? 'definition' : 'definitions'}`);

  const parts: string[] = [];
  if (template) parts.push(`Template: ${template[1]}`);
  if (setup.length) parts.push(`Setup: ${setup.join(', ')}`);
  if (parts.length) return parts.join(' · ');

  // Anything else in a hidden run is named by its calls, in source order.
  const names = new Set<string>();
  for (const match of text.matchAll(/#([A-Za-z][\w.]*)/g)) names.add(`#${match[1]}`);
  return names.size ? [...names].join(', ') : null;
}

export const typst: Language = {
  id: 'typst',
  name: 'Typst',
  rules,
  style,
  commands,
  table,
  figure,
  colorCommands: ['text', 'highlight'],
  imageSrc(source, token) {
    const args = token.args?.[0];
    if (!args) return null;

    const match = /^\s*["']([^"']+)["']/.exec(source.slice(args.from, args.to));
    return match ? match[1] : null;
  },
  imageStyle: typstImageStyle,
  hiddenSummary
};

export { rules } from './rules';

registerLanguage(typst);
