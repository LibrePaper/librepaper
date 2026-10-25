<script>
  // Where documents of one format render, for every document in this browser:
  // in the browser, or on this computer with the commands set below. This
  // computer is offered only in the desktop app. Quarto has no browser engine,
  // so its section shows the commands alone.
  import SettingRow from "./SettingRow.svelte";
  import ToolCommand from "./ToolCommand.svelte";
  import { LOCAL_OUTPUTS, desktop } from "../../lib/desktop/client.js";
  import { defaults, read, update } from "../../lib/build-preferences.js";

  let { format, userId = "anonymous", onpreferences = undefined } = $props();

  const NAMES = { latex: "LaTeX", typst: "Typst", markdown: "Markdown", quarto: "Quarto" };
  const hasDesktop = Boolean(desktop());
  const scope = () => ({ origin: globalThis.location?.origin || "", user: userId });
  /** @type {import("../../lib/reader/build-settings.svelte.js").BuildPreference} */
  let preference = $state(defaults());
  $effect.pre(() => { preference = read(scope(), format); });

  const local = $derived(preference.backend === "local");

  function choose(value) {
    preference = update(scope(), format, { backend: value });
    onpreferences?.(format, preference);
  }
</script>

{#if hasDesktop}
  {#if format !== "quarto"}
    <SettingRow id={`render-${format}-where`} title="Renders in"
                description={`${NAMES[format]} documents render in this browser, or on this computer while Run code is on.`}>
      <select class="select setting-select" aria-label={`${NAMES[format]} renders in`} value={local ? "local" : "browser"}
              onchange={(event) => choose(event.currentTarget.value)}>
        <option value="browser">Browser</option>
        <option value="local">This computer</option>
      </select>
    </SettingRow>
  {/if}
  {#if local}
    {#each LOCAL_OUTPUTS[format] as output (output)}
      <ToolCommand {format} {output} />
    {/each}
    <p class="settings-section-note">The tool may download packages on its own.</p>
  {/if}
{/if}
