<script>
  // How the pages look for the person at this browser.
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { choice, onChoice, setChoice } from "../../lib/theme.js";

  // Follows the choice, which another tab of this browser may make too.
  let current = $state(choice());
  onMount(() => onChoice((now) => { current = now; }));
</script>

<SettingRow id="appearance-theme" title="Theme" description="Light, dark, or whatever this computer uses.">
  <select class="select setting-select" aria-label="Theme" value={current}
          onchange={(event) => setChoice(event.currentTarget.value)}>
    <option value="system">System</option>
    <option value="light">Light</option>
    <option value="dark">Dark</option>
  </select>
</SettingRow>
