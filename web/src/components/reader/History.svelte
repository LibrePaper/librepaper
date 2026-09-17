<script>
  // The document's past: a month to pick a day, and that day's versions.
  //
  // This panel had a great deal more in it, and all of it went. A day was an
  // axis of clock time, then a feed of sittings with sparklines, then an
  // event timeline with a spine, activity bars and the silences written out.
  // Each of those was defensible on its own, and together they made a sidebar
  // holding four versions look like a dashboard. Almost none of it was what
  // anybody had opened the panel to find.
  //
  // What is left is what the panel is for: a list of times, a word beside the
  // few that have one, and the details of the one being looked at. A version
  // the document took by itself says nothing but its time -- "Autosaved" on
  // three rows out of four is the absence of information set in type.
  //
  // The structure -- entries that open in place rather than steps that
  // replace the view -- is the one every version history settles on, and
  // there was no reason to invent another. What is ours is the coarsening
  // rule: a run of autosaves is gathered by *significance* rather than by the
  // clock, and it says what the whole run changed, because "3:14 to 4:02" is
  // not the question anybody arrived with.
  //
  // The minutes somebody wrote in that nothing was saved from are still
  // reachable, because being able to reach them is why the whole operation
  // history is kept. They are one folded line at the end, which is the weight
  // they deserve beside the versions.
  //
  // Presentation only: the shared source controller owns selection, and this
  // reports outwards.
  import { day as dayOf } from "../../lib/dates.js";
  import { shortSha } from "../../lib/history.js";
  import {
    dayEntries, deliberate, monthGrid, monthOf, monthSpan, shiftDay,
    shiftMonth, unsavedMinutes, versionDays,
  } from "../../lib/history-calendar.js";
  import IconButton from "../IconButton.svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import CopyLink from "../CopyLink.svelte";

  let {
    checkpoints = [],
    durability = null,
    viewing = null,
    canEdit = false,
    onview,
    oncopy,
    problem = "",
    onname,
    currentLabel = "",
    // When the document was written, minute by minute, and the anchors that
    // open those moments.
    activity = [],
    activityProblem = "",
    activityLoading = false,
    viewingMoment = "",
    onmoment,
  } = $props();

  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  const today = $derived(dayOf(new Date(), timezone));

  // The checkpoint being named, and what it is being named. One at a time,
  // because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // Which of the two steps is on screen. The panel opens on the day, not on
  // the month: somebody opening a version history almost always wants the
  // last thing they did, and the month is one click above it.
  let picking = $state(false);
  let pickedDay = $state("");
  let pickedMonth = $state("");

  const byDay = $derived(versionDays(checkpoints, activity, timezone));
  const span = $derived(monthSpan(byDay, today));
  const latest = $derived([...byDay.keys()].sort().pop() || today);
  // The day the selection is on, so that opening a version from a link, or
  // from the named list, brings its own day with it.
  const viewingDay = $derived.by(() => {
    if (viewing) {
      const point = checkpoints.find((one) => one.sha === viewing);
      if (point) return dayOf(point.at, timezone);
    }
    if (viewingMoment) {
      const row = activity.find((one) => one.frontier === viewingMoment);
      if (row) return dayOf(row.at, timezone);
    }
    return "";
  });
  const day = $derived(pickedDay || viewingDay || latest);
  const month = $derived(pickedMonth || monthOf(day));
  const grid = $derived(monthGrid(month, byDay, { today }));
  const named = $derived(checkpoints.filter((point) => point.label));
  const anything = $derived(checkpoints.length > 0 || activity.length > 0);

  const entries = $derived(dayEntries(checkpoints, day, timezone));
  // Which runs are open. A run holding the version being compared is open
  // whether or not anybody opened it: the alternative is a selection nobody
  // can see.
  let unrolled = $state(new Set());
  const showing = (entry) =>
    unrolled.has(entry.key) || entry.points.some((point) => point.sha === viewing);
  function unroll(entry) {
    const next = new Set(unrolled);
    if (next.has(entry.key)) next.delete(entry.key);
    else next.add(entry.key);
    unrolled = next;
  }
  const unsaved = $derived(unsavedMinutes(checkpoints, activity, day, timezone));
  // Whether the minutes nobody saved are on the page. Not a `<details>`: a
  // closed one keeps its contents laid out, which on a busy day is hundreds
  // of times built and measured so that nobody can see them.
  let unfolded = $state(false);
  const showUnsaved = $derived(unfolded || Boolean(viewingMoment));

  const WEEKDAYS = ["S", "M", "T", "W", "T", "F", "S"];
  const versions = (n) => `${n} version${n === 1 ? "" : "s"}`;

  function open(value) {
    pickedDay = value;
    pickedMonth = monthOf(value);
    picking = false;
  }

  function step(months) {
    const next = shiftMonth(month, months);
    pickedMonth = next < span.first ? span.first : next > span.last ? span.last : next;
  }

  const monthLabel = $derived.by(() => {
    const when = new Date(`${month}-01T12:00:00Z`);
    return Number.isNaN(when.getTime())
      ? month
      : when.toLocaleDateString([], { year: "numeric", month: "long", timeZone: "UTC" });
  });

  function dayLabel(value, long = true) {
    if (!value) return "";
    if (value === today) return "Today";
    if (value === shiftDay(today, -1)) return "Yesterday";
    const when = new Date(`${value}T12:00:00Z`);
    if (Number.isNaN(when.getTime())) return value;
    return when.toLocaleDateString([], {
      weekday: long ? "long" : undefined,
      month: long ? "long" : "short",
      day: "numeric",
      timeZone: "UTC",
    });
  }

  // What a cell says when a reader stops on it.
  function says(cell) {
    const when = dayLabel(cell.day);
    if (cell.count) return `${when}: ${versions(cell.count)}${cell.named ? ", one of them named" : ""}`;
    if (cell.worked) return `${when}: written on, nothing saved`;
    return `${when}: nothing`;
  }

  /// Arrows move a day at a time sideways and a week at a time up and down,
  /// which is how a grid reads; page keys move a month. Walking off the end
  /// of the month carries the calendar into the next one, because the reader
  /// asked for the day, not for the page it happens to be drawn on.
  function acrossCalendar(event) {
    const days = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 };
    const months = { PageUp: -1, PageDown: 1 };
    if (event.key in days) {
      const next = shiftDay(day, days[event.key]);
      if (monthOf(next) < span.first || monthOf(next) > span.last) return;
      event.preventDefault();
      pickedDay = next;
      pickedMonth = monthOf(next);
      queueMicrotask(() => {
        const cell = document.querySelector(`[data-history-day="${next}"]`);
        if (cell instanceof HTMLElement) cell.focus({ preventScroll: true });
      });
      return;
    }
    if (event.key in months) {
      event.preventDefault();
      step(months[event.key]);
    }
  }

  /* ------------------------------------------------------------- the clock */

  // Minutes from midnight, turned back into something a person reads.
  const ran = (entry) => {
    if (entry.to <= entry.minute) return at(entry.minute);
    const from = at(entry.minute);
    const to = at(entry.to);
    const half = to.replace(/^\S+\s*/, "");
    return `${half && from.endsWith(half) ? from.slice(0, -half.length).trim() : from} – ${to}`;
  };
  const saves = (n) => `${n} autosave${n === 1 ? "" : "s"}`;
  const at = (minute) =>
    new Date(Date.UTC(2000, 0, 1, Math.floor(minute / 60), minute % 60)).toLocaleTimeString([], {
      hour: "numeric", minute: "2-digit", timeZone: "UTC",
    });

  const clock = (value) => {
    const when = new Date(value);
    return Number.isNaN(when.getTime())
      ? ""
      : when.toLocaleTimeString([], { hour: "numeric", minute: "2-digit", timeZone: timezone });
  };

  function stamp(value) {
    const when = new Date(value);
    return Number.isNaN(when.getTime()) ? "" : when.toLocaleString([], {
      dateStyle: "medium", timeStyle: "short", timeZone: timezone,
    });
  }

  /* ------------------------------------------------------------ a version */

  // What a checkpoint was taken for, in words rather than in the manifest's
  // own vocabulary -- and only when there is something to say. `quiet` and
  // `left` are the two nobody asked for and they are the common case: three
  // rows out of four reading "Autosaved" is the absence of information set in
  // type, and a row with no word on it already says the document saved it by
  // itself. The tooltip and the screen reader do have to name it, so `told`
  // is there for them.
  const WHY = {
    comment: "Commented",
    cli: "Published",
    publish: "Published",
    restore: "Restored",
    superseded: "Replaced by a restore",
    recovered: "Recovered",
    accept: "Accepted a suggestion",
  };
  const said = (point) => point.label || WHY[point.why] || "";
  const told = (point) => said(point) || "Autosaved";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  // Who, but only where the manifest can answer. A version somebody asked for
  // names the person who asked. An autosave names whoever sent the last
  // update before it fired, which on a document two people are writing at
  // once is a coin toss between them -- so it names nobody.
  const actor = (point) => {
    if (!deliberate(point) || !point?.by || point.by === "system") return "";
    return uuid.test(point.by) ? "Unknown editor" : point.by;
  };
  // What a version moved. The manifest answers this in three ways and they
  // are three different things: a list of paths, an empty list, and no answer
  // at all. No answer says nothing here, because a version that cannot
  // account for itself should not claim to. An empty list is an answer: no
  // file differs from the version before, so what this one holds differently
  // is outside the files.
  const files = (paths) => {
    if (!Array.isArray(paths) || !paths.length) return "";
    if (paths.length <= 2) return paths.map((path) => path.split("/").pop()).join(", ");
    return `${paths.length} files`;
  };
  const moved = (point) => {
    const paths = point?.changed;
    if (!Array.isArray(paths)) return "";
    if (paths.length === 0) return "No files changed";
    if (paths.length <= 2) return paths.map((path) => path.split("/").pop()).join(", ");
    return `${paths.length} files changed`;
  };

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
  <PanelHeader title="Version history">
    {#if problem}
      <p class="text-error-500" role="status">{problem}</p>
    {:else if !anything && !activityLoading}
      <p class="panel-muted">
        Nothing yet. A version is saved when the typing stops, when the last
        editor leaves, and whenever the document is published to.
      </p>
    {/if}
  </PanelHeader>

  {#if durability?.live_save === "pending"}
    <p class="panel-muted px-2 pb-1 text-xs" role="status">Live edits are still being saved.</p>
  {/if}

  {#if activityProblem && !checkpoints.length}
    <p class="text-error-500 px-2 py-1 text-sm" role="status">{activityProblem}</p>
  {:else if activityLoading && !anything}
    <p class="panel-muted px-2 py-1">Reading this document's past…</p>
  {:else if anything && picking}
    {@render monthStep()}
  {:else if anything}
    {@render dayStep()}
  {/if}
</div>

<!-- Step one: which day. The month has one job and no other furniture. -->
{#snippet monthStep()}
  <div class="cal">
    <div class="cal-head">
      <IconButton icon="chevron-left" tone="plain" size="btn-icon-sm" label="Previous month"
                  disabled={month <= span.first} onclick={() => step(-1)} />
      <button type="button" class="cal-month" title="Go to the last day anything happened"
              onclick={() => open(latest)}>{monthLabel}</button>
      <IconButton icon="chevron-right" tone="plain" size="btn-icon-sm" label="Next month"
                  disabled={month >= span.last} onclick={() => step(1)} />
    </div>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="cal-grid" role="group" aria-label="Days with versions" onkeydown={acrossCalendar}>
      <ol class="cal-week cal-weekdays" aria-hidden="true">
        {#each WEEKDAYS as name, index (index)}<li>{name}</li>{/each}
      </ol>
      {#each grid.weeks as week (week[0].day)}
        <ol class="cal-week">
          {#each week as cell (cell.day)}
            <li>
              <button
                type="button"
                class="cal-cell"
                data-history-day={cell.day}
                data-level={cell.level}
                class:cal-outside={!cell.inMonth}
                class:cal-quiet={!cell.count && !cell.worked}
                class:cal-today={cell.today}
                class:cal-shown={cell.day === day}
                aria-pressed={cell.day === day}
                tabindex={cell.day === day ? 0 : -1}
                title={says(cell)}
                aria-label={says(cell)}
                onclick={() => open(cell.day)}
              >
                <span class="cal-date">{cell.date}</span>
                {#if cell.count}<span class="cal-count">{cell.count}</span>{/if}
                <!-- A day somebody wrote on and saved nothing from. Without
                     this there is no way to find one: it has no versions to
                     count, and the day below is a list of versions. -->
                {#if !cell.count && cell.worked}<span class="cal-mark"></span>{/if}
              </button>
            </li>
          {/each}
        </ol>
      {/each}
    </div>
  </div>
  {#if named.length}
    <!-- The one way into the past that is not a date: somebody wrote these
         names down precisely so they would not have to remember when. -->
    <div class="names">
      <h4 class="panel-section-title">Named versions</h4>
      <ol>
        {#each named as point (point.sha)}
          <li>
            <button type="button" class="names-row" class:timeline-here={viewing === point.sha}
                    onclick={() => { open(dayOf(point.at, timezone)); onview?.(point.sha); }}>
              <strong>{point.label}</strong>
              <span class="panel-meta">{stamp(point.at)}</span>
            </button>
          </li>
        {/each}
      </ol>
    </div>
  {/if}
{/snippet}

<!-- Step two: which version. A list of times, and a word beside the few that
     have one. -->
{#snippet dayStep()}
  <div class="crumb">
    <button type="button" class="crumb-back" onclick={() => (picking = true)}
            aria-label="Back to {monthLabel}">
      <span aria-hidden="true">‹</span> {monthLabel}
    </button>
    <span class="crumb-day">{dayLabel(day, false)}</span>
  </div>

  <div class="day">
    <ol>
      <!-- The live document. It is not a checkpoint, but it is what every
           checkpoint is compared against and the only way back from one, so
           it belongs at the head of the list rather than in a banner above
           the month: the list runs newest first, and nothing is newer. -->
      <li class="day-row day-now">
        {#if naming === "current"}
          {@render nameField("Name this version", "Name the current version")}
        {:else}
          <button
            type="button"
            class="timeline-point day-point"
            class:timeline-here={!viewing}
            aria-current={!viewing ? "true" : undefined}
            title="Show the current source"
            onclick={() => onview?.("")}
          >
            <span class="day-when">Now</span>
            <span class="day-what">{currentLabel || "Current version"}</span>
          </button>
          {#if canEdit && !viewing}
            <IconButton icon="pencil" tone="plain" size="btn-icon-sm" label="Name the current version"
                        onclick={() => startNaming({ sha: "current", label: currentLabel })} />
          {/if}
        {/if}
      </li>
      {#each entries as entry (entry.key)}
        {#if entry.kind === "run"}
          <!-- The autosaves between two things somebody asked for. One entry
               that says what the whole run changed, and opens into the times
               in it -- in place, because replacing the view to show four more
               rows is a step nobody asked to take. -->
          <li class="day-run" data-run={entry.key}>
            <button type="button" class="run-head" aria-expanded={showing(entry)}
                    onclick={() => unroll(entry)}>
              <span class="run-caret" aria-hidden="true">{showing(entry) ? "▾" : "▸"}</span>
              <span class="run-when">{ran(entry)}</span>
              <span class="run-what panel-meta">
                {[saves(entry.points.length), files(entry.changed)].filter(Boolean).join(" · ")}
              </span>
            </button>
            {#if showing(entry)}
              <ol class="run-rows">
                {#each entry.points as point (point.sha)}
                  {@render version(point)}
                {/each}
              </ol>
            {/if}
          </li>
        {:else}
          {@render version(entry.points[0])}
        {/if}
      {/each}
      {#if !entries.length && !unsaved.length}
        <li><p class="panel-muted">Nothing was written on this day.</p></li>
      {/if}
    </ol>

    {#if unsaved.length}
      <!-- Everything else that happened. Folded, because there are always far
           more of these than there are versions and none of them is what the
           panel was opened for -- but kept, because the document can be
           opened at any of them and nothing else offers that. -->
      <div class="unsaved">
        <button type="button" class="unsaved-more panel-meta" aria-expanded={showUnsaved}
                onclick={() => (unfolded = !showUnsaved)}>
          <span class="unsaved-caret" aria-hidden="true">{showUnsaved ? "▾" : "▸"}</span>
          {unsaved.length} moment{unsaved.length === 1 ? "" : "s"} not saved as a version
        </button>
        {#if showUnsaved}
          <ol>
            {#each unsaved as one (one.minute)}
              <li>
                <button type="button" class="unsaved-row"
                        class:unsaved-here={one.frontier === viewingMoment}
                        aria-current={one.frontier === viewingMoment ? "true" : undefined}
                        title="Show the document as it stood at {at(one.minute)}"
                        onclick={() => onmoment?.(one.frontier, one)}>{at(one.minute)}</button>
              </li>
            {/each}
          </ol>
        {/if}
      </div>
    {/if}
  </div>
{/snippet}

<!-- One version: the time it was taken, and a word only where there is one.
     The same row whether it stands on its own or inside a run. -->
{#snippet version(point)}
  <li class="day-row" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField("sent to the journal", "Name this point")}
    {:else}
      <button
        type="button"
        class="timeline-point day-point"
        class:timeline-here={viewing === point.sha}
        aria-current={viewing === point.sha ? "true" : undefined}
        title="Compare {told(point)} from {stamp(point.at) || shortSha(point.sha)} with the current source"
        onclick={() => onview?.(point.sha)}
      >
        <span class="day-when">{clock(point.at)}</span>
        {#if said(point)}
          <span class="day-what">{said(point)}</span>
        {:else}
          <span class="sr-only">Autosaved</span>
        {/if}
      </button>
      {#if viewing === point.sha}
        {@const about = [actor(point), moved(point)].filter(Boolean).join(" · ")}
        <!-- What the chosen version holds, and what can be done with it. Only
             ever the chosen one: the list is for finding a moment, and what
             the moment holds is a question asked after it has been found. -->
        <div class="day-card">
          {#if about}<p class="panel-meta">{about}</p>{/if}
          <div class="day-card-actions">
            {#if canEdit}
              <IconButton icon="pencil" tone="plain" size="btn-icon-sm"
                          label={point.label ? "Rename this version" : "Name this version"}
                          onclick={() => startNaming(point)} />
            {/if}
            <CopyLink href={oncopy?.(point.sha) || undefined}
                      label="Copy the link to this version" tone="plain" />
          </div>
        </div>
      {/if}
    {/if}
  </li>
{/snippet}

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

<style>
  /* Two levels of type, one accent, and no boxes. Every mark, bar, rule and
     spine that used to be here is gone: a day holding four versions is four
     lines of text, and the only colour in it says which line is being looked
     at. */

  /* --------------------------------------------------------------- the month */

  .cal { flex: none; padding: 0 calc(var(--spacing) * 2); }
  /* The two arrows sit against the month rather than at the ends of the
     panel: they belong to the word they step, and a control at each margin
     reads as a pair of unrelated buttons. */
  .cal-head {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: calc(var(--spacing) * 0.5);
  }
  .cal-month {
    min-width: 9.5rem;
    padding: var(--spacing);
    border-radius: var(--radius-container);
    font-size: var(--panel-font-size);
    font-weight: 500;
    text-align: center;
    cursor: pointer;
  }
  .cal-month:hover { background: var(--color-row-hover); }
  .cal-grid { display: flex; flex-direction: column; margin-top: var(--spacing); }
  .cal-week { display: grid; grid-template-columns: repeat(7, 1fr); }
  .cal-weekdays {
    padding-bottom: var(--spacing);
    font-size: var(--text-xs);
    color: var(--color-surface-500);
    text-align: center;
  }
  /* A cell is a date on the panel's own ground, not a button on a grid of
     buttons. Its height is given over to whitespace around the number. */
  .cal-cell {
    position: relative;
    display: grid;
    place-items: center;
    width: 100%;
    aspect-ratio: 1;
    border-radius: var(--radius-container);
    font-size: var(--text-xs);
    line-height: 1;
    cursor: pointer;
  }
  .cal-cell:hover { background: var(--color-row-hover); }
  .cal-date {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    font-variant-numeric: tabular-nums;
  }
  /* A day nobody opened recedes; a day outside the month recedes further. It
     is still drawn, so the weeks stay whole and the month can be counted
     across. */
  .cal-quiet .cal-date { color: var(--color-surface-500); }
  .cal-outside { opacity: 0.45; }
  .cal-today .cal-date { color: var(--color-primary-600-400); font-weight: 600; }
  /* The day being read: a filled circle on the date itself, not a filled
     cell. The cell is forty pixels of ground, and colouring all of it says
     nothing more than colouring the number does. */
  .cal-shown .cal-date {
    background: var(--color-primary-500);
    color: var(--color-surface-50);
    font-weight: 500;
  }
  .cal-cell:focus-visible { outline: 2px solid var(--color-primary-500); outline-offset: -2px; }
  /* How many versions landed on the day -- the only thing the month has to
     say beyond the date itself. */
  .cal-count {
    position: absolute;
    top: 2px;
    right: 3px;
    font-size: 0.5625rem;
    line-height: 1;
    font-weight: 600;
    color: var(--color-primary-600-400);
  }
  /* A day written on that nothing was saved from: no number to show, so a
     dot, which is the least that can be said and still be seen. */
  .cal-mark {
    position: absolute;
    bottom: 5px;
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--color-primary-500);
    opacity: 0.5;
  }

  .names {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    padding: calc(var(--spacing) * 3) calc(var(--spacing) * 2);
  }
  .names-row {
    display: flex;
    flex-direction: column;
    gap: 1px;
    width: 100%;
    padding: var(--spacing);
    border-radius: var(--radius-container);
    line-height: 1.25;
    text-align: left;
    cursor: pointer;
  }
  .names-row:hover { background: var(--color-row-hover); }

  /* ----------------------------------------------------------------- the day */

  /* The month, once it has answered: a way back, and the date it chose. */
  .crumb {
    flex: none;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: calc(var(--spacing) * 2);
    padding: 0 calc(var(--spacing) * 2) calc(var(--spacing) * 2);
  }
  .crumb-back {
    padding-right: calc(var(--spacing) * 1.5);
    font-size: var(--panel-meta-size);
    color: var(--panel-muted);
    white-space: nowrap;
    cursor: pointer;
  }
  .crumb-back:hover { color: var(--color-surface-950-50); }
  .crumb-day { font-size: var(--panel-font-size); font-weight: 600; }

  .day {
    flex: 0 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding: calc(var(--spacing) * 2) calc(var(--spacing) * 2) calc(var(--spacing) * 4);
    border-top: 1px solid var(--color-surface-200-800);
  }
  /* A version: the time it was taken, and a word only where there is one.
     Nothing else -- the row is already a row, and a marker beside it would be
     a second way of saying so. */
  /* The live document's row: the same row as the others, with its one
     control beside it rather than under it. */
  .day-now {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 0.5);
  }
  .day-now .day-point { flex: 1; min-width: 0; }
  .day-point {
    display: flex;
    align-items: baseline;
    gap: calc(var(--spacing) * 2);
    width: 100%;
    min-width: 0;
    padding: calc(var(--spacing) * 0.75) var(--spacing);
    border-radius: var(--radius-container);
    line-height: 1.3;
    text-align: left;
    cursor: pointer;
  }
  .day-point:hover { background: var(--color-row-hover); }
  .day-when {
    flex: none;
    font-size: var(--panel-font-size);
    font-variant-numeric: tabular-nums;
    color: var(--color-surface-600);
  }
  .day-what {
    min-width: 0;
    font-size: var(--panel-font-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .timeline-here .day-when,
  .timeline-here .day-what { font-weight: 600; }
  .timeline-here .day-when { color: var(--color-primary-700-300); }
  .day-card {
    padding: calc(var(--spacing) * 0.5) var(--spacing) var(--spacing)
             calc(var(--spacing) * 2);
    line-height: 1.3;
  }
  .day-card-actions {
    display: flex;
    gap: calc(var(--spacing) * 0.5);
    margin-top: calc(var(--spacing) * 0.5);
  }

  /* A run of autosaves, gathered. It reads like a version row with a caret
     in front of it, because that is what it is: one entry standing for
     several, opening where it stands. */
  .run-head {
    display: grid;
    grid-template-columns: 0.75rem minmax(0, 1fr);
    align-items: baseline;
    gap: 0 calc(var(--spacing) * 1.5);
    width: 100%;
    padding: calc(var(--spacing) * 0.75) var(--spacing);
    border-radius: var(--radius-container);
    line-height: 1.3;
    text-align: left;
    cursor: pointer;
  }
  .run-head:hover { background: var(--color-row-hover); }
  .run-caret { font-size: 0.625rem; color: var(--panel-muted); }
  .run-when {
    font-size: var(--panel-font-size);
    font-variant-numeric: tabular-nums;
    color: var(--color-surface-600);
    white-space: nowrap;
  }
  /* What the run changed -- the thing "3:14 to 4:02" cannot say, and nearly
     always the thing somebody came to the panel for. */
  .run-what {
    grid-column: 2;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The versions inside it, indented under the caret so that the run they
     belong to is obvious without a rule or a box to say so. */
  .run-rows { padding-left: calc(var(--spacing) * 3); }

  /* Everything that was not saved. One line, closed, at the end. */
  .unsaved { margin-top: calc(var(--spacing) * 3); }
  .unsaved-more {
    display: flex;
    align-items: baseline;
    gap: var(--spacing);
    width: 100%;
    padding: var(--spacing);
    border-radius: var(--radius-container);
    text-align: left;
    cursor: pointer;
  }
  .unsaved-more:hover { color: var(--color-surface-950-50); }
  .unsaved-caret { font-size: 0.625rem; }
  .unsaved ol {
    display: flex;
    flex-wrap: wrap;
    gap: calc(var(--spacing) * 0.5);
    padding: var(--spacing);
  }
  .unsaved-row {
    padding: 2px calc(var(--spacing) * 1.5);
    border-radius: var(--radius-container);
    background: var(--color-row-hover);
    font-size: var(--panel-meta-size);
    font-variant-numeric: tabular-nums;
    color: var(--panel-muted);
    cursor: pointer;
  }
  .unsaved-row:hover { color: var(--color-surface-950-50); }
  .unsaved-here {
    background: var(--color-primary-500);
    color: var(--color-surface-50);
  }
</style>
