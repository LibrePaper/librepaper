<script>
  import { TAG_COLOURS, tagColour } from "../lib/tag-colours.js";

  // The fixed palette as a row of swatches. Each one is a toggle button that
  // says which colour is chosen, so a keyboard user tabs through the row and
  // hears the colour names rather than nine unlabelled squares.
  let { value = "gray", label = "Colour", onchange = undefined } = $props();
</script>

<div class="tag-swatches" role="group" aria-label={label}>
  {#each TAG_COLOURS as colour (colour.key)}
    <button type="button" class="tag-swatch" style="--swatch: {tagColour(colour.key)}"
            aria-label={colour.name} title={colour.name}
            aria-pressed={value === colour.key}
            onclick={() => onchange?.(colour.key)}></button>
  {/each}
</div>

<style>
  /* Padded by the width of the chosen swatch's ring, so a dialog body that
     scrolls does not clip it on any side. */
  .tag-swatches { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); padding: 4px; }
  /* The chosen swatch gets a ring in the colour it is, set apart from the
     card by a gap, so the choice is a shape and not only a tint. */
  .tag-swatch { width: 1.5rem; height: 1.5rem; border-radius: 50%; border: 0; padding: 0; background: var(--swatch); cursor: pointer; }
  .tag-swatch[aria-pressed="true"] { box-shadow: 0 0 0 2px var(--color-raised), 0 0 0 4px var(--swatch); }
  .tag-swatch:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 3px; }
</style>
