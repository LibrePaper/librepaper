<script>
  import { ICONS } from "../lib/icons.js";

  // Skeleton's button sizes the icon inside it, so the default here is only
  // for an icon that stands on its own.
  //
  // A few of these say whether something is on -- a star is either a favourite
  // or it is not -- and an outline that only changes colour reads as a click
  // that did not register. Those are drawn solid instead.
  let { name, size = null, filled = false } = $props();
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  fill={filled ? "currentColor" : "none"}
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#each ICONS[name] ?? [] as [shape, geometry]}
    {#if shape === "rect"}
      <rect {...geometry} />
    {:else if shape === "circle"}
      <circle {...geometry} />
    {:else if shape === "line"}
      <line {...geometry} />
    {:else if shape === "polyline"}
      <polyline {...geometry} />
    {:else}
      <path d={geometry} />
    {/if}
  {/each}
</svg>

<style>
  /* An inline SVG sits on the text baseline, which leaves a few pixels of
     descender space under it and pushes it off centre in a button that is
     otherwise square. */
  svg {
    display: block;
    flex: none;
  }
</style>
