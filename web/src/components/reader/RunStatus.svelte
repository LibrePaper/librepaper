<script>
  // A small status in the preview's corner for a local run that is taking a
  // while: what is running, for how long, and a Stop. It is laid over the pane
  // without taking room, and says nothing for the first two seconds.
  import { onMount } from "svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";
  import { elapsedSeconds, runStatusVisible } from "../../lib/reader/run-status.js";

  /** @type {{ since: number, onstop?: () => void, tool?: "quarto" | "calepin" }} */
  let { since, onstop = () => {}, tool = "quarto" } = $props();
  let now = $state(Date.now());
  onMount(() => {
    const timer = setInterval(() => { now = Date.now(); }, 500);
    return () => clearInterval(timer);
  });
  const elapsed = $derived(now - since);
  const seconds = $derived(new Intl.NumberFormat(locale()).format(elapsedSeconds(elapsed)));
</script>

{#if runStatusVisible(elapsed)}
  <div class="run-status" role="status">
    <span>{tool === "calepin" ? m.runstatus_running_calepin({ seconds }) : m.runstatus_running({ seconds })}</span>
    <button type="button" class="run-status-stop" onclick={onstop}>{m.runbutton_stop()}</button>
  </div>
{/if}

<style>
  .run-status {
    position: absolute;
    right: calc(var(--spacing) * 3);
    bottom: calc(var(--spacing) * 3);
    z-index: 9;
    display: inline-flex;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * 0.5) calc(var(--spacing) * 2);
    border-radius: var(--radius-base);
    background: var(--color-row-hover);
    color: var(--color-text-secondary);
    font-size: var(--text-xs);
    font-family: var(--font-sans, inherit);
  }
  .run-status-stop {
    padding: 0;
    border: 0;
    background: none;
    color: var(--color-text);
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }
  .run-status-stop:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
  }
</style>
