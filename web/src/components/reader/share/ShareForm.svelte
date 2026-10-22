<script>
  import { onMount } from "svelte";
  import { EXPIRIES, rolesOf, fullDateOf } from "./shared.js";

  // The new and the edit form for a link or a token. Permission is chosen
  // when something is created and fixed afterwards.
  let { kind, mode, draft = $bindable(), item = null, busy = false, allowed = () => true, onsubmit, oncancel } = $props();
  const creating = $derived(mode === "create");
  const noun = $derived(kind === "link" ? "link" : "token");
  let first;
  onMount(() => first?.focus());
  const expiries = $derived(kind === "token" ? EXPIRIES.filter(([value]) => value !== "never") : EXPIRIES);
  const needsName = $derived(kind === "token");
  const valid = $derived(!needsName || draft.name.trim() !== "");
  const spends = $derived(kind === "token" || draft.role !== "reader");
</script>

<form class="share-form" onsubmit={(event) => { event.preventDefault(); if (valid && !busy) onsubmit(); }}>
  <label class="share-field panel-meta">{kind === "link" ? "Name (optional)" : "Agent name"}
    <input class="input share-input" type="text" maxlength="80" required={needsName} bind:this={first} bind:value={draft.name} disabled={busy} />
    <span>{kind === "link" ? "A private name to help you remember who you shared this link with." : "Its comments and suggestions appear under this name."}</span>
  </label>
  {#if creating}
    <label class="share-field panel-meta">Permission
      <select class="select share-select" bind:value={draft.role} disabled={busy} onchange={(event) => { if (kind === "token" && event.currentTarget.value === "editor" && draft.until === "7d") draft.until = "1d"; }}>
        {#each rolesOf(kind) as role (role.id)}
          <option value={role.id} disabled={!allowed(role.id)}>{role.label}{allowed(role.id) ? "" : " (not allowed here)"}</option>
        {/each}
      </select>
    </label>
    {#if kind === "token" && draft.role === "editor"}
      <p class="share-note panel-meta">Edit lets the agent change the project directly, without review.</p>
    {/if}
  {/if}
  <label class="share-field panel-meta">Expires
    <select class="select share-select" bind:value={draft.until} disabled={busy}>
      {#if !creating && item?.until}<option value="">Unchanged ({fullDateOf(item.until)})</option>{/if}
      {#each expiries as [value, says] (value)}<option {value}>{says}</option>{/each}
    </select>
  </label>
  {#if spends}
    <details class="share-advanced">
      <summary class="panel-meta">Advanced</summary>
      <label class="share-field panel-meta">Comments/hour
        <input class="input share-input" type="number" min="0" step="1" bind:value={draft.budget} disabled={busy} />
        <span>Leave empty for the deployment default.</span>
      </label>
    </details>
  {/if}
  <div class="share-form-actions">
    <button type="button" class="share-action" disabled={busy} onclick={oncancel}>Cancel</button>
    <button type="submit" class="btn btn-sm lp-control-brand" disabled={busy || !valid}>{creating ? `Create ${noun}` : "Save"}</button>
  </div>
</form>

<style>
  .share-form { display: grid; gap: calc(var(--spacing) * 2); padding-block: calc(var(--spacing) * 2); }
  .share-field { display: grid; gap: var(--spacing); }
  .share-field > span, .share-note { font-size: 11px; }
  .share-note { margin: 0; }
  .share-input, .share-select { width: 100%; min-width: 0; max-width: 100%; }
  .share-select { border: 0; background-color: var(--color-row-hover); font: inherit; border-radius: var(--radius-base); }
  .share-advanced summary { cursor: pointer; padding-block: var(--spacing); }
  .share-advanced .share-field { margin-top: var(--spacing); }
  .share-form-actions { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); }
  .share-action { padding: .15rem .35rem; border: 0; border-radius: .25rem; background: none; color: var(--color-link); font: inherit; cursor: pointer; }
  .share-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .share-action:disabled { opacity: .5; cursor: default; }
</style>
