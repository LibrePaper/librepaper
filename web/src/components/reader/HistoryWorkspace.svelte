<script>
  import { loadMergeEditor } from "../../lib/merge-editor.js";
  import { CURRENT, rankOf } from "../../lib/history-endpoints.js";
  import { endpointLabel, versionLabels } from "../../lib/version-label.js";
  import { m } from "../../lib/paraglide/messages.js";
  import IconButton from "../IconButton.svelte";

  // The comparison editor is fetched when there is something to compare, not
  // when the reader is built. A static import here would undo the dynamic one
  // Reader.svelte makes for the same component: it puts CodeMirror and the
  // CRDT on the critical path of every reader, including everyone who never
  // opens a version.
  let MergeEditor = $state(null);
  let mergeFailed = $state(false);
  // `versions` is the timeline's labels, oldest first. The selects list them
  // newest first.
  let { source, versions = [], canEdit = false, onrestore, ondownload, downloading = "", downloadProblem = "" } = $props();
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

  // The labels are read in this browser's time zone, as the history list reads them.
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const labels = $derived(versionLabels(versions, { timeZone }));
  const shas = $derived(versions.map((point) => point.sha).reverse());
  // A value that is neither a version nor the current one, such as the
  // beginning of history or a moment, still needs an option, or its select
  // would show nothing.
  const extra = (value) => (value && value !== CURRENT && !shas.includes(value) ? [value] : []);
  const olderChoices = $derived([...shas, ...extra(source.older)]);
  const newerChoices = $derived([CURRENT, ...shas, ...extra(source.newer)]);
  // Older has to come before Newer. An endpoint that is not in the timeline
  // cannot be ranked, and is left selectable rather than guessed at.
  const before = (older, newer) => {
    const low = rankOf(older, versions), high = rankOf(newer, versions);
    return low === null || high === null || low < high;
  };
</script>

<section class="history-workspace" aria-label={m.historyworkspace_source_label()} aria-busy={source.loading}>
  {#if source.loading}
    <p role="status">{m.historyworkspace_loading_source()}</p>
  {:else if source.problem}
    <div role="alert">
      <p>{source.problem}</p>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => source.reload()}>{m.common_retry()}</button>
    </div>
  {:else if result}
    <div class="history-source-bar">
      <div class="history-versions">
        <label class="history-field">
          <span>{m.historyworkspace_older_version()}</span>
          <select class="select" value={source.older} onchange={event => source.setOlder(event.currentTarget.value)}>
            {#each olderChoices as sha (sha)}
              <option value={sha} disabled={!before(sha, source.newer)}>{endpointLabel(sha, labels)}</option>
            {/each}
          </select>
        </label>
        <label class="history-field">
          <span>{m.historyworkspace_newer_version()}</span>
          <select class="select" value={source.newer} onchange={event => source.setNewer(event.currentTarget.value)}>
            {#each newerChoices as sha (sha)}
              <option value={sha} disabled={!before(source.older, sha)}>{endpointLabel(sha, labels)}</option>
            {/each}
          </select>
        </label>
      </div>
      <div class="history-source-row">
        {#if result.changed.length > 1}
          <label class="history-source-file">
            {m.common_file()}
            <select class="select" aria-label={m.historyworkspace_file_select()} value={path} onchange={event => source.selectFile(event.currentTarget.value)}>
              {#each result.paths as name (name)}<option value={name}>{nameOf(name)}</option>{/each}
            </select>
          </label>
        {:else}
          <span class="history-source-name">{path}</span>
        {/if}
        <span class="history-source-summary panel-meta">
          {result.changed.length
            ? m.historyworkspace_files_changed({ count: result.changed.length })
            : m.historyworkspace_no_differences()}
        </span>
        <span class="history-source-actions">
          {#if source.newer === CURRENT}
            <IconButton icon="refresh-cw" label={m.historyworkspace_reload_current()} size="btn-icon-sm" onclick={() => source.reload()} />
          {/if}
          <!-- A version's archive is produced on request rather than kept
               ready (§8.5), so the button says what it is waiting on rather
               than sitting quiet or, worse, looking done before the file
               exists. Editor-only, like the rest of this panel: a reader or
               commenter link cannot reach the history routes this asks. -->
          {#if canEdit && source.target}
            <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={downloading === source.target}
                    title={m.historyworkspace_download_title({ version: endpointLabel(source.target, labels) })}
                    onclick={() => ondownload?.(source.target)}>
              {downloading === source.target ? m.historyworkspace_downloading() : m.historyworkspace_download_version()}
            </button>
            <button type="button" class="btn btn-sm lp-control-tonal-brand"
                    title={m.historyworkspace_restore_title({ version: endpointLabel(source.target, labels) })}
                    onclick={() => onrestore?.(source.target)}>{m.historyworkspace_restore_version()}</button>
          {/if}
        </span>
      </div>
    </div>
    {#if downloadProblem}
      <p class="lp-text-error text-sm" role="alert">{downloadProblem}</p>
    {/if}
    {#if binary}
      <p role="status">{BINARY_NOTE[status]}</p>
    {:else if oldText === undefined && newText === undefined}
      <p role="status">{path ? m.historyworkspace_no_text_source() : m.historyworkspace_no_source_files()}</p>
    {:else}
      {#if fileNote}<p class="history-note panel-meta">{fileNote}</p>{/if}
      {#if result.diff}
        <div class="history-sides panel-meta" aria-hidden="true">
          <span>{m.historyworkspace_older()}</span><span>{m.historyworkspace_newer()}</span>
        </div>
      {/if}
      {#if MergeEditor}
        <MergeEditor {path} oldText={oldText ?? ""} newText={newText ?? ""}
          diff={result.diff} editable={false} header={false}
          baselineLabel={endpointLabel(result.older, labels)} targetLabel={endpointLabel(result.newer, labels)} />
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
  .history-source-bar { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.5rem; }
  /* Two fields side by side while they fit, one above the other when not. */
  .history-versions { display: flex; flex-wrap: wrap; gap: 0.5rem; }
  .history-field { display: flex; flex: 1 1 12rem; flex-direction: column; gap: 0.25rem; min-width: 0; }
  .history-field select { width: 100%; }
  .history-source-row { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; min-width: 0; }
  .history-source-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .history-source-file { display: flex; align-items: center; gap: 0.5rem; }
  .history-source-file select { width: auto; min-width: 12rem; }
  .history-source-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; margin-left: auto; }
  /* Half and half, as the two panes of the merge view below. */
  .history-sides { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem; padding: 0 0.5rem; }
  .history-note { padding: 0 0.5rem; }
  .history-workspace :global(.merge-editor) { flex: 1; min-height: 0; }
</style>
