<script>
  // The bar every page of the static site wears: the mark, a way back to the
  // docs, and the button into the application. Not Nav.svelte - that bar asks
  // the application who is signed in and shows their account; this site has
  // no application behind it to ask, so the button always reads "Start
  // writing" and sends you to the application to find out. New visitors look
  // for a way to sign up, and signing in is how an account is created, so the
  // button names what they came to do. Every page, the landing page included,
  // shows the mark.
  import Logo from "../components/Logo.svelte";
  import Icon from "../components/Icon.svelte";

  // Replaced at build time by vite.site.config.js: the published site points
  // at the deployment, a local build at whatever `make demo` started.
  const app = __APP_ORIGIN__;
</script>

<nav class="site-bar">
  <div class="site-bar-inner">
    <a class="flex items-center gap-2" href="/" aria-label="LibrePaper home">
      <Logo />
      <span class="max-sm:hidden">LibrePaper</span>
    </a>
    <div class="flex items-center gap-3 sm:gap-4 ml-auto">
      <a class="lp-text-secondary lp-hover-brand max-sm:hidden" href="/#features">Features</a>
      <a class="lp-text-secondary lp-hover-brand" href="/what.html">Documentation</a>
      <a
        class="lp-text-secondary lp-hover-brand flex items-center gap-1.5"
        href="https://github.com/LibrePaper/librepaper"
        aria-label="GitHub"
        title="GitHub"
      >
        <Icon name="github" size={16} />
        <span class="max-sm:hidden">GitHub</span>
      </a>
      <a class="btn btn-sm lp-control-brand" href={`${app}/auth/login`}>Start writing</a>
    </div>
  </div>
</nav>

<style>
  .site-bar {
    min-height: 3.5rem;
    padding-left: max(1.5rem, calc((100vw - 80rem) / 2 + 1.5rem));
    padding-right: max(1.5rem, calc((100vw - 80rem) / 2 + 1.5rem));
    border: none;
    background: transparent;
    display: flex;
    align-items: center;
  }

  /* One size for everything in the bar, the wordmark and the button
     included: Skeleton sizes .btn-sm on its own scale otherwise. */
  .site-bar,
  .site-bar :global(a),
  .site-bar :global(.btn) {
    font-size: 0.875rem;
    line-height: 1.25rem;
  }

  .site-bar :global(.btn) {
    border-radius: 4px;
    padding: 0.25rem 0.75rem;
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
