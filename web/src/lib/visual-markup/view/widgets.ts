import { EditorView, WidgetType } from '@codemirror/view';
import type { FigureModel } from '../core/language';
import { textOf, type Range } from '../core/tokens';
import { createEditableMath, readEditableMath, type MathSyntax } from './math-field';
import { ImageResolver, imageResolver, isExternal, resolveImagePath } from './images';
import { typst2tex } from 'tex2typst';
import { loadKatex } from '../../katex.js';
import { registerWidget, replaceRange, WidgetContext } from './widget-registry';

export class MathWidget extends WidgetType {
  private text: string;
  private content: string;
  private display: boolean;
  private open: string;
  private close: string;
  private syntax: MathSyntax;

  constructor(context: WidgetContext) {
    super();
    const { token, source } = context;
    this.text = textOf(source, token);
    this.content = token.body ? textOf(source, token.body) : this.text;
    this.display = Boolean(token.display);
    this.open = token.meta?.open ?? '$';
    this.close = token.meta?.close ?? '$';
    this.syntax = context.language.id === 'typst' ? 'typst' : 'latex';
  }

  eq(other: WidgetType): boolean {
    return other instanceof MathWidget && other.text === this.text && other.syntax === this.syntax;
  }

  toDOM(view: EditorView): HTMLElement {
    const container = document.createElement(this.display ? 'div' : 'span');
    container.className = `cm-lv-widget cm-lv-math ${this.display ? 'cm-lv-math-display' : 'cm-lv-math-inline'}`;

    const field = () => container.firstElementChild as (HTMLElement & { value?: string }) | null;

    const commit = () => {
      const element = field();
      if (!element || element.value === undefined) return;
      const value = readEditableMath(element, this.syntax);
      if (value === undefined) return;
      const content = this.syntax === 'typst' ? (this.display ? ` ${value.trim()} ` : value.trim()) : value;
      replaceRange(view, container, this.text, `${this.open}${content}${this.close}`);
    };

    container.addEventListener('click', event => {
      const element = field();
      if (!element || element.value === undefined) return;
      event.stopPropagation();
      element.focus();
    });

    container.addEventListener('focusout', event => {
      if (keepsFocus(event as FocusEvent, container)) return;
      commit();
    });

    container.addEventListener('change', commit);

    container.appendChild(createEditableMath(this.content.trim(), this.display, this.syntax));
    return container;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

export class ImageWidget extends WidgetType {
  private src: string;
  private alt: string;
  private destroyed = false;
  private style: string;

  constructor(context: WidgetContext) {
    super();
    this.src = context.language.imageSrc?.(context.source, context.token) ?? '';
    this.alt = this.src.slice(this.src.lastIndexOf('/') + 1);
    this.style = context.language.imageStyle?.(context.source, context.token) ?? '';
    this.resolver = context.state.facet(imageResolver);
  }

  private resolver: ImageResolver | null;

  eq(other: WidgetType): boolean {
    return other instanceof ImageWidget && other.src === this.src && other.style === this.style;
  }

  toDOM(view: EditorView): HTMLElement {
    const container = document.createElement('span');
    container.className = 'cm-lv-widget cm-lv-image';
    revealOnMousedown(view, container);

    const image = document.createElement('img');
    image.decoding = 'async';
    image.loading = 'lazy';
    image.alt = this.alt;
    image.title = this.src;
    if (this.style) image.setAttribute('style', this.style);
    container.appendChild(image);

    if (!this.src) {
      container.classList.add('cm-lv-image-missing');
      return container;
    }

    if (isExternal(this.src) || !this.resolver) {
      image.src = this.src;
      return container;
    }

    const path = resolveImagePath(this.resolver.currentPath(), this.src);

    Promise.resolve(this.resolver.resolve(path, this.src)).then(url => {
      if (this.destroyed) return;
      if (url) image.src = url;
      else container.classList.add('cm-lv-image-missing');
    });

    return container;
  }

  destroy(): void {
    this.destroyed = true;
  }

  ignoreEvent(): boolean {
    return true;
  }
}


export class FigureWidget extends WidgetType {
  private model: FigureModel;
  private key: string;
  private resolver: ImageResolver | null;
  private destroyed = false;
  private source: string;

  constructor(context: WidgetContext) {
    super();
    this.model = context.language.figure?.parse(context.source, context.token) ?? { panels: [] };
    this.source = context.source;
    this.key = JSON.stringify(this.model);
    this.resolver = context.state.facet(imageResolver);
  }

  eq(other: WidgetType): boolean {
    return other instanceof FigureWidget && other.key === this.key;
  }

  toDOM(view: EditorView): HTMLElement {
    const container = document.createElement('span');
    container.className = 'cm-lv-widget cm-lv-figure-preview';
    if (this.model.wide) container.classList.add('cm-lv-figure-wide');
    if (this.model.align) container.classList.add(`cm-lv-figure-${this.model.align}`);
    if (this.model.captionPosition === 'side') container.classList.add('cm-lv-figure-caption-side');
    if (this.model.width) container.style.setProperty('--lv-figure-width', this.model.width);
    container.style.setProperty('--lv-figure-columns', String(Math.max(1, this.model.columns ?? 1)));
    revealOnMousedown(view, container);

    const caption = this.model.caption ? figureCaption(
      view,
      this.model.caption,
      'cm-lv-figure-caption',
      this.model.captionRange,
      // Figures never edit captions in place; a click moves the caret to the caption source.
      false,
      this.source,
      this.model.captionTypst
    ) : null;
    if (caption && this.model.captionPosition === 'top') container.appendChild(caption);

    const body = document.createElement('span');
    body.className = 'cm-lv-figure-body';
    if (this.model.tracks?.length) body.style.gridTemplateColumns = this.model.tracks.join(' ');

    for (const panel of this.model.panels) {
      const panelDOM = document.createElement('span');
      panelDOM.className = 'cm-lv-figure-panel';

      const media = document.createElement('span');
      media.className = 'cm-lv-figure-media';
      for (const item of panel.images) media.appendChild(this.image(view, item.src, item.alt, item.style));
      panelDOM.appendChild(media);

      if (panel.caption) panelDOM.appendChild(figureCaption(
        view,
        panel.caption,
        'cm-lv-subcaption',
        panel.captionRange,
        // Figures never edit subcaptions in place; a click moves the caret to the caption source.
        false,
        this.source
      ));
      body.appendChild(panelDOM);
    }

    container.appendChild(body);
    if (caption && this.model.captionPosition !== 'top') container.appendChild(caption);
    return container;
  }

  private image(view: EditorView, src: string, alt = '', style = ''): HTMLImageElement {
    const image = document.createElement('img');
    image.decoding = 'async';
    image.loading = 'lazy';
    image.alt = alt;
    image.title = src;
    image.addEventListener('load', () => view.requestMeasure());
    image.addEventListener('error', () => view.requestMeasure());
    if (style) image.setAttribute('style', style);

    if (!src) return image;
    if (isExternal(src) || !this.resolver) {
      image.src = src;
      return image;
    }

    const path = resolveImagePath(this.resolver.currentPath(), src);
    Promise.resolve(this.resolver.resolve(path, src)).then(url => {
      if (!this.destroyed && url) image.src = url;
      else if (!url) image.classList.add('cm-lv-image-missing');
    });
    return image;
  }

  destroy(): void {
    this.destroyed = true;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

// Clicks on a widget must reach the editor as a caret move, not be swallowed by the widget.
// Clicking a widget moves the caret onto it, which reveals its source.
export function revealOnMousedown(view: EditorView, element: HTMLElement): void {
  element.addEventListener('mousedown', event => {
    event.preventDefault();
    view.dispatch({ selection: { anchor: view.posAtDOM(element) } });
    view.focus();
  });
}

function figureCaption(
  view: EditorView,
  text: string,
  className: string,
  range?: Range,
  editable = false,
  source = '',
  typst = ''
): HTMLElement {
  const caption = document.createElement('span');
  caption.className = className;
  if (typst) drawTypstInline(caption, typst);
  else caption.textContent = text;

  if (!range) return caption;

  if (!editable) {
    caption.classList.add('cm-lv-caption-source');
    caption.title = 'Click to edit caption source';
    caption.addEventListener('mousedown', event => {
      event.preventDefault();
      event.stopPropagation();
      view.dispatch({ selection: { anchor: range.from } });
      view.focus();
    });
    return caption;
  }

  caption.classList.add('cm-lv-caption-editor');
  caption.setAttribute('contenteditable', 'plaintext-only');
  caption.setAttribute('role', 'textbox');
  caption.setAttribute('aria-label', className.includes('subcaption') ? 'Edit subcaption' : 'Edit caption');
  caption.spellcheck = true;
  const original = text;
  const originalSource = source.slice(range.from, range.to);

  caption.addEventListener('mousedown', event => event.stopPropagation());
  caption.addEventListener('click', event => event.stopPropagation());
  caption.addEventListener('keydown', event => {
    if ((event as KeyboardEvent).key === 'Escape') {
      caption.textContent = original;
      caption.blur();
    }
  });
  caption.addEventListener('blur', () => {
    const next = caption.textContent ?? '';
    if (next === original) return;
    if (view.state.doc.sliceString(range.from, range.to) !== originalSource) return;
    view.dispatch({ changes: { from: range.from, to: range.to, insert: next } });
  });
  return caption;
}

// Draws Typst inline markup: *strong*, _emphasis_ and $maths$. Anything else stays text.
function drawTypstInline(parent: HTMLElement, text: string): void {
  const pattern = /\$([^$]+)\$|\*([^*]+)\*|(?<![\w])_([^_]+)_(?![\w])/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    parent.append(text.slice(last, match.index));
    last = match.index + match[0].length;
    if (match[1] !== undefined) {
      const math = document.createElement('span');
      math.className = 'cm-lv-math cm-lv-math-inline';
      math.textContent = match[0];
      let tex = '';
      try {
        tex = typst2tex(match[1].trim(), { blockMathMode: false });
      } catch {
        // The raw source stays.
      }
      if (tex) {
        loadKatex().then(katex => {
          if (math.isConnected) katex.render(tex, math, { displayMode: false, throwOnError: false });
        }).catch(() => {});
      }
      parent.append(math);
    } else {
      const mark = document.createElement(match[2] !== undefined ? 'strong' : 'em');
      drawTypstInline(mark, match[2] ?? match[3]);
      parent.append(mark);
    }
  }
  parent.append(text.slice(last));
}

// The text after the closing delimiter that matches the one at `open`, or null.
function balanced(source: string, open: number, pair: string): string | null {
  let depth = 0;
  for (let index = open; index < source.length; index++) {
    const char = source[index];
    if (char === '\\') index++;
    else if (char === pair[0]) depth++;
    else if (char === pair[1] && --depth === 0) return source.slice(open + 1, index);
  }
  return null;
}

// Markup reduced to the words it shows, for a one-line caption.
function plainCaption(text: string): string {
  return text
    .replace(/\\(?:emph|textbf|textit|texttt|textrm|textsc)\s*\{([^{}]*)\}/g, '$1')
    .replace(/\\[a-zA-Z]+\*?/g, '')
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[\\{}*_`$#~]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

// The caption of a table: Markdown keeps it on the token. A LaTeX table float
// and a Typst figure around a table are the token itself, so the caption is read
// from inside its range.
function tableCaption({ token, source, language }: WidgetContext): string {
  if (token.meta?.caption) return plainCaption(token.meta.caption);
  const text = source.slice(token.from, token.to);
  if (language.id === 'latex') {
    const caption = /\\caption\s*(?:\[[^\]]*\])?\s*\{/.exec(text);
    return caption ? plainCaption(balanced(text, caption.index + caption[0].length - 1, '{}') ?? '') : '';
  }
  if (language.id === 'typst' && token.meta?.figureTable === 'true') {
    const open = text.indexOf('(');
    const inner = open < 0 ? null : balanced(text, open, '()');
    const caption = inner === null ? null : /caption\s*:\s*\[/.exec(inner);
    return caption ? plainCaption(balanced(inner, caption.index + caption[0].length - 1, '[]') ?? '') : '';
  }
  return '';
}

// A block pill: one framed line standing in for source that is not drawn. A
// click shows the source. `kind` adds a class that tests and other code select.
export class PillWidget extends WidgetType {
  constructor(
    readonly text: string,
    readonly title = 'Show the source',
    private kind = ''
  ) {
    super();
  }

  eq(other: WidgetType): boolean {
    return other instanceof PillWidget && other.text === this.text && other.title === this.title && other.kind === this.kind;
  }

  toDOM(view: EditorView): HTMLElement {
    const gap = document.createElement('div');
    gap.className = 'cm-lp-pill-gap';
    const box = document.createElement('div');
    box.className = this.kind ? `cm-lp-pill ${this.kind}` : 'cm-lp-pill';
    box.title = this.title;
    box.textContent = this.text;
    gap.append(box);
    revealOnMousedown(view, gap);
    return gap;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

// A table is drawn as a pill: the caption when there is one, never the cells.
// A caret in the table or a click on the pill shows the source.
function tableBox(caption: string): PillWidget {
  return new PillWidget(caption ? `Table: ${caption}` : 'Table', 'Show the table source', 'cm-lp-table-box');
}

function keepsFocus(event: FocusEvent, container: HTMLElement): boolean {
  const target = event.relatedTarget as Node | null;
  if (!target) return false;
  if (container.contains(target)) return true;

  const keyboard = (window as unknown as { mathVirtualKeyboard?: { element?: HTMLElement } }).mathVirtualKeyboard;
  if (keyboard?.element?.contains(target)) return true;

  return Boolean((target as Element).closest?.('.ML__keyboard'));
}

registerWidget('math', context => new MathWidget(context));
registerWidget('image', context => new ImageWidget(context));
registerWidget('tablebox', context => tableBox(tableCaption(context)));
registerWidget('figure', context => context.language.figure ? new FigureWidget(context) : null);
