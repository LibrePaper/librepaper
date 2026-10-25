<script>
  import ToolRow from "./ToolRow.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import { forgetCapabilities } from "../../lib/desktop/client.js";
  import { toolItems } from "../../lib/desktop/tools.js";

  // The page does not own the machine view; the dialog creates and starts it.
  let { view } = $props();
  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());

  const DESCRIPTIONS = {
    zotero: "Citations from your Zotero library.",
  };

  const connected = $derived(view.available);
  // The render commands are set in Settings > Rendering. Their programs are listed here.
  const capabilities = $derived({ ...view.state?.tools, renderers: view.state?.settings?.renderers });
  // The programs worth showing are listed even when the desktop app did not report them.
  const tools = $derived(toolItems(capabilities));

  // The rescan also refreshes what the rest of the page knows about the tools.
  async function rescan() {
    if (await view.act("rescan", "Tool scan complete.", "/tools/rescan", { method: "POST" })) forgetCapabilities();
  }

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

  const jobs = $derived(view.list(view.state?.jobs));
  const sessions = $derived(view.list(view.state?.sessions));
  const running = $derived(jobs.length + sessions.length > 0);

  // Elapsed time when the desktop app says when the work started.
  function elapsed(item) {
    const seconds = typeof item.elapsed_seconds === "number" ? item.elapsed_seconds : (Date.now() - Date.parse(item.started_at || item.started || "")) / 1000;
    if (!Number.isFinite(seconds) || seconds < 0) return "";
    return seconds < 60 ? `${Math.round(seconds)} s` : `${Math.floor(seconds / 60)} min ${Math.round(seconds % 60)} s`;
  }
</script>

<DesktopOnly description="Render documents with the programs on your computer, run their code locally.">
{#if view.error}
  <p class="setting-description desktop-error" role="alert">{view.error}</p>
{/if}
{#if view.loadError}
  <p class="setting-description desktop-error" role="alert">{view.loadError}</p>
{/if}
{#if view.notice}
  <p class="setting-description desktop-notice" role="status">{view.notice}</p>
{/if}

{#if view.available}
  <section id="tools-programs" class="settings-subsection" aria-labelledby="tools-programs-heading">
    <div class="settings-section-title">
      <h4 class="settings-subhead" id="tools-programs-heading">Programs</h4>
      <button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void rescan()}>{view.pending === "rescan" ? "Scanning…" : "Rescan"}</button>
    </div>
    {#each tools as { key, title, tool } (key)}
      <ToolRow id={`tools-${key}`} {title} description={description(key, title, tool)} status={status(tool)} />
    {/each}
  </section>

  {#if running}
    <section id="tools-activity" class="settings-subsection">
      <div class="settings-section-title">
        <h4 class="settings-subhead">Activity</h4>
      </div>
      <table class="desktop-table">
        <thead><tr><th scope="col">Name</th><th scope="col">Elapsed</th><th scope="col">Status</th><th scope="col"><span class="sr-only">Action</span></th></tr></thead>
        <tbody>
          {#each jobs as job (job.id)}
            <tr>
              <th scope="row">{[job.kind, job.stage].filter(Boolean).join(" · ") || "Local job"}</th>
              <td>{elapsed(job)}</td>
              <td>{job.status || "running"}{job.error ? ` · ${job.error}` : ""}</td>
              <td class="desktop-action">{#if active(job.status)}<button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`job-${job.id}`, "Cancellation requested.", `/jobs/${view.id(job.id)}/cancel`, { method: "POST" })}>Cancel</button>{/if}</td>
            </tr>
          {/each}
          {#each sessions as session (session.id)}
            {@const sessionStatus = session.status || session.state || "active"}
            <tr>
              <th scope="row">{session.name || session.agent || session.id || "Agent session"}</th>
              <td>{elapsed(session)}</td>
              <td>{sessionStatus}</td>
              <td class="desktop-action">{#if active(sessionStatus)}<button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`session-${session.id}`, "Agent stop requested.", `/agents/sessions/${view.id(session.id)}/cancel`, { method: "POST" })}>Stop</button>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}
{/if}
</DesktopOnly>

<style>
  .desktop-error { color: var(--color-error-text); }
  .desktop-notice { color: var(--color-success-text); }
  .desktop-table { width: 100%; margin-bottom: calc(var(--spacing) * 3); border-collapse: collapse; font-size: var(--panel-meta-size); }
  .desktop-table thead th { color: var(--panel-muted); font-weight: 500; }
  .desktop-table th, .desktop-table td { text-align: left; padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 4) calc(var(--spacing) * 1.5) 0; vertical-align: middle; }
  .desktop-table tbody tr { border-top: 1px solid var(--color-divider); }
  .desktop-table tbody th { font-weight: 500; }
  .desktop-action { text-align: right; padding-right: 0; }
</style>
