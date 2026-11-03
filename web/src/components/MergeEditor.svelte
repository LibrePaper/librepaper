<script>
  // A compact two pane merge editor. The old label is deliberately
  // read-only; the live side is editable and is written back through the
  // caller after every accepted hunk or manual edit.
  import { m } from "../lib/paraglide/messages.js";
  import { untrack } from "svelte";
  import { EditorState, Prec, StateEffect, StateField } from "@codemirror/state";
  import { Decoration, EditorView, lineNumbers, keymap } from "@codemirror/view";
  import { defaultKeymap, indentWithTab } from "@codemirror/commands";
  import { MergeView, getChunks, unifiedMergeView } from "@codemirror/merge";
  import { LoroExtensions } from "../../vendor/loro-codemirror/index.ts";
  import { undoManagerField, undo as undoCommand, redo as redoCommand } from "../lib/loro-undo.js";
  import { UndoManager } from "loro-crdt";
  import { DIRECTORY_ORIGIN } from "../lib/project-session.js";
  import { sourceLanguage } from "../lib/source-language.js";
  import { wordDiff } from "../lib/word-diff.js";
  import { sourceHighlighting } from "../lib/source-highlight.js";
  import IconButton from "./IconButton.svelte";

  let {
    path = "",
    oldText = "",
    newText = "",
    liveText = null,
    ephemeral = null,
    loroDoc = null,
    editable = false,
    diff = true,
    baselineLabel = m.mergeeditor_baseline_default(),
    targetLabel = m.mergeeditor_target_default(),
    // A short remark for the header, such as which file the comparison is
    // about.
    note = "",
    // Off when the caller shows the sides itself, as the history workspace does.
    header = true,
    onlive,
    onclose,
    // Called with the number of changed blocks after each (re)build.
    onchanges = undefined,
    // When > 0 (and the view is a read-only diff), a host narrower than this
    // many CSS pixels shows one unified editor instead of two panes.
    unifiedBelow = 0,
  } = $props();

  let host = $state(null);
  let hostWidth = $state(0);
  let merge = null;
  // The editor(s) of the current build and their chunks, for goToChange.
  let views = [];
  /** @type {readonly import("@codemirror/merge").Chunk[]} */
  let chunkList = [];
  let active = -1;

  // Long one-line paragraphs need a deep scan to be diffed by word, so the
  // limits are well above the merge package's default of 500.
  const diffConfig = {
    scanLimit: 20000,
    timeout: 1000,
    override: (/** @type {string} */ a, /** @type {string} */ b) => wordDiff(a, b, { scanLimit: 20000, timeout: 1000 }),
  };
  const collapse = { margin: 3, minSize: 4 };

  // The active change is marked by a line class on its lines.
  /** @type {import("@codemirror/state").StateEffectType<{ from: number, to: number } | null>} */
  const setActive = StateEffect.define();
  const activeLine = Decoration.line({ class: "cm-activeChange" });
  const activeField = StateField.define({
    create: () => Decoration.none,
    update(value, tr) {
      value = value.map(tr.changes);
      for (const effect of tr.effects) {
        if (!effect.is(setActive)) continue;
        const doc = tr.state.doc;
        const marks = [];
        if (effect.value) {
          const from = Math.min(effect.value.from, doc.length);
          const to = Math.min(Math.max(effect.value.to - 1, from), doc.length);
          for (let line = doc.lineAt(from).number; line <= doc.lineAt(to).number; line++) {
            marks.push(activeLine.range(doc.line(line).from));
          }
        }
        value = Decoration.set(marks);
      }
      return value;
    },
    provide: (field) => EditorView.decorations.from(field),
  });

  const unified = $derived(unifiedBelow > 0 && diff && !editable && hostWidth > 0 && hostWidth < unifiedBelow);

  $effect(() => {
    if (!host) return;
    const observer = new ResizeObserver(([entry]) => {
      hostWidth = entry.contentRect.width;
    });
    observer.observe(host);
    hostWidth = host.clientWidth;
    return () => observer.disconnect();
  });

  // `side` names the pane a screen reader has landed in: CodeMirror's editable
  // is a textbox, and two unnamed textboxes side by side are two of "edit
  // text" with nothing to tell them apart.
  function extensions(readOnly = false, collaborative = false, side = "", loroDoc = null, loroText = null, undoManager = null) {
    // Ours has to outrank the Mod-z LoroExtensions binds at Prec.high, and
    // `undoManagerField` hands it the same manager. See lib/loro-undo.js.
    const keyboardExtensions = collaborative && loroDoc && loroText && undoManager
      ? [undoManagerField.init(() => undoManager),
         Prec.highest(keymap.of([
           { key: "Mod-z", run: undoCommand, preventDefault: true },
           { key: "Mod-Shift-z", run: redoCommand, preventDefault: true },
         ]))]
      : [];
    const collaborativeExtensions = collaborative && loroDoc && loroText && undoManager
      ? [LoroExtensions(loroDoc, undefined, undoManager, () => loroText)]
      : [];
    return [
      EditorView.contentAttributes.of({ "aria-label": side || m.common_source() }),
      lineNumbers(),
      keymap.of([indentWithTab, ...defaultKeymap]),
      ...keyboardExtensions,
      EditorView.lineWrapping,
      activeField,
      ...collaborativeExtensions,
      sourceHighlighting,
      sourceLanguage(path),
      ...(readOnly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : []),
    ];
  }

  $effect(() => {
    // Rebuild when permissions change. This matters when an editor's lease is
    // revoked while a comparison is open: the editable side disappears with
    // the permission and no stale revert control remains actionable.
    void editable;
    const single = unified;
    if (!host) return;
    // An undo manager is built from the document, not from one text in it:
    // it records the operations this peer made, and a peer makes them across
    // files. Handing it the text threw "expected instance of LoroDoc" from
    // inside wasm while the effect was running, which Svelte reported as an
    // unhandled rejection naming neither this line nor the argument.
    const undoManager = editable && liveText && loroDoc
      ? new UndoManager(loroDoc, { excludeOriginPrefixes: [DIRECTORY_ORIGIN] })
      : null;
    merge = diff && single
      ? new EditorView({
        state: EditorState.create({
          doc: String(newText || ""),
          extensions: [
            extensions(true, false, targetLabel),
            unifiedMergeView({ original: String(oldText || ""), highlightChanges: true, gutter: true, mergeControls: false, collapseUnchanged: collapse, diffConfig }),
          ],
        }),
        parent: host,
      })
      : diff
      ? new MergeView({
        a: { doc: String(oldText || ""), extensions: extensions(true, false, baselineLabel) },
        b: {
          doc: liveText?.toString() ?? String(newText || ""),
          extensions: extensions(!editable, Boolean(editable && liveText), targetLabel, editable && liveText ? loroDoc : null, editable && liveText ? liveText : null, undoManager),
        },
        orientation: "a-b",
        ...(editable ? { revertControls: "a-to-b" } : {}),
        highlightChanges: true,
        gutter: true,
        collapseUnchanged: collapse,
        diffConfig,
        parent: host,
      })
      : new EditorView({
        state: EditorState.create({ doc: String(oldText || ""), extensions: extensions(true, false, baselineLabel) }),
        parent: host,
      });
    active = -1;
    if (merge instanceof MergeView) {
      views = [merge.a, merge.b];
      chunkList = merge.chunks;
    } else if (diff && merge instanceof EditorView) {
      views = [merge];
      chunkList = getChunks(merge.state)?.chunks ?? [];
    } else {
      views = [];
      chunkList = [];
    }
    const count = chunkList.length;
    untrack(() => onchanges?.(count));
    return () => {
      merge?.destroy();
      merge = null;
      views = [];
      chunkList = [];
    };
  });

  export function changeCount() {
    return chunkList.length;
  }

  // Scrolls change `index` (0-based, document order) to the middle of the host
  // and marks it. The host is the scroller, not the editors, so after asking
  // CodeMirror to scroll the position is also worked out from its coordinates.
  export function goToChange(index) {
    if (!host || !chunkList.length) return;
    const i = Math.max(0, Math.min(chunkList.length - 1, Math.trunc(Number(index)) || 0));
    const chunk = chunkList[i];
    active = i;
    // [view, from, to] per editor; the unified editor shows the new text.
    const targets = views.length === 2
      ? [[views[0], chunk.fromA, chunk.toA], [views[1], chunk.fromB, chunk.toB]]
      : views.map((view) => [view, chunk.fromB, chunk.toB]);
    for (const [view, from, to] of targets) {
      const pos = Math.min(from, view.state.doc.length);
      view.dispatch({
        effects: [setActive.of({ from: pos, to: Math.min(to, view.state.doc.length) }), EditorView.scrollIntoView(pos, { y: "center" })],
      });
    }
    requestAnimationFrame(() => {
      if (!host || active !== i || !views.length) return;
      const view = views[views.length - 1];
      const coords = view.coordsAtPos(Math.min(targets[targets.length - 1][1], view.state.doc.length));
      if (!coords) return;
      const box = host.getBoundingClientRect();
      host.scrollTop += coords.top - box.top - box.height / 2;
    });
  }
