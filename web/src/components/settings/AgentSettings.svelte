<script>
  import SettingRow from "./SettingRow.svelte";
  import ToolRow from "./ToolRow.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import { splitArgs } from "../../lib/desktop/args.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { view } = $props();

  let adding = $state(false);
  let invalid = $state("");
  let label = $state("");
  let command = $state("");

  const customAgentIds = $derived(new Set(view.list(view.state?.custom_agents).map((agent) => String(agent.id))));
  const detectedAgents = $derived(view.list(view.state?.agents).filter((agent) => !customAgentIds.has(String(agent.id))));

  const agentStatus = (agent) => agent.assistant_blocked ? { label: m.agentsettings_status_setup(), tone: "warn" } : agent.assistant ? { label: m.agentsettings_status_ready(), tone: "good" } : { label: m.agentsettings_status_unsupported(), tone: "neutral" };

  // A blocked agent never fetches, so its reason and its note are the whole text.
  const fetchText = (agent) => [m.agentsettings_fetches(), agent.assistant_install ? m.agentsettings_install_hint({ command: agent.assistant_install }) : ""].filter(Boolean).join(" ");
  const agentDescription = (agent) => [agent.assistant_blocked || agent.assistant_note, agent.assistant_fetches ? fetchText(agent) : ""].filter(Boolean).join(" ");

  async function addAgent(event) {
    event.preventDefault();
    if (view.pending) return;
    invalid = "";
    const argv = splitArgs(command.trim());
    if (!label.trim() || !argv.length) {
      invalid = m.agentsettings_name_required();
      return;
    }
    if (await view.act("agent-add", m.agentsettings_added(), "/agents", { method: "POST", body: { label: label.trim(), command: argv } })) close();
  }

  function close() {
    adding = false;
    invalid = "";
    label = "";
    command = "";
  }
</script>

<DesktopOnly description={m.agentsettings_desktop_description()}>
{#if view.available}
  <section class="settings-subsection" id="agents-list" aria-labelledby="agents-list-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="agents-list-heading">{m.agentsettings_heading()}</h4></div>
    <p class="setting-description">{m.agentsettings_list_description()}</p>
    {#if view.loadError}<p class="setting-description management-error" role="alert">{view.loadError}</p>{/if}
    {#each detectedAgents as agent (agent.id)}
      {#if agent.assistant_blocked}
        <!-- An older desktop app sends no assistant_docs, so the link falls back to the agents page. -->
        <ToolRow title={agent.label || agent.id} description={agentDescription(agent)} status={agentStatus(agent)}>
          {#snippet actions()}
            <a class="btn btn-sm lp-control-tonal-brand" href={agent.assistant_docs || "https://librepaper.org/agents.html"} target="_blank" rel="noreferrer">{m.agentsettings_setup_docs()}</a>
          {/snippet}
        </ToolRow>
      {:else}
        <ToolRow title={agent.label || agent.id} description={agentDescription(agent)} status={agentStatus(agent)} />
      {/if}
    {/each}
    {#each view.list(view.state?.custom_agents) as agent (agent.id)}
      <ToolRow title={agent.label || agent.id || m.agentsettings_configured_fallback()} description={view.list(agent.command).join(" ")} status={{ label: m.agentsettings_status_added(), tone: "neutral" }}>
        {#snippet actions()}
          <button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(m.agentsettings_remove_confirm({ name: agent.label || m.agentsettings_this_agent() }))) void view.act(`agent-${agent.id}`, m.agentsettings_removed(), `/agents/${view.id(agent.id)}`, { method: "DELETE" }); }}>{m.agentsettings_remove()}</button>
        {/snippet}
      </ToolRow>
    {/each}
    {#if view.state && !detectedAgents.length && !view.list(view.state?.custom_agents).length}<p class="setting-description">{m.agentsettings_none()}</p>{/if}

    <div id="agents-add" class="agent-add">
      {#if adding}
        <form class="agent-form" onsubmit={addAgent}>
          <label class="label agent-name-field"><span class="label-text">{m.agentsettings_name()}</span><input class="input input-sm setting-input agent-name" name="label" aria-label={m.agentsettings_name_aria()} bind:value={label} disabled={Boolean(view.pending)} /></label>
          <label class="label agent-command-field"><span class="label-text">{m.agentsettings_command()}</span><input class="input input-sm setting-input agent-command" name="command" aria-label={m.agentsettings_command()} bind:value={command} disabled={Boolean(view.pending)} /></label>
          <button class="btn btn-sm lp-control-brand" type="submit" disabled={Boolean(view.pending)}>{view.pending === "agent-add" ? m.agentsettings_adding() : m.agentsettings_add()}</button>
          <button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={close}>{m.agentsettings_cancel()}</button>
        </form>
        <p class="setting-description">{m.agentsettings_form_hint()}</p>
      {:else}
        <button class="btn btn-sm lp-control-tonal-brand" type="button" onclick={() => { adding = true; }}>{m.agentsettings_add()}</button>
      {/if}
      {#if invalid || view.error}<p class="setting-description management-error" role="alert">{invalid || view.error}</p>{/if}
    </div>
  </section>
{/if}
</DesktopOnly>

<style>
  .management-error { color: var(--color-error-text); }
  .agent-add { margin-top: calc(var(--spacing) * 2); }
  .agent-form { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .agent-name-field { width: 10rem; }
  .agent-name { width: 100%; }
  .agent-command-field { flex: 1; min-width: 12rem; }
  .agent-command { width: 100%; }
</style>
