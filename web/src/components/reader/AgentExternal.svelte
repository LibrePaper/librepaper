<script>
  // The Agent tab: the tokens that let an agent you run yourself reach
  // this project. Copying a token gives one block that installs the skill if it
  // is missing and connects with the token, so there are no steps to follow.
  // The shell mounts this only for an owner and owns the sharing state, which
  // the Tasks tab's copy uses too.
  import { tick } from "svelte";
  import Icon from "../Icon.svelte";
  import Modal from "../Modal.svelte";
  import ShareRow from "./share/ShareRow.svelte";
  import ShareForm from "./share/ShareForm.svelte";
  import TokenReveal from "./share/TokenReveal.svelte";
  import ConnectionMethods from "./share/ConnectionMethods.svelte";
  import { allows, budgetOf } from "./share/shared.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { open = false, sharing } = $props();

  let root = $state(null);
  // Whether the create form is open. While it is, the tab shows only the form.
  let creating = $state(false);
  let draft = $state({ name: "", role: "commenter", until: "7d", budget: "" });
  // The token waiting to be revoked, or null. It stops a credential somebody
  // may already be holding.
  let confirming = $state(null);
  // The token just made. It is kept only here and never written to any storage.
  // The reveal shows its connection block until Done.
  let reveal = $state(null);

  const allowed = (role) => allows(sharing.sharing, role);
  const tokens = $derived(sharing.sharing?.tokens ?? []);
  const copyable = $derived(tokens.filter((item) => item.token));
  const REVOKE = [m.share_revoke_token_title(), m.share_revoke_token_body(), m.common_revoke_token()];

  // Sharing is read while the panel is open, and reset when it closes.
  $effect(() => {
    if (open) {
      sharing.load();
      return () => { sharing.reset(); creating = false; confirming = null; reveal = null; };
    }
  });

  async function focusBack() {
    await tick();
    root?.querySelector("[data-share-new]")?.focus();
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

<div class="agent-external" bind:this={root}>
  {#if creating}
    <h3 class="agent-title">{m.agent_new_token()}</h3>
    <ShareForm kind="token" bind:draft busy={sharing.busy} {allowed} onsubmit={commit} oncancel={cancelForm} />
    {@render messages()}
  {:else}
    {#if !reveal}<p class="agent-help">{m.agent_external_intro()}</p>{/if}
    {#if reveal}
      <TokenReveal token={reveal.token} copied={sharing.copied} copyFallback={sharing.copyFallback} error={sharing.error} oncopy={sharing.copy} ondone={done} />
    {:else if sharing.loading}
      <p class="panel-muted" role="status">{m.share_loading()}</p>
    {:else if sharing.sharing}
      {#if tokens.length > 0}
        <ul class="share-list" aria-label={m.common_agent()}>
          {#each tokens as item (item.id)}
            <ShareRow kind="token" {item} busy={sharing.busy} copied={sharing.copied} oncopy={sharing.copy} onrevoke={() => (confirming = { item })} />
          {/each}
        </ul>
      {/if}
      <button type="button" class="btn btn-sm lp-control-tonal-brand agent-new" data-share-new disabled={sharing.busy} onclick={startCreate}>
        <Icon name="plus" size={14} />{m.agent_new_token()}
      </button>
      {#if tokens.length > 0}<p class="panel-meta">{m.agent_token_warning()}</p>{/if}
    {:else if sharing.error}
      <button class="btn btn-sm lp-control-tonal-brand" onclick={() => sharing.load()}>{m.common_try_again()}</button>
    {/if}
    {#if !reveal}{@render messages()}{/if}
    {#if reveal || sharing.sharing}
      <ConnectionMethods token={reveal?.token ?? ""} tokens={copyable} copied={sharing.copied} copyFallback={sharing.copyFallback} oncopy={sharing.copy} />
    {/if}
  {/if}
</div>

{#if confirming}
  <Modal open title={REVOKE[0]} onclose={() => (confirming = null)}
         confirm={{ label: REVOKE[2], tone: "error", oncancel: () => (confirming = null), onclick: confirmRevoke }}>
    <p>{REVOKE[1]}</p>
  </Modal>
{/if}

<style>
  /* A flex column that fills the tab, so ConnectionMethods' margin-top: auto
     pushes it to the foot. */
  .agent-external { display: flex; flex: 1 0 auto; flex-direction: column; gap: calc(var(--spacing) * 2); min-width: 0; }
  .agent-title { margin: 0; font-size: 16px; font-weight: 600; }
  .agent-help { margin: 0; font-size: 13px; color: var(--panel-muted); }
  .agent-new { display: flex; align-items: center; justify-content: center; gap: calc(var(--spacing) * 2); width: 100%; }
  .share-list { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-text-muted); font-size: var(--panel-meta-size); }
  /* Phones: the share controls are finger-sized. */
  @media (max-width: 767px) {
    .agent-external :global(.icon-control) { min-width: 2.75rem; min-height: 2.75rem; }
    .agent-new { min-height: 2.75rem; }
  }
</style>
