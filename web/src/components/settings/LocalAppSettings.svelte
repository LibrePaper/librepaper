<script>
  // The LibrePaper app on this computer: whether this browser is connected to
  // it, how to connect, and what it can do, whatever format is open.
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { sourceFormat = "", main = "", mayEdit = false, onbindingid } = $props();
  const quarto = $derived(sourceFormat === "quarto");
  const projectBinding = $derived(["quarto", "typst", "markdown"].includes(sourceFormat));

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  // The pane about the local app is the one place where looking for it needs
  // no other excuse; every other surface waits for a gesture that needs it.
  $effect(() => void localBridge.probe());

  let address = $state(localBridge.address());
  let pairingCode = $state("");
  let acceptsCodeExecution = $state(false);
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
      doctor = error?.message || "librepaper local status could not be reached";
    }
  }

  async function pair() {
    if (connecting) return;
    connecting = true;
    try {
      await localBridge.connectApp();
    }
    catch (error) { doctor = error?.message || "Could not open the companion permission window."; }
    finally { connecting = false; }
  }

  async function chooseFolder() {
    if (!mayEdit || choosingFolder) return;
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

<section id="local-install-help" class="setting-description local-install-help" aria-label="About and install the LibrePaper Companion">
  <p>The LibrePaper Companion runs on your computer and connects this browser to local services. It can make installed coding agents such as Claude Code, Codex, pi, and OpenCode available to work with a project, search your Zotero library, and run local tools such as Quarto. Those tools must be installed separately.</p>
  <p>Continuous one-way sync (backup) to a directory on your file system is not available yet. Project folders currently support local builds and previews.</p>
</section>

<div id="local-status" class="setting-status" data-tone={tone}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title" role="status">{WORDS[local?.state] || WORDS.unknown}</div>
    <div class="setting-description">
      {#if connected}
        The companion is paired with this project. It can use tools installed on this computer, including Quarto, R, Python, TeX, and Zotero.
      {:else if local?.state === "unreachable"}
        Start the companion, then retry the connection here.
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
    {#if canPair}<button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting} onclick={pair}>{connecting ? "Waiting…" : "Connect companion"}</button>{/if}
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void localBridge.retry()}>Retry</button>
  </div>
</div>

<section class="local-install-help setting-description" aria-label="Install and run the companion">
  <h4 class="setting-title">Install and run</h4>
  <p>Install the companion on this computer, then start it once. Choose your operating system:</p>
  <ul>
    <li>Linux: save and run the <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/install-companion.sh">Linux installer</a> with <code>sh install-companion.sh</code>; it adds LibrePaper to the applications menu.</li>
    <li>macOS: download the <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper_darwin_arm64.app.zip">macOS Apple silicon</a> or <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper_darwin_amd64.app.zip">macOS Intel</a> app, unzip it, move it to <code>~/Applications</code>, and open it. Quit the current app before replacing it with an update.</li>
    <li>Windows: run <a href="https://github.com/LibrePaper/librepaper/releases/latest/download/install-companion.cmd">Windows setup</a> (<code>install-companion.cmd</code>); it installs the companion and creates a settings shortcut.</li>
  </ul>
  <p>Use <strong>Start at login</strong> in companion settings if you want it to launch automatically. <a href="https://github.com/LibrePaper/librepaper/blob/main/deploy/README.md" target="_blank" rel="noreferrer">Full installation instructions</a>.</p>
</section>

{#if !connected}
  <details id="local-pairing" class="setting-advanced">
    <summary>Advanced connection options</summary>
    <label class="setting-description">
      <input type="checkbox" bind:checked={acceptsCodeExecution} />
      I understand that paired Quarto documents can execute arbitrary code on this computer with my user account's access.
    </label>
    <SettingRow title="Pairing code"
                description="Enter the one-time code printed by the companion if the permission window cannot open.">
    <input class="input input-sm setting-input" type="text" inputmode="numeric" aria-label="Pairing code"
           bind:value={pairingCode} placeholder="Code from the local app" />
    <button type="button" class="btn btn-sm preset-filled-primary-500" disabled={connecting || !pairingCode || !acceptsCodeExecution} onclick={connect}>Connect</button>
    </SettingRow>
  </details>
{/if}

<SettingRow id="local-address" title="Address" description="Keep the default unless you started the local app on another address or port.">
  <input class="input input-sm setting-input" type="text" aria-label="Local app address" value={address}
         oninput={(event) => (address = event.currentTarget.value)} onblur={() => localBridge.setAddress(address)} />
</SettingRow>
<p class="setting-description"><a href={`${local?.address || localBridge.address()}librepaper/local/manage`} target="_blank" rel="noreferrer">Open companion settings</a></p>

{#if projectBinding}
  <SettingRow id="local-binding" title="Project folder" description="Use this folder for local project builds and previews. Selecting a folder does not upload its contents.">
    <input class="input input-sm setting-input" type="text" aria-label="Project entrypoint" placeholder={quarto ? "main.qmd" : sourceFormat === "typst" ? "main.typ" : "main.md"} bind:value={entrypoint} disabled={!mayEdit} />
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" disabled={!mayEdit || !connected || choosingFolder || !entrypoint.trim()} onclick={() => chooseFolder()}>{choosingFolder ? "Choosing…" : "Choose project folder…"}</button>
  </SettingRow>
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

<style>
  .local-install-help { display: grid; gap: calc(var(--spacing) * 3); margin-block: calc(var(--spacing) * 3); }
  .local-install-help ul { list-style: disc; padding-left: calc(var(--spacing) * 5); display: grid; gap: calc(var(--spacing) * 2); }
</style>
