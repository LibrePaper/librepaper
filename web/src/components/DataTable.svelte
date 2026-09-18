<script>
  // Every table in the project management pages, once.
  //
  // The listing used to build its own table on every page it has: the header
  // row, the sort buttons, the widths and the empty state were written out
  // again inside ternaries on `place`, and the three places disagreed about
  // all four. What differs between them is which columns there are, which is
  // data; the table around those columns is not, and it lives here.
  //
  // The columns are a list of `{ key, label, width, min, sortable }`. A
  // caller supplies the cells as one snippet and switches on the column's
  // key, so a new column is a line in the list and a branch in the snippet
  // rather than a new table.
  //
  // Widths are the reader's, not ours. A column can be dragged by the edge of
  // its heading and stays where it was put, per table, across reloads: the
  // one thing a listing of other people's work cannot know in advance is
  // which column the person reading it came to read.

  let {
    // Which table this is, so that a width dragged here comes back here and
    // not on the next page that happens to have an Owner column.
    id = "",
    columns = [],
    rows = [],
    rowKey = (row) => row.id,
    rowClass = () => "",
    label = "",
    // The column being sorted on, and which way. Sorting stays the caller's:
    // it owns the rows, and the order they arrive in is the order shown.
    sortBy = "",
    ascending = false,
    onsort = null,
    className = "",
    // What goes in a heading beyond its label (the select-all box), in a
    // cell, and in place of the whole body when there is nothing to show.
    head = null,
    cell,
    empty = null,
  } = $props();

  const STORE = "librepaper:table-widths";
  const FLOOR = 48;

  // The widths this table has been given, by column key, in pixels. Absent
  // means "whatever the column asked for", which is the common case: a table
  // nobody has dragged is laid out by its own defaults.
  let widths = $state({});
  let dragging = $state("");
  let table = $state(null);

  function stored() {
    try {
      return JSON.parse(localStorage.getItem(STORE) || "{}")[id] || {};
    } catch {
      return {};
    }
  }

  function remember() {
    try {
      const all = JSON.parse(localStorage.getItem(STORE) || "{}");
      all[id] = widths;
      localStorage.setItem(STORE, JSON.stringify(all));
    } catch {
      // A browser that refuses storage still gets to drag a column; it just
      // does not get to keep it.
    }
  }

  // The widths belong to the table, so changing which table this is brings
  // its own back.
  $effect(() => {
    widths = stored();
  });

  const floor = (column) => Math.max(FLOOR, column.min || FLOOR);
  const sized = (column) => widths[column.key] ?? column.width ?? null;
  // A table whose columns have been pinned wider than the page scrolls rather
  // than squeezing them back to where they were.
  const least = $derived(
    columns.reduce((total, column) => total + (widths[column.key] ?? column.min ?? 0), 0),
  );

  /// Drag from the edge of a heading.
  ///
  /// Every column is pinned at the width it currently has before the one
  /// being dragged moves, so that a table laid out by proportion does not
  /// redistribute the other five columns the moment one of them is touched.
  function grab(event, column) {
    if (event.button !== 0) return;
    event.preventDefault();
    const heads = table?.querySelectorAll("th[data-column]") || [];
    const pinned = { ...widths };
    for (const th of heads) {
      pinned[th.dataset.column] = Math.round(th.getBoundingClientRect().width);
    }
    widths = pinned;
    const from = event.clientX;
    const start = pinned[column.key];
    dragging = column.key;
    const move = (moved) => {
      widths = {
        ...widths,
        [column.key]: Math.max(floor(column), Math.round(start + moved.clientX - from)),
      };
    };
    const drop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", drop);
      window.removeEventListener("pointercancel", drop);
      dragging = "";
      remember();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", drop);
    window.addEventListener("pointercancel", drop);
  }

  // Back to the width the column asked for. The same gesture every resizable
  // thing has, and the only way back once a column has been dragged to
  // nothing.
  function reset(column) {
    const next = { ...widths };
    delete next[column.key];
    widths = next;
    remember();
  }

  /// The keyboard's version of the drag. A grip is focusable and is a
  /// separator, which is what a screen reader is told it is; arrows move it,
  /// and Home puts the column back.
  function nudge(event, column) {
    const steps = { ArrowLeft: -1, ArrowRight: 1 };
    if (event.key === "Home" || event.key === "Enter") {
      event.preventDefault();
      reset(column);
      return;
    }
    if (!(event.key in steps)) return;
    event.preventDefault();
    const th = table?.querySelector(`th[data-column="${column.key}"]`);
    const now = widths[column.key] ?? Math.round(th?.getBoundingClientRect().width || 0);
    const by = steps[event.key] * (event.shiftKey ? 4 : 16);
    widths = { ...widths, [column.key]: Math.max(floor(column), now + by) };
    remember();
  }

  const aria = (column) =>
    column.sortable && sortBy === column.key ? (ascending ? "ascending" : "descending") : undefined;
