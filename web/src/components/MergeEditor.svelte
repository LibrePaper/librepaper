<script>
  // A compact two pane merge editor. The old checkpoint is deliberately
  // read-only; the live side is editable and is written back through the
  // caller after every accepted hunk or manual edit.
  import { EditorState } from "@codemirror/state";
  import { EditorView, lineNumbers, keymap } from "@codemirror/view";
  import { defaultKeymap, indentWithTab } from "@codemirror/commands";
  import { MergeView } from "@codemirror/merge";
  import { yCollab, yUndoManagerKeymap } from "y-codemirror.next";
  import IconButton from "./IconButton.svelte";

  let {
    path = "",
    oldText = "",
    newText = "",
    liveText = null,
    awareness = null,
    editable = false,
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

  function extensions(readOnly = false, collaborative = false) {
    return [
      lineNumbers(),
      keymap.of([indentWithTab, ...defaultKeymap, ...(collaborative ? yUndoManagerKeymap : [])]),
      EditorView.lineWrapping,
      ...(readOnly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : []),
    ];
  }

  $effect(() => {
    // Rebuild when permissions change. This matters when an editor's lease is
    // revoked while a comparison is open: the editable side disappears with
    // the permission and no stale revert control remains actionable.
    void editable;
    if (!host) return;
    merge = new MergeView({
      a: { doc: String(oldText || ""), extensions: extensions(true) },
      b: {
        doc: liveText?.toString() ?? String(newText || ""),
        extensions: [
          ...extensions(!editable, Boolean(editable && liveText)),
          // Binding the editable side to the live Y.Text means a revert
          // button inserts only its hunk and preserves a coauthor's update
          // that landed while this view was open.
          ...(editable && liveText ? [yCollab(liveText, awareness)] : []),
        ],
      },
      orientation: "a-b",
      ...(editable ? { revertControls: "a-to-b" } : {}),
      highlightChanges: true,
      gutter: true,
      collapseUnchanged: { margin: 3, minSize: 4 },
      parent: host,
    });
    return () => {
      merge?.destroy();
      merge = null;
    };
  });
</script>

<section class="merge-editor flex h-full flex-col" aria-label="Compare checkpoint with live document">
  <header class="merge-toolbar flex items-center justify-between gap-2 border-surface-200-800 border-b p-2">
    <div class="truncate text-sm">
      <strong>{path || "document"}</strong><span class="panel-muted"> · checkpoint on the left, {targetLabel} on the right</span>
      {#if note}<span class="panel-muted"> — {note}</span>{/if}
    </div>
    {#if onlive}<button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onlive()}>Compare with live to restore passages</button>{/if}
    <IconButton icon="x" label="Close diff" tone="plain" onclick={() => onclose?.()} />
  </header>
  <div class="merge-host min-h-0 flex-1" bind:this={host}></div>
</section>

<style>
  .merge-host { min-height: 12rem; overflow: auto; }
  :global(.merge-editor .cm-mergeView) { min-height: 100%; }
  :global(.merge-editor .cm-editor) { min-height: 100%; }
  :global(.merge-editor .cm-scroller) { overflow: auto; }
</style>
