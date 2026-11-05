<script>
  // The agent panel: the tokens that let an agent you run yourself reach this
  // project, and the tasks you copy into that agent. A user who cannot share
  // has no tokens, so they see the tasks alone and no tab row.
  import { tick } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import Icon from "../Icon.svelte";
  import Modal from "../Modal.svelte";
  import ShareRow from "./share/ShareRow.svelte";
  import ShareForm from "./share/ShareForm.svelte";
  import TokenReveal from "./share/TokenReveal.svelte";
  import { allows, budgetOf, INSTALL_INSTRUCTIONS } from "./share/shared.js";
  import { createSharing } from "../../lib/sharing.svelte.js";
  import { TASK_GROUPS, filterTaskGroups } from "../../lib/agent-tasks.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { open = false, slug, canShare = false } = $props();
  const sharing = createSharing({ slug: () => slug, canShare: () => canShare });

  // Tokens is the first and default tab for an owner.
  const TABS = [{ id: "tokens", label: m.agent_tab_tokens() }, { id: "tasks", label: m.agent_tab_tasks() }];
  let tab = $state("tokens");
  const current = $derived(canShare ? tab : "tasks");

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

  let root = $state(null);
  // Whether the create form is open. While it is, the tab shows only the form.
  let creating = $state(false);
  let draft = $state({ name: "", role: "commenter", until: "7d", budget: "" });
  // The token waiting to be revoked, or null. It stops a credential somebody
  // may already be holding.
  let confirming = $state(null);
  // The token just made. It is kept only here and never written to any storage.
  // The reveal shows its connection step until Done.
  let reveal = $state(null);

  const allowed = (role) => allows(sharing.sharing, role);
  const tokens = $derived(sharing.sharing?.tokens ?? []);
  const REVOKE = [m.share_revoke_token_title(), m.share_revoke_token_body(), m.common_revoke_token()];

  // Sharing is only read for an owner, and only while the panel is open. The
  // tasks need neither, so they never wait for it.
  $effect(() => {
    if (open && canShare) {
      sharing.load();
      return () => { sharing.reset(); creating = false; confirming = null; reveal = null; };
    }
  });

  async function focusBack() {
    await tick();
    root?.querySelector("[data-share-new]")?.focus();
  }

  function pick(value) {
    tab = value;
    creating = false;
    // The manual-copy text may hold the token, and each tab shows it.
    sharing.clearCopy();
  }

  function startCreate() {
    draft = { name: "", role: "commenter", until: "7d", budget: "" };
    creating = true;
  }

  function cancelForm() {
    creating = false;
    focusBack();
  }

  async function commit() {
    if (!creating) return;
    // A token needs no name, so an unnamed one is called after its kind.
    const name = draft.name.trim() || m.common_agent();
    const answer = await sharing.change({ create: { kind: "token", role: draft.role, name, until: draft.until, budget: budgetOf(draft.budget) } });
    if (!answer) return;
    creating = false;
    if (answer.created?.token) reveal = { token: answer.created.token };
    else focusBack();
  }

  async function confirmRevoke() {
    if (!confirming) return;
    const { item } = confirming;
    confirming = null;
    await sharing.change({ revoke: { id: item.id } });
    focusBack();
  }

  // The manual-copy fallback may hold the token, so it goes with the reveal.
  function done() {
    if (!reveal) return;
    reveal = null;
    sharing.clearCopy();
    focusBack();
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

{#snippet tokensPane()}
  {#if creating}
    <h3 class="agent-title">{m.agent_new_token()}</h3>
    <ShareForm kind="token" bind:draft busy={sharing.busy} {allowed} onsubmit={commit} oncancel={cancelForm} />
    {@render messages()}
  {:else}
    <section class="agent-step">
      <div class="agent-step-head">
        <span class="token-number" aria-hidden="true">1</span>
        <h3 class="agent-title">{m.agent_step_install()}</h3>
      </div>
      <div class="agent-step-body">
        <p class="agent-help">{m.agent_step_install_help()}</p>
        <button type="button" class="btn btn-sm lp-control-tonal-brand agent-step-button" class:agent-step-failed={sharing.copyFallback === INSTALL_INSTRUCTIONS}
                onclick={() => sharing.copy(INSTALL_INSTRUCTIONS)}>
          <Icon name={sharing.copied === INSTALL_INSTRUCTIONS ? "check" : "copy"} size={14} />
          {#if sharing.copyFallback === INSTALL_INSTRUCTIONS}{m.tokenreveal_copy_failed()}{:else if sharing.copied === INSTALL_INSTRUCTIONS}{m.tokenreveal_copied()}{:else}{m.agent_copy_install()}{/if}
        </button>
      </div>
    </section>
    <div class="agent-divider"></div>
    <section class="agent-step">
      <div class="agent-step-head">
        <span class="token-number" aria-hidden="true">2</span>
        <h3 class="agent-title">{m.agent_step_connect()}</h3>
      </div>
      <div class="agent-step-body">
        {#if !reveal}<p class="agent-help">{m.agent_step_connect_help()}</p>{/if}
        {#if reveal}
          <TokenReveal token={reveal.token} copied={sharing.copied} copyFallback={sharing.copyFallback} error={sharing.error} oncopy={sharing.copy} ondone={done} />
        {:else if sharing.loading}
          <p class="panel-muted" role="status">{m.share_loading()}</p>
        {:else if sharing.sharing}
          {#if tokens.length > 0}
            <ul class="share-list" aria-label={m.agent_tab_tokens()}>
              {#each tokens as item (item.id)}
                <ShareRow kind="token" {item} busy={sharing.busy} copied={sharing.copied} oncopy={sharing.copy} onrevoke={() => (confirming = { item })} />
              {/each}
            </ul>
          {/if}
          <button type="button" class="btn btn-sm agent-step-button {tokens.length === 0 ? 'lp-control-brand' : 'lp-control-tonal-brand'}" data-share-new disabled={sharing.busy} onclick={startCreate}>
            <Icon name="plus" size={14} />{m.agent_new_token()}
          </button>
          {#if tokens.length > 0}<p class="panel-meta">{m.agent_token_warning()}</p>{/if}
        {:else if sharing.error}
          <button class="btn btn-sm lp-control-tonal-brand" onclick={() => sharing.load()}>{m.common_try_again()}</button>
        {/if}
      </div>
    </section>
    {#if !reveal}{@render messages()}{/if}
  {/if}
{/snippet}

{#snippet tasksPane()}
  <p class="panel-muted">{m.agent_tasks_intro()}</p>
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
          <li class="agent-task">
            <div class="agent-task-text">
              <span class="agent-task-name">{task.label}</span>
              <span class="agent-task-body">{task.text}</span>
            </div>
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

<section class="panel panel-tabbed agent-panel" aria-label={m.agent_panel_label()} bind:this={root}>
  <PanelHeader title={m.common_agent()} />
  {#if canShare}
    <PanelTabs id="agent" label={m.common_agent()} tabs={TABS} value={current} onchange={pick}>
      <Tabs.Content value="tokens">{#if current === "tokens"}{@render tokensPane()}{/if}</Tabs.Content>
      <Tabs.Content value="tasks">{#if current === "tasks"}{@render tasksPane()}{/if}</Tabs.Content>
    </PanelTabs>
  {:else}
    <div class="agent-solo">
      {@render tasksPane()}
    </div>
  {/if}
</section>

{#if confirming}
  <Modal open title={REVOKE[0]} onclose={() => (confirming = null)}
         confirm={{ label: REVOKE[2], tone: "error", oncancel: () => (confirming = null), onclick: confirmRevoke }}>
    <p>{REVOKE[1]}</p>
  </Modal>
{/if}

<style>
  .agent-panel :global([role="tabpanel"]) { overflow-y: auto; gap: calc(var(--spacing) * 2); }
  .agent-panel :global([role="tabpanel"]) > :global(*) { flex-shrink: 0; }
  .agent-solo { display: flex; flex: 1 1 0; min-height: 0; flex-direction: column; gap: calc(var(--spacing) * 2); overflow-y: auto; padding: var(--panel-padding); }
  .agent-solo > :global(*) { flex-shrink: 0; }
  /* The step circle is the one the reveal's steps share, so its rule lives here. */
  .agent-panel :global(.token-number) { display: grid; flex: none; place-items: center; width: 28px; height: 28px; border-radius: 9999px; background: var(--color-row-hover); font-size: 13px; font-weight: 500; }
  /* Each step is a circle and heading, with its content indented under the heading text. */
  .agent-step { display: grid; gap: calc(var(--spacing) * 2); }
  .agent-step-head { display: flex; align-items: center; gap: calc(var(--spacing) * 3); }
  .agent-step-body { display: grid; gap: calc(var(--spacing) * 2); min-width: 0; padding-left: calc(28px + var(--spacing) * 3); }
  .agent-title { margin: 0; font-size: 16px; font-weight: 600; }
  .agent-help { margin: 0; font-size: 13px; color: var(--panel-muted); }
  .agent-divider { border-top: 1px solid var(--color-divider); }
  .agent-step-button { display: flex; align-items: center; justify-content: center; gap: calc(var(--spacing) * 2); width: 100%; }
  .agent-step-failed { color: var(--color-error-text); }
  .share-list { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  /* The tasks are a list of two-line rows: the name, then the instruction in
     muted type, with one copy button at the right. Only the button is live. */
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
</style>
