<script>
  import ToolRow from "./ToolRow.svelte";
  import ToolCommand from "./ToolCommand.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import { forgetCapabilities } from "../../lib/desktop/client.js";
  import { toolItems } from "../../lib/desktop/tools.js";

  // The page does not own the machine view; the dialog creates and starts it.
  let { view } = $props();

  // The rescan also refreshes what the rest of the page knows about the tools.
  async function rescan() {
    if (await view.act("rescan", "Tool scan complete.", "/tools/rescan", { method: "POST" })) forgetCapabilities();
  }

  const CONFIGURABLE = ["quarto", "calepin"];
  const DESCRIPTIONS = {
    quarto: "Renders Quarto and Markdown documents.",
    pandoc: "Renders Markdown documents.",
    calepin: "Renders Typst and Calepin documents.",
    zotero: "Citations from your Zotero library.",
  };

  const connected = $derived(view.available);
  const capabilities = $derived(view.state?.tools);
  // The programs worth showing are listed even when the desktop app did not report them.
  const tools = $derived(toolItems(capabilities));

  function status(tool) {
    if (!connected) return { label: "Not checked", tone: "neutral" };
    if (tool?.available) return { label: tool.version ? `Available · ${tool.version}` : "Available", tone: "good" };
    return { label: "Not found", tone: "warn" };
  }

  function description(key, title, tool) {
    if (tool?.note) return tool.note;
    if (connected && !tool?.available) {
      return key === "zotero"
        ? "Open Zotero and allow other applications to communicate with it (Zotero settings, Advanced)."
        : `Install ${title} on this computer, then rescan.`;
    }
    return DESCRIPTIONS[key] || "";
  }
</script>

<DesktopOnly description="Render Quarto, R and Python documents with the tools installed on your computer.">
{#if view.error}
  <p class="setting-description tools-error" role="alert">{view.error}</p>
{/if}
{#if view.loadError}
  <p class="setting-description tools-error" role="alert">{view.loadError}</p>
{/if}
{#if view.notice}
  <p class="setting-description tools-notice" role="status">{view.notice}</p>
{/if}


<section id="tools-list" class="settings-subsection" aria-labelledby="tools-list-heading">
  <div class="settings-section-title">
    <h4 class="settings-subhead" id="tools-list-heading">Programs</h4>
    {#if view.available}<button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void rescan()}>{view.pending === "rescan" ? "Scanning…" : "Rescan"}</button>{/if}
  </div>
  {#each tools as { key, title, tool } (key)}
    {#if CONFIGURABLE.includes(key)}
      <ToolRow id={`tools-${key}`} {title} description={description(key, title, tool)} status={status(tool)}><ToolCommand name={key} /></ToolRow>
    {:else}
      <ToolRow id={`tools-${key}`} {title} description={description(key, title, tool)} status={status(tool)} />
    {/if}
  {/each}
</section>
</DesktopOnly>

<style>
  .tools-error { color: var(--color-error-text); }
  .tools-notice { color: var(--color-success-text); }
</style>