</script>

<div class="table-wrap" class:table-dragging={dragging}>
  <table
    bind:this={table}
    class="table data-table {className}"
    style:min-width={least ? `${least}px` : undefined}
    aria-label={label || undefined}
  >
    <!-- The widths live here rather than on the cells: one column, one
         declaration, and a table that does not have to be measured to be laid
         out. -->
    <colgroup>
      {#each columns as column (column.key)}
        <col style:width={sized(column) === null ? undefined : `${sized(column)}px`} />
      {/each}
    </colgroup>
    <thead>
      <tr>
        {#each columns as column (column.key)}
          <th
            data-column={column.key}
            class={column.class || ""}
            style:text-align={column.align || undefined}
            aria-sort={aria(column)}
          >
            <span class="th-inner">
              {#if column.sortable && onsort}
                <button
                  type="button"
                  class="th-sort"
                  class:th-sorted={sortBy === column.key}
                  onclick={() => onsort(column.key)}
                >
                  {column.label}
                  {#if sortBy === column.key}<span aria-hidden="true">{ascending ? "▲" : "▼"}</span>{/if}
                </button>
              {:else if column.label}
                <span class="th-label">{column.label}</span>
              {/if}
              {#if head}{@render head(column)}{/if}
            </span>
            <!-- The edge itself is the handle. Not an icon: a column edge is
                 where everybody already reaches for it, and a control drawn
                 there would be one more mark in a header that is mostly
                 whitespace on purpose. -->
            {#if column.resizable !== false}
              <!-- A focusable separator is ARIA's window splitter, which is
                   exactly what this is; the lint's list of interactive roles
                   does not carry that exception, so it is stated here. -->
              <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <span
                class="col-grip"
                class:col-gripping={dragging === column.key}
                role="separator"
                aria-orientation="vertical"
                aria-label="Resize the {column.label || column.key} column"
                tabindex="0"
                onpointerdown={(event) => grab(event, column)}
                ondblclick={() => reset(column)}
                onkeydown={(event) => nudge(event, column)}
              ></span>
            {/if}
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as row (rowKey(row))}
        <tr class={rowClass(row)}>
          {#each columns as column (column.key)}
            <td class={column.class || ""} style:text-align={column.align || undefined}>
              {@render cell(column, row)}
            </td>
          {/each}
        </tr>
      {:else}
        <tr>
          <td colspan={columns.length} class="table-empty">
            {#if empty}{@render empty()}{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  /* The table scrolls sideways rather than squeezing: a column dragged wide
     was dragged wide on purpose. */
  .table-wrap {
    min-width: 0;
    overflow-x: auto;
  }
  /* Fixed layout, because the widths are now something the reader sets. An
     auto table re-measures itself from its contents and would undo every
     drag on the next keystroke in the search box. */
  .data-table {
    width: 100%;
    table-layout: fixed;
  }
  /* While a column is being dragged the pointer keeps the resize cursor
     wherever it goes, and nothing on the page takes a selection. */
  .table-dragging {
    cursor: col-resize;
    user-select: none;
  }

  .data-table th {
    position: relative;
  }
  .th-inner {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing));
    min-width: 0;
  }
  .th-sort, .th-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .th-sort { cursor: pointer; }
  .th-sorted { color: var(--color-primary-500); font-weight: 600; }

  /* A hair of ground either side of the edge, so that the handle can be hit
     without aiming at a 1px line. It shows itself under the pointer and
     while it is being dragged, and not otherwise. */
  .col-grip {
    position: absolute;
    top: 0;
    right: -3px;
    z-index: 1;
    width: 7px;
    height: 100%;
    cursor: col-resize;
    touch-action: none;
  }
  .col-grip::after {
    content: "";
    position: absolute;
    top: 15%;
    left: 3px;
    width: 1px;
    height: 70%;
    background: var(--color-surface-400-600);
    opacity: 0;
    transition: opacity 120ms ease;
  }
  .col-grip:hover::after,
  .col-grip:focus-visible::after,
  .col-gripping::after { opacity: 1; }
  .col-grip:focus-visible::after { background: var(--color-primary-500); width: 2px; }

  /* Every cell keeps to its column. Without this a long title widens the
     column it is in whatever the colgroup says. */
  .data-table :global(td) {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .table-empty {
    height: 12rem;
    color: var(--color-surface-600-400);
    text-align: center;
    vertical-align: middle;
  }
</style>
