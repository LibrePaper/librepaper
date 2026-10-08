<script>
  import SettingRow from "./SettingRow.svelte";
  import ToolRow from "./ToolRow.svelte";
  import ToolCommand from "./ToolCommand.svelte";
  import { companion } from "../../lib/companion/status.svelte.js";

  // The page does not own the machine view; the dialog creates and starts it.
  let { view } = $props();

  // Watching never probes, so opening this page cannot prompt for local network access.
  $effect(() => companion.watch());

  const CONFIGURABLE = ["quarto", "calepin"];
  const COMMON = ["Quarto", "Calepin", "Zotero"];
  const DESCRIPTIONS = {
    quarto: "Renders Quarto and Markdown documents.",
    pandoc: "Renders Markdown documents.",
    calepin: "Renders Typst and Calepin documents.",
    zotero: "Citations from your Zotero library.",
  };

  function items(capabilities) {
    const tools = capabilities && capabilities.tools && typeof capabilities.tools === "object" ? capabilities.tools : {};
    const entries = Object.entries(tools);
    if (capabilities?.calepin) entries.push(["Calepin", capabilities.calepin]);
    if (capabilities?.zotero) entries.push(["Zotero", capabilities.zotero]);
    if (capabilities?.quarto?.tool) entries.push(["Quarto", capabilities.quarto.tool]);
    for (const builder of Array.isArray(capabilities?.builders) ? capabilities.builders : []) if (builder?.id) entries.push([builder.id, builder]);
    const unique = new Map();
    for (const [name, tool] of entries) {
      const key = name.toLowerCase();
      const old = unique.get(key);
      if (!old || (!old.tool?.available && tool?.available) || (!old.tool?.version && tool?.version)) unique.set(key, { name, tool });
    }
    return [...unique.values()];
  }

  const connected = $derived(view.available || companion.status?.state === "connected");
  const capabilities = $derived(view.available ? view.state?.tools : companion.status?.capabilities);
  // The programs worth showing are listed even when the companion did not report them.
  const tools = $derived.by(() => {
    const found = items(capabilities);
    for (const name of COMMON) if (!found.some((entry) => entry.name.toLowerCase() === name.toLowerCase())) found.push({ name, tool: null });
    return found.map(({ name, tool }) => ({ key: name.toLowerCase(), title: name.charAt(0).toUpperCase() + name.slice(1), tool }));
  });

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

{#if view.error}
  <p class="setting-description tools-error" role="alert">{view.error}</p>
{/if}
{#if view.loadError}
  <p class="setting-description tools-error" role="alert">{view.loadError}</p>
{/if}
{#if view.notice}
  <p class="setting-description tools-notice" role="status">{view.notice}</p>
{/if}


{#if view.available && view.list(view.state?.approvals).length}
<section id="tools-approvals" class="settings-subsection" aria-labelledby="tools-approvals-heading">
  <div class="settings-section-title"><h4 class="settings-subhead" id="tools-approvals-heading">Waiting for your answer</h4></div>
  <p class="setting-description">Sites ask before they connect to this computer or change its settings.</p>
  {#each view.list(view.state?.approvals) as approval (approval.id)}
    <SettingRow title={approval.title || "Approval request"} description={approval.message || "This request has no additional details."}>
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`approval-${approval.id}`, "Request denied.", `/approvals/${view.id(approval.id)}`, { method: "POST", body: { decision: "deny" } })}>Deny</button>
      <button class="btn btn-sm lp-control-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`approval-${approval.id}`, "Request allowed.", `/approvals/${view.id(approval.id)}`, { method: "POST", body: { decision: "allow" } })}>{approval.allow_label || "Allow"}</button>
    </SettingRow>
  {/each}
</section>
{/if}

<section id="tools-list" class="settings-subsection" aria-labelledby="tools-list-heading">
  <div class="settings-section-title">
    <h4 class="settings-subhead" id="tools-list-heading">Programs</h4>
    {#if view.available}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act("rescan", "Tool scan complete.", "/tools/rescan", { method: "POST" })}>{view.pending === "rescan" ? "Scanning…" : "Rescan"}</button>{/if}
  </div>
  {#each tools as { key, title, tool } (key)}
    {#if CONFIGURABLE.includes(key)}
      <ToolRow id={`tools-${key}`} {title} description={description(key, title, tool)} status={status(tool)}><ToolCommand name={key} /></ToolRow>
    {:else}
      <ToolRow id={`tools-${key}`} {title} description={description(key, title, tool)} status={status(tool)} />
    {/if}
  {/each}
</section>

{#if view.available}
  <section id="tools-sites" class="settings-subsection" aria-labelledby="tools-sites-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="tools-sites-heading">Connected sites</h4></div>
    {#each view.list(view.state?.pairings) as pairing (pairing.id)}
      <SettingRow title={pairing.origin || "Connected site"} description={pairing.created_at ? `Connected ${new Date(Number(pairing.created_at) * 1000).toLocaleString()}` : "This site can use the companion."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Revoke access for ${pairing.origin}?`)) void view.act(`pairing-${pairing.id}`, "Site access revoked.", `/pairings/${view.id(pairing.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {:else}
    <p class="setting-description">None.</p>
    {/each}
  </section>

  {#if view.list(view.state?.bindings).length}
  <section id="tools-folders" class="settings-subsection" aria-labelledby="tools-folders-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="tools-folders-heading">Project folders</h4></div>
    {#each view.list(view.state?.bindings) as binding (binding.id)}
      <SettingRow title={binding.project || "Authorized folder"} description={`${binding.origin || "Connected site"} · ${binding.entrypoint || "project folder"}${binding.root ? ` · ${binding.root}` : ""}${binding.execution_granted === false ? " · Code execution is not authorized" : ""}`}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) void view.act(`binding-${binding.id}`, "Folder authorization removed.", `/bindings/${view.id(binding.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {/each}
  </section>
  {/if}
{/if}

<style>
  .tools-error { color: var(--color-error-text); }
  .tools-notice { color: var(--color-success-text); }
</style>
