<script>
  // The slim band of controls across the top of a pane: the preview's page
  // controls, and the formatting tools above the source. Both are this, so the
  // two sides stand at the same height with the same rule under them.
  //
  // Nothing is decided up front about what is in a menu. Every item is a
  // button while the band is wide enough; when it is not, items move into a
  // "..." menu starting from the right-most, and the "..." button exists only
  // while at least one has moved.
  //
  // `items` are in order, each `{ id, group, label, icon, pressed, disabled,
  // onselect, pinned }`. An item with `pinned` never moves (a page box, a
  // select); one without a `label` cannot be a menu entry and is pinned too.
  // `item(it)` is a snippet that draws one item as it stands in the band. A
  // divider is drawn between neighbouring groups and goes with a group's last
  // visible item. `end` is drawn at the right end and never moves.
  //
  // How it fits: a hidden copy of the items (every item, always) is measured,
  // so an item that is out of the band still has a width, and fonts or zoom
  // that change a width are seen by the observer on that copy. The band's own
  // width comes from an observer on the band, which is a size container and so
  // does not depend on what is in it; there is no loop. lib/band-fit.js decides.
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import MenuIconButton from "./MenuIconButton.svelte";
  import { fitBand } from "../lib/band-fit.js";
  import { m } from "../lib/paraglide/messages.js";

  // `hidden` reports the ids that have moved into the menu, for a caller that
  // keeps a roving tabindex. `toolbar` makes the band a toolbar with arrow-key
  // movement over its visible buttons. `onmenuselect` runs after a menu entry
  // has run, so a caller can hand the focus back.
  let { items = [], item, end = undefined, label = "", toolbar = false, hidden = $bindable([]), onmenuselect = undefined } = $props();

  let row = $state(null);
  let measure = $state(null);
  let endWidth = $state(0);
  let available = $state(Infinity);
  let widths = $state({});
  let parts = $state({ gap: 0, divider: 1, more: 32 });

  function measureItems() {
    if (!measure || !row) return;
    const next = {};
    for (const child of measure.querySelectorAll("[data-measure]")) next[child.dataset.measure] = child.getBoundingClientRect().width;
    widths = next;
    const style = getComputedStyle(row);
    const probe = measure.querySelector("[data-more]");
    parts = {
      gap: Number.parseFloat(style.columnGap) || 0,
      divider: measure.querySelector("[data-divider]")?.getBoundingClientRect().width ?? 1,
      more: probe?.getBoundingClientRect().width ?? 32,
    };
    available = row.clientWidth - (Number.parseFloat(style.paddingLeft) || 0) - (Number.parseFloat(style.paddingRight) || 0);
  }

  $effect(() => {
    if (!row || !measure) return;
    measureItems();
    const observer = new ResizeObserver(measureItems);
    observer.observe(row);
    observer.observe(measure);
    return () => observer.disconnect();
  });
  // An item added, removed or relabelled changes the copy's size, which the
  // observer sees; this covers one that changes without changing size.
  $effect(() => {
    items.map((it) => it.id);
    queueMicrotask(measureItems);
  });

  const isPinned = (it) => Boolean(it.pinned) || !it.label;
  const fit = $derived.by(() => {
    const room = available - (end && endWidth ? endWidth + parts.gap : 0);
    return fitBand({
      items: items.map((it) => ({ width: widths[it.id] ?? 0, pinned: isPinned(it), group: it.group })),
      available: room,
      gap: parts.gap,
      divider: parts.divider,
      more: parts.more,
    });
  });
  const overflow = $derived(items.filter((_, index) => !fit.visible[index]));
  $effect(() => {
    const ids = overflow.map((it) => it.id);
    if (ids.join("\n") !== hidden.join("\n")) hidden = ids;
  });

  function onkeydown(event) {
    if (!toolbar) return;
    const buttons = [...row.querySelectorAll(".band-items button:not(:disabled), .band-more-slot button")];
    const here = buttons.indexOf(document.activeElement);
    if (here === -1) return;
    const step = { ArrowRight: here + 1, ArrowLeft: here - 1, Home: 0, End: buttons.length - 1 }[event.key];
    if (step === undefined) return;
    event.preventDefault();
    buttons[(step + buttons.length) % buttons.length].focus();
  }

  function selected(choice) {
    const entry = items.find((it) => it.id === choice.value);
    if (entry && !entry.disabled) entry.onselect?.();
    onmenuselect?.(entry);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
<div class="tool-band" role={toolbar ? "toolbar" : "group"} tabindex={toolbar ? -1 : undefined} aria-label={label} bind:this={row} {onkeydown}>
  <div class="band-items">
    {#each items as it, index (it.id)}
      {#if fit.visible[index]}
        {#if fit.dividers[index]}<span class="band-divider" aria-hidden="true"></span>{/if}
        {@render item(it)}
      {/if}
    {/each}
  </div>
  {#if overflow.length}
    <span class="band-more-slot">
      <Menu onSelect={selected} positioning={{ placement: "bottom-end" }}>
        <MenuIconButton icon="more-horizontal" label={m.previewcontrols_more()} size="1rem" tabindex={toolbar ? -1 : undefined}
                        onmousedown={(event) => event.preventDefault()} />
        <ExplorerMenu>
          {#each overflow as it (it.id)}
            <Menu.Item value={it.id} disabled={Boolean(it.disabled)} class="menuitem">
              <span class="menuitem-check" aria-hidden="true">{it.pressed ? "✓" : ""}</span>
              <span class="menuitem-label">{it.label}</span>
            </Menu.Item>
          {/each}
        </ExplorerMenu>
      </Menu>
    </span>
  {/if}
  <span class="band-spacer"></span>
  {#if end}<div class="band-end" bind:clientWidth={endWidth}>{@render end()}</div>{/if}

  <!-- Every item, whether it is showing or not, so each has a width. -->
  <div class="band-measure" bind:this={measure} aria-hidden="true" inert>
    {#each items as it (it.id)}
      <span class="band-measured" data-measure={it.id}>{@render item(it)}</span>
    {/each}
    <span class="band-divider" data-divider></span>
    <span class="band-measured" data-more><MenuIconButton icon="more-horizontal" label={m.previewcontrols_more()} size="1rem" /></span>
  </div>
</div>

<style>
  /* The band is a size container: its width is the pane's, not its contents',
     which is what keeps measuring from feeding back into itself. */
  .tool-band {
    position: relative;
    container-type: inline-size;
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 1);
    box-sizing: border-box;
    min-height: var(--librepaper-format-bar);
    min-width: 0;
    padding-inline: calc(var(--spacing) * 2);
    border-bottom: 1px solid var(--color-divider);
    background: var(--color-shell);
    overflow: hidden;
    white-space: nowrap;
  }
  .band-items {
    display: flex;
    align-items: center;
    gap: inherit;
    flex: 0 0 auto;
  }
  .band-more-slot { display: inline-flex; flex: none; }
  .band-spacer { flex: 1 1 auto; min-width: 0; }
  .band-end { display: flex; flex: none; align-items: center; gap: inherit; }
  .band-divider {
    flex: none;
    width: 1px;
    height: 1.25rem;
    background: var(--color-divider);
  }
  .band-measure {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 0;
    display: flex;
    width: max-content;
    height: 0;
    overflow: hidden;
    visibility: hidden;
    pointer-events: none;
  }
  .band-measured { display: inline-flex; flex: none; align-items: center; }
</style>
