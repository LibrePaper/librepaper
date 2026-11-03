<script>
  import { loadMergeEditor } from "../../lib/merge-editor.js";
  import { CURRENT, NOTHING } from "../../lib/history-endpoints.js";
  import { endpointLabel, versionLabels } from "../../lib/version-label.js";
  import { m } from "../../lib/paraglide/messages.js";
  import { isMoment } from "../../lib/moment.js";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";

  // The comparison editor is fetched when there is something to compare, not
  // when the reader is built. A static import here would undo the dynamic one
  // Reader.svelte makes for the same component: it puts CodeMirror and the
  // CRDT on the critical path of every reader, including everyone who never
  // opens a version.
  let MergeEditor = $state(null);
  let mergeFailed = $state(false);
  // `versions` is the timeline's labels, oldest first.
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

  // A version's archive and restore read a stored version: the live project,
  // the empty start and a moment are not one.
  const stored = (id) => Boolean(id) && id !== CURRENT && id !== NOTHING && !isMoment(id);
  const olderLabel = $derived(endpointLabel(source.older, labels));
  const newerLabel = $derived(endpointLabel(source.newer, labels));
  const filePosition = $derived((result?.changed ?? []).indexOf(path) + 1);

  // Focus returns to the endpoint button whose selection was cancelled.
  let olderButton = $state(null);
  let newerButton = $state(null);
  $effect(() => {
    if (source.cancelled === "older") olderButton?.focus();
    else if (source.cancelled === "newer") newerButton?.focus();
  });

  // The change being looked at, counted from 0, in the diff below.
  let mergeEditor = $state(null);
  let changeCount = $state(0);
  let activeChange = $state(0);
  const go = (index) => {
    if (index < 0 || index >= changeCount) return;
    activeChange = index;
    mergeEditor?.goToChange(index);
  };
  const keydown = (event) => {
    if (event.key === "Escape" && source.mode !== "normal") {
      event.preventDefault();
      source.cancelSelect();
    } else if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      go(activeChange + (event.key === "ArrowUp" ? -1 : 1));
    }
  };
</script>

