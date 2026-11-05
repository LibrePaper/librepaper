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
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";
  import { day as dayOf } from "../../lib/dates.js";
  import { shortSha } from "../../lib/history.js";
  import { buildPeriods, nearestPeriod, pathTo, periodTitle } from "../../lib/history-groups.js";
  import { CURRENT } from "../../lib/history-endpoints.js";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import PanelTabs from "../PanelTabs.svelte";

  // The two ends of a comparison, each a version's sha or CURRENT for the live
  // document. They are marked in the list so the reader does not need the colour.
  // `selecting` is the endpoint being chosen, "normal" when none is. While it is
  // not, the list says which end to pick and every eligible row says so too.
  let {
    labels = [],
    viewing = null,
    older = "",
    newer = "",
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
    selecting = "normal",
    onview,
    oncancelselect,
    onname,
    onretry,
    onloadmore,
    onloaduntil,
    onloadsaved,
    // Authorship: `{rows, since, loading, error, noFile}` or null, the row
    // key whose text is isolated, and the two callbacks that report to the
    // editor. `onauthorsview` says whether the authors tab is on screen.
    authors = null,
    authorsOnly = null,
    onauthorsview = undefined,
    onisolate = undefined,
  } = $props();

  const SHOW_FIRST = 5;
  const SHOW_MORE = 50;

  let timezone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  const today = $derived(dayOf(new Date(), timezone));
  const periods = $derived(buildPeriods(labels, { timeZone: timezone, today, partial: !complete }));

  // Which tab is showing (all, saved or authors), and whether the date popover
  // is open.
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
  // mutated, so the template sees the change. Multi-version sessions start
  // closed, except the one holding the live document, which starts open.
  let toggled = $state(new Set());
  let toggledEvents = $state(new Set());
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
  // The session holding the live document starts open; the others start closed.
  const eventOpen = (key) => toggledEvents.has(key) !== (key === newestEventKey);
  function setEvent(key, want) {
    if (eventOpen(key) !== want) toggledEvents = flipped(toggledEvents, key);
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
    restore: m.history_restored_version(),
    created: m.history_created(),
  };
  // Editing and collaborative sessions have no title of their own: their time
  // range says what they are. The other events carry information in theirs.
  const ranged = (event) => event.type === "editing" || event.type === "collaborative";
  // A push is named by its message, and says who pushed it: the author the
  // caller gave, or the account behind the token.
  const titleOf = (event) => {
    if (event.type === "saved") return event.points[0].label;
    if (event.type === "push") return event.authors.length ? m.history_pushed_by({ author: event.authors[0] }) : m.history_push();
    return TITLE[event.type];
  };
  const primaryOf = (event) => (ranged(event) ? range(event) : titleOf(event));

  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  // Who made a version, where the manifest can say. A machine save names
  // nobody, and neither does an identifier that is not a name.
  const editorOf = (point) => {
    if (!point?.by || point.by === "system" || uuid.test(point.by)) return "";
    return point.by;
  };

  // The newest version overall stands for the live document: its row says
  // Current, and choosing it views the live document, not that stored version.
  // Periods and events are newest first.
  const newestOf = (list) => {
    const first = list[0];
    if (!first) return null;
    return first.events ? first.events[0]?.points[0] ?? null : newestOf(first.children ?? []);
  };
  const latestSha = $derived(newestOf(periods)?.sha ?? "");
  // The same walk, stopping at the event that holds the newest version.
  const newestEventOf = (list) => {
    const first = list[0];
    if (!first) return null;
    return first.events ? first.events[0] ?? null : newestEventOf(first.children ?? []);
  };
  const newestEventKey = $derived(newestEventOf(periods)?.key ?? "");
  const isLive = (sha) => latestSha !== "" && sha === latestSha;
  // What a click on the row of this version asks for, and whether it is the
  // one being read.
  const pickOf = (sha) => (isLive(sha) ? "" : sha);
  const hereAt = (sha) => (isLive(sha) ? !viewing || viewing === sha : viewing === sha);
  // Naming the live document is "current", and it happens on the newest row.
  const namingAt = (sha) => naming === sha || (isLive(sha) && naming === "current");

  // The endpoint a row carries, as "older", "newer" or nothing. The live
  // document has no sha of its own, and an empty `older` must not tag it.
  const endpointKind = (sha) => {
    if (!sha) return "";
    if (sha === older) return "older";
    if (sha === newer || (isLive(sha) && newer === CURRENT)) return "newer";
    return "";
  };
  const endpointWord = (sha) => {
    const kind = endpointKind(sha);
    return kind === "older" ? m.history_older_tag() : kind === "newer" ? m.history_newer_tag() : "";
  };
  const currentEnd = $derived(newer === CURRENT ? "newer" : "");
  const currentTag = $derived(newer === CURRENT ? m.history_newer_tag() : "");
  // A row's description for the tooltip and the screen reader: the words the
  // eye gets from the layout, Current and the endpoint among them.
  const describe = (what, detail, sha) =>
    [m.history_row_tip({ what, detail }), isLive(sha) ? m.history_current_label() : "", endpointWord(sha)]
      .filter(Boolean).join(", ");
  const tipWith = (what, detail, endpoint) =>
    endpoint ? m.history_row_tip_endpoint({ what, detail, endpoint }) : m.history_row_tip({ what, detail });

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
  // The second line of a session. The range is already the first line of an
  // editing or collaborative one.
  const eventMeta = (event) =>
    [
      ranged(event) ? "" : range(event),
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

  // Authorship: a share as a percentage, a date in the reader's timezone, and
  // the line under an author's name.
  const pct = (value) => new Intl.NumberFormat(locale(), { style: "percent", maximumFractionDigits: 0 }).format(value);
  const shortDate = (value) => new Intl.DateTimeFormat(locale(), { dateStyle: "medium", timeZone: timezone }).format(new Date(value));
  const authorLine = (row) => [
    m.history_authors_share({ share: pct(row.share) }),
    row.last ? m.history_authors_last_edit({ date: shortDate(row.last) }) : "",
  ].filter(Boolean).join(" · ");

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

  // The strip's one handler. The all and saved tabs keep their own logic.
  function choose(value) {
    if (value === "all") showAll();
    else if (value === "saved") showSaved();
    else {
      mode = "authors";
      dateOpen = false;
      note = "";
    }
  }

  // The editor is told while the authors tab is on screen, and told again
  // when the tab is left or the panel goes away.
  $effect(() => {
    if (mode !== "authors") return;
    untrack(() => onauthorsview?.(true));
    return () => untrack(() => onauthorsview?.(false));
  });

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

  // ArrowLeft on a version goes up to the session it belongs to.
  function toSession(event) {
    if (event.key !== "ArrowLeft") return;
    const parent = event.currentTarget.closest("li.hx-event")?.querySelector("button.hx-session");
    if (!parent) return;
    event.preventDefault();
    parent.focus();
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

<!-- The strip is PanelTabs', the one the other panels wear. The calendar
     control rides on its row and belongs to the all view. -->
<!-- Escape leaves selection mode from anywhere in the panel. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="panel timeline panel-tabbed" aria-label={m.common_history()} onkeydown={(event) => {
  if (event.key === "Escape" && selecting !== "normal") {
    event.preventDefault();
    oncancelselect?.();
  }
}}>
  {#snippet dateControl()}
    <span class="hx-date-wrap" bind:this={dateWrap}>
      <IconButton icon="calendar" tone="plain" size="btn-icon-sm" label={m.history_go_to_date()}
                  expanded={dateOpen} controls="hx-date" onclick={toggleDate} />
      {#if dateOpen}
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
    </span>
  {/snippet}

  <!-- The list is drawn once, by whichever of the all and saved panes is showing. -->
  {#snippet timelineList()}
    {#if note}<p class="panel-meta hx-note" role="status">{note}</p>{/if}

    {#if selecting !== "normal"}
      <div class="hx-select" role="status">
        <span>{selecting === "select-older" ? m.history_select_older_prompt() : m.history_select_newer_prompt()}</span>
        <button type="button" class="btn btn-sm" onclick={() => oncancelselect?.()}>{m.common_cancel()}</button>
      </div>
    {/if}

    <!-- The handler only moves focus between the buttons inside the list. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <ol class="hx-list" class:hx-selecting={selecting !== "normal"} bind:this={listEl} onkeydown={navigate}>
      {#if mode === "all"}
        <!-- The live document is the newest version's row. Until there is one,
             a compact row keeps it reachable. -->
        {#if !labels.length}
          <li class="hx-current hx-row" class:hx-here={!viewing}>
            {#if naming === "current"}
              {@render nameField(m.history_name_bookmark())}
            {:else}
              <button
                type="button"
                class="hx-single"
                data-nav
                class:timeline-here={!viewing}
                aria-current={!viewing ? "true" : undefined}
                aria-label={[m.history_current_version(), currentTag].filter(Boolean).join(", ")}
                onclick={() => onview?.("")}
              >
                <span class="hx-ell">{m.history_current_version()}</span>
                {@render endMark(currentEnd)}
              </button>
              {#if canEdit && !viewing}{@render liveTool()}{/if}
            {/if}
          </li>
        {/if}

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
  {/snippet}

  <PanelTabs id="history" label={m.history_views_label()}
             tabs={[
               { id: "all", label: m.history_all_short() },
               { id: "saved", label: m.history_saved_short() },
               { id: "authors", label: m.history_authors() },
             ]}
             value={mode} onchange={choose}
             actions={mode === "all" ? dateControl : undefined}>
    <Tabs.Content value="all">{#if mode === "all"}{@render timelineList()}{/if}</Tabs.Content>
    <Tabs.Content value="saved">{#if mode === "saved"}{@render timelineList()}{/if}</Tabs.Content>
    <Tabs.Content value="authors">
      <div class="hx-authors">
        {#if authors?.loading}
          <p class="panel-muted" role="status">{m.history_authors_loading()}</p>
        {:else if authors?.error}
          <p class="lp-text-error" role="status">{authors.error}</p>
        {:else if authors?.noFile}
          <p class="panel-muted">{m.history_authors_no_file()}</p>
        {:else if authors}
          <ul class="hx-author-list">
            {#each authors.rows as row (row.key)}
              {@const pressed = authorsOnly === row.key}
              {@const named = pressed ? m.history_authors_show_all() : m.history_authors_isolate({ name: row.name })}
              <li>
                <button
                  type="button"
                  class="hx-main hx-author"
                  class:timeline-here={pressed}
                  aria-pressed={pressed}
                  title={named}
                  onclick={() => onisolate?.(pressed ? null : row.key)}
                >
                  <span class="hx-swatch" class:hx-swatch-unknown={row.unknown}
                        style:background={row.unknown ? undefined : row.colour} aria-hidden="true"></span>
                  <span class="hx-text">
                    <span class="hx-line1">{row.name}</span>
                    <span class="hx-line2 panel-meta">{authorLine(row)}</span>
                  </span>
                </button>
              </li>
            {/each}
          </ul>
          <p class="panel-meta hx-foot">
            {authors.since ? m.history_authors_since({ date: shortDate(authors.since) }) : m.history_authors_since_none()}
          </p>
        {/if}
      </div>
    </Tabs.Content>
  </PanelTabs>
</section>

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
      <span class="hx-chev" aria-hidden="true"><Icon name={open ? "chevron-down" : "chevron-right"} /></span>
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

<!-- An event. One point is one selectable row. More than one is a session: a
     row that only opens and closes its versions. -->
{#snippet eventNode(event)}
  {@const multi = event.points.length > 1}
  {@const expanded = eventOpen(event.key)}
  {@const holds = event.points.some((point) => hereAt(point.sha))}
  {@const shown = countFor(event)}
  {@const left = Math.max(0, event.points.length - shown)}
  {#if !multi}
    {@render singleRow(event)}
  {:else}
    <li class="hx-event" data-event={event.key} data-type={event.type}>
      <div class="hx-row">
        <button
          type="button"
          class="hx-session"
          class:hx-holds={holds && !expanded}
          data-nav
          aria-expanded={expanded}
          title={m.history_row_tip({ what: primaryOf(event), detail: eventMeta(event) })}
          aria-label={m.history_row_tip({ what: primaryOf(event), detail: eventMeta(event) })}
          onclick={() => setEvent(event.key, !expanded)}
          onkeydown={(keyed) => arrow(keyed, expanded, (want) => setEvent(event.key, want))}
        >
          <span class="hx-chev" aria-hidden="true"><Icon name={expanded ? "chevron-down" : "chevron-right"} /></span>
          {@render eventText(event, primaryOf(event), eventMeta(event))}
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

<!-- An event of one version. It is not a disclosure, so it is the row of that
     version, at the session's indent. -->
{#snippet singleRow(event)}
  {@const first = event.points[0]}
  {@const plain = event.type === "editing"}
  {@const who = event.authors[0] ?? ""}
  {@const here = hereAt(first.sha)}
  {@const what = plain ? m.history_version_at({ time: clock(first.at) }) : titleOf(event)}
  {@const detail = plain ? who : soloLine(event)}
  <li class="hx-event" data-event={event.key} data-type={event.type} data-sha={first.sha}>
    {#if namingAt(first.sha)}
      {@render nameField(first.label && naming !== "current" ? m.history_rename_bookmark() : m.history_name_bookmark())}
    {:else}
      <div class="hx-row" class:hx-here={here}>
        <button
          type="button"
          class="hx-single"
          data-nav
          class:timeline-here={here}
          aria-current={here ? "true" : undefined}
          title={describe(what, detail, first.sha)}
          aria-label={describe(what, detail, first.sha)}
          onclick={() => onview?.(pickOf(first.sha))}
        >
          {#if plain}
            <span class="hx-when">{clock(first.at)}</span>
            {#if who}<span class="hx-who panel-meta" title={who}>{who}</span>{/if}
          {:else}
            {@render eventText(event, titleOf(event), soloLine(event))}
          {/if}
          {@render trail(first.sha)}
        </button>
        {@render rowTools(first)}
      </div>
    {/if}
  </li>
{/snippet}

<!-- The two lines of an event: what it is, and when and by whom. A bookmark
     is marked, and a session of several people says so. -->
{#snippet eventText(event, primary, detail)}
  <span class="hx-text">
    <span class="hx-line1 hx-ev1">
      {#if event.type === "saved"}<span class="hx-ico" aria-hidden="true"><Icon name="bookmark" filled /></span>{/if}
      <span class="hx-ell">{primary}</span>
    </span>
    <span class="hx-line2 panel-meta hx-ev2">
      {#if event.type === "collaborative"}<span class="hx-ico" aria-hidden="true"><Icon name="users" /></span>{/if}
      <span class="hx-ell">{detail}</span>
    </span>
  </span>
{/snippet}

<!-- The right edge of a row is one slot. It holds the endpoint word when the row
     is an endpoint, otherwise Current on the newest version, otherwise nothing.
     The newest version as the newer end shows Current alone. -->
{#snippet trail(sha)}
  {@const end = endpointKind(sha)}
  {@render endMark(end === "older" ? "older" : isLive(sha) ? "current" : end)}
{/snippet}

<!-- The one right-hand slot of a row: Current, or a comparison endpoint word.
     The slot is always there, pushed to the right edge at a fixed minimum width,
     so a row never shifts or wraps when it is marked. -->
{#snippet endMark(kind)}
  <span class="hx-end panel-meta" class:hx-now={kind === "current"} aria-hidden="true">{kind === "current" ? m.history_current_label() : kind === "newer" ? m.history_newer_tag() : kind === "older" ? m.history_older_tag() : ""}</span>
{/snippet}

<!-- One version inside an expanded session. The whole row is the button. -->
{#snippet versionRow(point, event)}
  {@const here = hereAt(point.sha)}
  {@const who = editorOf(point)}
  {@const showWho = Boolean(who) && (event.type === "collaborative" || who !== event.authors[0])}
  {@const why = said(point)}
  <li class="hx-version" data-sha={point.sha}>
    {#if namingAt(point.sha)}
      {@render nameField(m.history_name_bookmark())}
    {:else}
      <div class="hx-row" class:hx-here={here}>
        <button
          type="button"
          class="hx-version-main"
          class:timeline-here={here}
          data-nav
          aria-current={here ? "true" : undefined}
          title={m.history_row_tip({ what: told(point), detail: stamp(point.at) || shortSha(point.sha) })}
          aria-label={[m.history_version_at({ time: clock(point.at) }), who ? m.history_by({ who }) : "", why, isLive(point.sha) ? m.history_current_label() : "", endpointWord(point.sha)].filter(Boolean).join(", ")}
          onclick={() => onview?.(pickOf(point.sha))}
          onkeydown={toSession}
        >
          <span class="hx-when">{clock(point.at)}</span>
          {#if showWho}<span class="hx-who panel-meta" title={who}>{who}</span>{/if}
          {@render trail(point.sha)}
        </button>
        {@render rowTools(point)}
      </div>
    {/if}
  </li>
{/snippet}

<!-- The tools of a version's row. On the newest one the bookmark is the live
     document's, while the live document is the one being read. -->
{#snippet rowTools(point)}
  {#if isLive(point.sha) && canEdit && !viewing}{@render liveTool()}{:else}{@render markTool(point)}{/if}
{/snippet}

{#snippet liveTool()}
  <span class="hx-tools">
    <IconButton icon="bookmark" tone="plain" size="btn-icon-sm"
                label={m.history_bookmark_current()}
                onclick={() => startNaming({ sha: "current", label: "" })} />
  </span>
{/snippet}

<!-- A saved version, in the saved view. Renaming it and removing its name
     need the right to change the document. -->
{#snippet savedRow(point)}
  {@const end = endpointWord(point.sha)}
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
          aria-label={tipWith(point.label, savedWhen(point.at), end)}
          onclick={() => onview?.(point.sha)}
        >
          <span class="hx-text">
            <span class="hx-line1">{point.label}</span>
            <span class="hx-line2 panel-meta">{savedWhen(point.at)}</span>
          </span>
          {@render endMark(endpointKind(point.sha))}
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
  /* One column, three levels: dates, sessions and versions. Each level's row
     sets its own left padding from the list's edge (0, 20 and 44px), so
     nothing accumulates. The only colour says which row is being read. */

  .timeline { position: relative; display: flex; flex-direction: column; overflow: hidden; container-type: inline-size; }
  .timeline :is(button):focus-visible { outline: 2px solid var(--color-brand); outline-offset: -2px; }

  /* The date popover is a small box under the header, not a calendar. */
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

  /* The calendar control sits on the strip's row, and its popover hangs from it. */
  .hx-date-wrap { position: relative; display: inline-flex; }
  .hx-note { margin: 0 0 calc(var(--spacing) * 2); }
  .hx-foot { margin: calc(var(--spacing) * 3) 0 0; }

  /* Authors: the rows of the history, each with its author's colour. */
  .hx-authors { flex: 1 1 auto; min-height: 0; overflow-y: auto; overflow-x: hidden; }
  .hx-author { width: 100%; }
  .hx-author-list { list-style: none; margin: 0; padding: 0; }
  .hx-swatch {
    flex: none;
    width: 0.875rem;
    height: 0.875rem;
    border-radius: 3px;
    background: var(--color-divider);
  }
  .hx-swatch-unknown {
    background: repeating-linear-gradient(45deg, var(--color-divider) 0 2px, transparent 2px 5px);
  }

  /* The list takes what is left and scrolls on its own. */
  .hx-list {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    list-style: none;
    margin: 0;
    padding: 0 4px;
    /* The session chevron and the gap after it. A single-version row steps in
       by the same amount, so its text lines up with a session's label. */
    --hx-chev: 1rem;
    --hx-gap: 8px;
  }
  .hx-list ol { list-style: none; margin: 0; padding: 0; }
  .hx-list li { min-width: 0; }
  .hx-list .input { width: 100%; margin-block: 2px; }

  .hx-status { padding: var(--spacing) calc(var(--spacing) * 2); font-size: var(--panel-font-size); }
  .hx-status .btn { margin-top: var(--spacing); }

  /* ----------------------------------------------------------- periods */

  .hx-top { margin-top: 8px; }
  .hx-top ~ .hx-top { margin-top: 18px; }
  /* A period inside a year or a month steps in a little, for its heading and
     its rows alike. */
  .hx-nested { --hx-shift: 12px; margin-top: 4px; }
  .hx-heading {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    min-width: 0;
    min-height: 2rem;
    padding: 6px 10px 6px var(--hx-shift, 0px);
    border-radius: 5px;
    font-size: var(--panel-font-size);
    font-weight: 600;
    text-align: left;
  }
  .hx-heading:hover { background: var(--color-row-hover); }
  .hx-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hx-count { flex: none; margin-left: auto; font-weight: 400; white-space: nowrap; }
  .hx-chev {
    flex: none;
    display: inline-flex;
    width: var(--hx-chev);
    height: var(--hx-chev);
    color: var(--panel-muted);
  }
  .hx-chev :global(svg) { width: 100%; height: 100%; }
  .hx-events, .hx-children { padding-top: 2px; }

  /* ------------------------------------------------------------- rows */

  .hx-row {
    display: flex;
    align-items: stretch;
    min-width: 0;
    border-radius: 5px;
  }
  /* Saved and author rows. */
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
  .hx-text { display: flex; flex: 1 1 auto; flex-direction: column; min-width: 0; line-height: 1.3; }
  .hx-line1 {
    min-width: 0;
    font-size: var(--panel-font-size);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hx-line2 { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Sessions and the rows beneath them: 13px type, compact, indented from the
     list's edge. A session only opens and closes. */
  .hx-session,
  .hx-single,
  .hx-version-main {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    gap: var(--hx-gap);
    min-width: 0;
    padding-block: 6px;
    padding-right: 10px;
    border-radius: 5px;
    font-size: 0.8125rem;
    text-align: left;
  }
  .hx-session { padding-left: calc(var(--hx-shift, 0px) + 20px); font-weight: 500; }
  /* No chevron, but the same step as a session's label: indent, chevron, gap. */
  .hx-single {
    min-height: 2.125rem;
    padding-left: calc(var(--hx-shift, 0px) + 20px + var(--hx-chev) + var(--hx-gap));
  }
  .hx-version-main { min-height: 2.125rem; padding-left: calc(var(--hx-shift, 0px) + 44px); }
  .hx-session:hover,
  .hx-single:hover:not(.timeline-here),
  .hx-version-main:hover:not(.timeline-here) { background: var(--color-row-hover); }
  /* The selected row: a quiet background, an accent on its edge and a
     heavier time. The global class is overridden here. */
  .hx-single.timeline-here,
  .hx-version-main.timeline-here { border-radius: 5px; box-shadow: inset 2px 0 var(--color-brand); }
  .timeline-here .hx-when { font-weight: 600; }
  /* A closed session holding the version being read: the selected row's edge
     accent, without its background, and a heavier label. */
  .hx-holds { box-shadow: inset 2px 0 var(--color-brand); }
  .hx-holds .hx-ev1 { font-weight: 600; }
  .hx-ev1, .hx-ev2 { display: flex; align-items: center; gap: 4px; }
  .hx-ev1 { font-size: 0.8125rem; }
  .hx-ev2 { font-size: var(--panel-meta-size); }
  .hx-single > .hx-ell { flex: 1 1 auto; }
  .hx-ell { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hx-ico { flex: none; display: inline-flex; width: 0.75rem; height: 0.75rem; color: var(--panel-muted); }
  .hx-ico :global(svg) { width: 100%; height: 100%; }
  .hx-when { flex: none; font-weight: 500; font-variant-numeric: tabular-nums; }
  .hx-who {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The row's one right-hand slot: Current or an endpoint word, pushed to the
     right edge. It keeps its minimum width whether or not a word is in it, and
     the word never wraps. */
  .hx-end {
    flex: none;
    box-sizing: border-box;
    min-width: 3.5rem;
    margin-left: auto;
    overflow: hidden;
    text-align: right;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* While selecting an endpoint, every version row says it can be chosen. Layout
     does not move; the outline is drawn inside the row. */
  .hx-selecting .hx-main:hover,
  .hx-selecting .hx-single:hover,
  .hx-selecting .hx-version-main:hover {
    cursor: pointer;
    outline: 1px solid var(--color-brand);
    outline-offset: -1px;
  }
  /* The prompt above the list, on one line when it fits. */
  .hx-select {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--spacing);
    margin: 0 var(--panel-padding) calc(var(--spacing) * 2);
    padding: var(--spacing) calc(var(--spacing) * 2);
    border: 1px solid var(--color-brand);
    border-radius: 4px;
    font-size: var(--panel-meta-size);
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
    padding: var(--spacing) 10px var(--spacing) calc(var(--hx-shift, 0px) + 44px);
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
  /* Phones have no hover, so the bookmark, rename and remove controls stay on
     their rows, and every row and disclosure is a finger's height. */
  @media (max-width: 767px) {
    .hx-tools { display: flex; }
    .hx-tools :global(.icon-control) { min-width: 2.75rem; min-height: 2.75rem; }
    .hx-heading,
    .hx-session,
    .hx-single,
    .hx-version-main,
    .hx-main { min-height: 2.75rem; }
    .hx-more { display: flex; align-items: center; min-height: 2.75rem; }
    .hx-list .input,
    .hx-popover .input { font-size: 1rem; }
  }
</style>
