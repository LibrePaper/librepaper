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
  import { allows, budgetOf, INSTALL_COMMAND } from "./share/shared.js";
  import { createSharing } from "../../lib/sharing.svelte.js";
  import { TASK_GROUPS, filterTaskGroups } from "../../lib/agent-tasks.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { open = false, slug, canShare = false } = $props();
  const sharing = createSharing({ slug: () => slug, canShare: () => canShare });

  // Tokens is the first and default tab for an owner.
  const TABS = [{ id: "tokens", label: m.agent_tab_tokens() }, { id: "tasks", label: m.agent_tab_tasks() }];
  let tab = $state("tokens");
  const current = $derived(canShare ? tab : "tasks");

  // The search box filters the tasks. Folds are the ids of closed groups; every
  // group starts open, and the folds are kept only while the panel is mounted.
  let taskQuery = $state("");
  let folded = $state([]);
  const shownGroups = $derived(filterTaskGroups(TASK_GROUPS, taskQuery));
  const searching = $derived(taskQuery.trim() !== "");
  const isOpen = (group) => searching || !folded.includes(group.id);

  function toggleGroup(id) {
    folded = folded.includes(id) ? folded.filter((item) => item !== id) : [...folded, id];
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
  // Whether the create form is open. It stands in for the toolbar.
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
  <section class="agent-install">
    <div class="agent-install-head">
      <span class="token-number" aria-hidden="true">1</span>
      <div class="agent-install-text">
        <h3 class="agent-install-title">{m.tokenreveal_step_install()}</h3>
        <p class="agent-install-help">{m.agent_install_help()}</p>
      </div>
    </div>
    <div class="agent-install-command">
      <code>{INSTALL_COMMAND}</code>
      <IconButton icon={sharing.copied === INSTALL_COMMAND ? "check" : "copy"} label={m.agent_copy_install_command()} title={m.agent_copy_install_command()} tone="plain" size="btn-icon-sm"
                  onclick={() => sharing.copy(INSTALL_COMMAND)} />
    </div>
  </section>
  {#if reveal}
    <TokenReveal token={reveal.token} copied={sharing.copied} copyFallback={sharing.copyFallback} error={sharing.error} oncopy={sharing.copy} ondone={done} />
  {:else if sharing.loading}
    <p class="panel-muted" role="status">{m.share_loading()}</p>
  {:else if sharing.sharing}
    <p class="panel-meta">{m.share_tokens_intro()}</p>
    {#if !creating}
      <div class="share-toolbar">
        <button type="button" class="share-new" data-share-new disabled={sharing.busy} onclick={startCreate}>
          <Icon name="plus" size={14} />{m.share_new()}
        </button>
      </div>
    {/if}
    {#if creating}
      <ShareForm kind="token" bind:draft busy={sharing.busy} {allowed} onsubmit={commit} oncancel={cancelForm} />
    {/if}
    {#if tokens.length === 0}
      <p class="panel-meta">{m.share_no_tokens()}</p>
    {:else}
      <ul class="share-list" aria-label={m.agent_tab_tokens()}>
        {#each tokens as item (item.id)}
          <ShareRow kind="token" {item} busy={sharing.busy} copied={sharing.copied} oncopy={sharing.copy} onrevoke={() => (confirming = { item })} />
        {/each}
      </ul>
      <p class="panel-meta">{m.tokenreveal_warning()}</p>
    {/if}
    <p class="panel-meta">{m.share_tokens_scope_note()}</p>
    {@render messages()}
  {:else}
    {@render messages()}
    {#if sharing.error}<button class="btn btn-sm lp-control-tonal-brand" onclick={() => sharing.load()}>{m.common_try_again()}</button>{/if}
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
  /* The install step at the top of the Tokens tab. The step circle is shared
     with the reveal's connection step, so its rule lives here for both. */
  .agent-panel :global(.token-number) { display: grid; flex: none; place-items: center; width: 28px; height: 28px; border-radius: 9999px; background: var(--color-row-hover); font-size: 13px; font-weight: 500; }
  .agent-install { padding-bottom: calc(var(--spacing) * 3); border-bottom: 1px solid var(--color-divider); font-size: 13px; }
  .agent-install-head { display: flex; align-items: flex-start; gap: calc(var(--spacing) * 3); }
  .agent-install-text { min-width: 0; }
  .agent-install-title { margin: 0; font-size: 14px; font-weight: 500; }
  .agent-install-help { margin: calc(var(--spacing) * 1) 0 0; font-size: 13px; color: var(--panel-muted); }
  .agent-install-command { display: flex; align-items: center; gap: calc(var(--spacing) * 2); margin-top: calc(var(--spacing) * 3); }
  .agent-install-command code { flex: 1 1 auto; min-width: 0; padding: calc(var(--spacing) * 2) calc(var(--spacing) * 3); border-radius: var(--radius-base); background: var(--color-row-hover); font-family: var(--font-mono, monospace); font-size: 12px; overflow-wrap: anywhere; }
  .share-toolbar { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 1.5); }
  .share-new { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1); padding: .2rem .5rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; font-size: 13px; cursor: pointer; }
  .share-new:hover:not(:disabled) { background: var(--color-row-hover); }
  .share-new:disabled { opacity: .5; cursor: default; }
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
