<script>
  import { untrack } from "svelte";
  // The timeline, as a column beside the document.
  //
  // A history is read backwards -- what happened, then what happened before
  // that -- so the newest day is at the top and the newest mark within it,
  // with the live document above them all. A labelled checkpoint is what
  // somebody came here to find, so it is the one thing in the column that is
  // not grey; a working afternoon of quiet marks by one author is folded to
  // its first and last, because thirty rows of the same name say less than
  // three do.
  //
  // The changes themselves are not listed here. They are painted into the
  // document, the way a version history does it: clicking a row shows the
  // document as it was then, with what changed since the baseline struck
  // through and underlined in place, and the head of the column says how
  // many changes there are and steps through them. A bracket down the gutter
  // joins the two ends of the range. The list of changes as prose, and the
  // files they touched, are there for whoever wants them, folded away.
  //
  // Nothing here fetches. The panel is given the checkpoints and reports what
  // was clicked; the page it sits in owns the requests, as it owns every other
  // one.
  import { coalesce, shortSha, sizeDelta, timeline } from "../lib/history.js";
  import IconButton from "./IconButton.svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import CopyLink from "./CopyLink.svelte";

  let {
    checkpoints = [],
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
  } = $props();

  // The runs a reader has asked to see inside, by the SHA of the row that
  // stands for them. Forgotten when the panel closes, which is the right
  // lifetime: it is a glance, not a setting.
  let opened = $state(new Set());
  let namedOnly = $state(false);
  let lastRevealed = $state("");

  // The checkpoint being named, and what it is being named. One at a time,
  // because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // Filtering affects the rows a reader sees, while the selected checkpoint
  // remains in the list so its preview never disappears under the filter.
  const timelinePoints = $derived.by(() => {
    if (!namedOnly) return checkpoints;
    const selected = checkpoints.find((point) => point.sha === viewing);
    return checkpoints.filter((point) => point.label || point.sha === viewing);
  });
  const days = $derived(timeline(timelinePoints));
  const namedCount = $derived(checkpoints.filter((point) => point.label).length);

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

  // A colour per author, in the order they first appear, so that the dot
  // beside a row says who without a word. Five is more people than a
  // document usually has; a sixth shares the first's colour.
  const authors = $derived.by(() => {
    const seen = new Map();
    for (const point of checkpoints) {
      if (point.by && !seen.has(point.by)) seen.set(point.by, seen.size % 5);
    }
    return seen;
  });
  const swatch = (by) => `author-${authors.get(by) ?? 0}`;

  // How much a checkpoint changed the document's size, for the density bar
  // beside its row. Looked up against the manifest passed in, the same as
  // every other relation between checkpoints in this panel.
  const delta = (point) => sizeDelta(point, checkpoints);

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
      : when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }

  function stamp(at) {
    const when = new Date(at);
    return Number.isNaN(when.getTime()) ? "" : when.toLocaleString([], {
      dateStyle: "medium",
      timeStyle: "short",
    });
  }

  function dayLabel(day) {
    if (!day) return "Unknown date";
    const today = new Date();
    const todayKey = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
    const yesterday = new Date(today);
    yesterday.setDate(today.getDate() - 1);
    const yesterdayKey = `${yesterday.getFullYear()}-${String(yesterday.getMonth() + 1).padStart(2, "0")}-${String(yesterday.getDate()).padStart(2, "0")}`;
    if (day === todayKey) return "Today";
    if (day === yesterdayKey) return "Yesterday";
    const date = new Date(`${day}T12:00:00`);
    return Number.isNaN(date.getTime()) ? day : date.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" });
  }

  function sessionSummary(row) {
    const count = row.hidden.length + 2;
    const range = `${clock(row.last.at)}–${clock(row.first.at)}`;
    return `${range} · ${row.first.by || "somebody"} · ${count} versions`;
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
    render: "rendered",
  };
  const QUIET = new Set(["quiet", "left", "render", "automatic", "sync"]);
  const reason = (why) => WHY[why] || why;
  const said = (point) => (QUIET.has(point.why) ? "" : reason(point.why));

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
    title="History"
    meta={checkpoints.length
      ? `${checkpoints.length} version${checkpoints.length === 1 ? "" : "s"}`
      : undefined}
  >
    {#if problem}
      <p class="text-error-500">{problem}</p>
    {:else if checkpoints.length === 0}
      <p class="panel-muted">
        Nothing yet. A version is saved when the typing stops, when the last
        editor leaves, and whenever the document is published to.
      </p>
    {/if}
  </PanelHeader>

  {#if checkpoints.length > 0}
    <label class="history-switch label px-2 py-1">
      <input type="checkbox" class="checkbox" bind:checked={namedOnly} />
      <span class="label-text text-xs">Named versions only</span>
    </label>
    {#if namedOnly && namedCount === 0}
      <p class="panel-muted px-2 py-1 text-sm" role="status">No named versions yet. Name a version to find it here.</p>
    {:else if namedOnly && viewing && !checkpoints.find((point) => point.sha === viewing)?.label}
      <p class="panel-muted px-2 py-1 text-sm" role="status">The selected version is shown below even though it is unnamed.</p>
    {/if}
  {/if}

  {#if checkpoints.length > 0}
    <section class="history-range border-surface-200-800 border-b" aria-label="Changes">
      {#if !baseline}
        <p class="panel-muted text-sm">Choose a version to see the words that changed.</p>
      {:else}
        <p class="history-span panel-meta">
          {#if target}Changes from <strong>{name(baseline)}</strong> to <strong>{name(target)}</strong>
          {:else}Changes since <strong>{name(baseline)}</strong>{/if}
        </p>
        {#if viewing && !comparingCurrent}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => oncomparecurrent?.(viewing)}>Compare with current</button>
        {:else if comparingCurrent}
          <p class="history-span panel-meta">Comparing <strong>{name(baseline)}</strong> with <strong>Current version</strong>.</p>
          {#if newerEdits}
            <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onrefreshcurrent?.()}>Newer edits available · Refresh</button>
          {/if}
        {/if}
        <div class="history-nav">
          {#if problem}
            <span class="panel-muted text-sm">The passage comparison is unavailable.</span>
          {:else if viewing && !target}
            <button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => onback?.()}>Back to now to see changes</button>
          {:else if changes === null}
            <span class="panel-muted text-sm" role="status">Loading changes…</span>
          {:else if !groups.length}
            <span class="panel-muted text-sm">{!comparingCurrent && viewing === checkpoints[0]?.sha ? "First version." : "No text changed."}</span>
          {:else}
            <span class="history-count text-sm">{count}</span>
            <span class="history-steps">
              <IconButton icon="chevron-up" tone="plain" size="btn-icon-sm" label="Previous change ([)" onclick={() => goTo(step < 0 ? groups.length - 1 : step - 1)} />
              <IconButton icon="chevron-down" tone="plain" size="btn-icon-sm" label="Next change (])" onclick={() => goTo(step + 1)} />
            </span>
          {/if}
          <label class="history-switch label">
            <input
              type="checkbox"
              class="checkbox"
              checked={redlines}
              onchange={(event) => onredlines?.(event.currentTarget.checked)}
            />
            <span class="label-text text-xs">Show changes</span>
          </label>
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
        {#if paths.length}
          <details class="history-files">
            <summary class="panel-meta">{paths.length === 1 ? "1 file changed" : `${paths.length} files changed`}</summary>
            <div class="history-paths">
              {#each paths as path (path)}
                <button type="button" class="btn btn-sm preset-outlined-surface-300-700 justify-start" onclick={() => onfilediff?.(path)}>{path}</button>
              {/each}
            </div>
          </details>
        {/if}
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
    </section>

    <ol class="timeline-list">
      <!-- The live document is a row like the others, hollow because it is
           not a checkpoint yet. -->
      <li
        class="timeline-row timeline-now"
        class:in-range={!target && baselineAt >= 0}
        class:range-end={!target && baselineAt >= 0}
      >
        {#if naming === "current"}
          <input
            bind:this={field}
            bind:value={draft}
            class="input"
            placeholder="Name this version"
            aria-label="Name the current version"
            onkeydown={(event) => {
              if (event.key === "Enter") finishNaming();
              if (event.key === "Escape") naming = "";
            }}
            onblur={finishNaming}
          />
        {:else}<button
          type="button"
          class="timeline-point"
          class:timeline-here={!viewing}
          aria-current={!viewing ? "true" : undefined}
          title="Show the live document"
          onclick={() => onview?.("")}
        >
          <span class="timeline-dot timeline-dot-now"></span>
          <span class="timeline-when panel-meta">now</span>
          <span class="timeline-what">{#if currentLabel}<strong>{currentLabel}</strong>{/if}<span class="timeline-who">Current version</span></span>
        </button>
        {/if}
        {#if canEdit && naming !== "current"}
          <span class="timeline-actions">
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
        title="Show version from {stamp(point.at) || shortSha(point.sha)} ({point.by || "somebody"} {reason(point.why)})"
        onclick={() => onview?.(point.sha)}
      >
        <span class="timeline-dot {swatch(point.by)}"></span>
        <span class="timeline-when panel-meta">{clock(point.at)}</span>
        <span class="timeline-what">
          {#if point.label}<strong>{point.label}</strong>{/if}
          <span class="timeline-who">{point.by || "somebody"}</span>
          {#if said(point)}<span class="timeline-why panel-meta">{said(point)}</span>{/if}
        </span>
      </button>
      {@const size = delta(point)}
      {#if size}
        <span
          class="timeline-size"
          class:timeline-size-grew={size.grew}
          class:timeline-size-shrank={!size.grew}
          style="width: {size.width}px"
          title={size.title}
        ></span>
      {/if}
      <span class="timeline-actions">
        <IconButton
          icon="diff"
          tone={baseline?.sha === point.sha ? "tonal" : "plain"}
          size="btn-icon-sm"
          label="Compare since this version"
          pressed={baseline?.sha === point.sha}
          onclick={() => oncompare?.(point.sha)}
        />
        {#if canEdit}
          <IconButton
            icon="pencil"
            tone="plain"
            size="btn-icon-sm"
            label={point.label ? "Rename this point" : "Name this point"}
            onclick={() => startNaming(point)}
          />
          <IconButton icon="history" tone="plain" size="btn-icon-sm" label="Restore this version" onclick={() => onrestore?.(point.sha)} />
        {/if}
        <CopyLink href={oncopy?.(point.sha) || undefined} label="Copy the link to this version" tone="plain" />
      </span>
    {/if}
  </li>
  {#if viewing === point.sha && point.parent && point.changed?.length}
    <!-- The files this checkpoint touched, under the row being looked at and
         no other: each opens against the checkpoint before. -->
    <li class="timeline-row timeline-files" class:in-range={inRange(point.sha)}>
      {#each point.changed as path (path)}
        <button type="button" class="timeline-file" onclick={() => oncheckpointfile?.(point, path)} title="Compare this file with the previous version">{path}</button>
      {/each}
    </li>
  {/if}
{/snippet}
