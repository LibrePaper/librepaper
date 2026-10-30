import { EditorState, Extension, Facet, Range as CMRange, RangeSet } from '@codemirror/state';
import { Decoration, DecorationSet, EditorView, WidgetType } from '@codemirror/view';
import { Language, TokenStyle } from '../core/language';
import { Tokenizer } from '../core/tokenizer';
import { Range, Token } from '../core/tokens';
import { createWidget } from './widget-registry';
import { revealOnMousedown } from './widgets';

export interface BuildOptions {
  language: Language;
  showCommands: boolean;
  maxDepth?: number;
}

export interface BuildResult {
  decorations: DecorationSet;
  atomic: RangeSet<Decoration>;
  tokens: Token[];
}

const hidden = Decoration.replace({});

export interface RevealRange extends Range {
  color?: string;
}

export const revealRanges = Facet.define<readonly RevealRange[], readonly RevealRange[]>({
  combine: values => values.flat()
});

export function revealAt(positions: readonly number[], color?: string): Extension {
  return revealRanges.of(positions.map(pos => ({ from: pos, to: pos, color })));
}

export function revealFrom(
  deps: Parameters<typeof revealRanges.compute>[0],
  compute: (state: EditorState) => readonly RevealRange[]
): Extension {
  return revealRanges.compute(deps, compute);
}

class MarkerWidget extends WidgetType {
  constructor(
    private label: string,
    private className = 'cm-lv-marker'
  ) {
    super();
  }

  eq(other: WidgetType): boolean {
    return other instanceof MarkerWidget && other.label === this.label && other.className === this.className;
  }

  toDOM(): HTMLElement {
    const element = document.createElement('span');
    element.className = this.className;
    element.textContent = this.label;
    return element;
  }
}

// A run of fully hidden lines (for example \newpage or \end{document}) shows as one chip.
// Clicking it places the caret in the run, which reveals the source.
class HiddenRunWidget extends WidgetType {
  constructor(private summary: string) {
    super();
  }

  eq(other: WidgetType): boolean {
    return other instanceof HiddenRunWidget && other.summary === this.summary;
  }

  toDOM(view: EditorView): HTMLElement {
    const gap = document.createElement('div');
    gap.className = 'cm-lp-block-gap cm-lp-block-gap-tight';
    const element = document.createElement('div');
    element.className = 'cm-lp-hidden';
    element.textContent = this.summary;
    element.title = 'Show the source';
    gap.append(element);
    revealOnMousedown(view, gap);
    return gap;
  }

  // The click is handled by revealOnMousedown, which moves the caret into the run.
  ignoreEvent(): boolean {
    return true;
  }
}

interface HiddenRun extends Range {
  firstLine: number;
  lastLine: number;
}

// Finds runs of consecutive lines whose text is entirely hidden spans plus whitespace.
function hiddenRuns(state: EditorState, spans: Range[], replaced: Range[]): HiddenRun[] {
  const byLine = new Map<number, Range[]>();
  for (const span of spans) {
    let start = span.from;
    while (start < span.to) {
      const line = state.doc.lineAt(start);
      const end = Math.min(span.to, line.to);
      if (end > start) {
        const parts = byLine.get(line.number) ?? [];
        parts.push({ from: start, to: end });
        byLine.set(line.number, parts);
      }
      start = line.to + 1;
    }
  }

  // Sorted and merged, so the replaced range a line meets is a binary search
  // away rather than a scan of every widget in the document.
  const widgets = merge(replaced);
  const meetsWidget = (from: number, to: number) => {
    let low = 0;
    let high = widgets.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (widgets[middle].to <= from) low = middle + 1;
      else high = middle;
    }
    return low < widgets.length && widgets[low].from < to;
  };

  const full: number[] = [];
  for (const [number, parts] of byLine) {
    const line = state.doc.line(number);
    if (line.length === 0) continue;
    // A replaced range on this line (such as a widget) must stay visible, so the line is not collapsed.
    if (meetsWidget(line.from, line.to)) continue;

    const hiddenMask = new Array<boolean>(line.length).fill(false);
    for (const part of parts) {
      for (let pos = part.from; pos < part.to; pos++) hiddenMask[pos - line.from] = true;
    }
    const text = state.doc.sliceString(line.from, line.to);
    let whole = true;
    for (let index = 0; index < text.length; index++) {
      if (!hiddenMask[index] && !/\s/.test(text[index])) {
        whole = false;
        break;
      }
    }
    if (whole) full.push(number);
  }
  full.sort((a, b) => a - b);

  const runs: HiddenRun[] = [];
  for (const number of full) {
    const last = runs[runs.length - 1];
    if (last && number === last.lastLine + 1) {
      last.lastLine = number;
      last.to = state.doc.line(number).to;
    } else {
      const line = state.doc.line(number);
      runs.push({ from: line.from, to: line.to, firstLine: number, lastLine: number });
    }
  }
  return runs;
}

