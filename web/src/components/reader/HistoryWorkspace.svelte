<script>
  import { loadMergeEditor } from "../../lib/merge-editor.js";
  import { isMoment } from "../../lib/moment.js";
  import { m } from "../../lib/paraglide/messages.js";

  // The comparison editor is fetched when there is something to compare, not
  // when the reader is built. A static import here would undo the dynamic one
  // Reader.svelte makes for the same component: it puts CodeMirror and the
  // CRDT on the critical path of every reader, including everyone who never
  // opens a version.
  let MergeEditor = $state(null);
  let mergeFailed = $state(false);
  let { source, heading = "", canEdit = false, onrestore, ondownload, downloading = "", downloadProblem = "" } = $props();
  const result = $derived(source.result);
  const path = $derived(source.path);
  const oldText = $derived(result?.oldTree.texts?.[path]);
  const newText = $derived(result?.newTree.texts?.[path]);
  const status = $derived(result?.status?.[path] || "same");
  const binary = $derived(Boolean(result?.binary?.[path]));
  // Asked for once. A failure is remembered rather than retried on every
  // redraw: an import that failed because the chunk is not there will fail
  // the same way a moment later, and the reader is told instead. The effect
  // can run again before the first load finishes. The shared loader keeps
  // that to one import.
  $effect(() => {
    if (MergeEditor || mergeFailed || !result || binary) return;
    loadMergeEditor().then(
      (component) => (MergeEditor = component),
      () => (mergeFailed = true),
    );
  });

  // What the file selector says about each file, so the choice itself carries
  // the answer to "what changed?" rather than making the reader open each one.
  const MARK = { added: m.historyworkspace_mark_added(), removed: m.historyworkspace_mark_removed(), changed: m.historyworkspace_mark_changed() };
  const nameOf = (name) => (MARK[result?.status?.[name]] ? m.historyworkspace_file_marked({ name, status: MARK[result.status[name]] }) : name);
  // A binary file has no comparison to show, only a fact about it.
  const BINARY_NOTE = {
    added: m.historyworkspace_binary_added(),
    removed: m.historyworkspace_binary_removed(),
    changed: m.historyworkspace_binary_changed(),
    same: m.historyworkspace_binary_same(),
  };
  const fileNote = $derived(status === "added" ? m.historyworkspace_file_added()
    : status === "removed" ? m.historyworkspace_file_removed()
      : oldText === "" && newText === "" ? m.historyworkspace_empty_file() : "");
</script>

<section class="history-workspace" aria-label={m.historyworkspace_source_label()} aria-busy={source.loading}>
  {#if source.loading}
    <p role="status">{m.historyworkspace_loading_source()}</p>
  {:else if source.problem}
    <div role="alert">
      <p>{source.problem}</p>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => source.select()}>{m.common_retry()}</button>
    </div>
  {:else if result}
    <div class="history-source-bar">
      {#if !result.against && source.selected && heading}
        <!-- Says which past version is on screen, so the reader never has to
             work it out from the list. -->
        <span class="history-source-title"><span class="history-source-name">{heading}</span><span class="panel-meta"> · {m.historyworkspace_historical()}</span></span>
      {/if}
      {#if result.against}
        <!-- The base is the old side and the selected version the new one, so
             the line names them in that order. -->
        <span class="history-source-ends">{result.oldLabel} → {result.newLabel}</span>
        <button type="button" class="btn btn-sm" onclick={() => source.swap()}>{m.historyworkspace_swap()}</button>
        <button type="button" class="btn btn-sm" onclick={() => source.compareWith("")}>{m.historyworkspace_compare_current()}</button>
      {/if}
      <label class="history-source-file">
        {m.common_file()}
        <select class="select" aria-label={m.historyworkspace_file_select()} value={path} onchange={event => source.selectFile(event.currentTarget.value)}>
          {#each result.paths as name (name)}<option value={name}>{nameOf(name)}</option>{/each}
        </select>
      </label>
      <span class="history-source-summary panel-meta">
        {result.changed.length
          ? m.historyworkspace_files_differ({ count: result.changed.length })
          : m.historyworkspace_no_differences()}
      </span>
      <span class="history-source-actions">
        <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => source.select()}>{m.common_refresh()}</button>
        <!-- A moment is a position in the editing history, not a label: it has
             no archive to download and nothing to restore to by name. -->
        {#if canEdit && source.selected && !isMoment(source.selected)}
          <!-- A version's archive is produced on request rather than kept
               ready (§8.5), so the button says what it is waiting on rather
               than sitting quiet or, worse, looking done before the file
               exists. Editor-only, like the rest of this panel: a reader or
               commenter link cannot reach the history routes this asks. -->
          <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={downloading === source.selected}
                  onclick={() => ondownload?.(source.selected)}>
            {downloading === source.selected ? m.historyworkspace_downloading() : m.common_download()}
          </button>
        {/if}
        {#if canEdit && source.selected && !isMoment(source.selected)}
          <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => onrestore?.(source.selected)}>{m.historyworkspace_restore()}</button>
        {/if}
      </span>
    </div>
    {#if downloadProblem}
      <p class="lp-text-error text-sm" role="alert">{downloadProblem}</p>
    {/if}
    {#if binary}
      <p role="status">{BINARY_NOTE[status]}</p>
    {:else if oldText === undefined && newText === undefined}
      <p role="status">{path ? m.historyworkspace_no_text_source() : m.historyworkspace_no_source_files()}</p>
    {:else}
      {#if MergeEditor}
        <MergeEditor {path} oldText={oldText ?? ""} newText={newText ?? ""}
          diff={result.diff} editable={false} baselineLabel={result.oldLabel} targetLabel={result.newLabel}
          note={fileNote} />
      {:else if mergeFailed}
        <p role="alert">{m.historyworkspace_comparison_failed()}</p>
      {:else}
        <p role="status">{m.historyworkspace_loading_comparison()}</p>
      {/if}
    {/if}
  {:else}
    <p role="status">{m.historyworkspace_select_prompt()}</p>
  {/if}
</section>

<style>
  .history-workspace { height: 100%; min-width: 0; display: flex; flex-direction: column; overflow: hidden; }
  .history-workspace > p, .history-workspace > div[role="alert"] { padding: 1rem; }
  .history-source-bar { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.5rem; }
  .history-source-ends, .history-source-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .history-source-name { font-weight: 500; }
  .history-source-file { display: flex; align-items: center; gap: 0.5rem; }
  .history-source-file select { width: auto; min-width: 12rem; }
  .history-source-actions { display: flex; align-items: center; gap: 0.5rem; margin-left: auto; }
  .history-workspace :global(.merge-editor) { flex: 1; min-height: 0; }
</style>
