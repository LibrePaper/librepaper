<script>
  import { onMount, untrack } from "svelte";
  import { loadQuotaPreferences, validateTimezone } from "../../lib/quota-preferences.js";
  import { day as dayOf } from "../../lib/dates.js";
  // The timeline, as a column beside the document.
  //
  // A history starts with the current document and reads backwards through
  // progressively older versions. A labelled checkpoint is what
  // somebody came here to find, so it is the one thing in the column that is
  // not grey; a working afternoon of quiet marks by one author is folded to
  // its first and last, because thirty rows of the same name say less than
  // three do.
  //
  // The changes themselves are not listed here. They are painted into the
  // document, the way a version history does it: clicking a row shows the
  // document as it was then, with what changed since the baseline struck
  // through and underlined in place.
  //
  // What the comparison covers, how many changes are in it, and the files
  // they touched appear under the row being looked at -- one row at a time --
  // rather than at the head of the panel. They sat at the head for a while
  // and took half its height, so the column read as three things at once:
  // versions in time, the edits inside one of them, and the files those edits
  // touched. The timeline answers when and which event actor recorded it; the document answers
  // what. The list of changes as prose is still there, folded away.
  //
  // Nothing here fetches. The panel is given the checkpoints and reports what
  // was clicked; the page it sits in owns the requests, as it owns every other
  // one.
  import { coalesce, shortSha, timeline } from "../../lib/history.js";
  import IconButton from "../IconButton.svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import CopyLink from "../CopyLink.svelte";

  let {
    checkpoints = [],
    durability = null,
    viewing = null,
    canEdit = false,
    baseline = null,
    target = null,
    changes = null,
    changedPaths = [],
    redlines = true,
    onredlines,
    fileDiff = null,
    onclosefilediff,
    onview,
    oncompare,
    oncomparecurrent,
    onrefreshcurrent,
    comparingCurrent = false,
    newerEdits = false,
    onstep,
    onfilediff,
    oncheckpointfile,
    onrestore,
    oncopy,
    problem = "",
    onback,
    onname,
    currentLabel = "",
    path = "",
  } = $props();

  // The runs a reader has asked to see inside, by the SHA of the row that
  // stands for them. Forgotten when the panel closes, which is the right
  // lifetime: it is a glance, not a setting.
  let opened = $state(new Set());
  let filter = $state("all");
  let comparison = $state("current");
  let lastRevealed = $state("");
  let timezone = $state("UTC");
  onMount(() => {
    let active = true;
    const refresh = () => loadQuotaPreferences().then((snapshot) => {
      const chosen = snapshot.preferences?.displayTimezone;
      if (active && validateTimezone(chosen)) timezone = chosen;
    }).catch(() => {});
    void refresh();
    window.addEventListener("librepaper-quota-preferences", refresh);
    return () => { active = false; window.removeEventListener("librepaper-quota-preferences", refresh); };
  });

  // The checkpoint being named, and what it is being named. One at a time,
  // because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // History follows the file currently open in the editor when the manifest
  // carries per-checkpoint `changed` paths. A checkpoint is relevant only
  // when those paths include that file, so an edit confined to references.bib
  // does not appear while manuscript.tex is selected. Changing files changes
  // this list without changing or discarding the document-wide manifest.
  // PostgreSQL manifests created before path summaries were added omit that
  // evidence entirely; show their checkpoints rather than presenting an
  // empty history and falsely implying that revisions were never saved.
  //
  // The named/published filter is applied after that file scope. A selected
  // checkpoint remains visible through those secondary filters, but never
  // leaks into the history of a file it did not change.
  const hasFileScope = $derived(checkpoints.some((point) =>
    Array.isArray(point.changed) && point.changed.length > 0));
  const fileCheckpoints = $derived(path && hasFileScope
    ? checkpoints.filter((point) => Array.isArray(point.changed) && point.changed.includes(path))
    : checkpoints);
  const timelinePoints = $derived.by(() => {
    if (filter === "all") return fileCheckpoints;
    const selected = fileCheckpoints.find((point) => point.sha === viewing);
    return fileCheckpoints.filter((point) =>
      (filter === "named" ? point.label : point.why === "cli") || point.sha === selected?.sha,
    );
  });
  const days = $derived(timeline(timelinePoints, timezone));
  const namedCount = $derived(fileCheckpoints.filter((point) => point.label).length);
  const selectedPoint = $derived(checkpoints.find((point) => point.sha === viewing) || null);

  // Where each checkpoint stands in the manifest, oldest first, so that the
  // range can be drawn: every row strictly after the baseline and up to the
  // compare point -- or up to the live document, when there is none.
  const order = $derived(new Map(checkpoints.map((point, at) => [point.sha, at])));
  const baselineAt = $derived(baseline ? (order.get(baseline.sha) ?? -1) : -1);
  const targetAt = $derived(target ? (order.get(target.sha) ?? -1) : checkpoints.length);
  const inRange = (sha) => {
    const at = order.get(sha);
    return baselineAt >= 0 && at !== undefined && at > baselineAt && at <= targetAt;
  };
  // Whether the bracket runs through a day heading: it does when the start
  // of the range is on or below the heading and the end is above it.
  const throughDay = (rows) => {
    const first = rows[0];
    const sha = first?.kind === "point" ? first.point.sha : first?.first.sha;
    const at = order.get(sha);
    return baselineAt >= 0 && at !== undefined && at >= baselineAt && at < targetAt;
  };

  function unfold(row) {
    const next = new Set(opened);
    next.add(row.first.sha);
    opened = next;
  }

  function toggleFold(row) {
    const next = new Set(opened);
    if (next.has(row.first.sha)) next.delete(row.first.sha);
    else next.add(row.first.sha);
    opened = next;
  }

  // A direct link may point into a collapsed session. Reveal that row as soon
  // as it becomes the selected preview.
  $effect(() => {
    if (!viewing) {
      lastRevealed = "";
      return;
    }
    const row = days.flatMap(({ rows }) => rows).find((candidate) =>
      candidate.kind === "folded" && [candidate.first, ...candidate.hidden, candidate.last].some((point) => point.sha === viewing),
    );
    const key = row ? `${viewing}:${row.first.sha}` : "";
    if (row && key !== lastRevealed) {
      const next = new Set(untrack(() => opened));
      next.add(row.first.sha);
      opened = next;
      lastRevealed = key;
    }
  });

  // The time alone: the day is the heading above it, and repeating the date on
  // every row is thirty copies of what the reader just read.
  function clock(at) {
    const when = new Date(at);
    return Number.isNaN(when.getTime())
      ? ""
      : when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", timeZone: timezone });
  }

  function stamp(at) {
    const when = new Date(at);
    return Number.isNaN(when.getTime()) ? "" : when.toLocaleString([], {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: timezone,
    });
  }

  function dayLabel(day) {
    if (!day) return "Unknown date";
    const today = new Date();
    const todayKey = dayOf(today, timezone);
    // Shift the target-timezone calendar date in UTC at noon. Mutating a
    // browser-local Date crosses the wrong DST boundary when the reader's
    // timezone differs from the browser's, and a 24-hour subtraction is not
    // a calendar-day operation on DST transitions.
    const shiftCalendarDay = (value, offset) => {
      const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value || "");
      if (!match) return "";
      const shifted = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3]) + offset, 12));
      return Number.isNaN(shifted.getTime()) ? "" : dayOf(shifted, "UTC");
    };
    const yesterdayKey = shiftCalendarDay(todayKey, -1);
    if (day === todayKey) return "Today";
    if (day === yesterdayKey) return "Yesterday";
    const date = new Date(`${day}T12:00:00Z`);
    return Number.isNaN(date.getTime()) ? day : date.toLocaleDateString([], {
      year: "numeric", month: "short", day: "numeric", timeZone: timezone,
    });
  }

  function sessionSummary(row) {
    const count = row.hidden.length + 2;
    const range = `${clock(row.last.at)}–${clock(row.first.at)}`;
    return `${range} · ${actor(row.first)} · ${count} versions`;
  }

  // What a checkpoint was taken for, in words rather than in the manifest's
  // own vocabulary. `quiet` and `left` are the two nobody asked for, and
  // calling them "autosaved" would say the wrong thing about who was there:
  // what they have in common is that the author stopped. They are the common
  // case, so the row does not say them; the tooltip does.
  const WHY = {
    quiet: "paused",
    left: "closed the tab",
    comment: "commented",
    cli: "published",
    sync: "synced",
    restore: "restored",
    label: "named",
    recovered: "recovered",
    accept: "accepted a suggestion",
  };
  const QUIET = new Set(["quiet", "left", "automatic", "sync"]);
  const reason = (why) => WHY[why] || why;
  const said = (point) => (QUIET.has(point.why) ? "" : reason(point.why));
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const actor = (point) => {
    if (point?.label) return "";
    if (QUIET.has(point?.why) && !point?.label) return "Autosaved";
    if (!point?.by || point.by === "system") return "Autosaved";
    return uuid.test(point.by) ? "Unknown editor" : point.by;
  };
  const important = (point) => Boolean(point.label) || !QUIET.has(point.why);

  async function startNaming(point) {
    naming = point.sha;
    draft = point.label || "";
    await Promise.resolve();
    field?.select();
  }

  function finishNaming() {
    if (!naming) return;
    const sha = naming;
    const given = draft.trim();
    naming = "";
    if (sha === "current" && !given) return;
    onname?.(sha, given);
  }

  // The caller obtains these from the shared WASM word-diff service. Keeping
  // the component presentation-only ensures the offsets it steps through are
  // the offsets the frame painted the redlines at.
  const visibleChanges = $derived(Array.isArray(changes) ? changes : []);
  const groups = $derived(coalesce(visibleChanges));
  const paths = $derived([...new Set([...(changedPaths || []), ...visibleChanges.map((hunk) => hunk.path).filter(Boolean)])]);
  const name = (point) => (point ? point.label || stamp(point.at) || "Unnamed version" : "");

  // The words around a change, a few of them: the diff keeps six on each
  // side so that a deletion still has a place, but a row in a narrow column
  // reads best with three.
  const clip = (text, fromEnd) => {
    const words = String(text || "").split(/\s+/).filter(Boolean);
    const kept = fromEnd ? words.slice(-3) : words.slice(0, 3);
    const trimmed = kept.length < words.length;
    return fromEnd
      ? `${trimmed ? "… " : ""}${kept.join(" ")} `
      : ` ${kept.join(" ")}${trimmed ? " …" : ""}`;
  };

  // Which change the reader is at, stepping with the arrows. A new
  // comparison starts over: the old position meant nothing in it.
  let step = $state(-1);
  $effect(() => {
    void changes;
    step = -1;
  });
  function goTo(index) {
    if (!groups.length) return;
    step = ((index % groups.length) + groups.length) % groups.length;
    onstep?.(groups[step]);
  }
  const count = $derived(
    step >= 0
      ? `Change ${step + 1} of ${groups.length}`
      : groups.length === 1 ? "1 change" : `${groups.length} changes`,
  );

  // `]` and `[` step the same as the two arrow buttons, for whoever would
  // rather keep a hand on the keyboard. They are ignored while the target is
  // somewhere a bracket means something else: a field, or the editor.
  function stepKey(event) {
    if (event.key !== "]" && event.key !== "[") return;
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const target = event.target;
    if (target?.closest?.("input, textarea, select, [contenteditable], .cm-content")) return;
    event.preventDefault();
    goTo(step + (event.key === "]" ? 1 : -1));
  }
  $effect(() => {
    window.addEventListener("keydown", stepKey);
    return () => window.removeEventListener("keydown", stepKey);
  });
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="panel timeline">
  <PanelHeader
    title="Version history"
    meta={checkpoints.length
      ? `${fileCheckpoints.length} version${fileCheckpoints.length === 1 ? "" : "s"}`
      : undefined}
  >
    {#if problem && checkpoints.length === 0}
      <p class="text-error-500" role="status">{problem}</p>
    {:else if checkpoints.length === 0}
      <p class="panel-muted">
        Nothing yet. A version is saved when the typing stops, when the last
        editor leaves, and whenever the document is published to.
      </p>
    {:else if fileCheckpoints.length === 0}
      <p class="panel-muted">No saved revisions for {path || "this file"}.</p>
    {/if}
  </PanelHeader>
  {#if durability?.live_save === "pending"}
    <p class="panel-muted px-2 py-1 text-xs" role="status">Live edits are still being saved.</p>
  {/if}
  {#if fileCheckpoints.length > 0}
    <div class="history-filter px-2 py-1">
      <label>
        <span class="sr-only">Compare selected checkpoint</span>
        <select class="select" aria-label="Compare selected checkpoint" bind:value={comparison} onchange={() => viewing && onview?.(viewing, comparison)}>
          <option value="previous">vs previous</option>
          <option value="current">vs current</option>
          <option value="checkpoint">checkpoint</option>
        </select>
      </label>
      <label>
        <span class="sr-only">Filter version history</span>
        <select class="select" aria-label="Filter version history" bind:value={filter}>
          <option value="all">All versions</option>
          <option value="named">Named versions</option>
          <option value="published">Published versions</option>
        </select>
      </label>
    </div>
    {#if filter === "named" && namedCount === 0}
      <p class="panel-muted px-2 py-1 text-sm" role="status">No named versions yet. Name a version to find it here.</p>
    {/if}
  {/if}

  {#if checkpoints.length > 0}
    <ol class="timeline-list">
      <li class="timeline-section"><h4>Current</h4></li>
      <!-- The live document is a row like the others, hollow because it is
           not a checkpoint yet. -->
      <li
        class="timeline-row timeline-now"
        class:in-range={!target && baselineAt >= 0}
        class:range-end={!target && baselineAt >= 0}
      >
        {#if naming === "current"}
          {@render nameField("Name this version", "Name the current version")}
        {:else}<button
          type="button"
          class="timeline-point"
          class:timeline-here={!viewing}
          aria-current={!viewing ? "true" : undefined}
          title="Show the live document"
          onclick={() => onview?.("")}
        >
          <span class="timeline-marker timeline-marker-current"></span>
          <span class="timeline-what"><strong>{currentLabel || "Current version"}</strong><span class="timeline-who">Saved just now</span></span>
        </button>
        {/if}
        {#if canEdit && !viewing && naming !== "current"}
          <span class="timeline-inline-action">
            <IconButton icon="pencil" tone="plain" size="btn-icon-sm" label="Name the current version" onclick={() => startNaming({ sha: "current", label: currentLabel })} />
          </span>
        {/if}
      </li>
      {#each days as { day, rows } (day)}
        <li class="timeline-day-row" class:in-range={throughDay(rows)}>
          <h4 class="panel-section-title timeline-day sticky top-0 z-1">{dayLabel(day)}</h4>
        </li>
        {#each rows as row (row.kind === "point" ? row.point.sha : row.first.sha)}
          {#if row.kind === "point"}
            {@render mark(row.point)}
          {:else if opened.has(row.first.sha)}
            {@render mark(row.first)}
            {#each row.hidden as point (point.sha)}
              {@render mark(point)}
            {/each}
            {@render mark(row.last)}
            <li class="timeline-row timeline-fold">
              <button type="button" class="timeline-folded" onclick={() => toggleFold(row)} aria-label="Collapse editing session">Collapse session</button>
            </li>
          {:else}
            {@render mark(row.first)}
            <li class="timeline-row timeline-fold" class:in-range={inRange(row.hidden[0].sha)}>
              <button type="button" class="timeline-folded" onclick={() => onview?.(row.first.sha)} title="Show the newest version in this session">
                {sessionSummary(row)}
              </button>
              <IconButton icon="chevron-down" tone="plain" size="btn-icon-sm" label="Expand editing session" onclick={() => unfold(row)} />
            </li>
            {@render mark(row.last)}
          {/if}
        {/each}
      {/each}
    </ol>
  {/if}
</div>

{#snippet nameField(placeholder, label)}
  <input
    bind:this={field}
    bind:value={draft}
    class="input"
    {placeholder}
    aria-label={label}
    onkeydown={(event) => {
      if (event.key === "Enter") finishNaming();
      if (event.key === "Escape") naming = "";
    }}
    onblur={finishNaming}
  />
{/snippet}

{#snippet mark(point)}
  <li
    class="timeline-row"
    class:in-range={inRange(point.sha)}
    class:range-start={baseline?.sha === point.sha}
    class:range-end={target?.sha === point.sha}
    data-sha={point.sha}
  >
    {#if naming === point.sha}
      <!-- Naming happens where the name will appear, rather than in a dialog
           over the list: what is being named is the row under the cursor. -->
      {@render nameField("sent to the journal", "Name this point")}
    {:else}
      <button
        type="button"
        class="timeline-point"
        class:timeline-named={Boolean(point.label)}
        class:timeline-here={viewing === point.sha}
        aria-current={viewing === point.sha ? "true" : undefined}
        title="Show {point.label || actor(point)} from {stamp(point.at) || shortSha(point.sha)}"
        onclick={() => onview?.(point.sha, comparison)}
      >
        <span class="timeline-marker" class:timeline-marker-important={important(point)}></span>
        <span class="timeline-when panel-meta">{clock(point.at)}</span>
        <span class="timeline-what">
          {#if point.label}<strong>{point.label}</strong>{/if}
          {#if actor(point)}<span class="timeline-who">{actor(point)}{#if said(point)} · {said(point)}{/if}</span>{/if}
        </span>
      </button>
      {#if viewing === point.sha}
        <span class="timeline-inline-actions">
          {#if canEdit}
            <IconButton
              icon="pencil"
              tone="plain"
              size="btn-icon-sm"
              label={point.label ? "Rename this version" : "Name this version"}
              onclick={() => startNaming(point)}
            />
            <IconButton icon="history" tone="plain" size="btn-icon-sm" label="Restore this version" onclick={() => onrestore?.(point.sha)} />
          {/if}
          <CopyLink href={oncopy?.(point.sha) || undefined} label="Copy the link to this version" tone="plain" />
        </span>
      {/if}
    {/if}
  </li>
  {#if viewing === point.sha && point.parent && point.changed?.length > 1}
    <!-- The files this checkpoint touched, under the row being looked at and
         no other: each opens against the checkpoint before. -->
    <li class="timeline-row timeline-files" class:in-range={inRange(point.sha)}>
      {#each point.changed as path (path)}
        <button type="button" class="timeline-file" onclick={() => oncheckpointfile?.(point, path)} title="Compare this file with the previous version">{path}</button>
      {/each}
    </li>
  {/if}
{/snippet}

{#snippet versionDetail()}
  <!-- What changed belongs to the version under the cursor, not to the top of
       the panel. The timeline above stays a sparse index -- when, and by whom
       -- and this is the one row that carries detail: how far the comparison
       reaches, how many changes are in it, and the files they touched. The
       changes themselves are still read in the document, where they are
       painted in place. -->
  <section class="timeline-detail-row" aria-label="Selected version details">
    <div class="timeline-detail">
      {#if selectedPoint}
        <div class="history-selection">
          <strong>{stamp(selectedPoint.at)}</strong>
          <span class="panel-muted">{selectedPoint.label || actor(selectedPoint)}</span>
        </div>
      {/if}
      <p class="history-span panel-meta">
        {#if selectedPoint && !comparingCurrent}Changes made in this version
        {:else if comparingCurrent}Changes since <strong>{name(baseline)}</strong>
        {:else if target}Compared with <strong>{name(target)}</strong>
        {:else}Changes since <strong>{name(baseline)}</strong>{/if}
      </p>
      {#if selectedPoint && !comparingCurrent}
        <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => oncomparecurrent?.(selectedPoint.sha)}>Show everything changed since this version</button>
      {:else if comparingCurrent && newerEdits}
        <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onrefreshcurrent?.()}>Newer edits available · Refresh</button>
      {/if}
      <div class="history-nav">
        {#if problem}
          <span class="panel-muted text-sm">{paths.length ? "The rendered comparison is unavailable. Showing source changes instead." : "This comparison is unavailable."}</span>
        {:else if viewing && !target}
          <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onback?.()}>Back to now to see changes</button>
        {:else if changes === null}
          <span class="panel-muted text-sm" role="status">Loading changes…</span>
        {:else if !groups.length}
          <span class="panel-muted text-sm" role="status">{!comparingCurrent && viewing === checkpoints[0]?.sha ? "First retained version." : "No changes."}</span>
        {:else}
          <span class="history-count text-sm" role="status">{count}</span>
          <span class="history-steps">
            <IconButton icon="chevron-up" tone="plain" size="btn-icon-sm" label="Previous change ([)" onclick={() => goTo(step < 0 ? groups.length - 1 : step - 1)} />
            <IconButton icon="chevron-down" tone="plain" size="btn-icon-sm" label="Next change (])" onclick={() => goTo(step + 1)} />
          </span>
        {/if}
        {#if groups.length}
          <button
            type="button"
            class="btn btn-sm preset-outlined-surface-300-700"
            aria-pressed={redlines}
            onclick={() => onredlines?.(!redlines)}
          >{redlines ? "Hide highlights" : "Show highlights"}</button>
        {/if}
      </div>
      {#if groups.length}
        <details class="history-changes">
          <summary class="panel-meta">List changes</summary>
          <ol class="history-hunks">
            {#each groups as group, index (`${group.path || ""}-${group.position}-${index}`)}
              <li>
                <button
                  type="button"
                  class="history-hunk"
                  class:history-hunk-here={index === step}
                  onclick={() => goTo(index)}
                  title="Find this change in the document"
                >
                  <span class="history-context">{clip(group.before, true)}</span>{#each group.parts as part, at (at)}{#if part.keep !== undefined}<span class="history-context">{part.keep}</span>{:else}{#if part.old}<del>{part.old}</del>{/if}{#if part.old && part.insert}{" "}{/if}{#if part.insert}<ins>{part.insert}</ins>{/if}{/if}{/each}<span class="history-context">{clip(group.after, false)}</span>
                </button>
              </li>
            {/each}
          </ol>
        </details>
      {/if}
      {#if paths.length > 0}
        <!-- Source comparison remains available even for a single file,
             including when rendered passage mapping is unavailable. -->
        <details class="history-files">
          <summary class="panel-meta">{paths.length} {paths.length === 1 ? "file" : "files"} changed</summary>
          <div class="history-paths">
            {#each paths as path (path)}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700 justify-start" onclick={() => onfilediff?.(path)}>{path}</button>
            {/each}
          </div>
        </details>
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
                  <span class="panel-muted">{hunk.before}</span>{#if hunk.old}<del>{hunk.old}</del>{/if}{#if hunk.insert}<ins>{hunk.insert}</ins>{/if}<span class="panel-muted">{hunk.after}</span>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {/if}
    </div>
  </section>
{/snippet}
