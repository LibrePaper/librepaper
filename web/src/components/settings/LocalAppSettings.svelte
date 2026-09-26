<script>
  import Modal from "../Modal.svelte";
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let {
    sourceFormat = "", main = "", mayEdit = false, onbindingid,
    localExecution = false, onlocalexecution,
  } = $props();
  const quarto = $derived(sourceFormat === "quarto");
  const projectBinding = $derived(["quarto", "typst", "markdown"].includes(sourceFormat));
  const local = $derived(companion.status);
  $effect(() => companion.watch());
  $effect(() => void localBridge.probe());

  let address = $state(localBridge.address());
  let addressDraft = $state(localBridge.address());
  let pairingCode = $state("");
  let connecting = $state(false);
  let copying = $state("");
  let doctor = $state("");
  let connectionError = $state("");
  let folderError = $state("");
  let copyError = $state(false);
  let manualPairingNeeded = $state(false);
  let choosingFolder = $state(false);
  let entrypoint = $state("");
  let detailsOpen = $state(false);
  let addressOpen = $state(false);
  let pairConfirmOpen = $state(false);

  $effect(() => { entrypoint = main; });
  $effect(() => {
    if (!connected) return;
    connectionError = "";
    manualPairingNeeded = false;
    copyError = false;
  });
  const connected = $derived(local?.state === "connected");
  const canPair = $derived(["unauthorized", "reachable"].includes(local?.state));
  const tone = $derived(connected ? "good" : canPair || ["denied", "incompatible"].includes(local?.state) ? "warn" : "off");
  const status = $derived(({ unknown: "Companion not connected", unreachable: "Companion not connected", denied: "Local network access blocked", reachable: "Companion found", unauthorized: "Companion needs permission", connected: "Companion connected", incompatible: "Companion needs an update" })[local?.state] || "Companion not connected");
  const installer = "https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh";
  const windowsInstaller = "https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1";
  const settingsUrl = $derived(`${local?.address || localBridge.address()}librepaper/local/manage`);

  async function connect() {
    if (!pairingCode || connecting) return;
    pairConfirmOpen = true;
  }

  async function confirmPairing() {
    if (!pairingCode || connecting) return;
    pairConfirmOpen = false;
    connecting = true;
    try {
      await localBridge.connect(pairingCode);
      pairingCode = "";
    } catch (error) {
      doctor = error?.message || "The pairing code was not accepted. Check the code and retry.";
      detailsOpen = true;
    } finally { connecting = false; }
  }

  async function pair() {
    if (connecting) return;
    connectionError = "";
    connecting = true;
    try {
      await localBridge.connectApp();
    } catch (error) {
      connectionError = error?.message || "Could not open the companion permission window.";
      manualPairingNeeded = true;
    }
    finally { connecting = false; }
  }

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

  async function copy(text, label) {
    try {
      await navigator.clipboard.writeText(text);
      copyError = false;
      copying = label;
      setTimeout(() => { if (copying === label) copying = ""; }, 1500);
    } catch { copyError = true; }
  }

  async function doctorReport() {
    detailsOpen = true;
    try {
      const capabilities = await localBridge.capabilities({ rescan: true });
      doctor = JSON.stringify(capabilities, null, 2);
    } catch (error) { doctor = error?.message || "Local setup check could not reach the companion."; }
  }

  function saveAddress() {
    localBridge.setAddress(addressDraft);
    address = addressDraft;
    addressOpen = false;
    void localBridge.retry();
  }

  function openAddress() {
    addressDraft = localBridge.address();
    addressOpen = true;
  }
</script>

<p class="setting-description local-intro">Connect LibrePaper to apps and tools installed on this computer, including coding agents, Zotero, Quarto, and local project folders.</p>

