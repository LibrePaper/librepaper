<script>
  // How the pages look for the person at this browser.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { choice, onChoice, setChoice, PALETTES, PALETTE_NAMES, palette, onPalette, setPalette, PAPERS, paper, onPaper, setPaper } from "../../lib/theme.js";
  import { FONTS, installed, MIN_SIZE, MAX_SIZE, font, setFont, onFont, size, setSize, onSize, lineHeight, setLineHeight, onLineHeight } from "../../lib/editor-type.js";

  // Follows the choice, which another tab of this browser may make too.
  let current = $state(choice());
  onMount(() => onChoice((now) => { current = now; }));

  let currentPalette = $state(palette());
  onMount(() => onPalette((now) => { currentPalette = now; }));

  let currentPaper = $state(paper());
  onMount(() => onPaper((now) => { currentPaper = now; }));

  // Follows the font, size and line height, which a pinch or another tab may change.
  let currentFont = $state(font());
  onMount(() => onFont((now) => { currentFont = now; }));

  let currentSize = $state(size());
  onMount(() => onSize((now) => { currentSize = now; }));

  let currentLineHeight = $state(lineHeight());
  onMount(() => onLineHeight((now) => { currentLineHeight = now; }));
</script>

<SettingRow id="appearance-theme" title="Theme">
  <select class="select setting-select" aria-label="Theme" value={current}
          onchange={(event) => setChoice(event.currentTarget.value)}>
    <option value="system">System</option>
    <option value="light">Light</option>
    <option value="dark">Dark</option>
  </select>
</SettingRow>

<SettingRow id="appearance-colours" title="Colours">
  <select class="select setting-select" aria-label="Colours" value={currentPalette}
          onchange={(event) => setPalette(event.currentTarget.value)}>
    {#each PALETTES as name (name)}
      <option value={name}>{PALETTE_NAMES[name]}</option>
    {/each}
  </select>
</SettingRow>

<SettingRow id="appearance-preview-paper" title="Preview paper"
            description="Dark mode can show HTML previews on dark paper. Always light shows the page as readers of the exported HTML see it.">
  <select class="select setting-select" aria-label="Preview paper" value={currentPaper}
          onchange={(event) => setPaper(event.currentTarget.value)}>
    {#each PAPERS as name (name)}
      <option value={name}>{name === "theme" ? "Follow theme" : "Always light"}</option>
    {/each}
  </select>
</SettingRow>

<SettingRow id="appearance-font" title="Font"
            description="Six fonts are included and download when you pick one. Another font has to be installed on this computer.">
  <span class="setting-inline">
    <select class="select setting-select" aria-label="Font" value={currentFont.id}
            onchange={(event) => setFont(event.currentTarget.value, currentFont.custom)}>
      {#each FONTS as entry (entry.id)}
        <option value={entry.id}>{entry.name}</option>
      {/each}
    </select>
    {#if currentFont.id === "custom"}
      <input type="text" class="input input-sm setting-input" aria-label="Font name" placeholder="Font name"
             value={currentFont.custom}
             onchange={(event) => setFont("custom", event.currentTarget.value)} />
      {#if currentFont.custom && !installed(currentFont.custom)}
        <span class="font-missing">Not found on this computer.</span>
      {/if}
    {/if}
  </span>
</SettingRow>

<SettingRow id="appearance-font-size" title="Font size"
            description="Pinch or Ctrl and scroll over the source to change it there too.">
  <span class="setting-inline">
    <input type="number" class="input input-sm setting-input setting-number" min={MIN_SIZE} max={MAX_SIZE} step="1"
           aria-label="Font size in pixels" value={currentSize}
           onchange={(event) => {
             const field = event.currentTarget;
             const next = field.value.trim() === "" ? NaN : Number(field.value);
             if (Number.isFinite(next)) setSize(next);
             field.value = String(size());
           }} />
    <span>px</span>
  </span>
</SettingRow>

<SettingRow id="appearance-line-height" title="Line height">
  <select class="select setting-select" aria-label="Line height" value={currentLineHeight}
          onchange={(event) => setLineHeight(event.currentTarget.value)}>
    <option value="compact">Compact</option>
    <option value="normal">Normal</option>
    <option value="wide">Wide</option>
  </select>
</SettingRow>

<style>
  /* A font name goes under the list when both do not fit beside the words. */
  .setting-inline {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: calc(var(--spacing) * 2);
  }
  .font-missing {
    flex-basis: 100%;
    text-align: right;
    color: var(--panel-muted);
    font-size: var(--panel-meta-size);
  }
  /* A size is two digits, so its field is narrow. */
  .setting-number {
    width: 5rem;
  }
</style>
