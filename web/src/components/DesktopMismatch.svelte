<script>
  // A one-line banner for a page and a desktop app that cannot work together.
  // Inside the app only, and only when the two disagree. Nothing shows in a
  // browser or when they agree. SPEC-wire.md 5.3 and 5.4.
  import { desktop, negotiation, checkForUpdate, mismatchText, INSTALL_URL } from "../lib/desktop/client.js";
  import { m } from "../lib/paraglide/messages.js";

  // The desktop global is fixed for the whole page load, so the answer is too.
  const state = desktop() ? negotiation() : "ok";
  const text = mismatchText(state);

  let busy = $state(false);
  let usePackageManager = $state(false);

  async function update() {
    busy = true;
    try {
      const via = await checkForUpdate();
      // "updater" needs nothing more: the shell's own dialog takes over.
      if (via === "package_manager") usePackageManager = true;
      else if (via === "download") window.open(INSTALL_URL, "_blank", "noreferrer");
    } finally {
      busy = false;
    }
  }
</script>

{#if state !== "ok"}
  <div class="desktop-mismatch" role="status">
    <span class="desktop-mismatch-message">{text}</span>
    {#if state === "app_outdated"}
      {#if usePackageManager}
        <span class="desktop-mismatch-action">{m.desktopmismatch_use_package_manager()}</span>
      {:else}
        <button type="button" class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={update}>{m.desktopmismatch_update_now()}</button>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .desktop-mismatch {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    margin-block-end: calc(var(--spacing) * 2);
    font-size: var(--text-sm);
  }
  .desktop-mismatch-message,
  .desktop-mismatch-action {
    margin: 0;
    color: var(--color-text-secondary);
  }
</style>
