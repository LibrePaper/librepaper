<script>
  // Fills the editor area while one changed file of the folder's repository is
  // compared: `HEAD` on the left, the working tree on the right, both read
  // only. The Git panel picks the file and the differ
  // (lib/reader/git-diff.svelte.js) reads it; this only shows it. Nothing is
  // written from here.
  import { loadMergeEditor } from "../../lib/merge-editor.js";

  let { differ } = $props();

  // The comparison editor is fetched when there is something to compare, not
  // when the reader is built. The shared loader keeps a second request from
  // starting a second import, and a failure is remembered rather than retried
  // on every redraw.
  let MergeEditor = $state(null);
  let mergeFailed = $state(false);
  const view = $derived(differ.state);
  $effect(() => {
    if (MergeEditor || mergeFailed || view.loading || view.error || view.binary) return;
    loadMergeEditor().then(
      (component) => (MergeEditor = component),
      () => (mergeFailed = true),
    );
  });

  // A side that is not there says so in its label, so an added or a deleted
  // file reads as itself rather than as an empty pane.
  const baseline = $derived(view.head === null ? "HEAD (not in the last commit)" : "HEAD");
  const target = $derived(view.work === null ? "Working tree (file deleted)" : "Working tree");
</script>

<section class="git-diff" aria-label="Changes in {view.path}" aria-busy={view.loading}>
  {#if view.error}
    <div class="git-diff-note" role="alert">
      <p class="lp-text-error">{view.error}</p>
      <div class="git-diff-actions">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => differ.refresh()}>Try again</button>
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => differ.close()}>Close</button>
      </div>
    </div>
  {:else if view.loading}
    <p class="git-diff-note" role="status">Loading…</p>
  {:else if view.binary}
    <div class="git-diff-note">
      <p>{view.path} is not text, so there is nothing to compare here.</p>
      <div class="git-diff-actions">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => differ.close()}>Close</button>
      </div>
    </div>
  {:else if MergeEditor}
    <MergeEditor path={view.path} oldText={view.head ?? ""} newText={view.work ?? ""} diff
      editable={false} baselineLabel={baseline} targetLabel={target} onclose={() => differ.close()} />
  {:else if mergeFailed}
    <p class="git-diff-note" role="alert">The comparison could not be loaded. Check the installation and reopen this file.</p>
  {:else}
    <p class="git-diff-note" role="status">Loading the comparison…</p>
  {/if}
</section>

<style>
  .git-diff { display: flex; flex-direction: column; height: 100%; min-width: 0; min-height: 0; overflow: hidden; }
  .git-diff :global(.merge-editor) { flex: 1; min-height: 0; }
  .git-diff-note { margin: 0; padding: 1rem; overflow-wrap: anywhere; }
  .git-diff-note p { margin: 0 0 0.5rem; }
  .git-diff-actions { display: flex; flex-wrap: wrap; gap: 0.5rem; }
</style>