// A LaTeX preamble is source: the preamble fold draws it, folded or revealed
// as plain text, so nothing before \begin{document} is decorated here.
function bodyTokens(parsed: Token[]): Token[] {
  const body = parsed.findIndex(token => token.kind === 'container' && token.name === 'document');
  return body > 0 ? parsed.slice(body) : parsed;
}

// The lines drawn as plain source: every line a selection or a reveal range
// touches, widened to the whole of any block that is drawn as more than one
// line (an environment, list, callout, code block, figure, table, widget).
// The document environment covers the whole body and never widens.
export function revealedLines(state: EditorState, language: Language, parsed: Token[]): Set<number> {
  const doc = state.doc;
  const revealed = new Set<number>();
  for (const range of [...state.selection.ranges, ...state.facet(revealRanges)]) {
    const last = doc.lineAt(Math.min(range.to, doc.length)).number;
    for (let n = doc.lineAt(Math.min(range.from, doc.length)).number; n <= last; n++) revealed.add(n);
  }

  const blocks: Array<[number, number]> = [];
  const visit = (list: Token[]) => {
    for (const token of list) {
      const style = language.style(token);
      if (style && (style.block || style.widget) && !(token.kind === 'container' && token.name === 'document')) {
        const first = doc.lineAt(token.from).number;
        const end = token.to > token.from && doc.sliceString(token.to - 1, token.to) === '\n' ? token.to - 1 : token.to;
        const last = doc.lineAt(end).number;
        if (last > first) blocks.push([first, last]);
      }
      if (token.children) visit(token.children);
    }
  };
  visit(bodyTokens(parsed));

  // Widening can bring in lines of other blocks, so repeat until nothing is added.
  let changed = blocks.length > 0;
  while (changed) {
    changed = false;
    const sorted = [...revealed].sort((a, b) => a - b);
    for (const [first, last] of blocks) {
      let low = 0;
      let high = sorted.length;
      while (low < high) {
        const middle = (low + high) >> 1;
        if (sorted[middle] < first) low = middle + 1;
        else high = middle;
      }
      if (low === sorted.length || sorted[low] > last) continue;
      for (let n = first; n <= last; n++) {
        if (!revealed.has(n)) {
          revealed.add(n);
          changed = true;
        }
      }
    }
  }
  return revealed;
}

