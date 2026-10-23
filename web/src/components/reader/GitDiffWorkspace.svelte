<script>
  // Fills the editor area while the project is compared with its Git remote.
  // It shows one file, picked in the Git panel, as a unified diff. The
  // comparison lives in the differ (lib/reader/git-diff.svelte.js); this only
  // shows it. Nothing is written from here.
  import { loadGitMergeView } from "../../lib/merge-editor.js";
  import Icon from "../Icon.svelte";

  let { differ } = $props();

  // The comparison is fetched when a file is shown, not when the reader is
  // built. The shared loader keeps a second request from starting a second
  // import, and a failure is remembered rather than retried on every redraw.
  let GitMergeView = $state(null);
  let mergeFailed = $state(false);
  $effect(() => {
    if (GitMergeView || mergeFailed) return;
    loadGitMergeView().then(
      (component) => (GitMergeView = component),
      () => (mergeFailed = true),
    );
  });

  const item = $derived(differ.state.item);

  // The folder is shown after the file name, as the Git panel shows changes.
  function split(path) {
    const slash = path.lastIndexOf("/");
    return slash > 0 ? [path.slice(slash + 1), path.slice(0, slash)] : [path, ""];
  }
  const [name, folder] = $derived.by(() => split(differ.state.selected || ""));
</script>

<section class="git-diff" aria-label="Compare with the remote" aria-busy={differ.state.loading}>
  <div class="git-diff-bar">
    <strong class="git-diff-file" title={differ.state.selected}>{name}</strong>
    {#if folder}<span class="panel-meta git-diff-folder">{folder}</span>{/if}
    <span class="panel-meta git-diff-branch"><Icon name="git-branch" size={14} /><span>{differ.state.branch}</span></span>
    <span class="git-diff-actions">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={differ.state.loading}
        onclick={() => differ.refresh()}>Refresh</button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => differ.close()}>Close</button>
    </span>
  </div>
  {#if differ.state.error}<p class="lp-text-error" role="alert">{differ.state.error}</p>{/if}

  <div class="git-diff-compare">
    {#if differ.state.loading}
      <p role="status">Loading…</p>
    {:else if !differ.state.files.length}
      {#if !differ.state.error}<p>LibrePaper and the remote have the same files.</p>{/if}
    {:else if differ.state.itemLoading}
      <p role="status">Loading…</p>
    {:else if differ.state.selected && !differ.state.files.some((file) => file.path === differ.state.selected)}
      <p>LibrePaper and the remote have the same copy of this file.</p>
    {:else if item}
      {#key item.path}
        {#if item.kind !== "text"}
          <p class="panel-meta">This file is not text.</p>
        {:else if item.ours != null && item.theirs != null}
          {#if GitMergeView}
            <GitMergeView unified path={item.path} theirs={item.theirs} result={item.ours} revision={0} editable={false}
              labels={{ left: `The remote (${differ.state.branch})`, right: "LibrePaper", rightName: "LibrePaper" }} />
          {:else if mergeFailed}
            <p role="alert">Could not load the comparison.</p>
          {:else}
            <p role="status">Loading the comparison…</p>
          {/if}
        {:else}
          <p class="panel-meta git-diff-caption">{item.ours != null ? "Only in LibrePaper." : "Only on the remote."}</p>
          <pre class="git-diff-side">{item.ours ?? item.theirs ?? ""}</pre>
        {/if}
      {/key}
    {/if}
  </div>
</section>

<style>
  .git-diff { display: flex; flex-direction: column; height: 100%; min-height: 0; min-width: 0; overflow: hidden; container-type: inline-size; }
  .git-diff-bar { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.5rem; }
  .git-diff-file { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-diff-folder { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin: 0; }
  .git-diff-branch { display: flex; align-items: center; gap: calc(var(--spacing) * 1); min-width: 0; margin: 0; color: var(--color-text-muted); }
  .git-diff-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; margin-left: auto; }
  .git-diff > p[role="alert"] { padding: 0 0.5rem; }
  .git-diff-compare { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .git-diff-compare > p { padding: 1rem; }
  .git-diff-compare > p.git-diff-caption { padding: 0.25rem 0.5rem; border-bottom: 1px solid var(--color-divider); }
  .git-diff-side { flex: 1; min-height: 0; margin: 0; padding: 1rem; overflow: auto; white-space: pre; font-family: var(--font-mono, monospace); }
</style>