<!-- Escape and Alt+Arrow work from anywhere inside, the editor included. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="history-workspace" aria-label={m.historyworkspace_source_label()} aria-busy={source.loading} onkeydown={keydown}>
  <!-- The two ends stay on screen while a comparison loads, so changing one
       does not take the control away from under the reader. Each is a button
       that asks the timeline for a new version to put there. -->
  {#if source.newer}
    <div class="history-source-bar history-endpoints">
      <button type="button" class="history-endpoint" bind:this={olderButton}
              aria-pressed={source.mode === "select-older"}
              title={m.historyworkspace_change_older()}
              aria-label={m.historyworkspace_change_older_aria({ version: olderLabel })}
              onclick={() => source.beginSelect("older")}>
        <span class="history-endpoint-side panel-muted">{m.historyworkspace_older()}</span>
        <span class="history-endpoint-label">{olderLabel}</span>
        <Icon name="pencil" size={14} />
      </button>
      <span class="history-arrow panel-muted"><Icon name="arrow-right" size={16} /></span>
      <button type="button" class="history-endpoint" bind:this={newerButton}
              aria-pressed={source.mode === "select-newer"}
              title={m.historyworkspace_change_newer()}
              aria-label={m.historyworkspace_change_newer_aria({ version: newerLabel })}
              onclick={() => source.beginSelect("newer")}>
        <span class="history-endpoint-side panel-muted">{m.historyworkspace_newer()}</span>
        <span class="history-endpoint-label">{newerLabel}</span>
        <Icon name="pencil" size={14} />
      </button>
      <span class="history-sr" aria-live="polite">{m.historyworkspace_comparing({ older: olderLabel, newer: newerLabel })}</span>
    </div>
  {/if}
  {#if source.loading}
    <p role="status">{m.historyworkspace_loading_source()}</p>
  {:else if source.problem}
    <div role="alert">
      <p>{source.problem}</p>
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => source.reload()}>{m.common_retry()}</button>
    </div>
  {:else if result}
    <div class="history-source-bar">
      <div class="history-source-row">
        {#if result.changed.length > 1}
          <select class="select history-file-select" aria-label={m.historyworkspace_file_select()} value={path} onchange={event => source.selectFile(event.currentTarget.value)}>
            {#each result.changed as name (name)}<option value={name}>{nameOf(name)}</option>{/each}
          </select>
          {#if filePosition}
            <span class="panel-meta panel-muted">{m.historyworkspace_file_position({ index: filePosition, count: result.changed.length })}</span>
          {/if}
        {:else}
          <span class="history-source-name">{path}</span>
          {#if !result.changed.length}
            <span class="panel-meta panel-muted">{m.historyworkspace_no_differences()}</span>
          {/if}
        {/if}
        {#if source.newer === CURRENT}
          <IconButton icon="refresh-cw" label={m.historyworkspace_reload_current()} size="btn-icon-sm" onclick={() => source.reload()} />
        {/if}
        {#if result.changed.length && !binary}
          <span class="history-changes">
            <span class="panel-meta panel-muted">
              {changeCount ? m.historyworkspace_change_position({ index: activeChange + 1, count: changeCount }) : m.historyworkspace_no_changes()}
            </span>
            <IconButton icon="chevron-up" label={m.historyworkspace_previous_change()} size="btn-icon-sm"
                        disabled={activeChange <= 0 || !changeCount} onclick={() => go(activeChange - 1)} />
            <IconButton icon="chevron-down" label={m.historyworkspace_next_change()} size="btn-icon-sm"
                        disabled={activeChange >= changeCount - 1} onclick={() => go(activeChange + 1)} />
          </span>
        {/if}
      </div>
    </div>
    {#if binary}
      <p role="status">{BINARY_NOTE[status]}</p>
    {:else if oldText === undefined && newText === undefined}
      <p role="status">{path ? m.historyworkspace_no_text_source() : m.historyworkspace_no_source_files()}</p>
    {:else}
      {#if fileNote}<p class="history-note panel-meta">{fileNote}</p>{/if}
      {#if MergeEditor}
        <MergeEditor bind:this={mergeEditor} {path} oldText={oldText ?? ""} newText={newText ?? ""}
          diff={result.diff} editable={false} header={false} unifiedBelow={720}
          onchanges={(count) => { changeCount = count; activeChange = 0; }}
          baselineLabel={endpointLabel(result.older, labels)} targetLabel={endpointLabel(result.newer, labels)} />
      {:else if mergeFailed}
        <p role="alert">{m.historyworkspace_comparison_failed()}</p>
      {:else}
        <p role="status">{m.historyworkspace_loading_comparison()}</p>
      {/if}
    {/if}
    {#if downloadProblem}
      <p class="lp-text-error text-sm" role="alert">{downloadProblem}</p>
    {/if}
    <!-- A version's archive is produced on request rather than kept ready
         (§8.5), so the button says what it is waiting on rather than sitting
         quiet or, worse, looking done before the file exists. Editor-only,
         like the rest of this panel: a reader or commenter link cannot reach
         the history routes this asks. -->
    {#if canEdit && (stored(source.older) || stored(source.newer))}
      <div class="history-footer">
        {#if stored(source.older)}
          <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={downloading === source.older}
                  title={m.historyworkspace_download_title({ version: olderLabel })}
                  onclick={() => ondownload?.(source.older)}>
            {downloading === source.older ? m.historyworkspace_downloading() : m.historyworkspace_download_older()}
          </button>
        {/if}
        {#if stored(source.newer)}
          <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={downloading === source.newer}
                  title={m.historyworkspace_download_title({ version: newerLabel })}
                  onclick={() => ondownload?.(source.newer)}>
            {downloading === source.newer ? m.historyworkspace_downloading() : m.historyworkspace_download_newer()}
          </button>
        {/if}
        {#if stored(source.older)}
          <button type="button" class="btn btn-sm lp-control-tonal-brand history-restore"
                  title={m.historyworkspace_restore_title({ version: olderLabel })}
                  onclick={() => onrestore?.(source.older)}>{m.historyworkspace_restore_older()}</button>
        {/if}
      </div>
    {/if}
  {:else}
    <p role="status">{m.historyworkspace_select_prompt()}</p>
  {/if}
</section>

<style>
  .history-workspace { height: 100%; min-width: 0; display: flex; flex-direction: column; overflow: hidden; }
  .history-workspace > p, .history-workspace > div[role="alert"] { padding: 1rem; }
  .history-source-bar { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.5rem; }
  .history-endpoints { flex-direction: row; flex-wrap: wrap; align-items: center; gap: 0.5rem; padding-bottom: 0; }
  .history-endpoint {
    display: flex; align-items: center; gap: 0.5rem; min-width: 0; min-height: 2.25rem; padding: 0 0.625rem;
    font-size: var(--panel-font-size); text-align: left; cursor: pointer;
    color: inherit; background: var(--color-raised); border: 1px solid var(--color-divider); border-radius: 6px;
  }
  .history-endpoint:hover { border-color: var(--color-brand); }
  .history-endpoint[aria-pressed="true"] { border-color: var(--color-brand); box-shadow: 0 0 0 1px var(--color-brand); }
  .history-endpoint:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 2px; }
  .history-endpoint-side { font-size: var(--panel-meta-size); flex: none; }
  .history-endpoint-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .history-arrow { display: flex; align-items: center; }
  .history-sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .history-source-row { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; min-width: 0; }
  .history-source-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .history-file-select { width: auto; min-width: 12rem; max-width: 100%; }
  .history-changes { display: flex; align-items: center; gap: 0.25rem; margin-left: auto; }
  .history-footer { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; padding: 0.5rem; border-top: 1px solid var(--color-divider); }
  .history-restore { margin-left: auto; }
  .history-note { padding: 0 0.5rem; }
  .history-workspace :global(.merge-editor) { flex: 1; min-height: 0; }
</style>
