<script>
  // The document's past, as one list: a heading for each day that has
  // versions, newest day first. The newest day is open, so the latest work is
  // the first thing shown. Every other day folds to its heading, which says
  // how many versions it holds and which files they changed, and opens on a
  // click. Bookmarks are a filter on the same list, not a view of their own.
  //
  // The day is said where the versions are read. A list of times with the
  // date kept somewhere else meant a reader could not see which day they were
  // looking at, and a calendar in a second tab was the only way to find out.
  // A day that is folded costs one line, so a long history stays cheap to
  // draw, and no version is drawn until its day is opened.
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
  // operation history is still kept and still read -- a comment anchors to a
  // frontier -- but it is not enumerated here.
  //
  // Presentation only: the shared source controller owns selection, and this
  // reports outwards.
  import { day as dayOf } from "../../lib/dates.js";
  import { shortSha } from "../../lib/history.js";
  import {
    changedAcross, dayEntries, deliberate, shiftDay, versionDays,
  } from "../../lib/history-days.js";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import PanelHeader from "../PanelHeader.svelte";

  let {
    labels = [],
    viewing = null,
    against = "",
    canEdit = false,
    onview,
    oncompare,
    problem = "",
    onname,
    currentLabel = "",
  } = $props();

  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  const today = $derived(dayOf(new Date(), timezone));

  // The label being bookmarked, and what it is being called. One at a
  // time, because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // A bookmark is a version somebody put a name on. Nothing else writes a
  // label, so the two are the same thing said twice.
  const bookmarked = (point) => Boolean(point?.label);
  const anything = $derived(labels.length > 0);
  const anyBookmarks = $derived(labels.some(bookmarked));

  // The filter shows only the bookmarked versions, grouped by day. It is off
  // when the panel opens, so the whole history is the first thing read.
  let onlyMarked = $state(false);

  // Every day that has versions, newest first. The days are ISO strings, so
  // they sort as text.
  const days = $derived(
    [...versionDays(labels, timezone).values()].sort((a, b) => (a.day < b.day ? 1 : -1)),
  );
  const latest = $derived(days[0]?.day ?? "");
  const shown = $derived(onlyMarked ? days.filter((group) => group.named) : days);

  // Which days are open. A day holding the version being compared, or the
  // base it is compared with, is open whether or not anybody opened it: the
  // alternative is a selection nobody can see. The filter opens every day
  // that holds a bookmark. Otherwise the newest day starts open and the rest
  // start folded, and a click flips a day from where it started.
  let toggled = $state(new Set());
  const isOpen = (group) =>
    onlyMarked
    || group.points.some((point) => point.sha === viewing || point.sha === against)
    || toggled.has(group.day) !== (group.day === latest);
  function toggleDay(value) {
    const next = new Set(toggled);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    toggled = next;
  }

  // Which runs are open. A run holding the version being compared, or the
  // base it is compared with, is open whether or not anybody opened it: the
  // alternative is a selection nobody can see.
  let unrolled = $state(new Set());
  const showing = (entry) =>
    unrolled.has(entry.key)
    || entry.points.some((point) => point.sha === viewing || point.sha === against);
  function unroll(entry) {
    const next = new Set(unrolled);
    if (next.has(entry.key)) next.delete(entry.key);
    else next.add(entry.key);
    unrolled = next;
  }
  const versions = (n) => `${n} version${n === 1 ? "" : "s"}`;

  // What a day's heading says it holds: how many versions, and which files
  // they changed when the manifest can say.
  const dayMeta = (group) =>
    [versions(group.count), files(changedAcross(group.points))].filter(Boolean).join(" · ");

  // The day as its heading reads it: a word for today and yesterday, and
  // otherwise the weekday and the date, with the year only when it is not
  // this year.
  function dayName(value) {
    if (value === today) return "Today";
    if (value === shiftDay(today, -1)) return "Yesterday";
    const when = new Date(`${value}T12:00:00Z`);
    if (Number.isNaN(when.getTime())) return value;
    return when.toLocaleDateString([], {
      weekday: "short",
      month: "short",
      day: "numeric",
      ...(value.slice(0, 4) !== today.slice(0, 4) ? { year: "numeric" } : {}),
      timeZone: "UTC",
    });
  }

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

  // What a label was taken for, in words rather than in the manifest's
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
    git: "Pulled from Git",
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

  // What a row's title says it is compared with: the base, when there is one
  // and this row is not it, and the current source otherwise.
  const compareTo = (point) => (against && against !== point.sha ? "comparison base" : "current source");

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

