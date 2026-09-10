<script>
  // The LibrePaper app on this computer: whether this browser is connected to
  // it, how to connect, and what it can do. Shared by LaTeX, which uses it for
  // an installed TeX, and Quarto, which uses it for every local render.
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/latex/local.js";

  let { sourceFormat = "", bindingId = "", onbindingid } = $props();
  const quarto = $derived(sourceFormat === "quarto");

  let local = $state(localBridge.status());
  $effect(() => localBridge.subscribe((status) => (local = status)));

  let address = $state(localBridge.address());
  let pairingCode = $state("");
  let connecting = $state(false);
  let doctor = $state("");

  const WORDS = {
    unknown: "Not checked yet.",
    unreachable: "No local app answers at this address.",
    denied: "This browser declined the local-network permission.",
    reachable: "Reachable, not yet connected.",
    unauthorized: "Connected, but this project is not authorized.",
    connected: "Connected.",
    incompatible: "Connected, but its version does not match this browser.",
  };
  const paired = $derived(["connected", "unauthorized", "incompatible"].includes(local?.state));
  const tone = $derived(local?.state === "connected" ? "good" : paired || local?.state === "reachable" ? "warn" : "off");

  async function connect() {
    if (!pairingCode || connecting) return;
    connecting = true;
    try {
      await localBridge.connect(pairingCode);
      pairingCode = "";
    } catch {
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
</script>

<div id="local-status" class="setting-status" data-tone={tone}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title">{WORDS[local?.state] || WORDS.unknown}</div>
    <div class="setting-description">
      {#if paired}
        {quarto ? "Renders run on this computer, with your installed Quarto and packages." : "PDF builds can use the LaTeX installed on this computer."}
      {:else}
        Run <code>librepaper local start</code> in a terminal on this computer; a LibrePaper server running here already provides it.
        {#if local?.instructions}{local.instructions}{/if}
      {/if}
    </div>
  </div>
  <div class="setting-control">
    {#if paired}
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.disconnect()}>Disconnect</button>
    {/if}
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.retry()}>Retry</button>
  </div>
</div>

{#if !paired}
  <SettingRow id="local-pairing" title="Pairing code"
              description="The code the local app prints when it starts. Choosing a render from the menu asks the app to allow this site instead, without a code.">
    <input class="input input-sm setting-input" type="text" inputmode="numeric" aria-label="Pairing code"
           bind:value={pairingCode} placeholder="Code from the local app" />
    <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting || !pairingCode} onclick={connect}>Connect</button>
  </SettingRow>
{/if}

<SettingRow id="local-address" title="Address" description="Keep the default unless you started the local app on another address or port.">
  <input class="input input-sm setting-input" type="text" aria-label="Local app address" value={address}
         oninput={(event) => (address = event.currentTarget.value)} onblur={() => localBridge.setAddress(address)} />
</SettingRow>

{#if quarto}
  <SettingRow id="local-binding" title="Binding ID" description="Which of the local app's workspaces this project renders in. Filled in when the app pairs; change it only if the app tells you to.">
    <input class="input input-sm setting-input" type="text" aria-label="Local Quarto binding ID" placeholder="binding ID" value={bindingId}
           onchange={(event) => onbindingid?.(event.currentTarget.value.trim())} />
  </SettingRow>
{/if}

<h4 class="settings-heading">Diagnostics</h4>
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
