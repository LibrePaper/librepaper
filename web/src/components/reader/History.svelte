<script>
  import { untrack } from "svelte";
  import { day as dayOf } from "../../lib/dates.js";
  // Presentation only: the shared source controller owns selection and mode.
  import { shortSha, timeline } from "../../lib/history.js";
  import IconButton from "../IconButton.svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import CopyLink from "../CopyLink.svelte";

  let {
    checkpoints = [],
    durability = null,
    viewing = null,
    canEdit = false,
    comparison = "current",
    onview,
    onrestore,
    oncopy,
    problem = "",
    onname,
    currentLabel = "",
    path = "",
  } = $props();

  // The runs a reader has asked to see inside, by the SHA of the row that
  // stands for them. Forgotten when the panel closes, which is the right
  // lifetime: it is a glance, not a setting.
  let opened = $state(new Set());
  let filter = $state("all");
  let lastRevealed = $state("");
  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");

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
    {#if problem}
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
        <select class="select" aria-label="Compare selected checkpoint" value={comparison} onchange={event => onview?.(viewing || "", event.currentTarget.value)}>
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


      >
        {#if naming === "current"}
          {@render nameField("Name this version", "Name the current version")}
        {:else}<button
          type="button"
          class="timeline-point"
          class:timeline-here={!viewing}
          aria-current={!viewing ? "true" : undefined}
          title="Show the live document"
          onclick={() => onview?.("", comparison)}
        >
          <span class="timeline-marker timeline-marker-current"></span>
          <span class="timeline-what"><strong>{currentLabel || "Current version"}</strong><span class="timeline-who">Live draft</span></span>
        </button>
        {/if}
        {#if canEdit && !viewing && naming !== "current"}
          <span class="timeline-inline-action">
            <IconButton icon="pencil" tone="plain" size="btn-icon-sm" label="Name the current version" onclick={() => startNaming({ sha: "current", label: currentLabel })} />
          </span>
        {/if}
      </li>
      {#each days as { day, rows } (day)}
        <li class="timeline-day-row">
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
            <li class="timeline-row timeline-fold">
              <button type="button" class="timeline-folded" onclick={() => onview?.(row.first.sha, comparison)} title="Show the newest version in this session">
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
{/snippet}
