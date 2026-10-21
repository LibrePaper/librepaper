<script>
  // How the pages look for the person at this browser.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { choice, onChoice, setChoice, PALETTES, PALETTE_NAMES, palette, onPalette, setPalette } from "../../lib/theme.js";

  // Follows the choice, which another tab of this browser may make too.
  let current = $state(choice());
  onMount(() => onChoice((now) => { current = now; }));

  let currentPalette = $state(palette());
  onMount(() => onPalette((now) => { currentPalette = now; }));
</script>

<SettingRow id="appearance-theme" title="Theme" description="Light, dark, or whatever this computer uses.">
  <select class="select setting-select" aria-label="Theme" value={current}
          onchange={(event) => setChoice(event.currentTarget.value)}>
    <option value="system">System</option>
    <option value="light">Light</option>
    <option value="dark">Dark</option>
  </select>
</SettingRow>

<SettingRow id="appearance-colours" title="Colours" description="The colours of the window, the editor and its highlighting. Each one has a light and a dark version, and the Theme above picks which.">
  <select class="select setting-select" aria-label="Colours" value={currentPalette}
          onchange={(event) => setPalette(event.currentTarget.value)}>
    {#each PALETTES as name (name)}
      <option value={name}>{PALETTE_NAMES[name]}</option>
    {/each}
  </select>
</SettingRow>
