<script>
  // The bar every page of the static site wears: the mark, a way back to the
  // docs, and the button that signs you in. Not Nav.svelte -- that bar asks
  // the application who is signed in and shows their account; this site has
  // no application behind it to ask, so the button always reads "Sign in" and
  // sends you to the application to find out.
  import Logo from "../components/Logo.svelte";
  import Icon from "../components/Icon.svelte";

  // Replaced at build time by vite.site.config.js: the published site points
  // at the deployment, a local build at whatever `make deploy` started.
  const app = __APP_ORIGIN__;
</script>

<nav class="flex items-center justify-between gap-4 px-4 py-3">
  <a class="flex items-center gap-2" href="/" aria-label="LibrePaper home">
    <Logo />
    <span>LibrePaper</span>
  </a>
  <div class="flex items-center gap-4">
    <a class="lp-text-secondary lp-hover-brand text-sm" href="/start.html">Docs</a>
    <a class="lp-text-secondary lp-hover-brand text-sm" href={`${app}/try`}>Try</a>
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
</nav>
