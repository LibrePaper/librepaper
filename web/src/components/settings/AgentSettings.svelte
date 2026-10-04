<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { createMachineView } from "../../lib/companion/machine.svelte.js";
  import { splitArgs } from "../../lib/companion/args.js";

  const view = createMachineView();
  const { list, id } = view;

  let adding = $state(false);
  let invalid = $state("");
  let label = $state("");
  let command = $state("");

  const customAgentIds = $derived(new Set(list(view.state?.custom_agents).map((agent) => String(agent.id))));
  const detectedAgents = $derived(list(view.state?.agents).filter((agent) => !customAgentIds.has(String(agent.id))));

  async function addAgent(event) {
    event.preventDefault();
    if (view.pending) return;
    invalid = "";
    const argv = splitArgs(command.trim());
    if (!label.trim() || !argv.length) {
      invalid = "Enter a name and a command.";
      return;
    }
    if (await view.act("agent-add", "Agent added.", "/agents", { method: "POST", body: { label: label.trim(), command: argv } })) close();
  }

  function close() {
    adding = false;
    invalid = "";
    label = "";
    command = "";
  }

  onMount(() => view.start());
</script>

{#if view.available}
  <section class="settings-subsection" id="agents-list" aria-labelledby="companion-agents-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-agents-heading">Agents on this computer</h4></div>
    {#if view.loadError}<p class="setting-description management-error" role="alert">{view.loadError}</p>{/if}
    {#each detectedAgents as agent (agent.id)}
      <SettingRow title={agent.label || agent.id || "Detected agent"}>
        {#if agent.assistant_blocked}<span class="setting-description management-error">{agent.assistant_blocked}</span>{/if}
      </SettingRow>
    {/each}
    {#each list(view.state?.custom_agents) as agent (agent.id)}
      <SettingRow title={agent.label || agent.id || "Configured agent"} description={list(agent.command).join(" ")}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Remove ${agent.label || "this agent"}?`)) void view.act(`agent-${agent.id}`, "Agent removed.", `/agents/${id(agent.id)}`, { method: "DELETE" }); }}>Remove</button>
      </SettingRow>
    {/each}
    {#if view.state && !detectedAgents.length && !list(view.state?.custom_agents).length}<p class="setting-description">No agents found.</p>{/if}

    <div id="agents-add" class="agent-add">
      {#if adding}
        <form class="agent-form" onsubmit={addAgent}>
          <input class="input input-sm setting-input agent-name" name="label" aria-label="Agent name" bind:value={label} disabled={Boolean(view.pending)} />
          <input class="input input-sm setting-input agent-command" name="command" aria-label="Command" bind:value={command} disabled={Boolean(view.pending)} />
          <button class="btn btn-sm lp-control-brand" type="submit" disabled={Boolean(view.pending)}>{view.pending === "agent-add" ? "Adding…" : "Add agent"}</button>
          <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={close}>Cancel</button>
        </form>
        <p class="setting-description">Name, then the program and its arguments as typed in a terminal.</p>
      {:else}
        <button class="btn btn-sm lp-control-outline" type="button" onclick={() => { adding = true; }}>Add agent</button>
      {/if}
      {#if invalid || view.error}<p class="setting-description management-error" role="alert">{invalid || view.error}</p>{/if}
    </div>
  </section>
{:else}
  <SettingRow title="This computer" description="Connect to this computer's companion to see and add agents.">
    {#if view.connectError}<span class="setting-description management-error" role="alert">{view.connectError}</span>{/if}
    <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.manageThisComputer()}>{view.pending === "connect" ? "Connecting…" : "Manage this computer"}</button>
  </SettingRow>
{/if}

<style>
  .management-error { color: var(--color-error-text); }
  .agent-add { margin-top: calc(var(--spacing) * 2); }
  .agent-form { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); }
  .agent-name { width: 10rem; }
  .agent-command { flex: 1; min-width: 12rem; }
</style>
