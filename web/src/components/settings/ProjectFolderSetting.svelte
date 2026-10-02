<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { sourceFormat = "", main = "", mayEdit = false, onbindingid } = $props();
  const local = $derived(companion.status);
  $effect(() => companion.watch());
  // The companion pane handles probing; showing this pane should not trigger
  // a local network access request on its own.
  const quarto = $derived(sourceFormat === "quarto");
  const connected = $derived(local?.state === "connected");

  let folderError = $state("");
  let choosingFolder = $state(false);
  let entrypoint = $state("");

  $effect(() => { entrypoint = main; });

  async function chooseFolder() {
    if (!mayEdit || choosingFolder) return;
    folderError = "";
    choosingFolder = true;
    try {
      const result = await localBridge.chooseFolderBinding({ entrypoint: entrypoint.trim() });
      if (result?.id) onbindingid?.(result.id);
      entrypoint = result?.entrypoint || entrypoint;
    } catch (error) { folderError = error?.message || "The companion could not choose a project folder."; }
    finally { choosingFolder = false; }
  }
</script>

{#if connected}
  <SettingRow id="render-folder" title="Project folder" description="Use a folder on this computer for live previews. One-shot builds run in a temporary copy.">
    <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" placeholder={quarto ? "main.qmd" : sourceFormat === "typst" ? "main.typ" : "main.md"} bind:value={entrypoint} disabled={!mayEdit} />
    <button type="button" class="btn btn-sm lp-control-outline" disabled={!mayEdit || choosingFolder || !entrypoint.trim()} onclick={() => void chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose folder…"}</button>
  </SettingRow>
  {#if folderError}<p class="setting-description folder-error" role="alert">{folderError}</p>{/if}
{/if}

<style>
  .folder-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
</style>
