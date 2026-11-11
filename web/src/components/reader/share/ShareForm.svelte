<script>
  import { onMount } from "svelte";
  import { m } from "../../../lib/paraglide/messages.js";
  import { EXPIRIES, ROLES } from "./shared.js";

  // The form for a new link or token.
  let { kind, draft = $bindable(), busy = false, allowed = () => true, anonymousAllowed = () => false, onsubmit, oncancel } = $props();
  let nameInput;
  let permission = $state();
  // A token starts on its permission; a link starts on its name.
  onMount(() => (kind === "token" ? permission : nameInput)?.focus());
  const expiries = $derived(kind === "token" ? EXPIRIES.filter(([value]) => value !== "never") : EXPIRIES);
  // Only the roles this user may grant are offered.
  const offered = $derived(ROLES.filter((role) => allowed(role.id)));
  // A role that is not on offer is never the one the form submits.
  $effect(() => { if (offered.length && !offered.some((role) => role.id === draft.role)) draft.role = offered[0].id; });
  const spends = $derived(draft.role !== "reader");
  // A link can let anyone in without a sign-in, if the deployment allows it for
  // its role. Where it does not, the choice falls to signing in.
  const canAnyone = $derived(anonymousAllowed(draft.role));
  const anyone = $derived(draft.anonymous && canAnyone);
  const reach = {
    reader: m.shareform_reach_reader(),
    commenter: m.shareform_reach_commenter(),
    suggester: m.shareform_reach_suggester(),
    editor: m.shareform_reach_editor(),
  };
</script>

{#snippet nameField()}
  <label class="share-field">
    <span class="share-label">{kind === "link" ? m.shareform_name_optional() : m.common_agent_name()}</span>
    <input class="input share-input" type="text" maxlength="80" placeholder={kind === "token" ? m.shareform_optional() : undefined} bind:this={nameInput} bind:value={draft.name} disabled={busy} />
  </label>
{/snippet}

{#snippet budgetField()}
  {#if spends}
    <label class="share-field">
      <span class="share-label">{m.shareform_budget_label()}</span>
      <input class="input share-input" type="number" min="0" step="1" placeholder={kind === "token" ? m.shareform_optional() : undefined} bind:value={draft.budget} disabled={busy} />
      {#if kind === "link"}<span class="share-note">{m.shareform_budget_hint()}</span>{/if}
    </label>
  {/if}
{/snippet}

<form class="share-form" onsubmit={(event) => { event.preventDefault(); if (!busy) onsubmit(); }}>
  <div class="share-group">
    {#if kind === "link"}
      {@render nameField()}
    {/if}
    <label class="share-field">
      <span class="share-label">{m.shareform_permission()}</span>
      {#if offered.length === 1}
        <!-- One role on offer: say it rather than offer a choice of one. -->
        <span class="share-text">{offered[0].label}</span>
      {:else}
      <select class="select share-select" bind:this={permission} bind:value={draft.role} disabled={busy} onchange={(event) => { if (kind === "token" && event.currentTarget.value === "editor" && draft.until === "7d") draft.until = "1d"; if (kind === "link") draft.anonymous = event.currentTarget.value === "reader"; }}>
        {#each offered as role (role.id)}
          <option value={role.id}>{role.label}</option>
        {/each}
      </select>
      {/if}
      {#if kind === "link"}<span class="share-note">{reach[draft.role]}</span>{/if}
    </label>
    {#if kind === "link"}
      <fieldset class="share-field share-choices" disabled={busy}>
        <legend class="share-label">{m.shareform_who_label()}</legend>
        {#if canAnyone}
          <label class="share-choice">
            <input type="radio" name="share-who" value="anyone" checked={anyone} onchange={() => (draft.anonymous = true)} />
            <span>{m.shareform_who_anyone()}</span>
          </label>
          <label class="share-choice">
            <input type="radio" name="share-who" value="signin" checked={!anyone} onchange={() => (draft.anonymous = false)} />
            <span>{m.shareform_who_signin()}</span>
          </label>
        {:else}
          <p class="share-text">{m.shareform_who_signin()}</p>
        {/if}
      </fieldset>
    {/if}
    {#if kind === "token" && draft.role === "editor"}
      <p class="share-note">{m.shareform_edit_note()}</p>
    {/if}
  </div>
  <div class="share-group">
    <label class="share-field">
      <span class="share-label">{m.shareform_expires()}</span>
      <select class="select share-select" bind:value={draft.until} disabled={busy}>
        {#each expiries as [value, says] (value)}<option {value}>{says}</option>{/each}
      </select>
    </label>
    {#if kind === "token"}
      {@render nameField()}
    {/if}
    {@render budgetField()}
  </div>
  <div class="share-form-actions">
    <button type="button" class="share-action" disabled={busy} onclick={oncancel}>{m.common_cancel()}</button>
    <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy}>{kind === "link" ? m.shareform_create_link() : m.shareform_create_token()}</button>
  </div>
</form>

<style>
  .share-form { display: grid; gap: calc(var(--spacing) * 4); padding-block: calc(var(--spacing) * 2); }
  .share-group { display: grid; gap: calc(var(--spacing) * 3); }
  .share-field { display: grid; gap: var(--spacing); }
  /* Field labels are the primary level; notes beneath them are the secondary. */
  .share-label { font-weight: var(--panel-title-weight); }
  .share-note { margin: 0; color: var(--panel-muted); font-size: 12px; }
  .share-text { margin: 0; }
  .share-choices { min-width: 0; margin: 0; padding: 0; border: 0; }
  .share-choice { display: flex; align-items: center; gap: calc(var(--spacing) * 1.5); }
  .share-choice:has(input:disabled) { color: var(--panel-muted); }
  .share-input, .share-select { width: 100%; min-width: 0; max-width: 100%; }
  .share-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); }
  .share-input::placeholder { color: var(--panel-muted); }
  .share-form-actions { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); }
  .share-action { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .share-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .share-action:disabled { opacity: .5; cursor: default; }
  /* Phones: fields keep 16px so iOS does not zoom, and the cancel action is a
     finger's height. */
  @media (max-width: 767px) {
    .share-input, .share-select { font-size: 1rem; }
    .share-action { min-height: 2.75rem; }
  }
</style>
