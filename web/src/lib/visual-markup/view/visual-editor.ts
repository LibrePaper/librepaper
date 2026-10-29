import { foldedRanges } from '@codemirror/language';
import { Compartment, EditorState, Extension, Facet, Prec, RangeSet, StateEffect, StateField } from '@codemirror/state';
import { Decoration, DecorationSet, EditorView, keymap, ViewPlugin, ViewUpdate } from '@codemirror/view';
import { ConfigService, DEFAULT_CONFIG, EditorConfig } from '../core/config';
import { getLanguage } from '../core/language';
import { Token } from '../core/tokens';
import { buildDecorations, revealRanges } from './decorations';
import { createTheme } from './theme';
import './widgets';

export interface VisualEditorOptions {
  language?: string;
  showCommands?: boolean;
  onModeChange?: (mode: 'source' | 'visual') => void;
  config?: Partial<EditorConfig>;
}

interface VisualState {
  enabled: boolean;
  showCommands: boolean;
  language: string;
  maxDepth: number;
}

export const setVisualState = StateEffect.define<Partial<VisualState>>();
export const refreshVisual = StateEffect.define<null>();

// Rebuilding on every keystroke re-parses the whole document. Waiting for a
// pause in typing costs one rebuild per pause instead of one per key.
const REBUILD_DELAY = 100;

interface VisualDecorations {
  decorations: DecorationSet;
  atomic: RangeSet<Decoration>;
  // Null while a rebuild is pending (the document changed since the parse).
  tokens: Token[] | null;
}

// Seeds the initial visual state, so a host can turn the visual mode on from
// configuration rather than with a dispatch after the state exists.
export const visualDefaults = Facet.define<Partial<VisualState>, Partial<VisualState>>({
  combine: values => Object.assign({}, ...values)
});

const visualState = StateField.define<VisualState>({
  create: state => ({
    enabled: false,
    showCommands: DEFAULT_CONFIG.showCommands,
    language: DEFAULT_CONFIG.language,
    maxDepth: DEFAULT_CONFIG.maxDepth,
    ...state.facet(visualDefaults)
  }),
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setVisualState)) value = { ...value, ...effect.value };
    }
    return value;
  }
});

const emptyDecorations: VisualDecorations = {
  decorations: Decoration.none,
  atomic: RangeSet.empty,
  tokens: null
};

const decorationField = StateField.define<VisualDecorations>({
  create: state => compute(state),
  update(value, tr) {
    const state = tr.state.field(visualState);
    if (!state.enabled) return emptyDecorations;

    if (
      tr.effects.some(effect => effect.is(setVisualState) || effect.is(refreshVisual)) ||
      tr.startState.facet(revealRanges) !== tr.state.facet(revealRanges)
    ) {
      return compute(tr.state);
    }

    // Typing maps the old decorations and leaves the rebuild to the plugin
    // below. The text around the caret is shown as source anyway.
    if (tr.docChanged) {
      return {
        decorations: value.decorations.map(tr.changes),
        atomic: value.atomic.map(tr.changes),
        tokens: null
      };
    }

    // A cursor move keeps the parse. A stale mapped set cannot be used here,
    // since its atomic ranges would make the caret jump over stale widgets.
    if (tr.selection) {
      return value.tokens === null ? compute(tr.state) : compute(tr.state, value.tokens);
    }

    return value;
  },
  provide: field => [
    EditorView.decorations.from(field, value => value.decorations),
    EditorView.atomicRanges.of(view => view.state.field(field).atomic)
  ]
});

function compute(state: EditorState, tokens?: Token[]): VisualDecorations {
  const options = state.field(visualState, false);
  if (!options?.enabled) return emptyDecorations;

  const result = buildDecorations(
    state,
    {
      language: getLanguage(options.language),
      showCommands: options.showCommands,
      maxDepth: options.maxDepth
    },
    tokens
  );

  return { decorations: result.decorations, atomic: result.atomic, tokens: result.tokens };
}

// Runs the full rebuild once typing pauses, so a fresh parse follows the
// mapped decorations. Dispatching from update() would re-enter the view, so
// the dispatch waits for a timer.
const rebuildAfterPause = ViewPlugin.fromClass(
  class {
    private timer: ReturnType<typeof setTimeout> | undefined;

    // A state put back into a view (an editor keeping one state per file) can
    // arrive with its rebuild still pending.
    constructor(private view: EditorView) {
      if (view.state.field(visualState).enabled && view.state.field(decorationField).tokens === null) this.schedule();
    }

    update(update: ViewUpdate): void {
      if (!update.docChanged || !update.state.field(visualState).enabled) return;
      this.schedule();
    }

    private schedule(): void {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => {
        this.timer = undefined;
        if (this.view.state.field(decorationField).tokens === null) {
          this.view.dispatch({ effects: refreshVisual.of(null) });
        }
      }, REBUILD_DELAY);
    }

    destroy(): void {
      clearTimeout(this.timer);
    }
  }
);

function isFoldedAt(state: EditorState, pos: number): boolean {
  let folded = false;
  foldedRanges(state).between(pos, pos, (from, to) => {
    if (from <= pos && pos < to) folded = true;
  });
  return folded;
}

