<script>
  // The account itself, rather than a preference: the one irreversible thing
  // you can ask this deployment to do. Offered only to a signed-in browser,
  // because there is no account to erase otherwise.
  //
  // Erasure is deliberately slow to start and impossible to take back from
  // here: the server invalidates the session on the spot and refuses to sign
  // the account in again, so the typed confirmation below is the last moment
  // anybody can change their mind without asking the operator.
  import SettingRow from "./SettingRow.svelte";
  import { eraseAccount } from "../../lib/api.js";

  let { account = {} } = $props();

  // The handle the erasure confirmation is typed against: a GitHub login or a
  // Google email address, whichever this account signed in with.
  const handle = $derived(account.handle || account.name || "");

  // "asking" is the typed confirmation, "erasing" the request in flight,
  // "started" the answer. Only the first is reversible.
  let asking = $state(false);
  let erasing = $state(false);
  let started = $state(false);
  let typed = $state("");
  let error = $state("");

  // The handle, exactly, without the @ a GitHub account is displayed with:
  // this is a deliberate piece of friction, not a password.
  const confirmed = $derived(typed.trim().toLowerCase() === handle.toLowerCase() && handle !== "");

  function ask() {
    typed = "";
    error = "";
    asking = true;
  }

  async function erase() {
    if (!confirmed || erasing) return;
    erasing = true;
    error = "";
    try {
      await eraseAccount();
      asking = false;
      started = true;
    } catch (cause) {
      error = cause.message || "The account could not be erased.";
    } finally {
      erasing = false;
    }
  }
</script>

<div class="flex flex-col gap-6">
  <SettingRow id="account-erase" stacked title="Erase this account"
              description="Deletes the account and everything it owns. This cannot be undone from here.">
    {#if started}
      <div class="flex flex-col gap-2" role="status">
        <p>Erasure has started. You are signed out, and this account cannot sign in again.</p>
        <p class="text-surface-600-400 text-sm">Documents and their files are removed after a recovery window the operator sets; your comments elsewhere stay on the documents they were left on, relabelled “Deleted user” and no longer linked to you.</p>
        <a class="btn btn-sm preset-filled-primary-500 self-start" href="/">Leave</a>
      </div>
    {:else}
      <div class="flex flex-col gap-3">
        <ul class="account-consequences">
          <li>Every project you own is deleted, with its files, comments, labels and share links.</li>
          <li>Comments and suggestions you left on other people's documents stay there, relabelled “Deleted user” and no longer linked to this account.</li>
          <li>The account record — provider, handle, name and email — is deleted once its documents are gone.</li>
          <li>You are signed out immediately and cannot sign in again, so nothing here can call it off. Only the operator can, and only before the deletion runs.</li>
        </ul>
        {#if error}<p role="alert">{error}</p>{/if}
        {#if asking}
          <label class="label flex flex-col gap-1">
            <span class="label-text">Type <strong>{handle}</strong> to confirm</span>
            <input class="input input-sm" type="text" autocomplete="off" spellcheck="false"
                   aria-label="Type your handle to confirm erasure" bind:value={typed} />
          </label>
          <div class="flex gap-2">
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => (asking = false)} disabled={erasing}>Cancel</button>
            <button type="button" class="btn btn-sm preset-filled-error-500" onclick={erase} disabled={!confirmed || erasing}>
              {erasing ? "Erasing…" : "Erase account"}
            </button>
          </div>
        {:else}
          <button type="button" class="btn btn-sm preset-filled-error-500 self-start" onclick={ask} disabled={!handle}>Erase account…</button>
        {/if}
      </div>
    {/if}
  </SettingRow>
</div>

<style>
  .account-consequences { display: flex; flex-direction: column; gap: calc(var(--spacing) * 1); padding-left: calc(var(--spacing) * 4); list-style: disc; }
</style>
