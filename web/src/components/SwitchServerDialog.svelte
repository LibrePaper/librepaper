<script>
  import { untrack } from "svelte";
  import Modal from "./Modal.svelte";
  import { DEFAULT_SERVER, listServers, switchServer } from "../lib/desktop/client.js";
  import { m } from "../lib/paraglide/messages.js";

  // Picks which LibrePaper server the desktop app opens. The list is asked of
  // the shell each time the dialog opens, so it is never stale. An address
  // typed in the field wins over the radio choice. Choosing the server already
  // open just closes the dialog. Switching signs nobody out: the shell confirms
  // natively and then navigates the window itself.
  let { open = $bindable(false) } = $props();
  // The Modal keeps its body in the page while closed, so two dialogs on one
  // page would share a form id and the action would submit the wrong one.
  const uid = $props.id();
  const formId = `switch-server-${uid}`;

  let loading = $state(true);
  let servers = $state([]);
  let current = $state(null);
  let choice = $state("");
  let typed = $state("");
  let busy = $state(false);
  let error = $state("");

  function hostOf(server) {
    try { return new URL(server).host; } catch { return server; }
  }

  function originOf(address) {
    try { return new URL(address).origin; } catch { return ""; }
  }

  async function load() {
    loading = true;
    error = "";
    typed = "";
    busy = false;
    try {
      const answer = await listServers();
      servers = answer.servers;
      current = answer.current;
      choice = answer.current ?? answer.servers[0] ?? DEFAULT_SERVER;
    } catch (failure) {
      error = failure.message;
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) untrack(load);
  });

  // A bad address is answered by the shell with a message, which is shown under
  // the field, and the dialog stays open for another try.
  async function submit(event) {
    event.preventDefault();
    const target = typed.trim() || choice;
    if (!target || busy) return;
    const origin = originOf(target);
    if (origin && origin === current) {
      open = false;
      return;
    }
    busy = true;
    error = "";
    try {
      await switchServer(target);
    } catch (failure) {
      error = failure.message;
      busy = false;
    }
  }
</script>

<Modal bind:open title={m.switchserverdialog_title()}
       confirm={{ form: formId, label: busy ? m.switchserverdialog_switching() : m.switchserverdialog_switch(), disabled: loading || busy }}>
  <form id={formId} data-switch-server class="flex flex-col gap-3" onsubmit={submit}>
    <fieldset class="flex flex-col gap-2">
      <legend class="lp-text-secondary text-sm">{m.switchserverdialog_servers()}</legend>
      {#if loading}
        <p class="lp-text-secondary text-sm">{m.switchserverdialog_loading()}</p>
      {:else}
        {#each servers as server (server)}
          <label class="flex flex-wrap items-center gap-2">
            <input type="radio" name="server" value={server} bind:group={choice} disabled={busy} />
            <span>{hostOf(server)}</span>
            {#if server === current}
              <span class="lp-text-secondary rounded-md border border-[var(--color-border)] px-1.5 text-xs">{m.switchserverdialog_current()}</span>
            {/if}
            {#if server === DEFAULT_SERVER}
              <span class="lp-text-secondary text-xs">{m.switchserverdialog_main()}</span>
            {/if}
          </label>
        {/each}
      {/if}
    </fieldset>
    <label class="label">
      <span class="label-text">{m.switchserverdialog_other()}</span>
      <input class="input" type="text" inputmode="url" autocomplete="off" spellcheck="false"
             placeholder={m.switchserverdialog_placeholder()} bind:value={typed} disabled={busy} />
    </label>
    {#if error}<p class="lp-text-error text-sm" role="alert">{error}</p>{/if}
    <p class="lp-text-secondary text-sm">{m.switchserverdialog_note()}</p>
  </form>
</Modal>
