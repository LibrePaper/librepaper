<script>
  // The controls for a paged document: which page, how large it is drawn,
  // what the pointer does on it, and a search through its text.
  //
  // These used to be a toolbar inside the PDF frame, drawn in the operating
  // system's own button colours, which made the paged preview look like a
  // different application from the flowing one an inch to its left. They are
  // ordinary controls in a slim band across the top of the preview pane now,
  // above the frame rather than over it, so they never cover a page. A format
  // with no controls renders nothing here and the band costs it no height.
  //
  // Nothing is decided here: the frame owns the page it is on, the scale it
  // actually drew at and every other fact in `view`, and says so over the
  // channel. This draws what it was told and sends back what was asked for.
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import { tick } from "svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import MenuIconButton from "./MenuIconButton.svelte";
  import IconButton from "./IconButton.svelte";

  // `view` is the whole validated viewer-state. `send` puts a message on the
  // channel to the frame. `ondownload` is null when there is nothing to save
  // yet, which hides the item rather than greying it: the band has no room to
  // explain why. `findAsk` counts the times the frame asked for the search
  // row (Ctrl or Cmd+F pressed inside it), so a second press focuses the field
  // again even though the row is already open.
  let { view, tool = "select", busy = false, dark = false, findAsk = 0, send, onmode, ontool, ondownload = null, onpresent } = $props();

  // pdf.js's own steps, and its own ceiling and floor. A zoom button moves by
  // a tenth of the *drawn* scale rather than by a step of this list: the list
  // is where somebody lands when they pick a percentage, not the path they
  // take when they press `+` twice.
  const STEPS = [50, 75, 100, 125, 150, 200, 300, 400];
  const MIN = 0.1;
  const MAX = 10;

  // The scale the frame reports is in PDF points; the percentages a reader
  // reads are CSS pixels, and 100% is 96/72 of a point. `viewerScale` does the
  // same conversion on the way in.
  const percent = $derived(view.scale ? Math.round(view.scale * 100) : null);
  const at = $derived(view.scale ?? 1);
  const step = (factor) =>
    onmode(String(Math.max(MIN, Math.min(MAX, Number((at * factor).toFixed(2))))));

  // The page box shows the frame's page until the reader starts typing in it.
  // While it has focus the frame's scrolling must not overwrite what is being
  // typed, and leaving it without Enter puts the real page back.
  let pageText = $state("");
  let editingPage = false;
  $effect(() => {
    const page = view.page;
    if (!editingPage) pageText = page ? String(page) : "";
  });
  function jump(event) {
    if (event.key === "Escape") {
      event.preventDefault();
      pageText = view.page ? String(view.page) : "";
      event.currentTarget.blur();
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    const wanted = Number.parseInt(pageText, 10);
    if (Number.isFinite(wanted) && view.pages > 0) {
      const page = Math.max(1, Math.min(view.pages, wanted));
      pageText = String(page);
      send({ type: "viewer-page", page });
    }
    event.currentTarget.blur();
  }

  // The search row. Typing searches after a short pause, so a word typed
  // quickly is one search through the document rather than one per letter.
  let searching = $state(false);
  let query = $state("");
  let field = $state(null);
  let timer = null;
  const reveal = async () => {
    searching = true;
    await tick();
    field?.focus();
    field?.select();
  };
  $effect(() => {
    if (findAsk > 0) void reveal();
  });
  $effect(() => () => clearTimeout(timer));
  function typed() {
    clearTimeout(timer);
    timer = setTimeout(() => send({ type: "viewer-find", query, step: 0 }), 200);
  }
  function closeSearch() {
    clearTimeout(timer);
    searching = false;
    query = "";
    send({ type: "viewer-find", query: "", step: 0 });
  }
  function stepFind(step) {
    clearTimeout(timer);
    if (!query) return;
    send({ type: "viewer-find", query, step });
  }
  function searchKey(event) {
    if (event.key === "Enter") {
      event.preventDefault();
      stepFind(event.shiftKey ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeSearch();
    } else if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "f") {
      // The browser's own find would search this page, not the document.
      event.preventDefault();
      field?.select();
    }
  }
  const found = $derived(
    !query ? "" : view.find && view.find.total > 0 ? `${view.find.current} of ${view.find.total}` : view.find ? "No results" : "",
  );

  function chosen(item) {
    if (item.value === "spread") send({ type: "viewer-spread", spread: !view.spread });
    else if (item.value === "invert") send({ type: "viewer-invert", invert: !view.invert });
    else if (item.value === "present") onpresent?.();
    else if (item.value === "download") ondownload?.();
  }
</script>

<div class="preview-band">
  <div class="preview-controls" role="group" aria-label="Page view">
    <IconButton icon="panel-left" label="Pages" tone="plain" size="btn-icon-sm"
                pressed={view.sidebar} onclick={() => send({ type: "viewer-sidebar", open: !view.sidebar })} />
    <IconButton icon="search" label="Search" tone="plain" size="btn-icon-sm"
                pressed={searching} onclick={() => (searching ? closeSearch() : void reveal())} />
    <span class="preview-page">
      <input class="preview-page-input" inputmode="numeric" aria-label="Page" autocomplete="off"
             bind:value={pageText} onkeydown={jump}
             onfocus={() => (editingPage = true)}
             onblur={() => { editingPage = false; pageText = view.page ? String(view.page) : ""; }} />
      <span class="preview-page-total">/ {view.pages}</span>
    </span>
    <IconButton icon="zoom-out" label="Zoom out" tone="plain" size="btn-icon-sm"
                disabled={at <= MIN} onclick={() => step(1 / 1.1)} />
    <select class="preview-zoom" aria-label="Zoom" value={view.mode}
            onchange={(event) => onmode(event.currentTarget.value)}>
      <option value="auto">Automatic</option>
      <option value="page-actual">Actual size</option>
      <option value="page-fit">Fit page</option>
      <option value="page-width">Fit width</option>
      <!-- Where a reader lands after pressing a zoom button: a percentage that
           is not one of the steps, shown rather than left as an empty box. -->
      {#if percent !== null && !STEPS.includes(percent) && !["auto", "page-actual", "page-fit", "page-width"].includes(view.mode)}
        <option value={view.mode}>{percent}%</option>
      {/if}
      {#each STEPS as number}
        <option value={String(number / 100)}>{number}%</option>
      {/each}
    </select>
    <IconButton icon="zoom-in" label="Zoom in" tone="plain" size="btn-icon-sm"
                disabled={at >= MAX} onclick={() => step(1.1)} />
    <div class="preview-tools">
      <IconButton icon="text-cursor" label="Select text" size="btn-icon-sm"
                  pressed={tool !== "hand"} onclick={() => ontool("select")} />
      <IconButton icon="hand" label="Move the page" size="btn-icon-sm"
                  pressed={tool === "hand"} onclick={() => ontool("hand")} />
    </div>
    <span class="preview-spacer"></span>
    {#if busy}<span class="preview-busy" role="status" aria-label="Compiling"><span class="spinner"></span></span>{/if}
    <Menu onSelect={chosen} positioning={{ placement: "bottom-end" }}>
      <MenuIconButton icon="more-horizontal" label="More" size="1rem" />
      <ExplorerMenu>
        <Menu.Item value="spread" class="menuitem">
          <span class="menuitem-check" aria-hidden="true">{view.spread ? "✓" : ""}</span>
          <span class="menuitem-label">Two pages side by side</span>
        </Menu.Item>
        {#if dark}
          <Menu.Item value="invert" class="menuitem">
            <span class="menuitem-check" aria-hidden="true">{view.invert ? "✓" : ""}</span>
            <span class="menuitem-label">Invert pages</span>
          </Menu.Item>
        {/if}
        <Menu.Item value="present" class="menuitem">
          <span class="menuitem-check" aria-hidden="true"></span>
          <span class="menuitem-label">Present</span>
        </Menu.Item>
        {#if ondownload}
          <hr class="hr my-1" />
          <Menu.Item value="download" class="menuitem">
            <span class="menuitem-check" aria-hidden="true"></span>
            <span class="menuitem-label">Download PDF</span>
          </Menu.Item>
        {/if}
      </ExplorerMenu>
    </Menu>
  </div>
  {#if searching}
    <div class="preview-find" role="search" aria-label="Search the document">
      <input class="preview-find-input" type="text" placeholder="Search" aria-label="Search the document"
             autocomplete="off" spellcheck="false" maxlength="200"
             bind:this={field} bind:value={query} oninput={typed} onkeydown={searchKey} />
      <span class="preview-find-count" role="status" aria-live="polite">{found}</span>
      <IconButton icon="chevron-up" label="Previous match" tone="plain" size="btn-icon-sm"
                  disabled={!query} onclick={() => stepFind(-1)} />
      <IconButton icon="chevron-down" label="Next match" tone="plain" size="btn-icon-sm"
                  disabled={!query} onclick={() => stepFind(1)} />
      <IconButton icon="x" label="Close search" tone="plain" size="btn-icon-sm" onclick={closeSearch} />
    </div>
  {/if}
</div>
