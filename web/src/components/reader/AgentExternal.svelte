<script>
  // The Connect tab: a connector for apps and agents that can add one, then the
  // tokens for everything else. Copying a token gives one block that installs
  // the skill if it is missing and connects with the token, so there are no
  // steps to follow. The shell mounts this only for an owner and owns the
  // sharing state, which the Tasks tab's copy uses too.
  import { tick } from "svelte";
  import Icon from "../Icon.svelte";
  import IconButton from "../IconButton.svelte";
  import Modal from "../Modal.svelte";
  import ShareRow from "./share/ShareRow.svelte";
  import ShareForm from "./share/ShareForm.svelte";
  import TokenReveal from "./share/TokenReveal.svelte";
  import ConnectionMethods from "./share/ConnectionMethods.svelte";
  import { allows, budgetOf } from "./share/shared.js";
  import { AGENTS_CONNECT_URL } from "../../lib/site-links.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { open = false, sharing, slug = "" } = $props();

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
  // The "Command line and MCP configuration" disclosure. It lives here, not in
  // ConnectionMethods, because the reveal and the normal view mount separate
  // instances, and Done would otherwise fold it shut.
  let methodsOpen = $state(false);

  const allowed = (role) => allows(sharing.sharing, role);
  const tokens = $derived(sharing.sharing?.tokens ?? []);
  const copyable = $derived(tokens.filter((item) => item.token));
  // The connector URL names the project; an app or agent adds it as a custom
  // connector. Shown only when the deployment offers connectors.
  const connectorUrl = $derived(slug ? `${location.origin}/mcp/${slug}` : "");
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

{#snippet methods()}
  <ConnectionMethods token={reveal?.token ?? ""} tokens={copyable} copied={sharing.copied} copyFallback={sharing.copyFallback} oncopy={sharing.copy} bind:open={methodsOpen} />
{/snippet}

<div class="agent-external" bind:this={root}>
  {#if creating}
    <h3 class="agent-title">{m.agent_new_token()}</h3>
    <ShareForm kind="token" bind:draft busy={sharing.busy} {allowed} onsubmit={commit} oncancel={cancelForm} />
    {@render messages()}
  {:else if reveal}
    <TokenReveal token={reveal.token} copied={sharing.copied} copyFallback={sharing.copyFallback} error={sharing.error} oncopy={sharing.copy} ondone={done} />
    <section class="agent-section">{@render methods()}</section>
  {:else if sharing.loading}
    <p class="panel-muted" role="status">{m.share_loading()}</p>
    {@render messages()}
  {:else if sharing.sharing}
    {#if tokens.length > 0}
      <section class="agent-section" aria-labelledby="agent-rows-title">
        <h3 class="agent-subtitle" id="agent-rows-title">{m.agentexternal_rows_title()}</h3>
        <ul class="share-list" aria-label={m.common_agent()}>
          {#each tokens as item (item.id)}
            <ShareRow kind="token" {item} busy={sharing.busy} copied={sharing.copied} oncopy={sharing.copy} onrevoke={() => (confirming = { item })} />
          {/each}
        </ul>
      </section>
    {/if}
    {#if sharing.sharing.connectors === true && connectorUrl}
      <section class="agent-section agent-connector" aria-labelledby="agent-connector-title">
        <h3 class="agent-title" id="agent-connector-title">{m.agentexternal_connect_title()}</h3>
        <p class="agent-help">{m.agentexternal_connect_lede()}</p>
        <ol class="agent-help agent-steps">
          <li>
            <div class="agent-step-body">
              {m.agentexternal_step_copy()}
              <div class="agent-connector-row">
                <input class="input agent-connector-url" readonly value={connectorUrl} aria-label={m.agentexternal_connector_url_label()} onclick={(event) => event.currentTarget.select()} />
                <IconButton icon={sharing.copied === connectorUrl ? "check" : "copy"} label={m.agentexternal_connector_copy()} title={m.agentexternal_connector_copy()} tone="plain" size="btn-icon-sm"
                            colour={sharing.copyFallback === connectorUrl ? "lp-text-error" : null} onclick={() => sharing.copy(connectorUrl)} />
              </div>
            </div>
          </li>
          <li>
            <div class="agent-step-body">
              {m.agentexternal_step_add()}
              <a href={AGENTS_CONNECT_URL} target="_blank" rel="noreferrer">{m.agentexternal_connect_docs()}</a>
            </div>
          </li>
          <li>{m.agentexternal_step_approve()}</li>
        </ol>
      </section>
    {/if}
    <section class="agent-section" aria-labelledby="agent-tokens-title">
      <h3 class="agent-title" id="agent-tokens-title">{m.agentexternal_tokens_title()}</h3>
      <p class="agent-help">{m.agentexternal_tokens_help()}</p>
      <button type="button" class="btn btn-sm lp-control-tonal-brand agent-new" data-share-new disabled={sharing.busy} onclick={startCreate}>
        <Icon name="plus" size={14} />{m.agent_new_token()}
      </button>
      {#if tokens.length > 0}<p class="panel-meta">{m.agent_token_warning()}</p>{/if}
      {@render messages()}
      {@render methods()}
    </section>
  {:else if sharing.error}
    <button class="btn btn-sm lp-control-tonal-brand" onclick={() => sharing.load()}>{m.common_try_again()}</button>
    {@render messages()}
  {/if}
</div>

{#if confirming}
  <Modal open title={REVOKE[0]} onclose={() => (confirming = null)}
         confirm={{ label: REVOKE[2], tone: "error", oncancel: () => (confirming = null), onclick: confirmRevoke }}>
    <p>{REVOKE[1]}</p>
  </Modal>
{/if}

<style>
  .agent-external { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); min-width: 0; }
  .agent-title { margin: 0; font-size: 16px; font-weight: 600; }
  .agent-help { margin: 0; font-size: 13px; color: var(--panel-muted); }
  .agent-section { display: grid; gap: calc(var(--spacing) * 2); min-width: 0; }
  .agent-subtitle { margin: 0; font-size: 13px; font-weight: 600; color: var(--panel-muted); }
  .agent-connector { display: grid; gap: calc(var(--spacing) * 2); min-width: 0; }
  /* The connector steps are numbered by the list; each step's controls sit under its text. */
  /* The list draws the step numbers; each li stays a list item so the marker shows. */
  .agent-steps { margin: 0; padding-left: 1.25em; list-style: decimal; min-width: 0; }
  .agent-steps li { display: list-item; min-width: 0; }
  .agent-steps li + li { margin-top: calc(var(--spacing) * 2); }
  .agent-step-body { display: grid; gap: calc(var(--spacing) * 1.5); min-width: 0; }
  .agent-steps a { color: var(--color-link); text-decoration: underline; justify-self: start; overflow-wrap: anywhere; }
  .agent-connector-row { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-width: 0; }
  .agent-connector-url { flex: 1 1 auto; min-width: 0; font-family: var(--font-mono, monospace); font-size: 12px; }
  .agent-new { display: flex; align-items: center; justify-content: center; gap: calc(var(--spacing) * 2); width: 100%; }
  .share-list { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--color-divider); }
  .share-copy-fallback { border: 0; background: transparent; resize: none; color: var(--color-text-secondary); font-size: var(--panel-meta-size); }
  /* Phones: the share controls are finger-sized. */
  @media (max-width: 767px) {
    .agent-external :global(.icon-control) { min-width: 2.75rem; min-height: 2.75rem; }
    .agent-new { min-height: 2.75rem; }
  }
</style>
