<script>
  // Fills the editor area while a pull's conflicts are resolved. The choices
  // and their saving live in the resolver (lib/reader/git-resolver.svelte.js);
  // this only shows them.
  import { loadGitMergeView } from "../../lib/merge-editor.js";

  let { resolver } = $props();

  // The comparison is fetched when a conflict is shown, not when the reader is
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

  const item = $derived(resolver.state.items.find((entry) => entry.path === resolver.state.selected) ?? null);
  const chosen = $derived(item ? resolver.state.resolutions[item.path] : undefined);
</script>

<section class="git-conflicts" aria-label="Resolve conflicts" aria-busy={resolver.state.loading}>
  <div class="git-conflicts-bar">
    <strong>Resolve conflicts</strong>
    <label class="git-conflicts-file">File
      <select class="select" value={resolver.state.selected} onchange={(event) => resolver.select(event.currentTarget.value)}>
        {#each resolver.state.items as entry (entry.path)}
          <option value={entry.path}>{entry.path}</option>
        {/each}
      </select>
    </label>
    {#if item}
      <span class="git-conflicts-choices">
        {#if resolver.deleteVsModify(item)}
          <button type="button" disabled={resolver.state.busy} class="btn btn-sm {chosen?.keep === resolver.keepSide(item) ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
            onclick={() => resolver.choose(item.path, { keep: resolver.keepSide(item) })}>Keep the file</button>
          <button type="button" disabled={resolver.state.busy} class="btn btn-sm {chosen?.delete ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
            onclick={() => resolver.choose(item.path, { delete: true })}>Delete the file</button>
        {:else}
          <button type="button" disabled={resolver.state.busy} class="btn btn-sm {chosen?.keep === 'ours' ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
            onclick={() => resolver.choose(item.path, { keep: "ours" })}>Use LibrePaper's</button>
          <button type="button" disabled={resolver.state.busy} class="btn btn-sm {chosen?.keep === 'theirs' ? 'lp-control-brand' : 'lp-control-tonal-brand'}"
            onclick={() => resolver.choose(item.path, { keep: "theirs" })}>Use the repository's</button>
        {/if}
      </span>
    {/if}
    <span class="git-conflicts-actions">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={!resolver.allResolved() || resolver.state.busy}
        onclick={() => resolver.apply()}>{resolver.state.busy ? "Applying…" : "Apply"}</button>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => resolver.close()}>Close</button>
    </span>
  </div>
  <p class="panel-meta git-conflicts-note">The project does not change until every file is resolved and you apply the result.</p>
  {#if resolver.state.error}<p class="lp-text-error" role="alert">{resolver.state.error}</p>{/if}

  <div class="git-conflicts-body">
    {#if resolver.state.loading}
      <p role="status">Loading…</p>
    {:else if item}
      {#key `${item.path}:${resolver.state.loads}`}
        {#if item.kind !== "text"}
          <p class="panel-meta">This file is not text. Keep one side's copy.</p>
        {:else if item.ours != null && item.theirs != null}
          {#if GitMergeView}
            <GitMergeView path={item.path} theirs={item.theirs} result={resolver.resultText(item)} revision={resolver.state.revision}
              editable={!resolver.state.busy} onedit={(text) => resolver.edit(item.path, text)} />
          {:else if mergeFailed}
            <p role="alert">Could not load the comparison.</p>
          {:else}
            <p role="status">Loading the comparison…</p>
          {/if}
        {:else}
          <pre class="git-conflicts-side">{item.ours ?? item.theirs ?? ""}</pre>
          {#if chosen?.delete}<p class="panel-meta">Nothing is kept. This file will be deleted.</p>{/if}
        {/if}
      {/key}
    {/if}
  </div>
</section>

<style>
  .git-conflicts { display: flex; flex-direction: column; height: 100%; min-height: 0; min-width: 0; overflow: hidden; }
  .git-conflicts-bar { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.5rem; }
  .git-conflicts-file { display: flex; align-items: center; gap: 0.5rem; }
  .git-conflicts-file select { width: auto; min-width: 12rem; }
  .git-conflicts-choices { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; }
  .git-conflicts-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; margin-left: auto; }
  .git-conflicts-note { padding: 0 0.5rem; }
  .git-conflicts > p[role="alert"] { padding: 0 0.5rem; }
  .git-conflicts-body { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .git-conflicts-body > p { padding: 1rem; }
  .git-conflicts-side { flex: 1; min-height: 0; margin: 0; padding: 1rem; overflow: auto; white-space: pre; font-family: var(--font-mono, monospace); }
</style>
