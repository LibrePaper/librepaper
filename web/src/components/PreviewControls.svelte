<script>
  // The controls for a paged document: how large the page is drawn, and what
  // the pointer does on it.
  //
  // These used to be a toolbar inside the PDF frame, drawn in the operating
  // system's own button colours, which made the paged preview look like a
  // different application from the flowing one an inch to its left. They are
  // ordinary controls in the preview header now, so a format that has zoom
  // and a format that has none differ by which controls are in the row rather
  // than by whether the row is there at all.
  //
  // Nothing is decided here: the frame owns the scale it actually drew at, and
  // says so over the channel. This draws what it was told and sends back what
  // was asked for.
  import IconButton from "./IconButton.svelte";

  let { mode = "auto", scale = null, tool = "select", onmode, ontool } = $props();

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
  const percent = $derived(scale ? Math.round(scale * 100) : null);
  const at = $derived(scale ?? 1);
  const step = (factor) =>
    onmode(String(Math.max(MIN, Math.min(MAX, Number((at * factor).toFixed(2))))));
</script>

<div class="preview-controls" role="group" aria-label="Page view">
  <IconButton icon="zoom-out" label="Zoom out" tone="plain" size="btn-icon-sm"
              disabled={at <= MIN} onclick={() => step(1 / 1.1)} />
  <IconButton icon="zoom-in" label="Zoom in" tone="plain" size="btn-icon-sm"
              disabled={at >= MAX} onclick={() => step(1.1)} />
  <select class="preview-zoom" aria-label="Zoom" value={mode}
          onchange={(event) => onmode(event.currentTarget.value)}>
    <option value="auto">Automatic</option>
    <option value="page-actual">Actual size</option>
    <option value="page-fit">Fit page</option>
    <option value="page-width">Fit width</option>
    <!-- Where a reader lands after pressing a zoom button: a percentage that
         is not one of the steps, shown rather than left as an empty box. -->
    {#if percent !== null && !STEPS.includes(percent) && !["auto", "page-actual", "page-fit", "page-width"].includes(mode)}
      <option value={mode}>{percent}%</option>
    {/if}
    {#each STEPS as number}
      <option value={String(number / 100)}>{number}%</option>
    {/each}
  </select>
  <div class="preview-tools">
    <IconButton icon="text-cursor" label="Select text" size="btn-icon-sm"
                pressed={tool !== "hand"} onclick={() => ontool("select")} />
    <IconButton icon="hand" label="Move the page" size="btn-icon-sm"
                pressed={tool === "hand"} onclick={() => ontool("hand")} />
  </div>
</div>
