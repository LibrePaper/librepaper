<script>
  // Instant preview updates for LaTeX, for this person in this browser: a small
  // edit shows in the HTML preview at once and the compile confirms it.
  import SettingRow from "./SettingRow.svelte";
  import { readLivePatch, writeLivePatch } from "../../lib/build-preferences.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { userId = "anonymous" } = $props();

  const scope = () => ({ origin: globalThis.location?.origin || "", user: userId });
  let value = $state(false);
  $effect.pre(() => { value = readLivePatch(scope(), "latex"); });

  function choose(next) {
    value = writeLivePatch(scope(), "latex", next);
  }
</script>

<SettingRow id="render-latex-live-patch" title={m.livepatch_title()} description={m.livepatch_description()}>
  <input type="checkbox" class="checkbox" aria-label={m.livepatch_title()} checked={value}
         onchange={(event) => choose(event.currentTarget.checked)} />
</SettingRow>
