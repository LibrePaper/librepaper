<script>
  // The comparison editor is fetched when there is something to compare, not
  // when the reader is built. A static import here would undo the dynamic one
  // Reader.svelte makes for the same component: it puts CodeMirror and the
  // CRDT on the critical path of every reader, including everyone who never
  // opens a version.
  let MergeEditor = $state(null);
  let mergeFailed = $state(false);
  let { source, canEdit = false, onrestore, ondownload, downloading = "", downloadProblem = "" } = $props();
  const result = $derived(source.result);
  const path = $derived(source.path);
  const oldText = $derived(result?.oldTree.texts?.[path]);
  const newText = $derived(result?.newTree.texts?.[path]);
  const status = $derived(result?.status?.[path] || "same");
  const binary = $derived(Boolean(result?.binary?.[path]));
  // Asked for once. A failure is remembered rather than retried on every
  // redraw: an import that failed because the chunk is not there will fail
  // the same way a moment later, and the reader is told instead.
  $effect(() => {
    if (MergeEditor || mergeFailed || !result || binary) return;
    import("../MergeEditor.svelte").then(
      (module) => (MergeEditor = module.default),
      () => (mergeFailed = true),
    );
  });

  // What the file selector says about each file, so the choice itself carries
  // the answer to "what changed?" rather than making the reader open each one.
  const MARK = { added: "added", removed: "removed", changed: "changed" };
  const nameOf = (name) => (MARK[result?.status?.[name]] ? `${name} · ${MARK[result.status[name]]}` : name);
  // A binary file has no comparison to show, only a fact about it.
  const BINARY_NOTE = {
    added: "This file was added.",
    removed: "This file was removed.",
    changed: "This file changed. Images and other binary files are not compared here.",
    same: "This file is unchanged.",
  };
  const fileNote = $derived(status === "added" ? "File added."
    : status === "removed" ? "File removed."
      : oldText === "" && newText === "" ? "Empty file." : "");
</script>

<section class="history-workspace" aria-label="History source" aria-busy={source.loading}>
  {#if source.loading}
    <p role="status">Loading version source…</p>
  {:else if source.problem}
    <div role="alert">
      <p>{source.problem}</p>
      <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => source.select()}>Retry</button>
    </div>
  {:else if result}
    <div class="history-source-bar">
      <label class="history-source-file">
        File
        <select class="select" aria-label="History file" value={path} onchange={event => source.selectFile(event.currentTarget.value)}>
          {#each result.paths as name (name)}<option value={name}>{nameOf(name)}</option>{/each}
        </select>
      </label>
      <span class="history-source-summary panel-meta">
        {result.changed.length
          ? `${result.changed.length} file${result.changed.length === 1 ? "" : "s"} differ`
          : "No differences"}
      </span>
      <span class="history-source-actions">
        <button type="button" class="btn btn-sm" onclick={() => source.select()}>Refresh comparison</button>
        {#if canEdit && source.selected}
          <!-- A version's archive is produced on request rather than kept
               ready (§8.5), so the button says what it is waiting on rather
               than sitting quiet or, worse, looking done before the file
               exists. Editor-only, like the rest of this panel: a reader or
               commenter link cannot reach the history routes this asks. -->
          <button type="button" class="btn btn-sm" disabled={downloading === source.selected}
                  onclick={() => ondownload?.(source.selected)}>
            {downloading === source.selected ? "Preparing download…" : "Download this version"}
          </button>
        {/if}
        {#if canEdit && source.selected}
          <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onrestore?.(source.selected)}>Restore this version</button>
        {/if}
      </span>
    </div>
    {#if downloadProblem}
      <p class="text-error-500 text-sm" role="alert">{downloadProblem}</p>
    {/if}
    {#if binary}
      <p role="status">{BINARY_NOTE[status]}</p>
    {:else if oldText === undefined && newText === undefined}
      <p role="status">{path ? "This file has no text source to display." : "This version has no source files."}</p>
    {:else}
      {#if MergeEditor}
        <MergeEditor {path} oldText={oldText ?? ""} newText={newText ?? ""}
          diff={result.diff} editable={false} baselineLabel={result.oldLabel} targetLabel={result.newLabel}
          note={fileNote} />
      {:else if mergeFailed}
        <p role="alert">The comparison could not be loaded. Check your connection and reopen this version.</p>
      {:else}
        <p role="status">Loading the comparison…</p>
      {/if}
    {/if}
  {:else}
    <p role="status">Select a version to compare it with the current source.</p>
  {/if}
</section>

<style>
  .history-workspace { height: 100%; min-width: 0; display: flex; flex-direction: column; overflow: hidden; }
  .history-workspace > p, .history-workspace > div[role="alert"] { padding: 1rem; }
  .history-source-bar { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.5rem; }
  .history-source-file { display: flex; align-items: center; gap: 0.5rem; }
  .history-source-file select { width: auto; min-width: 12rem; }
  .history-source-actions { display: flex; align-items: center; gap: 0.5rem; margin-left: auto; }
  .history-workspace :global(.merge-editor) { flex: 1; min-height: 0; }
</style>
