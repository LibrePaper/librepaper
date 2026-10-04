<script>
  import SettingRow from "./SettingRow.svelte";
  import ConnectionRow from "./ConnectionRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  // The folder on this computer that live previews build in. It watches the
  // companion's status and never probes for it: opening the Render page must
  // not make the browser ask for local network access.
  let { sourceFormat = "", main = "", mayEdit = false, onbindingid } = $props();
  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const quarto = $derived(sourceFormat === "quarto");
  const connected = $derived(local?.state === "connected");

  let folderError = $state("");
  let choosingFolder = $state(false);
  let entrypoint = $state("");
  const canChoose = $derived(mayEdit && connected && !choosingFolder);

  $effect(() => { entrypoint = main; });

  async function chooseFolder() {
    if (!canChoose) return;
    folderError = "";
    choosingFolder = true;
    const requestedEntrypoint = entrypoint.trim();
    const requestedAddress = local?.address || "";
    const requestedInstance = local?.instance || "";
    try {
      const result = await localBridge.chooseFolderBinding({ entrypoint: requestedEntrypoint });
      if (connected && mayEdit && (local?.address || "") === requestedAddress && (local?.instance || "") === requestedInstance) {
        if (result?.id) onbindingid?.(result.id);
        entrypoint = result?.entrypoint || entrypoint;
      }
    } catch (error) { folderError = error?.message || "The companion could not choose a project folder."; }
    finally { choosingFolder = false; }
  }
</script>

{#if !connected}
  <ConnectionRow needs="project folders" />
{/if}
<SettingRow id="render-folder" title="Project folder" scope="This computer" description="Folder used for live previews; one-shot builds use a temporary copy.">
  <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" placeholder={quarto ? "main.qmd" : sourceFormat === "typst" ? "main.typ" : "main.md"} bind:value={entrypoint} disabled={!canChoose} />
  <button type="button" class="btn btn-sm lp-control-outline" disabled={!canChoose || !entrypoint.trim()} onclick={() => void chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose folder…"}</button>
</SettingRow>
{#if folderError}<p class="setting-description folder-error" role="alert">{folderError}</p>{/if}

<style>
  .folder-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
</style>
