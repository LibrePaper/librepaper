<script>
  // A compact two pane merge editor. The old checkpoint is deliberately
  // read-only; the live side is editable and is written back through the
  // caller after every accepted hunk or manual edit.
  import { EditorState, Prec } from "@codemirror/state";
  import { EditorView, lineNumbers, keymap } from "@codemirror/view";
  import { defaultKeymap, indentWithTab } from "@codemirror/commands";
  import { MergeView } from "@codemirror/merge";
  import { LoroExtensions } from "../../vendor/loro-codemirror/index.ts";
  import { undoManagerField, undo as undoCommand, redo as redoCommand } from "../lib/loro-undo.js";
  import { UndoManager } from "loro-crdt";
  import { DIRECTORY_ORIGIN } from "../lib/project-session.js";
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
    baselineLabel = "checkpoint",
    targetLabel = "Live document",
    // A stale suggestion opens this editor with an explanation of why: the
    // passage it named no longer matches, so the header carries that reason
    // instead of leaving the reader to guess why the merge editor appeared.
    note = "",
    onlive,
    onclose,
  } = $props();

  let host = $state(null);
  let merge = null;

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
      EditorView.contentAttributes.of({ "aria-label": side || "Source" }),
      lineNumbers(),
      keymap.of([indentWithTab, ...defaultKeymap]),
      ...keyboardExtensions,
      EditorView.lineWrapping,
      ...collaborativeExtensions,
      ...(readOnly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : []),
    ];
  }

  $effect(() => {
    // Rebuild when permissions change. This matters when an editor's lease is
    // revoked while a comparison is open: the editable side disappears with
    // the permission and no stale revert control remains actionable.
    void editable;
    if (!host) return;
    // An undo manager is built from the document, not from one text in it:
    // it records the operations this peer made, and a peer makes them across
    // files. Handing it the text threw "expected instance of LoroDoc" from
    // inside wasm while the effect was running, which Svelte reported as an
    // unhandled rejection naming neither this line nor the argument.
    const undoManager = editable && liveText && loroDoc
      ? new UndoManager(loroDoc, { excludeOriginPrefixes: [DIRECTORY_ORIGIN] })
      : null;
    merge = diff
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
        collapseUnchanged: { margin: 3, minSize: 4 },
        parent: host,
      })
      : new EditorView({
        state: EditorState.create({ doc: String(oldText || ""), extensions: extensions(true, false, baselineLabel) }),
        parent: host,
      });
    return () => {
      merge?.destroy();
      merge = null;
    };
  });
</script>

<section class="merge-editor flex h-full flex-col" aria-label={diff ? `Compare ${baselineLabel} with ${targetLabel}` : "Checkpoint source"}>
  <header class="merge-toolbar flex items-center justify-between gap-2 border-surface-200-800 border-b p-2">
    <div class="truncate text-sm">
      <strong>{path || "document"}</strong><span class="panel-muted">{diff ? ` · ${baselineLabel} on the left, ${targetLabel} on the right` : ` · ${baselineLabel} source`}</span>
      {#if note}<span class="panel-muted"> — {note}</span>{/if}
    </div>
    {#if onlive}<button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onlive()}>Compare with live to restore passages</button>{/if}
    {#if onclose}<IconButton icon="x" label="Close diff" tone="plain" onclick={() => onclose()} />{/if}
  </header>
  <div class="merge-host min-h-0 flex-1" bind:this={host}></div>
</section>

<style>
  .merge-host { min-height: 12rem; overflow: auto; }
  :global(.merge-editor .cm-mergeView) { min-height: 100%; }
  :global(.merge-editor .cm-editor) { min-height: 100%; }
  :global(.merge-editor .cm-scroller) { overflow: auto; }
</style>
