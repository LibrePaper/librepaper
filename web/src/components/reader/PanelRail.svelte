<script>
  import IconButton from "../IconButton.svelte";
  import { slotId } from "../../lib/panels.js";

  // The row of panel icons. It is drawn twice -- down the side of the column
  // beside a mouse, along the bottom of the window under a thumb -- and an
  // icon, a label or an active mark that differed between the two would be a
  // panel a reader cannot recognize when the window changes shape. So both
  // are this, and the only thing they disagree about is what a closed column
  // looks like.
  let {
    tabs = [],
    panel = "",
    // Whether the panel area is showing. The rail beside the column keeps the
    // selected icon marked while the column is collapsed -- the icon is the
    // only thing left to say where a reader is. The row along the bottom is
    // itself the navigation, so there "on" means the panel is in front of you.
    open = false,
    marksWhenClosed = false,
    badges = {},
    onselect,
  } = $props();
</script>

{#each tabs as tab (tab.id)}
  {@const badge = badges[tab.id] ?? {}}
  <div class="rail-item">
    <IconButton icon={tab.icon} label={badge.says || tab.says}
      pressed={panel === tab.id && (open || marksWhenClosed)}
      controls={slotId(tab.id)} expanded={panel === tab.id && open}
      onclick={() => onselect?.(tab.id)} />
    {#if badge.dot}<span class="rail-dot" aria-hidden="true"></span>{/if}
    {#if badge.counts?.length}
      <span class="rail-counts" aria-hidden="true">
        {#each badge.counts as count (count.tone)}
          <span class="rail-count {count.tone}">{count.of}</span>
        {/each}
      </span>
    {/if}
  </div>
{/each}

<style>
  /* One shape for every button, badge or no badge, so a panel that grows a
     count does not move the ones beside it. */
  .rail-item { position: relative; display: flex; flex-direction: column; align-items: center; gap: 2px; }
  .rail-dot { position: absolute; right: 1px; top: 1px; width: 7px; height: 7px; border-radius: 50%; background: var(--color-primary-500); pointer-events: none; }
  .rail-counts { display: flex; gap: 4px; font-size: .625rem; line-height: 1; font-weight: 600; font-variant-numeric: tabular-nums; }
  .rail-count.errors { color: var(--color-error-500); }
  .rail-count.warnings { color: var(--color-warning-500); }
</style>
