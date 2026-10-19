<script>
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import Logo from "./Logo.svelte";
  import { me as whoami } from "../lib/api.js";
  import { INSTALL_URL, desktop, shellAction } from "../lib/desktop/client.js";

  // The page the server shows when there is more than one way in. It is served
  // for /auth/login, so the choice is made here and every other page can link
  // to the one address without knowing what this deployment configured.
  /** @type {{providers?: {id: string, kind: string, label: string}[]}} */
  let me = $state({});
  $effect(() => {
    whoami().then((who) => (me = who ?? {}));
  });

  // Carried through from the query rather than read off location.pathname:
  // this page is not where the person was going.
  const next = new URLSearchParams(location.search).get("next") || "/";
  const signedOut = new URLSearchParams(location.search).has("signed_out");
  const href = (provider) => `/auth/login/${provider.id}?next=${encodeURIComponent(next)}`;

  // Only the desktop app can switch the server it opens, so only it offers the choice.
  const inDesktop = Boolean(desktop());
  let changing = $state(false);
  let serverUrl = $state("");
  let serverError = $state("");
  let choosing = $state(false);
  async function chooseServer(event) {
    event.preventDefault();
    serverError = "";
    choosing = true;
    try { await shellAction("server", { url: serverUrl.trim() }); }
    catch (error) { serverError = error?.message || "The server was not changed."; }
    finally { choosing = false; }
  }
</script>

<Nav {me} signIn={false} />
<main id="main" tabindex="-1" class="signin-main">
  <section class="signin-form" aria-labelledby="signin-title">
    <div class="signin-head">
      <Logo size="signin" />
      <h1 id="signin-title">Sign in or create an account</h1>
    </div>
    {#if signedOut}<p class="signin-signed-out" role="status">You are signed out.</p>{/if}
    <!-- The service is young and may change under people, so a local copy kept
         by the desktop app is the safety net. -->
    <div class="signin-warning" role="note">
      <p><strong>LibrePaper is experimental.</strong> The website and service may change at any time.</p>
      <p>We recommend you use the <a href={INSTALL_URL}>desktop app</a> to make <a href="https://librepaper.org/backups.html">automatic local backups</a> of your projects as you edit them.</p>
    </div>
    <div class="signin-providers">
      {#each me.providers ?? [] as provider (provider.id)}
        <a class="signin-provider" href={href(provider)}>
          <span class="signin-provider-content">
            <span class="signin-provider-icon" aria-hidden="true">
              {#if provider.kind === "github"}
                <Icon name="github" size={20} />
              {:else if provider.kind === "google"}
                <img src="/assets/google-g.svg" width="20" height="20" alt="" />
              {:else if provider.kind === "orcid"}
                <img src="/assets/orcid-id.svg" width="20" height="20" alt="" />
              {:else}
                <Icon name="key" size={20} />
              {/if}
            </span>
            <span>Continue with {provider.label}</span>
          </span>
        </a>
      {/each}
    </div>
  </section>
  {#if inDesktop}
    <!-- The desktop app confirms the switch in its own native dialog, so the page asks nothing more. -->
    <footer class="signin-server">
      {#if changing}
        <form class="signin-server-form" onsubmit={chooseServer}>
          <label for="signin-server-url">You are signing in to {location.host}. Enter another server's address.</label>
          <div class="signin-server-row">
            <input id="signin-server-url" class="input input-sm" type="url" required placeholder="https://paper.example.org" bind:value={serverUrl} disabled={choosing} />
            <button class="btn btn-sm lp-control-brand" type="submit" disabled={choosing}>{choosing ? "Waiting..." : "Use this server"}</button>
          </div>
          <button class="signin-server-link" type="button" onclick={() => { changing = false; serverError = ""; }}>Cancel</button>
          {#if serverError}<p class="signin-server-error" role="alert">{serverError}</p>{/if}
        </form>
      {:else}
        <span class="signin-server-host">{location.host}</span>
        <button class="signin-server-link signin-server-change" type="button" onclick={() => (changing = true)}>Change server</button>
      {/if}
    </footer>
  {/if}
</main>

<style>
  .signin-main {
    min-height: calc(100dvh - var(--librepaper-bar));
    display: grid;
    grid-template-rows: 1fr auto;
    justify-items: center;
    padding: 2.5rem 1rem 1rem;
  }

  .signin-form {
    align-self: center;
    display: grid;
    gap: 1.5rem;
    width: min(100%, 26rem);
    color: var(--color-text);
  }

  .signin-head {
    display: grid;
    gap: 1.25rem;
  }

  h1 {
    margin: 0;
    color: var(--color-text);
    font-size: 1.5rem;
    font-weight: 650;
    line-height: 1.25;
    text-wrap: balance;
  }

  .signin-signed-out {
    margin: 0;
    color: var(--color-text-secondary);
  }

  .signin-providers {
    display: grid;
    gap: 0.75rem;
  }

  .signin-provider {
    display: flex;
    min-height: 3rem;
    align-items: center;
    justify-content: center;
    padding: 0.5rem 1rem;
    border: 1px solid var(--color-border);
    border-radius: 9px;
    background: var(--color-raised);
    color: var(--color-text);
    font-size: 0.9375rem;
    font-weight: 550;
    text-decoration: none;
    transition: background-color 120ms ease, border-color 120ms ease;
  }

  .signin-provider:hover {
    border-color: var(--color-border-strong);
    background: var(--color-row-hover);
  }

  .signin-provider:active {
    background: var(--color-row-selected);
  }

  .signin-provider:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
  }

  .signin-provider-content {
    display: inline-flex;
    width: min(100%, 12rem);
    align-items: center;
    gap: 0.625rem;
  }

  .signin-provider-icon {
    display: grid;
    width: 20px;
    height: 20px;
    flex: 0 0 20px;
    place-items: center;
  }

  .signin-provider-icon :global(svg) {
    width: 20px;
    height: 20px;
  }

  .signin-warning {
    padding: 0.875rem 1rem;
    border: 1px solid var(--color-warning-border);
    border-left: 4px solid var(--color-warning-solid);
    border-radius: 9px;
    background: var(--color-warning-bg);
    color: var(--color-warning-text);
    font-size: 0.9375rem;
    line-height: 1.5;
  }

  .signin-warning p {
    margin: 0;
  }

  .signin-warning p + p {
    margin-top: 0.5rem;
  }

  .signin-warning a {
    color: inherit;
    text-decoration: underline;
    font-weight: 600;
  }

  .signin-server {
    width: 100%;
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.25rem 1rem;
    padding-top: 2rem;
    font-size: 0.8125rem;
    color: var(--color-text-secondary);
  }

  .signin-server-form {
    display: grid;
    gap: 0.5rem;
    width: min(100%, 26rem);
    margin-inline: auto;
  }

  .signin-server-row {
    display: flex;
    gap: 0.5rem;
  }

  .signin-server-row input {
    flex: 1;
    min-width: 0;
  }

  .signin-server-link {
    justify-self: start;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }

  .signin-server-link:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
  }

  .signin-server-error {
    margin: 0;
    color: var(--color-error-text);
  }

  @media (max-width: 36rem) {
    .signin-main {
      padding: 1.5rem 1rem 1rem;
    }
  }

  @media (max-height: 40rem) {
    .signin-form {
      align-self: start;
    }
  }
</style>
