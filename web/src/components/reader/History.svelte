<script>
  // The document's past, in three views: the day's versions, a month to pick
  // another day, and the marks somebody left on the versions worth finding
  // again.
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
  // a machine asked for says nothing but its time -- "Synced" on three rows
  // out of four is the absence of information set in type.
  //
  // The structure -- entries that open in place rather than steps that
  // replace the view -- is the one every version history settles on, and
  // there was no reason to invent another. What is ours is the coarsening
  // rule: a run of autosaves is gathered by *significance* rather than by the
  // clock, and it says what the whole run changed, because "3:14 to 4:02" is
  // not the question anybody arrived with.
  //
  // The minutes somebody wrote in that nothing was saved from are not on the
  // list. They were, as one folded line at the end, and every one of them read
  // "10:42 · 37": a clock time cannot say which minute is the one wanted, so
  // reaching them meant clicking minutes until the document looked right. The
  // operation history is still kept and still read -- the calendar marks a day
  // written on with nothing saved, and a comment anchors to a frontier -- but
  // it is not enumerated here.
  //
  // Presentation only: the shared source controller owns selection, and this
  // reports outwards.
  import { day as dayOf } from "../../lib/dates.js";
  import { shortSha } from "../../lib/history.js";
  import {
    dayEntries, deliberate, monthGrid, monthOf, monthSpan, shiftDay,
    shiftMonth, versionDays,
  } from "../../lib/history-calendar.js";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";

  let {
    checkpoints = [],
    durability = null,
    viewing = null,
    canEdit = false,
    onview,
    problem = "",
    onname,
    currentLabel = "",
    // When the document was written, minute by minute, and the anchors that
    // open those moments.
    activity = [],
    activityProblem = "",
    activityLoading = false,
  } = $props();

  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  const today = $derived(dayOf(new Date(), timezone));

  // The checkpoint being bookmarked, and what it is being called. One at a
  // time, because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // Three views of the same past, and the panel opens on the one somebody
  // opening a version history almost always wants: the day they were last
  // working on. The month is a tab away rather than a step above, and the
  // marks somebody left are a tab away rather than a list under the month.
  const TABS = [
    { id: "timeline", label: "Timeline" },
    { id: "calendar", label: "Calendar" },
    { id: "bookmarks", label: "Bookmarks" },
  ];
  let tab = $state("timeline");
  let pickedDay = $state("");
  let pickedMonth = $state("");

  const byDay = $derived(versionDays(checkpoints, activity, timezone));
  const span = $derived(monthSpan(byDay, today));
  const latest = $derived([...byDay.keys()].sort().pop() || today);
  // The day the selection is on, so that opening a version from the
  // bookmarks brings its own day with it.
  const viewingDay = $derived.by(() => {
    if (viewing) {
      const point = checkpoints.find((one) => one.sha === viewing);
      if (point) return dayOf(point.at, timezone);
    }
    return "";
  });
  const day = $derived(pickedDay || viewingDay || latest);
  const month = $derived(pickedMonth || monthOf(day));
  const grid = $derived(monthGrid(month, byDay, { today }));
  // A bookmark is a version somebody put a name on. Nothing else writes a
  // label, so the two are the same thing said twice.
  const bookmarks = $derived(
    checkpoints.filter((point) => point.label).sort((a, b) => (a.at < b.at ? 1 : -1)),
  );
  const bookmarked = (point) => Boolean(point?.label);
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
  const WEEKDAYS = ["S", "M", "T", "W", "T", "F", "S"];
  const versions = (n) => `${n} version${n === 1 ? "" : "s"}`;

  // Choosing a day anywhere -- on the month, or by opening a bookmark -- is
  // asking to see that day's versions, so it answers on the timeline.
  function open(value) {
    pickedDay = value;
    pickedMonth = monthOf(value);
    tab = "timeline";
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

  // Only ever spoken, never shown: what a calendar cell tells a reader who
  // stops on it, so it says the day in full.
  function dayLabel(value) {
    if (!value) return "";
    if (value === today) return "Today";
    if (value === shiftDay(today, -1)) return "Yesterday";
    const when = new Date(`${value}T12:00:00Z`);
    if (Number.isNaN(when.getTime())) return value;
    return when.toLocaleDateString([], {
      weekday: "long",
      month: "long",
      day: "numeric",
      timeZone: "UTC",
    });
  }

  // What a cell says when a reader stops on it.
  function says(cell) {
    const when = dayLabel(cell.day);
    if (cell.count) return `${when}: ${versions(cell.count)}${cell.named ? ", one of them bookmarked" : ""}`;
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

  // The same time to the second, for the one row that has room to say it.
  const precise = (value) => {
    const when = new Date(value);
    return Number.isNaN(when.getTime())
      ? ""
      : when.toLocaleTimeString([], { hour: "numeric", minute: "2-digit", second: "2-digit", timeZone: timezone });
  };

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
  // own vocabulary -- and only when there is something to say. `sync` is the
  // one nobody asked for by hand, and on a document edited from the command
  // line it is most of the rows: a word on every one of them is the absence
  // of information set in type, and a row with no word on it already says a
  // machine wrote it. The tooltip and the screen reader do have to name it,
  // so `told` is there for them.
  // Nothing here says "Published". Publishing was something a person did,
  // and it is not any more: a document arrives, and what a reader is shown
  // follows the source on its own.
  const WHY = {
    comment: "Commented",
    created: "Created",
    cli: "Saved",
    publish: "Shown to readers",
    restore: "Restored",
    superseded: "Replaced by a restore",
    recovered: "Recovered",
    accept: "Accepted a suggestion",
  };
  const said = (point) => point.label || WHY[point.why] || "";
  const told = (point) => said(point) || "Synced";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  // Who, but only where the manifest can answer. A version somebody asked for
  // names the person who asked. A machine save names whoever sent the last
  // update before it was written, which on a document two people are writing
  // at once is a coin toss between them -- so it names nobody.
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

  // What a new bookmark is called before anybody calls it anything.
  //
  // A bookmark is looked for later, in a list, by somebody who remembers
  // roughly when rather than exactly what -- so the name it arrives with says
  // which version it is. It is a default and not a suggestion: it comes up
  // selected, so the first key typed replaces the whole of it.
  const defaultName = (point) => {
    const when = new Date(point?.at || Date.now());
    const day = Number.isNaN(when.getTime()) ? new Date() : when;
    return `Draft of ${day.toLocaleDateString([], {
      month: "short", day: "numeric", year: "numeric", timeZone: timezone,
    })}`;
  };

  async function startNaming(point) {
    naming = point.sha;
    draft = point.label || defaultName(point);
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

  // Taking the bookmark off a version. The version itself is untouched --
  // this removes the name, which is all a bookmark ever was.
  function unbookmark(point) {
    if (naming === point.sha) naming = "";
    onname?.(point.sha, "");
  }
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. The three inside it are this panel's own,
     and they are the strip every tabbed panel wears. -->
<div class="panel panel-tabbed timeline">
  <PanelHeader title="Version history" />
  <PanelTabs id="history" label="Version history views" listClass="history-tabs"
             tabs={TABS} value={tab} onchange={(value) => (tab = value)}>
    <!-- Only the view being read is built. A hidden pane holding a busy day's
         ninety rows, or a whole month of cells, is work nobody asked for and
         a second set of rows for anything counting them to find. -->
    <Tabs.Content value="timeline">
      {#if tab === "timeline"}
        {@render notices()}
        {#if anything}{@render dayStep()}{/if}
      {/if}
    </Tabs.Content>
    <Tabs.Content value="calendar">
      {#if tab === "calendar"}
        {@render notices()}
        {#if anything}{@render monthStep()}{/if}
      {/if}
    </Tabs.Content>
    <Tabs.Content value="bookmarks">
      {#if tab === "bookmarks"}
        {@render notices()}
        {@render bookmarkList()}
      {/if}
    </Tabs.Content>
  </PanelTabs>
</div>

<!-- What the panel has to say before any of its views can say anything. -->
{#snippet notices()}
  {#if problem}
    <p class="text-error-500" role="status">{problem}</p>
  {:else if !anything && !activityLoading}
    <p class="panel-muted">
      Nothing yet. A version is saved when the typing stops, when the last
      editor leaves, and whenever the document is published to.
    </p>
  {/if}
  {#if durability?.live_save === "pending"}
    <p class="panel-muted text-xs" role="status">Live edits are still being saved.</p>
  {/if}
  {#if activityProblem && !checkpoints.length}
    <p class="text-error-500 text-sm" role="status">{activityProblem}</p>
  {:else if activityLoading && !anything}
    <p class="panel-muted">Reading this document's past…</p>
  {/if}
{/snippet}

<!-- Which day. The month has one job and no other furniture. -->
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
{/snippet}

<!-- The marks somebody left. The one way into the past that is not a date:
     these names were written down precisely so that nobody would have to
     remember when. The row is the timeline's row, because a bookmark is a
     version and reading one should not be a different act here; what changes
     is what can be done with it, which is rename it and take the mark off. -->
{#snippet bookmarkList()}
  <div class="marks">
    {#if bookmarks.length}
      <ol>
        {#each bookmarks as point (point.sha)}
          <li class="mark-row" data-sha={point.sha}>
            {#if naming === point.sha}
              {@render nameField("sent to the journal", "Name this bookmark")}
            {:else}
              <button type="button" class="timeline-point mark-point"
                      class:timeline-here={viewing === point.sha}
                      aria-current={viewing === point.sha ? "true" : undefined}
                      title="Compare {point.label} from {stamp(point.at) || shortSha(point.sha)} with the current source"
                      onclick={() => { open(dayOf(point.at, timezone)); onview?.(point.sha); }}>
                <span class="mark-name">{point.label}</span>
                <span class="mark-when panel-meta">{stamp(point.at)}</span>
              </button>
              {#if canEdit}
                <span class="day-tools">
                  <IconButton icon="pencil" tone="plain" size="btn-icon-sm"
                              label="Rename this bookmark" onclick={() => startNaming(point)} />
                  <IconButton icon="trash" tone="plain" size="btn-icon-sm"
                              label="Remove this bookmark" onclick={() => unbookmark(point)} />
                </span>
              {/if}
            {/if}
          </li>
        {/each}
      </ol>
    {:else if anything}
      <p class="panel-muted">
        No bookmarks yet. Bookmark a version on the timeline to find it again
        by name rather than by the day it happened on.
      </p>
    {/if}
  </div>
{/snippet}

<!-- Which version. A list of times, and a word beside the few that have one. -->
{#snippet dayStep()}
  <div class="day">
    <ol>
      <!-- The live document. It is not a checkpoint, but it is what every
           checkpoint is compared against and the only way back from one, so
           it belongs at the head of the list rather than in a banner above
           the month: the list runs newest first, and nothing is newer. -->
      <li class="day-row day-now">
        {#if naming === "current"}
          {@render nameField("sent to the journal", "Name this bookmark")}
        {:else}
          <!-- Not an event, so not shaped like one: a label at the left and
               what it points at at the right, rather than a time and a word
               in the columns every version below it uses. -->
          <button
            type="button"
            class="timeline-point now-point"
            class:timeline-here={!viewing}
            aria-current={!viewing ? "true" : undefined}
            aria-label="Show the current source"
            onclick={() => onview?.("")}
          >
            <span class="now-label">Now</span>
            <span class="now-what">{currentLabel || "Current version"}</span>
          </button>
          {#if canEdit && !viewing}
            <span class="day-tools" class:on={Boolean(currentLabel)}>
              <IconButton icon="bookmark" tone="plain" size="btn-icon-sm" filled={Boolean(currentLabel)}
                          label={currentLabel ? "Rename this bookmark" : "Bookmark the current version"}
                          onclick={() => startNaming({ sha: "current", label: currentLabel })} />
            </span>
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
      {#if !entries.length}
        <!-- A day with no versions is two different days, and saying the
             wrong one of them is worse than saying nothing: somebody may have
             written all afternoon and saved none of it. -->
        <li><p class="panel-muted">
          {byDay.get(day)?.worked
            ? "Written on, but nothing was saved as a version."
            : "Nothing was written on this day."}
        </p></li>
      {/if}
    </ol>
  </div>
{/snippet}

<!-- One version: the time it was taken, and a word only where there is one.
     The same row whether it stands on its own or inside a run. -->
{#snippet version(point)}
  {@const here = viewing === point.sha}
  {@const who = actor(point)}
  <li class="day-row" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField("sent to the journal", "Name this bookmark")}
    {:else}
      <button
        type="button"
        class="timeline-point day-point"
        class:timeline-here={here}
        aria-current={here ? "true" : undefined}
        title="Compare {told(point)} from {stamp(point.at) || shortSha(point.sha)} with the current source"
        onclick={() => onview?.(point.sha)}
      >
        <span class="day-when">{clock(point.at)}</span>
        <!-- The word, where there is one. The cell stays either way, so that
             what follows it keeps its column; the word an autosave withholds
             from the page is still owed to a screen reader, and it is said
             out of the flow, beside the cell rather than in it. -->
        <span class="day-what">{said(point)}</span>
        {#if !said(point)}<span class="sr-only">Synced</span>{/if}
        <!-- Who, at the right edge, where it can be ignored. It is metadata
             on a list whose job is moving through time, so it is the first
             thing to go: it stands down for the controls when this is the row
             being read, and says itself in full on the line below. -->
        {#if who && !here}<span class="day-who">{who}</span>{/if}
        {#if here}
          {@const about = [who, precise(point.at), moved(point)].filter(Boolean).join(" · ")}
          {#if about}<span class="day-about">{about}</span>{/if}
        {/if}
      </button>
      <!-- The one thing that can be done to a version from the timeline: mark
           it, so that it can be found again by name. It is on every row, since
           a row has to be markable before it is the one being read; but a
           control on every line of a list is noise, so it is drawn only under
           the pointer, on the row being read, on the row a keyboard has
           reached -- and always where the mark is already set, because there
           the icon is not a control offering itself, it is the answer. -->
      {#if canEdit}
        <span class="day-tools" class:on={bookmarked(point)}>
          <IconButton icon="bookmark" tone="plain" size="btn-icon-sm" filled={bookmarked(point)}
                      label={bookmarked(point) ? "Rename this bookmark" : "Bookmark this version"}
                      onclick={() => startNaming(point)} />
        </span>
      {:else if bookmarked(point)}
        <span class="day-tools on mark-still"><Icon name="bookmark" filled /></span>
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

  .cal { flex: none; }
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

  /* ----------------------------------------------------------- the marks */

  .marks { flex: 1 1 auto; min-height: 0; overflow-y: auto; overflow-x: hidden; }
  /* The name and what can be done to it, at the two ends of the row. The
     controls do not wait to be hovered here: this view is about the marks
     themselves, so the two things that can be done to one are simply on it. */
  .mark-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
  }
  /* The name is the row's subject and the date is its metadata, which is the
     reverse of the timeline, where the time is how a version is found. */
  .mark-point {
    display: flex;
    flex-direction: column;
    gap: 1px;
    width: 100%;
    min-width: 0;
    padding: calc(var(--spacing) * 0.5) var(--spacing);
    border-radius: 4px;
    line-height: 1.25;
    text-align: left;
    cursor: pointer;
  }
  .mark-point:hover { background: var(--color-row-hover); }
  .mark-name {
    min-width: 0;
    font-size: var(--panel-font-size);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mark-when { font-variant-numeric: tabular-nums; }
  /* A reader who cannot set one still has to see that a version carries a
     mark, so the icon is drawn where the button would be. */
  .mark-still {
    width: var(--spacing);
    padding-inline: calc(var(--spacing) * 1.5);
    color: var(--color-primary-500);
    box-sizing: content-box;
  }

  /* ----------------------------------------------------------------- the day */

  .day {
    flex: 0 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding-top: calc(var(--spacing) * 2);
    border-top: 1px solid var(--color-surface-200-800);
  }
  /* A revision log, not a column of controls. A row has no border, no pill,
     no icon and no ground of its own; the only rows that are drawn at all are
     the one under the pointer and the one being read.
  
     Three columns, the same three on every row: when, what, who. The time is
     a fixed width so that the words start at one x down the whole day, and is
     set in tabular numerals so the digits line up within it. */
  .day-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: start;
  }
  .day-point {
    display: grid;
    grid-template-columns: 3.75rem minmax(0, 1fr) auto;
    align-items: center;
    column-gap: calc(var(--spacing) * 2);
    width: 100%;
    min-width: 0;
    min-height: 2.125rem;
    padding: calc(var(--spacing) * 0.5) var(--spacing);
    border-radius: 4px;
    line-height: 1.3;
    text-align: left;
    cursor: pointer;
  }
  .day-point:hover { background: var(--color-row-hover); }
  /* The time is the key to the row but not its subject: muted, so that the
     word beside it is what the eye lands on going down the list. */
  .day-when {
    font-size: var(--panel-font-size);
    font-variant-numeric: tabular-nums;
    color: var(--panel-muted);
    white-space: nowrap;
  }
  .day-what {
    min-width: 0;
    font-size: var(--panel-font-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .day-who {
    max-width: 7rem;
    font-size: var(--panel-meta-size);
    color: var(--panel-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The chosen row says what it holds on a second line, under the word rather
     than under the time, and that is the whole of the difference: medium
     weight and a tinted ground, no rule, no box, no third row. */
  .timeline-here .day-what { font-weight: 500; }
  .day-about {
    grid-column: 2 / -1;
    padding-bottom: 2px;
    font-size: var(--panel-meta-size);
    color: var(--panel-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The controls, at the right of the row they belong to. A control offering
     itself on every line of a list is noise, so on the timeline it waits for
     the pointer, for the keyboard, or for the row to be the one being read.
     A mark already set is not an offer and does not hide. */
  .day-tools {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 0.5);
    padding-top: 1px;
  }
  .day-row .day-tools:not(.on) { opacity: 0; }
  .day-row:hover .day-tools,
  .day-row:focus-within .day-tools,
  .day-row:has(.timeline-here) .day-tools { opacity: 1; }
  /* The mark itself is coloured; an empty bookmark waiting to be set is not,
     or every row would read as half-marked. */
  .day-tools.on { color: var(--color-primary-500); }

  /* The live document. A label and what it points at, at the two ends of a
     row that is plainly not one of the events below it. */
  .day-now { margin-bottom: calc(var(--spacing) * 1.5); }
  .now-point {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: calc(var(--spacing) * 2);
    width: 100%;
    min-width: 0;
    min-height: 2.125rem;
    padding: calc(var(--spacing) * 0.5) var(--spacing);
    border-radius: 4px;
    text-align: left;
    cursor: pointer;
  }
  .now-point:hover { background: var(--color-row-hover); }
  .now-label {
    font-size: var(--panel-meta-size);
    font-weight: 600;
    letter-spacing: .06em;
    text-transform: uppercase;
    color: var(--panel-muted);
  }
  .now-what {
    min-width: 0;
    font-size: var(--panel-font-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* A run of autosaves, gathered. It reads like a version row with a caret
     in front of it, because that is what it is: one entry standing for
     several, opening where it stands. */
  .run-head {
    display: grid;
    grid-template-columns: 0.75rem minmax(0, 1fr);
    align-items: center;
    gap: 0 calc(var(--spacing) * 1.5);
    width: 100%;
    min-height: 2.125rem;
    padding: calc(var(--spacing) * 0.5) var(--spacing);
    border-radius: 4px;
    line-height: 1.3;
    text-align: left;
    cursor: pointer;
  }
  .run-head:hover { background: var(--color-row-hover); }
  .run-caret { font-size: 0.625rem; color: var(--panel-muted); }
  .run-when {
    font-size: var(--panel-font-size);
    font-variant-numeric: tabular-nums;
    color: var(--panel-muted);
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
</style>
