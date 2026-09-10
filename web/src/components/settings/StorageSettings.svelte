<script>
  // What this browser has downloaded and kept, and how to forget it: the
  // LaTeX compiler and packages, and the speech models. Nothing here touches
  // a document.
  import SettingRow from "./SettingRow.svelte";
  import * as latex from "../../lib/latex.js";
  import { readSettings, writeSettings } from "../../lib/dictation/settings.js";
  import { MODELS } from "../../lib/dictation/models.js";
  import { removeCachedModel } from "../../lib/dictation/purge.js";
  import { done } from "../../lib/toast.svelte.js";
  import { megabytes } from "./words.js";

  let { sourceFormat = "", mayEdit = false } = $props();
  const showsLatex = $derived(sourceFormat === "latex" && mayEdit);

  /* ---------------------------------------------------------------- latex */

  let cacheSize = $state(null);
  $effect(() => {
    if (!showsLatex) return;
    latex.resources
      .size()
      .then((bytes) => (cacheSize = bytes))
      .catch(() => {});
  });

  async function clearCache() {
    await latex.resources.clear();
    cacheSize = await latex.resources.size().catch(() => cacheSize);
    done("Downloaded LaTeX files removed");
  }

  /* ------------------------------------------------------------ dictation */

  const storage = (() => {
    try {
      return typeof localStorage !== "undefined" ? localStorage : null;
    } catch {
      return null;
    }
  })();

  function confirmedModels() {
    try {
      return readSettings(storage).confirmed;
    } catch {
      return [];
    }
  }

  const LOCAL_MODELS = MODELS.filter((entry) => entry.kind === "local");
  // A model whose download was confirmed is one this browser has, or had
  // until it was removed here.
  let downloaded = $state(confirmedModels());

  async function removeModel(entry) {
    await removeCachedModel(entry, { caches: typeof caches !== "undefined" ? caches : undefined });
    downloaded = confirmedModels().filter((id) => id !== entry.id);
    writeSettings(storage, { confirmed: downloaded });
    done("Model removed");
  }
</script>

{#if showsLatex}
  <h4 class="settings-heading">LaTeX</h4>
  <SettingRow id="storage-latex" title="Downloaded LaTeX files"
              description="{cacheSize === null ? 'Measuring…' : cacheSize < 100_000 ? 'Nothing downloaded yet.' : 'Storage used: ' + megabytes(cacheSize) + '.'} The compiler and packages are kept so later PDF builds are faster; removing them frees the space, and what a build needs downloads again. Your documents are kept.">
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={clearCache}>Remove</button>
  </SettingRow>
{/if}

<h4 class="settings-heading">Speech models</h4>
{#each LOCAL_MODELS as entry (entry.id)}
  <SettingRow id={entry.id === LOCAL_MODELS[0].id ? "storage-dictation" : undefined} title={entry.label}
              description="{megabytes(entry.sizeBytes)}, {entry.languages.length} languages. {downloaded.includes(entry.id) ? 'Downloaded. Removing it frees the space; it downloads again, with the same confirmation, the next time you dictate with it.' : 'Not downloaded.'}">
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
            disabled={!downloaded.includes(entry.id)} onclick={() => removeModel(entry)}>Remove</button>
  </SettingRow>
{/each}
