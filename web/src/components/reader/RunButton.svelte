<script>
  // The Run control for a Quarto document: one split button. The main half
  // runs the document, or stops a run that is going. The chevron opens the
  // rarer actions (Stop, Share results, the Run directory) and the state of
  // the last run, so the top bar keeps a single control whatever happened. The
  // two halves share one border, with a divider between them, so they read as
  // one button.
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "../Icon.svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";

  let {
    phase = "idle", stale = 0, lastRunAt = "", onrun = () => {}, onstop = () => {}, ondirectory = () => {},
    share = true, onshare = () => {}, shareLine = "", directoryName = "", directoryPath = "", directoryMissing = false,
  } = $props();

  const running = $derived(phase === "running");
  const failed = $derived(phase === "failed");
  // A failed run says so in the hover text, which is where someone looks
  // before pressing Run again.
  const primaryTip = $derived(running ? undefined : failed ? m.runbutton_failed() : m.runbutton_run_hint());
  const staleLabel = $derived(m.runbutton_stale({ count: stale }));
  const lastRunTime = $derived(lastRunAt ? new Intl.DateTimeFormat(locale(), { timeStyle: "short" }).format(new Date(lastRunAt)) : "");

  function choose(item) {
    if (item.value === "stop") onstop();
    else if (item.value === "directory") ondirectory();
  }
</script>

<div class="run-button" role="group" aria-label={m.runbutton_run()}>
  <button type="button" class="run-main" data-tip={primaryTip} onclick={() => (running ? onstop() : onrun())}>
    {#if running}
      <span class="spinner" aria-hidden="true"></span>
      <span class="run-label">{m.runbutton_stop()}</span>
    {:else}
      <Icon name="play" size="0.875rem" />
      <span class="run-label">{m.runbutton_run()}</span>
      {#if stale > 0}
        <span class="run-stale" role="img" aria-label={staleLabel} data-tip={staleLabel}>{stale}</span>
      {/if}
      {#if failed}
        <span class="run-failed-dot" aria-hidden="true"></span>
      {/if}
    {/if}
  </button>
  <Menu onSelect={choose} positioning={{ placement: "bottom-end" }}>
    <Menu.Trigger class="run-chevron" aria-label={m.runbutton_options()} data-tip={m.runbutton_options()}>
      <Icon name="chevron-down" size="0.875rem" />
    </Menu.Trigger>
    <ExplorerMenu>
      {#if running}
        <Menu.Item value="stop" class="menuitem">
          <span class="menuitem-check" aria-hidden="true"></span>
          <span class="menuitem-label">{m.runbutton_stop()}</span>
        </Menu.Item>
      {/if}
      {#if running}<Menu.Separator class="hr my-1" />{/if}
      <Menu.OptionItem type="checkbox" value="share" checked={share} onCheckedChange={(on) => onshare(on)} class="menuitem">
        <span class="menuitem-check" aria-hidden="true">{share ? "✓" : ""}</span>
        <span class="menuitem-label">{m.rundirectorycontrol_share_results()}</span>
      </Menu.OptionItem>
      {#if shareLine}
        <div class="run-note" role="status">{shareLine}</div>
      {/if}
      <Menu.Item value="directory" class="menuitem" data-tip={directoryPath || undefined} data-tip-place="left">
        <span class="menuitem-check" aria-hidden="true"><Icon name={directoryMissing ? "triangle-alert" : "folder"} size="0.875rem" /></span>
        <span class="menuitem-label">{directoryName ? m.runbutton_directory_named({ name: directoryName }) : m.runbutton_choose_directory()}</span>
      </Menu.Item>
      {#if lastRunAt || stale > 0 || failed}<Menu.Separator class="hr my-1" />{/if}
      {#if lastRunAt}
        <div class="run-note">{m.runbutton_last_run({ time: lastRunTime })}</div>
      {/if}
      {#if stale > 0}
        <div class="run-note">{staleLabel}</div>
      {/if}
      {#if failed}
        <div class="run-note">{m.runbutton_failed()}</div>
      {/if}
    </ExplorerMenu>
  </Menu>
</div>

<style>
  /* The height and type match the top bar's other controls (.bar-toggle). */
  .run-button {
    display: inline-flex;
    align-items: stretch;
    height: 1.75rem;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-base);
    background: var(--color-subtle);
    color: var(--color-text);
    font-size: var(--text-sm);
    white-space: nowrap;
  }
  .run-main,
  :global(.run-chevron) {
    display: inline-flex;
    align-items: center;
    gap: calc(var(--spacing) * 1.5);
    padding: 0 calc(var(--spacing) * 2.5);
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .run-main {
    border-radius: calc(var(--radius-base) - 1px) 0 0 calc(var(--radius-base) - 1px);
  }
  :global(.run-chevron) {
    padding: 0 calc(var(--spacing) * 1.5);
    border-left: 1px solid var(--color-border);
    border-radius: 0 calc(var(--radius-base) - 1px) calc(var(--radius-base) - 1px) 0;
  }
  .run-main:hover,
  :global(.run-chevron:hover),
  :global(.run-chevron[data-state="open"]) {
    background: var(--color-row-hover);
  }
  .run-main:focus-visible,
  :global(.run-chevron:focus-visible) {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }
  .run-stale {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.25rem;
    height: 1.125rem;
    padding: 0 .3rem;
    border-radius: 999px;
    background: var(--color-raised);
    color: var(--color-text-secondary);
    font-size: var(--text-xs);
    line-height: 1;
  }
  .run-failed-dot {
    width: .4rem;
    height: .4rem;
    border-radius: 50%;
    background: var(--color-error-solid);
  }
  .run-note {
    padding: .25rem .75rem;
    color: var(--color-text-secondary);
    font-size: var(--text-xs);
    white-space: normal;
  }
</style>
