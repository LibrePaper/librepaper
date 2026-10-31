<script>
  // The document's history as a list of events, not of raw versions. An event
  // is a stretch of editing, a push, a restore, a bookmark or the
  // creation of the document, and it says what that stretch was and when. Its
  // versions open under it on request, so a long editing session is one line.
  //
  // Periods adapt to the history. Recent days each get a heading. Older days
  // fold into the rest of the month, into months, and into years, so the list
  // stays short however old the document is. Older pages are loaded when the
  // reader reaches the end of the list or asks for a date.
  //
  // Presentation only. The shared source controller owns selection, and this
  // reports outwards. A bookmark is a version with a name, and naming it uses
  // the same field everywhere in the panel.
  import { tick, untrack } from "svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";
  import { day as dayOf } from "../../lib/dates.js";
  import { shortSha } from "../../lib/history.js";
  import { buildPeriods, nearestPeriod, pathTo, periodTitle } from "../../lib/history-groups.js";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import PanelHeader from "../PanelHeader.svelte";

  let {
    labels = [],
    viewing = null,
    against = "",
    canEdit = false,
    problem = "",
    loaded = false,
    complete = true,
    loadingMore = false,
    moreProblem = "",
    saved = [],
    savedLoaded = false,
    savedLoading = false,
    savedProblem = "",
    onview,
    oncompare,
    onname,
    onretry,
    onloadmore,
    onloaduntil,
    onloadsaved,
  } = $props();

  const SHOW_FIRST = 5;
  const SHOW_MORE = 50;

  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  const today = $derived(dayOf(new Date(), timezone));
  const periods = $derived(buildPeriods(labels, { timeZone: timezone, today, partial: !complete }));

  // Which view is showing, and whether the date popover is open.
  let mode = $state("all");
  let dateOpen = $state(false);
  let month = $state("");
  let note = $state("");

  // Elements the script needs to reach.
  let listEl = $state(null);
  let dateWrap = $state(null);
  let monthField = $state(null);

  // The label being bookmarked, and what it is being called. One at a time.
  let naming = $state("");
  let draft = $state("");
  let field = $state(null);

  // Open state is the default from the groups, flipped by a click. Each set
  // holds the keys whose state the reader has changed. Sets are replaced, not
  // mutated, so the template sees the change.
  let toggled = $state(new Set());
  let expandedEvents = $state(new Set());
  let reveal = $state(new Map());

  const openDefaults = (list, map = new Map()) => {
    for (const period of list) {
      map.set(period.key, period.open);
      if (period.children) openDefaults(period.children, map);
    }
    return map;
  };
  const defaults = $derived(openDefaults(periods));
  const openNow = (key) => toggled.has(key) !== (defaults.get(key) ?? false);
  const flipped = (set, key) => {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  };
  function setPeriod(key, want) {
    note = "";
    if (openNow(key) !== want) toggled = flipped(toggled, key);
  }
  function setEvent(key, want) {
    if (expandedEvents.has(key) !== want) expandedEvents = flipped(expandedEvents, key);
  }

  // The viewed version opens its period and its ancestors, once, when it
  // changes. Events are not opened, so the reader can still close them.
  let followed;
  $effect(() => {
    const sha = viewing;
    if (sha === followed) return;
    followed = sha;
    if (!sha) return;
    untrack(() => {
      for (const key of pathTo(periods, sha)) {
        if (key.includes(":")) setPeriod(key, true);
      }
    });
  });

  // An expanded event shows its first few versions, and any deeper version
  // being viewed, and more on request.
  function countFor(event) {
    const base = reveal.get(event.key) ?? SHOW_FIRST;
    const deepest = event.points.findIndex((point) => point.sha === viewing);
    return Math.max(base, deepest + 1);
  }
  function more(event) {
    reveal = new Map(reveal).set(event.key, countFor(event) + SHOW_MORE);
  }

  /* ------------------------------------------------------------ wording */

  // What a label was taken for, in words, for the tooltip and the screen
  // reader. `sync` is the one nobody asked for, and it is said as "Synced".
  const WHY = {
    comment: m.history_commented(),
    created: m.history_created(),
    cli: m.common_saved(),
    publish: m.history_shown_to_readers(),
    restore: m.history_restored(),
    superseded: m.history_replaced_by_restore(),
    recovered: m.history_recovered(),
    push: m.history_push(),
  };
  const said = (point) => point.label || WHY[point.why] || "";
  const told = (point) => said(point) || m.history_synced();

  const TITLE = {
    editing: m.history_editing_session(),
    collaborative: m.history_collaborative_session(),
    restore: m.history_restored_version(),
    created: m.history_created(),
  };
  const ICON = {
    editing: "pencil",
    collaborative: "users",
    push: "upload",
    restore: "history",
    saved: "bookmark",
    created: "file-plus",
  };
  // A push is named by its message, and says who pushed it: the author the
  // caller gave, or the account behind the token.
  const titleOf = (event) => {
    if (event.type === "saved") return event.points[0].label;
    if (event.type === "push") return event.authors.length ? m.history_pushed_by({ author: event.authors[0] }) : m.history_push();
    return TITLE[event.type];
  };

  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  // Who made a version, where the manifest can say. A machine save names
  // nobody, and neither does an identifier that is not a name.
  const editorOf = (point) => {
    if (!point?.by || point.by === "system" || uuid.test(point.by)) return "";
    return point.by;
  };

  const versionsText = (n, partial = false) =>
    partial ? m.history_versions_partial({ count: n }) : m.history_versions({ count: n });

  // The clock time in the reader's timezone, and the same to the second for
  // the tooltip.
  const clock = (value) => {
    const when = new Date(value);
    return Number.isNaN(when.getTime())
      ? ""
      : when.toLocaleTimeString(locale(), { hour: "numeric", minute: "2-digit", timeZone: timezone });
  };
  function stamp(value) {
    const when = new Date(value);
    return Number.isNaN(when.getTime()) ? "" : when.toLocaleString(locale(), {
      dateStyle: "medium", timeStyle: "short", timeZone: timezone,
    });
  }

  // A time range joined by an en dash, written as an escape. One time when the
  // two round to the same minute.
  const range = (event) => {
    const from = clock(event.start);
    const to = clock(event.end);
    if (from === to) return from;
    // "9:00\u20139:03 AM" rather than saying AM twice.
    const suffix = to.match(/\s\D+$/)?.[0] || "";
    const start = suffix && from.endsWith(suffix) ? from.slice(0, -suffix.length) : from;
    return `${start}\u2013${to}`;
  };
  const eventMeta = (event) =>
    [
      range(event),
      event.type === "editing" && event.authors.length === 1 ? event.authors[0] : "",
      event.type === "collaborative" && event.authors.length ? m.history_contributors({ count: event.authors.length }) : "",
      m.history_versions({ count: event.points.length }),
    ].filter(Boolean).join(" · ");
  const soloLine = (event) =>
    [clock(event.points[0].at), event.type === "push" ? event.points[0].label : event.authors.length === 1 ? event.authors[0] : ""]
      .filter(Boolean).join(" · ");

  // A saved version in the saved list: the date, with the year only when it
  // is not this year.
  function savedWhen(value) {
    const when = new Date(value);
    if (Number.isNaN(when.getTime())) return "";
    const thisYear = dayOf(when, timezone).slice(0, 4) === today.slice(0, 4);
    const date = when.toLocaleDateString(locale(), {
      month: "short",
      day: "numeric",
      ...(thisYear ? {} : { year: "numeric" }),
      timeZone: timezone,
    });
    return `${date} · ${clock(value)}`;
  }

  function monthName(value) {
    return new Date(`${value}-01T12:00:00Z`).toLocaleDateString(locale(), {
      month: "long", year: "numeric", timeZone: "UTC",
    });
  }

  /* ----------------------------------------------------------- actions */

  // What a new bookmark is called before anybody calls it anything. It comes
  // up selected, so the first key typed replaces it.
  const defaultName = (point) => {
    const when = new Date(point?.at || Date.now());
    const day = Number.isNaN(when.getTime()) ? new Date() : when;
    return m.history_draft_name({ date: day.toLocaleDateString(locale(), {
      month: "short", day: "numeric", year: "numeric", timeZone: timezone,
    }) });
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

  // Taking the bookmark off a version. The version itself is untouched.
  function unbookmark(point) {
    if (naming === point.sha) naming = "";
    onname?.(point.sha, "");
  }

  function pick(event, sha) {
    if (event.shiftKey) oncompare?.(sha);
    else onview?.(sha);
  }

  function showAll() {
    mode = "all";
    note = "";
  }
  function showSaved() {
    mode = "saved";
    dateOpen = false;
    note = "";
    onloadsaved?.();
  }

  async function toggleDate() {
    dateOpen = !dateOpen;
    if (!dateOpen) return;
    if (!month) month = today.slice(0, 7);
    await tick();
    monthField?.focus();
  }

  // The keys from the outermost period down to `key`, or null when absent.
  function pathKeys(list, key, trail = []) {
    for (const period of list) {
      const here = [...trail, period.key];
      if (period.key === key) return here;
      const found = period.children ? pathKeys(period.children, key, here) : null;
      if (found) return found;
    }
    return null;
  }

  // Load down to the month, open the period that answers it, and bring it to
  // the top of the list.
  async function goTo() {
    if (!month) return;
    await onloaduntil?.(`${month}-01T00:00:00Z`);
    const hit = nearestPeriod(periods, month);
    dateOpen = false;
    if (!hit) {
      note = m.history_no_versions_shown();
      return;
    }
    for (const key of pathKeys(periods, hit.key) ?? []) setPeriod(key, true);
    note = hit.exact ? "" : m.history_no_versions_in_month({ month: monthName(month) });
    await tick();
    listEl?.querySelector(`[data-period="${hit.key}"]`)?.scrollIntoView({ block: "start" });
  }

  function closeDate() {
    dateOpen = false;
    dateWrap?.querySelector("button")?.focus();
  }

  // Arrow keys move between the buttons of the list in document order.
  function navigate(event) {
    if (event.target instanceof HTMLInputElement) return;
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const target = event.target;
    const nodes = [...(listEl?.querySelectorAll("[data-nav]") ?? [])];
    const found = event.key === "ArrowDown"
      ? nodes.find((node) => target.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING)
      : nodes.filter((node) => node.compareDocumentPosition(target) & Node.DOCUMENT_POSITION_FOLLOWING).pop();
    if (!found) return;
    event.preventDefault();
    found.focus();
  }

  // ArrowRight opens and ArrowLeft closes a period or an event.
  function arrow(event, open, set) {
    if (event.key === "ArrowRight" && !open) {
      event.preventDefault();
      set(true);
    } else if (event.key === "ArrowLeft" && open) {
      event.preventDefault();
      set(false);
    }
  }

</script>

<div class="panel timeline">
  <PanelHeader title={m.common_history()} />

  <!-- The filter and the date control share one row: the panel's title is
       for screen readers only, so a header row would hold a lone icon. -->
  <div class="hx-bar">
    <div class="hx-segment" role="group" aria-label={m.history_filter_group()}>
      <!-- A narrow panel shows the short words. The long ones stay the
           buttons' names, so a screen reader hears the same thing at any width. -->
      <button type="button" aria-pressed={mode === "all"} aria-label={m.history_all()} onclick={showAll}>
        <span class="hx-long">{m.history_all()}</span><span class="hx-short" aria-hidden="true">{m.history_all_short()}</span>
      </button>
      <button type="button" aria-pressed={mode === "saved"} aria-label={m.history_saved_versions()} onclick={showSaved}>
        <span class="hx-long">{m.history_saved_versions()}</span><span class="hx-short" aria-hidden="true">{m.history_saved_short()}</span>
      </button>
    </div>
    {#if mode === "all"}
      <span class="hx-date-wrap" bind:this={dateWrap}>
        <IconButton icon="calendar" tone="plain" size="btn-icon-sm" label={m.history_go_to_date()}
                    expanded={dateOpen} controls="hx-date" onclick={toggleDate} />
      </span>
    {/if}
    {#if dateOpen && mode === "all"}
      <div class="hx-popover" id="hx-date" role="group" aria-label={m.history_go_to_month()}>
        <input
          type="month"
          class="input"
          aria-label={m.history_month()}
          bind:value={month}
          bind:this={monthField}
          onkeydown={(event) => {
            if (event.key === "Enter") goTo();
            if (event.key === "Escape") closeDate();
          }}
        />
        <button type="button" class="btn btn-sm" disabled={!month} onclick={goTo}>{m.history_go()}</button>
      </div>
    {/if}
  </div>

  {#if note}<p class="panel-meta hx-note" role="status">{note}</p>{/if}

  <!-- The handler only moves focus between the buttons inside the list. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <ol class="hx-list" bind:this={listEl} onkeydown={navigate}>
    {#if mode === "all"}
      <!-- The live document. It is what every bookmark is compared against,
           so it heads the list, and it is not an event. -->
      <li class="hx-current hx-row" class:hx-here={!viewing}>
        {#if naming === "current"}
          {@render nameField(m.history_name_bookmark())}
        {:else}
          <button
            type="button"
            class="hx-main"
            data-nav
            class:timeline-here={!viewing}
            aria-current={!viewing ? "true" : undefined}
            onclick={() => onview?.("")}
          >
            <span class="hx-text"><span class="hx-line1">{m.history_current_version()}</span></span>
          </button>
          {#if canEdit && !viewing}
            <span class="hx-tools">
              <IconButton icon="bookmark" tone="plain" size="btn-icon-sm"
                          label={m.history_bookmark_current()}
                          onclick={() => startNaming({ sha: "current", label: "" })} />
            </span>
          {/if}
        {/if}
      </li>

      {#if problem}
        <li class="hx-status">
          <p class="lp-text-error" role="status">{problem}</p>
          {#if !labels.length}<button type="button" class="btn btn-sm" onclick={() => onretry?.()}>{m.common_retry()}</button>{/if}
        </li>
      {/if}

      {#if !labels.length}
        {#if !loaded && !problem}
          <li class="hx-status panel-muted">{m.history_loading()}</li>
        {:else if loaded && !problem}
          <li class="hx-status panel-muted">{m.history_none()}</li>
        {/if}
      {:else}
        {#each periods as period (period.key)}
          {@render periodNode(period, false)}
        {/each}
      {/if}

      <!-- Older pages load when asked for, here or by the date control. An
           observer at the end of the list would page through everything,
           because with the months folded the end is nearly always in view. -->
      {#if loaded && (!complete || loadingMore || moreProblem)}
        <li class="hx-older">
          {#if loadingMore}
            <span class="panel-muted">{m.history_loading_older()}</span>
          {:else if moreProblem}
            <span class="lp-text-error" role="status">{moreProblem}</span>
            <button type="button" class="btn btn-sm" onclick={() => onloadmore?.()}>{m.common_retry()}</button>
          {:else}
            <button type="button" class="btn btn-sm" onclick={() => onloadmore?.()}>{m.history_load_older()}</button>
          {/if}
        </li>
      {/if}
    {:else}
      {#if savedProblem}
        <li class="hx-status">
          <p class="lp-text-error" role="status">{savedProblem}</p>
          <button type="button" class="btn btn-sm" onclick={() => onloadsaved?.()}>{m.common_retry()}</button>
        </li>
      {:else if savedLoading && !savedLoaded}
        <li class="hx-status panel-muted">{m.history_loading_saved()}</li>
      {:else if savedLoaded && !saved.length}
        <li class="hx-status panel-muted">{m.history_no_saved()}</li>
      {/if}
      {#each saved as point (point.sha)}
        {@render savedRow(point)}
      {/each}
    {/if}
  </ol>
</div>

<!-- A period: a day, the rest of a month, a month or a year. Its children are
     periods, or its events when it lists them directly. -->
{#snippet periodNode(period, nested)}
  {@const open = openNow(period.key)}
  {@const label = periodTitle(period, today)}
  {@const count = versionsText(period.count, period.partial)}
  <li class="hx-period" class:hx-top={!nested} class:hx-nested={nested} data-period={period.key}>
    <button
      type="button"
      class="hx-heading"
      data-nav
      aria-expanded={open}
      aria-label={m.history_row_tip({ what: label, detail: count })}
      onclick={() => setPeriod(period.key, !open)}
      onkeydown={(event) => arrow(event, open, (want) => setPeriod(period.key, want))}
    >
      <span class="hx-chev" class:hx-open={open} aria-hidden="true"><Icon name="chevron-right" /></span>
      <span class="hx-title">{label}</span>
      <span class="hx-count panel-meta">{count}</span>
    </button>
    {#if open}
      {#if period.events}
        <ol class="hx-events">
          {#each period.events as event (event.key)}
            {@render eventNode(event)}
          {/each}
        </ol>
      {:else if period.children}
        <ol class="hx-children">
          {#each period.children as child (child.key)}
            {@render periodNode(child, true)}
          {/each}
        </ol>
      {/if}
    {/if}
  </li>
{/snippet}

<!-- An event. One point is one row. More than one is a caret, which opens its
     versions, and a row that previews the newest of them. -->
{#snippet eventNode(event)}
  {@const first = event.points[0]}
  {@const multi = event.points.length > 1}
  {@const expanded = expandedEvents.has(event.key)}
  {@const holds = event.points.some((point) => point.sha === viewing)}
  {@const shown = countFor(event)}
  {@const left = Math.max(0, event.points.length - shown)}
  {#if !multi}
    <li class="hx-event" data-event={event.key} data-type={event.type} data-sha={first.sha}>
      {#if naming === first.sha}
        {@render nameField(first.label ? m.history_rename_bookmark() : m.history_name_bookmark())}
      {:else}
        <div class="hx-row" class:hx-here={viewing === first.sha}>
          <button
            type="button"
            class="hx-main"
            data-nav
            class:timeline-here={viewing === first.sha}
            class:hx-base={against === first.sha}
            aria-current={viewing === first.sha ? "true" : undefined}
            title={m.history_row_tip({ what: titleOf(event), detail: soloLine(event) })}
            aria-label={against === first.sha
            ? m.history_row_tip_base({ what: titleOf(event), detail: soloLine(event) })
            : m.history_row_tip({ what: titleOf(event), detail: soloLine(event) })}
            onclick={(click) => pick(click, first.sha)}
          >
            {@render eventBody(event, soloLine(event))}
          </button>
          {@render markTool(first)}
        </div>
      {/if}
    </li>
  {:else}
    <li class="hx-event" data-event={event.key} data-type={event.type}>
      <div class="hx-row">
        <button
          type="button"
          class="hx-caret-btn"
          aria-expanded={expanded}
          aria-label={m.history_show_versions({ count: event.points.length })}
          onclick={() => setEvent(event.key, !expanded)}
          onkeydown={(keyed) => arrow(keyed, expanded, (want) => setEvent(event.key, want))}
        >
          <span class="hx-chev" class:hx-open={expanded} aria-hidden="true"><Icon name="chevron-right" /></span>
        </button>
        <button
          type="button"
          class="hx-main"
          data-nav
          class:timeline-here={!expanded && holds}
          aria-current={!expanded && holds ? "true" : undefined}
          title={m.history_group_tip({ what: titleOf(event), detail: eventMeta(event) })}
          aria-label={m.history_group_label({ what: titleOf(event), range: range(event), count: event.points.length })}
          onclick={(click) => pick(click, first.sha)}
        >
          {@render eventBody(event, eventMeta(event))}
        </button>
      </div>
      {#if expanded}
        <ol class="hx-versions">
          {#each event.points.slice(0, shown) as point (point.sha)}
            {@render versionRow(point, event)}
          {/each}
          {#if left > 0}
            <li>
              <button type="button" class="hx-more" data-nav onclick={() => more(event)}>
                {m.history_more_versions({ count: Math.min(SHOW_MORE, left) })}
              </button>
            </li>
          {/if}
        </ol>
      {/if}
    </li>
  {/if}
{/snippet}

<!-- The icon, the title and the line below it. -->
{#snippet eventBody(event, detail)}
  <span class="hx-icon" aria-hidden="true"><Icon name={ICON[event.type]} filled={event.type === "saved"} /></span>
  <span class="hx-text">
    <span class="hx-line1">{titleOf(event)}</span>
    <span class="hx-line2 panel-meta">{detail}</span>
  </span>
{/snippet}

<!-- One version inside an expanded event. The whole row is the button. -->
{#snippet versionRow(point, event)}
  {@const here = viewing === point.sha}
  {@const who = editorOf(point)}
  {@const showWho = Boolean(who) && (event.type === "collaborative" || who !== event.authors[0])}
  {@const why = said(point)}
  {@const base = against === point.sha}
  <li class="hx-version" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField(m.history_name_bookmark())}
    {:else}
      <div class="hx-row" class:hx-here={here}>
        <button
          type="button"
          class="hx-version-main"
          class:timeline-here={here}
          class:hx-base={base}
          data-nav
          aria-current={here ? "true" : undefined}
          title={m.history_row_tip({ what: told(point), detail: stamp(point.at) || shortSha(point.sha) })}
          aria-label={[m.history_version_at({ time: clock(point.at) }), who ? m.history_by({ who }) : "", why, base ? m.history_comparison_base() : ""].filter(Boolean).join(", ")}
          onclick={(click) => pick(click, point.sha)}
        >
          <span class="hx-when">{clock(point.at)}</span>
          {#if showWho}<span class="hx-who panel-meta">{who}</span>{/if}
        </button>
        {@render markTool(point)}
      </div>
    {/if}
  </li>
{/snippet}

<!-- A saved version, in the saved view. Renaming it and removing its name
     need the right to change the document. -->
{#snippet savedRow(point)}
  <li class="hx-saved" data-sha={point.sha}>
    {#if naming === point.sha}
      {@render nameField(m.history_rename_bookmark())}
    {:else}
      <div class="hx-row" class:hx-here={viewing === point.sha}>
        <button
          type="button"
          class="hx-main"
          data-nav
          class:timeline-here={viewing === point.sha}
          aria-current={viewing === point.sha ? "true" : undefined}
          title={m.history_row_tip({ what: point.label, detail: savedWhen(point.at) })}
          aria-label={m.history_row_tip({ what: point.label, detail: savedWhen(point.at) })}
          onclick={(click) => pick(click, point.sha)}
        >
          <span class="hx-text">
            <span class="hx-line1">{point.label}</span>
            <span class="hx-line2 panel-meta">{savedWhen(point.at)}</span>
          </span>
        </button>
        <span class="hx-tools">
          {#if canEdit}
            <IconButton icon="pencil" tone="plain" size="btn-icon-sm" label={m.common_rename()}
                        onclick={() => startNaming(point)} />
            <IconButton icon="trash" tone="plain" size="btn-icon-sm" label={m.history_remove_saved()}
                        onclick={() => unbookmark(point)} />
          {/if}
        </span>
      </div>
    {/if}
  </li>
{/snippet}

<!-- The bookmark control on a version or a single event. It shows on hover,
     on keyboard focus, and on the row being read. A bookmark a reader cannot
     change is shown as a plain mark. -->
{#snippet markTool(point)}
  {#if canEdit}
    <span class="hx-tools">
      <IconButton icon="bookmark" tone="plain" size="btn-icon-sm" filled={Boolean(point.label)}
                  label={point.label ? m.history_rename_bookmark() : m.history_bookmark_version()}
                  onclick={() => startNaming(point)} />
    </span>
  {:else if point.label}
    <span class="hx-mark" aria-hidden="true"><Icon name="bookmark" filled /></span>
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
  /* One column, two levels of type, and no boxes. The only colour says which
     row is being read. */

  .timeline { position: relative; display: flex; flex-direction: column; overflow: hidden; container-type: inline-size; }
  .timeline :global(.panel-actions) { justify-content: flex-end; }
  .timeline :is(button):focus-visible { outline: 2px solid var(--color-brand); outline-offset: -2px; }

  /* The date popover is a small box under the header, not a calendar. */
  .hx-bar {
    position: relative;
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--spacing);
    margin: 0 var(--panel-padding) calc(var(--spacing) * 2);
  }
  .hx-popover {
    position: absolute;
    top: calc(100% + var(--spacing));
    right: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * 2);
    background: var(--color-sidebar);
    border: 1px solid var(--color-divider);
    border-radius: 4px;
  }
  .hx-popover .input { width: auto; font-size: var(--panel-meta-size); }

  /* The filter: two halves of one control, the pressed half tinted. */
  .hx-segment {
    display: flex;
    flex: 1 1 auto;
    min-width: 0;
    border: 1px solid var(--color-divider);
    border-radius: 4px;
    overflow: hidden;
  }
  .hx-segment button {
    flex: 1 1 0;
    min-width: 0;
    padding: var(--spacing);
    font-size: var(--panel-meta-size);
    color: var(--panel-muted);
    text-align: center;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hx-short { display: none; }
  @container (max-width: 280px) {
    .hx-long { display: none; }
    .hx-short { display: inline; }
  }
  .hx-segment button + button { border-left: 1px solid var(--color-divider); }
  .hx-segment button[aria-pressed="true"] { background: var(--color-row-selected); color: inherit; font-weight: 500; }
  .hx-segment button:hover:not([aria-pressed="true"]) { background: var(--color-row-hover); }
  .hx-note { margin: 0 var(--panel-padding) calc(var(--spacing) * 2); }

  /* The list takes what is left and scrolls on its own. */
  .hx-list {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    list-style: none;
    margin: 0;
    padding: 0 var(--panel-padding) var(--panel-padding);
  }
  .hx-list ol { list-style: none; margin: 0; padding: 0; }
  .hx-list li { min-width: 0; }
  .hx-list .input { width: 100%; margin-block: 2px; }

  .hx-status { padding: var(--spacing) calc(var(--spacing) * 2); font-size: var(--panel-font-size); }
  .hx-status .btn { margin-top: var(--spacing); }

  /* ----------------------------------------------------------- periods */

  .hx-top { margin-top: calc(var(--spacing) * 3); }
  .hx-nested { margin-left: 12px; }
  .hx-heading {
    display: flex;
    align-items: baseline;
    gap: calc(var(--spacing) * 2);
    width: 100%;
    min-width: 0;
    min-height: 2rem;
    padding: var(--spacing) calc(var(--spacing) * 2);
    border-radius: 4px;
    font-size: var(--panel-font-size);
    font-weight: 500;
    text-align: left;
  }
  .hx-heading:hover { background: var(--color-row-hover); }
  .hx-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hx-count { flex: none; margin-left: auto; white-space: nowrap; }
  .hx-chev {
    flex: none;
    align-self: center;
    display: inline-flex;
    width: 0.75rem;
    height: 0.75rem;
    color: var(--panel-muted);
    transition: transform 120ms ease;
  }
  .hx-chev :global(svg) { width: 100%; height: 100%; }
  .hx-open { transform: rotate(90deg); }
  .hx-events, .hx-children { padding-top: 2px; }

  /* ------------------------------------------------------------- rows */

  .hx-row {
    display: flex;
    align-items: stretch;
    min-width: 0;
    border-radius: 4px;
  }
  .hx-main {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    min-width: 0;
    min-height: 2.75rem;
    padding: var(--spacing) calc(var(--spacing) * 2);
    border-radius: 4px;
    text-align: left;
  }
  .hx-main:hover:not(.timeline-here) { background: var(--color-row-hover); }
  .hx-current .hx-main { min-height: 2.25rem; }
  .hx-icon {
    flex: none;
    display: inline-flex;
    width: 1rem;
    height: 1rem;
    color: var(--panel-muted);
  }
  .hx-icon :global(svg) { width: 100%; height: 100%; }
  .hx-text { display: flex; flex: 1 1 auto; flex-direction: column; min-width: 0; line-height: 1.3; }
  .hx-line1 {
    min-width: 0;
    font-size: var(--panel-font-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hx-line2 { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* The caret of an event with more than one version. */
  .hx-caret-btn {
    flex: none;
    display: grid;
    place-items: center;
    width: 1.25rem;
    min-height: 2.75rem;
    border-radius: 4px;
    color: var(--panel-muted);
  }
  .hx-caret-btn:hover { background: var(--color-row-hover); }

  /* The versions of an expanded event, under its title. */
  .hx-list .hx-versions { margin-left: 24px; }
  .hx-version-main {
    flex: 1 1 auto;
    display: flex;
    align-items: baseline;
    gap: calc(var(--spacing) * 2);
    min-width: 0;
    min-height: 2rem;
    padding: var(--spacing) calc(var(--spacing) * 2);
    border-radius: 4px;
    font-size: var(--panel-font-size);
    text-align: left;
  }
  .hx-version-main:hover:not(.timeline-here) { background: var(--color-row-hover); }
  .hx-when { flex: none; font-variant-numeric: tabular-nums; }
  .hx-who {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hx-version-main.timeline-here {
    font-weight: 500;
    box-shadow: inset 2px 0 var(--color-brand);
  }
  /* The base of a comparison: a thin dashed edge, not the selected ground. */
  .hx-base {
    box-shadow: inset 1px 0 var(--color-brand);
    outline: 1px dashed var(--color-brand);
    outline-offset: -1px;
  }

  /* Bookmark controls: on hover, on keyboard focus, or on the row being read. */
  /* Hidden tools take no room, so a narrow panel gives the whole row to the
     name. */
  .hx-tools {
    flex: none;
    display: none;
    align-items: center;
  }
  .hx-row:hover .hx-tools,
  .hx-row:focus-within .hx-tools,
  .hx-row.hx-here .hx-tools { display: flex; }
  .hx-mark {
    flex: none;
    display: inline-flex;
    align-items: center;
    width: 1rem;
    margin-inline: calc(var(--spacing) * 1.5);
    color: var(--color-link);
  }
  .hx-mark :global(svg) { width: 100%; height: 100%; }

  .hx-more {
    display: block;
    padding: var(--spacing) calc(var(--spacing) * 2);
    border-radius: 4px;
    font-size: var(--panel-meta-size);
    color: var(--color-link);
    text-align: left;
  }
  .hx-more:hover { text-decoration: underline; }

  /* Older history, loaded on demand. */
  .hx-older {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--spacing);
    padding: var(--spacing) calc(var(--spacing) * 2);
  }
</style>
