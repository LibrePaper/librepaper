<script>
  import Nav from "./Nav.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import { me as whoami, post } from "../lib/api.js";
  import { m } from "../lib/paraglide/messages.js";

  // Where a terminal's `librepaper login` sends the person. The server has
  // already sent anyone without a session through /auth/login, so by the time
  // this renders there is an account to name.
  //
  // Nothing here approves on its own. A link is something somebody else can
  // send you, and approving from the link alone would put your identity on
  // their terminal; the button, and the POST behind it, are what stop that.
  /** @type {{name?: string}} */
  let me = $state({});
  let done = $state(false);
  let error = $state("");
  let sending = $state(false);

  $effect(() => {
    whoami().then((who) => (me = who ?? {}));
  });

  const code = (new URLSearchParams(location.search).get("code") || "")
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, "");

  async function approve() {
    sending = true;
    error = "";
    try {
      await post("/api/auth/device/approve", { user_code: code });
      done = true;
    } catch (problem) {
      error = problem.message || m.device_failed();
    } finally {
      sending = false;
    }
  }
</script>

<Nav {me} />
<Page>
  <Stack gap={4}>
    {#if done}
      <h1 class="h2">{m.device_approved_title()}</h1>
      <p class="lp-text-secondary">
        {m.device_approved_body({ name: me.name ?? "" })}
      </p>
    {:else}
      <h1 class="h2">{m.device_sign_in_title({ name: me.name ?? "" })}</h1>
      <p class="lp-text-secondary">
        {m.device_intro()}
      </p>
      <p class="text-2xl font-mono tracking-widest">{code}</p>
      {#if error}
        <p class="lp-text-error">{error}</p>
      {/if}
      <button
        class="btn lp-control-brand w-fit"
        disabled={sending || !code}
        onclick={approve}
      >
        {m.device_approve()}
      </button>
    {/if}
  </Stack>
</Page>
