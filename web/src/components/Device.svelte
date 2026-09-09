<script>
  import Nav from "./Nav.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import { me as whoami, post } from "../lib/api.js";

  // Where a terminal's `librepaper login` sends the person. The server has
  // already sent anyone without a session through /auth/login, so by the time
  // this renders there is an account to name.
  //
  // Nothing here approves on its own. A link is something somebody else can
  // send you, and approving from the link alone would put your identity on
  // their terminal; the button, and the POST behind it, are what stop that.
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
      error = problem.message || "that did not work";
    } finally {
      sending = false;
    }
  }
</script>

<Nav {me} />
<Page>
  <Stack gap={4}>
    {#if done}
      <h1 class="h2">Approved</h1>
      <p class="text-surface-600-400">
        The terminal is signed in as {me.name}. You can close this page.
      </p>
    {:else}
      <h1 class="h2">Sign in the terminal as {me.name}?</h1>
      <p class="text-surface-600-400">
        A terminal asked to sign in with this code. Approve it only if the code below is the one it
        is showing you.
      </p>
      <p class="text-2xl font-mono tracking-widest">{code}</p>
      {#if error}
        <p class="text-error-500">{error}</p>
      {/if}
      <button
        class="btn preset-filled-primary-500 w-fit"
        disabled={sending || !code}
        onclick={approve}
      >
        Approve
      </button>
    {/if}
  </Stack>
</Page>
