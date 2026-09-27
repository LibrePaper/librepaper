<script>
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { name = "quarto" } = $props();

  let integrationSettings = $state(null);
  let settingsError = $state("");
  let pendingDialogAction = $state("");
  let editingPath = $state("");
  let editingArgs = $state("");
  let defaultPath = $state("");
  let defaultArgs = $state([]);

  const local = $derived(companion.status);
  $effect(() => companion.watch());

  $effect(() => {
    void loadSettings();
  });

  function quoteAwareJoin(args) {
    if (!Array.isArray(args)) return "";
    return args.map((arg) => {
      if (arg.includes(" ") || arg.includes('"') || arg.includes("'")) {
        return `"${arg.replace(/"/g, '\\"')}"`;
      }
      return arg;
    }).join(" ");
  }

  function quoteAwareSplit(text) {
    const args = [];
    let current = "";
    let inQuote = false;
    let quoteChar = "";
    let i = 0;
    while (i < text.length) {
      const ch = text[i];
      if ((ch === '"' || ch === "'") && (i === 0 || text[i - 1] !== "\\")) {
        if (inQuote && ch === quoteChar) {
          inQuote = false;
          quoteChar = "";
        } else if (!inQuote) {
          inQuote = true;
          quoteChar = ch;
        } else {
          current += ch;
        }
      } else if (ch === " " && !inQuote) {
        if (current) args.push(current);
        current = "";
      } else {
        if (ch === "\\" && i + 1 < text.length && (text[i + 1] === '"' || text[i + 1] === "'")) {
          current += text[i + 1];
          i++;
        } else {
          current += ch;
        }
      }
      i++;
    }
    if (current) args.push(current);
    return args;
  }

  async function loadSettings() {
    try {
      integrationSettings = await localBridge.settings();
      const integration = integrationSettings?.integrations?.[name];
      if (integration) {
        defaultPath = integration.path || "";
        defaultArgs = integration.args || [];
        editingPath = integration.path || "";
        editingArgs = quoteAwareJoin(integration.args || []);
      }
    } catch (error) {
      settingsError = error?.message || "Could not load companion settings.";
    }
  }

  async function saveIntegration() {
    if (!editingPath && !editingArgs.trim()) {
      settingsError = "Either a path or arguments must be specified.";
      return;
    }
    const args = editingArgs.trim() ? quoteAwareSplit(editingArgs) : [];
    pendingDialogAction = `save-${name}`;
    try {
      const result = await localBridge.setIntegration(name, { path: editingPath || null, args });
      defaultPath = result.path || "";
      defaultArgs = result.args || [];
      editingPath = defaultPath;
      editingArgs = quoteAwareJoin(defaultArgs);
      settingsError = "";
    } catch (error) {
      const message = error?.message || `Could not update ${name} settings.`;
      if (message.includes("403") || message.includes("Refused")) {
        settingsError = `You declined the change in the dialog on your computer.`;
      } else if (message.includes("503")) {
        settingsError = `The companion could not save the changes.`;
      } else {
        settingsError = message;
      }
    } finally {
      pendingDialogAction = "";
    }
  }

  function resetToDefault() {
    editingPath = defaultPath;
    editingArgs = quoteAwareJoin(defaultArgs);
  }

  const capability = $derived.by(() => {
    if (!local?.capabilities) return null;
    if (name === "quarto") return local.capabilities?.quarto;
    if (name === "calepin") return local.capabilities?.calepin;
    if (name === "zotero") return local.capabilities?.zotero;
    return null;
  });

  const isConnected = $derived(local?.state === "connected");
  const showFields = $derived(name !== "zotero" && isConnected);
</script>

{#if settingsError}<p class="setting-description integration-error" role="alert">{settingsError}</p>{/if}

{#if !isConnected}
  <SettingRow id={`integration-${name}-status`} title={name === "quarto" ? "Quarto" : name === "calepin" ? "Calepin" : "Zotero"} description="Integration status">
    <span class="setting-description">Connect the local companion to configure {name}.</span>
  </SettingRow>
{:else}
  <SettingRow id={`integration-${name}-status`} title={name === "quarto" ? "Quarto" : name === "calepin" ? "Calepin" : "Zotero"} description="Integration status">
    {#if capability?.available}
      <span class="setting-description">{capability.version ? `${name} ${capability.version}` : `${name} available`}</span>
    {:else}
      <span class="setting-description">{capability?.note || `${name} not found on this computer`}</span>
    {/if}
  </SettingRow>

  {#if showFields}
    <SettingRow id={`integration-${name}-executable`} title="Executable" description="Path to the executable or folder containing it. Leave blank to use PATH.">
      <input class="input input-sm setting-input" type="text" aria-label="Executable path" placeholder="Found on PATH" bind:value={editingPath} disabled={pendingDialogAction} />
    </SettingRow>

    <SettingRow id={`integration-${name}-arguments`} title="Arguments" description="Extra command-line arguments passed when running this tool.">
      <textarea class="input input-sm setting-input" aria-label="Arguments" rows="2" bind:value={editingArgs} disabled={pendingDialogAction}></textarea>
    </SettingRow>

    <div class="integration-actions">
      <button type="button" class="btn btn-sm lp-control-brand" disabled={pendingDialogAction || (editingPath === defaultPath && quoteAwareJoin(quoteAwareSplit(editingArgs)) === quoteAwareJoin(defaultArgs))} onclick={() => void saveIntegration()}>
        {pendingDialogAction === `save-${name}` ? "Confirm on this computer…" : "Save"}
      </button>
      <button type="button" class="btn btn-sm lp-control-outline" disabled={pendingDialogAction} onclick={resetToDefault}>Reset to default</button>
    </div>
    {#if pendingDialogAction === `save-${name}`}
      <p class="setting-description">Approve the change in the dialog LibrePaper Companion opened on this computer.</p>
    {/if}
  {/if}

  {#if name === "zotero"}
    <p class="setting-description integration-note">LibrePaper reads Zotero through its local API. Enable it in Zotero Settings > Advanced > "Allow other applications on this computer to communicate with Zotero".</p>
  {/if}
{/if}

<style>
  .integration-error { margin-block: calc(var(--spacing) * 2); color: var(--color-error-text); }
  .integration-actions { display: flex; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .integration-note { max-width: 42rem; margin-top: calc(var(--spacing) * 2); }
</style>
