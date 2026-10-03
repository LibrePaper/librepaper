<script>
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import { me as whoami } from "../lib/api.js";

  // The page the server shows when there is more than one way in. It is served
  // for /auth/login, so the choice is made here and every other page can link
  // to the one address without knowing what this deployment configured.
  let me = $state({});
  $effect(() => {
    whoami().then((who) => (me = who ?? {}));
  });

  // Carried through from the query rather than read off location.pathname:
  // this page is not where the person was going.
  const next = new URLSearchParams(location.search).get("next") || "/";
  const href = (provider) => `/auth/login/${provider}?next=${encodeURIComponent(next)}`;
  const providers = ["github", "google"];
</script>

<Nav {me} />
<main id="main" tabindex="-1" class="signin-main">
  <section class="signin-card" aria-labelledby="signin-title">
    <h1 id="signin-title">Sign in</h1>
    <p class="signin-description">
      Sign in to read, comment, edit, and create documents and projects.
    </p>
    <!-- The service is young and may change; a local copy via the companion app is
         the safest way to preserve your work. -->
    <div class="signin-warning" role="note">
      <p><strong>LibrePaper is experimental.</strong> The website and service may change at any time.</p>
      <p>We recommend you use the <a href="/docs/install.html">companion app</a> to make automatic local backups of your projects as you edit them.</p>
    </div>
    <div class="signin-providers">
      {#each providers.filter((provider) => (me.providers ?? []).includes(provider)) as provider (provider)}
        <a class="signin-provider" href={href(provider)}>
          <span class="signin-provider-content">
            <span class="signin-provider-icon" aria-hidden="true">
              {#if provider === "github"}
                <Icon name="github" size={20} />
              {:else}
                <img src="/assets/google-g.svg" width="20" height="20" alt="" />
              {/if}
            </span>
            <span>Continue with {provider === "github" ? "GitHub" : "Google"}</span>
          </span>
        </a>
      {/each}
    </div>
  </section>
</main>

<style>
  .signin-main {
    min-height: calc(100dvh - var(--librepaper-bar));
    display: grid;
    place-items: center;
    padding: 2.5rem 1rem 5rem;
  }

  .signin-card {
    width: min(100%, 30rem);
    padding: 2.75rem;
    border: 1px solid var(--color-divider);
    border-radius: 12px;
    background: var(--color-raised);
    color: var(--color-text);
  }

  h1 {
    margin: 0;
    color: var(--color-text);
    font-size: 2rem;
    font-weight: 650;
    line-height: 1.2;
  }

  .signin-description {
    margin: 0.875rem 0 0;
    color: var(--color-text-secondary);
    font-size: 1rem;
    line-height: 1.5;
  }

  .signin-providers {
    display: grid;
    gap: 0.75rem;
    margin-top: 1.875rem;
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
    outline: 2px solid var(--color-brand);
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
    margin-top: 1.5rem;
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

  @media (max-width: 36rem) {
    .signin-main {
      padding: 1.5rem 1rem 3rem;
    }

    .signin-card {
      padding: 1.5rem;
    }
  }

  @media (max-height: 40rem) {
    .signin-main {
      place-items: start center;
    }
  }
</style>
