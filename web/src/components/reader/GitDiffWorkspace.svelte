<script>
  // Fills the editor area while the project is compared with its Git remote.
  // The comparison lives in the differ (lib/reader/git-diff.svelte.js); this
  // only shows it. Nothing is written from here.
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

  const different = $derived(differ.state.files.filter((file) => file.change === "different"));
  const local = $derived(differ.state.files.filter((file) => file.change === "local"));
  const remote = $derived(differ.state.files.filter((file) => file.change === "remote"));
  const summary = $derived([
    different.length ? `${different.length} different` : "",
    local.length ? `${local.length} only in LibrePaper` : "",
    remote.length ? `${remote.length} only on the remote` : "",
    `${differ.state.same} the same`,
  ].filter(Boolean).join(" · "));
  const item = $derived(differ.state.item);

  // The folder is shown after the file name, as the Git panel shows changes.
  function split(path) {
    const slash = path.lastIndexOf("/");
    return slash > 0 ? [path.slice(slash + 1), path.slice(0, slash)] : [path, ""];
  }
</script>

{#snippet group(title, mark, files)}
  {#if files.length}
    <h4 class="panel-section-title">{title} ({files.length})</h4>
    <ul class="git-diff-list">
      {#each files as file (file.path)}
        {@const [name, folder] = split(file.path)}
        <li>
          <button type="button" class="git-diff-row" title={file.path}
            aria-current={differ.state.selected === file.path ? "true" : undefined}
            onclick={() => differ.select(file.path)}>
            <span class="git-diff-name">{name}</span>
            {#if folder}<span class="git-diff-folder panel-meta">{folder}</span>{/if}
            <span class="git-diff-mark" aria-label={title}>{mark}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
{/snippet}

<section class="git-diff" aria-label="Compare with the remote" aria-busy={differ.state.loading}>
  <div class="git-diff-bar">
    <strong>Diff with {differ.state.label}</strong>
    <span class="panel-meta git-diff-branch"><Icon name="git-branch" size={14} /><span>{differ.state.branch}</span></span>
    <span class="git-diff-actions">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={differ.state.loading}
        onclick={() => differ.refresh()}>Refresh</button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => differ.close()}>Close</button>
    </span>
  </div>
  <p class="panel-meta git-diff-summary">{summary}</p>
  {#if differ.state.error}<p class="lp-text-error" role="alert">{differ.state.error}</p>{/if}

  <div class="git-diff-body">
    <aside class="git-diff-files">
      {#if !differ.state.loading}
        {@render group("Different", "M", different)}
        {@render group("Only in LibrePaper", "L", local)}
        {@render group("Only on the remote", "R", remote)}
        {#if differ.state.unsynced.length}
          <details class="git-diff-unsynced">
            <summary class="panel-section-title">Not synced ({differ.state.unsynced.length})</summary>
            <ul class="git-diff-list">
              {#each differ.state.unsynced as entry (entry.path)}
                <li class="panel-meta git-diff-unsynced-item">{entry.path}: {entry.reason}</li>
              {/each}
            </ul>
          </details>
        {/if}
      {/if}
    </aside>

    <div class="git-diff-compare">
      {#if differ.state.loading}
        <p role="status">Loading…</p>
      {:else if !differ.state.files.length}
        {#if !differ.state.error}<p>LibrePaper and the remote have the same files.</p>{/if}
      {:else if differ.state.itemLoading}
        <p role="status">Loading…</p>
      {:else if item}
        {#key item.path}
          {#if item.kind !== "text"}
            <p class="panel-meta">This file is not text.</p>
          {:else if item.ours != null && item.theirs != null}
            {#if GitMergeView}
              <GitMergeView path={item.path} theirs={item.theirs} result={item.ours} revision={0} editable={false}
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
  </div>
</section>

<style>
  .git-diff { display: flex; flex-direction: column; height: 100%; min-height: 0; min-width: 0; overflow: hidden; container-type: inline-size; }
  .git-diff-bar { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.5rem; }
  .git-diff-branch { display: flex; align-items: center; gap: calc(var(--spacing) * 1); min-width: 0; margin: 0; color: var(--color-text-muted); }
  .git-diff-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; margin-left: auto; }
  .git-diff-summary { padding: 0 0.5rem; }
  .git-diff > p[role="alert"] { padding: 0 0.5rem; }
  .git-diff-body { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .git-diff-files { flex: none; max-height: 40%; padding: 0.5rem; overflow-y: auto; border-bottom: 1px solid var(--color-divider); }
  .git-diff-list { margin: 0; padding: 0; list-style: none; }
  .git-diff-row { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); width: 100%; min-width: 0; min-height: 2rem; padding: 0.15rem 0.35rem; border: 0; border-radius: var(--radius-base); background: none; color: inherit; font: inherit; text-align: start; cursor: pointer; }
  .git-diff-row:hover { background: var(--color-row-hover); }
  .git-diff-row[aria-current="true"] { background: var(--color-row-hover); box-shadow: inset 2px 0 0 var(--color-link); }
  .git-diff-name { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-diff-folder { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .git-diff-mark { flex: none; margin-inline-start: auto; font-weight: 600; font-variant-numeric: tabular-nums; }
  .git-diff-unsynced { margin-top: 0.5rem; }
  .git-diff-unsynced summary { cursor: pointer; }
  .git-diff-unsynced-item { overflow-wrap: anywhere; padding: 0.15rem 0.35rem; }
  .git-diff-compare { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .git-diff-compare > p { padding: 1rem; }
  .git-diff-compare > p.git-diff-caption { padding: 0.25rem 0.5rem; border-bottom: 1px solid var(--color-divider); }
  .git-diff-side { flex: 1; min-height: 0; margin: 0; padding: 1rem; overflow: auto; white-space: pre; font-family: var(--font-mono, monospace); }
  @container (min-width: 700px) {
    .git-diff-body { flex-direction: row; }
    .git-diff-files { flex: 0 0 16rem; max-height: none; border-bottom: 0; border-right: 1px solid var(--color-divider); }
  }
</style>
