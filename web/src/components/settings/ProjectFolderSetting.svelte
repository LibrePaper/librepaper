<script>
  import SettingRow from "./SettingRow.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import * as desktopClient from "../../lib/desktop/client.js";

  // The folder on this computer that live previews build in.
  let { sourceFormat = "", main = "", mayEdit = false, onbindingid } = $props();

  let folderError = $state("");
  let choosingFolder = $state(false);
  let entrypoint = $state("");
  const canChoose = $derived(mayEdit && !choosingFolder);

  $effect(() => { entrypoint = main; });

  async function chooseFolder() {
    if (!canChoose) return;
    folderError = "";
    choosingFolder = true;
    const requestedEntrypoint = entrypoint.trim();
    try {
      const result = await desktopClient.chooseFolderBinding({ entrypoint: requestedEntrypoint });
      if (mayEdit) {
        if (result?.id) onbindingid?.(result.id);
        entrypoint = result?.entrypoint || entrypoint;
      }
    } catch (error) { folderError = error?.message || "The desktop app could not choose a project folder."; }
    finally { choosingFolder = false; }
  }
</script>

<DesktopOnly feature="The project folder setting">
<SettingRow id="render-folder" title="Project folder" description="Folder used for live previews; one-shot builds use a temporary copy.">
  <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" bind:value={entrypoint} disabled={!canChoose} />
  <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={!canChoose || !entrypoint.trim()} onclick={() => void chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose folder…"}</button>
</SettingRow>
{#if folderError}<p class="setting-description folder-error" role="alert">{folderError}</p>{/if}
</DesktopOnly>

<style>
  .folder-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
</style>
