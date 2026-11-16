<script>
  // Continuous compile of the LaTeX HTML preview, for this person in this browser:
  // it updates while typing instead of after a pause.
  import SettingRow from "./SettingRow.svelte";
  import { readLiveCompile, writeLiveCompile } from "../../lib/build-preferences.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { userId = "anonymous" } = $props();

  const scope = () => ({ origin: globalThis.location?.origin || "", user: userId });
  let value = $state(false);
  $effect.pre(() => { value = readLiveCompile(scope(), "latex"); });

  function choose(next) {
    value = writeLiveCompile(scope(), "latex", next);
  }
</script>

<SettingRow id="render-latex-live-compile" title={m.livecompile_title()} description={m.livecompile_description()}>
  <input type="checkbox" class="checkbox" aria-label={m.livecompile_title()} checked={value}
         onchange={(event) => choose(event.currentTarget.checked)} />
</SettingRow>