<div class="panel timeline">
  <PanelHeader title="Version history" actions={anything ? filterControl : undefined} />
  <!-- What the panel has to say before its list can say anything. -->
  {#if problem}
    <p class="lp-text-error" role="status">{problem}</p>
  {:else if !anything}
    <p class="panel-muted">
      Nothing yet. A version is saved when the typing stops, when the last
      editor leaves, and whenever the document is published to.
    </p>
  {/if}
  {#if anything && onlyMarked && !anyBookmarks}
    <p class="panel-muted">No bookmarks yet. Bookmark a version to find it again by name.</p>
  {:else if anything}
    <ol class="history-list">
      <!-- The live document. It is not a label, but it is what every
           label is compared against and the only way back from one, so
           it belongs at the head of the list rather than in a banner above
           the days: the list runs newest first, and nothing is newer. The
           filter leaves it out, since it is not a bookmark. -->
      {#if !onlyMarked}
        <li class="day-row day-now">
          {#if naming === "current"}
            {@render nameField("Name this bookmark")}
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
      {/if}
      {#each shown as group (group.day)}
        {@const expanded = isOpen(group)}
        <li class="day-group" data-day={group.day}>
          <button type="button" class="day-head" aria-expanded={expanded} onclick={() => toggleDay(group.day)}>
            <span class="day-name">{dayName(group.day)}</span>
            <span class="day-meta panel-meta">{dayMeta(group)}</span>
          </button>
          {#if expanded}
            {#if onlyMarked}
              <ol class="day-rows">
                {#each group.points.filter(bookmarked).reverse() as point (point.sha)}
                  {@render markRow(point)}
                {/each}
              </ol>
            {:else}
              {@const entries = dayEntries(labels, group.day, timezone)}
              <ol class="day-rows">
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
              </ol>
            {/if}
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<!-- The one control in the header: show only the bookmarked versions. The
     icon button reports its state through `pressed`, which it renders as
     aria-pressed. -->
{#snippet filterControl()}
  <span class="history-filter">
    <IconButton icon="bookmark" tone="plain" size="btn-icon-sm" filled={onlyMarked}
                pressed={onlyMarked} label="Show only bookmarked versions"
                onclick={() => (onlyMarked = !onlyMarked)} />
  </span>
{/snippet}

<!-- A bookmark in the filtered list. It is the version's row, with the same
     controls as anywhere else. The day is in the heading above it, so the
     row says only the time. -->
{#snippet markRow(point)}
  <li class="mark-row" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField("Name this bookmark")}
    {:else}
      <button type="button" class="timeline-point mark-point"
              class:timeline-here={viewing === point.sha}
              class:timeline-against={against === point.sha}
              aria-current={viewing === point.sha ? "true" : undefined}
              title="Compare {point.label} from {stamp(point.at) || shortSha(point.sha)} with the {compareTo(point)}"
              onclick={(event) => (event.shiftKey ? oncompare?.(point.sha) : onview?.(point.sha))}>
        <span class="mark-name">{point.label}</span>
        <span class="mark-when panel-meta">{clock(point.at)}</span>
        {#if against === point.sha}<span class="sr-only">Comparison base</span>{/if}
      </button>
      {#if canEdit || viewing !== point.sha}
        <span class="day-tools" class:on={against === point.sha}>
          {#if canEdit}
            <IconButton icon="pencil" tone="plain" size="btn-icon-sm"
                        label="Rename this bookmark" onclick={() => startNaming(point)} />
            <IconButton icon="trash" tone="plain" size="btn-icon-sm"
                        label="Remove this bookmark" onclick={() => unbookmark(point)} />
          {/if}
          {@render compareTool(point)}
        </span>
      {/if}
    {/if}
  </li>
{/snippet}

<!-- One version: the time it was taken, and a word only where there is one.
     The same row whether it stands on its own or inside a run. -->
{#snippet version(point)}
  {@const here = viewing === point.sha}
  {@const who = actor(point)}
  <li class="day-row" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField("Name this bookmark")}
    {:else}
      <button
        type="button"
        class="timeline-point day-point"
        class:timeline-here={here}
        class:timeline-against={against === point.sha}
        aria-current={here ? "true" : undefined}
        title="Compare {told(point)} from {stamp(point.at) || shortSha(point.sha)} with the {compareTo(point)}"
        onclick={(event) => (event.shiftKey ? oncompare?.(point.sha) : onview?.(point.sha))}
      >
        <span class="day-when">{clock(point.at)}</span>
        <!-- The word, where there is one. The cell stays either way, so that
             what follows it keeps its column; the word an autosave withholds
             from the page is still owed to a screen reader, and it is said
             out of the flow, beside the cell rather than in it. -->
        <span class="day-what">{said(point)}</span>
        {#if !said(point)}<span class="sr-only">Synced</span>{/if}
        {#if against === point.sha}<span class="sr-only">Comparison base</span>{/if}
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
      <!-- The one thing that can be done to a version from the list: mark
           it, so that it can be found again by name. It is on every row, since
           a row has to be markable before it is the one being read; but a
           control on every line of a list is noise, so it is drawn only under
           the pointer, on the row being read, on the row a keyboard has
           reached -- and always where the mark is already set, because there
           the icon is not a control offering itself, it is the answer. -->
      {#if canEdit || bookmarked(point) || !here}
        <span class="day-tools" class:on={bookmarked(point) || against === point.sha}>
          {#if canEdit}
            <IconButton icon="bookmark" tone="plain" size="btn-icon-sm" filled={bookmarked(point)}
                        label={bookmarked(point) ? "Rename this bookmark" : "Bookmark this version"}
                        onclick={() => startNaming(point)} />
          {:else if bookmarked(point)}
            <span class="mark-still"><Icon name="bookmark" filled /></span>
          {/if}
          {@render compareTool(point)}
        </span>
      {/if}
    {/if}
  </li>
{/snippet}

<!-- Comparing. Any version but the one being read can be made the base of a
     comparison, or stop being it, by anyone who can see the panel. -->
{#snippet compareTool(point)}
  {#if viewing !== point.sha}
    {#if against === point.sha}
      <IconButton icon="diff" tone="plain" size="btn-icon-sm"
                  label="Stop comparing with this version" onclick={() => oncompare?.("")} />
    {:else}
      <IconButton icon="diff" tone="plain" size="btn-icon-sm"
                  label="Compare with this version" onclick={() => oncompare?.(point.sha)} />
    {/if}
  {/if}
{/snippet}

{#snippet nameField(label)}
  <input
    bind:this={field}
    bind:value={draft}
    class="input"
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

  /* The panel is a column: the header and the notices sit at the top, and the
     list takes what is left and scrolls on its own. */
  .timeline { display: flex; flex-direction: column; overflow: hidden; }
  .timeline :global(.panel-actions) { justify-content: flex-end; }

  /* ----------------------------------------------------------- the list */

  /* The list runs the full width of the panel, so that a day's heading can
     hold the sidebar ground across the whole of it while the days under it
     scroll. The gutter is given back as padding inside the list, so the rows
     still start at the panel's own edge. */
  .history-list {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    margin-inline: calc(var(--panel-padding) * -1);
    padding-inline: var(--panel-padding);
    padding-bottom: var(--panel-padding);
  }
  .day-group { display: block; }

  /* A day's heading. It is the only thing a folded day shows, and it stays
     at the top of the list while its own versions scroll past under it. The
     heading takes the panel's ground, so the rows do not show through it. */
  .day-head {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    align-items: baseline;
    gap: calc(var(--spacing) * 2);
    width: 100%;
    margin-inline: calc(var(--panel-padding) * -1);
    padding: calc(var(--spacing) * 2) var(--panel-padding) calc(var(--spacing) * 0.5);
    background: var(--color-sidebar);
    text-align: left;
    cursor: pointer;
  }
  .day-head:hover .day-name { text-decoration: underline; }
  .day-name {
    flex: none;
    font-size: var(--panel-font-size);
    font-weight: 500;
    white-space: nowrap;
  }
  .day-meta {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .day-rows { padding-bottom: calc(var(--spacing) * 2); }

  /* ----------------------------------------------------------- the marks */

  /* The name and what can be done to it, at the two ends of the row. The
     controls do not wait to be hovered here: this view is about the marks
     themselves, so the two things that can be done to one are simply on it. */
  .mark-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
  }
  /* The name is the row's subject and the date is its metadata, which is the
     reverse of the list's versions, where the time is how a version is found. */
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
    color: var(--color-link);
    box-sizing: content-box;
  }

  /* ----------------------------------------------------------------- the day */

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
  /* The base of a comparison is a second place to look, not the one being
     read, so it gets a thin accent on the leading edge and no ground. */
  .timeline-against { box-shadow: inset 2px 0 var(--color-brand); }
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
     itself on every line of a list is noise, so on the list it waits for
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
  .day-row:has(.timeline-here) .day-tools,
  .day-row:has(.timeline-against) .day-tools { opacity: 1; }
  /* The mark itself is coloured; an empty bookmark waiting to be set is not,
     or every row would read as half-marked. */
  .day-tools.on { color: var(--color-link); }

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
