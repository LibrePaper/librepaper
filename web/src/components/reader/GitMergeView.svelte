<script>
  // The side by side comparison of a conflicted text file. The repository's
  // copy is read-only on the left. The Result on the right starts from
  // LibrePaper's copy, and typing reports back through onedit.
  import { untrack } from "svelte";
  import { EditorState, Annotation } from "@codemirror/state";
  import { EditorView, lineNumbers, keymap } from "@codemirror/view";
  import { defaultKeymap, indentWithTab } from "@codemirror/commands";
  import { MergeView, unifiedMergeView } from "@codemirror/merge";
  import { sourceLanguage } from "../../lib/source-language.js";
  import { sourceHighlighting } from "../../lib/source-highlight.js";

  // The resolver's pane names. The diff view passes its own.
  const RESOLVER_LABELS = { left: "The repository's copy", right: "Result (starts from LibrePaper's copy)", rightName: "Result" };
  let { path = "", theirs = "", result = "", revision = 0, editable = true, unified = false, onedit = undefined, labels = RESOLVER_LABELS } = $props();

  // Marks a transaction that loads the Result from the workspace, so the
  // update listener does not report it as a typed edit.
  const programmatic = Annotation.define();

  let host = $state(null);
  let merge = null;
  // The revision the Result was last built or reloaded at.
  let shownRevision = null;

  function extensions(label, readOnly, sourcePath) {
    return [
      EditorView.contentAttributes.of({ "aria-label": label }),
      lineNumbers(),
      keymap.of([indentWithTab, ...defaultKeymap]),
      EditorView.lineWrapping,
      sourceHighlighting,
      sourceLanguage(sourcePath),
      ...(readOnly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : []),
    ];
  }

  // Built per file. It is rebuilt when the file or whether the Result can be
  // edited changes, since the revert arrows exist only while it can be edited.
  // The Result's own text changes in place (see the revision effect below).
  $effect(() => {
    if (!host) return;
    const file = path;
    const canEdit = editable;
    const flat = unified;
    untrack(() => {
      shownRevision = revision;
      if (flat) {
        // One read-only editor: the remote's text is the original, LibrePaper's copy is the document.
        merge = {
          view: new EditorView({
            state: EditorState.create({
              doc: String(result || ""),
              extensions: [
                ...extensions(labels.rightName, true, file),
                unifiedMergeView({ original: String(theirs || ""), mergeControls: false, highlightChanges: true, gutter: true, collapseUnchanged: { margin: 3, minSize: 4 } }),
              ],
            }),
            parent: host,
          }),
          destroy() {
            this.view.destroy();
          },
        };
        return;
      }
      merge = new MergeView({
        a: { doc: String(theirs || ""), extensions: extensions(labels.left, true, file) },
        b: {
          doc: String(result || ""),
          extensions: [
            ...extensions(labels.rightName, !canEdit, file),
            EditorView.updateListener.of((update) => {
              if (!update.docChanged) return;
              if (update.transactions.some((transaction) => transaction.annotation(programmatic))) return;
              onedit?.(update.state.doc.toString());
            }),
          ],
        },
        orientation: "a-b",
        ...(canEdit ? { revertControls: "a-to-b" } : {}),
        highlightChanges: true,
        gutter: true,
        collapseUnchanged: { margin: 3, minSize: 4 },
        parent: host,
      });
    });
    return () => {
      merge?.destroy();
      merge = null;
    };
  });

  // A whole-file choice changes the Result from outside the editor, so its
  // text is replaced in one transaction. Typing never bumps the revision, so
  // it does not reset the view.
  $effect(() => {
    const next = revision;
    untrack(() => {
      if (!merge || !merge.b || shownRevision === next) return;
      shownRevision = next;
      const view = merge.b;
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: String(result || "") },
        annotations: programmatic.of(true),
      });
    });
  });
</script>

<section class="git-merge merge-editor" aria-label={`Compare ${labels.left} with ${labels.rightName}`}>
  {#if unified}
    <div class="git-merge-labels git-merge-legend panel-meta">
      <span>− {labels.left}</span>
      <span>+ {labels.right}</span>
    </div>
  {:else}
    <div class="git-merge-labels panel-meta">
      <span>{labels.left}</span>
      <span>{labels.right}</span>
    </div>
  {/if}
  <div class="git-merge-host" bind:this={host}></div>
</section>

<style>
  .git-merge { display: flex; flex-direction: column; flex: 1; min-height: 0; min-width: 0; }
  .git-merge-labels { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem; padding: 0.25rem 0.5rem; border-bottom: 1px solid var(--color-divider); }
  .git-merge-legend { display: flex; gap: 1rem; }
  .git-merge-host { flex: 1; min-height: 12rem; overflow: auto; }
  :global(.git-merge .cm-mergeView) { min-height: 100%; }
  :global(.git-merge .cm-editor) { min-height: 100%; }
  /* The host is the one scroller, as in MergeEditor. */
  :global(.git-merge .cm-scroller) { overflow: visible; }
  /* The merge-editor class brings the palette History uses. A conflict is
     about a few words, so they also get a tint, not only the underline. */
  :global(.git-merge.merge-editor .cm-mergeView .cm-editor.cm-merge-a .cm-changedText) {
    background: linear-gradient(var(--color-error-solid), var(--color-error-solid)) bottom / 100% 2px no-repeat,
      color-mix(in srgb, var(--color-error-solid) 22%, transparent);
  }
  :global(.git-merge.merge-editor .cm-mergeView .cm-editor.cm-merge-b .cm-changedText) {
    background: linear-gradient(var(--color-success-solid), var(--color-success-solid)) bottom / 100% 2px no-repeat,
      color-mix(in srgb, var(--color-success-solid) 22%, transparent);
  }
  /* The unified view: removed text from the remote, added or changed lines in LibrePaper's copy. */
  :global(.git-merge.merge-editor .cm-deletedChunk .cm-deletedText) { background: color-mix(in srgb, var(--color-error-solid) 22%, transparent); }
  :global(.git-merge.merge-editor .cm-changedLine .cm-changedText) { background: color-mix(in srgb, var(--color-success-solid) 22%, transparent); }
</style>
