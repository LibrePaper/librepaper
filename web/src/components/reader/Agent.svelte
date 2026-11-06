<script>
  // The agent panel: the built-in agent in the desktop app, the external agent
  // that a token lets you run yourself, and the tasks you copy into either.
  // The built-in agent stays mounted while another tab shows, because its
  // sessions keep running.
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import Icon from "../Icon.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import AgentSessions from "./AgentSessions.svelte";
  import AgentExternal from "./AgentExternal.svelte";
  import { createSharing } from "../../lib/sharing.svelte.js";
  import { desktop } from "../../lib/desktop/client.js";
  import { TASK_GROUPS, filterTaskGroups } from "../../lib/agent-tasks.js";
  import { m } from "../../lib/paraglide/messages.js";

  let {
    open = false, slug, canShare = false, request = null, link, path = "", selection = null, revision = "",
    diagnostics = [], comments = [], changes = [], onreview, onpreview, onsettings, userName,
  } = $props();
  // The tasks copy their text, and the external tab reads the same sharing state.
  const sharing = createSharing({ slug: () => slug, canShare: () => canShare });
  const paired = Boolean(desktop());

  const TABS = $derived([
    { id: "builtin", label: m.common_chat() },
    ...(canShare ? [{ id: "external", label: m.common_agent() }] : []),
    { id: "tasks", label: m.agent_tab_tasks() },
  ]);
  // The picked tab is kept only while it is still offered. Without one, the
  // default follows what the user can do, and is derived because canShare can
  // turn true after the document loads.
  let picked = $state(null);
  const current = $derived.by(() => {
    if (picked && TABS.some((tab) => tab.id === picked)) return picked;
    if (paired) return "builtin";
    if (canShare) return "external";
    return "tasks";
  });

  // A request from elsewhere in the reader, or a task run from here, goes to
  // the built-in agent.
  let outgoing = $state(null);
  $effect(() => {
    if (request) {
      outgoing = request;
      picked = "builtin";
    }
  });
  function run(task) {
    outgoing = { id: crypto.randomUUID(), taskId: task.run };
    picked = "builtin";
  }

  // The search box filters the tasks. Opened is the ids of the groups the user
  // has opened; every group starts closed, and the choice is kept only while the
  // panel is mounted. A search opens every match and leaves this list alone, so
  // clearing the search restores it.
  let taskQuery = $state("");
  let opened = $state([]);
  const shownGroups = $derived(filterTaskGroups(TASK_GROUPS, taskQuery));
  const searching = $derived(taskQuery.trim() !== "");
  const isOpen = (group) => searching || opened.includes(group.id);

  function toggleGroup(id) {
    opened = opened.includes(id) ? opened.filter((item) => item !== id) : [...opened, id];
  }

  // Escape clears a non-empty search and stops there, so it does not also close
  // the panel.
  function onsearchkeydown(event) {
    if (event.key === "Escape" && taskQuery !== "") {
      taskQuery = "";
      event.stopPropagation();
    }
  }

  function pick(value) {
    picked = value;
    // The manual-copy text may hold the token, and each tab shows it.
    sharing.clearCopy();
  }
</script>

