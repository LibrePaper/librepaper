<script>
  // Editor preferences belong to this browser. Compiler choices are shared
  // with the project, while local-app pairing belongs to this device.
  import PanelHeader from "./PanelHeader.svelte";
  import * as latex from "../lib/latex.js";
  // Quarto uses the same paired local service as LaTeX, but its status is a
  // separate store. Subscribe to that store directly so opening Settings
  // reflects a Quarto connection immediately, even before a LaTeX controller
  // has ever been configured on this page.
  import * as localBridge from "../lib/latex/local.js";
  // Dictation is offered to every reader who can chat or comment, not just
  // an editor, so this section (unlike the LaTeX/Quarto ones above) is never
  // gated on `mayEdit`.
  import { getDictation } from "../lib/dictation/service.js";
  import { readSettings, writeSettings } from "../lib/dictation/settings.js";
  import { MODELS, modelById } from "../lib/dictation/models.js";
  import { removeCachedModel } from "../lib/dictation/purge.js";
  import { done } from "../lib/toast.svelte.js";

  let {
    keys = "default",
    sourceFormat = "",
    mayEdit = false,
    latexSettings = { engine: "auto", release: null },
    onkeys,
    onlatexsettings,
  } = $props();

  const showsLatex = $derived(sourceFormat === "latex" && mayEdit);
  const showsQuarto = $derived(sourceFormat === "quarto" && mayEdit);
  const showsLocal = $derived(showsLatex || showsQuarto);

  /* --------------------------------------------------------------- engine */

  function setEngine(engine) {
    onlatexsettings?.({ ...latexSettings, engine });
  }

  /* -------------------------------------------------------------- release */

  // The manifest's releases, fetched once this section is actually shown --
  // a reader with nothing to compile never asks the mirror for its catalog.
  let releases = $state(null);
  $effect(() => {
    if (!showsLatex) return;
    latex
      .releases()
      .then((answer) => (releases = answer))
      .catch(() => {});
  });

  // "Revert" needs the id a release update replaced, which is nowhere else
  // once `latexSettings.release` has moved on to the new one.
  let previousRelease = $state(null);
  function pinRelease(id) {
    previousRelease = latexSettings.release;
    onlatexsettings?.({ ...latexSettings, release: id });
  }

  /* ---------------------------------------------------------------- local */

  // The local bridge owns this status, independently of the LaTeX compiler
  // controller. Quarto can therefore update this panel even when no LaTeX
  // compile has been configured on the page.
  let local = $state(localBridge.status());
  $effect(() => {
    if (!showsLocal) return;
    return localBridge.subscribe((status) => (local = status));
  });

  let address = $state(localBridge.address());
  let pairingCode = $state("");
  let connecting = $state(false);
  let doctor = $state("");

  function setAddress() {
    localBridge.setAddress(address);
  }

  async function connect() {
    if (!pairingCode || connecting) return;
    connecting = true;
    try {
      await localBridge.connect(pairingCode);
      pairingCode = "";
    } catch {
      // The connection state itself, read from `local` above, already says
      // what went wrong; nothing further to add here.
    } finally {
      connecting = false;
    }
  }

  function disconnect() {
    void localBridge.disconnect();
  }

  async function doctorReport() {
    try {
      const capabilities = await localBridge.capabilities({ rescan: true });
      doctor = JSON.stringify(capabilities, null, 2);
    } catch (error) {
      doctor = error?.message || "librepaper local doctor could not be reached";
    }
  }

  const CONNECTION_WORDS = {
    unknown: "Not checked yet.",
    unreachable: "Local LibrePaper is unavailable.",
    denied: "This browser declined the local-network permission.",
    reachable: "Reachable, not yet connected.",
    unauthorized: "Connected app, but this project is not authorized.",
    connected: "Connected.",
    incompatible: "Connected, but its version does not match this browser.",
  };

  /* ---------------------------------------------------------------- cache */

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
  }

  function megabytes(bytes) {
    if (bytes == null) return "…";
    const mb = bytes / (1024 * 1024);
    return mb < 0.1 ? "nothing" : mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
  }

  /* ------------------------------------------------------------ dictation */

  // Storage read the way service.js reads it: a plain global that a locked
  // down frame can refuse to hand back, tolerated once here rather than at
  // every call site below.
  const dictationStorage = (() => {
    try {
      return typeof localStorage !== "undefined" ? localStorage : null;
    } catch {
      return null;
    }
  })();

  function loadDictationSettings() {
    try {
      return readSettings(dictationStorage);
    } catch {
      return { backend: "local", model: "whisper-small", language: "auto", confirmed: [] };
    }
  }

  let dictation = $state(loadDictationSettings());
  const LOCAL_MODELS = MODELS.filter((entry) => entry.kind === "local");
  const selectedModel = $derived(modelById(dictation.model) || LOCAL_MODELS[0]);

  function setBackend(backend) {
    dictation = { ...dictation, backend };
    writeSettings(dictationStorage, { backend });
  }

  function setModel(id) {
    dictation = { ...dictation, model: id, language: "auto" };
    writeSettings(dictationStorage, { model: id, language: "auto" });
  }

  function setLanguage(language) {
    dictation = { ...dictation, language };
    writeSettings(dictationStorage, { language });
  }

  function languageName(tag) {
    try {
      return new Intl.DisplayNames([navigator.language || "en"], { type: "language" }).of(tag) || tag;
    } catch {
      return tag;
    }
  }

  // The live service, so this panel shows the model and device actually
  // running rather than only what settings asks for -- the two can differ
  // right after a change, until the next dictation start picks it up.
  let dictationStatus = $state({ state: "idle", model: null, device: null });
  $effect(() => getDictation().subscribe((snapshot) => (dictationStatus = snapshot)));

  const deviceLabel = $derived(
    dictationStatus.device === "webgpu" ? "WebGPU" : dictationStatus.device === "wasm" ? "CPU (wasm)" : null
  );
  const dictationStatusLine = $derived(
    dictationStatus.model && deviceLabel ? `${dictationStatus.model.label} — ${deviceLabel}` : "Not loaded"
  );

  async function removeDictationModel() {
    if (!selectedModel) return;
    await removeCachedModel(selectedModel, { caches: typeof caches !== "undefined" ? caches : undefined });
    const confirmed = loadDictationSettings().confirmed.filter((id) => id !== selectedModel.id);
    writeSettings(dictationStorage, { confirmed });
    done("Model removed");
  }
