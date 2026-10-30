<script>
  // How the whole app looks, what the desktop app does when its window closes, and the diagnostics report.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { choice, onChoice, setChoice, PALETTES, PALETTE_NAMES, palette, onPalette, setPalette, PAPERS, paper, onPaper, setPaper } from "../../lib/theme.js";
  import DiagnosticsPanel from "./DiagnosticsPanel.svelte";
  import { desktop, shellAction } from "../../lib/desktop/client.js";

  // Follows the choice, which another tab of this browser may make too.
  let current = $state(choice());
  onMount(() => onChoice((now) => { current = now; }));

  let currentPalette = $state(palette());
  onMount(() => onPalette((now) => { currentPalette = now; }));

  let currentPaper = $state(paper());
  onMount(() => onPaper((now) => { currentPaper = now; }));

  // What closing the last window does in the desktop app. The shell saves it.
  let closing = $state(desktop()?.close === "tray" ? "tray" : "quit");
  let closeError = $state("");
  async function chooseClose(next) {
    const before = closing;
    closing = next;
    closeError = "";
    try {
      await shellAction("close", { close: next });
    } catch (cause) {
      closing = before;
      closeError = cause?.message || "The desktop app could not save that.";
    }
  }
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

{#if desktop()}
  <SettingRow id="general-close" title="When the window closes"
              description="With the tray, LibrePaper keeps running after its last window closes.">
    <select class="select setting-select" aria-label="When the window closes" value={closing}
            onchange={(event) => chooseClose(event.currentTarget.value)}>
      <option value="quit">Quit LibrePaper</option>
      <option value="tray">Keep running in the tray</option>
    </select>
    {#snippet note()}{#if closeError}<span class="close-error" role="alert">{closeError}</span>{/if}{/snippet}
  </SettingRow>
{/if}

<section id="diagnostics-report" class="settings-subsection"><DiagnosticsPanel /></section>

<style>
  .close-error {
    color: var(--panel-muted);
    font-size: var(--panel-meta-size);
  }
</style>
