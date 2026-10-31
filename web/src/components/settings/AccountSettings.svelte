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
  import { m } from "../../lib/paraglide/messages.js";

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
      error = cause.message || m.accountsettings_erase_failed();
    } finally {
      erasing = false;
    }
  }
</script>

<div class="flex flex-col gap-6">
  <SettingRow id="account-erase" stacked title={m.common_erase_this_account()}
              description={m.accountsettings_erase_description()}>
    {#if started}
      <div class="flex flex-col gap-2" role="status">
        <p>{m.accountsettings_started()}</p>
        <p class="lp-text-secondary text-sm">{m.accountsettings_started_detail()}</p>
        <a class="btn btn-sm lp-control-brand self-start" href="/">{m.accountsettings_leave()}</a>
      </div>
    {:else}
      <div class="flex flex-col gap-3">
        <ul class="account-consequences">
          <li>{m.accountsettings_consequence_projects()}</li>
          <li>{m.accountsettings_consequence_comments()}</li>
          <li>{m.accountsettings_consequence_record()}</li>
          <li>{m.accountsettings_consequence_signout()}</li>
        </ul>
        {#if error}<p role="alert">{error}</p>{/if}
        {#if asking}
          <label class="label flex flex-col gap-1">
            <span class="label-text">{m.accountsettings_type_before()} <strong>{handle}</strong> {m.accountsettings_type_after()}</span>
            <input class="input input-sm" type="text" autocomplete="off" spellcheck="false"
                   aria-label={m.accountsettings_handle_aria()} bind:value={typed} />
          </label>
          <div class="flex gap-2">
            <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => (asking = false)} disabled={erasing}>{m.common_cancel()}</button>
            <button type="button" class="btn btn-sm lp-control-danger" onclick={erase} disabled={!confirmed || erasing}>
              {erasing ? m.accountsettings_erasing() : m.accountsettings_erase_button()}
            </button>
          </div>
        {:else}
          <button type="button" class="btn btn-sm lp-control-danger self-start" onclick={ask} disabled={!handle}>{m.accountsettings_erase_open()}</button>
        {/if}
      </div>
    {/if}
  </SettingRow>
</div>

<style>
  .account-consequences { display: flex; flex-direction: column; gap: calc(var(--spacing) * 1); padding-left: calc(var(--spacing) * 4); list-style: disc; }
</style>