{#snippet messages()}
  {#if sharing.error}
    <p class="lp-text-error" role="alert">{sharing.error}</p>
  {/if}
  {#if sharing.copyFallback}
    <textarea class="share-copy-fallback w-full" readonly rows="3" aria-label={m.share_copy_manual_label()} value={sharing.copyFallback} onclick={(event) => event.currentTarget.select()}></textarea>
  {/if}
{/snippet}

{#snippet tasksPane()}
  <p class="panel-muted">{paired ? m.agent_tasks_intro_desktop() : m.agent_tasks_intro()}</p>
  <input class="input agent-task-search" type="search" placeholder={m.agent_tasks_search_placeholder()}
         aria-label={m.agent_tasks_search_placeholder()} bind:value={taskQuery} onkeydown={onsearchkeydown} />
  {#each shownGroups as group (group.id)}
    {@const expanded = isOpen(group)}
    {@const listId = `agent-tasks-${group.id}`}
    <h3 class="agent-task-group">
      <button type="button" class="agent-task-toggle" aria-expanded={expanded ? "true" : "false"} aria-controls={listId}
              disabled={searching} onclick={() => toggleGroup(group.id)}>
        <Icon name={expanded ? "chevron-down" : "chevron-right"} size={14} />
        <span class="agent-task-group-label">{group.label}</span>
        <span class="agent-task-count">{group.tasks.length}</span>
      </button>
    </h3>
    {#if expanded}
      <ul class="agent-task-list" id={listId} aria-label={group.label}>
        {#each group.tasks as task (task.id)}
          {@const copyLabel = m.agent_task_copy_label({ task: task.label })}
          {@const runLabel = m.agent_task_run_label({ task: task.label })}
          <li class="agent-task">
            <div class="agent-task-text">
              <span class="agent-task-name">{task.label}</span>
              <span class="agent-task-body">{task.text}</span>
            </div>
            {#if paired && task.run}
              <!-- IconButton passes no data attributes through, so the marker wraps it. -->
              <span class="agent-task-run" data-task-run={task.run}>
                <IconButton icon="play" label={runLabel} title={runLabel} tone="plain" size="btn-icon-sm"
                            onclick={() => run(task)} />
              </span>
            {/if}
            <IconButton icon={sharing.copied === task.text ? "check" : "copy"} label={copyLabel} title={copyLabel} tone="plain" size="btn-icon-sm"
                        onclick={() => sharing.copy(task.text)} />
          </li>
        {/each}
      </ul>
    {/if}
  {/each}
  {#if searching && shownGroups.length === 0}
    <p class="panel-muted" role="status">{m.agent_tasks_no_match({ query: taskQuery.trim() })}</p>
  {/if}
  {@render messages()}
{/snippet}

<section class="panel panel-tabbed agent-panel" aria-label={m.common_agent()}>
  <PanelHeader title={m.common_agent()} />
  <PanelTabs id="agent" label={m.common_agent()} tabs={TABS} value={current} onchange={pick}>
    <Tabs.Content value="builtin" class="agent-builtin-pane">
      <DesktopOnly feature={m.agent_the_agent()} description={m.common_chat_with_coding_agent()}>
        <AgentSessions {slug} {canShare} {link} {path} {selection} {revision} {diagnostics} {comments} {changes}
                       {onreview} {onpreview} {onsettings} {userName} request={outgoing} />
      </DesktopOnly>
    </Tabs.Content>
    {#if canShare}
      <Tabs.Content value="external"><AgentExternal {open} {sharing} /></Tabs.Content>
    {/if}
    <Tabs.Content value="tasks">{#if current === "tasks"}{@render tasksPane()}{/if}</Tabs.Content>
  </PanelTabs>
</section>

<style>
  /* Every pane but the built-in agent scrolls as one column. The built-in agent
     fills the panel and scrolls inside its own sessions. */
  .agent-panel :global([role="tabpanel"]:not(.agent-builtin-pane)) { overflow-y: auto; gap: calc(var(--spacing) * 2); }
  .agent-panel :global([role="tabpanel"]:not(.agent-builtin-pane)) > :global(*) { flex-shrink: 0; }
  .agent-panel :global(.agent-builtin-pane) { display: flex; flex: 1 1 0; min-height: 0; flex-direction: column; overflow: hidden; }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  /* The tasks are a list of two-line rows: the name, then the instruction in
     muted type, with a copy button at the right and, where the desktop app can
     run the task, a Run button beside it. Only the buttons are live. */
  .agent-task-search { width: 100%; }
  .agent-task-group { margin: calc(var(--spacing) * 1) 0 0; font-size: var(--panel-meta-size); font-weight: 600; color: var(--panel-muted); }
  .agent-task-toggle { display: flex; align-items: center; gap: calc(var(--spacing) * 1); width: 100%; padding: .2rem .25rem; margin: 0; border: 0; border-radius: .25rem; background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
  .agent-task-toggle:hover:not(:disabled) { background: var(--color-row-hover); }
  .agent-task-toggle:disabled { cursor: default; }
  .agent-task-toggle:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .agent-task-count { margin-left: auto; font-weight: 400; }
  .agent-task-list { margin: 0; padding: 0; list-style: none; }
  .agent-task { display: flex; align-items: center; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 1.5); border-top: 1px solid var(--color-divider); }
  .agent-task:first-child { border-top: 0; }
  .agent-task-text { display: flex; flex: 1 1 auto; flex-direction: column; gap: calc(var(--spacing) * .5); min-width: 0; }
  .agent-task-name { font-size: 13px; font-weight: 500; line-height: 1.3; }
  .agent-task-body { font-size: 12.5px; line-height: 1.4; color: var(--panel-muted); overflow-wrap: anywhere; }
  .agent-task-run { display: contents; }
  /* Phones: the task controls are finger-sized, and the search keeps 16px so
     iOS does not zoom. */
  @media (max-width: 767px) {
    .agent-panel :global(.icon-control) { min-width: 2.75rem; min-height: 2.75rem; }
    .agent-task-toggle { min-height: 2.75rem; }
    .agent-task-search { font-size: 1rem; }
  }
</style>
