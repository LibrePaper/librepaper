<script>
  // This browser's own preferences, in one place -- plus, for a LaTeX
  // document an editor may change, the project's own compile settings and
  // this device's local-compilation connection. Everything above the LaTeX
  // section is unchanged: the keys the editor answers to, whether a click in
  // one pane takes the other along, how the window is divided. Each of those
  // lives in localStorage under the reader's existing keys; this panel only
  // shows and sets what the reader already remembers.
  //
  // The LaTeX section is the replacement for the old distribution chooser
  // (docs/specs/latex-compiler.md "Remove the distribution chooser and its book icon.
  // Settings contains: project engine, pinned release, local connection,
  // compiler cache."). Engine and release are project settings -- they
  // travel with the document through `session.setLatexSettings` and change
  // the compile for every collaborator. Local connection and cache are this
  // device's own, read straight from `latex.js`.
  import PanelHeader from "./PanelHeader.svelte";
  import { RATIOS } from "../lib/panes.js";
  import * as latex from "../lib/latex.js";

  let {
    keys = "default",
    linked = false,
    sourceSide = "left",
    ratio = 1 / 2,
    sourceFormat = "",
    mayEdit = false,
    latexSettings = { engine: "auto", release: null },
    onkeys,
    onlinked,
    onside,
    onratio,
    onlatexsettings,
  } = $props();

  const showsLatex = $derived(sourceFormat === "latex" && mayEdit);

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

  // `latex.subscribe` carries `local` as part of the whole compile status
  // (section 2.6 of the interfaces doc), so this reads the same store
  // `LatexStatus.svelte` does rather than polling `latex.local.status()`
  // itself.
  let local = $state(latex.status().local);
  $effect(() => {
    if (!showsLatex) return;
    return latex.subscribe((status) => (local = status.local));
  });

  let address = $state(latex.local.address());
  let pairingCode = $state("");
  let connecting = $state(false);
  let doctor = $state("");

  function setAddress() {
    latex.local.setAddress(address);
  }

  async function connect() {
    if (!pairingCode || connecting) return;
    connecting = true;
    try {
      await latex.local.connect(pairingCode);
      pairingCode = "";
    } catch {
      // The connection state itself, read from `local` above, already says
      // what went wrong; nothing further to add here.
    } finally {
      connecting = false;
    }
  }

  function disconnect() {
    latex.local.disconnect();
  }

  async function doctorReport() {
    try {
      const capabilities = await latex.local.capabilities({ rescan: true });
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
</script>

<section class="panel settings-panel" aria-label="Settings">
  <PanelHeader title="Settings">
    <p class="panel-muted">These are this browser's own; they change nothing for anyone else.</p>
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

  <section class="settings-section" aria-labelledby="settings-linked">
    <h3 id="settings-linked" class="panel-section-title">Keep in step</h3>
    <label class="settings-row">
      <input type="checkbox" class="checkbox" checked={linked}
             onchange={(event) => onlinked?.(event.currentTarget.checked)} />
      <span>Keep in step</span>
    </label>
    <p class="panel-meta">A click in the document opens the place in the source it came from.</p>
  </section>

  <section class="settings-section" aria-labelledby="settings-panes">
    <h3 id="settings-panes" class="panel-section-title">Source pane</h3>
    <label class="settings-row">
      <span class="settings-label">Side</span>
      <select class="select settings-select" aria-label="Which side the source is on" value={sourceSide}
              onchange={(event) => onside?.(event.currentTarget.value)}>
        <option value="left">Left</option>
        <option value="right">Right</option>
      </select>
    </label>
    <label class="settings-row">
      <span class="settings-label">Split</span>
      <!-- The same ratios a drag sticks to, for anyone who never finds that
           it does. A split left somewhere between them shows as none. -->
      <select class="select settings-select" aria-label="How the source and the document share the window"
              value={RATIOS.some((one) => one.share === ratio) ? String(ratio) : ""}
              onchange={(event) => onratio?.(Number(event.currentTarget.value))}>
        {#if !RATIOS.some((one) => one.share === ratio)}<option value="" disabled>Custom</option>{/if}
        {#each RATIOS as one}
          <option value={String(one.share)}>{one.says}</option>
        {/each}
      </select>
    </label>
  </section>

  {#if showsLatex}
    <section class="settings-section" aria-labelledby="settings-latex-engine">
      <h3 id="settings-latex-engine" class="panel-section-title">LaTeX</h3>
      <label class="settings-row">
        <span class="settings-label">Project engine</span>
        <select class="select settings-select" aria-label="Project engine" value={latexSettings.engine || "auto"}
                onchange={(event) => setEngine(event.currentTarget.value)}>
          <option value="auto">Automatic</option>
          <option value="pdflatex">pdfLaTeX</option>
          <option value="xelatex">XeLaTeX</option>
          <option value="lualatex">LuaLaTeX</option>
        </select>
      </label>
      <p class="panel-meta">
        Applies to every collaborator. Automatic follows the document's own
        <code>%!TEX program</code> line or its packages, and falls back to pdfLaTeX.
      </p>

      <div class="settings-row">
        <span class="settings-label">Browser release</span>
        <span class="panel-muted">
          {latexSettings.release || (releases ? `default (${releases.default})` : "default")}
        </span>
      </div>
      {#if releases}
        <div class="settings-row">
          {#if latexSettings.release && latexSettings.release !== releases.default}
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                    onclick={() => pinRelease(releases.default)}>
              Update to {releases.default}
            </button>
          {/if}
          {#if previousRelease && previousRelease !== latexSettings.release}
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                    onclick={() => pinRelease(previousRelease)}>
              Revert
            </button>
          {/if}
        </div>
      {/if}
    </section>

    <section class="settings-section" aria-labelledby="settings-local">
      <h3 id="settings-local" class="panel-section-title">Local compilation</h3>
      <p class="panel-muted">{CONNECTION_WORDS[local?.state] || "Not checked yet."}</p>

      <label class="settings-row">
        <span class="settings-label">Address</span>
        <input class="input" type="text" value={address}
               oninput={(event) => (address = event.currentTarget.value)} onblur={setAddress} />
      </label>

      {#if local?.state === "connected" || local?.state === "unauthorized" || local?.state === "incompatible"}
        <div class="settings-row">
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={disconnect}>
            Disconnect
          </button>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => latex.local.retry()}>
            Retry connection
          </button>
        </div>
      {:else}
        <label class="settings-row">
          <span class="settings-label">Pairing code</span>
          <input class="input" type="text" inputmode="numeric" bind:value={pairingCode}
                 placeholder="printed by librepaper local start" />
        </label>
        <div class="settings-row">
          <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting || !pairingCode}
                  onclick={connect}>
            Connect
          </button>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => latex.local.retry()}>
            Retry connection
          </button>
        </div>
      {/if}
      <p class="panel-meta">{local?.instructions || "Run `librepaper local start` and enter the pairing code it prints."}</p>

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
          Confinement: {local.capabilities.confinement?.kind || "none"}
          {#if local.capabilities.confinement?.reason}({local.capabilities.confinement.reason}){/if}
        </p>
      {/if}

      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={doctorReport}>
        librepaper local doctor
      </button>
      {#if doctor}<pre class="settings-doctor">{doctor}</pre>{/if}
    </section>

    <section class="settings-section" aria-labelledby="settings-cache">
      <h3 id="settings-cache" class="panel-section-title">Compiler cache</h3>
      <p class="panel-muted">{megabytes(cacheSize)} of engine and package files cached in this browser.</p>
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={clearCache}>
        Clear compiler cache
      </button>
      <p class="panel-meta">Clears only the compiler's own files. Your documents are never touched.</p>
    </section>
  {/if}
</section>

<style>
  .settings-section { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); margin-bottom: var(--panel-section-gap); }
  .settings-row { display: flex; align-items: center; gap: calc(var(--spacing) * 2); flex-wrap: wrap; }
  .settings-label { min-width: calc(var(--spacing) * 12); }
  .settings-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); max-width: 100%; }
  .settings-capabilities { font-size: 0.875rem; border-collapse: collapse; }
  .settings-capabilities th, .settings-capabilities td { text-align: left; padding: calc(var(--spacing) * 1) calc(var(--spacing) * 2) calc(var(--spacing) * 1) 0; }
  .settings-doctor { font-size: 0.75rem; max-height: 12rem; overflow: auto; white-space: pre-wrap; word-break: break-word; }
</style>