export function buildDecorations(state: EditorState, options: BuildOptions, tokens?: Token[]): BuildResult {
  const source = state.doc.toString();
  const parsed = tokens ?? new Tokenizer(options.language, { maxDepth: options.maxDepth }).tokenize(source);

  const decorations: CMRange<Decoration>[] = [];
  const atomic: CMRange<Decoration>[] = [];
  const replaced: Range[] = [];
  const concealed: Range[] = [];
  const revealed = revealedLines(state, options.language, parsed);

  const touchesRevealed = (from: number, to: number) => {
    if (revealed.size === 0) return false;
    const first = state.doc.lineAt(from).number;
    const last = state.doc.lineAt(to).number;
    if (last - first > revealed.size) {
      for (const n of revealed) if (n >= first && n <= last) return true;
      return false;
    }
    for (let n = first; n <= last; n++) if (revealed.has(n)) return true;
    return false;
  };

  const hide = (from: number, to: number) => {
    if (to <= from || options.showCommands) return;

    let start = from;
    while (start < to) {
      const line = state.doc.lineAt(start);
      const end = Math.min(to, line.to);
      if (end > start && !revealed.has(line.number)) concealed.push({ from: start, to: end });
      start = line.to + 1;
    }
  };

  // scopeEnd is where a declaration (\bfseries) stops: its enclosing group or environment body.
  const emit = (list: Token[], scopeEnd: number) => {
    for (const token of list) {
      const style = options.language.style(token);
      const childScope = token.body?.to ?? scopeEnd;

      if (!style) {
        if (token.children) emit(token.children, childScope);
        continue;
      }

      if (style.widget && !options.showCommands) {
        const widget = createWidget(style.widget, {
          token,
          source,
          state,
          language: options.language
        });
        const spansLines = state.doc.lineAt(token.from).number !== state.doc.lineAt(token.to).number;
        const block = Boolean(style.block);

        if (widget && (block || !spansLines)) {
          const decoration = Decoration.replace({ widget, block });
          decorations.push(decoration.range(token.from, token.to));
          atomic.push(decoration.range(token.from, token.to));
          replaced.push({ from: token.from, to: token.to });
          continue;
        }
      }

      applyStyle(state, decorations, token, style);

      if (style.declaration && token.to < scopeEnd) {
        decorations.push(Decoration.mark({ class: style.declaration }).range(token.to, scopeEnd));
      }

      // An environment header is replaced by its widget; the default concealment below still
      // hides the same range, which the replacement then overrides.
      if (style.header && token.body && !options.showCommands) {
        // A body that starts a line ends the header at the previous newline, or that line loses its line class.
        const atLineStart = state.doc.lineAt(token.body.from).from === token.body.from;
        const headTo = atLineStart && token.body.from > token.from ? token.body.from - 1 : token.body.from;
        const range = { from: token.from, to: headTo };
        const widget = createWidget(style.header, { token, source, state, language: options.language });
        const spansLines = state.doc.lineAt(range.from).number !== state.doc.lineAt(range.to).number;
        const startLine = state.doc.lineAt(range.from);
        const endLine = state.doc.lineAt(range.to);
        const block = range.from === startLine.from && (range.to === endLine.to || range.to === endLine.from);
        if (widget && (block || !spansLines)) {
          const decoration = Decoration.replace({ widget, block });
          decorations.push(decoration.range(range.from, range.to));
          atomic.push(decoration.range(range.from, range.to));
          replaced.push(range);
        }
      }

      // A widget drawn before the body. It is inline and not atomic, so the caret may sit beside it.
      if (style.before && !options.showCommands) {
        const widget = createWidget(style.before, { token, source, state, language: options.language });
        if (widget) {
          const at = token.body?.from ?? token.from;
          decorations.push(Decoration.widget({ widget, side: -1 }).range(at));
        }
      }

      if (style.footer !== undefined && token.body && !options.showCommands) {
        // Drawn after the last visible character of the body, not after a trailing newline.
        let position = token.body.to;
        while (position > token.body.from && /\s/.test(source[position - 1])) position--;
        const marker = new MarkerWidget(style.footer, 'cm-lp-footer');
        decorations.push(Decoration.widget({ widget: marker, side: 1 }).range(position));
      }

      if (style.hidden) {
        hide(token.from, token.to);
      } else if (style.replaceWith !== undefined) {
        if (!options.showCommands) {
          const marker = Decoration.replace({ widget: new MarkerWidget(style.replaceWith) });
          // A command, accent or ligature is replaced as a whole. An item keeps its optional label.
          const end = token.kind === 'item' ? token.body?.from ?? token.to : token.to;
          decorations.push(marker.range(token.from, end));
          replaced.push({ from: token.from, to: end });
        }
      } else if (!style.keepSyntax) {
        // \replaced{new}{old} conceals around the argument it shows.
        const body = style.argument !== undefined ? token.args?.[style.argument] : token.body;
        if (body) {
          hide(token.from, body.from);
          hide(body.to, token.to);
        }
      }

      if (token.children && !style.keepSyntax) emit(token.children, childScope);
    }
  };

  emit(bodyTokens(parsed), state.doc.length);

  // Lines drawn as source carry no decoration of ours. Points and replacements
  // that touch one are dropped; marks keep only their parts on other lines.
  const drawn: CMRange<Decoration>[] = [];
  for (const range of decorations) {
    const spec = range.value.spec;
    const isMark = range.from < range.to && (spec.class !== undefined || spec.attributes !== undefined);
    if (!isMark) {
      if (!touchesRevealed(range.from, range.to)) drawn.push(range);
      continue;
    }
    // A mark spanning several lines is cut at each revealed line.
    let start = -1;
    for (let line = state.doc.lineAt(range.from); ; line = state.doc.line(line.number + 1)) {
      if (revealed.has(line.number)) {
        if (start >= 0) drawn.push(range.value.range(start, state.doc.line(line.number - 1).to));
        start = -1;
      } else if (start < 0) {
        start = Math.max(range.from, line.from);
      }
      if (line.to >= range.to) {
        if (start >= 0 && Math.min(range.to, line.to) > start) drawn.push(range.value.range(start, Math.min(range.to, line.to)));
        break;
      }
    }
  }
  decorations.splice(0, decorations.length, ...drawn);
  atomic.splice(0, atomic.length, ...atomic.filter(range => !touchesRevealed(range.from, range.to)));
  replaced.splice(0, replaced.length, ...replaced.filter(range => !touchesRevealed(range.from, range.to)));

  const spans = subtract(merge(concealed), merge(replaced));

  // A collapsed run is one block replacement, so its spans get neither a per-span
  // replace nor a syntax-line decoration. Those would overlap the run's replacement.
  // The run is not added to `atomic`: the caret reaching its edge reveals the source.
  const collapsedLines = new Set<number>();
  for (const run of hiddenRuns(state, spans, replaced)) {
    for (let number = run.firstLine; number <= run.lastLine; number++) collapsedLines.add(number);
    const summary = options.language.hiddenSummary?.(state.doc.sliceString(run.from, run.to)) ?? null;
    const decoration = summary === null
      ? Decoration.replace({ block: true })
      : Decoration.replace({ widget: new HiddenRunWidget(summary), block: true });
    decorations.push(decoration.range(run.from, run.to));
  }

  for (const span of spans) {
    const line = state.doc.lineAt(span.from);
    if (collapsedLines.has(line.number)) continue;

    decorations.push(hidden.range(span.from, span.to));
    if (span.from === line.from && span.to === line.to && line.length > 0) {
      decorations.push(Decoration.line({ class: 'cm-lv-syntax-line' }).range(line.from));
    }
  }

  return {
    decorations: Decoration.set(decorations, true),
    atomic: RangeSet.of(atomic, true),
    tokens: parsed
  };
}

