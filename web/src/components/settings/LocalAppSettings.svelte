<script>
  // The LibrePaper app on this computer: whether this browser is connected to
  // it, how to connect, and what it can do. Shared by LaTeX, which uses it for
  // an installed TeX, and Quarto, which uses it for every local render.
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/latex/local.js";

  let { sourceFormat = "", main = "", bindingId = "", onbindingid } = $props();
  const quarto = $derived(sourceFormat === "quarto");

  let local = $state(localBridge.status());
  $effect(() => localBridge.subscribe((status) => (local = status)));

  let address = $state(localBridge.address());
  let pairingCode = $state("");
  let connecting = $state(false);
  let doctor = $state("");
  let choosingFolder = $state(false);
  let entrypoint = $state("");
  $effect(() => { entrypoint = main; });

  const WORDS = {
    unknown: "Not checked yet.",
    unreachable: "No local app answers at this address.",
    denied: "This browser declined the local-network permission.",
    reachable: "Reachable, not yet connected.",
    unauthorized: "Connected, but this project is not authorized.",
    connected: "Connected.",
    incompatible: "Connected, but its version does not match this browser.",
  };
  const connected = $derived(local?.state === "connected");
  const canPair = $derived(["unauthorized", "reachable"].includes(local?.state));
  const tone = $derived(connected ? "good" : canPair || local?.state === "incompatible" ? "warn" : "off");

  async function connect() {
    if (!pairingCode || connecting) return;
    connecting = true;
    try {
      await localBridge.connect(pairingCode);
      pairingCode = "";
    } catch (error) {
      doctor = error?.message || "The pairing code was not accepted. Check the code and retry.";
    } finally {
      connecting = false;
    }
  }

  async function doctorReport() {
    try {
      const capabilities = await localBridge.capabilities({ rescan: true });
      doctor = JSON.stringify(capabilities, null, 2);
    } catch (error) {
      doctor = error?.message || "librepaper local doctor could not be reached";
    }
  }

  async function pair() {
    if (connecting) return;
    connecting = true;
    try {
      if (canPair) await localBridge.pairViaApp();
      else await localBridge.connectViaApp();
    }
    catch (error) { doctor = error?.message || "Could not open the companion permission window."; }
    finally { connecting = false; }
  }

  async function chooseFolder() {
    if (choosingFolder) return;
    choosingFolder = true;
    try {
      const result = await localBridge.chooseFolderBinding({ entrypoint: entrypoint.trim() });
      if (result?.id) onbindingid?.(result.id);
      entrypoint = result?.entrypoint || entrypoint;
      doctor = "Project folder connected. Its path stays on this computer.";
    } catch (error) { doctor = error?.message || "The companion could not choose a project folder."; }
    finally { choosingFolder = false; }
  }

  function openCompanion() { return pair(); }
</script>

<div id="local-status" class="setting-status" data-tone={tone}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title">{WORDS[local?.state] || WORDS.unknown}</div>
    <div class="setting-description">
      {#if connected}
        {#if quarto && local?.capabilities?.quarto?.tool?.available === false}
          Quarto was not found on this computer. Install Quarto, then check the local setup below.
        {:else}
          {quarto ? "Renders run on this computer, with your installed Quarto and packages." : "PDF builds can use the LaTeX installed on this computer."}
        {/if}
      {:else if local?.state === "unreachable"}
        Start the companion once; this page will reconnect automatically when it is available.
      {:else if local?.state === "denied"}
        Allow local-network access for this site, then retry. The browser is preventing the connection.
      {:else if local?.state === "unauthorized" || local?.state === "reachable"}
        The companion is running. Allow this site to use it, or use the advanced pairing code below.
      {:else}
        {local?.instructions || "The companion is not ready yet."}
      {/if}
    </div>
  </div>
  <div class="setting-control">
    {#if local?.state === "unreachable"}<button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting} onclick={openCompanion}>Open companion</button>{/if}
    {#if connected}
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.disconnect()}>Disconnect</button>
    {/if}
    {#if canPair}<button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting} onclick={pair}>{connecting ? "Waiting…" : "Enable local rendering"}</button>{/if}
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.retry()}>Retry</button>
  </div>
</div>

{#if !connected}
  <p class="setting-description local-install-help">
    Install the companion once, then return here:
    <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/install-companion.sh">Linux installer</a>,
    <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper_darwin_arm64.app.zip">macOS Apple silicon</a>,
    <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper_darwin_amd64.app.zip">macOS Intel</a>, or
    <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/install-companion.cmd">Windows setup</a>.
    <a href="https://github.com/LibrePaper/librepaper/blob/main/deploy/README.md" target="_blank" rel="noreferrer">Installation instructions</a>.
  </p>
{/if}

{#if !connected}
  <details id="local-pairing" class="setting-advanced">
    <summary>Advanced connection options</summary>
    <SettingRow title="Pairing code"
                description="Enter the one-time code printed by the companion if the permission window cannot open.">
    <input class="input input-sm setting-input" type="text" inputmode="numeric" aria-label="Pairing code"
           bind:value={pairingCode} placeholder="Code from the local app" />
    <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting || !pairingCode} onclick={connect}>Connect</button>
    </SettingRow>
  </details>
{/if}

<SettingRow id="local-address" title="Address" description="Keep the default unless you started the local app on another address or port.">
  <input class="input input-sm setting-input" type="text" aria-label="Local app address" value={address}
         oninput={(event) => (address = event.currentTarget.value)} onblur={() => localBridge.setAddress(address)} />
</SettingRow>
<p class="setting-description"><a href={`${local?.address || localBridge.address()}librepaper/local/v1/manage`} target="_blank" rel="noreferrer">Open companion settings</a></p>

{#if quarto}
  <SettingRow id="local-binding" title="Project folder" description="Use this folder for Quarto render and export jobs. Live preview uses the shared project workspace. Selecting a folder does not upload its contents.">
    <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" placeholder="main.qmd" bind:value={entrypoint} />
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={!connected || choosingFolder || !entrypoint.trim()} onclick={() => chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose project folder…"}</button>
  </SettingRow>
  <details class="setting-advanced">
    <summary>Advanced: use an existing binding</summary>
    <SettingRow title="Binding ID" description="For projects already configured with the companion CLI.">
      <input class="input input-sm setting-input" type="text" aria-label="Local Quarto binding ID" placeholder="binding ID" value={bindingId}
             onchange={(event) => onbindingid?.(event.currentTarget.value.trim())} />
    </SettingRow>
  </details>
{/if}

<details><summary>Details and troubleshooting</summary>
{#if local?.capabilities?.tools}
  <SettingRow id="local-tools" title="Available tools" stacked
              description="What the local app found on this computer. File access protection: {local.capabilities.confinement?.kind || 'none'}{local.capabilities.confinement?.reason ? ` (${local.capabilities.confinement.reason})` : ''}.">
    <table class="setting-table">
      <thead><tr><th>Tool</th><th>Version</th></tr></thead>
      <tbody>
        {#each Object.entries(local.capabilities.tools) as [tool, info] (tool)}
          <tr>
            <td>{tool}</td>
            <td>{info.available ? info.version || "available" : info.note || "not found"}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </SettingRow>
{/if}
<SettingRow id="local-doctor" title="Check local setup" stacked={Boolean(doctor)}
            description="Asks the app to look again at what it can use, and shows the full report. Useful when reporting a problem.">
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={doctorReport}>Check</button>
  {#if doctor}<pre class="setting-log">{doctor}</pre>{/if}
</SettingRow>

</details>
