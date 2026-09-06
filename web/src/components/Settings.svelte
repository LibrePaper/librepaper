<script>
  // This browser's own preferences, in one place. Nothing here is about the
  // document or reaches anyone else: the keys the editor answers to, whether a
  // click in one pane takes the other along, how the window is divided, and
  // which TeX distribution this browser compiles with. Each one lives in
  // localStorage under the reader's existing keys; this panel only shows and
  // sets what the reader already remembers.
  import PanelHeader from "./PanelHeader.svelte";
  import { RATIOS } from "../lib/panes.js";
  import * as latex from "../lib/latex.js";

  let {
    keys = "default",
    linked = false,
    sourceSide = "left",
    ratio = 1 / 2,
    sourceFormat = "",
    canChooseTex = false,
    onkeys,
    onlinked,
    onside,
    onratio,
    onchoosetex,
  } = $props();

  // The manifest names a distribution by an id; the card shows its label, and
  // so does this. Fetched once the panel is drawn, and only for a LaTeX
  // document, since nothing else has a distribution to name.
  let labels = $state({});
  $effect(() => {
    if (sourceFormat !== "latex") return;
    latex
      .available()
      .then((list) => {
        labels = Object.fromEntries(list.map((one) => [one.name, one.label]));
      })
      .catch(() => {});
  });
  const distribution = $derived(latex.chosen());
  const distributionName = $derived(distribution ? labels[distribution] || distribution : "");
</script>

<section class="panel settings-panel" aria-label="Settings">
  <PanelHeader title="Settings">
    <p class="panel-muted">These are this browser's own; they change nothing for anyone else.</p>
  </PanelHeader>

  <section class="settings-section" aria-labelledby="settings-keys">
    <h3 id="settings-keys" class="panel-section-title">Editor keys</h3>
    <label class="settings-row">
      <input type="checkbox" class="checkbox" checked={keys === "vim"}
             onchange={(event) => onkeys?.(event.currentTarget.checked ? "vim" : "default")} />
      <span>Vim keys</span>
    </label>
    <p class="panel-meta">Applies to the source pane.</p>
  </section>

  <section class="settings-section" aria-labelledby="settings-linked">
    <h3 id="settings-linked" class="panel-section-title">Keep in step</h3>
    <label class="settings-row">
      <input type="checkbox" class="checkbox" checked={linked}
             onchange={(event) => onlinked?.(event.currentTarget.checked)} />
      <span>Keep in step</span>
    </label>
    <p class="panel-meta">A click in the document opens the place in the source it came from.</p>
  </section>

  <section class="settings-section" aria-labelledby="settings-panes">
    <h3 id="settings-panes" class="panel-section-title">Source pane</h3>
    <label class="settings-row">
      <span class="settings-label">Side</span>
      <select class="select settings-select" aria-label="Which side the source is on" value={sourceSide}
              onchange={(event) => onside?.(event.currentTarget.value)}>
        <option value="left">Left</option>
        <option value="right">Right</option>
      </select>
    </label>
    <label class="settings-row">
      <span class="settings-label">Split</span>
      <!-- The same ratios a drag sticks to, for anyone who never finds that
           it does. A split left somewhere between them shows as none. -->
      <select class="select settings-select" aria-label="How the source and the document share the window"
              value={RATIOS.some((one) => one.share === ratio) ? String(ratio) : ""}
              onchange={(event) => onratio?.(Number(event.currentTarget.value))}>
        {#if !RATIOS.some((one) => one.share === ratio)}<option value="" disabled>Custom</option>{/if}
        {#each RATIOS as one}
          <option value={String(one.share)}>{one.says}</option>
        {/each}
      </select>
    </label>
  </section>

  {#if sourceFormat === "latex"}
    <section class="settings-section" aria-labelledby="settings-tex">
      <h3 id="settings-tex" class="panel-section-title">TeX distribution</h3>
      <p class="panel-muted">
        {distribution ? `Compiling with ${distributionName}.` : "No distribution chosen in this browser."}
      </p>
      {#if canChooseTex}
        <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => onchoosetex?.()}>
          {distribution ? "Choose a different distribution" : "Choose a distribution"}
        </button>
      {/if}
    </section>
  {/if}
</section>

<style>
  .settings-section { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); margin-bottom: var(--panel-section-gap); }
  .settings-row { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .settings-label { min-width: calc(var(--spacing) * 12); }
  .settings-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); max-width: 100%; }
</style>
