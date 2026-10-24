<script>
  import { onDestroy, tick } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import Modal from "../Modal.svelte";
  import Icon from "../Icon.svelte";
  import ShareRow from "./share/ShareRow.svelte";
  import ShareForm from "./share/ShareForm.svelte";
  import TokenReveal from "./share/TokenReveal.svelte";
  import { getPrivate, post } from "../../lib/api.js";

  let { open = $bindable(false), slug, onclose, inline = false, canShare = false } = $props();
  let sharing = $state(null);
  let busy = $state(false);
  let loading = $state(false);
  let error = $state("");
  let generation = 0;
  let copied = $state("");
  let copyFallback = $state("");
  let copyTimer;
  // Bumped whenever the reveal ends or the panel closes, so a copy that
  // settles afterwards cannot put a dismissed token back on screen.
  let copyGeneration = 0;
  onDestroy(() => clearTimeout(copyTimer));
  let tab = $state("links");
  let root = $state(null);
  // The kind of the create form that is open, or null. Only one is ever open,
  // and it stands in for the toolbar.
  let creating = $state(null);
  let draft = $state({ name: "", role: "reader", until: "7d", budget: "" });
  // `{ item, kind }` while a revocation waits to be confirmed. It stops a
  // credential somebody may already be holding.
  let confirming = $state(null);
  // The token just made. It is shown once, kept only here, and
  // never written to any storage.
  let reveal = $state(null);

  const TABS = [{ id: "links", label: "Links" }, { id: "tokens", label: "Agent tokens" }];
  const kindOf = (id) => (id === "links" ? "link" : "token");
  const itemsOf = (kind) => (kind === "link" ? sharing?.links : sharing?.tokens) ?? [];
  // `publishers` and `commenters_policy` describe who the deployment lets
  // through; "nobody" is the one description that rules a permission out.
  const allowed = (role) => role === "reader" || (role === "commenter"
    ? sharing?.commenters_policy !== "nobody" : sharing?.publishers !== "nobody");

  async function load(documentSlug) {
    const request = ++generation;
    loading = true;
    copyFallback = "";
    error = "";
    try {
      const answer = canShare ? await getPrivate(`/api/documents/${documentSlug}/share`) : { links: [], tokens: [] };
      if (request !== generation) return;
      sharing = answer;
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not load sharing settings.";
    } finally {
      if (request === generation) loading = false;
    }
  }

  $effect(() => {
    const documentSlug = slug;
    if (open) {
      load(documentSlug);
      return () => { generation += 1; creating = null; confirming = null; reveal = null; copyFallback = ""; copyGeneration += 1; };
    }
  });

  // Runs one share action and answers the server's reply, or null.
  async function change(body) {
    if (busy || !canShare) return null;
    const documentSlug = slug;
    const request = generation;
    busy = true;
    error = "";
    try {
      const answer = await post(`/api/documents/${documentSlug}/share`, body);
      if (documentSlug !== slug || request !== generation) return null;
      copyFallback = "";
      sharing = answer;
      return answer;
    } catch (failure) {
      if (request === generation) error = failure.message || "Could not update access.";
      return null;
    } finally { busy = false; }
  }

  async function focusBack() {
    await tick();
    root?.querySelector("[data-share-new]")?.focus();
  }

  function startCreate(kind) {
    draft = { name: "", role: kind === "link" ? "reader" : "commenter", until: "7d", budget: "" };
    creating = kind;
  }

  function cancelForm() {
    creating = null;
    focusBack();
  }

  const budgetOf = (value) => (value == null || value === "" ? null : Number(value));

  async function commit() {
    if (!creating) return;
    const kind = creating;
    const answer = await change({ create: { kind, role: draft.role, name: draft.name.trim(), until: draft.until, budget: budgetOf(draft.budget) } });
    if (!answer) return;
    creating = null;
    if (answer.created?.token) reveal = { token: answer.created.token };
    else focusBack();
  }

  async function confirmRevoke() {
    if (!confirming) return;
    const { item } = confirming;
    confirming = null;
    await change({ revoke: { id: item.id } });
    focusBack();
  }

  // The manual-copy fallback may hold the token or its configuration, so it
  // goes with the reveal.
  function done() {
    reveal = null;
    copyFallback = "";
    error = "";
    copyGeneration += 1;
    focusBack();
  }

  async function copy(value) {
    const attempt = copyGeneration;
    try {
      await navigator.clipboard.writeText(value);
      if (attempt !== copyGeneration) return;
      copied = value;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = ""), 1800);
      copyFallback = "";
      error = "";
    } catch {
      if (attempt !== copyGeneration) return;
      copyFallback = value;
      error = "Copy was blocked. Select this text and copy it manually.";
    }
  }

  const CONFIRM = {
    link: ["Revoke link?", "This link will stop working. People who only have access through this link will lose access.", "Revoke link"],
    token: ["Revoke token?", "This token will stop working. The agent loses access at once.", "Revoke token"],
  };
  const ask = $derived(confirming ? CONFIRM[confirming.kind] : null);
