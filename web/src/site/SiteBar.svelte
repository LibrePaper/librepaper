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
    <a class="text-surface-600-400 hover:text-primary-500 text-sm" href="/start.html">Docs</a>
    <a
      class="text-surface-600-400 hover:text-primary-500 flex items-center"
      href="https://github.com/LibrePaper/librepaper"
      aria-label="LibrePaper on GitHub"
      title="GitHub"
    >
      <Icon name="github" size={18} />
    </a>
    <a
      class="btn btn-sm preset-filled-primary-500"
      href={`${app}/auth/login`}
    >
      Sign in
    </a>
  </div>
</nav>
