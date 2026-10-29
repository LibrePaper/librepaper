import type { WidgetKind } from '../../core/language';
import type { Render } from '../latex/catalog';

// How a Typst function call draws. Keys are full dotted names (figure.caption); the dispatcher
// falls back to the first segment (calc.min -> calc) when a dotted name has no entry of its own.
export interface FunctionSpec { render: Render }

type FunctionRow = [name: string, render: Render];

const mark = (cls: string): Render => ({ kind: 'mark', class: cls });
const symbol = (text: string): Render => ({ kind: 'symbol', text });
const chip = (text: string, flags: { block?: boolean; argument?: boolean } = {}): Render => ({ kind: 'chip', text, ...flags });
const widget = (kind: WidgetKind, block = false): Render => (block ? { kind: 'widget', widget: kind, block } : { kind: 'widget', widget: kind });
const hide: Render = { kind: 'hide' };
const source: Render = { kind: 'source' };

// Containers whose content is shown without their layout (alignment, padding, offsets).
const container: Render = mark('');

// Builds a map from rows, failing loudly if a name is listed twice (a later row would silently win).
function build(rows: FunctionRow[], label: string): Map<string, FunctionSpec> {
  const map = new Map<string, FunctionSpec>();
  for (const [name, render] of rows) {
    if (map.has(name)) throw new Error(`Duplicate ${label} in the Typst catalog: ${name}`);
    map.set(name, { render });
  }
  return map;
}

const FUNCTION_ROWS: FunctionRow[] = [
  // Section 2: document structure and model
  ['heading', source], // the dispatcher draws headings from their markup
  ['par', container],
  ['parbreak', hide],
  ['strong', mark('cm-lv-bold')],
  ['emph', mark('cm-lv-italic')],
  ['list', source],
  ['list.item', source],
  ['enum', source],
  ['enum.item', source],
  ['terms', source],
  ['terms.item', source],
  ['figure', source], // the dispatcher applies its figure handling
  ['figure.caption', source],
  ['footnote', widget('footnote')],
  ['footnote.entry', source],
  ['link', widget('link')],
  ['ref', widget('typref')],
  ['cite', widget('typref')],
  ['bibliography', chip('Bibliography', { block: true, argument: true })],
  ['outline', chip('Table of contents', { block: true })], // the dispatcher names it "List of figures" for target: figure
  ['outline.entry', source],
  ['numbering', source],
  ['quote', widget('quote')], // block when the block argument is true
  ['title', widget('typtitle', true)],
  ['document', hide],
  ['divider', chip('Divider', { block: true })],
  ['asset', source],
  ['label', source],

  // Section 3: text appearance and inline typography
  ['text', mark('cm-lp-text')],
  ['raw', mark('cm-lv-mono')],
  ['underline', mark('cm-lv-underline')],
  ['strike', mark('cm-lp-strike')],
  ['highlight', mark('cm-lp-highlight')],
  ['smallcaps', mark('cm-lv-smallcaps')],
  ['super', mark('cm-lp-sup')],
  ['sub', mark('cm-lp-sub')],
  ['overline', mark('cm-lp-overline')],
  ['upper', mark('cm-lp-uppercase')],
  ['lower', mark('cm-lp-lowercase')],
  ['smartquote', source],
  ['linebreak', symbol('↵')],
  ['lorem', chip('Placeholder text')],

  // Section 5: figures, images and transformed graphics
  ['image', widget('image')],
  ['image.decode', source],
  ['rotate', container],
  ['scale', container],
  ['skew', container],

  // Section 6: tables and grids (cell and rule internals are drawn by the table adapter)
  ['table', source],
  ['table.cell', source],
  ['table.header', source],
  ['table.footer', source],
  ['table.hline', source],
  ['table.vline', source],
  ['grid', source],
  ['grid.cell', source],
  ['grid.hline', source],
  ['grid.vline', source],

  // Section 7: paragraph and page layout
  ['page', source],
  ['pagebreak', chip('Page break', { block: true })],
  ['colbreak', chip('Column break', { block: true })],
  ['columns', container],
  ['align', container],
  ['block', container],
  ['box', container],
  ['h', hide],
  ['v', hide],
  ['pad', container],
  ['stack', container],
  ['place', container],
  ['move', container],
  ['hide', hide],
  ['repeat', container],
  ['layout', source],
  ['measure', source],

  // Section 8: drawing, fills and colors
  ['rect', chip('Shape')],
  ['square', chip('Shape')],
  ['circle', chip('Shape')],
  ['ellipse', chip('Shape')],
  ['polygon', chip('Shape')],
  ['line', chip('Drawing')],
  ['curve', chip('Drawing')],
  ['rgb', widget('swatch')],
  ['luma', widget('swatch')],
  ['cmyk', widget('swatch')],
  ['hsl', widget('swatch')],
  ['hsv', widget('swatch')],
  ['color.mix', widget('swatch')],
  ['gradient.linear', chip('Gradient')],
  ['gradient.radial', chip('Gradient')],
  ['gradient.conic', chip('Gradient')],
  ['tiling', chip('Pattern')],

  // Section 9 and 10: styling, selectors, scripting
  ['include', widget('typinclude', true)],
  ['context', source],
  ['selector', source],
  ['str', source],
  ['int', source],
  ['float', source],
  ['array', source],
  ['dictionary', source],
  ['eval', source],
  ['type', source],
  ['repr', source],
  ['assert', hide],
  ['panic', source],
  ['regex', source],
  ['plugin', source],
  ['sys.inputs', source],
  ['sys.version', source],

  // Section 11: introspection, state and counters
  ['counter', source],
  ['counter.display', source],
  ['counter.get', source],
  ['counter.at', source],
  ['counter.update', source],
  ['counter.step', source],
  ['state', source],
  ['state.get', source],
  ['state.update', source],
  ['here', source],
  ['locate', source],
  ['query', source],
  ['metadata', hide], // attaches a value that draws nothing
  ['location', source],

  // Section 12: data files
  ['read', source],
  ['csv', source],
  ['json', source],
  ['yaml', source],
  ['toml', source],
  ['xml', source],
  ['cbor', source],

  // Section 14: low-level types and utilities (methods such as array.map fall back to their family)
  ['calc', source],
  ['calc.min', source],
  ['calc.max', source],
  ['calc.round', source],
  ['calc.abs', source],
  ['calc.clamp', source],
  ['calc.rem', source],
  ['datetime', source],
  ['datetime.today', source],
  ['datetime.display', source],
  ['duration', source],
  ['bytes', source],
  ['path', source],
  ['target', source],
  ['version', source],

  // Section 15: specialized output (preserved as source)
  ['pdf', source],
  ['pdf.attach', source],
  ['pdf.artifact', source],
  ['pdf.header-cell', source],
  ['pdf.data-cell', source],
  ['html', source],
  ['html.elem', source],
  ['html.frame', source],
];

export const TYPST_FUNCTIONS: Map<string, FunctionSpec> = build(FUNCTION_ROWS, 'function');
