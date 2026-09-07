<script>
  // The timeline, as a column beside the document.
  //
  // A history is read backwards -- what happened, then what happened before
  // that -- so the newest day is at the top and the newest mark within it. A
  // labelled checkpoint is what somebody came here to find, so it is the one
  // thing in the column that is not grey; a working afternoon of quiet marks
  // by one author is folded to its first and last, because thirty rows of the
  // same name say less than three do.
  //
  // Nothing here fetches. The panel is given the checkpoints and reports what
  // was clicked; the page it sits in owns the requests, as it owns every other
  // one.
  import { shortSha, timeline } from "../lib/history.js";
  import IconButton from "./IconButton.svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import CopyLink from "./CopyLink.svelte";

  let {
    checkpoints = [],
    viewing = null,
    canEdit = false,
    baseline = null,
    currentLabel = "now",
    target = null,
    ontarget,
    changes = null,
    changedPaths = [],
    redlines = false,
    onredlines,
    redlinesDisabledReason = "",
    fileDiff = null,
    onclosefilediff,
    onbaseline,
    onreveal,
    onfilediff,
    oncheckpointfile,
    onrestore,
    oncopy,
    problem = "",
    onshow,
    onback,
    onname,
  } = $props();

  // The runs a reader has asked to see inside, by the SHA of the row that
  // stands for them. Forgotten when the panel closes, which is the right
  // lifetime: it is a glance, not a setting.
  let opened = $state(new Set());

  // The checkpoint being named, and what it is being named. One at a time,
  // because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  const days = $derived(timeline(checkpoints));

  function unfold(row) {
    const next = new Set(opened);
    next.add(row.first.sha);
    opened = next;
  }

  // The time alone: the day is the heading above it, and repeating the date on
  // every row is thirty copies of what the reader just read.
  function clock(at) {
    const when = new Date(at);
    return Number.isNaN(when.getTime())
      ? ""
      : when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }

  // What a checkpoint was taken for, in words rather than in the manifest's
  // own vocabulary. `quiet` and `left` are the two nobody asked for, and
  // calling them "autosaved" would say the wrong thing about who was there:
  // what they have in common is that the author stopped.
  const WHY = {
    quiet: "paused",
    left: "closed the tab",
    comment: "someone commented",
    cli: "published",
    sync: "synced",
    restore: "restored",
    label: "named",
    recovered: "recovered",
    accept: "accepted a suggestion",
  };
  const reason = (why) => WHY[why] || why;

  async function startNaming(point) {
    naming = point.sha;
    draft = point.label || "";
    await Promise.resolve();
    field?.select();
  }

  function finishNaming() {
    const sha = naming;
    const given = draft.trim();
    naming = "";
    onname?.(sha, given);
  }

  // The caller obtains these from the shared WASM word-diff service. Keeping
  // the component presentation-only ensures its hunk labels and anchors are
  // identical to the response export and the sync merge implementation.
  const visibleChanges = $derived(Array.isArray(changes) ? changes : []);
  const paths = $derived([...new Set([...(changedPaths || []), ...visibleChanges.map((hunk) => hunk.path).filter(Boolean)])]);
  const baselineName = $derived(baseline?.label || (baseline ? shortSha(baseline.sha) : ""));

  function showHunk(hunk) {
    const quote = String(hunk.new || "");
    if (quote) onreveal?.({ ...hunk, exact: quote });
  }
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="panel timeline">
  <PanelHeader
    title="History"
    meta={checkpoints.length
      ? `${checkpoints.length} checkpoint${checkpoints.length === 1 ? "" : "s"}`
      : undefined}
  >
    {#if problem}
      <p class="text-error-500">{problem}</p>
    {:else if checkpoints.length === 0}
      <p class="panel-muted">
        Nothing yet. A checkpoint is taken when the typing stops, when the last
        editor leaves, and whenever the document is published to.
      </p>
    {:else if viewing}
      <p class="panel-muted">
        Showing an earlier version. The document pane is what it said then.
      </p>
    {/if}
  </PanelHeader>

  {#if checkpoints.length > 0}
    <section class="history-changes border-surface-200-800 border-b px-3 py-3">
      <div class="mb-2 flex items-center justify-between gap-2">
        <h3 class="panel-section-title m-0">What changed since</h3>
        {#if baseline}
          <small class="panel-meta">{baselineName} → {currentLabel}</small>
        {/if}
      </div>
      <label class="label mb-2 flex items-center gap-2" title={redlinesDisabledReason || undefined}>
        <input
          type="checkbox"
          class="checkbox"
          checked={redlines}
          disabled={Boolean(redlinesDisabledReason)}
          onchange={(event) => onredlines?.(event.currentTarget.checked)}
        />
        <span class="label-text text-xs">Show in document</span>
      </label>
      {#if redlinesDisabledReason}
        <p class="panel-muted text-xs mb-2">{redlinesDisabledReason}</p>
      {/if}
      <label class="label mb-2">
        <span class="label-text text-xs">Compare with</span>
        <select class="select select-sm" value={baseline?.sha || ""} onchange={(event) => onbaseline?.(event.currentTarget.value)}>
          <option value="" disabled>Choose a checkpoint</option>
          {#each checkpoints as point (point.sha)}
            <option value={point.sha}>{point.label || shortSha(point.sha)} · {clock(point.at)}</option>
          {/each}
        </select>
      </label>
      <label class="label mb-2">
        <span class="label-text text-xs">Compare to</span>
        <select class="select select-sm" value={target?.sha || ""} onchange={(event) => ontarget?.(event.currentTarget.value)}>
          <option value="">Live document (now)</option>
          {#each checkpoints as point (point.sha)}
            {#if point.sha !== baseline?.sha}
              <option value={point.sha}>{point.label || shortSha(point.sha)} · {clock(point.at)}</option>
            {/if}
          {/each}
        </select>
      </label>
      {#if !baseline}
        <p class="panel-muted text-sm">Choose a checkpoint to see the words that changed.</p>
      {:else if problem}
        <p class="panel-muted text-sm">The passage comparison is unavailable.</p>
      {:else if viewing && !target}
        <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onback?.()}>Back to now to compare changes</button>
      {:else if changes === null}
        <p class="panel-muted text-sm" role="status">Loading changes…</p>
      {:else if !visibleChanges.length}
        <p class="panel-muted text-sm">No text changed after this checkpoint.</p>
      {:else}
        <ol class="history-hunks flex flex-col gap-2">
          {#each visibleChanges as hunk, index (`${hunk.path || ""}-${index}`)}
            <li class="history-hunk card preset-outlined-surface-200-800 p-2">
              <div class="mb-1 flex items-center justify-between gap-2">
                <span class="truncate text-xs">{hunk.path || "document"}</span>
                {#if hunk.old && hunk.new}<span class="badge preset-tonal-tertiary">changed</span>
                {:else if hunk.new}<span class="badge preset-tonal-secondary">inserted</span>
                {:else}<span class="badge preset-tonal-warning">deleted</span>{/if}
              </div>
              {#if hunk.old}<div class="history-old text-xs">− {hunk.old}</div>{/if}
              {#if hunk.new}<div class="history-new text-xs">+ {hunk.new}</div>{/if}
              {#if hunk.before || hunk.after}
                <div class="panel-muted mt-1 text-xs">… {hunk.contextBefore ?? hunk.before ?? ""} <strong>{hunk.new || hunk.old || ""}</strong> {hunk.contextAfter ?? hunk.after ?? ""} …</div>
              {/if}
              {#if hunk.new}
                <button type="button" class="btn btn-sm preset-tonal-primary mt-2" onclick={() => showHunk(hunk)}>Reveal in document</button>
              {/if}
            </li>
          {/each}
        </ol>
      {/if}
      {#if paths.length}
        <div class="mt-3 flex flex-col gap-1">
          <span class="panel-meta">Changed files</span>
          {#each paths as path (path)}
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700 justify-start" onclick={() => onfilediff?.(path)}>{path}</button>
          {/each}
        </div>
      {/if}
      {#if fileDiff}
        <section class="history-file-diff mt-3" aria-label="File diff">
          <div class="mb-1 flex items-center justify-between gap-2">
            <strong class="text-sm truncate">{fileDiff.path}</strong>
            <button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => onclosefilediff?.()}>Close</button>
          </div>
          {#if fileDiff.loading}
            <p class="panel-muted text-sm" role="status">Loading comparison…</p>
          {:else if fileDiff.problem}
            <p class="panel-muted text-sm">{fileDiff.problem}</p>
          {:else if fileDiff.old == null && fileDiff.new == null}
            <p class="panel-muted text-sm">{!fileDiff.oldEntry ? "File added." : !fileDiff.newEntry ? "File removed." : "Binary file changed."}</p>
          {:else if !fileDiff.hunks?.length}
            <p class="panel-muted text-sm">{fileDiff.old == null ? "Empty file added." : fileDiff.new == null ? "Empty file removed." : "The text is unchanged."}</p>
          {:else}
            <div class="history-file-diff-body rounded bg-surface-100-900 p-2 text-xs">
              {#each fileDiff.hunks as hunk}
                <div class="mb-3 whitespace-pre-wrap">
                  <span class="panel-muted">{hunk.before}</span>{#if hunk.old}<del class="history-old">{hunk.old}</del>{/if}{#if hunk.insert}<ins class="history-new">{hunk.insert}</ins>{/if}<span class="panel-muted">{hunk.after}</span>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {/if}
    </section>
  {/if}

  {#each days as { day, rows } (day)}
    <h4 class="panel-section-title timeline-day sticky top-0 z-1 py-2">{day}</h4>
    <ol class="mb-3 flex flex-col gap-1">
      {#each rows as row (row.kind === "point" ? row.point.sha : row.first.sha)}
        {#if row.kind === "point"}
          {@render mark(row.point)}
        {:else if opened.has(row.first.sha)}
          {@render mark(row.first)}
          {#each row.hidden as point (point.sha)}
            {@render mark(point)}
          {/each}
          {@render mark(row.last)}
        {:else}
          {@render mark(row.first)}
          <li>
            <button type="button" class="timeline-folded" onclick={() => unfold(row)}>
              {row.hidden.length} more by {row.first.by || "somebody"}
            </button>
          </li>
          {@render mark(row.last)}
        {/if}
      {/each}
    </ol>
  {/each}
</div>

{#snippet mark(point)}
  <li>
    {#if naming === point.sha}
      <!-- Naming happens where the name will appear, rather than in a dialog
           over the list: what is being named is the row under the cursor. -->
      <input
        bind:this={field}
        bind:value={draft}
        class="input"
        placeholder="sent to the journal"
        aria-label="Name this point"
        onkeydown={(event) => {
          if (event.key === "Enter") finishNaming();
          if (event.key === "Escape") naming = "";
        }}
        onblur={finishNaming}
      />
    {:else}
      <button
        type="button"
        class="timeline-point"
        class:timeline-named={Boolean(point.label)}
        class:timeline-here={viewing === point.sha}
        aria-current={viewing === point.sha ? "true" : undefined}
        title="Show the document as it was at {shortSha(point.sha)}"
        onclick={() => (viewing === point.sha ? onback?.() : onshow?.(point.sha))}
      >
        <span class="timeline-when panel-meta">{clock(point.at)}</span>
        <span class="timeline-what">
          {#if point.label}<strong>{point.label}</strong><br />{/if}
          {point.by || "somebody"} · {reason(point.why)}
        </span>
      </button>
      {#if canEdit}
        <IconButton
          icon="pencil"
          tone="plain"
          size="btn-icon-sm"
          label={point.label ? "Rename this point" : "Name this point"}
          onclick={() => startNaming(point)}
        />
        <IconButton icon="history" tone="plain" size="btn-icon-sm" label="Restore this checkpoint" onclick={() => onrestore?.(point.sha)} />
      {/if}
      <CopyLink href={oncopy?.(point.sha) || undefined} label="Copy the link to this checkpoint" />
    {/if}
  </li>
  {#if point.parent && point.changed?.length}
    <li class="flex flex-wrap gap-1 px-3 pb-2">
      {#each point.changed as path (path)}
        <button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => oncheckpointfile?.(point, path)} title="Compare this file with the previous checkpoint">{path}</button>
      {/each}
    </li>
  {/if}
{/snippet}
