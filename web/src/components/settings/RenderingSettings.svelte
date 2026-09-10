<script>
  // How a Quarto project is rendered: the format, the profile and the
  // parameters, shared by everyone editing it, and the local preview that
  // shows the result on this computer.
  import SettingRow from "./SettingRow.svelte";
  import QuartoRenderOptions from "../QuartoRenderOptions.svelte";

  let { preview = null, options, viewing = null, onapplyoptions } = $props();
</script>

<SettingRow id="rendering-preview" title="Local preview" description="Quarto's own preview of this project, served by the local app. Not shared.">
  {#if preview?.state === "starting"}
    <span class="setting-value">Starting…</span>
  {:else if preview}
    <a class="anchor" href={preview.url} target="_blank" rel="noopener noreferrer">Open</a>
  {:else}
    <span class="setting-value">Not running</span>
  {/if}
</SettingRow>

<h4 class="settings-heading">Format and profile</h4>
<SettingRow id="rendering-options" title="Render options" stacked
            description="The output format, the Quarto profile, and any parameters the document takes.">
  <QuartoRenderOptions {options} disabled={Boolean(viewing)} onapply={onapplyoptions} />
</SettingRow>
