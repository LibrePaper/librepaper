<script>
  // The bar every page of the static site wears: the mark, a way back to the
  // docs, and the button that opens the application. Not Nav.svelte -- that
  // bar asks the application who is signed in, and this site has no
  // application behind it to ask.
  import Logo from "../components/Logo.svelte";
</script>

<nav class="flex items-center justify-between gap-4 px-4 py-3">
  <a class="flex items-center gap-2" href="/" aria-label="LibrePaper home">
    <Logo />
  </a>
  <div class="flex items-center gap-4">
    <a class="text-surface-600-400 hover:text-primary-500 text-sm" href="/start/index.html">Docs</a>
    <a
      class="btn btn-sm preset-filled-primary-500"
      href="https://app.librepaper.org/try"
      target="_blank"
      rel="noopener"
    >
      Try it now
    </a>
  </div>
</nav>
