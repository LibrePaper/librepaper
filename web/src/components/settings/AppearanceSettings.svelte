<script>
  // How the pages look for the person at this browser.
  import SettingRow from "./SettingRow.svelte";
  import { choice, setChoice } from "../../lib/theme.js";

  let current = $state(choice());
</script>

<SettingRow id="appearance-theme" title="Theme" description="Light, dark, or whatever this computer uses.">
  <select class="select setting-select" aria-label="Theme" value={current}
          onchange={(event) => { setChoice(event.currentTarget.value); current = choice(); }}>
    <option value="system">System</option>
    <option value="light">Light</option>
    <option value="dark">Dark</option>
  </select>
</SettingRow>