</script>

<section class="merge-editor flex h-full flex-col" aria-label={diff ? m.mergeeditor_compare({ baseline: baselineLabel, target: targetLabel }) : m.mergeeditor_label_source()}>
  {#if header}
    <header class="merge-toolbar flex items-center justify-between gap-2 border-[var(--color-divider)] border-b p-2">
      <div class="truncate text-sm">
        <strong>{path || m.mergeeditor_document()}</strong><span class="panel-muted">{diff ? m.mergeeditor_sides({ baseline: baselineLabel, target: targetLabel }) : m.mergeeditor_source_only({ baseline: baselineLabel })}</span>
        {#if note}<span class="panel-muted"> — {note}</span>{/if}
      </div>
      {#if onlive}<button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => onlive()}>{m.mergeeditor_restore()}</button>{/if}
      {#if onclose}<IconButton icon="x" label={m.mergeeditor_close()} tone="plain" onclick={() => onclose()} />{/if}
    </header>
  {/if}
  <div class="merge-host min-h-0 flex-1" bind:this={host}></div>
</section>

<style>
  .merge-host { min-height: 12rem; overflow: auto; }
  :global(.merge-editor .cm-mergeView) { min-height: 100%; }
  :global(.merge-editor .cm-editor) { min-height: 100%; }
  /* The host is the one scroller. An editor scroller set to auto ends up a
     fraction of a pixel taller than its content, and Chromium hands it the
     wheel, which then scrolls nothing. Lines wrap, so it needs no scrolling. */
  :global(.merge-editor .cm-scroller) { overflow: visible; }

  /* Soft highlighting: a very pale wash on changed lines and a stronger one
     on the changed words only. Built from the status tokens, which are
     colour-blind safe, so it follows the theme in both modes. CodeMirror's
     own merge theme styles the same classes with selectors as specific as a
     plain override and is injected later, so the rules below name the host
     as well to win. */
  :global(.merge-editor) {
    --diff-del: var(--color-error-solid);
    --diff-add: var(--color-success-solid);
    --diff-line: 9%;
    --diff-word: 30%;
  }
  :global([data-theme="librepaper"][data-mode="dark"] .merge-editor) {
    --diff-line: 10%;
    --diff-word: 40%;
  }
  :global(.merge-editor .merge-host .cm-editor.cm-merge-a .cm-changedLine),
  :global(.merge-editor .merge-host .cm-editor .cm-deletedChunk) {
    background: color-mix(in srgb, var(--diff-del) var(--diff-line), transparent);
  }
  :global(.merge-editor .merge-host .cm-editor.cm-merge-b .cm-changedLine),
  :global(.merge-editor .merge-host .cm-editor .cm-inlineChangedLine),
  :global(.merge-editor .merge-host .cm-editor:not(.cm-merge-a):not(.cm-merge-b) .cm-changedLine) {
    background: color-mix(in srgb, var(--diff-add) var(--diff-line), transparent);
  }
  :global(.merge-editor .merge-host .cm-editor.cm-merge-a .cm-changedText),
  :global(.merge-editor .merge-host .cm-editor .cm-deletedChunk .cm-deletedText),
  :global(.merge-editor .merge-host .cm-editor.cm-merge-b .cm-deletedText) {
    background: color-mix(in srgb, var(--diff-del) var(--diff-word), transparent);
  }
  :global(.merge-editor .merge-host .cm-editor.cm-merge-b .cm-changedText),
  :global(.merge-editor .merge-host .cm-editor:not(.cm-merge-a):not(.cm-merge-b) .cm-changedText) {
    background: color-mix(in srgb, var(--diff-add) var(--diff-word), transparent);
  }
  :global(.merge-editor .merge-host .cm-editor.cm-merge-a .cm-changedLineGutter),
  :global(.merge-editor .merge-host .cm-editor .cm-deletedLineGutter) {
    background: var(--diff-del);
  }
  :global(.merge-editor .merge-host .cm-editor.cm-merge-b .cm-changedLineGutter),
  :global(.merge-editor .merge-host .cm-editor:not(.cm-merge-a):not(.cm-merge-b) .cm-changedLineGutter) {
    background: var(--diff-add);
  }
  :global(.merge-editor .merge-host .cm-editor .cm-collapsedLines) {
    color: var(--color-text-muted, inherit);
    background: color-mix(in srgb, currentColor 8%, transparent);
  }
  /* The change the toolbar last went to. */
  :global(.merge-editor .merge-host .cm-editor .cm-activeChange) {
    box-shadow: inset 3px 0 0 var(--color-brand);
  }
</style>