</script>

{#snippet messages()}
  {#if error}
    <p class="lp-text-error" role="alert">{error}</p>
  {/if}
  {#if copyFallback}
    <textarea class="share-copy-fallback w-full" readonly rows="3" aria-label="Text to copy manually" value={copyFallback} onclick={(event) => event.currentTarget.select()}></textarea>
  {/if}
{/snippet}

{#snippet list(kind)}
  {@const active = itemsOf(kind)}
  {#if creating !== kind}
    <div class="share-toolbar">
      <button type="button" class="share-new" data-share-new disabled={busy || creating !== null} onclick={() => startCreate(kind)}>
        <Icon name="plus" size={14} />New
      </button>
    </div>
  {/if}
  <div class="share-scroll">
    <!-- Inside the scroller, so a short panel still reaches the form's buttons. -->
    {#if creating === kind}
      <ShareForm {kind} bind:draft {busy} {allowed} onsubmit={commit} oncancel={cancelForm} />
    {/if}
    {#if kind === "link"}
      <p class="panel-meta">Every link opens the whole project. The permission decides what people can do.</p>
    {/if}
    {#if active.length === 0}
      <p class="panel-meta">{kind === "link" ? "No sharing links yet." : "No agent tokens yet."}</p>
    {:else}
      <ul class="share-list" aria-label={kind === "link" ? "Share links" : "Agent tokens"}>
        {#each active as item (item.id)}
          <ShareRow {kind} {item} {busy} {copied} oncopy={copy} onrevoke={() => (confirming = { item, kind })} />
        {/each}
      </ul>
    {/if}
    {@render messages()}
  </div>
{/snippet}

{#snippet content()}
  {#if reveal}
    <div class="share-pad">
      <TokenReveal token={reveal.token} oncopy={copy} ondone={done} />
      {@render messages()}
    </div>
  {:else if loading}
    <p class="panel-muted share-pad" role="status">Loading sharing settings…</p>
  {:else if sharing && canShare}
    <PanelTabs id="share" label="Share" tabs={TABS} value={tab} onchange={(value) => { tab = value; creating = null; }}>
      <Tabs.Content value="links">{#if tab === "links"}{@render list("link")}{/if}</Tabs.Content>
      <Tabs.Content value="tokens">{#if tab === "tokens"}{@render list("token")}{/if}</Tabs.Content>
    </PanelTabs>
  {:else}
    <div class="share-pad">
      {@render messages()}
      {#if error && !sharing}<button class="btn btn-sm lp-control-tonal-brand" onclick={() => load(slug)}>Try again</button>{/if}
    </div>
  {/if}
{/snippet}

<div bind:this={root} class="share-root">
  {#if inline}
    <section class="panel panel-tabbed share-sidebar" aria-label="Share">
      <PanelHeader title="Share" />
      {@render content()}
    </section>
  {:else}
    <Modal bind:open title="Share" onclose={onclose}
           confirm={{ label: "Done", cancel: false, onclick: () => (open = false) }}>
      <div class="panel-tabbed share-modal-body">
        {@render content()}
      </div>
    </Modal>
  {/if}
</div>

{#if confirming && ask}
  <Modal open title={ask[0]} onclose={() => (confirming = null)}
         confirm={{ label: ask[2], tone: "error", oncancel: () => (confirming = null), onclick: confirmRevoke }}>
    <p>{ask[1]}</p>
  </Modal>
{/if}

<style>
  .share-root { display: contents; }
  .share-sidebar :global(select) { max-width: 100%; }
  .share-modal-body { height: min(30rem, 60vh); min-width: 0; }
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
