<script>
  // The bar every page of the static site wears: the mark, a way back to the
  // docs, and the button that signs you in. Not Nav.svelte - that bar asks
  // the application who is signed in and shows their account; this site has
  // no application behind it to ask, so the button always reads "Sign in" and
  // sends you to the application to find out.
  import Logo from "../components/Logo.svelte";
  import Icon from "../components/Icon.svelte";

  // Replaced at build time by vite.site.config.js: the published site points
  // at the deployment, a local build at whatever `make deploy` started.
  const app = __APP_ORIGIN__;
</script>

<nav class="site-bar">
  <div class="site-bar-inner">
    <a class="flex items-center gap-2" href="/" aria-label="LibrePaper home">
      <Logo />
      <span>LibrePaper</span>
    </a>
    <div class="flex items-center gap-4">
      <a class="lp-text-secondary lp-hover-brand" href="/start.html">Docs</a>
      <a class="lp-text-secondary lp-hover-brand" href={`${app}/try`}>Try</a>
      <a
        class="lp-text-secondary lp-hover-brand flex items-center"
        href="https://github.com/LibrePaper/librepaper"
        aria-label="LibrePaper on GitHub"
        title="GitHub"
      >
        <Icon name="github" size={18} />
      </a>
      <a
        class="btn btn-sm lp-control-brand"
        href={`${app}/auth/login`}
      >
        Sign in
      </a>
    </div>
  </div>
</nav>

<style>
  .site-bar {
    min-height: clamp(4rem, 1rem + 8vw, 4.5rem);
    padding-left: max(1.5rem, calc((100vw - 80rem) / 2 + 1.5rem));
    padding-right: max(1.5rem, calc((100vw - 80rem) / 2 + 1.5rem));
    border: none;
    background: transparent;
    display: flex;
    align-items: center;
  }

  .site-bar :global(a) {
    font-size: clamp(0.875rem, 1.5vw, 0.9375rem);
  }

  .site-bar :global(.btn) {
    border-radius: 4px;
  }

  .site-bar-inner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    max-width: 80rem;
    margin: 0 auto;
    gap: 2rem;
  }

  @media (max-width: 640px) {
    .site-bar {
      padding-left: 1rem;
      padding-right: 1rem;
    }
  }
</style>
