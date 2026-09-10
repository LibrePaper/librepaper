<script>
  // How speech becomes text at this browser. Offered to every reader who can
  // chat or comment, not only an editor, so nothing here is gated on a role.
  import SettingRow from "./SettingRow.svelte";
  import { getDictation } from "../../lib/dictation/service.js";
  import { readSettings, writeSettings } from "../../lib/dictation/settings.js";
  import { MODELS, modelById } from "../../lib/dictation/models.js";
  import { megabytes } from "./words.js";

  const storage = (() => {
    try {
      return typeof localStorage !== "undefined" ? localStorage : null;
    } catch {
      return null;
    }
  })();

  function load() {
    try {
      return readSettings(storage);
    } catch {
      return { backend: "local", model: "whisper-small", language: "auto", confirmed: [] };
    }
  }

  let dictation = $state(load());
  const LOCAL_MODELS = MODELS.filter((entry) => entry.kind === "local");
  const selectedModel = $derived(modelById(dictation.model) || LOCAL_MODELS[0]);

  function setBackend(backend) {
    dictation = { ...dictation, backend };
    writeSettings(storage, { backend });
  }

  function setModel(id) {
    dictation = { ...dictation, model: id, language: "auto" };
    writeSettings(storage, { model: id, language: "auto" });
  }

  function setLanguage(language) {
    dictation = { ...dictation, language };
    writeSettings(storage, { language });
  }

  function languageName(tag) {
    try {
      return new Intl.DisplayNames([navigator.language || "en"], { type: "language" }).of(tag) || tag;
    } catch {
      return tag;
    }
  }

  // The live service, so the row shows the model and device actually running
  // rather than only what was asked for -- the two differ right after a
  // change, until the next dictation start picks it up.
  let status = $state({ state: "idle", model: null, device: null });
  $effect(() => getDictation().subscribe((snapshot) => (status = snapshot)));

  const device = $derived(status.device === "webgpu" ? "WebGPU" : status.device === "wasm" ? "CPU (wasm)" : null);
  const loaded = $derived(status.model && device ? `${status.model.label} on ${device}` : "Nothing loaded yet");
</script>

<SettingRow id="dictation-backend" title="Speech recognition"
            description="On this device, audio never leaves your computer once the model has downloaded. The browser's own recognition may send audio to the browser vendor.">
  <select class="select setting-select" aria-label="Dictation backend" value={dictation.backend}
          onchange={(event) => setBackend(event.currentTarget.value)}>
    <option value="local">On this device</option>
    <option value="browser">Browser built-in</option>
  </select>
</SettingRow>

{#if dictation.backend === "local"}
  <SettingRow id="dictation-model" title="Model" description="Larger models are more accurate and slower to download.">
    <select class="select setting-select" aria-label="Dictation model" value={dictation.model}
            onchange={(event) => setModel(event.currentTarget.value)}>
      {#each LOCAL_MODELS as entry (entry.id)}
        <option value={entry.id}>{entry.label} — {megabytes(entry.sizeBytes)}, {entry.languages.length} languages</option>
      {/each}
    </select>
  </SettingRow>

  <SettingRow id="dictation-language" title="Language" description="Choose one when detection picks the wrong language.">
    <select class="select setting-select" aria-label="Dictation language" value={dictation.language}
            onchange={(event) => setLanguage(event.currentTarget.value)}>
      <option value="auto">Detect automatically</option>
      {#if selectedModel}
        {#each selectedModel.languages as tag (tag)}
          <option value={tag}>{languageName(tag)}</option>
        {/each}
      {/if}
    </select>
  </SettingRow>
{/if}

<SettingRow id="dictation-status" title="Loaded model" description="What the last dictation ran on. A change above applies the next time you dictate.">
  <span class="setting-value">{loaded}</span>
</SettingRow>