<div id="local-status" class="setting-status" data-tone={tone}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title" role="status">{status}</div>
    <div class="setting-description">
      {#if connected}
        Local tools are available to LibrePaper.
      {:else if local?.state === "denied"}
        Allow local-network access for this site in your browser, then retry.
      {:else if local?.state === "incompatible"}
        Install the latest companion version, then retry.
      {:else if local?.state === "unauthorized" || local?.state === "reachable"}
        The companion is running. Connect to approve access for this site.
      {:else if local?.state === "unreachable"}
        Open the companion after installing it, then retry here.
      {:else}
        Install and open the companion to use local tools.
      {/if}
    </div>
  </div>
  <div class="setting-control">
    {#if connected}
      <a class="btn btn-sm preset-outlined-surface-300-700" href={settingsUrl} target="_blank" rel="noreferrer">Open companion settings</a>
    {:else if canPair}
      <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting} onclick={pair}>{connecting ? "Waiting…" : "Connect companion"}</button>
    {/if}
    {#if !connected}<button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={connecting} onclick={() => void localBridge.retry()}>Retry</button>{/if}
  </div>
</div>
{#if connectionError}<p class="setting-description local-error" role="alert">{connectionError}</p>{/if}
{#if copyError}<p class="setting-description local-error" role="status">Copy failed. Select and copy the command manually.</p>{/if}

{#if !connected}
  <section id="local-install-help" class="local-install" aria-label="Install LibrePaper Companion">
    <h4 class="setting-title">Install LibrePaper Companion</h4>
    <div class="install-option">
      <div class="setting-title">macOS &amp; Linux</div>
      <div class="command-line"><code>curl --proto '=https' --tlsv1.2 -LsSf {installer} | sh</code><button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void copy(`curl --proto '=https' --tlsv1.2 -LsSf ${installer} | sh`, "macOS & Linux")}>{copying === "macOS & Linux" ? "Copied" : "Copy"}</button></div>
    </div>
    <div class="install-option">
      <div class="setting-title">Windows</div>
      <div class="command-line"><code>powershell -ExecutionPolicy Bypass -c "irm {windowsInstaller} | iex"</code><button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void copy(`powershell -ExecutionPolicy Bypass -c "irm ${windowsInstaller} | iex"`, "Windows")}>{copying === "Windows" ? "Copied" : "Copy"}</button></div>
    </div>
    <p class="setting-description">After installation, open LibrePaper Companion and return here to connect.</p>
    <a class="quiet-link" href="https://github.com/LibrePaper/librepaper/blob/main/deploy/README.md" target="_blank" rel="noreferrer">Installation help</a>
  </section>
{/if}

{#if !connected && manualPairingNeeded}
  <section id="local-pairing" class="pairing">
    <h4 class="setting-title">Pair manually</h4>
    <p class="setting-description">If the companion did not open a permission prompt, enter the code shown in the local app.</p>
    <div class="pair-controls">
      <input class="input input-sm setting-input" type="text" inputmode="numeric" aria-label="Pairing code" bind:value={pairingCode} placeholder="Pairing code" />
      <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting || !pairingCode} onclick={connect}>Connect</button>
    </div>
  </section>
{/if}

{#if connected && quarto && mayEdit}
  <SettingRow id="local-execution" title="Local code execution" description="Code runs on this computer with your user account’s permissions.">
    <span class="setting-description">Allow paired Quarto documents to run local code</span>
    <button type="button" role="switch" class="switch local-execution-switch" aria-label="Allow paired Quarto documents to run local code" aria-checked={localExecution} data-state={localExecution ? "checked" : "unchecked"} onclick={() => onlocalexecution?.(!localExecution)}>
      <span class="switch-thumb" data-state={localExecution ? "checked" : "unchecked"}></span>
    </button>
  </SettingRow>
{/if}

{#if connected && projectBinding}
  <SettingRow id="local-binding" title="Project folder" description="Use a folder on this computer for local builds and previews.">
    <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" placeholder={quarto ? "main.qmd" : sourceFormat === "typst" ? "main.typ" : "main.md"} bind:value={entrypoint} disabled={!mayEdit} />
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={!mayEdit || choosingFolder || !entrypoint.trim()} onclick={() => void chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose folder…"}</button>
  </SettingRow>
  {#if folderError}<p class="setting-description local-error" role="alert">{folderError}</p>{/if}
{/if}

<nav class="local-footer" aria-label="Local connection help">
  <button type="button" class="quiet-link" id="local-details" onclick={() => (detailsOpen = true)}>Connection details</button>
  <button type="button" class="quiet-link" id="local-doctor" onclick={() => void doctorReport()}>Check local setup</button>
  <a class="quiet-link" href="https://github.com/LibrePaper/librepaper/issues" target="_blank" rel="noreferrer">Troubleshooting</a>
</nav>

<Modal bind:open={detailsOpen} title="Connection details" wide>
  <div class="details-content">
    <SettingRow title="Status" description={status}>
      <span class="setting-description">{local?.address || address}</span>
    </SettingRow>
    {#if local?.version}<SettingRow title="Version"><span class="setting-description">{local.version}</span></SettingRow>{/if}
    {#if connected}<div class="details-actions"><button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.disconnect()}>Disconnect</button></div>{/if}
    {#if local?.capabilities?.tools}
      <div class="details-tools"><div class="setting-title">Available tools</div><table class="setting-table"><thead><tr><th>Tool</th><th>Version</th></tr></thead><tbody>
        {#each Object.entries(local.capabilities.tools) as [tool, info] (tool)}<tr><td>{tool}</td><td>{info.available ? info.version || "available" : info.note || "not found"}</td></tr>{/each}
      </tbody></table></div>
    {/if}
    <div class="details-actions">
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={openAddress}>Custom companion address</button>
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void doctorReport()}>Check local setup</button>
    </div>
    {#if doctor}<pre class="setting-log" role="status">{doctor}</pre>{/if}
  </div>
</Modal>

<Modal bind:open={addressOpen} title="Custom companion address" confirm={{ label: "Save address", onclick: saveAddress }}>
  <p class="setting-description">Use the default unless you started the companion at another address or port.</p>
  <label class="setting-title" for="local-address">Custom companion address</label>
  <input id="local-address" aria-label="Custom companion address" class="input input-sm setting-input" type="url" bind:value={addressDraft} />
</Modal>

<Modal bind:open={pairConfirmOpen} title="Allow local tools on this computer?" confirm={{ label: "Pair companion", onclick: confirmPairing }}>
  <p class="setting-description">Pairing lets this browser use tools made available by LibrePaper Companion. Pairing alone does not run document code; local code execution asks for permission separately.</p>
</Modal>

<style>
  .local-intro { max-width: 42rem; margin-block: 0 calc(var(--spacing) * 3); }
  .local-install { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 4); }
  .install-option { display: grid; gap: calc(var(--spacing) * 1.5); }
  .command-line { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-width: 0; }
  .command-line code { flex: 1; min-width: 0; overflow-wrap: anywhere; padding: calc(var(--spacing) * 2); border-radius: var(--radius-container); background: var(--color-surface-100-900); }
  .pairing { display: grid; gap: calc(var(--spacing) * 2); margin-block: calc(var(--spacing) * 4); }
  .pair-controls { display: flex; gap: calc(var(--spacing) * 2); }
  .pair-controls .setting-input { flex: 1; }
  .quiet-link { color: var(--panel-muted); font-size: var(--panel-meta-size); text-decoration: underline; text-underline-offset: 2px; background: none; border: 0; padding: 0; cursor: pointer; }
  .quiet-link:hover { color: var(--color-primary-500); }
  .local-footer { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 3); margin-top: calc(var(--spacing) * 5); }
  .local-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-500); }
  .local-execution-switch { appearance: none; border: 0; padding: 0; cursor: pointer; }
  .local-execution-switch:focus-visible { outline: 2px solid var(--color-primary-500); outline-offset: 2px; }
  .details-content { display: grid; gap: calc(var(--spacing) * 3); }
  .details-tools { display: grid; gap: calc(var(--spacing) * 2); }
  .details-actions { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }
  .setting-log { max-height: 18rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
  @media (max-width: 700px) { .command-line, .pair-controls { align-items: stretch; flex-direction: column; } }
</style>
