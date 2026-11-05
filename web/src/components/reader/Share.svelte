<script>
  import { tick } from "svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import Modal from "../Modal.svelte";
  import Icon from "../Icon.svelte";
  import ShareRow from "./share/ShareRow.svelte";
  import ShareForm from "./share/ShareForm.svelte";
  import { allows, budgetOf } from "./share/shared.js";
  import { createSharing } from "../../lib/sharing.svelte.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { open = $bindable(false), slug, onclose, inline = false, canShare = false } = $props();
  const sharing = createSharing({ slug: () => slug, canShare: () => canShare });
  let root = $state(null);
  // Whether the create form is open. It stands in for the toolbar.
  let creating = $state(false);
  let draft = $state({ name: "", role: "reader", until: "7d", budget: "" });
  // The link waiting to be revoked, or null. It stops a link somebody may
  // already be holding.
  let confirming = $state(null);

  const allowed = (role) => allows(sharing.sharing, role);
  const links = $derived(sharing.sharing?.links ?? []);

  $effect(() => {
    if (open) {
      sharing.load();
      return () => { sharing.reset(); creating = false; confirming = null; };
    }
  });

  async function focusBack() {
    await tick();
    root?.querySelector("[data-share-new]")?.focus();
  }

  function startCreate() {
    draft = { name: "", role: "reader", until: "7d", budget: "" };
    creating = true;
  }

  function cancelForm() {
    creating = false;
    focusBack();
  }

  async function commit() {
    if (!creating) return;
    const answer = await sharing.change({ create: { kind: "link", role: draft.role, name: draft.name.trim(), until: draft.until, budget: budgetOf(draft.budget) } });
    if (!answer) return;
    creating = false;
    focusBack();
  }

  async function confirmRevoke() {
    if (!confirming) return;
    const { item } = confirming;
    confirming = null;
    await sharing.change({ revoke: { id: item.id } });
    focusBack();
  }

  const ask = [m.share_revoke_link_title(), m.share_revoke_link_body(), m.common_revoke_link()];
</script>

{#snippet messages()}
  {#if sharing.error}
    <p class="lp-text-error" role="alert">{sharing.error}</p>
  {/if}
  {#if sharing.copyFallback}
    <textarea class="share-copy-fallback w-full" readonly rows="3" aria-label={m.share_copy_manual_label()} value={sharing.copyFallback} onclick={(event) => event.currentTarget.select()}></textarea>
  {/if}
{/snippet}

{#snippet content()}
  {#if sharing.loading}
    <p class="panel-muted share-pad" role="status">{m.share_loading()}</p>
  {:else if sharing.sharing && canShare}
    <div class="share-pane">
      {#if !creating}
        <div class="share-toolbar">
          <button type="button" class="share-new" data-share-new disabled={sharing.busy} onclick={startCreate}>
            <Icon name="plus" size={14} />{m.share_new()}
          </button>
        </div>
      {/if}
      <div class="share-scroll">
        <!-- Inside the scroller, so a short panel still reaches the form's buttons. -->
        {#if creating}
          <ShareForm kind="link" bind:draft busy={sharing.busy} {allowed} onsubmit={commit} oncancel={cancelForm} />
        {/if}
        <p class="panel-meta">{m.share_links_scope_note()}</p>
        {#if links.length === 0}
          <p class="panel-meta">{m.share_no_links()}</p>
        {:else}
          <ul class="share-list" aria-label={m.share_links_list_label()}>
            {#each links as item (item.id)}
              <ShareRow kind="link" {item} busy={sharing.busy} copied={sharing.copied} oncopy={sharing.copy} onrevoke={() => (confirming = { item })} />
            {/each}
          </ul>
        {/if}
        {@render messages()}
      </div>
    </div>
  {:else}
    <div class="share-pad">
      {@render messages()}
      {#if sharing.error && !sharing.sharing}<button class="btn btn-sm lp-control-tonal-brand" onclick={() => sharing.load()}>{m.common_try_again()}</button>{/if}
    </div>
  {/if}
{/snippet}

<div bind:this={root} class="share-root">
  {#if inline}
    <section class="panel panel-tabbed share-sidebar" aria-label={m.common_share()}>
      <PanelHeader title={m.common_share()} />
      {@render content()}
    </section>
  {:else}
    <Modal bind:open title={m.common_share()} onclose={onclose}
           confirm={{ label: m.common_done(), cancel: false, onclick: () => (open = false) }}>
      <div class="panel-tabbed share-modal-body">
        {@render content()}
      </div>
    </Modal>
  {/if}
</div>

{#if confirming}
  <Modal open title={ask[0]} onclose={() => (confirming = null)}
         confirm={{ label: ask[2], tone: "error", oncancel: () => (confirming = null), onclick: confirmRevoke }}>
    <p>{ask[1]}</p>
  </Modal>
{/if}

<style>
  .share-root { display: contents; }
  .share-sidebar :global(select) { max-width: 100%; }
  .share-modal-body { height: min(30rem, 60vh); min-width: 0; }
  .share-pane { display: flex; flex: 1 1 0; min-height: 0; flex-direction: column; padding: var(--panel-padding); }
  .share-pad { padding: var(--panel-padding); overflow-y: auto; }
  .share-toolbar { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 1.5); }
  .share-new { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1); padding: .2rem .5rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; font-size: 13px; cursor: pointer; }
  .share-new:hover:not(:disabled) { background: var(--color-row-hover); }
  .share-new:disabled { opacity: .5; cursor: default; }
  .share-scroll { flex: 1 1 0; min-height: 0; overflow-y: auto; overflow-x: hidden; }
  .share-scroll > p { margin-block: calc(var(--spacing) * 1); }
  .share-list { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-text-muted); font-size: var(--panel-meta-size); }
</style>
