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
  import Row from "./layout/Row.svelte";

  let {
    checkpoints = [],
    viewing = null,
    canEdit = false,
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
</script>

<aside class="sidebar timeline">
  <header class="mb-3">
    <Row justify="between">
      <h3 class="h5">History</h3>
      {#if checkpoints.length}
        <small class="text-surface-600-400">{checkpoints.length} checkpoints</small>
      {/if}
    </Row>
    {#if problem}
      <p class="text-error-500 text-sm">{problem}</p>
    {:else if checkpoints.length === 0}
      <p class="text-surface-600-400 text-sm">
        Nothing yet. A checkpoint is taken when the typing stops, when the last
        editor leaves, and whenever the document is published to.
      </p>
    {:else if viewing}
      <p class="text-surface-600-400 text-sm">
        Showing an earlier version. The document pane is what it said then.
      </p>
    {/if}
  </header>

  {#each days as { day, rows } (day)}
    <h4 class="text-surface-600-400 timeline-day sticky top-0 z-1 py-2 text-sm font-semibold">{day}</h4>
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
</aside>

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
        <span class="timeline-when">{clock(point.at)}</span>
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
      {/if}
    {/if}
  </li>
{/snippet}

