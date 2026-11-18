<script>
  // The consent page for an app or agent that asks to work on one project. The
  // server has already sent a signed-out person through sign-in, and it names
  // the app, the project and the host the person will be sent back to.
  //
  // Nothing is approved by the page loading. The Approve button is what sends
  // the decision, with every parameter of the request, so a link cannot
  // approve a connection on somebody's behalf.
  import Nav from "./Nav.svelte";
  import Page from "./layout/Page.svelte";
  import Stack from "./layout/Stack.svelte";
  import { get, me as whoami, post } from "../lib/api.js";
  import { m } from "../lib/paraglide/messages.js";

  /** @type {{name?: string}} */
  let me = $state({});
  /** @type {null | {client_name: string, redirect_host: string, project: {slug: string, title: string}, owner: boolean, roles: string[], default_role: string, expires_days: number}} */
  let info = $state(null);
  let role = $state("suggest");
  let loadError = $state("");
  let error = $state("");
  let sending = $state(false);

  const roles = $derived([
    { id: "read", label: m.shared_role_reader(), help: m.connect_role_read_help() },
    { id: "comment", label: m.common_comment(), help: m.connect_role_comment_help() },
    { id: "suggest", label: m.common_suggest(), help: m.connect_role_suggest_help() },
    { id: "edit", label: m.common_edit(), help: m.connect_role_edit_help() },
  ]);

  // The server's own words when it gave some. A bare status code from the
  // shared helper is not a reason a person can read, so it gets the fallback.
  const reason = (problem, fallback) => {
    const text = String(problem?.message || "");
    return text && !/^\d+$/.test(text) ? text : fallback;
  };

  $effect(() => {
    whoami().then((who) => (me = who ?? {}));
    get(`/api/oauth/authorize-info${location.search}`)
      .then((answer) => {
        info = answer;
        role = answer.default_role || "suggest";
      })
      .catch((problem) => (loadError = reason(problem, m.connect_load_failed())));
  });

  // Every parameter of the request goes back with the decision, so the server
  // checks the same client, redirect, challenge and resource it showed.
  async function decide(decision) {
    sending = true;
    error = "";
    try {
      const request = Object.fromEntries(new URLSearchParams(location.search));
      const reply = await post("/api/oauth/approve", { ...request, role, decision });
      // Left sending on purpose: the page is leaving for the app.
      location.assign(reply.redirect);
    } catch (problem) {
      error = reason(problem, m.connect_approve_failed());
      sending = false;
    }
  }
</script>

<Nav {me} />
<Page>
  <Stack gap={4}>
    {#if loadError}
      <p class="lp-text-error" role="alert">{loadError}</p>
    {:else if info}
      <h1 class="h2">{m.connect_heading({ client: info.client_name, project: info.project.title })}</h1>
      <p class="lp-text-secondary">{m.connect_redirect({ host: info.redirect_host })}</p>
      {#if info.owner}
        <fieldset class="connect-roles" disabled={sending}>
          <legend class="connect-legend">{m.agent_access_label()}</legend>
          {#each roles as item (item.id)}
            <label class="connect-role">
              <input type="radio" name="role" value={item.id} bind:group={role} />
              <span class="connect-role-text">
                <span class="connect-role-name">{item.label}</span>
                <span class="lp-text-secondary">{item.help}</span>
              </span>
            </label>
          {/each}
        </fieldset>
        <p class="lp-text-secondary">{m.connect_expiry()}</p>
        {#if error}
          <p class="lp-text-error" role="alert">{error}</p>
        {/if}
        <div class="connect-actions">
          <button class="btn lp-control-brand" disabled={sending} onclick={() => decide("approve")}>{m.device_approve()}</button>
          <button class="btn lp-control-tonal-brand" disabled={sending} onclick={() => decide("deny")}>{m.common_cancel()}</button>
        </div>
      {:else}
        <p class="lp-text-secondary">{m.connect_owner_only()}</p>
      {/if}
    {/if}
  </Stack>
</Page>

<style>
  .connect-roles { display: grid; gap: calc(var(--spacing) * 2); min-width: 0; margin: 0; padding: 0; border: 0; }
  .connect-legend { padding: 0; margin-bottom: calc(var(--spacing) * 2); font-weight: 600; }
  .connect-role { display: flex; align-items: flex-start; gap: calc(var(--spacing) * 2); cursor: pointer; }
  .connect-role input { margin-top: 0.3em; flex: none; }
  .connect-role-text { display: grid; gap: 0; min-width: 0; }
  .connect-role-name { font-weight: 600; }
  .connect-actions { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }
  /* Phones: the choices and buttons are finger-sized. */
  @media (max-width: 767px) {
    .connect-role { min-height: 2.75rem; }
    .connect-actions .btn { min-height: 2.75rem; }
  }
</style>