</script>

<section class="panel settings-panel" aria-label="Settings">
  <PanelHeader title="Settings">
    <p class="panel-muted">Editor preferences apply only to this browser.</p>
  </PanelHeader>

  <section class="settings-section" aria-labelledby="settings-keys">
    <h3 id="settings-keys" class="panel-section-title">Editor keys</h3>
    <label class="settings-row">
      <input type="checkbox" class="checkbox" checked={keys === "vim"}
             onchange={(event) => onkeys?.(event.currentTarget.checked ? "vim" : "default")} />
      <span>Vim keys</span>
    </label>
    <p class="panel-meta">Applies to the source pane.</p>
  </section>

  <section class="settings-section" aria-labelledby="settings-dictation">
    <h3 id="settings-dictation" class="panel-section-title">Dictation</h3>
    <label class="settings-row">
      <span class="settings-label">Backend</span>
      <select class="select settings-select" aria-label="Dictation backend" value={dictation.backend}
              onchange={(event) => setBackend(event.currentTarget.value)}>
        <option value="local">On this device</option>
        <option value="browser">Browser built-in (may send audio to the browser vendor)</option>
      </select>
    </label>

    {#if dictation.backend === "local"}
      <label class="settings-row">
        <span class="settings-label">Model</span>
        <select class="select settings-select" aria-label="Dictation model" value={dictation.model}
                onchange={(event) => setModel(event.currentTarget.value)}>
          {#each LOCAL_MODELS as entry (entry.id)}
            <option value={entry.id}>{entry.label} — {megabytes(entry.sizeBytes)}, {entry.languages.length} languages</option>
          {/each}
        </select>
      </label>

      <label class="settings-row">
        <span class="settings-label">Language</span>
        <select class="select settings-select" aria-label="Dictation language" value={dictation.language}
                onchange={(event) => setLanguage(event.currentTarget.value)}>
          <option value="auto">Detect automatically</option>
          {#if selectedModel}
            {#each selectedModel.languages as tag (tag)}
              <option value={tag}>{languageName(tag)}</option>
            {/each}
          {/if}
        </select>
      </label>
    {/if}

    <p class="panel-muted">{dictationStatusLine}</p>

    <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
            disabled={!selectedModel} onclick={removeDictationModel}>
      Remove downloaded model
    </button>
    <p class="panel-meta">
      Speech recognition runs on this device once its model has downloaded;
      your audio never leaves it. This forgets the download, freeing space --
      it downloads again, with the same confirmation, the next time you dictate.
    </p>
  </section>

  {#if showsLatex}
    <details class="settings-advanced">
      <summary class="panel-section-title">Advanced LaTeX settings</summary>
      <p class="panel-muted">Use these if your document needs a specific compiler or you want to build PDFs with LaTeX installed on your computer. The default settings work for most documents.</p>
    <section class="settings-section" aria-labelledby="settings-latex-engine">
      <h3 id="settings-latex-engine" class="panel-section-title">PDF compiler</h3>
      <label class="settings-row">
        <span class="settings-label">Compiler</span>
        <select class="select settings-select" aria-label="Project engine" value={latexSettings.engine || "auto"}
                onchange={(event) => setEngine(event.currentTarget.value)}>
          <option value="auto">Automatic</option>
          <option value="pdflatex">pdfLaTeX</option>
          <option value="xelatex">XeLaTeX</option>
          <option value="lualatex">LuaLaTeX</option>
        </select>
      </label>
      <p class="panel-meta">
        Leave this on Automatic unless your template requires a particular
        compiler. This choice applies to everyone working on the document.
      </p>

      {#if releases}
        <div class="settings-row">
          {#if latexSettings.release && latexSettings.release !== releases.default}
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                    onclick={() => pinRelease(releases.default)}>
              Update browser compiler
            </button>
          {/if}
          {#if previousRelease && previousRelease !== latexSettings.release}
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                    onclick={() => pinRelease(previousRelease)}>
              Undo compiler update
            </button>
          {/if}
        </div>
      {/if}
    </section>

    </details>
  {/if}

  {#if showsLocal}
    <section class="settings-section" aria-labelledby="settings-local">
      <h3 id="settings-local" class="panel-section-title">{showsQuarto ? "Local Quarto app" : "Local compilation"}</h3>
      <p class="panel-muted">{showsQuarto
        ? "Optional. Connect the LibrePaper app on this computer to render Quarto projects locally."
        : "Optional. Connect the LibrePaper app on this computer to use your installed LaTeX tools, for example when a package is unavailable in the browser."}</p>
      <p class="panel-muted">{CONNECTION_WORDS[local?.state] || "Not checked yet."}</p>

      <details>
        <summary>Connection address</summary>
        <p class="panel-meta">Keep the default address unless you started the local app on a different address or port.</p>
      <label class="settings-row">
        <span class="settings-label">Address</span>
        <input class="input" type="text" value={address}
               oninput={(event) => (address = event.currentTarget.value)} onblur={setAddress} />
      </label>
      </details>

      {#if local?.state === "connected" || local?.state === "unauthorized" || local?.state === "incompatible"}
        <div class="settings-row">
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={disconnect}>
            Disconnect
          </button>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.retry()}>
            Retry connection
          </button>
        </div>
      {:else}
        <label class="settings-row">
          <span class="settings-label">Pairing code</span>
          <input class="input" type="text" inputmode="numeric" bind:value={pairingCode}
                 placeholder="Code from the local app" />
        </label>
        <div class="settings-row">
          <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting || !pairingCode}
                  onclick={connect}>
            Connect
          </button>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.retry()}>
            Retry connection
          </button>
        </div>
      {/if}
      <p class="panel-meta">Run <code>librepaper local start</code> in a terminal on your computer. Choosing Render locally then asks the app to allow this site in a window; entering the code it prints here does the same.</p>
      {#if local?.instructions}<p class="panel-meta">{local.instructions}</p>{/if}

      <details>
        <summary>Troubleshoot the connection</summary>
        <p class="panel-meta">Check which LaTeX tools the local app can use. These details can help when reporting a problem.</p>
      {#if local?.capabilities?.tools}
        <table class="settings-capabilities">
          <thead><tr><th>Tool</th><th>Version</th></tr></thead>
          <tbody>
            {#each Object.entries(local.capabilities.tools) as [tool, info]}
              <tr>
                <td>{tool}</td>
                <td>{info.available ? info.version || "available" : info.note || "not found"}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="panel-meta">
          File access protection: {local.capabilities.confinement?.kind || "none"}
          {#if local.capabilities.confinement?.reason}({local.capabilities.confinement.reason}){/if}
        </p>
      {/if}

      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={doctorReport}>
        Check local setup
      </button>
      {#if doctor}<pre class="settings-doctor">{doctor}</pre>{/if}
      </details>
    </section>
  {/if}

  {#if showsLatex}
    <section class="settings-section" aria-labelledby="settings-cache">
      <h3 id="settings-cache" class="panel-section-title">Downloaded LaTeX files</h3>
      <p class="panel-muted">Storage used: {megabytes(cacheSize)}. This browser saves compiler and package downloads to make future PDF builds faster.</p>
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={clearCache}>
        Remove downloaded files
      </button>
      <p class="panel-meta">Use this to free space or retry a broken download. Needed files will download again the next time you build a PDF. Your documents are kept.</p>
    </section>
  {/if}
</section>

<style>
  .settings-advanced > summary { cursor: pointer; margin-bottom: calc(var(--spacing) * 2); }
  .settings-advanced > p { margin-bottom: var(--panel-section-gap); }
  .settings-section summary { cursor: pointer; }
  .settings-section { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); margin-bottom: var(--panel-section-gap); }
  .settings-row { display: flex; align-items: center; gap: calc(var(--spacing) * 2); flex-wrap: wrap; }
  .settings-label { min-width: calc(var(--spacing) * 12); }
  .settings-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); max-width: 100%; }
  .settings-capabilities { font-size: 0.875rem; border-collapse: collapse; }
  .settings-capabilities th, .settings-capabilities td { text-align: left; padding: calc(var(--spacing) * 1) calc(var(--spacing) * 2) calc(var(--spacing) * 1) 0; }
  .settings-doctor { font-size: 0.75rem; max-height: 12rem; overflow: auto; white-space: pre-wrap; word-break: break-word; }
</style>
