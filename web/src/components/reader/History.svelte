<script>
  // The document's past, narrowed one step at a time.
  //
  // Month, then sitting, then -- only when a sitting is busy enough to need
  // it -- the minutes inside it. Each step answers one question and then
  // stands down; two of them are never on screen together.
  //
  // The middle step is the one that matters, and it is deliberately *not* a
  // clock. A day of writing a paper is twenty minutes after breakfast and an
  // hour after lunch; a twenty-four-hour axis spends five hours of height on
  // the nothing in between and squeezes the work into a few pixels. So the
  // day is cut where the work stopped, and the panel lists sittings. Vertical
  // distance between them means "next", not "three hours" -- the silence is
  // written in words, which costs one line instead of half the panel.
  //
  // A proportional axis is right for exactly one thing: a single sitting,
  // where the whole window is half an hour and spatial distance can honestly
  // mean elapsed time. That is the third step, and a sitting holding three
  // versions or fewer never needs it -- they are simply listed.
  //
  // Presentation only: the shared source controller owns selection, and this
  // reports outwards.
  import { day as dayOf } from "../../lib/dates.js";
  import { shortSha, sizeDelta } from "../../lib/history.js";
  import {
    SESSION_GAP_MINUTES, daySessions, monthGrid, monthOf, monthSpan,
    sessionBins, sessionSparkline, shiftDay, shiftMonth, versionDays,
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
    // open those moments. The sittings are cut from this and the versions
    // together; the sparklines and the strip are drawn from it alone.
    activity = [],
    activityProblem = "",
    activityLoading = false,
    viewingMoment = "",
    onmoment,
    /// How long a pause has to be before it is a different sitting. A fact
    /// about how the reader works, so it is theirs to set rather than the
    /// panel's to assume.
    sessionGap = SESSION_GAP_MINUTES,
  } = $props();

  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  const today = $derived(dayOf(new Date(), timezone));

  // The checkpoint being named, and what it is being named. One at a time,
  // because naming two moments at once is not a thing anybody does.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // Which step is on screen. The panel opens on the day's sittings, not on
  // the month: somebody opening a version history almost always wants the
  // last thing they did, and the month is one click above it.
  let picking = $state(false);
  let opened = $state("");
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
  const worked = $derived(byDay.get(day));

  const sessions = $derived(daySessions(checkpoints, activity, day, timezone, { gap: sessionGap }));
  const key = (session) => `${session.from}-${session.to}`;
  const session = $derived(sessions.find((one) => key(one) === opened) || null);
  // A version chosen from outside the open sitting closes it -- from a link,
  // from the named list, from the address bar. Leaving the reader looking at
  // a sitting that does not hold the thing they are comparing helps nobody,
  // and the day above it does hold it.
  $effect(() => {
    if (!opened || !viewing) return;
    const found = sessions.find((one) => key(one) === opened);
    if (found && !found.versions.some((one) => one.point.sha === viewing)) opened = "";
  });

  // How many versions a sitting will list before it stops. Three or fewer are
  // simply listed: a proportional view of four minutes holding two dots tells
  // a reader nothing they could not already see.
  const INLINE_VERSIONS = 3;
  const dense = (one) => one.versions.length > INLINE_VERSIONS;
  // When a sitting has more in it than the feed can show, and so is worth
  // opening: too many versions to list, or minutes somebody wrote in that no
  // version was taken of -- the axis is the only way to reach one of those.
  // A sitting that is a single publish has neither, and offers nothing.
  const openable = (one) => dense(one) || one.moments.some((m) => m.frontier);
  // A version with nothing written around it -- a publish from the command
  // line, a restore -- is one event, not a sitting. It reads as itself.
  const lone = (one) => one.versions.length === 1 && one.moments.length === 0;

  const WEEKDAYS = ["S", "M", "T", "W", "T", "F", "S"];
  const versions = (n) => `${n} version${n === 1 ? "" : "s"}`;
  const writes = (n) => `${n} write${n === 1 ? "" : "s"}`;

  function open(value) {
    pickedDay = value;
    pickedMonth = monthOf(value);
    picking = false;
    opened = "";
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

  // What a cell says when a reader stops on it, in versions and writes rather
  // than bytes: nobody thinks about their afternoon in kilobytes.
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

  // Minutes from midnight, turned back into something a person reads. Every
  // time in the panel goes through one of these, so a session heading and a
  // version's own label cannot disagree about what half past one is called.
  const at = (minute) =>
    new Date(Date.UTC(2000, 0, 1, Math.floor(minute / 60), minute % 60)).toLocaleTimeString([], {
      hour: "numeric", minute: "2-digit", timeZone: "UTC",
    });
  // A session's heading. One minute long is one time, not a range that says
  // the same thing twice.
  const range = (one) => (one.span > 0 ? `${at(one.from)}–${at(one.to)}` : at(one.from));
  // How long the silence before a sitting was. Rounded, because the point is
  // "a long time" or "a coffee", not a stopwatch.
  function lapse(minutes) {
    if (minutes >= 1440) {
      const days = Math.round(minutes / 1440);
      return `${days} day${days === 1 ? "" : "s"}`;
    }
    const hours = Math.floor(minutes / 60);
    const rest = minutes % 60;
    if (!hours) return `${rest}m`;
    return rest ? `${hours}h ${rest}m` : `${hours}h`;
  }
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

  /* --------------------------------------------------------- one sitting */

  // How tall a minute is inside a sitting. Chosen from the sitting's own
  // length, so half an hour and two hours both arrive at a readable height
  // rather than one of them being a stripe and the other a mile.
  const perMinute = $derived(
    session ? Math.max(4, Math.min(46, 360 / Math.max(1, session.span))) : 0,
  );
  const PAD = 14;
  const height = $derived(session ? Math.max(120, PAD * 2 + session.span * perMinute) : 0);
  /// Where a minute of this sitting sits on its axis.
  const down = (minute) => PAD + (minute - (session?.from ?? 0)) * perMinute;
  // A bin is a minute when there is room to draw one, and coarser when there
  // is not, so a bar is never thinner than a hairline.
  const binSpan = $derived(perMinute >= 6 ? 1 : Math.ceil(6 / Math.max(1, perMinute)));
  const bins = $derived(session ? sessionBins(session, binSpan) : []);
  // Ticks every 1, 2, 5, 10, 15, 30 or 60 minutes -- whichever first leaves
  // enough room between two of them to read the labels.
  const tickStep = $derived(
    [1, 2, 5, 10, 15, 30, 60].find((step) => step * perMinute >= 26) || 60,
  );
  const ticks = $derived.by(() => {
    if (!session) return [];
    const first = Math.ceil((session.from - 1) / tickStep) * tickStep;
    const out = [];
    for (let minute = first; minute <= session.to + 1; minute += tickStep) out.push(minute);
    return out;
  });

  // Where each version sits on the sitting's axis, and where its label sits.
  //
  // The dot is always at its own minute -- that is the whole point of having
  // an axis -- but two versions a minute apart would have their labels on top
  // of each other, so a label that would collide is pushed down and a leader
  // is drawn back to the dot it belongs to. The panel never moves a dot to
  // make room for words.
  const ROW = 26;
  const GAP = 3;
  let openHeight = $state(ROW);
  const placed = $derived.by(() => {
    if (!session) return [];
    const out = [];
    let floor = -Infinity;
    for (const one of session.versions) {
      const home = down(one.minute);
      const top = Math.max(home, floor);
      const tall = viewing === one.point.sha ? Math.max(ROW, openHeight) : ROW;
      out.push({ ...one, home, top, pushed: top - home > 1.5 });
      floor = top + tall + GAP;
    }
    return out;
  });

  /* ---------------------------------------------------------- one version */

  // What a checkpoint was taken for, in words rather than in the manifest's
  // own vocabulary. `quiet` and `left` are the two nobody asked for, and
  // calling them anything but autosaved would say the wrong thing about who
  // was there: what they have in common is that the author stopped.
  const WHY = {
    quiet: "Autosaved",
    left: "Autosaved",
    automatic: "Autosaved",
    sync: "Synced",
    comment: "Commented",
    cli: "Published",
    publish: "Published",
    restore: "Restored",
    superseded: "Replaced by a restore",
    label: "Named",
    recovered: "Recovered",
    accept: "Accepted a suggestion",
  };
  const QUIET = new Set(["quiet", "left", "automatic", "sync"]);
  const title = (point) => point.label || WHY[point.why] || point.why || "Saved";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const actor = (point) => {
    if (!point?.by || point.by === "system") return "";
    return uuid.test(point.by) ? "Unknown editor" : point.by;
  };
  // What a version moved. The manifest answers this in three ways and they
  // are three different things: a list of paths, an empty list, and no answer
  // at all. No answer says nothing here, because a version that cannot
  // account for itself should not claim to. An empty list is an answer: no
  // file differs from the version before, so what this one holds differently
  // is outside the files.
  const moved = (point) => {
    const paths = point?.changed;
    if (!Array.isArray(paths)) return "";
    if (paths.length === 0) return "No files changed";
    if (paths.length <= 2) return paths.map((path) => path.split("/").pop()).join(", ");
    return `${paths.length} files changed`;
  };
  // A version somebody asked for, rather than one the document took by
  // itself. It is the only thing that gets a filled dot.
  const deliberate = (point) => Boolean(point.label) || !QUIET.has(point.why);

  // The moment a bar opens: the last write in it that left an anchor.
  // Clicking the strip is how a reader reaches a minute nobody saved, which
  // is the whole reason the operation history is kept rather than thinned --
  // and it needs no rows of its own, because the bar is already at the right
  // place on the axis.
  const anchorOf = (bin) => [...bin.moments].reverse().find((one) => one.frontier)?.frontier || "";
  const showing = (bin) =>
    Boolean(viewingMoment) && bin.moments.some((one) => one.frontier === viewingMoment);

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

  <!-- The live document. Not part of navigating the past, so it is a status
       line above it rather than a fourth thing competing for the eye. -->
  <div class="now timeline-now" class:now-here={!viewing}>
    {#if naming === "current"}
      {@render nameField("Name this version", "Name the current version")}
    {:else}
      <button
        type="button"
        class="timeline-point now-point"
        class:timeline-here={!viewing}
        aria-current={!viewing ? "true" : undefined}
        title="Show the current source"
        onclick={() => onview?.("")}
      >
        <span class="now-dot"></span>
        <span class="now-what">{currentLabel || "Current version"}</span>
      </button>
      {#if canEdit && !viewing}
        <IconButton icon="pencil" tone="plain" size="btn-icon-sm" label="Name the current version"
                    onclick={() => startNaming({ sha: "current", label: currentLabel })} />
      {/if}
    {/if}
  </div>
  {#if durability?.live_save === "pending"}
    <p class="panel-muted px-2 pb-1 text-xs" role="status">Live edits are still being saved.</p>
  {/if}

  {#if activityProblem && !checkpoints.length}
    <p class="text-error-500 px-2 py-1 text-sm" role="status">{activityProblem}</p>
  {:else if activityLoading && !anything}
    <p class="panel-muted px-2 py-1">Reading this document's past…</p>
  {:else if anything && picking}
    {@render monthStep()}
  {:else if anything && session}
    {@render sessionStep(session)}
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
                <!-- How much was written that day, as a mark small enough to
                     scan past. A day nobody touched draws nothing at all, so
                     the month reads as the handful of days somebody worked
                     rather than as a grid of boxes. -->
                {#if cell.level || cell.worked}
                  <span class="cal-mark" data-level={cell.level || 1}></span>
                {/if}
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

<!-- Step two: which sitting. A feed, not a calendar: the gaps are sentences
     rather than empty space. -->
{#snippet dayStep()}
  <div class="crumb">
    <button type="button" class="crumb-back" onclick={() => (picking = true)}
            aria-label="Back to {monthLabel}">
      <span aria-hidden="true">‹</span> {monthLabel}
    </button>
    <span class="crumb-day">{dayLabel(day, false)}</span>
  </div>
  <p class="crumb-meta panel-meta">
    {worked?.count ? versions(worked.count) : "No versions"}{worked?.changes ? ` · ${writes(worked.changes)}` : ""}
  </p>

  <div class="feed">
    {#if !sessions.length}
      <p class="panel-muted">Nothing was written on this day.</p>
    {/if}
    {#each sessions as one (key(one))}
      {#if one.since !== null}
        <!-- Three hours of silence and twenty minutes of it take the same
             one line. Spending height in proportion to time not spent
             working is spending the panel on nothing. -->
        <p class="feed-gap panel-meta">{lapse(one.since)} later</p>
      {/if}
      {#if lone(one)}
        <!-- One version and nothing written around it: a publish, a restore.
             It is an event, not a sitting, and reads as itself. -->
        <div class="sitting sitting-lone">
          {@render mark(one.versions[0].point)}
        </div>
      {:else}
        <div class="sitting" data-session={key(one)}>
          <!-- A sitting with more in it than three lines can hold opens onto
               its own axis. This is the only place the panel asks for a third
               click, and only when the third view has something to add. -->
          {#if openable(one)}
            <button type="button" class="sitting-head sitting-open"
                    onclick={() => (opened = key(one))}
                    aria-label="Open {range(one)}, {versions(one.versions.length)}">
              {@render heading(one, true)}
            </button>
          {:else}
            <div class="sitting-head">{@render heading(one)}</div>
          {/if}
          <!-- Its versions stay listed where they are all the same, up to the
               handful a reader can take in at a glance. Hiding two rows
               behind a click buys nothing. -->
          {#if !dense(one)}
            <ol class="sitting-versions">
              {#each one.versions as found (found.point.sha)}
                <li>{@render mark(found.point)}</li>
              {/each}
            </ol>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/snippet}

{#snippet heading(one, more = false)}
  <!-- Three short lines rather than one long one: the panel is a sidebar, and
       a heading that needs four hundred pixels is a heading that scrolls
       sideways. -->
  <span class="sitting-top">
    <span class="sitting-when">{range(one)}</span>
    {#if more}<span class="sitting-more" aria-hidden="true">›</span>{/if}
  </span>
  {#if one.writes}
    <!-- The shape of the sitting: steady, or one burst at the end, or
         stop-start. A fixed width, because it is a word rather than a
         measurement -- the axis inside is where a bar means a time. -->
    <span class="spark" aria-hidden="true">
      {#each sessionSparkline(one, 24) as bar, index (index)}
        <span class="spark-bar" style={`height:${bar.share ? Math.max(12, bar.share * 100) : 0}%`}></span>
      {/each}
    </span>
  {/if}
  <span class="sitting-meta panel-meta">
    {one.writes ? `${writes(one.writes)} · ` : ""}{versions(one.versions.length)}
  </span>
{/snippet}

<!-- Step three: the minutes of one sitting, on a real axis. Half an hour of
     window, so spatial distance can honestly mean elapsed time. -->
{#snippet sessionStep(one)}
  <div class="crumb">
    <button type="button" class="crumb-back" onclick={() => (opened = "")}
            aria-label="Back to {dayLabel(day)}">
      <span aria-hidden="true">‹</span> {dayLabel(day, false)}
    </button>
    <span class="crumb-day">{range(one)}</span>
  </div>
  <p class="crumb-meta panel-meta">
    {one.writes ? `${writes(one.writes)} · ` : ""}{versions(one.versions.length)}
  </p>

  <div class="axis">
    <div class="axis-canvas" style={`height:${height}px`}>
      {#each ticks as minute (minute)}
        <span class="axis-hour" style={`top:${down(minute)}px`}>{at(minute)}</span>
        <span class="axis-rule" style={`top:${down(minute)}px`}></span>
      {/each}

      {#each bins as bin (bin.at)}
        {#if bin.share > 0}
          <button
            type="button"
            class="day-bar"
            class:day-bar-here={showing(bin)}
            data-level={bin.level}
            data-minute={bin.at}
            disabled={!anchorOf(bin)}
            style={`top:${down(bin.at)}px;height:${Math.max(2, bin.span * perMinute - 1)}px;width:${Math.max(3, bin.share * 44)}px`}
            title={`${at(bin.at)} — ${writes(bin.changes)}`}
            aria-label={`${writes(bin.changes)} at ${at(bin.at)}, not saved as a version`}
            onclick={() => onmoment?.(anchorOf(bin), bin.moments[bin.moments.length - 1])}
          ></button>
        {/if}
      {/each}

      {#each placed as found (found.point.sha)}
        <span class="day-dot" class:day-dot-deliberate={deliberate(found.point)}
              class:day-dot-here={viewing === found.point.sha}
              style={`top:${found.home}px`}></span>
        {#if found.pushed}
          <span class="day-leader" style={`top:${found.home}px;height:${found.top - found.home}px`}></span>
        {/if}
        <div class="axis-mark" style={`top:${found.top}px`}>{@render mark(found.point)}</div>
      {/each}
    </div>
  </div>
{/snippet}

<!-- One version, wherever it is drawn: in a sitting's list, alone as its own
     event, or against the axis. Details belong to the one being looked at and
     to no other. -->
{#snippet mark(point)}
  <div class="day-version" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField("sent to the journal", "Name this point")}
    {:else}
      <button
        type="button"
        class="timeline-point day-point"
        class:timeline-here={viewing === point.sha}
        aria-current={viewing === point.sha ? "true" : undefined}
        title="Compare {title(point)} from {stamp(point.at) || shortSha(point.sha)} with the current source"
        onclick={() => onview?.(point.sha)}
      >
        <span class="day-dot day-dot-inline" class:day-dot-deliberate={deliberate(point)}></span>
        <span class="day-when">{clock(point.at)}</span>
        <span class="day-what">{title(point)}</span>
      </button>
      {#if viewing === point.sha}
        <div class="day-card" bind:clientHeight={openHeight}>
          {#if actor(point)}<p class="day-card-by">{actor(point)}</p>{/if}
          {#if moved(point)}
            {@const size = sizeDelta(point, checkpoints)}
            <p class="day-card-moved panel-meta">
              {moved(point)}{#if size} · {size.title}{/if}
            </p>
          {/if}
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
  </div>
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
  /* Everything the day and the sitting draw is prefixed `day-`: the design
     system already has an unscoped `.mark`, `.bar` and `.card`, and a
     component style is scoped but does not stop somebody else's unscoped rule
     from also matching the element. A distinctive name is the only thing that
     does.

     Three levels of type and almost no boxes. What used to be grey furniture
     -- cell backgrounds, hour rules, bar troughs, timestamp pills -- is
     either the panel's own ground or gone, so the accent is left to mean the
     four things it should: the day being read, activity, a version, and the
     version chosen. */

  /* ------------------------------------------------------ the live document */

  .now {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--spacing);
    padding: 0 calc(var(--spacing) * 2) var(--spacing);
  }
  .now-point {
    flex: 1;
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 1.5);
    min-width: 0;
    padding: var(--spacing);
    border-radius: var(--radius-container);
    font-size: var(--panel-font-size);
    text-align: left;
    cursor: pointer;
  }
  .now-point:hover { background: var(--color-row-hover); }
  .now-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    box-shadow: inset 0 0 0 1.5px var(--color-surface-400-600);
  }
  .now-what { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .now-here .now-dot { background: var(--color-primary-500); box-shadow: none; }
  .now-here .now-what { font-weight: 500; }

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
     buttons. Its height is given over to whitespace around the number and the
     one small mark under it. */
  .cal-cell {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
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
  /* How much was written, in four steps of one hue. Two pixels tall, so it
     reads at about the weight of a diacritic: present, proportionate, and
     never competing with the date above it. */
  .cal-mark {
    width: 14px;
    height: 2px;
    border-radius: 1px;
    background: var(--color-primary-500);
    opacity: 0.25;
  }
  .cal-mark[data-level="2"] { opacity: 0.45; }
  .cal-mark[data-level="3"] { opacity: 0.7; }
  .cal-mark[data-level="4"] { opacity: 1; }
  /* How many versions, for the reader who wants the number rather than the
     impression. In the corner, because the date is what the cell is for. */
  .cal-count {
    position: absolute;
    top: 1px;
    right: 2px;
    font-size: 0.5625rem;
    line-height: 1;
    font-weight: 600;
    color: var(--color-primary-600-400);
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

  /* ------------------------------------------------------------ the crumbs */

  /* The step above, once it has answered: a way back, and what it chose. */
  .crumb {
    flex: none;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: calc(var(--spacing) * 2);
    padding: 0 calc(var(--spacing) * 2);
  }
  .crumb-back {
    padding: var(--spacing) calc(var(--spacing) * 1.5) var(--spacing) 0;
    font-size: var(--panel-meta-size);
    color: var(--panel-muted);
    white-space: nowrap;
    cursor: pointer;
  }
  .crumb-back:hover { color: var(--color-surface-950-50); }
  .crumb-day { font-size: var(--panel-font-size); font-weight: 600; }
  .crumb-meta {
    flex: none;
    padding: 0 calc(var(--spacing) * 2) calc(var(--spacing) * 2);
    text-align: right;
  }

  /* ---------------------------------------------------------- the sittings */

  .feed {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 0 calc(var(--spacing) * 2) calc(var(--spacing) * 4);
    border-top: 1px solid var(--color-surface-200-800);
  }
  /* The silence between two sittings, in words. A rule through it, so it
     reads as a break rather than as a third kind of entry. */
  .feed-gap {
    display: flex;
    align-items: center;
    gap: var(--spacing);
    margin: calc(var(--spacing) * 2) 0;
    white-space: nowrap;
  }
  .feed-gap::after {
    content: "";
    flex: 1;
    height: 1px;
    background: var(--color-surface-200-800);
  }
  .sitting { padding-block: calc(var(--spacing) * 0.5); }
  .sitting-head {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
    width: 100%;
    padding: var(--spacing) calc(var(--spacing) * 0.5);
    border-radius: var(--radius-container);
    text-align: left;
  }
  .sitting-open { cursor: pointer; }
  .sitting-open:hover { background: var(--color-row-hover); }
  .sitting-top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--spacing);
    width: 100%;
  }
  .sitting-when {
    font-size: var(--panel-font-size);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .sitting-meta { white-space: nowrap; }
  .sitting-more { flex: none; color: var(--panel-muted); }
  /* The sparkline: one hue, bottom-aligned, in a fixed width whatever the
     sitting's length. It is the only thing in the feed drawn rather than
     written, and it is small enough to read as a texture. */
  .spark {
    display: flex;
    align-items: flex-end;
    gap: 1px;
    height: 16px;
  }
  .spark-bar {
    width: 3px;
    min-height: 0;
    border-radius: 1px;
    background: var(--color-primary-500);
    opacity: 0.55;
  }
  .sitting-versions { padding-left: calc(var(--spacing) * 2); }

  /* ----------------------------------------------------------- one sitting */

  /* One axis, over one interval. Everything inside is positioned from the top
     in minutes, so a bar, a label and a dot at the same time are at the same
     height by construction rather than by agreement. */
  .axis {
    --gutter: 46px;
    --strip: 44px;
    --rail: calc(var(--gutter) + 8px + var(--strip) + 12px);
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    border-top: 1px solid var(--color-surface-200-800);
    padding-bottom: calc(var(--spacing) * 4);
  }
  .axis-canvas { position: relative; }
  .axis-hour {
    position: absolute;
    left: 0;
    width: var(--gutter);
    transform: translateY(-0.55em);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--color-surface-500);
    text-align: right;
    white-space: nowrap;
  }
  /* The tick's own line, kept faint enough to be a graticule rather than a
     divider: it is there to let the eye carry a marker back to its minute. */
  .axis-rule {
    position: absolute;
    left: calc(var(--gutter) + 8px);
    right: 0;
    height: 1px;
    background: var(--color-surface-200-800);
    opacity: 0.4;
  }
  .axis-mark {
    position: absolute;
    left: calc(var(--rail) + 8px);
    right: calc(var(--spacing) * 2);
  }
  /* A bar is a minute of writing. It starts at the axis and grows right,
     never past the strip, so its length stays readable as a quantity instead
     of turning into a rule across the panel. */
  .day-bar {
    position: absolute;
    left: calc(var(--gutter) + 8px);
    border-radius: 0 1px 1px 0;
    background: var(--color-primary-500);
    opacity: 0.3;
    cursor: pointer;
  }
  .day-bar[data-level="2"] { opacity: 0.5; }
  .day-bar[data-level="3"] { opacity: 0.7; }
  .day-bar[data-level="4"] { opacity: 0.85; }
  .day-bar:hover:not(:disabled) { opacity: 1; }
  .day-bar:disabled { cursor: default; }
  .day-bar-here { opacity: 1; box-shadow: 0 0 0 1px var(--color-primary-500); }

  /* ---------------------------------------------------------- one version */

  /* A version. Hollow for one the document took by itself, filled for one
     somebody asked for. On the axis it is placed at its own minute; in a
     list it sits at the head of its own row, and it is the same mark either
     way so the two views read as one vocabulary. */
  .day-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--color-surface-50-950);
    box-shadow: inset 0 0 0 1.5px var(--color-primary-500);
  }
  .day-version > .timeline-point > .day-dot { flex: none; }
  .axis-mark .day-dot-inline { display: none; }
  .axis-canvas > .day-dot {
    position: absolute;
    left: var(--rail);
    margin-top: -4.5px;
  }
  .day-dot-deliberate { background: var(--color-primary-500); }
  .day-dot-here {
    background: var(--color-primary-500);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary-500) 25%, transparent);
  }
  /* When a label had to be pushed clear of the one above it, this is what
     says where it belongs. The dot never moves. */
  .day-leader {
    position: absolute;
    left: calc(var(--rail) + 4px);
    width: 1px;
    background: var(--color-primary-500);
    opacity: 0.3;
  }
  .day-point {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 1.5);
    width: 100%;
    min-width: 0;
    padding: calc(var(--spacing) * 0.5) var(--spacing);
    border-radius: var(--radius-container);
    line-height: 1.25;
    text-align: left;
    cursor: pointer;
  }
  .day-point:hover { background: var(--color-row-hover); }
  .day-when {
    flex: none;
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--color-surface-500);
  }
  .day-what {
    min-width: 0;
    font-size: var(--panel-font-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .timeline-here .day-what { font-weight: 600; }
  .timeline-here .day-when { color: var(--color-primary-700-300); }
  /* The details of the one version being looked at, and only that one. The
     views above are for finding a moment; what the moment holds is a question
     asked after it is found. */
  .day-card {
    padding: var(--spacing) var(--spacing) calc(var(--spacing) * 0.5)
             calc(var(--spacing) * 4);
    line-height: 1.3;
  }
  .day-card-by { font-size: var(--panel-meta-size); }
  .day-card-actions {
    display: flex;
    gap: calc(var(--spacing) * 0.5);
    margin-top: calc(var(--spacing) * 0.5);
  }
</style>