// CodeMirror moves the caret over replaced (collapsed or widget) lines without
// stopping on them, so their source is never revealed from the keyboard. When
// a vertical move skips a document line, land on that line instead. Only a
// skip is intercepted; ordinary motion keeps the default behaviour.
function stepLine(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  if (!state.field(visualState).enabled) return false;
  const { selection, doc } = state;
  if (selection.ranges.length !== 1 || !selection.main.empty) return false;

  const line = doc.lineAt(selection.main.head);
  const next = forward ? line.number + 1 : line.number - 1;
  if (next < 1 || next > doc.lines) return false;

  const target = view.moveVertically(selection.main, forward);
  const landed = doc.lineAt(target.head).number;
  const skipped = forward ? landed > next : landed < next;
  if (!skipped) return false;

  // A folded range hides the line on purpose, so the default motion stays.
  const skippedLine = doc.line(next);
  if (isFoldedAt(state, skippedLine.from)) return false;

  view.dispatch({
    selection: { anchor: forward ? skippedLine.from : skippedLine.to },
    scrollIntoView: true,
    userEvent: 'select'
  });
  return true;
}

// CodeMirror's atomic ranges make a horizontal move jump a whole inline widget
// in one key press, so the caret never sits inside it. A jump longer than one
// character on the same line steps one position into the range instead.
function stepChar(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  if (!state.field(visualState).enabled) return false;
  const { selection, doc } = state;
  if (selection.ranges.length !== 1 || !selection.main.empty) return false;

  const head = selection.main.head;
  const target = view.moveByChar(selection.main, forward);
  if (doc.lineAt(target.head).number !== doc.lineAt(head).number) return false;
  if (Math.abs(target.head - head) <= 1) return false;
  // Only a jump over one of this layer's atomic widgets is stepped into: a long
  // move can also be a single character made of several code units (an emoji, a
  // letter with combining marks), where a step of one would split it.
  let widget = false;
  state.field(decorationField).atomic.between(head, head, (from, to) => {
    if (forward ? from === head && to > head : to === head && from < head) widget = true;
  });
  if (!widget) return false;

  view.dispatch({
    selection: { anchor: forward ? head + 1 : head - 1 },
    scrollIntoView: true,
    userEvent: 'select'
  });
  return true;
}

// Prec.high runs before the default cursor keymap. Only the plain keys are
// bound, so Shift and modifier variants keep their default behaviour.
const arrowKeymap = Prec.high(
  keymap.of([
    { key: 'ArrowDown', run: view => stepLine(view, true) },
    { key: 'ArrowUp', run: view => stepLine(view, false) },
    { key: 'ArrowRight', run: view => stepChar(view, true) },
    { key: 'ArrowLeft', run: view => stepChar(view, false) }
  ])
);

export function visualExtension(): Extension {
  return [visualState, decorationField, rebuildAfterPause, arrowKeymap];
}

// The tokens of the parse the visual layer last drew, or null when there is no
// visual layer or its parse is waiting for a rebuild after an edit.
export function visualTokens(state: EditorState): Token[] | null {
  return state.field(decorationField, false)?.tokens ?? null;
}

export class VisualCodeMirrorEditor {
  private view: EditorView;
  private options: VisualEditorOptions;
  private configService: ConfigService;
  private enabled = false;
  private themeCompartment = new Compartment();
  private unsubscribe: () => void;

  constructor(view: EditorView, options: VisualEditorOptions = {}) {
    this.view = view;
    this.options = options;
    this.configService = new ConfigService({
      ...options.config,
      showCommands: options.showCommands ?? options.config?.showCommands,
      language: options.language ?? options.config?.language
    });

    this.view.dispatch({
      effects: StateEffect.appendConfig.of([
        visualExtension(),
        this.themeCompartment.of(createTheme(this.configService.get()))
      ])
    });

    this.unsubscribe = this.configService.subscribe(config => this.sync(config));
    this.sync(this.configService.get());
  }

  setVisualMode(enabled: boolean): void {
    if (this.enabled === enabled) return;
    this.enabled = enabled;
    this.view.dispatch({ effects: setVisualState.of({ enabled }) });
    this.options.onModeChange?.(enabled ? 'visual' : 'source');
  }

  toggleMode(): void {
    this.setVisualMode(!this.enabled);
  }

  setLanguage(language: string): void {
    this.configService.update({ language });
  }

  updateOptions(options: VisualEditorOptions): void {
    this.options = { ...this.options, ...options };
    this.configService.update({
      ...options.config,
      ...(options.showCommands === undefined ? {} : { showCommands: options.showCommands }),
      ...(options.language === undefined ? {} : { language: options.language })
    });
  }

  getConfig(): EditorConfig {
    return this.configService.get();
  }

  destroy(): void {
    this.unsubscribe();
  }

  private sync(config: EditorConfig): void {
    this.view.dispatch({
      effects: [
        setVisualState.of({
          enabled: this.enabled,
          showCommands: config.showCommands,
          language: config.language,
          maxDepth: config.maxDepth
        }),
        this.themeCompartment.reconfigure(createTheme(config))
      ]
    });
  }
}
