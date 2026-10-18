<script>
  import DesktopOnly from "../DesktopOnly.svelte";
  import DiagnosticsPanel from "./DiagnosticsPanel.svelte";
  import { forgetCapabilities } from "../../lib/desktop/client.js";
  import { toolItems } from "../../lib/desktop/tools.js";

  let { view } = $props();
  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());

  // Tools that are fine to be missing; anything else missing is a real problem.
  const OPTIONAL = {
    zotero: "Optional. Install Zotero and allow other applications to communicate with it to cite from your library.",
    sandbox: "Optional. Without sandboxing, code runs with your own permissions.",
    sandboxing: "Optional. Without sandboxing, code runs with your own permissions.",
    pandoc: "Optional. Markdown documents can render in the browser instead.",
  };

  const rows = $derived(toolItems(view.state?.tools).map(({ key, title, tool }) => {
    const found = Boolean(tool?.available);
    const label = found ? "Available" : tool ? "Not found" : "Not configured";
    const optional = key in OPTIONAL;
    const hint = found ? "" : tool?.note || (optional ? OPTIONAL[key] : `Install ${title} on this computer, then rescan.`);
    return { key, title, found, label, version: found ? tool.version || "" : "", hint, problem: !found && !optional };
  }));

  let lastScan = $state(null);
  async function rescan() {
    if (await view.act("rescan", "Tool scan complete.", "/tools/rescan", { method: "POST" })) {
      forgetCapabilities();
      lastScan = new Date();
    }
  }

  const bindings = $derived(view.list(view.state?.bindings));
  const jobs = $derived(view.list(view.state?.jobs));
  const previews = $derived(view.list(view.state?.previews));
  const sessions = $derived(view.list(view.state?.sessions));
  const running = $derived(jobs.length + previews.length + sessions.length > 0);

  // Elapsed time when the desktop app says when the work started.
  function elapsed(item) {
    const seconds = typeof item.elapsed_seconds === "number" ? item.elapsed_seconds : (Date.now() - Date.parse(item.started_at || item.started || "")) / 1000;
    if (!Number.isFinite(seconds) || seconds < 0) return "";
    return seconds < 60 ? `${Math.round(seconds)} s` : `${Math.floor(seconds / 60)} min ${Math.round(seconds % 60)} s`;
  }
  const bindingNote = (binding) => [binding.origin || "Connected site", binding.entrypoint || "project folder", binding.execution_granted === false ? "Code execution is not authorized" : ""].filter(Boolean).join(" · ");
</script>

{#if view.error}
  <p class="setting-description desktop-error" role="alert">{view.error}</p>
{/if}
{#if view.loadError}
  <p class="setting-description desktop-error" role="alert">{view.loadError}</p>
{/if}
{#if view.notice}
  <p class="setting-description desktop-notice" role="status">{view.notice}</p>
{/if}

<DesktopOnly description="Link projects to folders on your computer and run their code locally.">
{#if view.available}
  <section id="desktop-environment" class="desktop-section" aria-label="Local environment">
    <div class="desktop-bar">
      <h4 class="desktop-title">Local environment</h4>
      <span class="setting-description">{lastScan ? `Last scan ${lastScan.toLocaleTimeString()}` : ""}</span>
      <button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void rescan()}>{view.pending === "rescan" ? "Scanning…" : "Rescan"}</button>
    </div>
    <table class="desktop-table">
      <thead><tr><th scope="col">Tool</th><th scope="col">Status</th><th scope="col">Version</th></tr></thead>
      <tbody>
        {#each rows as row (row.key)}
          <tr>
            <th scope="row">{row.title}</th>
            <td class:desktop-problem={row.problem}>{row.label}</td>
            <td>{row.version}</td>
          </tr>
          {#if row.hint}<tr class="desktop-hint"><td colspan="3" class="setting-description">{row.hint}</td></tr>{/if}
        {/each}
      </tbody>
    </table>
    {#if bindings.length}
      <table class="desktop-table" aria-label="Project folders">
        <thead><tr><th scope="col">Project folder</th><th scope="col">Details</th><th scope="col"><span class="sr-only">Action</span></th></tr></thead>
        <tbody>
          {#each bindings as binding (binding.id)}
            <tr>
              <th scope="row">{binding.project || "Authorized folder"}</th>
              <td>{bindingNote(binding)}</td>
              <td class="desktop-action"><button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) void view.act(`binding-${binding.id}`, "Folder authorization removed.", `/bindings/${view.id(binding.id)}`, { method: "DELETE" }); }}>Revoke</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    <details class="desktop-technical">
      <summary>View technical report</summary>
      <pre class="setting-log">{JSON.stringify(view.state?.tools, null, 2)}</pre>
    </details>
  </section>

  <section id="desktop-diagnostics" class="desktop-section" aria-label="Diagnostics">
    <h4 class="desktop-title">Diagnostics</h4>
    <DiagnosticsPanel />
  </section>

  {#if running}
  <section id="desktop-activity" class="desktop-section" aria-label="Activity">
    <h4 class="desktop-title">Activity</h4>
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
        {#each previews as preview (preview.id)}
          <tr>
            <th scope="row">{preview.label || preview.project || "Preview"}</th>
            <td>{elapsed(preview)}</td>
            <td>{preview.status || "active"}</td>
            <td class="desktop-action"><button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`preview-${preview.id}`, "Preview stopped.", `/previews/${view.id(preview.id)}`, { method: "DELETE" })}>Stop</button></td>
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
  .desktop-section { margin-top: calc(var(--spacing) * 5); padding-top: calc(var(--spacing) * 4); border-top: 1px solid var(--color-divider); }
  .desktop-section:first-of-type { margin-top: calc(var(--spacing) * 3); padding-top: 0; border-top: 0; }
  .desktop-title { margin: 0 0 calc(var(--spacing) * 2); font-size: var(--text-base); font-weight: var(--panel-title-weight); }
  .desktop-bar { display: flex; align-items: baseline; flex-wrap: wrap; gap: calc(var(--spacing) * 3); margin-bottom: calc(var(--spacing) * 2); }
  .desktop-bar .desktop-title { margin: 0; margin-right: auto; }
  .desktop-table { width: 100%; margin-bottom: calc(var(--spacing) * 3); border-collapse: collapse; font-size: var(--panel-meta-size); }
  .desktop-table thead th { color: var(--panel-muted); font-weight: 500; }
  .desktop-table th, .desktop-table td { text-align: left; padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 4) calc(var(--spacing) * 1.5) 0; vertical-align: middle; }
  .desktop-table tbody tr:not(.desktop-hint) { border-top: 1px solid var(--color-divider); }
  .desktop-table tbody th { font-weight: 500; }
  .desktop-hint td { padding-top: 0; padding-bottom: calc(var(--spacing) * 1.5); }
  .desktop-problem { color: var(--color-error-text); }
  .desktop-action { text-align: right; padding-right: 0; }
  .desktop-technical summary { cursor: pointer; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .desktop-technical pre { margin-top: calc(var(--spacing) * 2); }
</style>