function applyStyle(
  state: EditorState,
  decorations: CMRange<Decoration>[],
  token: Token,
  style: TokenStyle
): void {
  if (!style.class) return;

  if (style.block) {
    const last = state.doc.lineAt(Math.max(token.from, Math.min(token.to, state.doc.length)));
    const first = state.doc.lineAt(token.from);
    // The edge lines carry classes so that their padding can replace the margin,
    // which CodeMirror does not measure.
    const line = (extra: string) =>
      Decoration.line({ class: `${style.class}${extra}`, attributes: style.attributes });
    const middle = line('');
    const top = line(' cm-lp-block-first');
    const bottom = line(' cm-lp-block-last');
    const single = line(' cm-lp-block-first cm-lp-block-last');

    let current = first;
    while (true) {
      const starts = current.number === first.number;
      const ends = current.number >= last.number;
      const decoration = starts && ends ? single : starts ? top : ends ? bottom : middle;
      decorations.push(decoration.range(current.from));
      if (ends) break;
      current = state.doc.line(current.number + 1);
    }
    return;
  }

  const body: Range = token.body ?? token;
  if (body.to > body.from) {
    decorations.push(
      Decoration.mark({ class: style.class, attributes: style.attributes }).range(body.from, body.to)
    );
  }
}

function expandToLine(state: EditorState, source: string, from: number, to: number): Range {
  const first = state.doc.lineAt(from);
  const last = state.doc.lineAt(to);

  const before = source.slice(first.from, from);
  const after = source.slice(to, last.to);

  if (/^\s*$/.test(before) && /^\s*$/.test(after) && last.to < source.length) {
    return { from: first.from, to: last.to + 1 };
  }

  return { from, to };
}

function merge(ranges: Range[]): Range[] {
  const sorted = [...ranges].sort((a, b) => a.from - b.from || a.to - b.to);
  const merged: Range[] = [];

  for (const range of sorted) {
    const last = merged[merged.length - 1];
    if (last && range.from <= last.to) last.to = Math.max(last.to, range.to);
    else merged.push({ ...range });
  }

  return merged;
}

function subtract(ranges: Range[], holes: Range[]): Range[] {
  // Both inputs are sorted and merged (see merge()), so one hole index can advance monotonically.
  const result: Range[] = [];
  let first = 0;

  for (const range of ranges) {
    while (first < holes.length && holes[first].to <= range.from) first++;

    let start = range.from;

    for (let index = first; index < holes.length && holes[index].from < range.to; index++) {
      const hole = holes[index];
      if (hole.to <= start) continue;
      if (hole.from > start) result.push({ from: start, to: hole.from });
      start = Math.max(start, hole.to);
    }

    if (start < range.to) result.push({ from: start, to: range.to });
  }

  return result;
}
