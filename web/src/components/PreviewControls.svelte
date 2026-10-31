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
  import { m } from "../lib/paraglide/messages.js";

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
    !query ? "" : view.find && view.find.total > 0 ? m.previewcontrols_find_count({ current: view.find.current, total: view.find.total }) : view.find ? m.previewcontrols_find_none() : "",
  );

  function chosen(item) {
    if (item.value === "spread") send({ type: "viewer-spread", spread: !view.spread });
    else if (item.value === "invert") send({ type: "viewer-invert", invert: !view.invert });
    else if (item.value === "present") onpresent?.();
    else if (item.value === "download") ondownload?.();
  }
</script>

<div class="preview-band">
  <div class="preview-controls" role="group" aria-label={m.previewcontrols_page_view()}>
    <IconButton icon="panel-left" label={m.previewcontrols_pages()}tone="plain" size="btn-icon-sm"
                pressed={view.sidebar} onclick={() => send({ type: "viewer-sidebar", open: !view.sidebar })} />
    <IconButton icon="search" label={m.previewcontrols_search()}tone="plain" size="btn-icon-sm"
                pressed={searching} onclick={() => (searching ? closeSearch() : void reveal())} />
    <span class="preview-page">
      <input class="preview-page-input" inputmode="numeric" aria-label={m.previewcontrols_page()}autocomplete="off"
             bind:value={pageText} onkeydown={jump}
             onfocus={() => (editingPage = true)}
             onblur={() => { editingPage = false; pageText = view.page ? String(view.page) : ""; }} />
      <span class="preview-page-total">/ {view.pages}</span>
    </span>
    <IconButton icon="zoom-out" label={m.previewcontrols_zoom_out()}tone="plain" size="btn-icon-sm"
                disabled={at <= MIN} onclick={() => step(1 / 1.1)} />
    <select class="preview-zoom" aria-label={m.previewcontrols_zoom()} value={view.mode}
            onchange={(event) => onmode(event.currentTarget.value)}>
      <option value="auto">{m.previewcontrols_zoom_automatic()}</option>
      <option value="page-actual">{m.previewcontrols_zoom_actual()}</option>
      <option value="page-fit">{m.previewcontrols_zoom_fit_page()}</option>
      <option value="page-width">{m.previewcontrols_zoom_fit_width()}</option>
      <!-- Where a reader lands after pressing a zoom button: a percentage that
           is not one of the steps, shown rather than left as an empty box. -->
      {#if percent !== null && !STEPS.includes(percent) && !["auto", "page-actual", "page-fit", "page-width"].includes(view.mode)}
        <option value={view.mode}>{percent}%</option>
      {/if}
      {#each STEPS as number}
        <option value={String(number / 100)}>{number}%</option>
      {/each}
    </select>
    <IconButton icon="zoom-in" label={m.previewcontrols_zoom_in()}tone="plain" size="btn-icon-sm"
                disabled={at >= MAX} onclick={() => step(1.1)} />
    <div class="preview-tools">
      <IconButton icon="text-cursor" label={m.previewcontrols_select_text()}size="btn-icon-sm"
                  pressed={tool !== "hand"} onclick={() => ontool("select")} />
      <IconButton icon="hand" label={m.previewcontrols_move_page()}size="btn-icon-sm"
                  pressed={tool === "hand"} onclick={() => ontool("hand")} />
    </div>
    <span class="preview-spacer"></span>
    {#if busy}<span class="preview-busy" role="status" aria-label={m.previewcontrols_compiling()}><span class="spinner"></span></span>{/if}
    <Menu onSelect={chosen} positioning={{ placement: "bottom-end" }}>
      <MenuIconButton icon="more-horizontal" label={m.previewcontrols_more()}size="1rem" />
      <ExplorerMenu>
        <Menu.Item value="spread" class="menuitem">
          <span class="menuitem-check" aria-hidden="true">{view.spread ? "✓" : ""}</span>
          <span class="menuitem-label">{m.previewcontrols_spread()}</span>
        </Menu.Item>
        {#if dark}
          <Menu.Item value="invert" class="menuitem">
            <span class="menuitem-check" aria-hidden="true">{view.invert ? "✓" : ""}</span>
            <span class="menuitem-label">{m.previewcontrols_invert()}</span>
          </Menu.Item>
        {/if}
        <Menu.Item value="present" class="menuitem">
          <span class="menuitem-check" aria-hidden="true"></span>
          <span class="menuitem-label">{m.previewcontrols_present()}</span>
        </Menu.Item>
        {#if ondownload}
          <hr class="hr my-1" />
          <Menu.Item value="download" class="menuitem">
            <span class="menuitem-check" aria-hidden="true"></span>
            <span class="menuitem-label">{m.previewcontrols_download_pdf()}</span>
          </Menu.Item>
        {/if}
      </ExplorerMenu>
    </Menu>
  </div>
  {#if searching}
    <div class="preview-find" role="search" aria-label={m.previewcontrols_search_document()}>
      <input class="preview-find-input" type="text" placeholder={m.previewcontrols_search()} aria-label={m.previewcontrols_search_document()}
             autocomplete="off" spellcheck="false" maxlength="200"
             bind:this={field} bind:value={query} oninput={typed} onkeydown={searchKey} />
      <span class="preview-find-count" role="status" aria-live="polite">{found}</span>
      <IconButton icon="chevron-up" label={m.previewcontrols_previous_match()}tone="plain" size="btn-icon-sm"
                  disabled={!query} onclick={() => stepFind(-1)} />
      <IconButton icon="chevron-down" label={m.previewcontrols_next_match()}tone="plain" size="btn-icon-sm"
                  disabled={!query} onclick={() => stepFind(1)} />
      <IconButton icon="x" label={m.previewcontrols_close_search()}tone="plain" size="btn-icon-sm" onclick={closeSearch} />
    </div>
  {/if}
</div>
