<script>
  import MergeEditor from "../MergeEditor.svelte";
  let { source } = $props();
  const result = $derived(source.result);
  const path = $derived(source.path);
  const oldText = $derived(result?.oldTree.texts?.[path]);
  const newText = $derived(result?.newTree.texts?.[path]);
  const fileNote = $derived(oldText === undefined && newText !== undefined ? "File added."
    : oldText !== undefined && newText === undefined ? "File removed."
      : oldText === "" && newText === "" ? "Empty file." : "");
</script>

<section class="history-workspace" aria-label="History source" aria-busy={source.loading}>
  {#if source.loading}
    <p role="status">Loading checkpoint source…</p>
  {:else if source.problem}
    <div role="alert">
      <p>{source.problem}</p>
      <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => source.select()}>Retry</button>
    </div>
  {:else if result}
    <label class="history-source-file">
      File
      <select class="select" aria-label="History file" value={path} onchange={event => source.selectFile(event.currentTarget.value)}>
        {#each result.paths as name (name)}<option value={name}>{name}</option>{/each}
      </select>
      {#if source.selected && source.mode === "current"}
        <button type="button" class="btn btn-sm" onclick={() => source.select()}>Refresh current</button>
      {/if}
    </label>
    {#if oldText === undefined && newText === undefined}
      <p role="status">{path ? "This file has no text source to display." : "This version has no source files."}</p>
    {:else}
      <MergeEditor {path} oldText={oldText ?? ""} newText={newText ?? ""}
        diff={result.diff} editable={false} baselineLabel={result.oldLabel} targetLabel={result.newLabel}
        note={[result.note, fileNote].filter(Boolean).join(" ")} />
    {/if}
  {:else}
    <p role="status">Select a checkpoint to view its source.</p>
  {/if}
</section>

<style>
  .history-workspace { height: 100%; min-width: 0; display: flex; flex-direction: column; overflow: hidden; }
  .history-workspace > p, .history-workspace > div { padding: 1rem; }
  .history-source-file { display: flex; align-items: center; gap: 0.5rem; padding: 0.5rem; }
  .history-source-file select { width: auto; min-width: 12rem; }
  .history-workspace :global(.merge-editor) { flex: 1; min-height: 0; }
</style>
